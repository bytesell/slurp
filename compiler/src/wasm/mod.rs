// Entry points use JSON-serialised strings to avoid complex Rust/JS type mapping.
//
// SECURITY NOTE: `parse`, `render`, `render_dev`, `validate`, `extract_schema` and
// `render_section` do NOT run `security_check`. Security enforcement (SLURP_SECRET_
// access, middleware scope, redirect restrictions) is a server-side concern, and those
// six exist for browser/editor hosts where a preview is not a deploy.
//
// `validate_full` is the deliberate exception, and the reason it is a SEPARATE export
// rather than a change to `validate`: an authoring tool wants exactly what `slurp
// build` and `slurp validate` enforce, because a rule like "an unfiltered `${ }` may
// not appear in a `<script>` body" lives in the security walk and not in the parser.
// A tool that only ever calls `validate` therefore reports a file clean that the build
// then rejects. Keeping both means the editor-preview hosts are unaffected.
use wasm_bindgen::prelude::*;

use crate::error::{CompileError, ErrorAccumulator};
use crate::parser;
use crate::renderer::{self, BuildMode};

#[derive(serde::Serialize)]
struct ParseResult {
    ok: bool,
    ast: Option<serde_json::Value>,
    errors: Vec<ErrorJson>,
    warnings: Vec<ErrorJson>,
}

#[derive(serde::Serialize)]
struct RenderResult {
    ok: bool,
    html: String,
    errors: Vec<ErrorJson>,
    warnings: Vec<ErrorJson>,
}

#[derive(serde::Serialize)]
struct SchemaResult {
    ok: bool,
    /// The `section {}` schema as editor JSON, or null when none is declared.
    schema: Option<serde_json::Value>,
    errors: Vec<ErrorJson>,
    warnings: Vec<ErrorJson>,
}

#[derive(serde::Serialize)]
struct ErrorJson {
    code: String,
    message: String,
    file: String,
    line: usize,
    column: usize,
    severity: String,
}

impl From<&CompileError> for ErrorJson {
    fn from(e: &CompileError) -> Self {
        Self {
            code: format!("{:?}", e.code),
            message: e.message.clone(),
            file: e.file.clone(),
            line: e.line,
            column: e.column,
            severity: format!("{}", e.severity),
        }
    }
}

fn acc_to_json(acc: &ErrorAccumulator) -> (Vec<ErrorJson>, Vec<ErrorJson>) {
    let errors = acc.errors.iter().map(ErrorJson::from).collect();
    let warnings = acc.warnings.iter().map(ErrorJson::from).collect();
    (errors, warnings)
}

/// Parse a Slurp template source string. Returns a JSON-encoded `ParseResult`.
#[must_use]
#[wasm_bindgen]
pub fn parse(source: &str) -> JsValue {
    let result = match parser::parse(source, "wasm-input") {
        Err(lex_err) => {
            let err_json = ErrorJson {
                code: format!("{:?}", lex_err.code),
                message: lex_err.message.clone(),
                file: lex_err.file.clone(),
                line: lex_err.line,
                column: lex_err.column,
                severity: format!("{}", lex_err.severity),
            };
            ParseResult {
                ok: false,
                ast: None,
                errors: vec![err_json],
                warnings: vec![],
            }
        }
        Ok((doc, diag)) => {
            let (errors, warnings) = acc_to_json(&diag);
            let ast = serde_json::to_value(&doc).ok();
            ParseResult {
                ok: !diag.has_errors(),
                ast,
                errors,
                warnings,
            }
        }
    };

    to_js_value(&result)
}

/// Render a previously-parsed AST JSON string with the given context JSON. Returns a JSON-encoded `RenderResult`.
///
/// Renders in [`BuildMode::Production`]. See [`render_dev`] for the editor mode.
#[must_use]
#[wasm_bindgen]
pub fn render(ast_json: &str, context_json: &str) -> JsValue {
    render_in(ast_json, context_json, BuildMode::Production)
}

/// Render in [`BuildMode::Development`]: `{debug}` nodes render, and the
/// renderer's advisory warnings are recorded.
///
/// This exists because two diagnostics are emitted ONLY in development mode:
/// the CSS-structural strip and the undeclared JavaScript attribute slot. A
/// host that only ever calls [`render`] therefore watches a style value lose
/// characters with no diagnostic at all. That is the right trade for a
/// production SSR hot path and the wrong one for an editor preview, which is
/// exactly what these WASM exports are for.
///
/// The BUDGET diagnostics are not among them. Loop truncation, the output-byte
/// budget and the render-depth budget are reported in both modes, because each
/// one silently removes content from the page.
#[must_use]
#[wasm_bindgen]
pub fn render_dev(ast_json: &str, context_json: &str) -> JsValue {
    render_in(ast_json, context_json, BuildMode::Development)
}

fn render_in(ast_json: &str, context_json: &str, mode: BuildMode) -> JsValue {
    let doc: crate::ast::Document = match serde_json::from_str(ast_json) {
        Ok(d) => d,
        Err(e) => {
            let result = RenderResult {
                ok: false,
                html: String::new(),
                errors: vec![ErrorJson {
                    code: "InvalidAst".into(),
                    message: format!("Invalid AST JSON: {}", e),
                    file: "wasm-input".into(),
                    line: 0,
                    column: 0,
                    severity: "error".into(),
                }],
                warnings: vec![],
            };
            return to_js_value(&result);
        }
    };

    let context: serde_json::Value = match serde_json::from_str(context_json) {
        Ok(c) => c,
        Err(e) => {
            let result = RenderResult {
                ok: false,
                html: String::new(),
                errors: vec![ErrorJson {
                    code: "InvalidContext".into(),
                    message: format!("Invalid context JSON: {}", e),
                    file: "wasm-input".into(),
                    line: 0,
                    column: 0,
                    severity: "error".into(),
                }],
                warnings: vec![],
            };
            return to_js_value(&result);
        }
    };

    let (html, diag) = renderer::render(&doc, &context, mode);
    let (errors, warnings) = acc_to_json(&diag);
    let result = RenderResult {
        ok: !diag.has_errors(),
        html,
        errors,
        warnings,
    };

    to_js_value(&result)
}

/// Validate a Slurp template, returning a JSON-encoded array of `ErrorJson` objects.
#[must_use]
#[wasm_bindgen]
pub fn validate(source: &str) -> JsValue {
    let all_errors: Vec<ErrorJson> = match parser::parse(source, "wasm-input") {
        Err(lex_err) => {
            vec![ErrorJson::from(&lex_err)]
        }
        Ok((_, diag)) => diag.all().map(ErrorJson::from).collect(),
    };

    to_js_value(&all_errors)
}

/// Validate a template the way a BUILD does: every parse diagnostic PLUS the
/// compile-time security walk. Returns a JSON-encoded array of `ErrorJson`.
///
/// This is [`validate`] plus [`crate::check_security`], and it exists because the
/// two do not report the same thing. Parsing alone misses the rules the walk owns:
/// an unfiltered `${ }` in a `<script>` body, a `| js` slot in JavaScript statement
/// position, `env.SLURP_SECRET_*`, `request.*` outside middleware, and `{redirect}`
/// outside middleware. An authoring tool that reports only parse diagnostics tells
/// an author their file is clean and then the build rejects it.
///
/// `is_middleware` must match the value the eventual compile will use, because the
/// rules genuinely differ: `request.*` and `{redirect}` are legal ONLY in middleware
/// and most other roots are illegal there. A caller with no way to know passes
/// `false`, which is what `compile`, `compile_with_registry` and the `slurp validate`
/// CLI all do.
///
/// ONE BEHAVIOURAL DIFFERENCE worth knowing at the call site: the security walk
/// returns on its FIRST violation (it is a `Result`, not an accumulator), so at most
/// one security diagnostic is appended per call, whereas parse diagnostics accumulate.
/// Fix the reported one and re-run to see the next.
#[must_use]
#[wasm_bindgen]
pub fn validate_full(source: &str, is_middleware: bool) -> JsValue {
    let all_errors: Vec<ErrorJson> = match parser::parse(source, "wasm-input") {
        Err(lex_err) => vec![ErrorJson::from(&lex_err)],
        Ok((doc, diag)) => {
            let mut out: Vec<ErrorJson> = diag.all().map(ErrorJson::from).collect();
            if let Err(sec_err) = crate::check_security(&doc, "wasm-input", is_middleware) {
                out.push(ErrorJson::from(&sec_err));
            }
            out
        }
    };

    to_js_value(&all_errors)
}

/// Extract the `section {}` schema from a template source. Returns a
/// JSON-encoded `SchemaResult` - the editor uses `schema` to generate the
/// section settings form.
#[must_use]
#[wasm_bindgen]
pub fn extract_schema(source: &str) -> JsValue {
    let result = match parser::parse(source, "wasm-input") {
        Err(lex_err) => SchemaResult {
            ok: false,
            schema: None,
            errors: vec![ErrorJson::from(&lex_err)],
            warnings: vec![],
        },
        Ok((doc, diag)) => {
            let (errors, warnings) = acc_to_json(&diag);
            let schema = doc
                .frontmatter
                .as_ref()
                .and_then(|f| f.section.as_ref())
                .and_then(|s| serde_json::to_value(s).ok());
            SchemaResult {
                ok: !diag.has_errors(),
                schema,
                errors,
                warnings,
            }
        }
    };
    to_js_value(&result)
}

/// Render a section template AST with saved section state merged over its
/// schema defaults (instant preview of fetch-free sections). Returns a
/// JSON-encoded `RenderResult`.
#[must_use]
#[wasm_bindgen]
pub fn render_section(ast_json: &str, section_state_json: &str, context_json: &str) -> JsValue {
    fn input_error(code: &str, message: String) -> RenderResult {
        RenderResult {
            ok: false,
            html: String::new(),
            errors: vec![ErrorJson {
                code: code.into(),
                message,
                file: "wasm-input".into(),
                line: 0,
                column: 0,
                severity: "error".into(),
            }],
            warnings: vec![],
        }
    }

    let doc: crate::ast::Document = match serde_json::from_str(ast_json) {
        Ok(d) => d,
        Err(e) => {
            return to_js_value(&input_error(
                "InvalidAst",
                format!("Invalid AST JSON: {}", e),
            ));
        }
    };
    let state: serde_json::Value = match serde_json::from_str(section_state_json) {
        Ok(s) => s,
        Err(e) => {
            return to_js_value(&input_error(
                "InvalidSectionState",
                format!("Invalid section state JSON: {}", e),
            ));
        }
    };
    let context: serde_json::Value = match serde_json::from_str(context_json) {
        Ok(c) => c,
        Err(e) => {
            return to_js_value(&input_error(
                "InvalidContext",
                format!("Invalid context JSON: {}", e),
            ));
        }
    };

    let (html, diag) =
        crate::sections::render_section(&doc, &state, &context, BuildMode::Production);
    let (errors, warnings) = acc_to_json(&diag);
    let result = RenderResult {
        ok: !diag.has_errors(),
        html,
        errors,
        warnings,
    };
    to_js_value(&result)
}

/// Serialise via a JSON string rather than serde-wasm-bindgen to keep the WASM binary small.
fn to_js_value<T: serde::Serialize>(value: &T) -> JsValue {
    let json = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    JsValue::from_str(&json)
}
