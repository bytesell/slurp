pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod renderer;
pub mod sections;
mod stack;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

pub use ast::Document;
pub use error::{CompileError, CompileResult, ErrorAccumulator, ErrorCode, Severity};
pub use parser::parse;
pub use renderer::sanitize::sanitize_html;
pub use renderer::{
    BuildMode, MemoryBudget, RenderOptions, SECTIONS_HTML_KEY, render, render_with_options,
    render_with_registry, section_group_html_key,
};
pub use sections::{
    BlockCatalog, catalog_entry, merged_section_value, merged_section_value_with, render_section,
    render_section_with_options, render_section_with_registry,
    render_section_with_registry_and_blocks, theme_block_type,
};

/// Extract the `section {}` schema declared in a template's frontmatter as
/// editor-facing JSON. Returns `Ok((None, diag))` when the template declares no
/// section schema; parse diagnostics are returned alongside so callers can
/// reject invalid templates.
pub fn extract_schema(
    source: &str,
    file: impl Into<String>,
) -> CompileResult<(Option<serde_json::Value>, ErrorAccumulator)> {
    let (doc, diag) = parse(source, file)?;
    let schema = doc
        .frontmatter
        .as_ref()
        .and_then(|f| f.section.as_ref())
        .and_then(|s| serde_json::to_value(s).ok());
    Ok((schema, diag))
}

/// Extract the `block {}` schema a theme-block file (`blocks/<name>.slurp`) declares, as
/// editor-facing JSON. `Ok((None, diag))` when the file declares no block schema - which is how a
/// stray file under `blocks/` is ignored rather than rejected.
pub fn extract_block_schema(
    source: &str,
    file: impl Into<String>,
) -> CompileResult<(Option<serde_json::Value>, ErrorAccumulator)> {
    let (doc, diag) = parse(source, file)?;
    let schema = doc
        .frontmatter
        .as_ref()
        .and_then(|f| f.block.as_ref())
        .and_then(|s| serde_json::to_value(s).ok());
    Ok((schema, diag))
}

/// Returns `(variable_name, evaluated_url)` for every `{fetch}` node in the document.
/// Used by the dev server to pre-fetch API data before SSR rendering.
pub fn eval_fetch_urls(doc: &ast::Document, context: &serde_json::Value) -> Vec<(String, String)> {
    let mut r = renderer::Renderer::new("", renderer::BuildMode::Development);
    let scope = renderer::scope::Scope::root(context);
    let mut results = Vec::new();
    collect_fetch_urls(&doc.children, &mut r, &scope, &mut results, 0);
    results
}

/// Defence-in-depth ceiling for the `{fetch}` walk. It mirrors `MAX_CHECK_DEPTH`, and exists
/// for the same reason: the parser's `MAX_DEPTH` already bounds any document IT produced, but
/// `ast::Document` is a PUBLIC type and `eval_fetch_urls` is a PUBLIC function, so a
/// hand-constructed or deserialized document reaches this walk without ever having passed the
/// parser. This was the only recursive AST walk in the crate with neither its own ceiling nor
/// `stack::with_stack`, and a stack overflow is an uncatchable abort, not a failed render.
const MAX_FETCH_WALK_DEPTH: usize = 512;

fn collect_fetch_urls(
    nodes: &[ast::Node],
    r: &mut renderer::Renderer,
    scope: &renderer::scope::Scope<'_>,
    out: &mut Vec<(String, String)>,
    depth: usize,
) {
    if depth >= MAX_FETCH_WALK_DEPTH {
        return;
    }
    for node in nodes {
        match node {
            ast::Node::Fetch(f) => {
                let val = r.eval_expr(&f.source, scope);
                let url = renderer::filters::value_to_string(&val);
                if !url.is_empty() {
                    out.push((f.name.clone(), url));
                }
                collect_fetch_urls(&f.body, r, scope, out, depth + 1);
                if let Some(b) = &f.loading_branch {
                    collect_fetch_urls(b, r, scope, out, depth + 1);
                }
                if let Some(b) = &f.error_branch {
                    collect_fetch_urls(b, r, scope, out, depth + 1);
                }
                if let Some(b) = &f.empty_branch {
                    collect_fetch_urls(b, r, scope, out, depth + 1);
                }
            }
            ast::Node::Element(n) => collect_fetch_urls(&n.children, r, scope, out, depth + 1),
            ast::Node::Component(n) => collect_fetch_urls(&n.children, r, scope, out, depth + 1),
            ast::Node::Layout(n) => collect_fetch_urls(&n.children, r, scope, out, depth + 1),
            ast::Node::If(n) => {
                collect_fetch_urls(&n.then_branch, r, scope, out, depth + 1);
                for (_, branch) in &n.else_if_branches {
                    collect_fetch_urls(branch, r, scope, out, depth + 1);
                }
                if let Some(b) = &n.else_branch {
                    collect_fetch_urls(b, r, scope, out, depth + 1);
                }
            }
            ast::Node::Each(n) => {
                collect_fetch_urls(&n.body, r, scope, out, depth + 1);
                if let Some(b) = &n.empty_branch {
                    collect_fetch_urls(b, r, scope, out, depth + 1);
                }
            }
            ast::Node::Repeat(n) => collect_fetch_urls(&n.body, r, scope, out, depth + 1),
            ast::Node::Try(n) => {
                collect_fetch_urls(&n.body, r, scope, out, depth + 1);
                collect_fetch_urls(&n.error_branch, r, scope, out, depth + 1);
            }
            ast::Node::Head(nodes) => collect_fetch_urls(nodes, r, scope, out, depth + 1),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompileOptions {
    pub context_json: Option<String>,
    pub is_middleware: bool,
    pub virtual_files: Option<std::collections::HashMap<String, String>>,
    pub entry_file: Option<String>,
    /// Per-render memory caps. Defaults to [`MemoryBudget::DEFAULT`], so a caller
    /// that never mentions it behaves exactly as before.
    pub budget: MemoryBudget,
}

pub fn compile_template(source: &str, options: CompileOptions) -> Result<String, CompileError> {
    compile_template_inner(source, "template", options)
}

pub fn compile_with_options(options: CompileOptions) -> Result<String, CompileError> {
    let entry = options
        .entry_file
        .clone()
        .unwrap_or_else(|| "entry.slurp".to_string());

    let source = if let Some(files) = &options.virtual_files {
        detect_circular_imports(files, &entry)?;
        files
            .get(&entry)
            .map(|s| s.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };

    compile_template_inner(&source, &entry, options)
}

fn compile_template_inner(
    source: &str,
    file: &str,
    options: CompileOptions,
) -> Result<String, CompileError> {
    // Silently falls back to `{}` on invalid JSON; callers are expected to
    // validate the JSON themselves before compiling.
    let context = options
        .context_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));

    let (doc, parse_diag) = parse(source, file)?;

    security_check(&doc, file, options.is_middleware)?;

    if let Some(first_err) = parse_diag.errors.first() {
        return Err(first_err.clone());
    }

    let (html, render_diag) = render_with_options(
        &doc,
        &context,
        &RenderOptions::new(BuildMode::Production).with_budget(options.budget),
    );

    if let Some(first_err) = render_diag.errors.first() {
        return Err(first_err.clone());
    }

    Ok(html)
}

/// SECURITY: defense-in-depth recursion ceiling for the security walk. The AST
/// is already depth-bounded by the parser's `MAX_DEPTH`, but the walk is a
/// separate recursion over untrusted structure; bail cleanly rather than risk a
/// stack overflow if that invariant ever changes.
const MAX_CHECK_DEPTH: usize = 512;

/// SECURITY: the walk carries whether it is inside a `<script>` BODY, because
/// that changes what an expression is allowed to be. See
/// `renderer::Renderer::render_script_expression` for the whole argument; in
/// short, a raw-text element is handed to the JS engine WITHOUT decoding
/// character references, so the entity escaping the text path applies is inert
/// there and every JS metacharacter except `<` passes through untouched.
#[derive(Debug, Clone, Copy)]
struct CheckCtx {
    is_middleware: bool,
    in_script: bool,
}

fn security_check(
    doc: &ast::Document,
    file: &str,
    is_middleware: bool,
) -> Result<(), CompileError> {
    check_nodes(
        &doc.children,
        file,
        CheckCtx {
            is_middleware,
            in_script: false,
        },
        0,
    )
}

/// Run the compile-time security checks over an already-parsed document, without
/// rendering it.
///
/// This is the same walk `compile` and `compile_with_registry` run, exposed so a
/// caller that only wants to VALIDATE (`slurp validate`, an editor, a publish
/// gate) enforces exactly what a build enforces. Parsing alone does not: rules
/// like "an unfiltered `${ }` may not appear in a `<script>` body", the
/// secret-env block and the middleware-scope guard all live here, so a
/// parse-only check reports a clean file that then fails at build time.
///
/// `is_middleware` must match the value the eventual compile will use, because
/// the rules genuinely differ (`request.*` and `{redirect}` are legal only in
/// middleware, and everything else is illegal there). Middleware is a compile
/// OPTION rather than a path convention, so a caller that has no way to know -
/// the CLI, for instance - passes `false`, which is what both `compile` and
/// `compile_with_registry` do.
pub fn check_security(
    doc: &ast::Document,
    file: &str,
    is_middleware: bool,
) -> Result<(), CompileError> {
    security_check(doc, file, is_middleware)
}

fn check_nodes(
    nodes: &[ast::Node],
    file: &str,
    ctx: CheckCtx,
    depth: usize,
) -> Result<(), CompileError> {
    for node in nodes {
        check_node(node, file, ctx, depth)?;
    }
    Ok(())
}

fn check_node(
    node: &ast::Node,
    file: &str,
    ctx: CheckCtx,
    depth: usize,
) -> Result<(), CompileError> {
    use ast::Node;
    let is_middleware = ctx.is_middleware;
    if depth >= MAX_CHECK_DEPTH {
        return Err(CompileError::parse(
            file,
            0,
            0,
            format!("Maximum nesting depth ({MAX_CHECK_DEPTH}) exceeded"),
            ErrorCode::UnexpectedToken,
        ));
    }
    let depth = depth + 1;
    match node {
        Node::Expression(e) => {
            if ctx.in_script && !renderer::expr_has_script_filter(e) {
                return Err(CompileError::parse(
                    file,
                    e.line,
                    e.column,
                    renderer::SCRIPT_FILTER_REQUIRED,
                    ErrorCode::UnsafeScriptInterpolation,
                ));
            }
            check_expr(e, file, is_middleware)?
        }
        Node::RawHtml { expr, .. } => check_expr(expr, file, is_middleware)?,
        Node::If(n) => {
            check_expr(&n.condition, file, is_middleware)?;
            check_nodes(&n.then_branch, file, ctx, depth)?;
            for (cond, branch) in &n.else_if_branches {
                check_expr(cond, file, is_middleware)?;
                check_nodes(branch, file, ctx, depth)?;
            }
            if let Some(b) = &n.else_branch {
                check_nodes(b, file, ctx, depth)?;
            }
        }
        Node::Each(n) => {
            check_expr(&n.collection, file, is_middleware)?;
            check_nodes(&n.body, file, ctx, depth)?;
            if let Some(b) = &n.empty_branch {
                check_nodes(b, file, ctx, depth)?;
            }
        }
        Node::Match(n) => {
            check_expr(&n.value, file, is_middleware)?;
            for arm in &n.arms {
                check_nodes(&arm.body, file, ctx, depth)?;
            }
        }
        Node::Repeat(n) => {
            check_expr(&n.count, file, is_middleware)?;
            check_nodes(&n.body, file, ctx, depth)?;
        }
        Node::Fetch(n) => {
            check_expr(&n.source, file, is_middleware)?;
            check_nodes(&n.body, file, ctx, depth)?;
            // The three side branches RENDER (`{loading}` is in fact the default
            // server-side path, because the fetch variable is unresolved during SSR),
            // so they must be walked. Skipping them let a template leak
            // `env.SLURP_SECRET_*`, read `request.*` outside middleware, and smuggle a
            // `{redirect}` into a non-middleware file.
            if let Some(b) = &n.loading_branch {
                check_nodes(b, file, ctx, depth)?;
            }
            if let Some(b) = &n.error_branch {
                check_nodes(b, file, ctx, depth)?;
            }
            if let Some(b) = &n.empty_branch {
                check_nodes(b, file, ctx, depth)?;
            }
        }
        Node::Element(n) => {
            for attr in &n.attrs {
                check_attr_value(&attr.name, &attr.value, file, is_middleware)?;
            }
            // A `<script>` body is a raw-text element; entity escaping does not
            // reach it, so its expressions face a stricter rule. The flag only
            // ever turns ON - a nested element inside a script body is still in
            // the script's raw text as far as the JS engine is concerned.
            let child_ctx = CheckCtx {
                in_script: ctx.in_script || n.tag.eq_ignore_ascii_case("script"),
                ..ctx
            };
            check_nodes(&n.children, file, child_ctx, depth)?;
        }
        Node::Component(n) => {
            for prop in &n.props {
                if let ast::PropValue::Expr(e) = &prop.value {
                    check_expr(e, file, is_middleware)?;
                }
            }
            // `<C ...expr />`: the renderer does not read the spread today, but it is an
            // author-written expression in a rendered file and must obey the same rules.
            if let Some(e) = &n.spread {
                check_expr(e, file, is_middleware)?;
            }
            check_nodes(&n.children, file, ctx, depth)?;
        }
        Node::ClientLet { value, .. } => check_expr(value, file, is_middleware)?,
        Node::Debug(e) => check_expr(e, file, is_middleware)?,
        Node::Try(n) => {
            check_nodes(&n.body, file, ctx, depth)?;
            check_nodes(&n.error_branch, file, ctx, depth)?;
        }
        Node::Redirect(e) => {
            if !is_middleware {
                return Err(CompileError::parse(
                    file,
                    e.line,
                    e.column,
                    "{redirect} is only allowed inside middleware files",
                    ErrorCode::RedirectOutsideMiddleware,
                ));
            }
        }
        Node::Next => {
            if !is_middleware {
                return Err(CompileError::parse(
                    file,
                    0,
                    0,
                    "{next} is only allowed inside middleware files",
                    ErrorCode::RedirectOutsideMiddleware,
                ));
            }
        }
        Node::Layout(n) => {
            // `parse_layout` HOISTS `{head}` out of the children into `head`, so the
            // standalone `Node::Head` arm below never sees a layout's head - it has to be
            // walked from here or its contents are never checked at all.
            if let Some(head) = &n.head {
                check_nodes(head, file, ctx, depth)?;
            }
            check_nodes(&n.children, file, ctx, depth)?;
        }
        Node::Slot(n) => {
            if let Some(fb) = &n.fallback {
                check_nodes(fb, file, ctx, depth)?;
            }
        }
        Node::Head(nodes) => check_nodes(nodes, file, ctx, depth)?,
        // SECURITY: this match is EXHAUSTIVE on purpose - no `_` catch-all. Three
        // separate holes in this walk were caused by a node field being added while
        // this function silently kept compiling. A new variant must now be a COMPILE
        // ERROR here, forcing an explicit decision about whether it carries
        // author-written expressions or child nodes.
        Node::Text(_) | Node::Comment(_) | Node::Sections(_) | Node::Blocks => {}
    }
    Ok(())
}

fn check_attr_value(
    name: &str,
    val: &ast::AttributeValue,
    file: &str,
    is_middleware: bool,
) -> Result<(), CompileError> {
    match val {
        ast::AttributeValue::Interpolated(e) => {
            check_js_slot_positions(name, e, file)?;
            check_expr(e, file, is_middleware)
        }
        ast::AttributeValue::Expr(e) | ast::AttributeValue::ClassList(e) => {
            check_expr(e, file, is_middleware)
        }
        // A `{html ...}` attribute spread: enforce the same scope rules on its
        // condition + value expressions (e.g. no `request.*` outside middleware,
        // no `env.SLURP_SECRET_*`). Output-side sanitization of the spread is
        // done by `sanitize_attr_spread` in the renderer.
        ast::AttributeValue::RawAttrInjection { condition, expr } => {
            if let Some(c) = condition {
                check_expr(c, file, is_middleware)?;
            }
            check_expr(expr, file, is_middleware)
        }
        _ => Ok(()),
    }
}

/// SECURITY: refuse a `| js` declaration that is false about its own context.
///
/// `| js` means "this slot sits inside a quoted JavaScript string literal, so
/// escape the delimiters". A declared slot is then emitted VERBATIM, which is
/// correct inside a literal and catastrophic outside one:
/// `x-init="n = ${ v | js }"` with `1;alert(1)` rendered live code, because the
/// declaration turned the auto-escaper OFF without putting anything in its place.
///
/// Only the FALSE declaration is an error. An UNDECLARED slot in the same
/// position is handled by the renderer, which emits it as a JSON literal - a
/// self-delimiting encoding that is a complete answer there. Refusing that case
/// too would reject ordinary templates: numeric slots such as
/// `x-show="active == ${loop.index}"` are common, are not a bug, and cannot be
/// proved safe statically.
///
/// `| unsafe_js` and `| json` stay legal in every position: both are the author
/// declaring code / an already-encoded value, which is exactly what belongs here.
fn check_js_slot_positions(
    attr_name: &str,
    expr: &ast::Expression,
    file: &str,
) -> Result<(), CompileError> {
    if !renderer::is_js_attr(attr_name) {
        return Ok(());
    }
    let ast::ExprKind::TemplateLiteral(parts) = &expr.kind else {
        return Ok(());
    };
    let quotes = renderer::js_slot::slot_quote_contexts(parts).quotes;
    let slots = parts.iter().filter_map(|p| match p {
        ast::TemplatePart::Expr(e) => Some(e),
        ast::TemplatePart::Literal(_) => None,
    });
    for (slot, quote) in slots.zip(quotes) {
        if quote.is_none()
            && renderer::expr_has_filter(slot, "js")
            && !renderer::expr_has_filter(slot, "unsafe_js")
        {
            return Err(CompileError::parse(
                file,
                expr.line,
                expr.column,
                renderer::js_slot::js_filter_outside_string_literal(attr_name),
                ErrorCode::UnsafeScriptInterpolation,
            ));
        }
    }
    Ok(())
}

fn check_expr(expr: &ast::Expression, file: &str, is_middleware: bool) -> Result<(), CompileError> {
    use ast::ExprKind;
    match &expr.kind {
        ExprKind::Path(segs) | ExprKind::ReactivePath(segs) => {
            check_path(segs, expr, file, is_middleware)?;
        }
        ExprKind::BinaryOp { left, right, .. } => {
            check_expr(left, file, is_middleware)?;
            check_expr(right, file, is_middleware)?;
        }
        ExprKind::UnaryNot(e) | ExprKind::UnaryNeg(e) | ExprKind::Grouped(e) => {
            check_expr(e, file, is_middleware)?;
        }
        ExprKind::Ternary {
            condition,
            then,
            else_,
        } => {
            check_expr(condition, file, is_middleware)?;
            check_expr(then, file, is_middleware)?;
            check_expr(else_, file, is_middleware)?;
        }
        ExprKind::Call { callee, args } => {
            check_expr(callee, file, is_middleware)?;
            for a in args {
                check_expr(a, file, is_middleware)?;
            }
        }
        ExprKind::Index { object, index } => {
            check_expr(object, file, is_middleware)?;
            check_expr(index, file, is_middleware)?;
            // `${ (env)["SLURP_SECRET_API_KEY"] }` never reaches `check_path`: the
            // parenthesis makes it Index{ object: Grouped(Path([env])) }, whose two
            // halves are checked independently and neither is a rooted `env.KEY` path.
            // Re-join them here so the grouped form is policed like the plain one.
            if expr_is_env_root(object) {
                check_env_key_expr(index, expr, file)?;
            }
        }
        ExprKind::TemplateLiteral(parts) => {
            for p in parts {
                if let ast::TemplatePart::Expr(e) = p {
                    check_expr(e, file, is_middleware)?;
                }
            }
        }
        ExprKind::ClassList(entries) => {
            for entry in entries {
                check_expr(&entry.value, file, is_middleware)?;
                if let Some(c) = &entry.condition {
                    check_expr(c, file, is_middleware)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Is this expression exactly the bare path `env` (possibly parenthesized)?
fn expr_is_env_root(expr: &ast::Expression) -> bool {
    let mut cur = expr;
    // Unwrap `((env))`; the nesting is already bounded by the parser's MAX_DEPTH.
    loop {
        match &cur.kind {
            ast::ExprKind::Grouped(inner) => cur = inner,
            ast::ExprKind::Path(segs) | ast::ExprKind::ReactivePath(segs) => {
                return segs.len() == 1
                    && matches!(&segs[0], ast::PathSegment::Ident(n) if n == "env");
            }
            _ => return false,
        }
    }
}

/// The compile-time-constant name a subscript expression denotes, if any.
/// `None` means the key can only be known at render time.
fn const_index_name(expr: &ast::Expression) -> Option<String> {
    if !expr.filters.is_empty() {
        return None;
    }
    match &expr.kind {
        ast::ExprKind::StringLiteral(s) => Some(s.clone()),
        ast::ExprKind::NumberLiteral(n) => Some(n.to_string()),
        ast::ExprKind::Grouped(inner) => const_index_name(inner),
        ast::ExprKind::TemplateLiteral(parts) => {
            let mut out = String::new();
            for p in parts {
                match p {
                    ast::TemplatePart::Literal(l) => out.push_str(l),
                    ast::TemplatePart::Expr(_) => return None,
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// SECURITY: the secret-env gate for a SUBSCRIPT key - `env["SLURP_SECRET_X"]`.
/// `env.X` and `env["X"]` are the same access and the prefix test used to run on the
/// dotted form only, so the bracket form walked straight past it.
///
/// A key that is not a compile-time constant is REFUSED. There is no way to prove
/// statically that `env[k]` is not a secret, and guessing "probably fine" here is what
/// makes a static gate worthless.
fn check_env_key_expr(
    index: &ast::Expression,
    expr: &ast::Expression,
    file: &str,
) -> Result<(), CompileError> {
    match const_index_name(index) {
        Some(key) => check_env_key_name(&key, expr, file),
        None => Err(CompileError::parse(
            file,
            expr.line,
            expr.column,
            "env[...] must be indexed by a literal key so secret access can be checked at compile time",
            ErrorCode::SecretEnvInTemplate,
        )),
    }
}

fn check_env_key_name(key: &str, expr: &ast::Expression, file: &str) -> Result<(), CompileError> {
    if key.starts_with("SLURP_SECRET_") {
        return Err(CompileError::parse(
            file,
            expr.line,
            expr.column,
            format!(
                "env.{key} must not be accessed in templates; use server-side data binding instead"
            ),
            ErrorCode::SecretEnvInTemplate,
        ));
    }
    Ok(())
}

fn check_path(
    segs: &[ast::PathSegment],
    expr: &ast::Expression,
    file: &str,
    is_middleware: bool,
) -> Result<(), CompileError> {
    if segs.is_empty() {
        return Ok(());
    }

    // SECURITY: a subscript is an author-written expression in its own right. Without
    // this, `${ foo[request.cookies.session] }` never faces the middleware-scope guard,
    // because only the OUTER path was ever inspected.
    for seg in segs {
        if let ast::PathSegment::Index(inner) = seg {
            check_expr(inner, file, is_middleware)?;
        }
    }

    let root = match &segs[0] {
        ast::PathSegment::Ident(n) => n.as_str(),
        _ => return Ok(()),
    };

    if root == "env" {
        match segs.get(1) {
            None => {}
            Some(ast::PathSegment::Ident(key)) => check_env_key_name(key, expr, file)?,
            Some(ast::PathSegment::Index(idx)) => check_env_key_expr(idx, expr, file)?,
        }
    }

    if root == "request" && !is_middleware {
        return Err(CompileError::parse(
            file,
            expr.line,
            expr.column,
            "request.* is only accessible in middleware files",
            ErrorCode::MiddlewareScopeViolation,
        ));
    }

    if is_middleware && root != "request" && root != "env" {
        // "loop" is the only other path root allowed in middleware (loop variables
        // inside {each} blocks). "env", "request" are already handled above.
        // Keyword literals ("true", "false", "null") are never ExprKind::Path.
        if root != "loop" {
            return Err(CompileError::parse(
                file,
                expr.line,
                expr.column,
                format!("'{root}' is not accessible in middleware; only request.* is allowed"),
                ErrorCode::MiddlewareScopeViolation,
            ));
        }
    }

    Ok(())
}

/// SECURITY: hard cap on how long an import CHAIN may be. The depth of this DFS is
/// attacker-controlled on the `compile_with_options` / WASM virtual-files path (a chain
/// of N files recurses N frames, and each frame runs a full `parse()`), and a stack
/// overflow is a process ABORT, not a catchable error. Measured on a 2 MB stack: 500
/// files fine, 5000 files overflowed. The cap is far above any real theme (imports
/// nest a handful deep) and turns the abort into a clean diagnostic.
const MAX_IMPORT_CHAIN: usize = 128;

fn detect_circular_imports(
    files: &std::collections::HashMap<String, String>,
    entry: &str,
) -> Result<(), CompileError> {
    let mut visited = std::collections::HashSet::new();
    let mut stack = std::collections::HashSet::new();
    detect_circular_dfs(files, entry, &mut visited, &mut stack, 0)
}

fn detect_circular_dfs(
    files: &std::collections::HashMap<String, String>,
    current: &str,
    visited: &mut std::collections::HashSet<String>,
    stack: &mut std::collections::HashSet<String>,
    depth: usize,
) -> Result<(), CompileError> {
    if depth >= MAX_IMPORT_CHAIN {
        return Err(CompileError::parse(
            current,
            0,
            0,
            format!("Import chain too deep (limit {MAX_IMPORT_CHAIN}) at '{current}'"),
            ErrorCode::CircularImport,
        ));
    }
    if stack.contains(current) {
        return Err(CompileError::parse(
            current,
            0,
            0,
            format!("Circular import detected: '{current}' is already in the import chain"),
            ErrorCode::CircularImport,
        ));
    }
    if visited.contains(current) {
        return Ok(());
    }

    stack.insert(current.to_string());
    visited.insert(current.to_string());

    // The cap above bounds the depth; `with_stack` keeps each frame (which holds a whole
    // parsed Document) off the fixed 2 MB worker stack, so the CAP is the real limit.
    let result = stack::with_stack(|| {
        if let Some(source) = files.get(current)
            && let Ok((doc, _)) = parse(source, current)
            && let Some(fm) = &doc.frontmatter
        {
            for import in &fm.imports {
                let path = match import {
                    ast::Import::Default { path, .. } => path.clone(),
                    ast::Import::Types { path, .. } => path.clone(),
                };
                let full_path = if path.ends_with(".slurp") {
                    path.clone()
                } else {
                    format!("{path}.slurp")
                };
                detect_circular_dfs(files, &full_path, visited, stack, depth + 1)?;
            }
        }
        Ok(())
    });
    result?;

    stack.remove(current);
    Ok(())
}

#[must_use]
pub fn compile_with_registry(
    source: &str,
    file: impl Into<String>,
    context: &serde_json::Value,
    mode: BuildMode,
    file_registry: std::sync::Arc<std::collections::HashMap<String, String>>,
) -> (String, ErrorAccumulator) {
    compile_with_render_options(
        source,
        file,
        context,
        &RenderOptions::new(mode).with_registry(file_registry),
    )
}

/// As [`compile_with_registry`], taking a [`RenderOptions`] so the host can set a non-default
/// [`MemoryBudget`]. `compile` and `compile_with_registry` delegate here with the default budget.
#[must_use]
pub fn compile_with_render_options(
    source: &str,
    file: impl Into<String>,
    context: &serde_json::Value,
    options: &RenderOptions,
) -> (String, ErrorAccumulator) {
    let file = file.into();
    let (doc, mut parse_diag) = match parse(source, &file) {
        Ok(result) => result,
        Err(lex_err) => {
            let mut acc = ErrorAccumulator::new();
            acc.push(lex_err);
            return (String::new(), acc);
        }
    };
    // `is_middleware = false`: every entry point that reaches here compiles a PAGE. A middleware
    // file is never compiled at all - the CLI checks it with `check_security(.., true)` and stops
    // there, and the server resolves it at request time. Passing `true` here would relax the page
    // rules; passing a caller-supplied flag would let a page opt out of them.
    if let Err(sec_err) = security_check(&doc, &file, false) {
        parse_diag.push(sec_err);
        return (String::new(), parse_diag);
    }
    let (html, render_diag) = render_with_options(&doc, context, options);
    for e in render_diag.errors.into_iter().chain(render_diag.warnings) {
        parse_diag.push(e);
    }
    (html, parse_diag)
}

#[must_use]
pub fn compile(
    source: &str,
    file: impl Into<String>,
    context: &serde_json::Value,
    mode: BuildMode,
) -> (String, ErrorAccumulator) {
    compile_with_render_options(source, file, context, &RenderOptions::new(mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_compile_basic() {
        let (html, diag) = compile(
            "${name}",
            "test.slurp",
            &json!({"name": "World"}),
            BuildMode::Production,
        );
        assert!(!diag.has_errors());
        assert_eq!(html, "World");
    }

    #[test]
    fn test_compile_xss_escaped() {
        let (html, _) = compile(
            "${msg}",
            "test.slurp",
            &json!({"msg": "<script>alert(1)</script>"}),
            BuildMode::Production,
        );
        assert!(!html.contains("<script>"), "XSS not escaped: {html}");
        assert!(html.contains("&lt;"));
    }

    #[test]
    fn test_compile_empty_source() {
        let (html, diag) = compile("", "test.slurp", &json!({}), BuildMode::Production);
        assert!(!diag.has_errors());
        assert_eq!(html, "");
    }

    #[test]
    fn test_compile_with_frontmatter() {
        let src = "---\nusing \"@c/Nav\"\n---\n<div>hi</div>";
        let (html, diag) = compile(src, "test.slurp", &json!({}), BuildMode::Production);
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        assert!(html.contains("hi"));
    }

    #[test]
    fn test_compile_never_panics_on_bad_input() {
        let bad_inputs = [
            "${",
            "{if",
            "<div>unclosed",
            "{{{{",
            "\x00\x01\x02",
            "{each x in}",
        ];
        for input in &bad_inputs {
            let _ = compile(input, "test.slurp", &json!({}), BuildMode::Production);
        }
    }

    #[test]
    fn test_extract_schema() {
        let src = "---\nsection {\n  name: \"Hero\"\n  settings { heading: text = \"Hi\" }\n}\n---\n<h1>${section.settings.heading}</h1>";
        let (schema, diag) = extract_schema(src, "hero.slurp").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let schema = schema.expect("schema missing");
        assert_eq!(schema["name"], "Hero");
        assert_eq!(schema["settings"][0]["type"], "text");
    }

    #[test]
    fn test_extract_schema_none_without_section() {
        let (schema, diag) = extract_schema("<div>plain</div>", "p.slurp").unwrap();
        assert!(!diag.has_errors());
        assert!(schema.is_none());
    }

    /// `SlotNode.fallback` is walked by the security gate.
    ///
    /// This one is asserted against a hand-built AST on purpose. `parse_slot` only ever
    /// fills `fallback` from malformed source (well-formed `{slot}x{/slot}` leaves the
    /// body as SIBLING nodes), so there is no template that reaches the field cleanly
    /// today - the hole is latent, not live. A structural test is what keeps it closed
    /// if the parser is ever fixed to populate the field properly.
    #[test]
    fn test_security_check_walks_slot_fallback() {
        let secret = ast::Expression::new(
            ast::ExprKind::Path(vec![
                ast::PathSegment::Ident("env".into()),
                ast::PathSegment::Ident("SLURP_SECRET_API_KEY".into()),
            ]),
            1,
            1,
        );
        let doc = ast::Document {
            frontmatter: None,
            children: vec![ast::Node::Slot(ast::SlotNode {
                name: None,
                fallback: Some(vec![ast::Node::Expression(secret)]),
            })],
            file: "t.slurp".into(),
        };
        let err = security_check(&doc, "t.slurp", false)
            .expect_err("a secret inside a slot fallback must be refused");
        assert_eq!(err.code, ErrorCode::SecretEnvInTemplate);
    }

    #[test]
    fn test_compile_safe_navigation_null() {
        let (html, diag) = compile(
            "${a.b.c.d}",
            "test.slurp",
            &json!({"a": null}),
            BuildMode::Production,
        );
        assert!(!diag.has_errors());
        assert_eq!(html, "");
    }
}
