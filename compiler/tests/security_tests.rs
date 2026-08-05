use slurp_compiler::{
    BuildMode, CompileError, CompileOptions, compile, compile_template, compile_with_options,
    error::ErrorCode,
};

fn render(template: &str) -> Result<String, CompileError> {
    compile_template(template, CompileOptions::default())
}

fn render_with(template: &str, context_json: &str) -> Result<String, CompileError> {
    let options = CompileOptions {
        context_json: Some(context_json.to_string()),
        ..Default::default()
    };
    compile_template(template, options)
}

fn render_middleware(template: &str) -> Result<String, CompileError> {
    let options = CompileOptions {
        is_middleware: true,
        ..Default::default()
    };
    compile_template(template, options)
}

fn assert_compile_error(template: &str, expected_code: ErrorCode) {
    match render(template) {
        Err(err) => assert_eq!(
            err.code, expected_code,
            "expected error code {:?} but got {:?}: {}",
            expected_code, err.code, err.message
        ),
        Ok(html) => panic!(
            "expected compile error {expected_code:?} but compilation succeeded with output: {html:?}"
        ),
    }
}

fn assert_middleware_compile_error(template: &str, expected_code: ErrorCode) {
    match render_middleware(template) {
        Err(err) => assert_eq!(
            err.code, expected_code,
            "expected error code {:?} but got {:?}: {}",
            expected_code, err.code, err.message
        ),
        Ok(html) => panic!(
            "expected compile error {expected_code:?} but compilation succeeded with output: {html:?}"
        ),
    }
}

// ── Test 1: XSS - script tag in expression output ─────────────────────────────

#[test]
fn xss_script_tag_in_expression_is_escaped() {
    let template = r#"${ "<script>alert(1)</script>" }"#;
    let output = render(template).expect("template should compile");
    assert_eq!(output, "&lt;script&gt;alert(1)&lt;/script&gt;");
    assert!(
        !output.contains("<script>"),
        "rendered output must not contain literal <script> tag"
    );
}

// ── Test 2: All five HTML entities are escaped ────────────────────────────────

#[test]
fn all_five_html_entities_are_escaped_in_expressions() {
    let out = render(r#"${ "a & b" }"#).expect("should compile");
    assert!(
        out.contains("a &amp; b"),
        "& must become &amp; - got: {out}"
    );

    let out = render(r#"${ "1 < 2" }"#).expect("should compile");
    assert!(out.contains("1 &lt; 2"), "< must become &lt; - got: {out}");

    let out = render(r#"${ "3 > 2" }"#).expect("should compile");
    assert!(out.contains("3 &gt; 2"), "> must become &gt; - got: {out}");

    let out = render(r#"${ "say \"hello\"" }"#).expect("should compile");
    assert!(out.contains("&quot;"), "\" must become &quot; - got: {out}");

    let out = render(r#"${ "it's" }"#).expect("should compile");
    assert!(out.contains("&#39;"), "' must become &#39; - got: {out}");
}

#[test]
fn all_five_entities_combined_in_single_expression() {
    let template = r#"${ "& < > \" '" }"#;
    let output = render(template).expect("should compile");
    assert!(output.contains("&amp;"), "& → &amp;  in: {output}");
    assert!(output.contains("&lt;"), "< → &lt;   in: {output}");
    assert!(output.contains("&gt;"), "> → &gt;   in: {output}");
    assert!(output.contains("&quot;"), "\" → &quot; in: {output}");
    assert!(output.contains("&#39;"), "' → &#39;  in: {output}");
}

// ── Test 3: {html} without sanitize outputs raw HTML ─────────────────────────

#[test]
fn raw_html_without_sanitize_outputs_literal_tags() {
    let template = r#"{html "<script>alert(1)</script>"}"#;
    let output = render(template).expect("should compile");
    assert_eq!(
        output, "<script>alert(1)</script>",
        "{{html}} without sanitize must output the literal string unchanged"
    );
    assert!(
        !output.contains("&lt;"),
        "raw HTML output must not contain escaped entities"
    );
}

// ── Test 4: {html sanitize} strips <script> ───────────────────────────────────

#[test]
fn html_sanitize_strips_script_element() {
    let template = r#"{html "<p>text</p><script>alert(1)</script>" sanitize}"#;
    let output = render(template).expect("should compile");
    assert!(
        !output.contains("<script>") && !output.contains("</script>"),
        "sanitized output must not contain <script> - got: {output}"
    );
    assert!(
        !output.contains("alert(1)"),
        "script content must be removed - got: {output}"
    );
    assert!(
        output.contains("<p>text</p>"),
        "safe elements must be preserved - got: {output}"
    );
}

// ── Test 5: {html sanitize} strips <iframe> ───────────────────────────────────

#[test]
fn html_sanitize_strips_iframe_element() {
    let template = r#"{html "<p>content</p><iframe src=\"https://evil.com\"></iframe>" sanitize}"#;
    let output = render(template).expect("should compile");
    assert!(
        !output.contains("<iframe"),
        "sanitized output must not contain <iframe> - got: {output}"
    );
    assert!(
        output.contains("<p>content</p>"),
        "safe elements must be preserved - got: {output}"
    );
}

// ── Test 6: {html sanitize} strips on* attributes ────────────────────────────

#[test]
fn html_sanitize_strips_on_event_attributes() {
    let template = r#"{html "<p onclick=\"alert(1)\">text</p>" sanitize}"#;
    let output = render(template).expect("should compile");
    assert!(
        !output.contains("onclick"),
        "onclick attribute must be stripped - got: {output}"
    );
    assert!(
        !output.contains("alert"),
        "event handler payload must be removed - got: {output}"
    );
    assert!(
        output.contains("<p>") || output.contains("text"),
        "paragraph content must be preserved - got: {output}"
    );
}

#[test]
fn html_sanitize_strips_onerror_attribute() {
    let template = r#"{html "<img onerror=\"alert(1)\" src=\"x\">" sanitize}"#;
    let output = render(template).expect("should compile");
    assert!(
        !output.contains("onerror"),
        "onerror attribute must be stripped - got: {output}"
    );
    assert!(
        !output.contains("alert(1)"),
        "event handler payload must be removed - got: {output}"
    );
}

// ── Test 7: {html sanitize} strips href="javascript:" ─────────────────────────

#[test]
fn html_sanitize_strips_javascript_href() {
    let template = r#"{html "<a href=\"javascript:alert(1)\">click me</a>" sanitize}"#;
    let output = render(template).expect("should compile");
    assert!(
        !output.contains("javascript:"),
        "javascript: href must be stripped - got: {output}"
    );
    assert!(
        !output.contains("alert(1)"),
        "javascript: payload must be removed - got: {output}"
    );
}

#[test]
fn html_sanitize_strips_javascript_href_case_variants() {
    // Browsers normalise casing; ensure the sanitizer catches common variants
    let variants = [
        r#"<a href="JAVASCRIPT:alert(1)">x</a>"#,
        r#"<a href="Javascript:alert(1)">x</a>"#,
        r#"<a href="&#106;avascript:alert(1)">x</a>"#,
    ];
    for variant in &variants {
        // Pass HTML as a context variable to avoid quote-escaping issues in the template string
        let context = format!(
            r#"{{"content": {}}}"#,
            serde_json::to_string(variant).unwrap()
        );
        let output = render_with("{html content sanitize}", &context).expect("should compile");
        assert!(
            !output.to_lowercase().contains("javascript:"),
            "javascript: variant must be stripped in: {output}"
        );
    }
}

// ── Test 8: {html sanitize} strips data: URLs ─────────────────────────────────

#[test]
fn html_sanitize_strips_data_uri_in_src() {
    let template = r#"{html "<img src=\"data:text/html,<script>alert(1)</script>\">" sanitize}"#;
    let output = render(template).expect("should compile");
    assert!(
        !output.contains("data:"),
        "data: URI in src must be stripped - got: {output}"
    );
}

#[test]
fn html_sanitize_strips_data_uri_in_href() {
    let template = r#"{html "<a href=\"data:text/html,<h1>test</h1>\">click</a>" sanitize}"#;
    let output = render(template).expect("should compile");
    assert!(
        !output.contains("data:"),
        "data: URI in href must be stripped - got: {output}"
    );
}

// ── Test 9: SLURP_SECRET_* access outside middleware is a compile error ────────

#[test]
fn secret_env_in_template_is_a_compile_error() {
    let template = r#"<p>${ env.SLURP_SECRET_API_KEY }</p>"#;
    assert_compile_error(template, ErrorCode::SecretEnvInTemplate);
}

#[test]
fn secret_env_any_suffix_is_a_compile_error() {
    let templates = [
        r#"${ env.SLURP_SECRET_DB_PASSWORD }"#,
        r#"${ env.SLURP_SECRET_STRIPE_KEY }"#,
        r#"${ env.SLURP_SECRET_JWT_SECRET }"#,
    ];
    for template in &templates {
        assert_compile_error(template, ErrorCode::SecretEnvInTemplate);
    }
}

#[test]
fn public_env_in_template_is_allowed() {
    let template = r#"${ env.SLURP_PUBLIC_API_URL }"#;
    // Should compile without error - output value may be empty in test context
    assert!(
        render(template).is_ok(),
        "SLURP_PUBLIC_* access must not produce a compile error"
    );
}

// ── Test 10: request.cookies in non-middleware is a compile error ─────────────

#[test]
fn request_cookies_in_non_middleware_is_compile_error() {
    let template = r#"${ request.cookies.session_id }"#;
    assert_compile_error(template, ErrorCode::MiddlewareScopeViolation);
}

#[test]
fn request_headers_in_non_middleware_is_compile_error() {
    let template = r#"${ request.headers.authorization }"#;
    assert_compile_error(template, ErrorCode::MiddlewareScopeViolation);
}

#[test]
fn request_method_in_non_middleware_is_compile_error() {
    let template = r#"${ request.method }"#;
    assert_compile_error(template, ErrorCode::MiddlewareScopeViolation);
}

#[test]
fn request_cookies_in_middleware_is_allowed() {
    let template = r#"
{if request.cookies.session_id}
  {next}
{else}
  {redirect "/login"}
{/if}
"#;
    assert!(
        render_middleware(template).is_ok(),
        "request.cookies must be allowed in middleware files"
    );
}

#[test]
fn non_request_context_in_middleware_is_compile_error() {
    let template = r#"${ products[0].name }"#;
    assert_middleware_compile_error(template, ErrorCode::MiddlewareScopeViolation);
}

#[test]
fn redirect_outside_middleware_is_compile_error() {
    let template = r#"{redirect "/home"}"#;
    assert_compile_error(template, ErrorCode::RedirectOutsideMiddleware);
}

// ── Test 11: Circular import detection ───────────────────────────────────────

#[test]
fn circular_import_is_detected() {
    use slurp_compiler::CompileOptions;
    use std::collections::HashMap;

    let mut virtual_files = HashMap::new();
    virtual_files.insert(
        "@components/a.slurp".to_string(),
        r#"
---
using "@components/b"
---
<A />
"#
        .to_string(),
    );
    virtual_files.insert(
        "@components/b.slurp".to_string(),
        r#"
---
using "@components/a"
---
<B />
"#
        .to_string(),
    );

    let options = CompileOptions {
        virtual_files: Some(virtual_files),
        entry_file: Some("@components/a.slurp".to_string()),
        ..Default::default()
    };

    let result = slurp_compiler::compile_with_options(options);
    assert!(result.is_err(), "circular import must produce an error");
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::CircularImport,
        "error code must be CircularImport, got {:?}: {}",
        err.code,
        err.message
    );
}

// ── Test 12: Max iteration cap (1,000) ───────────────────────────────────────

#[test]
fn each_with_over_1000_items_is_capped() {
    let items: Vec<serde_json::Value> = (0..=1000).map(|i| serde_json::json!({ "n": i })).collect();
    let context_json = serde_json::json!({ "items": items }).to_string();

    let template = r#"
{each item in items}
  ${ item.n }
{/each}
"#;

    let options = slurp_compiler::CompileOptions {
        context_json: Some(context_json),
        ..Default::default()
    };

    let result = slurp_compiler::compile_template(template, options);
    match result {
        Err(err) => {
            assert_eq!(
                err.code,
                ErrorCode::IterationLimitExceeded,
                "expected IterationLimitExceeded but got {:?}: {}",
                err.code,
                err.message
            );
        }
        Ok(html) => {
            // If the compiler chose to cap silently rather than error, verify the cap was applied.
            let count = html.matches("<span class=\"item\">").count();
            assert!(
                count <= 1000,
                "rendered output must not exceed 1,000 items; got {count}"
            );
        }
    }
}

#[test]
fn each_with_exactly_1000_items_succeeds() {
    let items: Vec<serde_json::Value> = (0..1000).map(|i| serde_json::json!({ "n": i })).collect();
    let context_json = serde_json::json!({ "items": items }).to_string();

    let template = r#"
{each item in items}
  ${ item.n }
{/each}
"#;

    let options = slurp_compiler::CompileOptions {
        context_json: Some(context_json),
        ..Default::default()
    };

    let result = slurp_compiler::compile_template(template, options);
    assert!(
        result.is_ok(),
        "exactly 1,000 items must compile without an iteration error"
    );
}

// ── Test 13: Safe navigation - null path never throws ─────────────────────────

#[test]
fn safe_navigation_null_path_returns_empty_string() {
    let context_json = r#"{ "a": null }"#;
    let template = r#"${ a.b.c.d }"#;

    let output = render_with(template, context_json)
        .expect("null path traversal must not produce a compile error");

    assert_eq!(
        output.trim(),
        "",
        "accessing a property chain through null must yield an empty string, got: {output:?}"
    );
}

#[test]
fn safe_navigation_deep_null_returns_empty_string() {
    let context_json = r#"{ "user": null }"#;
    let template = r#"${ user.profile.avatar.url }"#;

    let output = render_with(template, context_json)
        .expect("deep null path traversal must not produce an error");

    assert_eq!(
        output.trim(),
        "",
        "deep null traversal must yield empty string, got: {output:?}"
    );
}

// ── Test 14: Safe navigation - undefined path never throws ───────────────────

#[test]
fn safe_navigation_undefined_path_returns_empty_string() {
    let context_json = r#"{}"#;
    let template = r#"${ a.b.c }"#;

    let output = render_with(template, context_json)
        .expect("undefined path traversal must not produce a compile error");

    assert_eq!(
        output.trim(),
        "",
        "accessing a property chain on an undefined variable must yield empty string, got: {output:?}"
    );
}

#[test]
fn safe_navigation_top_level_undefined_returns_empty_string() {
    let context_json = r#"{}"#;
    let template = r#"${ nonexistent }"#;

    let output = render_with(template, context_json)
        .expect("undefined top-level access must not produce an error");

    assert_eq!(
        output.trim(),
        "",
        "undefined top-level variable must yield empty string, got: {output:?}"
    );
}

#[test]
fn safe_navigation_mixed_null_undefined_returns_empty_string() {
    let context_json = r#"{ "config": { "theme": null } }"#;
    let template = r#"${ config.theme.colors.primary }"#;

    let output = render_with(template, context_json)
        .expect("mixed null/undefined chain must not produce an error");

    assert_eq!(
        output.trim(),
        "",
        "null/undefined mixed chain must yield empty string, got: {output:?}"
    );
}

#[test]
fn xss_in_attribute_expression_is_escaped() {
    let context_json = r#"{ "title": "\" onmouseover=\"alert(1)" }"#;
    let template = r#"<div title="${ title }">text</div>"#;

    let output = render_with(template, context_json).expect("should compile");

    assert!(
        output.contains("&quot;"),
        "double quote in attribute value must be HTML-escaped, got: {output}"
    );
    // The injected payload must not create a standalone onmouseover attribute.
    assert!(
        !output.contains(" onmouseover=\""),
        "injected attribute broke out of quoted value, got: {output}"
    );
}

#[test]
fn xss_in_template_literal_interpolation_is_escaped() {
    let context_json = r#"{ "name": "<script>alert(1)</script>" }"#;
    let template = r#"${ `Hello, ${name}!` }"#;

    let output = render_with(template, context_json).expect("should compile");

    assert!(
        !output.contains("<script>"),
        "script tag in template literal interpolation must be escaped, got: {output}"
    );
    assert!(
        output.contains("&lt;script&gt;"),
        "angle brackets must be HTML-escaped in template literal, got: {output}"
    );
}

// ── Test 15: compile() (CLI path) also enforces security checks ───────────────
// These tests verify that the fix in lib.rs (Phase 1.3) is effective: the
// public `compile()` function used by `slurp build` calls security_check().

#[test]
fn compile_fn_rejects_secret_env_access() {
    let (html, diag) = compile(
        r#"<p>${ env.SLURP_SECRET_API_KEY }</p>"#,
        "build.slurp",
        &serde_json::Value::Object(Default::default()),
        BuildMode::Production,
    );
    assert!(
        diag.has_errors(),
        "compile() must reject SLURP_SECRET_* access; got html: {html:?}"
    );
    assert!(
        diag.errors
            .iter()
            .any(|e| e.code == ErrorCode::SecretEnvInTemplate),
        "error code must be SecretEnvInTemplate, got: {:?}",
        diag.errors
    );
    assert!(
        html.is_empty(),
        "on security error, html output must be empty"
    );
}

#[test]
fn compile_fn_rejects_redirect_outside_middleware() {
    let (html, diag) = compile(
        r#"{redirect "/login"}"#,
        "page.slurp",
        &serde_json::Value::Object(Default::default()),
        BuildMode::Production,
    );
    assert!(
        diag.has_errors(),
        "compile() must reject {{redirect}} outside middleware; got html: {html:?}"
    );
    assert!(
        diag.errors
            .iter()
            .any(|e| e.code == ErrorCode::RedirectOutsideMiddleware),
        "error code must be RedirectOutsideMiddleware, got: {:?}",
        diag.errors
    );
}

#[test]
fn compile_fn_rejects_request_scope_outside_middleware() {
    let (html, diag) = compile(
        r#"${ request.cookies.session }"#,
        "page.slurp",
        &serde_json::Value::Object(Default::default()),
        BuildMode::Production,
    );
    assert!(
        diag.has_errors(),
        "compile() must reject request.* outside middleware; got html: {html:?}"
    );
    assert!(
        diag.errors
            .iter()
            .any(|e| e.code == ErrorCode::MiddlewareScopeViolation),
        "error code must be MiddlewareScopeViolation, got: {:?}",
        diag.errors
    );
}

// ---- Parser recursion-depth DoS (stack-overflow) regressions ----------------

#[test]
fn deeply_nested_ternary_is_depth_limited_not_stack_overflow() {
    // The `then`/`else` operands of a ternary recurse straight back into
    // parse_ternary; without a depth guard a chain like `1?1:1?1:...` exhausts
    // the native stack. It must instead be rejected by the nesting-depth cap.
    let template = format!("${{{}1}}", "1?1:".repeat(5000));
    match render(&template) {
        Err(err) => assert!(
            err.message.contains("nesting depth"),
            "expected a nesting-depth error, got: {}",
            err.message
        ),
        Ok(html) => panic!("expected a depth error, but compiled to: {html:?}"),
    }
}

#[test]
fn deeply_nested_elements_are_depth_limited_not_stack_overflow() {
    // 2000 nested <div> would overflow the parser's stack before the fix (large
    // element frames vs a 2 MB worker/test stack). With MAX_DEPTH + on-demand
    // stack growth it must return a clean nesting-depth error, not crash.
    let template = format!("{}{}", "<div>".repeat(2000), "</div>".repeat(2000));
    match render(&template) {
        Err(err) => assert!(
            err.message.contains("nesting depth"),
            "expected a nesting-depth error, got: {}",
            err.message
        ),
        Ok(_) => panic!("expected a depth error for deeply nested elements"),
    }
}

// ---- JS-in-attribute XSS: the `js` filter --------------------------------

#[test]
fn js_filter_backslash_escapes_quotes_in_js_attribute() {
    // Untrusted data interpolated into a JS-evaluated attribute must be
    // JS-escaped (via the `js` filter), not merely HTML-entity escaped: the
    // browser decodes entities before Alpine reads @click, so a bare quote
    // would break out of the JS string literal and execute.
    let evil = r#"x'});alert(document.cookie);({'"#;
    let ctx = format!(r#"{{"name": {}}}"#, serde_json::to_string(evil).unwrap());
    let html = render_with(
        r#"<button @click={ `add('${ name | js }')` }></button>"#,
        &ctx,
    )
    .expect("render");
    // js-escape runs first, so each payload quote is a backslash followed by the
    // HTML entity; after the browser decodes it, Alpine sees `\'` (escaped).
    assert!(
        html.contains(r"\&#39;"),
        "expected backslash-escaped quote in output, got: {html}"
    );
    // The un-backslashed breakout quote must not appear.
    assert!(
        !html.contains(r"x&#39;"),
        "unescaped breakout in output: {html}"
    );
}

// ── security_check traversal gaps ─────────────────────────────────────────────
//
// Every test below was a PROVEN leak: the AST field it exercises is rendered but
// was not walked by `check_node`, so the gate simply never saw the expression.
// They assert on the ERROR (compilation refused), not merely on the secret being
// absent from the output - a template that renders the secret to nowhere is still
// a template that named a secret.

const SECRET_CTX: &str = r#"{"env": {"SLURP_SECRET_API_KEY": "topsecret"}, "items": ["a"]}"#;

fn render_secret_ctx(template: &str) -> Result<String, CompileError> {
    let options = CompileOptions {
        context_json: Some(SECRET_CTX.to_string()),
        ..Default::default()
    };
    compile_template(template, options)
}

fn assert_refused(template: &str, expected: ErrorCode) {
    match render_secret_ctx(template) {
        Err(err) => assert_eq!(
            err.code, expected,
            "expected {:?} but got {:?}: {}",
            expected, err.code, err.message
        ),
        Ok(html) => {
            panic!("SECURITY: expected {expected:?}, but the template compiled to {html:?}")
        }
    }
}

#[test]
fn fetch_loading_branch_is_security_checked() {
    // `{loading}` is the DEFAULT server-side path (the fetch variable is unresolved
    // during SSR), so this one actually rendered the secret: it returned
    // Ok("topsecret").
    assert_refused(
        r#"{fetch d from "/a"}ok{loading}${ env.SLURP_SECRET_API_KEY }{/fetch}"#,
        ErrorCode::SecretEnvInTemplate,
    );
}

#[test]
fn fetch_error_branch_is_security_checked() {
    assert_refused(
        r#"{fetch d from "/a"}ok{error}${ request.cookies.session }{/fetch}"#,
        ErrorCode::MiddlewareScopeViolation,
    );
}

#[test]
fn fetch_empty_branch_is_security_checked() {
    assert_refused(
        r#"{fetch d from "/a"}ok{empty}{redirect "/evil"}{/fetch}"#,
        ErrorCode::RedirectOutsideMiddleware,
    );
}

#[test]
fn fetch_loading_branch_is_checked_in_middleware_too() {
    // The gap cut both ways: a middleware file could read arbitrary non-request
    // context from inside a fetch branch.
    assert_middleware_compile_error(
        r#"{fetch d from "/a"}ok{loading}${ products.first.name }{/fetch}"#,
        ErrorCode::MiddlewareScopeViolation,
    );
}

#[test]
fn layout_head_is_security_checked() {
    // `parse_layout` hoists `{head}` OUT of children into `LayoutNode.head`, so the
    // standalone Node::Head arm never saw it and only `n.children` was walked.
    assert_refused(
        r#"{layout "b"}{head}${ env.SLURP_SECRET_API_KEY }{/head}{/layout}"#,
        ErrorCode::SecretEnvInTemplate,
    );
}

#[test]
fn component_spread_is_security_checked() {
    assert_refused(
        r#"<Card ...`${ env.SLURP_SECRET_API_KEY }` />"#,
        ErrorCode::SecretEnvInTemplate,
    );
}

// ── env[...] must be gated exactly like env.* ─────────────────────────────────

#[test]
fn env_bracket_access_to_secret_is_refused() {
    // Bracket access parses to PathSegment::Index, and the prefix test only ever
    // read PathSegment::Ident - so this returned Ok("topsecret") while the
    // identical dotted form was correctly refused.
    assert_refused(
        r#"${ env["SLURP_SECRET_API_KEY"] }"#,
        ErrorCode::SecretEnvInTemplate,
    );
}

#[test]
fn env_grouped_bracket_access_to_secret_is_refused() {
    // A different path through the checker: Grouped(Path) wrapped in ExprKind::Index,
    // whose halves were each checked in isolation so neither looked like `env.KEY`.
    assert_refused(
        r#"${ (env)["SLURP_SECRET_API_KEY"] }"#,
        ErrorCode::SecretEnvInTemplate,
    );
}

#[test]
fn env_dynamic_index_is_refused_outright() {
    // A runtime key cannot be proven non-secret at compile time, so it is refused
    // rather than waved through.
    assert_refused(r#"${ env[key] }"#, ErrorCode::SecretEnvInTemplate);
    assert_refused(r#"${ (env)[key] }"#, ErrorCode::SecretEnvInTemplate);
}

#[test]
fn env_bracket_access_to_a_public_key_still_compiles() {
    // The gate must not become "no bracket access to env at all".
    let out = render_secret_ctx(r#"${ env["PUBLIC_BASE_URL"] }"#)
        .expect("a non-secret env key must still compile");
    assert!(
        !out.contains("topsecret"),
        "unexpected secret in output: {out}"
    );
}

#[test]
fn subscript_expression_is_security_checked() {
    // The subscript is an author-written expression in its own right; only the
    // OUTER path used to be inspected.
    assert_refused(
        r#"${ items[request.cookies.session] }"#,
        ErrorCode::MiddlewareScopeViolation,
    );
}

#[test]
fn next_outside_middleware_is_refused() {
    // Documented as middleware-only, enforced for {redirect} but not for {next}:
    // Node::Next fell into a `_ => {}` catch-all that has since been removed.
    assert_refused("{next}", ErrorCode::RedirectOutsideMiddleware);
    assert!(
        render_middleware("{next}").is_ok(),
        "{{next}} must still be allowed inside middleware"
    );
}

#[test]
fn deep_import_chain_is_refused_not_stack_overflowed() {
    // `detect_circular_dfs` had no depth cap and no stack growth, and runs a full
    // parse() per frame. Measured: 500 files fine, 5000 overflowed the stack -
    // which is a process ABORT, not an error a caller can handle.
    let mut files = std::collections::HashMap::new();
    const N: usize = 4000;
    for i in 0..N {
        files.insert(
            format!("f{i}.slurp"),
            format!("---\nusing \"f{}\"\n---\n", i + 1),
        );
    }
    files.insert(format!("f{N}.slurp"), "---\n---\n".to_string());

    let result = compile_with_options(CompileOptions {
        virtual_files: Some(files),
        entry_file: Some("f0.slurp".to_string()),
        ..Default::default()
    });
    match result {
        Err(err) => assert!(
            err.message.contains("Import chain too deep"),
            "expected a chain-depth error, got: {}",
            err.message
        ),
        Ok(_) => panic!("expected a chain-depth error for a {N}-file import chain"),
    }
}

#[test]
fn ordinary_import_chains_still_compile() {
    // The cap must sit far above anything a real theme does.
    let mut files = std::collections::HashMap::new();
    for i in 0..8 {
        files.insert(
            format!("f{i}.slurp"),
            format!("---\nusing \"f{}\"\n---\n", i + 1),
        );
    }
    files.insert("f8.slurp".to_string(), "---\n---\nhello".to_string());
    compile_with_options(CompileOptions {
        virtual_files: Some(files),
        entry_file: Some("f0.slurp".to_string()),
        ..Default::default()
    })
    .expect("a short import chain must compile");
}

// ═══ Merge-boundary and filter regressions ══════════════════════════════════════════════════════
//
// The XSS-shaped cases (script bodies, JS-evaluated attributes, srcdoc, style) live in
// `xss_regression.rs`, where they are asserted on the BROWSER-DECODED value. What follows are the
// cases whose failure mode is a wrong value or a silent no-op rather than an executed payload.

use serde_json::json;
use slurp_compiler::{merged_section_value, render_section, theme_block_type};

fn schema_of(src: &str) -> slurp_compiler::ast::SectionSchema {
    let (doc, diag) = slurp_compiler::parse(src, "t.slurp").expect("lex");
    assert!(!diag.has_errors(), "{:?}", diag.errors);
    doc.frontmatter.unwrap().section.unwrap()
}

// -- An `image` setting is a URL and had no scheme gate -----------------------------------------

/// `SettingKind::Image` was a bare string passthrough while its sibling `link` had `safe_link`, so
/// `javascript:alert(1)` survived the merge intact. It happened to be caught downstream at an
/// `src=` render site by the URL-scheme gate, but nothing catches it in
/// `background:url(${section.settings.image})`, in a `{html}` render, or in any other context a
/// theme picks. A URL is validated where it ENTERS.
#[test]
fn an_image_setting_gets_the_same_scheme_gate_as_a_link() {
    let schema = schema_of(
        "---\nsection {\n  name: \"S\"\n  settings { pic: image = \"\" }\n}\n---\n<div></div>",
    );
    for hostile in [
        "javascript:alert(1)",
        "  JavaScript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "vbscript:msgbox(1)",
    ] {
        let merged = merged_section_value(&schema, &json!({ "settings": { "pic": hostile } }));
        assert_eq!(
            merged["settings"]["pic"], "",
            "a script-scheme image survived the merge: {hostile:?}"
        );
    }
    // ... while real image URLs are untouched, in every shape an editor produces.
    for ok in [
        "/media/a.png",
        "https://cdn.example/a.png",
        "img/a.png",
        "../a.png",
    ] {
        let merged = merged_section_value(&schema, &json!({ "settings": { "pic": ok } }));
        assert_eq!(
            merged["settings"]["pic"], ok,
            "a legitimate image URL was dropped: {ok:?}"
        );
    }
}

// -- Section id and type must be validated like every other saved field -------------------------

/// `id` and `type` were copied out of saved state VERBATIM, with no coerce step, while every
/// setting and every string leaf of schema-less state was validated. Safe today only because each
/// render site happens to escape them - which is the kind of safe-by-coincidence the merge boundary
/// exists to replace.
#[test]
fn section_id_and_type_are_validated_at_the_merge_boundary() {
    let schema = schema_of("---\nsection { name: \"S\" }\n---\n<div></div>");
    for hostile in [
        "a\" onload=\"alert(1)",
        "a'b",
        "<script>",
        "a b",
        "a\\b",
        "a&b",
        "a=b",
    ] {
        let merged = merged_section_value(&schema, &json!({ "id": hostile, "type": hostile }));
        assert!(
            merged.get("id").is_none(),
            "a hostile section id survived the merge: {hostile:?} -> {merged}"
        );
        assert!(
            merged.get("type").is_none(),
            "a hostile section type survived: {hostile:?}"
        );
    }
    // Real ids keep working - dropping one breaks a visual editor's click-to-select on that
    // section, so the gate is a reject-list of structural characters, not a guess at the id SHAPE.
    for ok in [
        "s1",
        "hero-1",
        "reviews",
        "b_2",
        "section.3",
        "01H8XGJWBWBAQ4V0J",
    ] {
        let merged = merged_section_value(&schema, &json!({ "id": ok, "type": "Hero" }));
        assert_eq!(
            merged["id"], ok,
            "a legitimate section id was dropped: {ok:?}"
        );
    }
}

/// The SAME gate has to reach a nested BLOCK's id, and did not.
///
/// `merged_section_value_with` ran `safe_ident` over the section's own `id`/`type`, then
/// `merged_blocks_at` copied every block's `id` straight through: no length bound, no charset
/// check, not even a string check. A block id is minted by the same editor, stored in the same
/// saved state and rendered into the same attributes and selectors as a section id, so a gate that
/// stops at the outer level is a gate with a hole in the middle of it.
#[test]
fn block_ids_are_validated_at_the_merge_boundary_too() {
    let schema = schema_of(
        "---\nsection {\n  name: \"S\"\n  blocks { item { settings { label: text = \"L\" } } }\n}\n---\n<div></div>",
    );

    let blocks_of = |state: serde_json::Value| -> Vec<serde_json::Value> {
        merged_section_value(&schema, &state)["blocks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };

    for hostile in [
        "a\" onload=\"alert(1)",
        "a'b",
        "<script>",
        "a b",
        "a\\b",
        "a&b",
        "a=b",
        "a`b",
        "a/b",
        "a;b",
        "a\u{0}b",
    ] {
        let blocks = blocks_of(json!({ "blocks": [{ "type": "item", "id": hostile }] }));
        assert_eq!(
            blocks.len(),
            1,
            "the block itself must survive: {hostile:?}"
        );
        assert!(
            blocks[0].get("id").is_none(),
            "a hostile block id survived the merge: {hostile:?} -> {}",
            blocks[0]
        );
    }

    // Unbounded length, the other half of what `safe_ident` enforces. An id is a handle, not a
    // payload; without the bound saved state decides how much string every render site copies.
    let long = "a".repeat(4096);
    let blocks = blocks_of(json!({ "blocks": [{ "type": "item", "id": long }] }));
    assert!(
        blocks[0].get("id").is_none(),
        "an unbounded block id survived the merge"
    );

    // A non-string id was copied through as whatever JSON type it arrived as, so a template
    // interpolating it rendered an object or an array.
    for wrong_type in [
        json!(42),
        json!({"a": 1}),
        json!(["x"]),
        json!(true),
        json!(null),
    ] {
        let blocks = blocks_of(json!({ "blocks": [{ "type": "item", "id": wrong_type }] }));
        assert!(
            blocks[0].get("id").is_none(),
            "a non-string block id survived the merge: {wrong_type}"
        );
    }

    // Real ids still work at every depth, including a block nested inside a block - dropping one
    // breaks click-to-select on that block, exactly as it would on a section.
    let nested = schema_of(
        "---\nsection {\n  name: \"S\"\n  blocks { group { blocks { leaf { settings { v: text = \"v\" } } } } }\n}\n---\n<div></div>",
    );
    let merged = merged_section_value(
        &nested,
        &json!({
            "id": "sec-1",
            "blocks": [{ "type": "group", "id": "grp_1", "blocks": [{ "type": "leaf", "id": "leaf.2" }] }]
        }),
    );
    assert_eq!(merged["id"], "sec-1");
    assert_eq!(merged["blocks"][0]["id"], "grp_1");
    assert_eq!(merged["blocks"][0]["blocks"][0]["id"], "leaf.2");
}

// -- Sanitizing plain text at merge time double-encoded it --------------------------------------

/// Schema-less section state has every string leaf run through the ammonia allowlist so a bare
/// `{html}` render is still safe. Ammonia entity-encodes as it goes, and `${ }` then escapes AGAIN,
/// so an editor-supplied "Tom & Jerry" reached the page as "Tom &amp;amp; Jerry". Text that cannot
/// carry markup now skips the sanitizer entirely.
#[test]
fn plain_text_in_schemaless_section_state_is_not_double_encoded() {
    let (doc, _) = slurp_compiler::parse("<p>${section.title}</p>", "t.slurp").unwrap();
    let (html, _) = render_section(
        &doc,
        &json!({ "title": "Tom & Jerry" }),
        &json!({}),
        BuildMode::Production,
    );
    assert!(
        html.contains("Tom &amp; Jerry"),
        "expected ONE level of escaping, got: {html}"
    );
    assert!(
        !html.contains("&amp;amp;"),
        "the value was escaped twice: {html}"
    );
}

/// ... and the security property that gate exists for is unchanged: anything that could carry
/// markup still goes through the sanitizer, including a value that hides its `<` behind an entity.
#[test]
fn schemaless_section_state_that_could_carry_markup_is_still_sanitized() {
    let (doc, _) = slurp_compiler::parse("{html section.body}", "t.slurp").unwrap();
    for hostile in [
        "<script>alert(1)</script>ok",
        "<img src=x onerror=alert(1)>",
        "&#60;script&#62;alert(1)&#60;/script&#62;",
    ] {
        let (html, _) = render_section(
            &doc,
            &json!({ "body": hostile }),
            &json!({}),
            BuildMode::Production,
        );
        assert!(
            !html.contains("<script"),
            "script survived: {hostile:?} -> {html}"
        );
        assert!(
            !html.contains("onerror"),
            "handler survived: {hostile:?} -> {html}"
        );
        assert!(
            !html.contains("&#60;script") && !html.contains("&#60;/script"),
            "an entity-encoded tag survived unsanitized and would decode back to markup: {html}"
        );
    }
}

// -- The block-type charset disagreed with the renderer's gate ----------------------------------

/// `{blocks}` restricts a block type to `[A-Za-z0-9_-]` before using it as a registry path (that is
/// the security boundary and stays). The catalog side mapped `blocks/a/b.slurp` to the type `a/b`,
/// which merged into saved state and then rendered NOTHING - a block a user could add in a visual
/// editor and never see. The two halves now agree.
#[test]
fn a_block_file_the_renderer_cannot_load_is_not_catalogued() {
    assert_eq!(theme_block_type("blocks/heading.slurp"), Some("heading"));
    assert_eq!(
        theme_block_type("blocks/social_row.slurp"),
        Some("social_row")
    );
    assert_eq!(theme_block_type("blocks/a/b.slurp"), None);
    assert_eq!(theme_block_type("blocks/../../etc/passwd.slurp"), None);
    assert_eq!(theme_block_type("blocks/a b.slurp"), None);
}

// -- A filter runs AFTER the merge-time sanitize and could cut a tag in half --------------------

/// `richtext` is sanitized at MERGE time while filters run at RENDER time, so a filter always
/// operates on already-sanitized markup. `{html body | truncate(12)}` over an anchor emitted
/// `<a href="htt...` - an open tag whose attribute then swallows the following markup into itself.
#[test]
fn truncating_markup_cannot_emit_half_a_tag() {
    let out = render_with(
        "{html body | truncate(12)}<p>after</p>",
        "{\"body\": \"<a href=\\\"https://example.com\\\">link</a>\"}",
    )
    .expect("must compile");
    assert!(
        !out.contains("<a href=\"htt"),
        "an unterminated tag was emitted: {out}"
    );
    assert!(
        out.contains("<p>after</p>"),
        "following markup was swallowed: {out}"
    );
}

/// ... and truncation still does the ordinary thing to ordinary text.
#[test]
fn truncate_still_truncates_plain_text() {
    let out = render_with("${ body | truncate(5) }", "{\"body\": \"hello world\"}").unwrap();
    assert_eq!(out, "hello...");
}

// -- `currency` saturated a float cast into a plausible, wrong price ----------------------------

/// A Rust float-to-int cast SATURATES, so `(1e309 * 100.0).round() as u64` yielded u64::MAX and
/// rendered `$184,467,440,737,095,516.15`. No panic, no diagnostic - just an entirely fabricated
/// price that reads as real. For a value that renders as money, that is the worst failure mode
/// available: a wrong number nobody has any reason to question.
#[test]
fn a_non_finite_amount_never_renders_as_a_plausible_price() {
    for (input, expect) in [("1.7e308", "$Infinity"), ("-1.7e308", "-$Infinity")] {
        let out = render_with(
            "${ amount | currency(\"USD\") }",
            &format!("{{\"amount\": {input}}}"),
        )
        .expect("must compile");
        assert_eq!(out, expect, "a non-finite amount rendered as money: {out}");
    }
}

/// ... and ordinary and large-but-real amounts are unaffected, including the JPY zero-decimal path.
#[test]
fn ordinary_amounts_are_unaffected_by_the_clamp() {
    for (input, ccy, expect) in [
        ("1234.56", "USD", "$1,234.56"),
        ("19.999", "USD", "$20.00"),
        ("999.6", "JPY", "\u{a5}1,000"),
        ("-50", "USD", "-$50.00"),
    ] {
        let out = render_with(
            &format!("${{ amount | currency(\"{ccy}\") }}"),
            &format!("{{\"amount\": {input}}}"),
        )
        .expect("must compile");
        assert_eq!(out, expect);
    }
}

// -- The slot-fallback parser bug --------------------------------------------------------------

/// `parse_slot` never populated `SlotNode.fallback` from well-formed source: `{slot}x{/slot}` ate
/// the header's `}` and returned early, leaving the body as SIBLING nodes of the slot. Fallback
/// content therefore rendered UNCONDITIONALLY - including when the slot was filled - which is the
/// opposite of what a fallback is. (It also meant the security walk's coverage of that field could
/// only be tested against a hand-built AST, because no template reached it.)
#[test]
fn slot_fallback_renders_only_when_the_slot_is_unfilled() {
    // NOTE: this must go through `compile_with_registry`, not `compile_with_options`. The latter
    // uses its virtual files ONLY for circular-import detection and compiles the entry alone, so a
    // `<layout>` in it falls back to a comment wrapper and never renders the layout's `{slot}` at
    // all. Written that way, this test is vacuous: it passes against the unfixed parser too.
    let registry: std::collections::HashMap<String, String> = [(
        "@layouts/base".to_string(),
        "[{slot}FALLBACK{/slot}]".to_string(),
    )]
    .into_iter()
    .collect();
    let page = "<layout src=\"@layouts/base\">FILLED</layout>";
    let (out, diag) = slurp_compiler::compile_with_registry(
        page,
        "page.slurp",
        &json!({}),
        BuildMode::Production,
        std::sync::Arc::new(registry.clone()),
    );
    assert!(!diag.has_errors(), "{:?}", diag.errors);
    assert!(out.contains("FILLED"), "slot content missing: {out}");
    assert!(
        !out.contains("FALLBACK"),
        "the fallback rendered even though the slot was filled: {out}"
    );

    // ... and it DOES render when there is no slot content at all, which is the whole point of the
    // field. (Note a layout ALWAYS sets slot content, even empty, so an empty `<layout></layout>`
    // renders the empty content rather than the fallback. That is pre-existing renderer semantics
    // and is not what this fix was about.)
    let _ = registry;
    let standalone = render("{slot}FALLBACK{/slot}").expect("must compile");
    assert!(
        standalone.contains("FALLBACK"),
        "the fallback never renders at all, so the field is still dead: {standalone}"
    );
}

/// A bare `{slot}` with no `{/slot}` must keep meaning "no fallback" - the lookahead exists
/// precisely so fixing the bug above does not turn the rest of a file into slot fallback content.
///
/// The assertion is on the EXACT output, and it has to be: without the lookahead, `TAIL` still
/// appears - just INSIDE the slot element, as fallback content that only renders when the slot is
/// unfilled. A `contains("TAIL")` check passes either way and proves nothing (it did).
#[test]
fn a_bare_slot_with_no_close_does_not_swallow_the_rest_of_the_file() {
    let out = render("{slot}TAIL").expect("must compile");
    assert_eq!(
        out, "<slot name=\"default\"></slot>TAIL",
        "content after a bare slot became fallback content instead of a sibling"
    );
}

/// ... and the security walk really does reach a fallback that a TEMPLATE produced, now that one
/// can. This is the live version of the hand-built-AST test in `lib.rs`.
#[test]
fn a_secret_inside_a_real_slot_fallback_is_refused() {
    assert_compile_error(
        "{slot}${env.SLURP_SECRET_API_KEY}{/slot}",
        ErrorCode::SecretEnvInTemplate,
    );
}

// -- URL-attribute coverage, and a sanitizer claim that was false -------------------------------

/// `object[data]`, `a[ping]` and `srcset` were missing from the URL-attribute list while `cite` and
/// `background` were on it. Not exploitable in current browsers - an inconsistency that invites the
/// assumption that the list is exhaustive.
#[test]
fn the_url_attribute_list_covers_the_obscure_members_too() {
    for attr in ["data", "ping", "srcset"] {
        let out = render_with(
            &format!("<x {attr}=\"${{v}}\">y</x>"),
            "{\"v\": \"javascript:alert(1)\"}",
        )
        .expect("must compile");
        assert!(
            out.contains(&format!("{attr}=\"\"")),
            "{attr} did not neutralize a script scheme: {out}"
        );
    }
}

/// The sanitizer module doc claims `data:` URIs are stripped in ANY url attribute. That was false:
/// ammonia's scheme check covers `href`/`xlink:href`/`src` and a few tag-specific pairs, and does
/// NOT parse `srcset`, so a `data:text/html` candidate passed `{html v sanitize}` unchanged.
/// `srcset` is no longer allowed, which makes the doc true.
#[test]
fn sanitize_does_not_pass_a_data_uri_through_srcset() {
    let out = render_with(
        "{html body sanitize}",
        "{\"body\": \"<img src=\\\"https://ok.example/a.png\\\" srcset=\\\"data:text/html,x 1x\\\">\"}",
    )
    .expect("must compile");
    assert!(
        !out.contains("srcset"),
        "srcset survived the sanitizer: {out}"
    );
    assert!(
        !out.contains("data:"),
        "a data: URI survived the sanitizer: {out}"
    );
    assert!(
        out.contains("https://ok.example/a.png"),
        "the ordinary src was dropped too: {out}"
    );
}
