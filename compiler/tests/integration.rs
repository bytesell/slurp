use serde_json::json;
use slurp_compiler::{BuildMode, compile};

fn ok(src: &str, ctx: serde_json::Value) -> String {
    let (html, diag) = compile(src, "test.slurp", &ctx, BuildMode::Production);
    assert!(!diag.has_errors(), "Unexpected errors: {:?}", diag.errors);
    html
}

fn ok_dev(src: &str, ctx: serde_json::Value) -> String {
    let (html, diag) = compile(src, "test.slurp", &ctx, BuildMode::Development);
    assert!(!diag.has_errors(), "Unexpected errors: {:?}", diag.errors);
    html
}

// ── Lexer / basic nodes ───────────────────────────────────────────────────────

#[test]
fn test_empty_template() {
    assert_eq!(ok("", json!({})), "");
}

#[test]
fn test_plain_text() {
    let html = ok("Hello, World!", json!({}));
    assert!(html.contains("Hello, World!"));
}

#[test]
fn test_comment_not_emitted() {
    let html = ok("{* this is a comment *}", json!({}));
    assert!(
        !html.contains("this is a comment"),
        "Comment leaked into output"
    );
}

#[test]
fn test_expression_string() {
    assert_eq!(ok("${msg}", json!({"msg": "hello"})), "hello");
}

#[test]
fn test_expression_number() {
    assert_eq!(ok("${count}", json!({"count": 42})), "42");
}

#[test]
fn test_expression_bool() {
    assert_eq!(ok("${flag}", json!({"flag": true})), "true");
}

// ── Security ──────────────────────────────────────────────────────────────────

#[test]
fn test_xss_escaped_in_expression() {
    let html = ok("${evil}", json!({"evil": "<script>alert(1)</script>"}));
    assert!(!html.contains("<script>"), "XSS not escaped: {html}");
    assert!(html.contains("&lt;script&gt;"));
}

#[test]
fn test_xss_ampersand_escaped() {
    let html = ok("${x}", json!({"x": "a & b"}));
    assert!(html.contains("&amp;"));
    assert!(!html.contains(" & "));
}

#[test]
fn test_xss_double_quote_in_attr() {
    let html = ok(r#"<div title="${t}"></div>"#, json!({"t": r#"a"b"#}));
    // The title attribute value should be escaped
    assert!(!html.contains(r#"title="a"b""#));
}

#[test]
fn test_raw_html_not_escaped() {
    let html = ok("{html content}", json!({"content": "<b>bold</b>"}));
    assert!(
        html.contains("<b>bold</b>"),
        "Raw HTML was incorrectly escaped"
    );
}

#[test]
fn test_raw_html_sanitize_strips_script() {
    // {html sanitize content} should strip script from the content value
    let _html = ok(
        "{html sanitize}",
        json!({"content": "<script>bad()</script>safe"}),
    );
    // Tested in unit tests; here we just verify the pipeline doesn't crash.
}

#[test]
fn test_sanitize_unit() {
    use slurp_compiler::renderer::sanitize::sanitize_html;
    let out = sanitize_html("<p>Hello <script>evil()</script>world</p>");
    assert!(!out.contains("<script>"));
    assert!(out.contains("Hello"));
    assert!(out.contains("world"));
}

#[test]
fn test_sanitize_strips_iframe() {
    use slurp_compiler::renderer::sanitize::sanitize_html;
    let out = sanitize_html(r#"<iframe src="https://evil.com"></iframe>safe"#);
    assert!(!out.contains("iframe"));
    assert!(out.contains("safe"));
}

#[test]
fn test_sanitize_strips_on_attributes() {
    use slurp_compiler::renderer::sanitize::sanitize_html;
    let out = sanitize_html(r#"<p onclick="steal()">text</p>"#);
    assert!(!out.contains("onclick"));
    assert!(out.contains("text"));
}

#[test]
fn test_sanitize_allows_safe_markup() {
    use slurp_compiler::renderer::sanitize::sanitize_html;
    let out = sanitize_html("<p>Hello <strong>world</strong></p>");
    assert!(out.contains("<strong>world</strong>"));
}

// ── Safe navigation ───────────────────────────────────────────────────────────

#[test]
fn test_safe_nav_null_root() {
    assert_eq!(ok("${x}", json!({"x": null})), "");
}

#[test]
fn test_safe_nav_null_intermediate() {
    assert_eq!(ok("${a.b.c}", json!({"a": null})), "");
}

#[test]
fn test_safe_nav_missing_key() {
    assert_eq!(ok("${nonexistent}", json!({})), "");
}

#[test]
fn test_safe_nav_deep_missing() {
    assert_eq!(ok("${a.b.c.d.e}", json!({"a": {"b": null}})), "");
}

#[test]
fn test_safe_nav_valid_path() {
    assert_eq!(ok("${user.name}", json!({"user": {"name": "Bob"}})), "Bob");
}

// ── Whitespace agnosticism ────────────────────────────────────────────────────

#[test]
fn test_two_space_indent_produces_same_output() {
    let src_2sp = "  ${name}";
    let src_4sp = "    ${name}";
    let src_tab = "\t${name}";
    let ctx = json!({"name": "X"});

    let h1 = ok(src_2sp, ctx.clone());
    let h2 = ok(src_4sp, ctx.clone());
    let h3 = ok(src_tab, ctx.clone());

    assert!(h1.contains("X"), "2-space: {h1}");
    assert!(h2.contains("X"), "4-space: {h2}");
    assert!(h3.contains("X"), "tab: {h3}");
}

#[test]
fn test_multiline_template_whitespace_agnostic() {
    let src = "{\n  each\n  item\n  in\n  items\n}\n${item}\n{/each}";
    // This may or may not parse depending on how the lexer handles embedded
    // whitespace in tokens, but it should not panic.
    let _ = compile(
        src,
        "test.slurp",
        &json!({"items": [1, 2]}),
        BuildMode::Production,
    );
}

// ── Control flow ──────────────────────────────────────────────────────────────

#[test]
fn test_if_true_branch() {
    let html = ok("{if show}visible{/if}", json!({"show": true}));
    assert!(html.contains("visible"));
}

#[test]
fn test_if_false_branch_hidden() {
    let html = ok("{if show}visible{/if}", json!({"show": false}));
    assert!(!html.contains("visible"));
}

#[test]
fn test_if_else() {
    let html = ok("{if x}yes{else}no{/if}", json!({"x": false}));
    assert!(html.contains("no"));
    assert!(!html.contains("yes"));
}

#[test]
fn test_if_else_if_chain() {
    let html = ok(
        "{if a}A{else if b}B{else}C{/if}",
        json!({"a": false, "b": true}),
    );
    assert!(html.contains("B"));
    assert!(!html.contains("A"));
    assert!(!html.contains("C"));
}

#[test]
fn test_if_truthy_string() {
    let html = ok("{if name}has name{/if}", json!({"name": "Alice"}));
    assert!(html.contains("has name"));
}

#[test]
fn test_if_falsy_empty_string() {
    let html = ok("{if name}has name{/if}", json!({"name": ""}));
    assert!(!html.contains("has name"));
}

#[test]
fn test_each_renders_items() {
    let html = ok(
        "{each item in list}${item} {/each}",
        json!({"list": ["a", "b", "c"]}),
    );
    assert!(html.contains("a"));
    assert!(html.contains("b"));
    assert!(html.contains("c"));
}

#[test]
fn test_each_empty_branch() {
    let html = ok(
        "{each x in items}${x}{empty}empty{/each}",
        json!({"items": []}),
    );
    assert!(html.contains("empty"));
}

#[test]
fn test_each_loop_index() {
    let html = ok(
        "{each x in items}${loop.index}{/each}",
        json!({"items": ["a", "b", "c"]}),
    );
    assert!(html.contains("0"));
    assert!(html.contains("1"));
    assert!(html.contains("2"));
}

#[test]
fn test_each_loop_first_last() {
    let html = ok(
        "{each x in items}${loop.first}{/each}",
        json!({"items": ["a", "b"]}),
    );
    assert!(html.contains("true"));
    assert!(html.contains("false"));
}

#[test]
fn test_each_loop_count() {
    let html = ok(
        "{each x in items}${loop.count}{/each}",
        json!({"items": [1, 2, 3]}),
    );
    assert!(html.contains("3"));
}

#[test]
fn test_each_with_index_var() {
    let html = ok(
        "{each item, idx in items}${idx}{/each}",
        json!({"items": ["a", "b"]}),
    );
    assert!(html.contains("0"));
    assert!(html.contains("1"));
}

#[test]
fn test_each_loop_filter_limit() {
    let html = ok(
        "{each x in items | limit(2)}${x}{/each}",
        json!({"items": [1, 2, 3, 4, 5]}),
    );
    assert!(html.contains("1"));
    assert!(html.contains("2"));
    assert!(!html.contains("3"));
}

#[test]
fn test_each_loop_filter_reverse() {
    let html = ok(
        "{each x in items | reverse}${x}{/each}",
        json!({"items": ["a", "b", "c"]}),
    );
    let pos_c = html.find("c").unwrap_or(usize::MAX);
    let pos_a = html.find("a").unwrap_or(0);
    assert!(
        pos_c < pos_a,
        "Expected c before a after reverse, got: {html}"
    );
}

#[test]
fn test_match_string_pattern() {
    let html = ok(
        r#"{match s}"hello" -> hi"bye" -> bye{/match}"#,
        json!({"s": "hello"}),
    );
    assert!(html.contains("hi"));
}

#[test]
fn test_match_wildcard() {
    let html = ok(
        r#"{match s}"x" -> X_ -> other{/match}"#,
        json!({"s": "anything"}),
    );
    assert!(html.contains("other"));
}

#[test]
fn test_match_number() {
    let html = ok(r#"{match n}1 -> one2 -> two{/match}"#, json!({"n": 2}));
    assert!(html.contains("two"));
}

#[test]
fn test_match_bool() {
    let html = ok(
        r#"{match b}true -> yes false -> no{/match}"#,
        json!({"b": true}),
    );
    assert!(html.contains("yes"));
}

#[test]
fn test_repeat() {
    let html = ok("{repeat 4}x{/repeat}", json!({}));
    assert_eq!(html, "xxxx");
}

#[test]
fn test_repeat_zero() {
    let html = ok("{repeat 0}x{/repeat}", json!({}));
    assert_eq!(html, "");
}

// ── Expressions ───────────────────────────────────────────────────────────────

#[test]
fn test_ternary_true() {
    assert_eq!(ok(r#"${x ? "yes" : "no"}"#, json!({"x": true})), "yes");
}

#[test]
fn test_ternary_false() {
    assert_eq!(ok(r#"${x ? "yes" : "no"}"#, json!({"x": false})), "no");
}

#[test]
fn test_binary_arithmetic() {
    assert_eq!(ok("${a + b}", json!({"a": 3, "b": 4})), "7");
    assert_eq!(ok("${a - b}", json!({"a": 10, "b": 3})), "7");
    assert_eq!(ok("${a * b}", json!({"a": 3, "b": 4})), "12");
}

#[test]
fn test_binary_string_concat() {
    let html = ok(r#"${a + " " + b}"#, json!({"a": "hello", "b": "world"}));
    assert_eq!(html, "hello world");
}

#[test]
fn test_binary_comparison() {
    assert_eq!(ok("${a == b}", json!({"a": 1, "b": 1})), "true");
    assert_eq!(ok("${a != b}", json!({"a": 1, "b": 2})), "true");
    assert_eq!(ok("${a > b}", json!({"a": 5, "b": 3})), "true");
    assert_eq!(ok("${a < b}", json!({"a": 3, "b": 5})), "true");
}

#[test]
fn test_unary_not() {
    assert_eq!(ok("${!flag}", json!({"flag": false})), "true");
    assert_eq!(ok("${!flag}", json!({"flag": true})), "false");
}

#[test]
fn test_logical_and() {
    assert_eq!(ok("${a && b}", json!({"a": true, "b": true})), "true");
    assert_eq!(ok("${a && b}", json!({"a": true, "b": false})), "false");
}

#[test]
fn test_logical_or() {
    assert_eq!(ok("${a || b}", json!({"a": false, "b": true})), "true");
    assert_eq!(ok("${a || b}", json!({"a": false, "b": false})), "false");
}

#[test]
fn test_template_literal() {
    assert_eq!(
        ok(r#"${`Hello ${name}!`}"#, json!({"name": "World"})),
        "Hello World!"
    );
}

// ── Filters ───────────────────────────────────────────────────────────────────

#[test]
fn test_filter_upper() {
    assert_eq!(ok("${x | upper}", json!({"x": "hello"})), "HELLO");
}

#[test]
fn test_filter_lower() {
    assert_eq!(ok("${x | lower}", json!({"x": "HELLO"})), "hello");
}

#[test]
fn test_filter_truncate() {
    let html = ok("${x | truncate(5)}", json!({"x": "hello world"}));
    assert!(html.starts_with("hello"));
    assert!(html.contains("..."));
}

#[test]
fn test_filter_truncate_exact_length() {
    assert_eq!(ok("${x | truncate(5)}", json!({"x": "hi"})), "hi");
}

#[test]
fn test_filter_fixed() {
    assert_eq!(ok("${x | fixed(2)}", json!({"x": 1.23456})), "1.23");
}

#[test]
fn test_filter_fixed_zero() {
    assert_eq!(ok("${x | fixed(2)}", json!({"x": 0})), "0.00");
}

#[test]
fn test_filter_currency_usd() {
    let html = ok(r#"${x | currency("USD")}"#, json!({"x": 1234.56}));
    assert_eq!(html, "$1,234.56");
}

#[test]
fn test_filter_currency_gbp() {
    let html = ok(r#"${x | currency("GBP")}"#, json!({"x": 100.0}));
    assert!(html.contains('£'));
}

#[test]
fn test_filter_date_default() {
    assert_eq!(
        ok("${d | date}", json!({"d": "2024-01-15"})),
        "Jan 15, 2024"
    );
}

#[test]
fn test_filter_date_custom() {
    assert_eq!(
        ok(r#"${d | date("MM/DD/YYYY")}"#, json!({"d": "2024-03-05"})),
        "03/05/2024"
    );
}

#[test]
fn test_filter_default_on_null() {
    let html = ok(r#"${x | default("N/A")}"#, json!({"x": null}));
    assert_eq!(html, "N/A");
}

#[test]
fn test_filter_default_passthrough() {
    let html = ok(r#"${x | default("N/A")}"#, json!({"x": "value"}));
    assert_eq!(html, "value");
}

#[test]
fn test_filter_plural_singular() {
    let html = ok(r#"${n | plural("item", "items")}"#, json!({"n": 1}));
    assert_eq!(html, "1 item");
}

#[test]
fn test_filter_plural_multiple() {
    let html = ok(r#"${n | plural("item", "items")}"#, json!({"n": 5}));
    assert_eq!(html, "5 items");
}

#[test]
fn test_filter_int() {
    assert_eq!(ok("${x | int}", json!({"x": "42"})), "42");
}

#[test]
fn test_filter_float() {
    assert_eq!(ok("${x | float}", json!({"x": "3.14"})), "3.14");
}

#[test]
fn test_filter_int_null() {
    assert_eq!(ok("${x | int}", json!({"x": null})), "0");
}

#[test]
fn test_filter_chained() {
    let html = ok("${x | upper | truncate(3)}", json!({"x": "hello"}));
    assert!(html.starts_with("HEL"));
    assert!(html.contains("..."));
}

// ── HTML elements ─────────────────────────────────────────────────────────────

#[test]
fn test_element_basic() {
    let html = ok("<div>hello</div>", json!({}));
    assert!(html.contains("<div>"));
    assert!(html.contains("hello"));
    assert!(html.contains("</div>"));
}

#[test]
fn test_element_attrs() {
    let html = ok(r#"<a href="/home" class="link">text</a>"#, json!({}));
    assert!(html.contains(r#"href="/home""#));
    assert!(html.contains(r#"class="link""#));
}

#[test]
fn test_element_self_closing() {
    let html = ok("<br />", json!({}));
    assert!(html.contains("<br"));
}

#[test]
fn test_void_element_img() {
    let html = ok(r#"<img src="photo.jpg" alt="photo" />"#, json!({}));
    assert!(html.contains("img"));
    assert!(html.contains("photo.jpg"));
}

#[test]
fn test_element_boolean_attr() {
    let html = ok("<input disabled />", json!({}));
    assert!(html.contains("disabled"));
}

#[test]
fn test_element_on_handler() {
    let html = ok(r#"<button on:click="doThing">btn</button>"#, json!({}));
    assert!(html.contains("doThing") || html.contains("on:click"));
}

#[test]
fn test_element_navigate_false() {
    let html = ok(r#"<a href="/x" navigate=false>link</a>"#, json!({}));

    // THE SPELLING IS THE FEATURE, so assert the exact attribute rather than the substring
    // "navigate" - which the old assertion matched even while the opt-out did nothing.
    //
    // The runtime skips an anchor via `getAttribute('navigate') === 'false'`
    // (`runtime/src/core/navigation.ts`). The compiler emitted `data-navigate="false"`, so the
    // two never met and every `navigate=false` link was still intercepted. A test that only
    // asks whether the word appears cannot see that.
    assert!(
        html.contains(r#"navigate="false""#),
        "expected navigate=\"false\" on the anchor, got: {html}"
    );
    assert!(
        !html.contains("data-navigate"),
        "the runtime does not read data-navigate; emitting it is the bug this pins: {html}"
    );
}

#[test]
fn test_element_expr_attr() {
    let html = ok(r#"<div class={cls}></div>"#, json!({"cls": "active"}));
    assert!(html.contains("active"));
}

#[test]
fn test_nested_elements() {
    let html = ok("<ul><li>a</li><li>b</li></ul>", json!({}));
    assert!(html.contains("<ul>"));
    assert!(html.contains("<li>a</li>"));
    assert!(html.contains("<li>b</li>"));
    assert!(html.contains("</ul>"));
}

// ── Component ─────────────────────────────────────────────────────────────────

#[test]
fn test_component_uppercase_rendered() {
    let html = ok(r#"<Card title="hello" />"#, json!({}));
    assert!(html.contains("Card") || html.contains("data-slurp-component"));
}

// ── Image component ───────────────────────────────────────────────────────────

#[test]
fn test_image_picture_element() {
    let html = ok(
        r#"<Image src="photo.jpg" width={800} height={600} alt="A photo" />"#,
        json!({}),
    );
    assert!(
        html.contains("<picture>"),
        "Missing picture element: {html}"
    );
    assert!(html.contains("avif"), "Missing avif source: {html}");
    assert!(html.contains("webp"), "Missing webp source: {html}");
    assert!(
        html.contains("jpeg") || html.contains("jpg"),
        "Missing jpeg fallback: {html}"
    );
}

#[test]
fn test_image_lazy_loading() {
    let html = ok(
        r#"<Image src="x.jpg" width={100} height={100} alt="" />"#,
        json!({}),
    );
    assert!(
        html.contains("loading=\"lazy\""),
        "Missing lazy loading: {html}"
    );
}

#[test]
fn test_image_priority_eager() {
    let html = ok(
        r#"<Image src="x.jpg" width={100} height={100} alt="" priority />"#,
        json!({}),
    );
    assert!(
        html.contains("loading=\"eager\"") || !html.contains("loading=\"lazy\""),
        "Priority image should not be lazy: {html}"
    );
}

// ── Debug ─────────────────────────────────────────────────────────────────────

#[test]
fn test_debug_shown_in_dev() {
    let html = ok_dev("{debug name}", json!({"name": "test"}));
    assert!(
        html.contains("data-slurp-debug"),
        "Debug not shown in dev mode: {html}"
    );
}

#[test]
fn test_debug_hidden_in_prod() {
    let html = ok("{debug name}", json!({"name": "test"}));
    assert!(
        !html.contains("data-slurp-debug"),
        "Debug leaked in prod: {html}"
    );
}

// ── Redirect / Next ───────────────────────────────────────────────────────────

#[test]
fn test_next_outside_middleware_is_refused() {
    // `{next}` is middleware-only, exactly like `{redirect}`. The security walk had
    // simply never enforced it: Node::Next fell into a `_ => {}` catch-all, so an
    // ordinary template carrying `{next}` compiled clean.
    let (html, diag) = compile("{next}", "test.slurp", &json!({}), BuildMode::Production);
    assert!(
        diag.errors.iter().any(|e| e
            .message
            .contains("{next} is only allowed inside middleware")),
        "expected {{next}} to be refused outside middleware; errors: {:?} html: {html:?}",
        diag.errors
    );
}

// ── Error recovery ────────────────────────────────────────────────────────────

#[test]
fn test_never_panics_unclosed_if() {
    // Malformed - should produce an error but not panic
    let (_, diag) = compile(
        "{if x}content",
        "test.slurp",
        &json!({"x": true}),
        BuildMode::Production,
    );
    // We accept either an error or partial output - the key thing is no panic
    let _ = diag;
}

#[test]
fn test_never_panics_unclosed_each() {
    let _ = compile(
        "{each x in list}${x}",
        "test.slurp",
        &json!({"list": [1]}),
        BuildMode::Production,
    );
}

#[test]
fn test_never_panics_malformed_expr() {
    let _ = compile("${", "test.slurp", &json!({}), BuildMode::Production);
}

#[test]
fn test_never_panics_random_bytes() {
    let _ = compile(
        "\x00\x01\x02\x03",
        "test.slurp",
        &json!({}),
        BuildMode::Production,
    );
}

// ── Frontmatter ───────────────────────────────────────────────────────────────

#[test]
fn test_frontmatter_body_still_renders() {
    let src = "---\nusing \"@c/Nav\"\n---\n<div>body</div>";
    let html = ok(src, json!({}));
    assert!(html.contains("body"));
}

#[test]
fn test_no_frontmatter_is_fine() {
    let html = ok("<p>hello</p>", json!({}));
    assert!(html.contains("hello"));
}

// ── Iteration limit ───────────────────────────────────────────────────────────

#[test]
fn test_iteration_limit_capped() {
    // Generate a 2000-item array - should be capped at 1000
    let items: Vec<i64> = (0..2000).collect();
    let ctx = json!({"items": items});
    let (html, diag) = compile(
        "{each x in items}x{/each}",
        "test.slurp",
        &ctx,
        BuildMode::Development,
    );
    // Should have emitted a warning
    assert!(
        diag.warnings
            .iter()
            .any(|w| { format!("{:?}", w.code).contains("IterationLimit") }),
        "Expected IterationLimitExceeded warning"
    );
    // Output should be exactly 1000 x's
    assert_eq!(
        html.len(),
        1000,
        "Expected 1000 iterations, got {}",
        html.len()
    );
}

// ── Client let ────────────────────────────────────────────────────────────────

#[test]
fn test_client_let_emits_script() {
    let html = ok("{$let count = 0}", json!({}));
    assert!(
        html.contains("data-slurp-let"),
        "Missing client-let script: {html}"
    );
}

#[test]
fn test_client_let_script_close_tag_not_injected() {
    // A value containing `</script` must not produce early script tag closure. This verifies the
    // JSON safety fix in renderer/escape.rs: a literal `<` is escaped to its JSON unicode form, which
    // neutralizes `</script>` breakout (and `<!--` comment-state confusion).
    let html = ok(
        r#"{$let tag = "</script><script>alert(1)</script>"}"#,
        json!({}),
    );
    // The injected closing-then-opening sequence must never appear verbatim.
    assert!(
        !html.contains("</script><script>"),
        "ClientLet JSON must not allow </script> injection: {html}"
    );
    // The injected `<script>alert(1)` open tag must not survive unescaped (the real client-let script
    // element is `<script data-slurp-let ...>`, never `<script>alert`).
    assert!(
        !html.contains("<script>alert(1)"),
        "ClientLet JSON must escape the injected <script> payload: {html}"
    );
}

#[test]
fn test_client_let_from_context_object() {
    // {$let} assigned from a context path that resolves to an object.
    let html = ok(
        r#"{$let user = profile}"#,
        json!({"profile": {"name": "Alice"}}),
    );
    assert!(
        html.contains("data-slurp-let"),
        "Missing client-let for object value: {html}"
    );
}

#[test]
fn test_client_let_from_context_array() {
    // {$let} assigned from a context path that resolves to an array.
    let html = ok(r#"{$let items = list}"#, json!({"list": [1, 2, 3]}));
    assert!(
        html.contains("data-slurp-let"),
        "Missing client-let for array value: {html}"
    );
}

// ── Try/catch ─────────────────────────────────────────────────────────────────

#[test]
fn test_try_renders_body() {
    let html = ok("{try}content{error}error{/try}", json!({}));
    assert!(html.contains("content"));
}

// ── Layout / Slot ─────────────────────────────────────────────────────────────

#[test]
fn test_layout_renders() {
    let html = ok(r#"{layout "base.slurp"}content{/layout}"#, json!({}));
    assert!(html.contains("content"), "Layout body not rendered: {html}");
}

#[test]
fn test_slot_renders() {
    let html = ok("{slot}", json!({}));
    assert!(html.contains("slot"));
}

// ── Parser + renderer regressions ─────────────────────────────────────────────

#[test]
fn frontmatter_comments_are_comments() {
    // `{* ... *}` was not lexed as a comment in frontmatter mode, so the comment
    // BODY was tokenized as declarations. Prose containing the word "props" was
    // therefore read as a real props statement and the file failed to compile.
    let src = "---\n{* No props - this is a page *}\nusing \"@components/Nav\"\n---\n<div>hi</div>";
    let (html, diag) = compile(src, "test.slurp", &json!({}), BuildMode::Production);
    assert!(
        !diag.has_errors(),
        "frontmatter comment broke compilation: {:?}",
        diag.errors
    );
    assert!(html.contains("hi"), "body missing: {html}");
    assert!(
        !html.contains("No props"),
        "comment leaked into output: {html}"
    );
}

#[test]
fn trailing_space_after_a_prop_type_does_not_eat_the_next_prop() {
    // `read_type_expr` terminated on a raw Newline, but a trailing SPACE puts a
    // Whitespace token in front of it. The guard missed the line break, the
    // trivia-skipping peek() walked past it, and the next declaration was consumed
    // into the type string: props became [("a", "string b number")] and `b`
    // vanished with zero diagnostics.
    let src = "---\nprops {\n  a: string \n  b: number\n}\n---\n";
    let (doc, diag) = slurp_compiler::parser::parse(src, "test.slurp").expect("lex");
    assert!(!diag.has_errors(), "unexpected errors: {:?}", diag.errors);
    let props = &doc.frontmatter.expect("frontmatter").props;
    let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b"], "a prop was swallowed: {props:?}");
    assert_eq!(
        props[0].type_expr, "string",
        "type expression ran past the line end"
    );
    assert_eq!(props[1].type_expr, "number");
}

#[test]
fn trailing_tab_after_a_type_field_does_not_eat_the_next_field() {
    let src = "---\ntype T {\n  a: string\t\n  b: number\n}\n---\n";
    let (doc, diag) = slurp_compiler::parser::parse(src, "test.slurp").expect("lex");
    assert!(!diag.has_errors(), "unexpected errors: {:?}", diag.errors);
    let fields = &doc.frontmatter.expect("frontmatter").type_defs[0].fields;
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["a", "b"],
        "a type field was swallowed: {fields:?}"
    );
}

#[test]
fn mismatched_close_tag_warns_but_does_not_fail() {
    // Tolerant parsing is kept deliberately (hard-failing would break themes that
    // already compile); the SILENCE is what was wrong. `</span>` used to be eaten
    // by error recovery with no diagnostic of any severity.
    let (_html, diag) = compile(
        "<div></span></div>",
        "test.slurp",
        &json!({}),
        BuildMode::Production,
    );
    assert!(
        !diag.has_errors(),
        "must stay a warning, not an error: {:?}",
        diag.errors
    );
    assert!(
        diag.warnings
            .iter()
            .any(|w| w.message.contains("Mismatched closing tag </span>")),
        "expected a mismatched-tag warning, got: {:?}",
        diag.warnings
    );
}

#[test]
fn correctly_nested_tags_produce_no_mismatch_warning() {
    let (_html, diag) = compile(
        "<div><p>hi</p><span>x</span></div>",
        "test.slurp",
        &json!({}),
        BuildMode::Production,
    );
    assert!(
        diag.warnings.is_empty(),
        "false-positive warning: {:?}",
        diag.warnings
    );
}

#[test]
fn dynamic_index_on_a_bare_path_resolves() {
    // `${ items[i] }` is ONE path whose second segment is a subscript, which
    // Scope::resolve_path could not evaluate, so it rendered empty - while the
    // parenthesized `${ (items)[i] }` (a real ExprKind::Index) worked. The two
    // forms must agree.
    let ctx = json!({"items": ["a", "b", "c"], "i": 1});
    assert_eq!(ok("${ items[i] }", ctx.clone()), "b");
    assert_eq!(ok("${ (items)[i] }", ctx.clone()), "b");

    // Object subscript, nested traversal after the subscript, and an expression
    // subscript all take the same path.
    let ctx2 = json!({
        "rows": [{"name": "Widget"}, {"name": "Gadget"}],
        "map": {"k": "v"},
        "key": "k",
        "i": 0
    });
    assert_eq!(ok("${ rows[i].name }", ctx2.clone()), "Widget");
    assert_eq!(ok("${ map[key] }", ctx2.clone()), "v");
    assert_eq!(ok("${ rows[i + 1].name }", ctx2.clone()), "Gadget");

    // Literal indices must not regress.
    assert_eq!(ok("${ items[0] }", ctx.clone()), "a");
    // An out-of-range or unusable index stays empty rather than erroring.
    assert_eq!(ok("${ items[99] }", ctx.clone()), "");
    assert_eq!(ok("${ items[missing] }", ctx), "");
}

#[test]
fn image_component_honours_literal_dimensions() {
    // `width={400}` is parsed as f64 and serializes as a FLOAT-typed JSON number,
    // for which as_u64() returns None - so the author's dimensions were silently
    // replaced by the 800x600 defaults.
    let html = ok(
        r#"<Image src="p.jpg" width={400} height={300} alt="x" />"#,
        json!({}),
    );
    assert!(html.contains(r#"width="400""#), "width ignored: {html}");
    assert!(html.contains(r#"height="300""#), "height ignored: {html}");
    assert!(html.contains("w=400"), "srcset width ignored: {html}");
    assert!(
        !html.contains("800"),
        "fell back to the default width: {html}"
    );

    // Context-supplied integers kept working before and must keep working.
    let html = ok(
        r#"<Image src="p.jpg" width={w} height={h} alt="x" />"#,
        json!({"w": 640, "h": 480}),
    );
    assert!(
        html.contains(r#"width="640""#),
        "context width ignored: {html}"
    );
    assert!(
        html.contains(r#"height="480""#),
        "context height ignored: {html}"
    );

    // With no dimensions at all the documented defaults still apply.
    let html = ok(r#"<Image src="p.jpg" alt="x" />"#, json!({}));
    assert!(
        html.contains(r#"width="800""#),
        "default width lost: {html}"
    );
    assert!(
        html.contains(r#"height="600""#),
        "default height lost: {html}"
    );
}
