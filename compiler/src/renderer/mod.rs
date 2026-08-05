pub mod escape;
pub mod filters;
pub(crate) mod js_slot;
pub mod sanitize;
pub mod scope;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use crate::ast::*;
use crate::error::{CompileError, ErrorAccumulator, ErrorCode};
use crate::renderer::escape::{
    escape_attr, escape_css_value, escape_html, escape_js_string, escape_script_safe,
};
use crate::renderer::filters::{apply_filter, apply_loop_filter, value_to_string};
use crate::renderer::js_slot::{js_filter_outside_string_literal, slot_quote_contexts};
use crate::renderer::sanitize::sanitize_html;
use crate::renderer::scope::Scope;

const MAX_ITERATIONS: usize = 1000;

/// A JSON number as a non-negative integer. `Value::as_u64()` alone is not enough:
/// a number that came from a template literal (`width={400}`) is FLOAT-typed, and
/// `as_u64()` returns None for it even when the value is integral.
fn json_uint(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| {
            let f = n.as_f64()?;
            (f >= 0.0 && f.is_finite()).then_some(f as u64)
        }),
        _ => None,
    }
}

/// Is this subscript expression already a literal `Scope::resolve_path` can resolve
/// on its own? Mirrors `scope::index_into`, which accepts exactly these two forms.
fn is_const_index(e: &Expression) -> bool {
    e.filters.is_empty()
        && matches!(
            e.kind,
            ExprKind::NumberLiteral(_) | ExprKind::StringLiteral(_)
        )
}

/// SECURITY: a hard cap on render RECURSION depth. The renderer recurses through
/// `render_nodes` for nested elements, component includes, and layout includes.
/// The parser caps a single file's nesting, but nothing else bounds a chain of
/// includes (A includes B includes C ...), and the renderer's stack frames are
/// large enough that even the parser's single-file cap can approach a 2 MB
/// worker-thread stack. This cap bounds total render recursion independently of
/// source, well under the stack a tokio SSR worker runs on; past it, a subtree
/// renders as empty and a diagnostic is recorded once.
const MAX_RENDER_DEPTH: usize = 96;

/// SECURITY: a GLOBAL per-render budget on the total number of loop iterations
/// across ALL `{each}`/`{repeat}` constructs. The per-construct `MAX_ITERATIONS`
/// cap alone does not stop multiplicative blowup - N nested `{repeat 1000}`
/// blocks would still emit 1000^N bodies and pin a render thread. Once the
/// budget is spent, loops stop iterating (and emit nothing further), bounding a
/// single render to ~1M iterations regardless of nesting.
const MAX_TOTAL_ITERATIONS: usize = 1_000_000;

/// SECURITY (memory): the largest a SINGLE intermediate filter result may be.
///
/// THE CANONICAL STATEMENT OF WHY THE FILTER BUDGETS EXIST; the other two point here.
///
/// Several filters expand their input - `json` roughly doubles a string (it re-quotes and
/// re-escapes something already quoted and escaped) and HTML escaping can grow it 6x - so an
/// unbounded chain of them is exponential. A chain of about 50 turns a 6-byte value into tens of
/// gigabytes, enough to OOM-kill a shared render service. Nothing else catches it: `MAX_OUTPUT_
/// BYTES` bounds what is EMITTED and `MAX_TOTAL_ITERATIONS` bounds how often a loop runs, but a
/// filter chain emits nothing until it finishes, so neither ever sees an intermediate.
///
/// A PER-VALUE cap is what makes 2^N unreachable, and that is why it is per-value rather than
/// cumulative: growth is multiplicative, so the first intermediate to cross this ends the chain
/// before the next doubling. The largest legitimate single value is a `json` of a full data
/// collection, far under this.
const MAX_FILTER_VALUE_BYTES: usize = 8 * 1024 * 1024;

/// SECURITY (memory): cumulative intermediate filter bytes for one render.
///
/// [`MAX_FILTER_VALUE_BYTES`] alone leaves the other shape open: many chains, each producing
/// values just under the per-value cap. This bounds the total.
const MAX_FILTER_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// SECURITY: a GLOBAL per-render budget on the total number of OUTPUT bytes.
/// The iteration budget bounds loop COUNT, not loop BODY size - a large static
/// body repeated within the iteration budget (or duplicated via slots) could
/// still build a multi-GB string. Once the budget is spent, rendering stops
/// emitting and a diagnostic is recorded, mirroring the iteration budget.
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// The per-render MEMORY budget, in BYTES, that a host may tune.
///
/// These three caps are what stop a crafted template from turning a small input into an unbounded
/// allocation; see [`MAX_FILTER_VALUE_BYTES`] for the shape they defend against.
///
/// WHAT LEGITIMATE WORK COSTS, so the defaults can be judged against something: a full multi-page
/// theme build (102 files, every page) peaks at 12 MB of live memory, a single-page theme at
/// 10 MB, and the worst seed in the adversarial fuzz corpus at 20 MB. Real work is TENS of
/// megabytes, so the defaults leave a wide margin above it.
///
/// Tune it to the host, not to the template. A shared multi-tenant renderer on a small box wants
/// these LOWER than the defaults, because the cost of a refused render is one error page while the
/// cost of an unbounded one is the whole process; a CLI building a large site on a workstation can
/// raise them. Crossing a cap is always a typed diagnostic
/// (`ErrorCode::IterationLimitExceeded`) and never a panic: the per-value cap ends that filter
/// chain, and the two cumulative caps stop the render from producing more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    /// The largest a SINGLE intermediate filter result may be. Default 8 MiB.
    pub filter_value_bytes: usize,
    /// Cumulative intermediate filter bytes across the whole render. Default 64 MiB.
    pub filter_total_bytes: usize,
    /// Cumulative EMITTED output bytes for the whole render. Default 16 MiB.
    pub output_bytes: usize,
}

impl MemoryBudget {
    /// The compiled-in defaults: 8 MiB per filter value, 64 MiB of filter work, 16 MiB of output.
    /// Every entry point that does not take a budget uses exactly this, so adding one to a call
    /// site can never change behaviour by accident.
    pub const DEFAULT: Self = Self {
        filter_value_bytes: MAX_FILTER_VALUE_BYTES,
        filter_total_bytes: MAX_FILTER_TOTAL_BYTES,
        output_bytes: MAX_OUTPUT_BYTES,
    };

    /// Every cap scaled by `factor`, clamped to at least one byte so a budget is never zero
    /// (a zero cap would refuse the first byte of every render, which reads as "the compiler is
    /// broken" rather than "the budget is too small").
    #[must_use]
    pub fn scaled(factor: f64) -> Self {
        let scale = |n: usize| ((n as f64) * factor).max(1.0) as usize;
        Self {
            filter_value_bytes: scale(MAX_FILTER_VALUE_BYTES),
            filter_total_bytes: scale(MAX_FILTER_TOTAL_BYTES),
            output_bytes: scale(MAX_OUTPUT_BYTES),
        }
    }
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Everything a render needs beyond the document and its context: which build mode, which files
/// are resolvable, and how much memory it may spend.
///
/// This exists so the memory budget is reachable without a new positional argument on the four
/// pre-existing render entry points, all of which are called from outside this crate. They all
/// delegate here with [`MemoryBudget::DEFAULT`].
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub mode: BuildMode,
    /// Registry of resolvable `@components/...` / `@layouts/...` paths to source.
    pub file_registry: Arc<HashMap<String, String>>,
    pub budget: MemoryBudget,
}

impl RenderOptions {
    #[must_use]
    pub fn new(mode: BuildMode) -> Self {
        Self {
            mode,
            file_registry: Arc::new(HashMap::new()),
            budget: MemoryBudget::DEFAULT,
        }
    }

    #[must_use]
    pub fn with_registry(mut self, file_registry: Arc<HashMap<String, String>>) -> Self {
        self.file_registry = file_registry;
        self
    }

    #[must_use]
    pub fn with_budget(mut self, budget: MemoryBudget) -> Self {
        self.budget = budget;
        self
    }
}

/// Reserved context key the SSR host uses to inject the pre-rendered HTML of a
/// page's ordered section list; `{sections}` outputs it raw. The host composes
/// it by rendering each section template (see `crate::sections::render_section`)
/// in order and concatenating - it is host-produced HTML, never user data.
pub const SECTIONS_HTML_KEY: &str = "__slurp_sections_html";

/// Reserved context key for a NAMED global section group (`{sections "header"}`), produced the same
/// way as `SECTIONS_HTML_KEY` but per group so a layout can host header/footer chrome. Unlike the
/// page key, group keys are NOT stripped when entering a layout/component scope (see
/// `scoped_context`), so the layout that composes the chrome can read them.
pub fn section_group_html_key(name: &str) -> String {
    format!("__slurp_section_group_{name}_html")
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BuildMode {
    Development,
    Production,
}

pub struct Renderer {
    file: String,
    mode: BuildMode,
    pub(crate) diagnostics: ErrorAccumulator,
    file_registry: Arc<HashMap<String, String>>,
    import_map: HashMap<String, String>,
    slot_content: Option<String>,
    head_content: Option<String>,
    rendering_stack: HashSet<String>,
    /// The top-level render context (site, theme, params, ...). Components and
    /// layouts render in their own root scope (props only), but fall back to these
    /// globals for anything not passed explicitly - explicit props always shadow.
    globals: serde_json::Map<String, Value>,
    /// SECURITY: running total of loop iterations this render, checked against
    /// `MAX_TOTAL_ITERATIONS` to bound nested-loop multiplicative blowup.
    total_iterations: usize,
    /// SECURITY: running total of emitted output bytes, checked against
    /// `MAX_OUTPUT_BYTES` to bound render memory.
    output_bytes: usize,
    /// Set once `output_bytes` crosses the budget; rendering stops emitting.
    output_exceeded: bool,
    /// Cumulative intermediate filter bytes. See `MemoryBudget::filter_total_bytes`.
    filter_bytes: usize,
    filter_budget_exceeded: bool,
    /// SECURITY (memory): the per-render allocation caps this render is held to.
    /// Host-configurable; see [`MemoryBudget`].
    budget: MemoryBudget,
    /// SECURITY: current render recursion depth, guarded against
    /// `MAX_RENDER_DEPTH` in `render_nodes`.
    render_depth: usize,
    /// Set once the render-depth cap is hit so the diagnostic is emitted once.
    render_depth_exceeded: bool,
    /// Per-render cache of parsed component/layout ASTs, keyed by registry path, so a component used
    /// repeatedly (e.g. inside a loop) is lexed + parsed ONCE, not on every instantiation.
    parse_cache: HashMap<String, Arc<Document>>,
    /// SECURITY: how many `<script>` elements enclose the node being rendered.
    /// A `<script>` body is a RAW TEXT element: the HTML parser does not decode
    /// character references inside it, so entity escaping is completely inert
    /// there. Interpolation stays alive in that context (the lexer keeps it on
    /// purpose), so the renderer has to know it is inside one - see
    /// `render_script_expression`.
    script_depth: usize,
}

impl Renderer {
    pub fn new(file: impl Into<String>, mode: BuildMode) -> Self {
        Self::with_budget(file, mode, MemoryBudget::DEFAULT)
    }

    /// A renderer held to a host-chosen memory budget. [`Renderer::new`] is this with
    /// [`MemoryBudget::DEFAULT`].
    pub fn with_budget(file: impl Into<String>, mode: BuildMode, budget: MemoryBudget) -> Self {
        Self {
            budget,
            file: file.into(),
            mode,
            diagnostics: ErrorAccumulator::new(),
            file_registry: Arc::new(HashMap::new()),
            import_map: HashMap::new(),
            slot_content: None,
            head_content: None,
            rendering_stack: HashSet::new(),
            globals: serde_json::Map::new(),
            total_iterations: 0,
            output_bytes: 0,
            output_exceeded: false,
            filter_bytes: 0,
            filter_budget_exceeded: false,
            render_depth: 0,
            render_depth_exceeded: false,
            parse_cache: HashMap::new(),
            script_depth: 0,
        }
    }

    /// Parse a registry file once per render, memoizing the resulting AST by path. Returns `None` if
    /// the path isn't in the registry or fails to parse (the caller then takes its fallback path).
    fn parse_cached(&mut self, path: &str) -> Option<Arc<Document>> {
        if let Some(doc) = self.parse_cache.get(path) {
            return Some(doc.clone());
        }
        let source = self.file_registry.get(path)?.clone();
        match crate::parser::parse(&source, path) {
            Ok((doc, _)) => {
                let doc = Arc::new(doc);
                self.parse_cache.insert(path.to_string(), doc.clone());
                Some(doc)
            }
            Err(_) => None,
        }
    }

    pub fn render(&mut self, doc: &Document, context: &Value) -> String {
        self.import_map = build_import_map(doc.frontmatter.as_ref());
        if let Value::Object(map) = context {
            self.globals = map.clone();
        }
        let scope = Scope::root(context);
        self.render_nodes(&doc.children, &scope)
    }

    /// Root context for a component or layout: the render-level globals overlaid by the
    /// values passed explicitly (props win on key collision).
    fn scoped_context(&self, own: serde_json::Map<String, Value>) -> Value {
        let mut merged = self.globals.clone();
        // The host-injected section HTML belongs to the page's own {sections} node; a
        // stray {sections} inside a component or layout must not duplicate it.
        merged.remove(SECTIONS_HTML_KEY);
        for (k, v) in own {
            merged.insert(k, v);
        }
        Value::Object(merged)
    }

    fn render_nodes(&mut self, nodes: &[Node], scope: &Scope<'_>) -> String {
        // SECURITY: bound render recursion depth (nested elements + include
        // chains) so a crafted theme cannot overflow the worker-thread stack.
        if self.render_depth >= MAX_RENDER_DEPTH {
            if !self.render_depth_exceeded {
                self.render_depth_exceeded = true;
                self.diagnostics.push(CompileError::warn(
                    &self.file,
                    0,
                    0,
                    format!("Maximum render depth ({MAX_RENDER_DEPTH}) exceeded; subtree omitted"),
                    ErrorCode::IterationLimitExceeded,
                ));
            }
            return String::new();
        }
        self.render_depth += 1;
        // Element rendering has large stack frames; grow the stack on native targets
        // so MAX_RENDER_DEPTH bounds recursion without risking the worker stack.
        let out = crate::stack::with_stack(|| {
            let mut out = String::with_capacity(nodes.len() * 32);
            for node in nodes {
                if self.output_exceeded {
                    break;
                }
                out.push_str(&self.render_node(node, scope));
            }
            out
        });
        self.render_depth -= 1;
        out
    }

    /// SECURITY (memory): charge one INTERMEDIATE filter result.
    ///
    /// Returns `false` when the value is too large on its own, or when the running total of
    /// intermediate bytes for this render is spent. See the call site in `eval_expr` for why the
    /// existing output and iteration budgets cannot cover this.
    ///
    /// Only string-shaped values are measured. A filter that returns an array or object has not
    /// serialized anything yet, and `Value::Array` length is already bounded by `MAX_ITERATIONS`
    /// wherever it is looped over.
    fn charge_filter_value(&mut self, v: &Value, filter: &Filter) -> bool {
        let bytes = match v {
            Value::String(s) => s.len(),
            _ => return true,
        };
        let per_value = self.budget.filter_value_bytes;
        if bytes > per_value {
            self.diagnostics.push(CompileError::render(
                &self.file,
                filter.line,
                filter.column,
                format!(
                    "Filter '{}' produced {bytes} bytes, over the per-value limit of \
                     {per_value}; expanding filters must not be chained without bound",
                    filter.name
                ),
                ErrorCode::IterationLimitExceeded,
            ));
            return false;
        }
        self.filter_bytes = self.filter_bytes.saturating_add(bytes);
        let total = self.budget.filter_total_bytes;
        if self.filter_bytes > total {
            if !self.filter_budget_exceeded {
                self.filter_budget_exceeded = true;
                self.diagnostics.push(CompileError::render(
                    &self.file,
                    filter.line,
                    filter.column,
                    format!("Filter work budget ({total} bytes) exceeded; rendering stopped"),
                    ErrorCode::IterationLimitExceeded,
                ));
            }
            return false;
        }
        true
    }

    /// SECURITY: charge the global per-render output-byte budget. Returns
    /// `false` once `MAX_OUTPUT_BYTES` is reached (a diagnostic is emitted
    /// exactly once); callers must then emit nothing.
    fn charge_output(&mut self, bytes: usize) -> bool {
        if self.output_exceeded {
            return false;
        }
        self.output_bytes = self.output_bytes.saturating_add(bytes);
        let cap = self.budget.output_bytes;
        if self.output_bytes > cap {
            self.output_exceeded = true;
            self.diagnostics.push(CompileError::warn(
                &self.file,
                0,
                0,
                format!("Render output budget ({cap} bytes) exceeded; output truncated"),
                ErrorCode::IterationLimitExceeded,
            ));
            return false;
        }
        true
    }

    /// Charge `s` against the output budget; the string passes through intact
    /// while the budget holds and collapses to empty once it is spent.
    fn emit(&mut self, s: String) -> String {
        if self.charge_output(s.len()) {
            s
        } else {
            String::new()
        }
    }

    fn render_node(&mut self, node: &Node, scope: &Scope<'_>) -> String {
        match node {
            Node::Text(t) => {
                if self.charge_output(t.len()) {
                    t.clone()
                } else {
                    String::new()
                }
            }
            Node::Comment(_) => String::new(),
            Node::Expression(expr) => {
                if self.script_depth > 0 {
                    return self.render_script_expression(expr, scope);
                }
                let val = self.eval_expr(expr, scope);
                let s = escape_html(&value_to_string(&val));
                self.emit(s)
            }
            Node::RawHtml { expr, sanitize } => {
                let val = self.eval_expr(expr, scope);
                let html = value_to_string(&val);
                let html = if *sanitize {
                    sanitize_html(&html)
                } else {
                    html
                };
                self.emit(html)
            }
            Node::If(n) => self.render_if(n, scope),
            Node::Each(n) => self.render_each(n, scope),
            Node::Match(n) => self.render_match(n, scope),
            Node::Repeat(n) => self.render_repeat(n, scope),
            Node::Fetch(n) => self.render_fetch(n, scope),
            Node::Component(n) => self.render_component(n, scope),
            Node::Element(n) => self.render_element(n, scope),
            Node::Layout(n) => self.render_layout(n, scope),
            Node::Slot(n) => self.render_slot(n, scope),
            Node::ClientLet { name, value } => {
                let val = self.eval_expr(value, scope);
                let json_val = serde_json::to_string(&val).unwrap_or_default();
                // SECURITY: escape `<` (kills `</script>` breakout AND `<!--`
                // comment-state confusion) and the JS line separators
                // U+2028/U+2029. Still valid JSON (legal string escapes).
                let safe_json = escape::escape_script_safe(&json_val);
                let s = format!(
                    r#"<script type="application/json" data-slurp-let="{}">{}</script>"#,
                    escape_attr(name),
                    safe_json,
                );
                self.emit(s)
            }
            Node::Debug(expr) => {
                if self.mode == BuildMode::Development {
                    let val = self.eval_expr(expr, scope);
                    let json = serde_json::to_string_pretty(&val).unwrap_or_default();
                    let s = format!("<pre data-slurp-debug>{}</pre>", escape_html(&json));
                    self.emit(s)
                } else {
                    String::new()
                }
            }
            Node::Try(n) => self.render_try(n, scope),
            Node::Redirect(_) => {
                // Redirect nodes are middleware-only; validated by security_check before rendering.
                String::new()
            }
            Node::Next => String::new(),
            Node::Head(nodes) => self.render_nodes(nodes, scope),
            Node::Sections(None) => match scope.get(SECTIONS_HTML_KEY) {
                Value::String(s) => self.emit(s),
                _ => String::new(),
            },
            Node::Sections(Some(name)) => match scope.get(section_group_html_key(name).as_str()) {
                Value::String(s) => self.emit(s),
                _ => String::new(),
            },
            Node::Blocks => self.render_block_children(scope),
        }
    }

    /// SECURITY: render a `${ ... }` that sits inside a `<script>` BODY.
    ///
    /// The ordinary text path entity-escapes, and inside a raw-text element that
    /// does nothing at all: the HTML parser hands a `<script>` body to the JS
    /// engine WITHOUT decoding character references, so `&#39;` stays the six
    /// characters `&#39;` and every JS metacharacter that is not `<` - backtick,
    /// `$`, `\`, `;`, `(`, `)`, `/`, newline - reaches the engine untouched.
    /// Escaping `<` blocks `</script>` breakout and that was the ONLY protection
    /// the old path provided. `<script>var n = ${count};</script>` with
    /// `count = "1;alert(1)"` rendered `var n = 1;alert(1);`.
    ///
    /// The fix is deliberately NOT an automatic escaper. `escape_js_string` is
    /// built for a value sitting INSIDE a JS string literal; applying it to
    /// `var n = ${count}` would leave `1;alert(1)` completely intact while
    /// looking like a fix, which is worse than the honest failure. So the author
    /// must say which context the value lands in:
    ///   - `| js`   - inside a quoted JS string literal (the escaper's contract).
    ///   - `| json` - a bare JSON value (object / array / number).
    ///   - `{html expr}` - "emit exactly these bytes", the pre-existing raw hatch.
    ///
    /// Anything else is a COMPILE ERROR (`lib.rs`'s security walk). This renderer
    /// check is the second half of the same rule, for the paths that render
    /// without going through the security gate: it emits NOTHING and records an
    /// error rather than falling back to an escaper that does not fit.
    ///
    /// A filtered value is emitted VERBATIM - exempting it from the entity pass is
    /// what makes `| js` usable at all. Before this, `var a = "${ v | js }"`
    /// rendered a literal `&#39;` inside the JS string, so there was no working
    /// way to interpolate into a script and the rule above would have been
    /// unsatisfiable. The exemption cannot leak into HTML contexts: it is reached
    /// only while `script_depth > 0`, and everywhere else `| js` output is still
    /// entity-escaped exactly as before.
    fn render_script_expression(&mut self, expr: &Expression, scope: &Scope<'_>) -> String {
        if !expr_has_script_filter(expr) {
            self.diagnostics.push(CompileError::render(
                &self.file,
                expr.line,
                expr.column,
                SCRIPT_FILTER_REQUIRED,
                ErrorCode::UnsafeScriptInterpolation,
            ));
            return String::new();
        }
        let val = self.eval_expr(expr, scope);
        let s = value_to_string(&val);
        self.emit(s)
    }

    /// `{blocks}` - render the child blocks of the nearest enclosing section or block, each through
    /// its own `blocks/<type>.slurp` file.
    ///
    /// The alternative is a section rendering its children with its OWN markup switching on
    /// `child.type`, which puts a block's appearance in every section that hosts it. Rendering the
    /// block's own
    /// file is what makes a block genuinely reusable rather than a settings shape that each section
    /// re-draws.
    ///
    /// Each child is wrapped in a `display: contents` element carrying `data-bs-block`, so the
    /// editor's click-to-select resolves it without the wrapper affecting layout - the same trick
    /// the hand-written block renderers already used.
    fn render_block_children(&mut self, scope: &Scope<'_>) -> String {
        // A block file renders with `block` as its root object; a section with `section`. Preferring
        // `block` is what makes {blocks} nest: a block hosting {blocks} renders ITS children.
        let list = match scope.get("block") {
            Value::Object(o) => o.get("blocks").cloned(),
            _ => match scope.get("section") {
                Value::Object(o) => o.get("blocks").cloned(),
                _ => None,
            },
        };
        let Some(Value::Array(children)) = list else {
            return String::new();
        };

        // PERFORMANCE (quadratic blowup): NOTHING PER CHILD MAY COPY THE GLOBALS MAP. Globals
        // contains `section`, which contains this very `blocks` array, so any per-child copy is
        // O(children) per child, i.e. O(n^2) overall - measured in seconds at a few thousand
        // blocks. The default `BLOCK_HARD_CEILING` of 200 hides that on a stock theme, but a
        // section schema is free to declare `max: 100000` and a theme can come from an untrusted
        // source.
        //
        // There are TWO such copies to keep hoisted, and hoisting only one still leaves it
        // quadratic: `scoped_context` clones the globals map, and `Scope::root` then deep-clones
        // every top-level VALUE out of it, `section` included. Both are done once, above the
        // loop. Per child we build a CHILD scope instead, which is what `Scope::child` is for: an
        // empty binding map linked to the parent by reference, so `block` is found in the child
        // and `section` resolves up the chain without being copied.
        //
        // Visibility must stay what the merged context gave it: globals-minus-SECTIONS_HTML_KEY
        // plus the block's own `block`.
        let mut base = self.globals.clone();
        base.remove(SECTIONS_HTML_KEY);
        let base_ctx = Value::Object(base);
        let root_scope = Scope::root(&base_ctx);

        let mut out = String::new();
        for child in &children {
            // SECURITY: charge the shared iteration budget per block, the same way components are
            // charged. Blocks nest, so without this a deep tree is an amplification primitive.
            if !self.charge_iteration() {
                break;
            }
            let Some(btype) = child.get("type").and_then(Value::as_str) else {
                continue;
            };
            // The block TYPE is the file name. Reject anything that could escape `blocks/`: the type
            // reaches here from untrusted saved state, and it is about to be used as a
            // registry path.
            if btype.is_empty()
                || !btype
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                continue;
            }
            let path = format!("@blocks/{btype}");
            // The cycle guard: a block whose file hosts its own type would otherwise recurse until
            // the budget ran out. Skipping is right - a block cannot contain itself.
            if self.rendering_stack.contains(&path) {
                continue;
            }
            let Some(doc) = self.parse_cached(&path) else {
                continue;
            };

            self.rendering_stack.insert(path.clone());
            let saved_imports = std::mem::replace(
                &mut self.import_map,
                build_import_map(doc.frontmatter.as_ref()),
            );
            let mut child_scope = root_scope.child();
            child_scope.set("block", child.clone());
            let html = self.render_nodes(&doc.children, &child_scope);
            self.import_map = saved_imports;
            self.rendering_stack.remove(&path);

            // SECURITY: the wrapper goes through `emit` so it is charged against
            // `MAX_OUTPUT_BYTES` like every other markup-producing path. It used to be a bare
            // `format!` onto a local `String`: the interior `{html}` was charged but the ~45-byte
            // wrapper and the escaped id were not, and `id` comes from saved editor state where
            // `escape_html` can expand it about 6x. Blocks are charged against
            // `MAX_TOTAL_ITERATIONS`, so the uncharged total was bounded rather than infinite,
            // but the 16 MiB ceiling was not the number it claimed to be. Charging the wrapper
            // and the body separately is deliberate: the body must still be emitted even if only
            // the wrapper pushes the total over, so the budget latches at the right place.
            let id = child.get("id").and_then(Value::as_str).unwrap_or_default();
            out.push_str(&self.emit(format!(
                "<div data-bs-block=\"{}\" style=\"display:contents\">{html}</div>",
                escape_html(id)
            )));
        }
        out
    }

    fn render_if(&mut self, node: &IfNode, scope: &Scope<'_>) -> String {
        let cond_val = self.eval_expr(&node.condition, scope);
        if self.is_truthy(&cond_val) {
            return self.render_nodes(&node.then_branch, scope);
        }
        for (cond, branch) in &node.else_if_branches {
            let cond_val = self.eval_expr(cond, scope);
            if self.is_truthy(&cond_val) {
                return self.render_nodes(branch, scope);
            }
        }
        if let Some(else_branch) = &node.else_branch {
            return self.render_nodes(else_branch, scope);
        }
        String::new()
    }

    fn render_each(&mut self, node: &EachNode, scope: &Scope<'_>) -> String {
        let collection_val = self.eval_expr(&node.collection, scope);
        let mut items: Vec<Value> = match collection_val {
            Value::Array(arr) => arr,
            Value::Null => Vec::new(),
            other => vec![other],
        };

        for f in &node.loop_filters {
            match apply_loop_filter(items, f, &self.file) {
                Ok(filtered) => items = filtered,
                Err(e) => {
                    self.diagnostics.push(e);
                    items = Vec::new();
                }
            }
        }

        if items.is_empty() {
            return match &node.empty_branch {
                Some(branch) => self.render_nodes(branch, scope),
                None => String::new(),
            };
        }

        let original_len = items.len();
        if original_len > MAX_ITERATIONS {
            // Reported in EVERY mode, like the output-byte and render-depth budgets. Truncating is
            // the right behaviour for a shared render service, but doing it silently is not: the
            // dropped rows leave no trace in the page, so a build that quietly loses a third of a
            // product list reports itself clean. `ErrorAccumulator::push` bounds the total, so a
            // template with many over-cap loops cannot grow this without limit.
            self.diagnostics.push(CompileError::warn(
                &self.file, 0, 0,
                format!(
                    "Iteration limit: collection has {original_len} items, capped at {MAX_ITERATIONS}"
                ),
                ErrorCode::IterationLimitExceeded,
            ));
            items.truncate(MAX_ITERATIONS);
        }

        let count = items.len();
        let mut out = String::new();

        for (idx, item) in items.into_iter().enumerate() {
            // SECURITY: charge the global iteration budget and stop once spent,
            // so nested loops can't multiply into an unbounded render.
            if !self.charge_iteration() {
                break;
            }
            let mut child = scope.child();
            child.set(&node.item, item);
            if let Some(index_name) = &node.index {
                child.set(index_name, Value::Number(idx.into()));
            }
            child.set("loop.index", Value::Number(idx.into()));
            child.set("loop.count", Value::Number(count.into()));
            child.set("loop.first", Value::Bool(idx == 0));
            child.set("loop.last", Value::Bool(idx == count - 1));

            out.push_str(&self.render_nodes(&node.body, &child));
        }

        out
    }

    /// SECURITY: increment the global per-render iteration counter and report
    /// whether the budget still has room. Returns `false` once
    /// `MAX_TOTAL_ITERATIONS` is reached (a single warning is emitted, in every
    /// mode); callers must then stop iterating.
    fn charge_iteration(&mut self) -> bool {
        if self.total_iterations >= MAX_TOTAL_ITERATIONS {
            // The equality test IS the emit-once latch, and the increment below is what arms it.
            // Keep the two together: separate them and this warns on every subsequent charge.
            if self.total_iterations == MAX_TOTAL_ITERATIONS {
                self.total_iterations += 1;
                self.diagnostics.push(CompileError::warn(
                    &self.file, 0, 0,
                    format!(
                        "Render iteration budget ({MAX_TOTAL_ITERATIONS}) exceeded; remaining loops truncated"
                    ),
                    ErrorCode::IterationLimitExceeded,
                ));
            }
            return false;
        }
        self.total_iterations += 1;
        true
    }

    fn render_match(&mut self, node: &MatchNode, scope: &Scope<'_>) -> String {
        let value = self.eval_expr(&node.value, scope);

        for arm in &node.arms {
            let matches = match (&arm.pattern, &value) {
                (MatchPattern::StringLiteral(s), Value::String(v)) => s == v,
                (MatchPattern::NumberLiteral(n), Value::Number(v)) => v
                    .as_f64()
                    .map(|f| (f - n).abs() < f64::EPSILON)
                    .unwrap_or(false),
                (MatchPattern::BoolLiteral(b), Value::Bool(v)) => b == v,
                (MatchPattern::Wildcard, _) => true,
                _ => false,
            };

            if matches {
                return self.render_nodes(&arm.body, scope);
            }
        }

        String::new()
    }

    fn render_repeat(&mut self, node: &RepeatNode, scope: &Scope<'_>) -> String {
        let count_val = self.eval_expr(&node.count, scope);
        let count = match &count_val {
            Value::Number(n) => {
                // as_u64() returns None for float-typed JSON numbers even when
                // the value is integral, so fall back to f64 conversion.
                n.as_u64()
                    .unwrap_or_else(|| n.as_f64().unwrap_or(0.0).max(0.0) as u64)
                    as usize
            }
            _ => 0,
        };
        let count = count.min(MAX_ITERATIONS);

        let mut out = String::new();
        for _ in 0..count {
            // SECURITY: charge the global iteration budget (nested-loop blowup guard).
            if !self.charge_iteration() {
                break;
            }
            out.push_str(&self.render_nodes(&node.body, scope));
        }
        out
    }

    fn render_fetch(&mut self, node: &FetchNode, scope: &Scope<'_>) -> String {
        let data = scope.get(&node.name);

        match &data {
            Value::Null => {
                if let Some(loading) = &node.loading_branch {
                    return self.render_nodes(loading, scope);
                }
                String::new()
            }
            Value::Object(obj) if obj.contains_key("__error") => {
                if let Some(error_branch) = &node.error_branch {
                    let mut child = scope.child();
                    child.set("error", obj.get("__error").cloned().unwrap_or(Value::Null));
                    return self.render_nodes(error_branch, &child);
                }
                String::new()
            }
            _ => {
                let is_empty = match &data {
                    Value::Array(a) => a.is_empty(),
                    _ => false,
                };

                if is_empty {
                    if let Some(empty) = &node.empty_branch {
                        return self.render_nodes(empty, scope);
                    }
                    return String::new();
                }

                let mut child = scope.child();
                child.set(&node.name, data);
                self.render_nodes(&node.body, &child)
            }
        }
    }

    fn render_try(&mut self, node: &TryNode, scope: &Scope<'_>) -> String {
        // Only the success branch is rendered server-side; the {error} branch
        // is reserved for the client runtime to display on fetch/reactive errors.
        self.render_nodes(&node.body, scope)
    }

    fn render_element(&mut self, node: &ElementNode, scope: &Scope<'_>) -> String {
        // HTML-syntax <layout src="..."> → treated as a LayoutNode
        if node.tag == "layout" {
            let src = node
                .attrs
                .iter()
                .find(|a| a.name == "src")
                .map(|a| match &a.value {
                    AttributeValue::StringLiteral(s) => s.clone(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            let layout_node = LayoutNode {
                src,
                head: None,
                children: node.children.clone(),
            };
            return self.render_layout(&layout_node, scope);
        }

        // HTML-syntax <slot /> or <slot name="head" /> → treated as a SlotNode
        if node.tag == "slot" {
            let name = node
                .attrs
                .iter()
                .find(|a| a.name == "name")
                .and_then(|a| match &a.value {
                    AttributeValue::StringLiteral(s) => Some(s.clone()),
                    _ => None,
                });
            let slot_node = SlotNode {
                name,
                fallback: if node.children.is_empty() {
                    None
                } else {
                    Some(node.children.clone())
                },
            };
            return self.render_slot(&slot_node, scope);
        }

        // SECURITY: structural markup (the open/close tags themselves) is charged
        // against MAX_OUTPUT_BYTES too, not just leaf text and attributes. Without
        // this, a loop body of attribute-less, text-less elements emits zero
        // charged bytes, so the output budget never latches and the loop runs to
        // the full iteration budget - defeating the guard.
        let mut out = self.emit(format!("<{}", node.tag));

        for attr in &node.attrs {
            let rendered = self.render_attr(attr, scope);
            out.push_str(&self.emit(rendered));
        }

        if node.self_closing {
            out.push_str(&self.emit(" />".to_string()));
            return out;
        }

        out.push_str(&self.emit(">".to_string()));
        // SECURITY: a `<script>` body is a raw-text element, where entity escaping
        // does nothing. Track it so `Node::Expression` can take the script path.
        let is_script = node.tag.eq_ignore_ascii_case("script");
        if is_script {
            self.script_depth += 1;
        }
        out.push_str(&self.render_nodes(&node.children, scope));
        if is_script {
            self.script_depth -= 1;
        }
        out.push_str(&self.emit(format!("</{}>", node.tag)));
        out
    }

    fn render_attr(&mut self, attr: &Attribute, scope: &Scope<'_>) -> String {
        // SECURITY (defense-in-depth): the branches below emit `attr.name`
        // literally into the start tag. Drop any attribute whose name is not a
        // well-formed HTML attribute name so a rendered token is always a single
        // attribute (no smuggled whitespace, `=`, quotes or `>`). The fixed/
        // sanitized-name branches (ClassList/NavigateFalse/RawAttrInjection) are
        // not affected.
        if matches!(
            &attr.value,
            AttributeValue::Boolean
                | AttributeValue::StringLiteral(_)
                | AttributeValue::Expr(_)
                | AttributeValue::Interpolated(_)
                | AttributeValue::EventHandler(_)
        ) && !is_well_formed_attr_name(&attr.name)
        {
            return String::new();
        }
        match &attr.value {
            AttributeValue::Boolean => format!(" {}", attr.name),
            // A quoted attribute with NO `${ }` in it: 100% author text (the
            // parser routes anything interpolated to `Interpolated`), so there is
            // no data in it to neutralize. Nothing is applied here, and that is
            // deliberate: a literal `style="display: flex;"` must reach the page
            // with its semicolon, and a literal `srcdoc` is the author's own
            // document.
            AttributeValue::StringLiteral(s) => self.finish_attr(&attr.name, s.clone(), true),
            AttributeValue::Interpolated(expr) => {
                // The `${ }` slots are DATA. In a JS-evaluated attribute they are
                // JS-escaped per slot; in a CSS one, CSS-neutralized. Doing
                // it PER SLOT is the whole point: it leaves the author's own
                // quotes in `f('${v}')` and the `url(` `)` around `${path}`
                // intact while neutralizing only what came from data.
                let ctx = InterpContext::for_attr(&attr.name);
                let expanded = self.eval_interpolated(expr, scope, ctx, &attr.name);
                let expanded = self.srcdoc_guard(&attr.name, expanded);
                self.finish_attr(&attr.name, expanded, false)
            }
            AttributeValue::Expr(expr) => {
                let val = self.eval_expr(expr, scope);
                let s = value_to_string(&val);
                // An `attr={ expr }` value is a WHOLE expression the theme author
                // composed, not a text template with a data slot, so it is left
                // alone - the same trust level as a `{html attrs}` spread (see
                // `is_safe_attr_name`). `style={ `a:${x}; b:${y}` }` is the author
                // BUILDING a declaration list, so a CSS neutralizer here would
                // corrupt exactly the code it was meant to protect. `srcdoc` is
                // the one exception: whoever composed it, the value becomes a
                // whole document.
                let s = self.srcdoc_guard(&attr.name, s);
                self.finish_attr(&attr.name, s, false)
            }
            AttributeValue::ClassList(expr) => {
                let class_str = self.render_class_list(expr, scope);
                if class_str.is_empty() {
                    String::new()
                } else {
                    format!(r#" class="{}""#, escape_attr(&class_str))
                }
            }
            AttributeValue::EventHandler(handler) => {
                format!(r#" {}="{}""#, attr.name, escape_attr(handler))
            }
            // `navigate="false"`, NOT `data-navigate="false"`. The client runtime reads
            // `anchor.getAttribute('navigate')` (`runtime/src/core/navigation.ts`), and it
            // reads a bare `navigate` on `<body>` to switch client navigation on at all, so
            // one spelling is used for both halves of the feature. Emitting the `data-`
            // form here made the documented opt-out silently do nothing: the attribute
            // reached the page and nothing ever looked at it. Change this only together
            // with the runtime.
            AttributeValue::NavigateFalse => r#" navigate="false""#.to_string(),
            AttributeValue::RawAttrInjection { condition, expr } => {
                if let Some(cond) = condition {
                    let cond_val = self.eval_expr(cond, scope);
                    if !self.is_truthy(&cond_val) {
                        return String::new();
                    }
                }
                let val = self.eval_expr(expr, scope);
                let s = value_to_string(&val);
                if s.is_empty() {
                    return String::new();
                }
                // SECURITY: a `{html ...}` ATTRIBUTE spread emits raw attribute
                // markup. Untrusted output must not introduce an event handler
                // (`on*=`) or a `javascript:`/`data:` URL, and each emitted token
                // must be a single well-formed attribute. `sanitize_attr_spread`
                // drops dangerous attributes and attribute-escapes every value.
                let safe = sanitize_attr_spread(&s);
                if safe.is_empty() {
                    String::new()
                } else {
                    format!(" {safe}")
                }
            }
        }
    }

    /// SECURITY: `srcdoc` round-trips straight back into live HTML, so the HTML
    /// attribute layer's escaping is CORRECT and then completely undone: the
    /// parser decodes `&lt;script&gt;` back to `<script>` and hands the result to
    /// the iframe as its document, same-origin by default.
    ///
    /// Sanitizing - rather than refusing the attribute, or double-escaping it into
    /// visible `&amp;lt;` garbage - is what keeps it usable for the one thing it
    /// exists for, embedded markup, while reusing the ammonia allowlist
    /// `{html sanitize}` already relies on. The sanitizer runs on the value the
    /// iframe will see AFTER that single decode, which is the right layer.
    ///
    /// The BOUND form (`:srcdoc` / `x-bind:srcdoc`) is deliberately not handled
    /// here: its value is a JavaScript expression evaluated in the browser, not
    /// markup, and the JS interpolation rules cover it instead.
    fn srcdoc_guard(&mut self, name: &str, value: String) -> String {
        if name.eq_ignore_ascii_case("srcdoc") {
            return sanitize_html(&value);
        }
        value
    }

    /// Emit one finished attribute: scheme-check it if it carries a URL, then
    /// attribute-escape and double-quote the value.
    ///
    /// `keep_empty` preserves the long-standing shape difference between the two
    /// attribute forms - a literal `alt=""` stays, while an expression that
    /// evaluates to nothing drops the attribute entirely.
    fn finish_attr(&mut self, name: &str, value: String, keep_empty: bool) -> String {
        if value.is_empty() && !keep_empty {
            return String::new();
        }
        if is_url_attr(name)
            && (has_forbidden_url_scheme(&value)
                || bound_url_literal(name, &value)
                    .is_some_and(|inner| has_forbidden_url_scheme(&inner)))
        {
            // Neutralize a script-scheme URL (javascript:/data:/vbscript:) in an href/src.
            return format!(r#" {name}="""#);
        }
        format!(r#" {name}="{}""#, escape_attr(&value))
    }

    fn warn_css_stripped(&mut self, name: &str, original: &str, at: (usize, usize)) {
        if self.mode == BuildMode::Development {
            self.diagnostics.push(CompileError::warn(
                &self.file, at.0, at.1,
                format!(
                    "CSS-structural characters were removed from a `{name}` value slot ({original:?}); a value interpolated into a style attribute cannot carry ; ( ) {{ }} quotes or @"
                ),
                ErrorCode::UnsafeScriptInterpolation,
            ));
        }
    }

    /// Evaluate the template literal an interpolated attribute was parsed into,
    /// escaping each `${...}` slot for the attribute's context.
    ///
    /// SECURITY: `escape_attr` is the wrong escaper for a JS-evaluated attribute
    /// and says so in its own doc comment: the HTML parser DECODES character
    /// references in an attribute value before the client-side framework or the
    /// event-handler compiler ever sees the string, so HTML-escaping alone lets a
    /// data slot break out of its JS string. With `escape_attr`,
    /// `<button @click="add('${v}')">` and `v = "');alert(1);('"` reach the JS
    /// engine as `add('');alert(1);('')`.
    ///
    /// The slots are escaped PER PART rather than over the whole expanded string,
    /// which is what keeps the theme's own literal text (the quotes in
    /// `f('...')`) intact while neutralizing only the data that lands between
    /// them.
    ///
    /// Two explicit opt-outs, because a JS-evaluated attribute carries two very
    /// different kinds of value and only the author can tell them apart:
    ///   - `| js`        - data. Already the right escaper; not applied twice.
    ///   - `| unsafe_js` - JavaScript the THEME wrote, passed to a component as a
    ///     prop and composed into a handler (`x-text="${ valueExpression }"`).
    ///     Escaping that would corrupt working code, so it must be declarable.
    ///
    /// Anything else is auto-escaped (safe by default) and, in development,
    /// warned about so the author is told exactly where to declare intent.
    ///
    /// WHERE the slot lands decides which escaper is the right one, and that is a
    /// lexical property of the author's own text (`js_slot::slot_quote_contexts`).
    /// Inside a JS string literal, `escape_js_string` is a complete answer. In
    /// STATEMENT or EXPRESSION position it is no answer at all - `;` and `(` have
    /// no JS string escape - so the value is emitted as a JSON literal instead,
    /// which IS self-delimiting whatever it contains. See `escape_slot`.
    fn eval_interpolated(
        &mut self,
        expr: &Expression,
        scope: &Scope<'_>,
        ctx: InterpContext,
        attr_name: &str,
    ) -> String {
        if ctx.is_plain() {
            let val = self.eval_expr(expr, scope);
            return value_to_string(&val);
        }
        // Diagnostics anchor on the ATTRIBUTE's own span. The `${ }` parts were
        // re-lexed from a synthetic one-line source, so their line/column point
        // into that scratch buffer and would send an author to line 1 of the file.
        let at = (expr.line, expr.column);
        match (&expr.kind, expr.filters.is_empty()) {
            (ExprKind::TemplateLiteral(parts), true) => {
                let quotes = slot_quote_contexts(parts).quotes;
                let mut slot_idx = 0usize;
                let mut out = String::new();
                for part in parts {
                    match part {
                        TemplatePart::Literal(lit) => out.push_str(lit),
                        TemplatePart::Expr(e) => {
                            let quote = quotes.get(slot_idx).copied().flatten();
                            slot_idx += 1;
                            let val = self.eval_expr(e, scope);
                            out.push_str(&self.escape_slot(&val, e, ctx, attr_name, at, quote));
                        }
                    }
                }
                out
            }
            // Not a parser-produced interpolation (the parser only ever builds
            // `Interpolated` from a template literal with no filters). There is no
            // surrounding author text to read a quote context out of, so the slot IS
            // the whole attribute value: statement position, the strict branch.
            _ => {
                let val = self.eval_expr(expr, scope);
                self.escape_slot(&val, expr, ctx, attr_name, at, None)
            }
        }
    }

    /// Neutralize ONE interpolated slot for the context it lands in. `quote` is the
    /// JS string literal enclosing it, `None` for statement/expression position.
    fn escape_slot(
        &mut self,
        val: &Value,
        expr: &Expression,
        ctx: InterpContext,
        attr_name: &str,
        at: (usize, usize),
        quote: Option<char>,
    ) -> String {
        let mut out = value_to_string(val);
        // CSS first: it REMOVES structural characters, and doing it after the JS
        // pass would strip the backslashes that pass put there.
        if ctx.css {
            let stripped = escape_css_value(&out);
            if stripped != out {
                self.warn_css_stripped(attr_name, &out, at);
            }
            out = stripped;
        }
        if !ctx.js {
            return out;
        }
        match quote {
            // INSIDE a JS string literal: the well-defined case. This is where
            // essentially every real call site lands, and they depend on it.
            Some(_) => {
                if expr_declares_js_intent(expr) {
                    return out;
                }
                self.warn_undeclared_js_slot(attr_name, at, true);
                escape_js_string(&out)
            }
            // STATEMENT or EXPRESSION position. `escape_js_string` cannot help here
            // and saying it does would be the fake fix a `<script>` body refuses to
            // ship. A JSON literal is the one encoding that IS self-delimiting in
            // this position: a string becomes a quoted string, a number stays a bare
            // number, and neither can contribute a `;` or a `(` to the program.
            None => {
                if expr_has_filter(expr, "unsafe_js") || expr_has_filter(expr, "json") {
                    // Declared: theme-written JavaScript, or a value the `json`
                    // filter has already encoded. Both are the author saying "this
                    // belongs in code position", which is exactly true here.
                    return out;
                }
                if expr_has_filter(expr, "js") {
                    // `| js` is a CLAIM that the slot is inside a string literal, and
                    // a declared slot is passed through VERBATIM - so a false claim is
                    // strictly worse than no claim. `lib.rs`'s security walk refuses
                    // this at compile time; this is the render-time half of the same
                    // rule, for the paths that render without the gate (sections).
                    self.diagnostics.push(CompileError::render(
                        &self.file,
                        at.0,
                        at.1,
                        js_filter_outside_string_literal(attr_name),
                        ErrorCode::UnsafeScriptInterpolation,
                    ));
                }
                self.warn_undeclared_js_slot(attr_name, at, false);
                json_literal(val, &out)
            }
        }
    }

    fn warn_undeclared_js_slot(&mut self, attr_name: &str, at: (usize, usize), in_string: bool) {
        if self.mode != BuildMode::Development {
            return;
        }
        let how = if in_string {
            "auto-escaped as a JS string"
        } else {
            "in statement position and is auto-encoded as a JSON literal"
        };
        self.diagnostics.push(CompileError::warn(
            &self.file, at.0, at.1,
            format!(
                "interpolation in the JavaScript-evaluated attribute `{attr_name}` is {how}; add `| js` (data inside a string literal) or `| json` (a bare JSON value) to confirm it is data, or `| unsafe_js` if it is JavaScript the theme wrote"
            ),
            ErrorCode::UnsafeScriptInterpolation,
        ));
    }

    fn render_class_list(&mut self, expr: &Expression, scope: &Scope<'_>) -> String {
        match &expr.kind {
            ExprKind::ClassList(entries) => {
                let mut classes: Vec<String> = Vec::new();
                for entry in entries {
                    let include = match &entry.condition {
                        None => true,
                        Some(cond) => {
                            let cv = self.eval_expr(cond, scope);
                            self.is_truthy(&cv)
                        }
                    };
                    if include {
                        let val = self.eval_expr(&entry.value, scope);
                        let s = value_to_string(&val);
                        if !s.is_empty() {
                            classes.push(s);
                        }
                    }
                }
                classes.join(" ")
            }
            _ => {
                let val = self.eval_expr(expr, scope);
                value_to_string(&val)
            }
        }
    }

    fn render_component(&mut self, node: &ComponentNode, scope: &Scope<'_>) -> String {
        // SECURITY: charge the global iteration budget per component
        // instantiation. The cycle guard (`rendering_stack`) only stops
        // RE-ENTRANT cycles; without a fan-out budget, N chained components
        // each referencing the next twice expand to 2^N renders.
        //
        // This charge is DELIBERATELY above the `Image` fast path. It used to sit below it,
        // so `<Image>` - the one built-in that expands to ~330 bytes of markup carrying three
        // url-encoded (up to 3x) copies of an input string - was the only component that
        // cost nothing against either budget. `{repeat 20000}<Image .../>{/repeat}` rendered
        // 420 MB from a template of 50 bytes.
        if !self.charge_iteration() {
            return String::new();
        }

        if node.name == "Image" {
            return self.render_image_component(node, scope);
        }

        let mut props_map = serde_json::Map::new();
        for prop in &node.props {
            let val = match &prop.value {
                PropValue::Boolean => Value::Bool(true),
                PropValue::Expr(e) => self.eval_expr(e, scope),
            };
            props_map.insert(prop.name.clone(), val);
        }

        // Render children of the component call as slot content
        let slot_html = self.render_nodes(&node.children, scope);

        if let Some(at_path) = self.import_map.get(&node.name).cloned()
            && !self.rendering_stack.contains(&at_path)
            && let Some(doc) = self.parse_cached(&at_path)
        {
            // Apply declared prop defaults for any prop not explicitly passed
            if let Some(fm) = doc.frontmatter.as_ref() {
                for prop_def in &fm.props {
                    if !props_map.contains_key(&prop_def.name)
                        && let Some(default_expr) = &prop_def.default_value
                    {
                        let default_val = self.eval_expr(default_expr, scope);
                        props_map.insert(prop_def.name.clone(), default_val);
                    }
                }
            }
            self.rendering_stack.insert(at_path.clone());
            let saved_imports = std::mem::replace(
                &mut self.import_map,
                build_import_map(doc.frontmatter.as_ref()),
            );
            let saved_slot = self.slot_content.replace(slot_html);
            let props_ctx = self.scoped_context(props_map);
            let child_scope = Scope::root(&props_ctx);
            let html = self.render_nodes(&doc.children, &child_scope);
            self.import_map = saved_imports;
            self.slot_content = saved_slot;
            self.rendering_stack.remove(&at_path);
            return html;
        }

        // Fallback: emit placeholder
        let props_json = serde_json::to_string(&Value::Object(props_map)).unwrap_or_default();
        let wrapper = format!(
            r#"<div data-slurp-component="{}" data-slurp-props="{}">"#,
            escape_attr(&node.name),
            escape_attr(&props_json),
        );
        let mut out = self.emit(wrapper);
        out.push_str(&self.render_nodes(&node.children, scope));
        out.push_str(&self.emit("</div>".to_string()));
        out
    }

    fn render_image_component(&mut self, node: &ComponentNode, scope: &Scope<'_>) -> String {
        // Evaluate props via a closure to avoid borrow conflicts on `self`.
        let eval_prop = |renderer: &mut Renderer, name: &str| -> Value {
            node.props
                .iter()
                .find(|p| p.name == name)
                .map(|p| match &p.value {
                    PropValue::Boolean => Value::Bool(true),
                    PropValue::Expr(e) => renderer.eval_expr(e, scope),
                })
                .unwrap_or(Value::Null)
        };

        let src_val = eval_prop(self, "src");
        let alt_val = eval_prop(self, "alt");
        let width_val = eval_prop(self, "width");
        let height_val = eval_prop(self, "height");
        let priority_val = eval_prop(self, "priority");

        let src = value_to_string(&src_val);
        let alt = value_to_string(&alt_val);
        // A template's `width={400}` is parsed as an f64 and serializes as a FLOAT-typed
        // JSON number, for which `as_u64()` returns None even though the value is
        // integral - so the author's dimensions were silently replaced by 800x600 while
        // context-supplied integers worked. `render_repeat` already carries this fallback.
        let width = json_uint(&width_val).unwrap_or(800);
        let height = json_uint(&height_val).unwrap_or(600);
        let lazy = !self.is_truthy(&priority_val);

        if src.is_empty() {
            self.diagnostics.push(CompileError::render(
                &self.file,
                node.line,
                node.column,
                "<Image> is missing the src prop",
                ErrorCode::MissingImageSrc,
            ));
            return String::new();
        }

        let encoded_src = urlencoding_encode(&src);
        let loading = if lazy { "lazy" } else { "eager" };

        // SECURITY: through `emit`, not a bare `format!`. Every other markup-producing path
        // charges the output budget; this one did not, which made the 16 MiB ceiling
        // unenforceable for the component whose expansion factor is by far the largest.
        self.emit(format!(
            r#"<picture>
  <source srcset="/_slurp/image?src={src}&w={w}&f=avif" type="image/avif" />
  <source srcset="/_slurp/image?src={src}&w={w}&f=webp" type="image/webp" />
  <img src="/_slurp/image?src={src}&w={w}&f=jpeg" width="{w}" height="{h}" alt="{alt}" loading="{loading}" decoding="async" />
</picture>"#,
            src = encoded_src,
            w = width,
            h = height,
            alt = escape_attr(&alt),
            loading = loading,
        ))
    }

    fn render_layout(&mut self, node: &LayoutNode, scope: &Scope<'_>) -> String {
        // SECURITY: layouts expand like components; charge the same budget.
        if !self.charge_iteration() {
            return String::new();
        }
        let head_html = match &node.head {
            Some(head_nodes) => self.render_nodes(head_nodes, scope),
            None => String::new(),
        };
        let body_html = self.render_nodes(&node.children, scope);

        let layout_path = if node.src.starts_with('@') {
            node.src.clone()
        } else {
            format!("@{}", node.src)
        };

        if !self.rendering_stack.contains(&layout_path)
            && let Some(doc) = self.parse_cached(&layout_path)
        {
            self.rendering_stack.insert(layout_path.clone());
            let saved_imports = std::mem::replace(
                &mut self.import_map,
                build_import_map(doc.frontmatter.as_ref()),
            );
            let saved_slot = self.slot_content.replace(body_html);
            let saved_head = self.head_content.replace(head_html);
            let layout_ctx = self.scoped_context(serde_json::Map::new());
            let layout_scope = Scope::root(&layout_ctx);
            let html = self.render_nodes(&doc.children, &layout_scope);
            self.slot_content = saved_slot;
            self.head_content = saved_head;
            self.import_map = saved_imports;
            self.rendering_stack.remove(&layout_path);
            return html;
        }

        // Fallback: emit comment wrapper. head_html/body_html are already charged
        // by render_nodes above, so charge only the structural bytes here.
        let prefix = self.emit(format!("<!-- layout:{} -->", escape_html(&node.src)));
        let mid = self.emit("<slot></slot>".to_string());
        let suffix = self.emit(format!("<!-- /layout:{} -->", escape_html(&node.src)));
        format!("{prefix}{head_html}{mid}{body_html}{suffix}")
    }

    fn render_slot(&mut self, node: &SlotNode, scope: &Scope<'_>) -> String {
        let name = node.name.as_deref().unwrap_or("default");
        // Slot content is CLONED per slot, so a layout with several `<slot/>`
        // elements duplicates it; each copy must charge the output budget.
        match name {
            "default" => {
                if let Some(content) = &self.slot_content {
                    let content = content.clone();
                    return self.emit(content);
                }
            }
            "head" => {
                if let Some(content) = &self.head_content {
                    let content = content.clone();
                    return self.emit(content);
                }
            }
            _ => {}
        }
        let fallback = match &node.fallback {
            Some(fb) => self.render_nodes(fb, scope),
            None => String::new(),
        };
        // `fallback` is already charged by render_nodes; charge only the tags.
        let open = self.emit(format!(r#"<slot name="{}">"#, escape_attr(name)));
        let close = self.emit("</slot>".to_string());
        format!("{open}{fallback}{close}")
    }

    pub fn eval_expr(&mut self, expr: &Expression, scope: &Scope<'_>) -> Value {
        let base = self.eval_expr_kind(&expr.kind, scope);

        let mut val = base;
        for filter in &expr.filters {
            match apply_filter(val.clone(), filter, &self.file) {
                Ok(filtered) => {
                    // SECURITY (memory): charge the INTERMEDIATE value HERE, before it is handed
                    // to the next filter. Charging the chain's result instead would let the
                    // doubling run to completion first, which is the whole exposure - see
                    // `MAX_FILTER_VALUE_BYTES`.
                    if !self.charge_filter_value(&filtered, filter) {
                        return Value::Null;
                    }
                    // "default" filter: if the filtered value is null, evaluate the fallback arg
                    if filter.name == "default" && filtered.is_null() && !filter.args.is_empty() {
                        val = self.eval_expr(&filter.args[0], scope);
                    } else {
                        val = filtered;
                    }
                }
                Err(e) => self.diagnostics.push(e),
            }
        }

        val
    }

    /// Pre-evaluate every non-literal subscript in a path so `Scope::resolve_path` (which
    /// has no renderer, and so can only resolve literal subscripts) sees constants.
    /// Returns `None` when nothing needs evaluating, which is the overwhelmingly common
    /// case and allocates nothing.
    fn resolve_index_segments(
        &mut self,
        segments: &[PathSegment],
        scope: &Scope<'_>,
    ) -> Option<Vec<PathSegment>> {
        let needs_eval = segments
            .iter()
            .any(|s| matches!(s, PathSegment::Index(e) if !is_const_index(e)));
        if !needs_eval {
            return None;
        }
        let mut out = Vec::with_capacity(segments.len());
        for seg in segments {
            match seg {
                PathSegment::Index(e) if !is_const_index(e) => {
                    let v = self.eval_expr(e, scope);
                    let kind = match v {
                        Value::Number(n) => ExprKind::NumberLiteral(n.as_f64().unwrap_or(-1.0)),
                        Value::String(s) => ExprKind::StringLiteral(s),
                        // Anything else cannot index an array or an object; Null makes
                        // `index_into` yield Null, matching `ExprKind::Index`.
                        _ => ExprKind::Null,
                    };
                    out.push(PathSegment::Index(Box::new(Expression::new(
                        kind, e.line, e.column,
                    ))));
                }
                other => out.push(other.clone()),
            }
        }
        Some(out)
    }

    fn eval_expr_kind(&mut self, kind: &ExprKind, scope: &Scope<'_>) -> Value {
        match kind {
            ExprKind::StringLiteral(s) => Value::String(s.clone()),
            ExprKind::NumberLiteral(n) => {
                Value::Number(serde_json::Number::from_f64(*n).unwrap_or_else(|| 0.into()))
            }
            ExprKind::BoolLiteral(b) => Value::Bool(*b),
            ExprKind::Null | ExprKind::Undefined => Value::Null,

            ExprKind::Path(segments) | ExprKind::ReactivePath(segments) => {
                // `${ items[i] }` is ONE path whose second segment is a subscript, and
                // `Scope::resolve_path` can only resolve a literal subscript (it has no
                // renderer to evaluate `i` with), so a variable index used to render
                // empty while the parenthesized `${ (items)[i] }` worked. Evaluate any
                // non-literal subscript here, then hand the path over already constant.
                match self.resolve_index_segments(segments, scope) {
                    Some(resolved) => scope.resolve_path(&resolved),
                    None => scope.resolve_path(segments),
                }
            }

            ExprKind::TemplateLiteral(parts) => {
                let mut s = String::new();
                for part in parts {
                    match part {
                        TemplatePart::Literal(lit) => s.push_str(lit),
                        TemplatePart::Expr(e) => {
                            let v = self.eval_expr(e, scope);
                            s.push_str(&value_to_string(&v));
                        }
                    }
                }
                Value::String(s)
            }

            ExprKind::BinaryOp { op, left, right } => self.eval_binary_op(op, left, right, scope),

            ExprKind::UnaryNot(inner) => {
                let v = self.eval_expr(inner, scope);
                Value::Bool(!self.is_truthy(&v))
            }

            ExprKind::UnaryNeg(inner) => {
                let v = self.eval_expr(inner, scope);
                let f = match &v {
                    Value::Number(n) => -n.as_f64().unwrap_or(0.0),
                    _ => 0.0,
                };
                Value::Number(serde_json::Number::from_f64(f).unwrap_or_else(|| 0.into()))
            }

            ExprKind::Ternary {
                condition,
                then,
                else_,
            } => {
                let cond = self.eval_expr(condition, scope);
                if self.is_truthy(&cond) {
                    self.eval_expr(then, scope)
                } else {
                    self.eval_expr(else_, scope)
                }
            }

            ExprKind::Call { .. } => {
                // Function calls at render time are not supported (they'd require
                // a JS runtime).
                Value::Null
            }

            ExprKind::Index { object, index } => {
                let obj = self.eval_expr(object, scope);
                let idx = self.eval_expr(index, scope);
                match (&obj, &idx) {
                    (Value::Array(arr), Value::Number(n)) => {
                        let i = n.as_u64().unwrap_or(0) as usize;
                        arr.get(i).cloned().unwrap_or(Value::Null)
                    }
                    (Value::Object(map), Value::String(key)) => {
                        map.get(key).cloned().unwrap_or(Value::Null)
                    }
                    _ => Value::Null,
                }
            }

            ExprKind::ClassList(_) => Value::String(String::new()),

            ExprKind::Grouped(inner) => self.eval_expr(inner, scope),
        }
    }

    fn eval_binary_op(
        &mut self,
        op: &BinaryOp,
        left: &Expression,
        right: &Expression,
        scope: &Scope<'_>,
    ) -> Value {
        match op {
            BinaryOp::And => {
                let lv = self.eval_expr(left, scope);
                if !self.is_truthy(&lv) {
                    return lv;
                }
                return self.eval_expr(right, scope);
            }
            BinaryOp::Or => {
                let lv = self.eval_expr(left, scope);
                if self.is_truthy(&lv) {
                    return lv;
                }
                return self.eval_expr(right, scope);
            }
            _ => {}
        }

        let lv = self.eval_expr(left, scope);
        let rv = self.eval_expr(right, scope);

        match op {
            BinaryOp::Add => match (&lv, &rv) {
                (Value::String(a), _) => Value::String(format!("{}{}", a, value_to_string(&rv))),
                (_, Value::String(b)) => Value::String(format!("{}{}", value_to_string(&lv), b)),
                _ => numeric_binop(lv, rv, |a, b| a + b),
            },
            BinaryOp::Sub => numeric_binop(lv, rv, |a, b| a - b),
            BinaryOp::Mul => numeric_binop(lv, rv, |a, b| a * b),
            BinaryOp::Div => numeric_binop(lv, rv, |a, b| if b == 0.0 { 0.0 } else { a / b }),
            BinaryOp::Mod => numeric_binop(lv, rv, |a, b| if b == 0.0 { 0.0 } else { a % b }),
            BinaryOp::Eq => Value::Bool(values_equal_deep(&lv, &rv)),
            BinaryOp::Neq => Value::Bool(!values_equal_deep(&lv, &rv)),
            BinaryOp::Gt => compare_values(&lv, &rv, std::cmp::Ordering::Greater),
            BinaryOp::Lt => compare_values(&lv, &rv, std::cmp::Ordering::Less),
            BinaryOp::Gte => {
                let eq = values_equal_deep(&lv, &rv);
                let gt = matches!(
                    compare_values(&lv, &rv, std::cmp::Ordering::Greater),
                    Value::Bool(true)
                );
                Value::Bool(eq || gt)
            }
            BinaryOp::Lte => {
                let eq = values_equal_deep(&lv, &rv);
                let lt = matches!(
                    compare_values(&lv, &rv, std::cmp::Ordering::Less),
                    Value::Bool(true)
                );
                Value::Bool(eq || lt)
            }
            BinaryOp::And | BinaryOp::Or => unreachable!(), // handled above
        }
    }

    fn is_truthy(&self, v: &Value) -> bool {
        match v {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        }
    }
}

/// The message a `${ ... }` in a `<script>` body gets when it does not declare
/// how it should be escaped. Shared by the compile-time gate (`lib.rs`) and the
/// render-time backstop so the two can never drift into saying different things.
///
/// It says "must carry", not "must end with", because that is the rule
/// `expr_has_script_filter` actually enforces: it searches the WHOLE chain, so
/// `${ v | js | default("x") }` is accepted. The message used to say "end with",
/// which describes a stricter engine than the one shipped and sends an author
/// reordering a filter chain that was already fine.
pub const SCRIPT_FILTER_REQUIRED: &str = "an expression in a <script> body must carry the `| js` filter (data inside a JS string literal) or `| json` (a bare JSON value); HTML entity escaping does not protect a script context, because a raw-text element is handed to the JS engine without decoding character references. For raw bytes, use {html expr}. A JavaScript template literal (`a ${b}`) cannot be written in a <script> body at all - slurp reads `${` as its own interpolation - so build the string by concatenation instead";

/// Does this expression's filter chain make it safe to emit VERBATIM into a
/// `<script>` body? `js` puts it inside a quoted JS string literal, `json`
/// makes it a bare JSON value; both run through an escaper that neutralizes
/// `</script>`.
///
/// The whole chain is searched rather than just the last filter, so
/// `${ v | js | default("x") }` is accepted. `unsafe_js` is deliberately NOT
/// accepted here: it is the "this is author-written JavaScript" marker for
/// attributes, and in a script body `{html expr}` already means exactly that,
/// more legibly.
pub(crate) fn expr_has_script_filter(expr: &Expression) -> bool {
    expr.filters
        .iter()
        .any(|f| f.name == "js" || f.name == "json")
}

/// Has the author declared what an interpolation in a JS-evaluated attribute is,
/// so the renderer must not escape it again? `js` = data, already escaped;
/// `unsafe_js` = JavaScript the theme wrote, pass through.
fn expr_declares_js_intent(expr: &Expression) -> bool {
    expr_has_filter(expr, "js") || expr_has_filter(expr, "unsafe_js")
}

/// Does this expression's filter chain contain `name`? The WHOLE chain is
/// searched, not just the last link, so `${ v | js | default("x") }` still counts
/// as declared - the same rule `expr_has_script_filter` uses.
pub(crate) fn expr_has_filter(expr: &Expression, name: &str) -> bool {
    expr.filters.iter().any(|f| f.name == name)
}

/// SECURITY: encode a slot value as a JSON literal, for a slot that lands in JS
/// statement or expression position.
///
/// A JSON literal is SELF-DELIMITING: whatever the value contains, the result is
/// one string / number / boolean / object / array, so the value cannot contribute
/// a `;`, a `(` or anything else structural to the surrounding program. That is
/// the property `escape_js_string` cannot provide outside a string literal, and it
/// is why this is a real fix rather than a fix-shaped one.
///
/// `text` is the value's string form AFTER any CSS neutralization, so a `:style`
/// slot keeps that pass; only a string value can be changed by it.
///
/// Null stays EMPTY rather than becoming `null`. That preserves the pre-existing
/// rendering exactly (an attribute whose whole value is empty is dropped), and an
/// empty slot cannot carry an injection.
fn json_literal(val: &Value, text: &str) -> String {
    match val {
        Value::Null => String::new(),
        Value::String(_) => escape_script_safe(
            &serde_json::to_string(&Value::String(text.to_string()))
                .unwrap_or_else(|_| "\"\"".to_string()),
        ),
        other => {
            escape_script_safe(&serde_json::to_string(other).unwrap_or_else(|_| "null".to_string()))
        }
    }
}

/// Which extra escaper(s) an interpolated attribute value needs on top of
/// `escape_attr`, derived from the attribute NAME.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InterpContext {
    /// The browser evaluates this attribute's value as JavaScript.
    js: bool,
    /// The value is (or becomes) CSS.
    css: bool,
}

impl InterpContext {
    fn for_attr(name: &str) -> Self {
        Self {
            js: is_js_attr(name),
            css: is_css_attr(name),
        }
    }

    /// Nothing beyond ordinary attribute escaping applies.
    fn is_plain(self) -> bool {
        !self.js && !self.css
    }
}

/// True when the BROWSER evaluates this attribute's value as JavaScript, so the
/// value has a second parser after the HTML one and entity escaping is undone
/// before it runs.
///
/// Covers native `on*` handlers, Alpine's `@`/`x-on:` listeners, every attribute
/// BIND (`:x` / `x-bind:x`, whose value is an expression), and the Alpine
/// directives that take a JS expression. Modifier suffixes are stripped, so
/// `x-model.lazy` is recognized. `x-transition:enter` and friends are NOT here on
/// purpose - their value is a class list, not an expression.
pub(crate) fn is_js_attr(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with('@') || lower.starts_with("x-on:") {
        return true;
    }
    if lower.starts_with("x-bind:") || lower.starts_with(':') {
        return true;
    }
    // A native event handler. No standard HTML attribute begins with `on`
    // without being one.
    if lower.len() > 2 && lower.starts_with("on") {
        return true;
    }
    let base = lower.split('.').next().unwrap_or(&lower);
    matches!(
        base,
        "x-data"
            | "x-init"
            | "x-effect"
            | "x-show"
            | "x-text"
            | "x-html"
            | "x-if"
            | "x-for"
            | "x-model"
    )
}

/// The attribute's value is CSS: a plain `style`, or a bound `:style` whose JS
/// expression builds a style string in the browser.
fn is_css_attr(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let target = lower
        .strip_prefix("x-bind:")
        .or_else(|| lower.strip_prefix(':'))
        .unwrap_or(&lower);
    target == "style"
}

/// A well-formed HTML attribute name for the renderer's own attributes (which
/// the parser produced, i.e. THEME-authored markup, not data). Permits the
/// names slurp emits - `class`, `data-*`, `aria-*`, Alpine `@click`/`x-*`/
/// `:bind` directives and `on:click` event bindings - while rejecting any
/// token carrying whitespace, `=`, quotes or `>`. Unlike `is_safe_attr_name`
/// (the untrusted `{html}` spread gate) this does NOT reject event-handler
/// names: the template author already controls this markup.
fn is_well_formed_attr_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.' | '@'))
}

/// SECURITY: attribute names allowed on the `{html ...}` attribute-spread path.
/// A valid name is ASCII letters, digits, `-`, `_`, `.`, plus the Alpine
/// directive punctuation `@` and `:` (so a rendered token is always a single
/// well-formed attribute and can't smuggle whitespace, `=`, quotes or `>` into
/// the tag).
///
/// TRUST BOUNDARY - read this before relying on the gate for anything:
/// **a `{html attrs}` spread is THEME-TRUST ONLY. This is not a sanitizer for
/// untrusted data and cannot be made into one.**
///
/// The gate does NOT make a spread safe when untrusted data reaches it. The
/// spread's whole purpose is forwarding Alpine directives - the theme's
/// Button/Input/Select components pass `@click`, `x-on:click`, `x-model`,
/// `:disabled`, `:id`, `@keydown.backspace` through an `attrs` prop - and Alpine
/// evaluates the value of EVERY directive and every bind as JavaScript. So
/// `x-show="alert(1)"`, `x-text="alert(1)"`, `x-for="i in [alert(1)]"`,
/// `:title="alert(1)"` and `style="..."` all pass this gate today, by design and
/// unavoidably: an allow-list that admits `:disabled="loading"` cannot also
/// exclude `:title="alert(1)"`, because they are the same construct.
///
/// What the two NAME rejections below actually buy is NARROWING, not safety.
/// They are kept because they cost nothing and the theme never authors them:
///   1. Native browser event handlers - `on*` literally, AND smuggled through an
///      Alpine attribute bind (`:onclick`, `x-bind:onclick`) which would SET the
///      native `onclick` attribute. These execute the value as script the moment
///      they hit the DOM, with no Alpine present at all.
///   2. `x-html`, `x-data`, `x-init`, `x-effect` - the innerHTML sink and the
///      three directives that run on element INITIALIZATION rather than on a DOM
///      event.
///
/// Note `x-on:click` (an Alpine event LISTENER) is allowed - it does not create
/// a native handler; only `x-bind:`/`:` prefixes target a native attribute name.
/// Value-level protections (script-scheme URLs, attribute escaping, quote-aware
/// single-attribute parsing) still apply in `sanitize_attr_spread`; this gate
/// only vets the NAME.
fn is_safe_attr_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // Match case-insensitively: the browser lowercases attribute names before
    // Alpine reads them, so `X-BIND:ONCLICK` reaches the DOM as `x-bind:onclick`.
    // Compare on the lowercased name so an uppercased prefix can't dodge a check.
    let lower = name.to_ascii_lowercase();
    // Native event handler, literal or via an Alpine attribute-bind alias
    // (`:onclick`, `x-bind:onclick`) that would set the native `onclick` attr.
    let bind_target = lower
        .strip_prefix("x-bind:")
        .or_else(|| lower.strip_prefix(':'))
        .unwrap_or(&lower);
    // `starts_with`, never a byte slice. The name arrives from rendered data, so it can begin
    // with a multi-byte character, and `&bind_target[..2]` would then split a UTF-8 sequence and
    // panic. The charset allow-list that would reject such a name runs later in this function,
    // so it cannot be relied on to filter the input to this check.
    if bind_target.starts_with("on") {
        return false;
    }
    // Alpine JS/innerHTML sinks with no event gate - the theme never forwards
    // these through `attrs`, so keep them blocked as defense-in-depth.
    if matches!(lower.as_str(), "x-html" | "x-data" | "x-init" | "x-effect") {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':'))
}

/// True when an attribute's VALUE is a URL the browser navigates to or fetches, so a script-scheme
/// value is an XSS vector. Covers the plain and Alpine-bound (`:href` / `x-bind:href`) forms. Used to
/// scheme-check a bare `href={...}` / `src={...}`, which (unlike the `{html attrs}` spread) otherwise
/// gets no scheme validation.
///
/// `data` (`<object data>`), `ping` (`<a ping>`) and `srcset` were added for
/// CONSISTENCY, not because they were exploitable: the list already carried
/// entries as obscure as `background` and `cite`, so leaving three URL-bearing
/// attributes out of it was an invitation to assume the list is exhaustive when
/// it was not.
///
/// KNOWN LIMIT on `srcset`: its value is a COMMA-SEPARATED list of
/// `url descriptor` pairs, and `has_forbidden_url_scheme` only inspects the
/// leading scheme, so a script scheme in the second or later candidate is not
/// caught here. Candidate URLs are fetched as images, so this is a resource-load
/// question rather than a script-execution one, and splitting srcset properly
/// means implementing its grammar (descriptors, whitespace, commas inside data
/// URLs). Recorded rather than half-implemented.
/// SECURITY: recover the URL from a BOUND url attribute so it can be scheme-checked.
///
/// `:href` and `x-bind:href` are on [`is_url_attr`]'s list, but their value is not a URL. It is
/// the JavaScript SOURCE Alpine will evaluate, and a slot that sits outside a JS string is
/// encoded as a JSON string literal on the way in. So a hostile value arrives here already
/// wrapped in quotes, and [`has_forbidden_url_scheme`] tests the leading character, finds `"`,
/// and reports no scheme:
///
/// ```text
/// template  <a :href="${ link }">          link = javascript:alert(1)
/// value     "javascript:alert(1)"          <- what finish_attr used to see, and pass
/// browser   <a :href='"javascript:alert(1)"'>
/// ```
///
/// Alpine then evaluates that literal back to the bare string and assigns it, so the guard was
/// absent on exactly the attributes it exists for. `href` and `href={ expr }` were both
/// neutralised correctly, which is what made this easy to miss: the bound form was the one place
/// it silently did not apply, and the protected forms are the ones people test.
///
/// Only a WHOLE literal is unwrapped. A concatenation, an identifier or a ternary has no value
/// that is knowable here, so it is left alone rather than guessed at: refusing to guess keeps
/// ordinary bindings like `:href="'/p/' + slug"` working, and those cannot carry an
/// attacker-chosen scheme in the leading position anyway.
fn bound_url_literal(name: &str, value: &str) -> Option<String> {
    if !is_js_attr(name) {
        return None;
    }
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })?;
    // A bare quote inside means this was never one literal (`"a" + x + "b"`), so the leading
    // characters are not the whole URL and unwrapping would be reading the wrong thing.
    if inner.contains('"') || inner.contains('\'') {
        return None;
    }
    Some(inner.to_string())
}

fn is_url_attr(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let target = lower
        .strip_prefix("x-bind:")
        .or_else(|| lower.strip_prefix(':'))
        .unwrap_or(&lower);
    matches!(
        target,
        "href"
            | "src"
            | "xlink:href"
            | "poster"
            | "formaction"
            | "action"
            | "cite"
            | "background"
            | "data"
            | "ping"
            | "srcset"
    )
}

/// SECURITY: reject URL schemes that execute script or smuggle content. The
/// scheme is resolved the way browsers do: ASCII control characters and spaces
/// (<= 0x20) are stripped anywhere in the URL BEFORE matching, so a value like
/// "\x01javascript:..." or "java\tscript:..." cannot bypass a prefix check.
///
/// Unicode whitespace (NBSP and friends) is stripped for DETECTION too, which is
/// deliberately STRICTER than a browser. A browser does not strip U+00A0, so
/// "\u{a0}javascript:x" is not actually a script URL to it - the scheme is
/// invalid and it degrades to a relative path. But relying on that means relying
/// on every current and future URL parser agreeing with us about which
/// characters are insignificant, and a false NEGATIVE here executes script. No
/// legitimate URL carries leading Unicode whitespace before a scheme, so being
/// stricter than the spec costs nothing and removes a whole evasion class.
/// Affects DETECTION only - a value that passes is emitted unchanged (attribute
/// escaped), never rewritten.
/// Characters that carry no meaning in a URL and must therefore not be able to
/// hide a scheme from [`has_forbidden_url_scheme`].
///
/// `char::is_whitespace` alone is NOT enough: U+FEFF (BOM / zero-width no-break
/// space) is Unicode category Cf, not White_Space, so it passes that test and
/// would leave a live `href="<BOM>javascript:alert(1)"`. The zero-width, joiner
/// and bidi-control ranges are the same class of problem, so they go too.
fn is_url_insignificant(c: char) -> bool {
    c <= '\u{20}'            // C0 controls and space
        || c.is_whitespace() // NBSP, line/paragraph separators, the rest of White_Space
        || c.is_control()    // C1 controls
        || matches!(c,
            '\u{200b}'..='\u{200f}'     // zero-width space/non-joiner/joiner + LRM/RLM
            | '\u{202a}'..='\u{202e}'   // bidi embedding / override
            | '\u{2060}'..='\u{2064}'   // word joiner + invisible operators
            | '\u{feff}'                // BOM / zero-width no-break space
        )
}

fn has_forbidden_url_scheme(value: &str) -> bool {
    let cleaned: String = value
        .chars()
        .filter(|c| !is_url_insignificant(*c))
        .collect::<String>()
        .to_ascii_lowercase();
    cleaned.starts_with("javascript:")
        || cleaned.starts_with("data:")
        || cleaned.starts_with("vbscript:")
}

/// SECURITY: sanitize a `{html ...}` attribute-spread fragment. The expression
/// can produce arbitrary attribute markup from untrusted data, so we parse it
/// with a quote-aware scanner into `name` / `name="value"` / `name=value`
/// attributes, DROP any with an unsafe name (see `is_safe_attr_name`) or a
/// script-scheme URL value, and re-emit every surviving attribute with an
/// attribute-escaped, double-quoted value. Anything that does not parse as a
/// whole attribute is dropped, never half-emitted.
fn sanitize_attr_spread(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut chars = raw.chars().peekable();

    'outer: while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }

        // Attribute name: everything up to whitespace or `=`.
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == '=' {
                break;
            }
            name.push(c);
            chars.next();
        }

        // Optional value. A quoted value may contain whitespace; it ends at
        // the matching quote. An unquoted value ends at whitespace.
        let mut value: Option<String> = None;
        let mut parse_ok = !name.is_empty();
        if chars.peek() == Some(&'=') {
            chars.next();
            match chars.peek() {
                Some(&q) if q == '"' || q == '\'' => {
                    chars.next();
                    let mut v = String::new();
                    let mut closed = false;
                    for c in chars.by_ref() {
                        if c == q {
                            closed = true;
                            break;
                        }
                        v.push(c);
                    }
                    if !closed {
                        // Unterminated quote: the rest of the fragment has no
                        // attribute structure; drop all of it.
                        break 'outer;
                    }
                    value = Some(v);
                }
                _ => {
                    let mut v = String::new();
                    while let Some(&c) = chars.peek() {
                        if c.is_whitespace() {
                            break;
                        }
                        v.push(c);
                        chars.next();
                    }
                    if v.is_empty() {
                        parse_ok = false;
                    }
                    value = Some(v);
                }
            }
        }

        if !parse_ok || !is_safe_attr_name(&name) {
            continue;
        }
        match value {
            None => out.push(name),
            Some(v) => {
                if has_forbidden_url_scheme(&v) {
                    continue;
                }
                out.push(format!(r#"{}="{}""#, name, escape_attr(&v)));
            }
        }
    }
    out.join(" ")
}

fn numeric_binop(lv: Value, rv: Value, op: impl Fn(f64, f64) -> f64) -> Value {
    let a = to_f64(&lv);
    let b = to_f64(&rv);
    let result = op(a, b);
    Value::Number(serde_json::Number::from_f64(result).unwrap_or_else(|| 0.into()))
}

fn to_f64(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        Value::Bool(true) => 1.0,
        Value::Bool(false) => 0.0,
        _ => 0.0,
    }
}

fn values_equal_deep(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .zip(y.as_f64())
            .map(|(a, b)| (a - b).abs() < f64::EPSILON)
            .unwrap_or(false),
        (Value::Array(x), Value::Array(y)) => x == y,
        (Value::Object(x), Value::Object(y)) => x == y,
        _ => false,
    }
}

fn compare_values(a: &Value, b: &Value, expected: std::cmp::Ordering) -> Value {
    let ord = match (a, b) {
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&y.as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    };
    Value::Bool(ord == expected)
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' | ':' => out.push(ch),
            c => {
                let mut buf = [0u8; 4];
                let bytes = c.encode_utf8(&mut buf).as_bytes();
                for b in bytes {
                    out.push('%');
                    out.push(
                        char::from_digit((b >> 4) as u32, 16)
                            .unwrap_or('0')
                            .to_ascii_uppercase(),
                    );
                    out.push(
                        char::from_digit((b & 0xf) as u32, 16)
                            .unwrap_or('0')
                            .to_ascii_uppercase(),
                    );
                }
            }
        }
    }
    out
}

fn build_import_map(frontmatter: Option<&Frontmatter>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(fm) = frontmatter {
        for import in &fm.imports {
            if let Import::Default { path, alias } = import {
                let name = alias
                    .as_deref()
                    .or_else(|| path.rsplit('/').next())
                    .unwrap_or(path.as_str())
                    .to_string();
                map.insert(name, path.clone());
            }
        }
    }
    map
}

#[must_use]
pub fn render(doc: &Document, context: &Value, mode: BuildMode) -> (String, ErrorAccumulator) {
    render_with_options(doc, context, &RenderOptions::new(mode))
}

#[must_use]
pub fn render_with_registry(
    doc: &Document,
    context: &Value,
    mode: BuildMode,
    file_registry: Arc<HashMap<String, String>>,
) -> (String, ErrorAccumulator) {
    render_with_options(
        doc,
        context,
        &RenderOptions::new(mode).with_registry(file_registry),
    )
}

/// Render with an explicit [`RenderOptions`], which is the only way to set a non-default
/// [`MemoryBudget`]. The three older entry points delegate here with the default budget.
#[must_use]
pub fn render_with_options(
    doc: &Document,
    context: &Value,
    options: &RenderOptions,
) -> (String, ErrorAccumulator) {
    let mut renderer = Renderer::with_budget(doc.file.clone(), options.mode, options.budget);
    renderer.file_registry = options.file_registry.clone();
    let html = renderer.render(doc, context);
    (html, renderer.diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use serde_json::json;

    fn render_template(src: &str, ctx: Value) -> String {
        let (doc, diag) = parse(src, "test.slurp").expect("lex");
        assert!(!diag.has_errors(), "Parse errors: {:?}", diag.errors);
        let (html, _render_diag) = render(&doc, &ctx, BuildMode::Development);
        html
    }

    fn render_with_files(src: &str, ctx: Value, files: &[(&str, &str)]) -> String {
        let registry: HashMap<String, String> = files
            .iter()
            .map(|(p, s)| (p.to_string(), s.to_string()))
            .collect();
        let (doc, diag) = parse(src, "test.slurp").expect("lex");
        assert!(!diag.has_errors(), "Parse errors: {:?}", diag.errors);
        let (html, _) =
            render_with_registry(&doc, &ctx, BuildMode::Development, Arc::new(registry));
        html
    }

    #[test]
    fn test_render_text() {
        let html = render_template("hello world", json!({}));
        assert!(html.contains("hello"));
    }

    #[test]
    fn include_chain_is_render_depth_limited_not_stack_overflow() {
        // A -> B -> C -> ... chain of component includes. Each file parses
        // shallow, so the parser's per-file depth cap does NOT bound it; only the
        // renderer's own depth guard does. Without it, a long chain overflows the
        // worker-thread stack. With it, the chain renders (truncated) and stays
        // bounded.
        let n = 400;
        let mut files: Vec<(String, String)> = (0..n)
            .map(|i| {
                (
                    format!("@components/c{i}"),
                    format!("---\nusing \"@components/c{}\" as C\n---\n<C />", i + 1),
                )
            })
            .collect();
        files.push((format!("@components/c{n}"), "end".to_string()));
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let page = "---\nusing \"@components/c0\" as C\n---\n<C />";
        let html = render_with_files(page, json!({}), &refs);
        assert!(
            html.len() < 100_000,
            "include chain not bounded: {} bytes",
            html.len()
        );
    }

    #[test]
    fn output_budget_charges_structural_markup() {
        // A loop body of attribute-less, text-less elements emits only structural
        // bytes (the open/close tags). Those must be charged against the output
        // budget; otherwise this renders to ~24 MB, well past MAX_OUTPUT_BYTES.
        let src = "{repeat 1000}{repeat 1000}<div><span></span></div>{/repeat}{/repeat}";
        let (doc, diag) = parse(src, "test.slurp").expect("lex");
        assert!(!diag.has_errors(), "Parse errors: {:?}", diag.errors);
        let (html, render_diag) = render(&doc, &json!({}), BuildMode::Development);
        assert!(
            html.len() <= MAX_OUTPUT_BYTES + 1024,
            "output not bounded by the budget: {} bytes",
            html.len()
        );
        assert!(
            render_diag
                .warnings
                .iter()
                .any(|w| w.message.contains("output budget")),
            "expected an output-budget diagnostic once the cap is hit"
        );
    }

    #[test]
    fn test_component_sees_globals_props_shadow() {
        let page = "---\nusing \"@components/Logo\" as Logo\nprops {}\n---\n<Logo /><Logo name=\"Passed\" />";
        let html = render_with_files(
            page,
            json!({"site": {"name": "My Shop"}, "name": "Global"}),
            &[("@components/Logo", "<b>${site.name}:${name}</b>")],
        );
        assert_eq!(
            html.trim_start(),
            "<b>My Shop:Global</b><b>My Shop:Passed</b>"
        );
    }

    #[test]
    fn test_layout_sees_globals() {
        let page = "---\nusing \"@layouts/base\" as BaseLayout\nprops {}\n---\n<layout src=\"@layouts/base\">body</layout>";
        let html = render_with_files(
            page,
            json!({"theme": {"settings": {"accent": "indigo"}}}),
            &[(
                "@layouts/base",
                "<title>${theme.settings.accent}</title><slot />",
            )],
        );
        assert_eq!(html.trim_start(), "<title>indigo</title>body");
    }

    // ── {blocks}: theme-block file rendering ────────────────────────────────────────────────

    const HEADING_BLOCK: &str = "---
block { name: \"Heading\" }
---
<h2>${ block.settings.text }</h2>";

    #[test]
    fn blocks_renders_each_child_from_its_own_file() {
        let section = "{blocks}";
        let ctx = json!({ "section": { "blocks": [
            { "id": "b1", "type": "heading", "settings": { "text": "First" } },
            { "id": "b2", "type": "heading", "settings": { "text": "Second" } }
        ]}});
        let html = render_with_files(section, ctx, &[("@blocks/heading", HEADING_BLOCK)]);
        assert!(html.contains("<h2>First</h2>"), "{html}");
        assert!(html.contains("<h2>Second</h2>"), "{html}");
        // The editor needs each child addressable, and the wrapper must not affect layout.
        assert!(html.contains(r#"data-bs-block="b1""#), "{html}");
        assert!(html.contains("display:contents"), "{html}");
    }

    /// A block hosting `{blocks}` renders ITS children - this is how `group`/`columns` compose, and
    /// it works because the renderer prefers `block` over `section` when resolving the list.
    #[test]
    fn blocks_nest_through_block_files() {
        let group = "---
block { name: \"Group\" }
---
<div class=\"g\">{blocks}</div>";
        let ctx = json!({ "section": { "blocks": [
            { "id": "g1", "type": "group", "blocks": [
                { "id": "h1", "type": "heading", "settings": { "text": "Nested" } }
            ]}
        ]}});
        let html = render_with_files(
            "{blocks}",
            ctx,
            &[("@blocks/group", group), ("@blocks/heading", HEADING_BLOCK)],
        );
        assert!(html.contains("<div class=\"g\">"), "{html}");
        assert!(
            html.contains("<h2>Nested</h2>"),
            "outer block did not render its children: {html}"
        );
    }

    /// The block type arrives from untrusted saved state and is used as a registry path.
    #[test]
    fn a_hostile_block_type_cannot_escape_the_blocks_directory() {
        for evil in ["../layouts/shell", "..", "a/b", "heading?x", "", "he ading"] {
            let ctx = json!({ "section": { "blocks": [{ "id": "x", "type": evil }] } });
            let html = render_with_files(
                "{blocks}",
                ctx,
                &[
                    ("@blocks/heading", HEADING_BLOCK),
                    ("@layouts/shell", "PWNED"),
                ],
            );
            assert!(
                !html.contains("PWNED"),
                "escaped with type {evil:?}: {html}"
            );
        }
    }

    /// A block file that hosts its own type would recurse until the iteration budget ran out. A
    /// block cannot contain itself, so skipping is the right answer rather than a slow failure.
    #[test]
    fn a_self_hosting_block_does_not_recurse_forever() {
        let recursive = "---
block { name: \"Loop\" }
---
<i>{blocks}</i>";
        let ctx = json!({ "section": { "blocks": [
            { "id": "l1", "type": "loop", "blocks": [{ "id": "l2", "type": "loop" }] }
        ]}});
        let html = render_with_files("{blocks}", ctx, &[("@blocks/loop", recursive)]);
        assert!(html.contains("<i>"), "{html}");
        assert!(html.matches("<i>").count() <= 2, "recursed too far: {html}");
    }

    #[test]
    fn blocks_is_inert_without_children_or_a_file() {
        // No section in context at all.
        assert_eq!(render_with_files("{blocks}", json!({}), &[]).trim(), "");
        // A child whose file is not in the registry is skipped, not rendered as a placeholder.
        let ctx = json!({ "section": { "blocks": [{ "id": "b", "type": "missing" }] } });
        assert_eq!(render_with_files("{blocks}", ctx, &[]).trim(), "");
    }

    #[test]
    fn test_sections_html_not_leaked_into_components() {
        let page = "---\nusing \"@components/C\" as C\nprops {}\n---\n{sections}<C />";
        let mut ctx = json!({});
        ctx[SECTIONS_HTML_KEY] = json!("<section>S</section>");
        let html = render_with_files(page, ctx, &[("@components/C", "[{sections}]")]);
        assert_eq!(html.trim_start(), "<section>S</section>[]");
    }

    #[test]
    fn test_render_expression() {
        let html = render_template("${name}", json!({"name": "Alice"}));
        assert_eq!(html, "Alice");
    }

    #[test]
    fn test_render_xss_escaped() {
        let html = render_template("${msg}", json!({"msg": "<script>alert(1)</script>"}));
        assert!(!html.contains("<script>"), "XSS not escaped: {html}");
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn test_render_raw_html_not_escaped() {
        let html = render_template("{html content}", json!({"content": "<b>bold</b>"}));
        assert!(html.contains("<b>bold</b>"));
    }

    #[test]
    fn test_render_html_sanitize_strips_script() {
        // {html expr sanitize} syntax: expression before "sanitize" keyword
        let html = render_template(
            "{html content sanitize}",
            json!({"content": "<script>evil()</script>safe"}),
        );
        assert!(!html.contains("<script"), "script not stripped: {html}");
        assert!(html.contains("safe"), "safe content missing: {html}");
    }

    #[test]
    fn test_render_if_true() {
        let html = render_template("{if show}yes{/if}", json!({"show": true}));
        assert_eq!(html.trim(), "yes");
    }

    #[test]
    fn test_render_if_false() {
        let html = render_template("{if show}yes{/if}", json!({"show": false}));
        assert!(!html.contains("yes"));
    }

    #[test]
    fn test_render_if_else() {
        let html = render_template("{if flag}a{else}b{/if}", json!({"flag": false}));
        assert!(html.contains("b"));
        assert!(!html.contains("a"));
    }

    #[test]
    fn test_render_each() {
        let html = render_template("{each x in items}${x}{/each}", json!({"items": [1, 2, 3]}));
        assert!(html.contains("1"));
        assert!(html.contains("2"));
        assert!(html.contains("3"));
    }

    #[test]
    fn test_render_each_empty() {
        let html = render_template(
            "{each x in items}${x}{empty}nothing{/each}",
            json!({"items": []}),
        );
        assert!(html.contains("nothing"));
    }

    #[test]
    fn test_render_match() {
        let html = render_template(
            r#"{match status}"open" -> open"closed" -> closed{/match}"#,
            json!({"status": "open"}),
        );
        assert!(html.contains("open"));
    }

    #[test]
    fn test_render_match_wildcard() {
        let html = render_template(
            r#"{match status}"open" -> open_ -> other{/match}"#,
            json!({"status": "unknown"}),
        );
        assert!(html.contains("other"));
    }

    #[test]
    fn test_render_repeat() {
        let html = render_template("{repeat 3}x{/repeat}", json!({}));
        assert_eq!(html, "xxx");
    }

    #[test]
    fn test_render_element() {
        let html = render_template(r#"<div class="container">hi</div>"#, json!({}));
        assert!(html.contains(r#"class="container""#));
        assert!(html.contains("hi"));
    }

    #[test]
    fn test_render_safe_navigation_null() {
        let html = render_template("${user.address.city}", json!({"user": null}));
        assert_eq!(html, "");
    }

    #[test]
    fn test_render_safe_navigation_missing() {
        let html = render_template("${a.b.c}", json!({}));
        assert_eq!(html, "");
    }

    #[test]
    fn test_render_ternary() {
        let html = render_template("${x ? \"yes\" : \"no\"}", json!({"x": true}));
        assert_eq!(html, "yes");
    }

    #[test]
    fn test_render_binary_add() {
        let html = render_template("${a + b}", json!({"a": 3, "b": 4}));
        assert_eq!(html, "7");
    }

    #[test]
    fn test_render_filter_upper() {
        let html = render_template("${name | upper}", json!({"name": "alice"}));
        assert_eq!(html, "ALICE");
    }

    #[test]
    fn test_render_filter_truncate() {
        let html = render_template("${text | truncate(5)}", json!({"text": "hello world"}));
        assert!(html.starts_with("hello"));
        assert!(html.contains("..."));
    }

    #[test]
    fn test_render_loop_auto_vars() {
        let html = render_template(
            "{each x in items}${loop.index}{/each}",
            json!({"items": ["a", "b", "c"]}),
        );
        assert!(html.contains("0"));
        assert!(html.contains("1"));
        assert!(html.contains("2"));
    }

    #[test]
    fn test_render_debug_dev_mode() {
        let html = render_template("{debug name}", json!({"name": "test"}));
        assert!(html.contains("data-slurp-debug"));
    }

    #[test]
    fn test_render_debug_prod_mode() {
        let (doc, _) = parse("{debug name}", "test.slurp").unwrap();
        let (html, _) = render(&doc, &json!({"name": "test"}), BuildMode::Production);
        assert!(!html.contains("data-slurp-debug"));
    }

    #[test]
    fn test_render_whitespace_agnostic() {
        // 2-space indented
        let h1 = render_template("  ${name}", json!({"name": "X"}));
        // 4-space indented
        let h2 = render_template("    ${name}", json!({"name": "X"}));
        // tab indented
        let h3 = render_template("\t${name}", json!({"name": "X"}));
        // All should contain "X"
        assert!(h1.contains("X"));
        assert!(h2.contains("X"));
        assert!(h3.contains("X"));
    }

    #[test]
    fn test_render_unary_not() {
        let html = render_template("${!active}", json!({"active": false}));
        assert_eq!(html, "true");
    }

    #[test]
    fn test_render_image_component() {
        let html = render_template(
            r#"<Image src="photo.jpg" width={800} height={600} alt="A photo" />"#,
            json!({}),
        );
        assert!(html.contains("<picture>"));
        assert!(html.contains("avif"));
        assert!(html.contains("webp"));
        assert!(html.contains("loading=\"lazy\""));
    }

    #[test]
    fn test_render_sections_outputs_injected_html() {
        assert_eq!(SECTIONS_HTML_KEY, "__slurp_sections_html");
        let html = render_template(
            "<main>{sections}</main>",
            json!({ "__slurp_sections_html": "<section>hero</section>" }),
        );
        assert_eq!(html, "<main><section>hero</section></main>");
    }

    #[test]
    fn test_render_sections_empty_without_host_injection() {
        let html = render_template("<main>{sections}</main>", json!({}));
        assert_eq!(html, "<main></main>");
    }

    #[test]
    fn test_sanitize_attr_spread_quoted_value_with_spaces() {
        let out = sanitize_attr_spread(r#"title="hello world" class='a b'"#);
        assert_eq!(out, r#"title="hello world" class="a b""#);
    }

    #[test]
    fn test_sanitize_attr_spread_drops_unparseable_not_half_emits() {
        // "name = " with spaces around '=' must not emit a mangled fragment.
        let out = sanitize_attr_spread(r#"name = "v""#);
        assert!(!out.contains('='), "half-emitted attribute: {out}");
        // An unterminated quote drops the rest of the fragment entirely.
        let out = sanitize_attr_spread(r#"class="a b title=x"#);
        assert_eq!(out, "");
    }

    #[test]
    fn test_sanitize_attr_spread_blocks_script_schemes() {
        assert_eq!(sanitize_attr_spread(r#"href="javascript:alert(1)""#), "");
        assert_eq!(sanitize_attr_spread(r#"href="vbscript:msgbox(1)""#), "");
        assert_eq!(sanitize_attr_spread(r#"href="data:text/html,x""#), "");
        // Control characters before or inside the scheme must not bypass the
        // check (browsers strip \x00-\x20 before resolving the scheme).
        assert_eq!(
            sanitize_attr_spread("href=\"\u{1}javascript:alert(1)\""),
            ""
        );
        assert_eq!(sanitize_attr_spread("href=\"java\tscript:alert(1)\""), "");
        // Safe URLs pass through.
        let out = sanitize_attr_spread(r#"href="https://example.com/a b""#);
        assert_eq!(out, r#"href="https://example.com/a b""#);
    }

    #[test]
    fn test_sanitize_attr_spread_blocks_on_handlers_keeps_alpine() {
        // `on*` native handlers execute the value as script on any element and
        // are ALWAYS dropped, whoever authored the spread - including when
        // smuggled through an Alpine attribute-bind alias.
        assert_eq!(sanitize_attr_spread(r#"onclick="alert(1)""#), "");
        assert_eq!(sanitize_attr_spread(r#"onerror="alert(1)""#), "");
        assert_eq!(sanitize_attr_spread(r#":onclick="alert(1)""#), "");
        assert_eq!(sanitize_attr_spread(r#"x-bind:onclick="alert(1)""#), "");
        // Case-insensitive: the browser lowercases attr names before Alpine, so
        // an uppercased prefix/name must not dodge the on*/sink checks.
        assert_eq!(sanitize_attr_spread(r#"X-BIND:onclick="alert(1)""#), "");
        assert_eq!(sanitize_attr_spread(r#":ONCLICK="alert(1)""#), "");
        assert_eq!(sanitize_attr_spread(r#"ONCLICK="alert(1)""#), "");
        assert_eq!(sanitize_attr_spread(r#"X-HTML="payload""#), "");
        // Alpine JS/innerHTML sinks with no event gate stay blocked (the theme
        // never forwards these through `attrs`).
        assert_eq!(sanitize_attr_spread(r#"x-html="payload""#), "");
        assert_eq!(sanitize_attr_spread(r#"x-data="{}""#), "");
        assert_eq!(sanitize_attr_spread(r#"x-init="fetch(x)""#), "");
        assert_eq!(sanitize_attr_spread(r#"x-effect="run()""#), "");
        // Event/model/bind directives are theme-authored (forwarded via a
        // component `attrs` prop) at the same trust level as direct markup, so
        // they SURVIVE - this is what lets Button/Input/Select forward
        // @click/x-model/:disabled.
        assert_eq!(
            sanitize_attr_spread(r#"@click="startCheckout()""#),
            r#"@click="startCheckout()""#
        );
        assert_eq!(
            sanitize_attr_spread(r#"x-model="email""#),
            r#"x-model="email""#
        );
        assert_eq!(
            sanitize_attr_spread(r#":disabled="loading""#),
            r#":disabled="loading""#
        );
        // x-on:click is an Alpine event LISTENER, not a native handler - allowed.
        assert_eq!(
            sanitize_attr_spread(r#"x-on:click="submit()""#),
            r#"x-on:click="submit()""#
        );
        // Plain data/aria attributes survive.
        let out = sanitize_attr_spread(r#"data-id="5" aria-label="ok" disabled"#);
        assert_eq!(out, r#"data-id="5" aria-label="ok" disabled"#);
    }

    /// REGRESSION (panic): `is_safe_attr_name` tested the `on` prefix with `&bind_target[..2]`.
    ///
    /// That is a BYTE index. The name arrives from RENDERED DATA - the whole point of a
    /// `{html attrs}` spread - so it can begin with any character, and a leading multi-byte one
    /// put the slice boundary inside a UTF-8 sequence: `byte index 2 is not a char boundary`,
    /// a panic on the render path from a single untrusted value.
    ///
    /// The charset allow-list that would have rejected such a name runs at the END of this
    /// function, AFTER the prefix test, so it never got the chance to filter the input. Ordering
    /// is what made a "surely unreachable" slice reachable, which is why this test drives the
    /// predicate directly rather than trusting the caller to have cleaned the name.
    #[test]
    fn test_is_safe_attr_name_never_slices_a_multibyte_name() {
        for name in [
            "\u{20ac}=1",       // the reported reproducer: a leading euro sign
            "\u{e9}",           // one 2-byte character, shorter than the old slice
            "\u{e9}n",          // 2-byte lead, then a byte the old slice would have split
            "\u{4f60}\u{597d}", // 3-byte characters
            "\u{1f600}nclick",  // 4-byte lead followed by the rest of "onclick"
            "\u{feff}onclick",  // a BOM hiding a real handler name
            "\u{a0}onerror",    // NBSP doing the same
            ":\u{20ac}",        // multi-byte immediately after an Alpine bind prefix
            "x-bind:\u{20ac}",  // and after the long form, where the strip leaves it leading
            "",
        ] {
            // The assertion is that this RETURNS. Its verdict is checked below; a panic here
            // is the failure being pinned.
            let allowed = is_safe_attr_name(name);
            // Nothing in this list is a legitimate attribute name, so every one must also be
            // refused by the charset gate - a name that panicked and a name that is admitted
            // are two different bugs, and fixing the first must not create the second.
            assert!(!allowed, "a hostile attribute name was admitted: {name:?}");
        }

        // A name SHORTER than the old two-byte slice is the other end of the same bug: `&x[..2]`
        // on a one-character name panics on the length, not on the boundary. `o` is a legal
        // attribute name and must come back allowed, not merely un-panicked.
        assert!(is_safe_attr_name("o"), "a one-character name was refused");

        // The prefix rule itself still works, in every alias and casing, which is what the
        // slice was there to do.
        for blocked in [
            "onclick",
            "ONCLICK",
            "onerror",
            ":onclick",
            ":ONCLICK",
            "x-bind:onclick",
            "X-BIND:onclick",
            "on",
        ] {
            assert!(
                !is_safe_attr_name(blocked),
                "a native event handler was admitted: {blocked:?}"
            );
        }

        // ...and ordinary names are still allowed, so the fix did not just refuse everything.
        for allowed in [
            "class",
            "data-id",
            "aria-label",
            "@click",
            "x-on:click",
            ":disabled",
            "o",
            "off",
        ] {
            assert!(
                is_safe_attr_name(allowed),
                "a legitimate attribute name was refused: {allowed:?}"
            );
        }

        // KNOWN and deliberate: the rule is a PREFIX test, so it also refuses names that merely
        // begin with "on" without being handlers. Pinned rather than fixed - it fails closed, no
        // HTML attribute or Alpine directive is named this way, and the alternative (a list of
        // every `on*` event, which grows with the platform) fails OPEN on whatever is missing.
        for over_broad in ["one", "only", "onward"] {
            assert!(
                !is_safe_attr_name(over_broad),
                "the prefix rule is documented as over-broad; {over_broad:?} changing means the \
                 rule changed"
            );
        }
    }

    /// The same panic, reached the way a real request reaches it: a value rendered through a
    /// `{html attrs}` spread. A unit test on the predicate proves the predicate; this proves
    /// that the render path really does hand it unfiltered data.
    #[test]
    fn test_attr_spread_with_multibyte_names_renders_instead_of_panicking() {
        for raw in [
            "\u{20ac}=1",
            "\u{4f60}\u{597d}=\"x\"",
            "\u{1f600}nclick=\"alert(1)\"",
            "\u{feff}onclick=\"alert(1)\"",
            "data-ok=\"1\" \u{20ac}=2",
        ] {
            let out = sanitize_attr_spread(raw);
            assert!(
                !out.contains('\u{20ac}') && !out.to_ascii_lowercase().contains("onclick"),
                "a hostile name survived the spread: {raw:?} -> {out:?}"
            );
        }
        // The well-formed attribute beside a dropped one is still emitted: the spread drops
        // per-attribute, it does not abandon the whole fragment.
        assert_eq!(
            sanitize_attr_spread("data-ok=\"1\" \u{20ac}=2"),
            r#"data-ok="1""#
        );
    }

    #[test]
    fn test_interpolated_attr_with_literal_backtick() {
        let html = render_template(
            "<div title=\"cost is ${price}` per unit\"></div>",
            json!({"price": 5}),
        );
        // Backticks are entity-encoded in attribute values (they can break out of a JS template
        // literal in Alpine attributes); the browser still renders `&#96;` as a literal backtick.
        assert!(html.contains("cost is 5&#96; per unit"), "got: {html}");
    }

    #[test]
    fn test_output_budget_stops_render() {
        // 1M iterations x 32 bytes = ~32MB, which must be cut at the 16MB cap.
        let body = "a".repeat(32);
        let src = format!("{{repeat 1000}}{{repeat 1000}}{body}{{/repeat}}{{/repeat}}");
        let (doc, _) = parse(&src, "test.slurp").expect("lex");
        let (html, diag) = render(&doc, &json!({}), BuildMode::Development);
        assert!(html.len() <= MAX_OUTPUT_BYTES + body.len());
        assert!(
            diag.warnings
                .iter()
                .any(|w| w.message.contains("output budget"))
        );
    }

    /// A truncated loop must be REPORTED in production, not only in development.
    ///
    /// Truncation leaves nothing behind in the page - the rows are simply absent - so a
    /// production render that says nothing is a build reporting success while dropping a third of
    /// a list. The truncation itself is correct and must not move: this asserts the count as well
    /// as the warning.
    #[test]
    fn an_over_cap_loop_warns_in_production_and_still_truncates() {
        let items: Vec<Value> = (0..1500).map(|i| Value::Number(i.into())).collect();
        let src = "{each item in items}<span>${item}</span>{/each}";
        let (doc, _) = parse(src, "test.slurp").expect("lex");

        for mode in [BuildMode::Production, BuildMode::Development] {
            let (html, diag) = render(&doc, &json!({ "items": items }), mode);
            assert_eq!(
                html.matches("<span>").count(),
                MAX_ITERATIONS,
                "truncation moved in {mode:?}"
            );
            assert!(!diag.has_errors(), "truncation is advisory, not an error");
            assert!(
                diag.warnings
                    .iter()
                    .any(|w| w.code == ErrorCode::IterationLimitExceeded
                        && w.message.contains("1500")),
                "no truncation warning in {mode:?}: {:?}",
                diag.warnings
            );
        }
    }

    /// The global iteration budget warns once per render, in every mode. "Once" is the part worth
    /// pinning: the latch lives in an equality test that a refactor can easily turn into `>=`,
    /// which would then warn on every charge past the budget.
    #[test]
    fn the_total_iteration_budget_warns_exactly_once_in_production() {
        let src = "{repeat 1000}{repeat 1000}{repeat 10}{/repeat}{/repeat}{/repeat}";
        let (doc, _) = parse(src, "test.slurp").expect("lex");
        let (_html, diag) = render(&doc, &json!({}), BuildMode::Production);
        let budget_warnings = diag
            .warnings
            .iter()
            .filter(|w| w.message.contains("iteration budget"))
            .count();
        assert_eq!(budget_warnings, 1, "got: {:?}", diag.warnings);
    }

    #[test]
    fn test_component_expansion_charges_iteration_budget() {
        // Exhaust the iteration budget with cheap empty loops, then verify a
        // component expansion refuses to render (fan-out DoS guard).
        let page = "---\nusing \"@components/A\" as A\nprops {}\n---\n{repeat 1000}{repeat 1000}{repeat 10}{/repeat}{/repeat}{/repeat}<A />";
        let html = render_with_files(page, json!({}), &[("@components/A", "MARKER")]);
        assert!(
            !html.contains("MARKER"),
            "component rendered past the budget"
        );
    }

    #[test]
    fn test_render_nested_each() {
        let html = render_template(
            "{each cat in categories}{each item in cat.items}${item}{/each}{/each}",
            json!({"categories": [{"items": ["a", "b"]}, {"items": ["c"]}]}),
        );
        assert!(html.contains("a"));
        assert!(html.contains("b"));
        assert!(html.contains("c"));
    }
}
