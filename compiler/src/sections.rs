//! Section state merging and rendering.
//!
//! A section template declares its editable surface with a `section { ... }`
//! frontmatter block. The editor saves per-page section state as
//! `{id, type, settings, blocks: [{id, type, settings}], disabled}`. This module
//! merges that saved state over the schema defaults with TOLERANT validation:
//! unknown keys are dropped, select values outside the option list fall back to
//! the default, numbers are clamped to min/max meta, type mismatches keep the
//! default, unknown block types are dropped, and block counts are capped at the
//! block's `max`. A removed or renamed setting in a newer theme version can
//! therefore never break saved editor state.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::ast::{BlockDef, BlockTargeting, Document, SectionSchema, SettingDef, SettingKind};
use crate::error::ErrorAccumulator;
use crate::renderer::{self, BuildMode};

/// Convert an f64 to a JSON number, using an integer representation when the
/// value is integral so schema JSON shows `8`, not `8.0`.
pub(crate) fn json_number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() <= i64::MAX as f64 {
        Value::Number(serde_json::Number::from(n as i64))
    } else {
        serde_json::Number::from_f64(n)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// Build the `section` scope object for rendering: saved state merged over the
/// schema defaults. `state` is one entry of a page's section list.
#[must_use]
pub fn merged_section_value(schema: &SectionSchema, state: &Value) -> Value {
    merged_section_value_with(schema, state, &BlockCatalog::new())
}

/// Theme-level blocks (`blocks/<name>.slurp`) available to any section that targets them, keyed by
/// block type. Built once per theme by the publish/render pipeline.
pub type BlockCatalog = HashMap<String, BlockDef>;

/// As [`merged_section_value`], with a theme-block catalog so `blocks { @theme }` resolves.
///
/// Without the catalog a targeted block is an UNKNOWN TYPE and the merge drops it - the saved state
/// survives in storage while the page renders as if nothing had been added. That is why this
/// parameter is threaded rather than defaulted at the render site.
#[must_use]
pub fn merged_section_value_with(
    schema: &SectionSchema,
    state: &Value,
    catalog: &BlockCatalog,
) -> Value {
    let mut out = Map::new();
    for key in ["id", "type"] {
        // SECURITY (boundary validation): `id` and `type` are the only
        // editor-controlled fields with no per-kind coerce step, unlike every
        // setting and every string leaf of schema-less state. Relying on each
        // render site to escape them would make them safe only by coincidence,
        // so they are validated here instead. See `safe_ident`.
        if let Some(v) = state.get(key).and_then(safe_ident) {
            out.insert(key.to_string(), v);
        }
    }
    out.insert(
        "settings".to_string(),
        merged_settings(&schema.settings, state.get("settings")),
    );
    out.insert(
        "blocks".to_string(),
        merged_blocks_at(
            &schema.blocks,
            schema.accepts.as_ref(),
            state.get("blocks"),
            catalog,
            0,
        ),
    );
    Value::Object(out)
}

/// Turn a theme block file's `block { ... }` schema into the `BlockDef` the merge and the editor
/// both speak, keyed by its TYPE - which is the file name (`blocks/heading.slurp` -> `heading`),
/// not the display name. Deriving the type from the path is what makes a theme block addressable
/// without a registry file to keep in sync.
#[must_use]
pub fn catalog_entry(block_type: &str, schema: &crate::ast::BlockSchema) -> BlockDef {
    BlockDef {
        block_type: block_type.to_string(),
        // A theme block has no per-section `max`; the SECTION that hosts it decides how many it
        // wants, which is the only place that knows.
        max: None,
        settings: schema.settings.clone(),
        blocks: schema.blocks.clone(),
        accepts: schema.accepts.clone(),
    }
}

/// The block TYPE for a theme block file path: `blocks/social_row.slurp` -> `social_row`.
/// Returns `None` for anything outside `blocks/`.
///
/// The charset here MUST match the one `renderer::render_block_children` enforces
/// before using a block type as a registry path (`[A-Za-z0-9_-]`). If the two
/// disagree the failure is silent: a nested file `blocks/a/b.slurp` would register
/// in the catalog as `a/b`, merge happily into saved state, and then render
/// NOTHING, because the renderer's gate refuses the `/`. That gate is the security
/// boundary and stays as it is, so a nested block file is ignored at catalog-build
/// time rather than becoming a block that can be added in the editor and never
/// appears on the page.
#[must_use]
pub fn theme_block_type(path: &str) -> Option<&str> {
    let stem = path.strip_prefix("blocks/")?.strip_suffix(".slurp")?;
    let renderable = !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    renderable.then_some(stem)
}

/// SECURITY: an identifier copied straight out of saved editor state (`section.id`,
/// `section.type`) and rendered into attributes, CSS selectors and JS strings by
/// templates the engine does not control.
///
/// Deliberately a REJECT-list of characters that are structural in one of those
/// three contexts rather than an allow-list of an id shape: ids are minted by the
/// editor and live in saved state for the life of a site, so an allow-list that
/// guessed the shape wrong would silently drop a real section id and break
/// click-to-select on it. Nothing legitimate carries a quote, an angle
/// bracket, a backslash, a backtick or whitespace, so refusing those closes the
/// boundary at no compatibility cost. A value that fails is DROPPED, not
/// rewritten: a mangled id is a broken reference either way, and dropping keeps
/// the failure visible.
fn safe_ident(v: &Value) -> Option<Value> {
    let s = v.as_str()?;
    if s.len() > 256 {
        return None;
    }
    let bad = |c: char| {
        matches!(
            c,
            '<' | '>' | '"' | '\'' | '`' | '\\' | '&' | '/' | '=' | '(' | ')' | ';' | '{' | '}'
        ) || c.is_whitespace()
            || c.is_control()
    };
    (!s.chars().any(bad)).then(|| Value::String(s.to_string()))
}

/// The block definition for `btype` at a level that declares `defs` inline and targets `accepts`.
///
/// An INLINE definition always wins: a theme that declares its own `heading` keeps it, so adding a
/// `heading` to the shared palette can never change what an existing section renders. Targeting then
/// gates the catalog - an empty allow-list means every theme block, a non-empty one is a whitelist.
fn resolve_block_def<'a>(
    btype: &str,
    defs: &'a [BlockDef],
    accepts: Option<&BlockTargeting>,
    catalog: &'a BlockCatalog,
) -> Option<&'a BlockDef> {
    if let Some(d) = defs.iter().find(|d| d.block_type == btype) {
        return Some(d);
    }
    let target = accepts?;
    if !target.allow.is_empty() && !target.allow.iter().any(|a| a == btype) {
        return None;
    }
    catalog.get(btype)
}

/// Merge saved settings over the schema defaults (tolerant - see module docs).
#[must_use]
pub fn merged_settings(defs: &[SettingDef], saved: Option<&Value>) -> Value {
    let saved = saved.and_then(Value::as_object);
    let mut out = Map::new();
    for def in defs {
        let mut val = def
            .default
            .clone()
            .unwrap_or_else(|| default_for_kind(&def.kind));
        if let Some(sv) = saved.and_then(|s| s.get(&def.key))
            && let Some(coerced) = coerce(&def.kind, sv, &def.meta)
        {
            val = coerced;
        }
        out.insert(def.key.clone(), val);
    }
    Value::Object(out)
}

fn default_for_kind(kind: &SettingKind) -> Value {
    match kind {
        SettingKind::Text
        | SettingKind::Richtext
        | SettingKind::Color
        | SettingKind::Image
        | SettingKind::Icon
        | SettingKind::Link => Value::String(String::new()),
        SettingKind::Select(opts) => Value::String(opts.first().cloned().unwrap_or_default()),
        SettingKind::Number => Value::Number(0.into()),
        SettingKind::Toggle => Value::Bool(false),
    }
}

/// Returns None when the saved value is invalid for the kind (caller keeps the default).
///
/// SECURITY (defense-in-depth): editor-supplied values are sanitized/validated HERE, at the merge
/// boundary, so safety does not depend on every theme template rendering them correctly. `richtext`
/// is run through the same ammonia allowlist as `{html sanitize}` (so even a bare `{html}` render is
/// safe); `link` rejects dangerous URL schemes (`javascript:`/`data:`/...); `color` and `icon` are
/// restricted to a safe character shape so they can't break out of an attribute/CSS context.
fn coerce(kind: &SettingKind, v: &Value, meta: &Map<String, Value>) -> Option<Value> {
    match kind {
        SettingKind::Text => v.as_str().map(|s| Value::String(s.to_string())),
        // `image` is a URL and gets the same scheme gate as `link`. It used to be
        // a bare string passthrough, so `javascript:alert(1)` survived the merge
        // intact. That happened to be caught downstream at an `src=` render site
        // by `has_forbidden_url_scheme`, but nothing catches it in
        // `background:url(${section.settings.image})` or in any other context a
        // theme chooses. A URL setting is validated where it enters, not where it
        // is used, because the merge boundary is the only place that knows the
        // value is a URL at all.
        SettingKind::Image => v.as_str().map(|s| Value::String(safe_link(s))),
        SettingKind::Richtext => v
            .as_str()
            .map(|s| Value::String(crate::renderer::sanitize::sanitize_html(s))),
        SettingKind::Link => v.as_str().map(|s| Value::String(safe_link(s))),
        SettingKind::Color => v.as_str().and_then(|s| safe_color(s).map(Value::String)),
        SettingKind::Icon => v.as_str().and_then(|s| safe_icon(s).map(Value::String)),
        SettingKind::Select(opts) => v
            .as_str()
            .filter(|s| opts.iter().any(|o| o == s))
            .map(|s| Value::String(s.to_string())),
        SettingKind::Toggle => v.as_bool().map(Value::Bool),
        SettingKind::Number => {
            let mut n = v.as_f64()?;
            if let Some(min) = meta.get("min").and_then(Value::as_f64) {
                n = n.max(min);
            }
            if let Some(max) = meta.get("max").and_then(Value::as_f64) {
                n = n.min(max);
            }
            Some(json_number(n))
        }
    }
}

/// Sanitize a `link` value: relative paths/anchors/queries are kept as-is; an absolute URL must use
/// a safe scheme (http/https/mailto/tel). Anything else (`javascript:`, `data:`, `vbscript:`, ...)
/// collapses to empty so it can never become an executable `href`.
fn safe_link(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        return String::new();
    }
    // Relative URL / anchor / query / protocol-relative-but-no-scheme: no scheme to abuse.
    // A scheme exists only if a ':' appears before the first '/', '?' or '#'.
    let scheme_end = t.find(['/', '?', '#']);
    let colon = t.find(':');
    let has_scheme = matches!((colon, scheme_end), (Some(c), Some(e)) if c < e)
        || (colon.is_some() && scheme_end.is_none());
    if !has_scheme {
        return t.to_string();
    }
    let scheme = t[..colon.unwrap()].to_ascii_lowercase();
    if matches!(scheme.as_str(), "http" | "https" | "mailto" | "tel") {
        t.to_string()
    } else {
        String::new()
    }
}

/// Validate a `color` value: a `#hex` (3/4/6/8 digits) or a known CSS color
/// keyword. Returns None for anything else (keeps the default) so a color can't
/// carry `;`, `(`, `url(...)` etc. into an attribute/style context. The keyword
/// arm must stay a real ALLOWLIST (the CSS named colors plus the system/keyword
/// values); an "is it all alphabetic" test would admit arbitrary identifiers.
fn safe_color(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        return Some(String::new());
    }
    if let Some(hex) = t.strip_prefix('#') {
        let ok = matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
        return ok.then(|| t.to_string());
    }
    is_named_css_color(t).then(|| t.to_string())
}

/// The CSS named colors plus the keyword/system values, matched case-insensitively.
fn is_named_css_color(name: &str) -> bool {
    // Length bound first - the longest CSS color name is "lightgoldenrodyellow" (20).
    if name.len() > 20 || !name.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    const NAMED: &[&str] = &[
        // keyword / system values
        "transparent",
        "currentcolor",
        "inherit",
        "initial",
        "unset",
        "revert",
        "none",
        // CSS Level 1-4 named colors
        "black",
        "silver",
        "gray",
        "grey",
        "white",
        "maroon",
        "red",
        "purple",
        "fuchsia",
        "green",
        "lime",
        "olive",
        "yellow",
        "navy",
        "blue",
        "teal",
        "aqua",
        "cyan",
        "magenta",
        "orange",
        "aliceblue",
        "antiquewhite",
        "aquamarine",
        "azure",
        "beige",
        "bisque",
        "blanchedalmond",
        "blueviolet",
        "brown",
        "burlywood",
        "cadetblue",
        "chartreuse",
        "chocolate",
        "coral",
        "cornflowerblue",
        "cornsilk",
        "crimson",
        "darkblue",
        "darkcyan",
        "darkgoldenrod",
        "darkgray",
        "darkgrey",
        "darkgreen",
        "darkkhaki",
        "darkmagenta",
        "darkolivegreen",
        "darkorange",
        "darkorchid",
        "darkred",
        "darksalmon",
        "darkseagreen",
        "darkslateblue",
        "darkslategray",
        "darkslategrey",
        "darkturquoise",
        "darkviolet",
        "deeppink",
        "deepskyblue",
        "dimgray",
        "dimgrey",
        "dodgerblue",
        "firebrick",
        "floralwhite",
        "forestgreen",
        "gainsboro",
        "ghostwhite",
        "gold",
        "goldenrod",
        "greenyellow",
        "honeydew",
        "hotpink",
        "indianred",
        "indigo",
        "ivory",
        "khaki",
        "lavender",
        "lavenderblush",
        "lawngreen",
        "lemonchiffon",
        "lightblue",
        "lightcoral",
        "lightcyan",
        "lightgoldenrodyellow",
        "lightgray",
        "lightgrey",
        "lightgreen",
        "lightpink",
        "lightsalmon",
        "lightseagreen",
        "lightskyblue",
        "lightslategray",
        "lightslategrey",
        "lightsteelblue",
        "lightyellow",
        "limegreen",
        "linen",
        "mediumaquamarine",
        "mediumblue",
        "mediumorchid",
        "mediumpurple",
        "mediumseagreen",
        "mediumslateblue",
        "mediumspringgreen",
        "mediumturquoise",
        "mediumvioletred",
        "midnightblue",
        "mintcream",
        "mistyrose",
        "moccasin",
        "navajowhite",
        "oldlace",
        "olivedrab",
        "orangered",
        "orchid",
        "palegoldenrod",
        "palegreen",
        "paleturquoise",
        "palevioletred",
        "papayawhip",
        "peachpuff",
        "peru",
        "pink",
        "plum",
        "powderblue",
        "rosybrown",
        "royalblue",
        "saddlebrown",
        "salmon",
        "sandybrown",
        "seagreen",
        "seashell",
        "sienna",
        "skyblue",
        "slateblue",
        "slategray",
        "slategrey",
        "snow",
        "springgreen",
        "steelblue",
        "tan",
        "thistle",
        "tomato",
        "turquoise",
        "violet",
        "wheat",
        "whitesmoke",
        "yellowgreen",
        "rebeccapurple",
    ];
    let lower = name.to_ascii_lowercase();
    NAMED.contains(&lower.as_str())
}

/// Validate an `icon` value: a Lucide kebab name, `[a-z0-9-]+`. Returns None otherwise.
fn safe_icon(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        return Some(String::new());
    }
    (!t.is_empty()
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
    .then(|| t.to_string())
}

/// Maximum nested-block recursion depth when merging saved state. A schema's
/// block-nesting depth is bounded by the parser's `MAX_DEPTH`, but saved state
/// is untrusted JSON and is walked in lock-step with the schema here;
/// this is a defensive ceiling so a pathological schema/state can't blow the
/// stack during a render.
const MAX_BLOCK_DEPTH: usize = 64;

/// Merge a saved block list against the schema's block definitions: unknown
/// types dropped, per-type count capped at `max`, each block's settings merged.
#[must_use]
pub fn merged_blocks(defs: &[BlockDef], saved: Option<&Value>) -> Value {
    merged_blocks_at(defs, None, saved, &BlockCatalog::new(), 0)
}

fn merged_blocks_at(
    defs: &[BlockDef],
    accepts: Option<&BlockTargeting>,
    saved: Option<&Value>,
    catalog: &BlockCatalog,
    depth: usize,
) -> Value {
    if depth >= MAX_BLOCK_DEPTH {
        return Value::Array(Vec::new());
    }
    let Some(items) = saved.and_then(Value::as_array) else {
        return Value::Array(Vec::new());
    };

    let mut counts: HashMap<&str, u32> = HashMap::new();
    let mut out = Vec::new();
    for item in items {
        let Some(btype) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        let Some(def) = resolve_block_def(btype, defs, accepts, catalog) else {
            continue;
        };
        let count = counts.entry(&def.block_type).or_insert(0);
        // A schema `max` is an explicit per-type cap; absent it, blocks are unbounded (the editor
        // allows any number of elements) but a high hard ceiling still guards against a
        // pathological saved state OOMing the render.
        const BLOCK_HARD_CEILING: u32 = 200;
        let limit = def.max.unwrap_or(BLOCK_HARD_CEILING);
        if *count >= limit {
            continue;
        }
        *count += 1;

        let mut block = Map::new();
        // SECURITY (boundary validation): a BLOCK's `id` is the same untrusted, editor-minted
        // string as a SECTION's, rendered into the same attributes, CSS selectors and JS strings
        // by templates the engine does not control. It therefore goes through the same
        // `safe_ident` gate as the section-level `id`/`type` two levels up - unbounded, unchecked,
        // or not even a string are all reachable here otherwise. Validating at the merge boundary
        // rather than at each render site is the principle stated on `merged_section_value_with`;
        // this is the same boundary, so it gets the same gate.
        //
        // A block's `type` needs no check: `resolve_block_def` above already refused anything
        // that is not a declared block type, so it can only be a name the schema published.
        if let Some(id) = item.get("id").and_then(safe_ident) {
            block.insert("id".to_string(), id);
        }
        block.insert("type".to_string(), Value::String(btype.to_string()));
        block.insert(
            "settings".to_string(),
            merged_settings(&def.settings, item.get("settings")),
        );
        // Recursively merge this block's OWN nested blocks (blocks inside blocks). Leaf blocks
        // (no nested defs) get an empty array, mirroring how the section always carries `blocks`.
        block.insert(
            "blocks".to_string(),
            merged_blocks_at(
                &def.blocks,
                def.accepts.as_ref(),
                item.get("blocks"),
                catalog,
                depth + 1,
            ),
        );
        out.push(Value::Object(block));
    }
    Value::Array(out)
}

/// Recursively sanitize every string leaf of a JSON value through the ammonia
/// allowlist (`{html sanitize}`). Used for schema-less section state so
/// editor-supplied strings can't carry script/handlers into a raw `{html}`
/// render. A bounded depth guards against pathological nesting.
fn sanitize_value_strings(v: &Value) -> Value {
    fn walk(v: &Value, depth: usize) -> Value {
        if depth >= MAX_BLOCK_DEPTH {
            return Value::Null;
        }
        match v {
            // A string that cannot produce a `<` is not markup, and running the
            // sanitizer over it can only do harm: ammonia entity-encodes as it
            // goes, and the value is then entity-escaped AGAIN by `${ }`, so a
            // saved "Tom & Jerry" would reach the page as "Tom &amp;amp; Jerry".
            //
            // The test is a real entity DECODE, not a `contains('<')` heuristic,
            // because the whole point of sanitizing here is that this value may be
            // emitted RAW by `{html}` - and `&#60;script&#62;` carries no literal
            // `<` while decoding to one in the browser. One decode is the right
            // depth: a browser decodes an entity exactly once, so a value that is
            // still entity-text after one round (`&amp;lt;script&amp;gt;`) is text
            // on the page too.
            Value::String(s) if !html_escape::decode_html_entities(s).contains('<') => {
                Value::String(s.clone())
            }
            Value::String(s) => Value::String(crate::renderer::sanitize::sanitize_html(s)),
            Value::Array(a) => Value::Array(a.iter().map(|e| walk(e, depth + 1)).collect()),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, e)| (k.clone(), walk(e, depth + 1)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk(v, 0)
}

/// Render a section template with its merged `section` object injected into the
/// root scope. `section_state` is the saved editor state for this instance; a
/// template without a `section {}` schema gets the state passed through as-is.
#[must_use]
pub fn render_section(
    doc: &Document,
    section_state: &Value,
    context: &Value,
    mode: BuildMode,
) -> (String, ErrorAccumulator) {
    render_section_with_registry(
        doc,
        section_state,
        context,
        mode,
        std::sync::Arc::new(std::collections::HashMap::new()),
    )
}

/// Like [`render_section`] but with a file registry so the section can resolve
/// `using` component imports.
#[must_use]
pub fn render_section_with_registry(
    doc: &Document,
    section_state: &Value,
    context: &Value,
    mode: BuildMode,
    file_registry: std::sync::Arc<std::collections::HashMap<String, String>>,
) -> (String, ErrorAccumulator) {
    render_section_with_registry_and_blocks(
        doc,
        section_state,
        context,
        mode,
        file_registry,
        &BlockCatalog::new(),
    )
}

/// As [`render_section_with_registry`], with the theme-block catalog so a section that declares
/// `blocks { @theme }` renders the blocks that were actually added instead of silently dropping
/// them as unknown types.
pub fn render_section_with_registry_and_blocks(
    doc: &Document,
    section_state: &Value,
    context: &Value,
    mode: BuildMode,
    file_registry: std::sync::Arc<std::collections::HashMap<String, String>>,
    catalog: &BlockCatalog,
) -> (String, ErrorAccumulator) {
    render_section_with_options(
        doc,
        section_state,
        context,
        catalog,
        &renderer::RenderOptions::new(mode).with_registry(file_registry),
    )
}

/// As [`render_section_with_registry_and_blocks`], taking a [`renderer::RenderOptions`] so the host
/// can set a non-default [`renderer::MemoryBudget`]. The older entry points delegate here with the
/// default budget.
#[must_use]
pub fn render_section_with_options(
    doc: &Document,
    section_state: &Value,
    context: &Value,
    catalog: &BlockCatalog,
    options: &renderer::RenderOptions,
) -> (String, ErrorAccumulator) {
    let merged = match doc.frontmatter.as_ref().and_then(|f| f.section.as_ref()) {
        Some(schema) => merged_section_value_with(schema, section_state, catalog),
        // SECURITY: with no `section {}` schema there is no per-setting coerce
        // step, so a bare `{html section.settings.x}` would render saved state
        // verbatim. Run every string leaf of the passed-through state through the
        // same ammonia allowlist used for `richtext`, so even a raw `{html}` of a
        // schema-less setting cannot inject script/handlers.
        None => sanitize_value_strings(section_state),
    };
    let mut ctx = match context {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    ctx.insert("section".to_string(), merged);
    renderer::render_with_options(doc, &Value::Object(ctx), options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use serde_json::json;

    fn schema_from(src: &str) -> SectionSchema {
        let (doc, diag) = parse(src, "test.slurp").expect("lex failed");
        assert!(!diag.has_errors(), "Parse errors: {:?}", diag.errors);
        doc.frontmatter.unwrap().section.expect("no section schema")
    }

    #[test]
    fn link_rejects_dangerous_schemes() {
        assert_eq!(safe_link("javascript:alert(1)"), "");
        assert_eq!(safe_link("  JavaScript:alert(1)"), "");
        assert_eq!(safe_link("data:text/html,<script>"), "");
        assert_eq!(safe_link("vbscript:x"), "");
        // Safe forms preserved.
        assert_eq!(safe_link("/products"), "/products");
        assert_eq!(safe_link("#faq"), "#faq");
        assert_eq!(safe_link("products/cool"), "products/cool");
        assert_eq!(safe_link("https://example.com"), "https://example.com");
        assert_eq!(safe_link("mailto:a@b.com"), "mailto:a@b.com");
    }

    #[test]
    fn color_only_hex_or_keyword() {
        assert_eq!(safe_color("#fff").as_deref(), Some("#fff"));
        assert_eq!(safe_color("#aabbccdd").as_deref(), Some("#aabbccdd"));
        assert_eq!(safe_color("red").as_deref(), Some("red"));
        assert_eq!(safe_color("transparent").as_deref(), Some("transparent"));
        assert_eq!(safe_color("red;background:url(x)"), None);
        assert_eq!(safe_color("#xyz"), None);
        assert_eq!(safe_color("rgb(1,2,3)"), None);
    }

    #[test]
    fn icon_only_kebab() {
        assert_eq!(safe_icon("arrow-right").as_deref(), Some("arrow-right"));
        assert_eq!(safe_icon("Arrow Right"), None);
        assert_eq!(safe_icon("a\"onload=x"), None);
    }

    #[test]
    fn richtext_is_sanitized_at_merge() {
        let schema = schema_from(
            "---\nsection {\n  name: \"S\"\n  settings { body: richtext = \"\" }\n}\n---\n<div></div>",
        );
        let state = json!({ "settings": { "body": "<p>ok</p><script>alert(1)</script>" } });
        let merged = merged_section_value(&schema, &state);
        let body = merged["settings"]["body"].as_str().unwrap();
        assert!(body.contains("<p>ok</p>"));
        assert!(!body.contains("<script"), "script survived merge: {body}");
    }

    // ── Theme blocks ────────────────────────────────────────────────────────────────────────

    fn catalog() -> BlockCatalog {
        let src = "---
block { name: \"Heading\" settings { text: text = \"Hi\" } }
---
<h2></h2>";
        let (doc, _) = crate::parse(src, "blocks/heading.slurp").unwrap();
        let schema = doc.frontmatter.unwrap().block.unwrap();
        let mut c = BlockCatalog::new();
        c.insert("heading".into(), catalog_entry("heading", &schema));
        c
    }

    fn section_with(blocks_decl: &str) -> SectionSchema {
        let src = format!(
            "---
section {{ name: \"S\" blocks {{ {blocks_decl} }} }}
---
<div></div>"
        );
        let (doc, diag) = crate::parse(&src, "t").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        doc.frontmatter.unwrap().section.unwrap()
    }

    fn state_with_heading() -> Value {
        serde_json::json!({
            "id": "s1", "type": "S",
            "blocks": [{ "id": "b1", "type": "heading", "settings": { "text": "Real heading" } }]
        })
    }

    /// The failure this whole seam exists to prevent: without the catalog a targeted block is an
    /// unknown type, so the merge drops it - the state survives in storage while the page renders
    /// as if nothing had been added.
    #[test]
    fn a_targeted_theme_block_renders_only_with_the_catalog() {
        let schema = section_with("@theme");
        let state = state_with_heading();

        let without = merged_section_value(&schema, &state);
        assert_eq!(
            without["blocks"].as_array().unwrap().len(),
            0,
            "no catalog = dropped"
        );

        let with = merged_section_value_with(&schema, &state, &catalog());
        let blocks = with["blocks"].as_array().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "heading");
        assert_eq!(blocks[0]["settings"]["text"], "Real heading");
        assert_eq!(blocks[0]["id"], "b1", "block identity is preserved");
    }

    #[test]
    fn a_whitelist_admits_only_what_it_names() {
        let allowed = merged_section_value_with(
            &section_with("@theme(heading)"),
            &state_with_heading(),
            &catalog(),
        );
        assert_eq!(allowed["blocks"].as_array().unwrap().len(), 1);

        let refused = merged_section_value_with(
            &section_with("@theme(text)"),
            &state_with_heading(),
            &catalog(),
        );
        assert_eq!(
            refused["blocks"].as_array().unwrap().len(),
            0,
            "not on the whitelist"
        );
    }

    #[test]
    fn a_section_that_targets_nothing_still_refuses_theme_blocks() {
        let schema = section_with("legacy { settings { a: text } }");
        let out = merged_section_value_with(&schema, &state_with_heading(), &catalog());
        assert_eq!(out["blocks"].as_array().unwrap().len(), 0);
    }

    /// Adding a block to the shared palette must never change what an existing section renders, so
    /// an inline definition of the same type wins.
    #[test]
    fn an_inline_definition_beats_the_catalog() {
        let schema = section_with("@theme heading { settings { text: text = \"local\" } }");
        let out = merged_section_value_with(
            &schema,
            &serde_json::json!({
                "blocks": [{ "type": "heading", "settings": { "other": "x" } }]
            }),
            &catalog(),
        );
        let b = &out["blocks"].as_array().unwrap()[0];
        // The LOCAL schema decided the shape: its default applied and the foreign key was dropped.
        assert_eq!(b["settings"]["text"], "local");
        assert!(b["settings"].get("other").is_none());
    }

    /// A theme block nested inside a theme block is how `group`/`columns` compose. The recursion has
    /// to carry the catalog down, and the depth ceiling still has to hold.
    #[test]
    fn theme_blocks_nest_through_the_catalog() {
        let group_src = "---
block { name: \"Group\" blocks { @theme } }
---
<div></div>";
        let (doc, _) = crate::parse(group_src, "blocks/group.slurp").unwrap();
        let mut c = catalog();
        c.insert(
            "group".into(),
            catalog_entry("group", &doc.frontmatter.unwrap().block.unwrap()),
        );

        let state = serde_json::json!({
            "blocks": [{ "type": "group", "blocks": [
                { "type": "heading", "settings": { "text": "nested" } }
            ]}]
        });
        let out = merged_section_value_with(&section_with("@theme"), &state, &c);
        let inner = &out["blocks"][0]["blocks"][0];
        assert_eq!(inner["type"], "heading");
        assert_eq!(inner["settings"]["text"], "nested");
    }

    #[test]
    fn theme_block_type_comes_from_the_path() {
        assert_eq!(
            theme_block_type("blocks/social_row.slurp"),
            Some("social_row")
        );
        assert_eq!(theme_block_type("sections/Hero.slurp"), None);
        assert_eq!(theme_block_type("blocks/x.json"), None);
    }

    /// A nested block file must not register in the catalog under a type the
    /// RENDERER refuses. `{blocks}` restricts a block type to `[A-Za-z0-9_-]`
    /// before using it as a registry path, so `a/b` would merge into saved state
    /// and then render nothing at all - a block that could be added in the editor
    /// and never seen. The renderer's gate is the security boundary and stays;
    /// this side agrees with it, so the file is simply not a block.
    #[test]
    fn a_block_type_the_renderer_would_refuse_is_not_catalogued() {
        assert_eq!(theme_block_type("blocks/a/b.slurp"), None);
        assert_eq!(theme_block_type("blocks/../secret.slurp"), None);
        assert_eq!(theme_block_type("blocks/.slurp"), None);
        // The renderer's own predicate, restated, so the two cannot drift apart
        // without one of these failing.
        for stem in ["heading", "social_row", "call-to-action", "b2"] {
            assert_eq!(
                theme_block_type(&format!("blocks/{stem}.slurp")),
                Some(stem)
            );
            assert!(
                stem.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            );
        }
    }

    const HERO: &str = r##"---
section {
  name: "Hero"
  settings {
    heading: text = "Welcome" { label: "Heading" }
    background: color = "#0a0a0a"
    align: select(left, center, right) = center
    max_items: number = 8 { min: 1, max: 24 }
    show_badge: toggle = true
  }
  blocks {
    faq_item {
      max: 2
      settings {
        question: text
        answer: richtext
      }
    }
  }
  max_per_page: 3
}
---
<div>${section.settings.heading}</div>
"##;

    #[test]
    fn test_defaults_when_no_saved_state() {
        let schema = schema_from(HERO);
        let merged = merged_section_value(&schema, &json!({}));
        let s = &merged["settings"];
        assert_eq!(s["heading"], json!("Welcome"));
        assert_eq!(s["background"], json!("#0a0a0a"));
        assert_eq!(s["align"], json!("center"));
        assert_eq!(s["max_items"], json!(8));
        assert_eq!(s["show_badge"], json!(true));
        assert_eq!(merged["blocks"], json!([]));
    }

    #[test]
    fn test_saved_overlay_and_unknown_keys_dropped() {
        let schema = schema_from(HERO);
        let state =
            json!({"id": "s1", "type": "hero", "settings": {"heading": "Hi", "ghost": "x"}});
        let merged = merged_section_value(&schema, &state);
        assert_eq!(merged["id"], json!("s1"));
        assert_eq!(merged["type"], json!("hero"));
        assert_eq!(merged["settings"]["heading"], json!("Hi"));
        assert!(
            merged["settings"].get("ghost").is_none(),
            "unknown key not dropped"
        );
    }

    #[test]
    fn test_select_clamped_to_options() {
        let schema = schema_from(HERO);
        let merged = merged_section_value(&schema, &json!({"settings": {"align": "diagonal"}}));
        assert_eq!(merged["settings"]["align"], json!("center"));
        let merged = merged_section_value(&schema, &json!({"settings": {"align": "right"}}));
        assert_eq!(merged["settings"]["align"], json!("right"));
    }

    #[test]
    fn test_number_clamped_to_meta_min_max() {
        let schema = schema_from(HERO);
        let merged = merged_section_value(&schema, &json!({"settings": {"max_items": 999}}));
        assert_eq!(merged["settings"]["max_items"], json!(24));
        let merged = merged_section_value(&schema, &json!({"settings": {"max_items": 0}}));
        assert_eq!(merged["settings"]["max_items"], json!(1));
    }

    #[test]
    fn test_type_mismatch_keeps_default() {
        let schema = schema_from(HERO);
        let state = json!({"settings": {"heading": 42, "show_badge": "yes", "max_items": "lots"}});
        let merged = merged_section_value(&schema, &state);
        assert_eq!(merged["settings"]["heading"], json!("Welcome"));
        assert_eq!(merged["settings"]["show_badge"], json!(true));
        assert_eq!(merged["settings"]["max_items"], json!(8));
    }

    #[test]
    fn test_blocks_merged_capped_and_unknown_dropped() {
        let schema = schema_from(HERO);
        let state = json!({"blocks": [
            {"id": "b1", "type": "faq_item", "settings": {"question": "Q1"}},
            {"id": "b2", "type": "alien", "settings": {}},
            {"id": "b3", "type": "faq_item", "settings": {"question": "Q2"}},
            {"id": "b4", "type": "faq_item", "settings": {"question": "Q3"}}
        ]});
        let merged = merged_section_value(&schema, &state);
        let blocks = merged["blocks"].as_array().unwrap();
        assert_eq!(
            blocks.len(),
            2,
            "max: 2 not enforced / unknown type kept: {blocks:?}"
        );
        assert_eq!(blocks[0]["settings"]["question"], json!("Q1"));
        assert_eq!(blocks[0]["settings"]["answer"], json!(""));
        assert_eq!(blocks[1]["settings"]["question"], json!("Q2"));
    }

    #[test]
    fn test_render_section_injects_merged_scope() {
        let (doc, diag) = parse(HERO, "hero.slurp").expect("lex failed");
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let state = json!({"settings": {"heading": "Big Sale"}});
        let (html, rdiag) = render_section(&doc, &state, &json!({}), BuildMode::Production);
        assert!(!rdiag.has_errors(), "{:?}", rdiag.errors);
        assert!(html.contains("Big Sale"), "merged setting missing: {html}");
    }

    #[test]
    fn test_render_section_without_schema_passes_state_through() {
        let (doc, _) = parse("<p>${section.settings.x}</p>", "raw.slurp").expect("lex failed");
        let state = json!({"settings": {"x": "thru"}});
        let (html, _) = render_section(&doc, &state, &json!({}), BuildMode::Production);
        assert!(html.contains("thru"), "{}", html);
    }

    #[test]
    fn test_blocks_iterable_in_each() {
        let src = r#"---
section {
  name: "FAQ"
  blocks { faq_item { settings { question: text = "?" } } }
}
---
{each block in section.blocks}<h3>${block.settings.question}</h3>{/each}
"#;
        let (doc, diag) = parse(src, "faq.slurp").expect("lex failed");
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let state = json!({"blocks": [
            {"type": "faq_item", "settings": {"question": "How?"}},
            {"type": "faq_item"}
        ]});
        let (html, _) = render_section(&doc, &state, &json!({}), BuildMode::Production);
        assert!(html.contains("How?"), "{}", html);
        assert!(html.contains("<h3>?</h3>"), "block default missing: {html}");
    }

    #[test]
    fn test_nested_blocks_parsed_merged_and_rendered() {
        // A block ("group") that declares its OWN blocks ("button"): blocks inside blocks.
        let src = r#"---
section {
  name: "Layout"
  blocks {
    group {
      settings { title: text = "Untitled" }
      blocks {
        button { max: 2 settings { label: text = "Click" } }
      }
    }
  }
}
---
{each g in section.blocks}<h3>${g.settings.title}</h3>{each b in g.blocks}<a>${b.settings.label}</a>{/each}{/each}
"#;
        let (doc, diag) = parse(src, "layout.slurp").expect("lex failed");
        assert!(!diag.has_errors(), "{:?}", diag.errors);

        // Schema captured the nested block def.
        let schema = doc
            .frontmatter
            .as_ref()
            .and_then(|f| f.section.as_ref())
            .unwrap();
        let group = &schema.blocks[0];
        assert_eq!(group.block_type, "group");
        assert_eq!(group.blocks.len(), 1, "nested block def not parsed");
        assert_eq!(group.blocks[0].block_type, "button");

        let state = json!({"blocks": [
            {"type": "group", "settings": {"title": "Hero"}, "blocks": [
                {"type": "button", "settings": {"label": "Buy"}},
                {"type": "button"},
                {"type": "button"},                 // exceeds max:2 - dropped
                {"type": "bogus"}                    // unknown nested type - dropped
            ]}
        ]});
        let (html, _) = render_section(&doc, &state, &json!({}), BuildMode::Production);
        assert!(html.contains("<h3>Hero</h3>"), "{}", html);
        assert!(html.contains("Buy"), "nested saved value missing: {html}");
        assert!(
            html.contains("<a>Click</a>"),
            "nested block default missing: {html}"
        );
        assert_eq!(
            html.matches("<a>").count(),
            2,
            "nested max not enforced: {html}"
        );
    }
}
