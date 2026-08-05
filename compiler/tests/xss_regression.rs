// XSS regression suite.
//
// `security_tests.rs` already covers the BASELINE escaping contract (entity escaping, ammonia
// sanitize, env/scope gating). This file is narrower and adds the layer that a happy-path assertion
// cannot: EVASIONS against the three defences below.
//
//   1. `escape_attr` - JS-string breakout out of an Alpine attribute (`x-on:click="f('${v}')"`).
//   2. `is_url_attr` + `has_forbidden_url_scheme` - script schemes in href/src/action/....
//   3. `sanitize_attr_spread` + `{html ... sanitize}` - attribute smuggling and tag injection.
//
// Every expectation here was OBSERVED from the renderer first and then pinned, not predicted. A test
// that asserts what the code was hoped to do is worth very little in a security suite.
//
// The point of the file is the negative space: an attacker does not send `javascript:alert(1)`, they
// send `JaVa\tScRiPt:alert(1)` and `\u{a0}javascript:` and `java&#115;cript:`. One assertion per
// defence proves the defence exists; these prove it cannot be walked around.

use slurp_compiler::{CompileError, CompileOptions, compile_template};

fn render_ctx(template: &str, ctx: serde_json::Value) -> String {
    compile_template(
        template,
        CompileOptions {
            context_json: Some(ctx.to_string()),
            ..Default::default()
        },
    )
    .expect("template must compile")
}

fn compile_err(template: &str, ctx: serde_json::Value) -> CompileError {
    compile_template(
        template,
        CompileOptions {
            context_json: Some(ctx.to_string()),
            ..Default::default()
        },
    )
    .expect_err("template must be REFUSED")
}

/// Renders `value` into `template`'s `${v}` / `v` slot.
fn with_value(template: &str, value: &str) -> String {
    render_ctx(template, serde_json::json!({ "v": value }))
}

// ── Reading our output the way a BROWSER does ───────────────────────────────────────────────────
//
// THE POINT OF THIS MODULE. Several findings this file pins are INVISIBLE to a raw-string
// assertion, because the thing that undoes our escaping is the HTML parser itself: it decodes
// character references in an attribute value BEFORE Alpine, the event-handler compiler, the CSS
// parser or an iframe's document parser ever sees the string. Asserting "no raw quote survived
// into the attribute" is therefore TRUE and PROVES NOTHING: it holds just as well when the payload
// is live, so a test built on it reads as coverage while covering nothing.
//
// So: parse the rendered HTML with a real HTML parser and assert on the DECODED value. Ammonia is
// already a dependency and its `attribute_filter` hands back exactly that.
mod browser {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    /// Every tag these tests build markup out of. Ammonia drops unknown tags, which would silently
    /// empty the document and make an assertion pass for the wrong reason.
    fn tags() -> HashSet<&'static str> {
        [
            "a", "p", "div", "span", "button", "img", "iframe", "x", "input", "form", "template",
        ]
        .into_iter()
        .collect()
    }

    /// The value of `attr` on the first element that carries it, AS THE BROWSER DECODED IT.
    /// `None` when the attribute is absent.
    pub fn decoded_attr(html: &str, attr: &str) -> Option<String> {
        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let sink = seen.clone();
        let wanted = attr.to_ascii_lowercase();
        let mut b = ammonia::Builder::new();
        b.tags(tags());
        // Only the attribute under test needs to survive the allowlist; ammonia calls the filter
        // for allowed attributes only.
        let allowed: HashSet<&str> = [attr].into_iter().collect();
        b.generic_attributes(allowed);
        b.attribute_filter(move |_el, name, value| {
            if name.eq_ignore_ascii_case(&wanted) {
                let mut slot = sink.lock().unwrap();
                if slot.is_none() {
                    *slot = Some(value.to_string());
                }
            }
            Some(value.into())
        });
        let _ = b.clean(html).to_string();
        // `clean` has returned, so the filter closure (and its Arc clone) are done with the slot.
        seen.lock().unwrap().clone()
    }

    /// The rendered markup re-serialized after a parse, with `img`/`iframe`/`script`-class tags
    /// kept if they really are tags. Used to prove that DATA never became an ELEMENT.
    pub fn reparse(html: &str) -> String {
        let mut b = ammonia::Builder::new();
        b.tags(tags());
        b.generic_attributes(["alt", "src", "href", "id"].into_iter().collect());
        b.clean(html).to_string()
    }
}

const HREF: &str = r#"<a href="${v}">x</a>"#;

// ── 1. Script schemes in URL attributes ─────────────────────────────────────────────────────────

/// The scheme check resolves the URL the way a browser does before matching, so none of the classic
/// prefix-check evasions survive. A blocked value becomes an EMPTY attribute, never a partial URL.
#[test]
fn script_schemes_are_neutralized_however_they_are_spelled() {
    let evasions = [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",         // case
        "java\tscript:alert(1)",       // embedded tab
        "java\nscript:alert(1)",       // embedded newline
        "java\rscript:alert(1)",       // embedded CR
        "\u{0}javascript:alert(1)",    // leading NUL
        "\u{1}javascript:alert(1)",    // leading C0 control
        "   javascript:alert(1)",      // leading spaces
        "\u{a0}javascript:alert(1)",   // leading NBSP (we are stricter than the URL spec)
        "\u{2028}javascript:alert(1)", // line separator
        "\u{feff}javascript:alert(1)", // BOM / zero-width no-break space
        "data:text/html,<script>alert(1)</script>",
        "DATA:text/html;base64,PHNjcmlwdD4=",
        "vbscript:msgbox(1)",
    ];
    for e in evasions {
        let out = with_value(HREF, e);
        assert_eq!(
            out, r#"<a href="">x</a>"#,
            "scheme evasion survived: {e:?} -> {out}"
        );
    }
}

/// The guard must not be so eager that it eats ordinary links - a false positive here silently
/// breaks navigation on every site using the theme, which is how an over-tightened filter gets
/// reverted.
#[test]
fn ordinary_urls_are_left_alone() {
    for ok in [
        "https://example.com/a?b=c#d",
        "http://example.com",
        "/products/some-thing",
        "../relative/path",
        "#anchor",
        "mailto:hi@example.com",
        "tel:+61400000000",
        // Contains the word but is not the scheme.
        "/blog/how-javascript-works",
        "https://example.com/?next=data:foo",
        "https://example.com/?a=1&b=2",
    ] {
        let out = with_value(HREF, ok);
        // Asserted on the DECODED value: the raw output legitimately differs (`&` -> `&amp;`), so a
        // raw-substring check would have to re-implement the escaper to know what to expect, and
        // would then agree with a broken escaper. What matters is the URL the browser resolves.
        assert_eq!(
            browser::decoded_attr(&out, "href").as_deref(),
            Some(ok),
            "legitimate URL was mangled: {ok:?} -> {out}"
        );
    }
}

/// HTML-entity smuggling is defeated by ESCAPING rather than by the scheme check: the value does not
/// match `javascript:`, but the `&` is attribute-escaped on the way out, so the browser sees the
/// literal text `java&#115;cript:` - not a scheme. Pinned because it looks like a miss and is not:
/// anyone "fixing" the scheme check to decode entities would also have to keep this escaping.
#[test]
fn entity_encoded_scheme_is_defused_by_escaping_not_by_the_scheme_check() {
    let out = with_value(HREF, "java&#115;cript:alert(1)");
    assert!(
        out.contains("java&amp;#115;cript:"),
        "the ampersand must be escaped: {out}"
    );
    // The claim above is about what the BROWSER resolves, so measure that rather than infer it:
    // one decode gives back the literal text, NOT a second round of entity decoding into a scheme.
    assert_eq!(
        browser::decoded_attr(&out, "href").as_deref(),
        Some("java&#115;cript:alert(1)"),
        "the value must decode to literal text, not to a javascript: URL: {out}"
    );
}

/// Every URL-bearing attribute is covered, not just href - `formaction` and `action` in particular
/// are the ones a reviewer forgets.
#[test]
fn every_url_attribute_is_covered() {
    for attr in [
        "href",
        "src",
        "poster",
        "formaction",
        "action",
        "cite",
        "background",
    ] {
        let tpl = format!(r#"<x {attr}="${{v}}">y</x>"#);
        let out = with_value(&tpl, "javascript:alert(1)");
        assert!(
            out.contains(&format!(r#"{attr}="""#)),
            "{attr} did not neutralize a script scheme: {out}"
        );
    }
}

// ── 2. JS-string breakout out of an attribute ───────────────────────────────────────────────────

/// Data interpolated into a JS-EVALUATED attribute must stay a single inert string literal
/// AFTER the browser decodes the attribute value.
///
/// The assertion has to be made on the DECODED value, and that distinction is the whole test.
/// "No raw quote survives into the attribute" is true of entity escaping and implies nothing about
/// safety: `escape_attr` turns `'` into `&#39;`, and the HTML parser turns it straight back into
/// `'` before Alpine compiles the handler, so `<div x-on:click="f('${v}')">` with `');alert(1);('`
/// reaches the JS engine as `f('');alert(1);('')`. Asserting on the ENCODED value therefore pins a
/// property that does not imply the safety it appears to.
///
/// The property that actually matters: every quote that came from DATA is backslash-escaped, so it
/// cannot terminate the literal the theme opened.
#[test]
fn data_cannot_break_out_of_a_js_string_in_an_attribute() {
    for attr in [
        "x-on:click",
        "@click",
        "onclick",
        "x-text",
        "x-init",
        ":title",
    ] {
        for payload in [
            r#"');alert(1);('"#,
            r#"");alert(1);(""#,
            "`+alert(1)+`",
            "${alert(1)}",
            "\\');alert(1);//",
        ] {
            let tpl = format!(r#"<div {attr}="f('${{v}}')">x</div>"#);
            let out = with_value(&tpl, payload);
            let decoded = browser::decoded_attr(&out, attr)
                .unwrap_or_else(|| panic!("attribute {attr} vanished from {out}"));
            // The theme wrote exactly two quotes, around the slot. Anything the DATA contributed
            // must be escaped, so every remaining quote/backtick in the decoded string is either
            // one of those two delimiters or is preceded by a backslash.
            let body = decoded
                .strip_prefix("f('")
                .and_then(|r| r.strip_suffix("')"))
                .unwrap_or_else(|| panic!("handler shape changed: {decoded:?}"));
            for (i, ch) in body.char_indices() {
                if matches!(ch, '\'' | '"' | '`') {
                    assert!(
                        i > 0 && body.as_bytes()[i - 1] == b'\\',
                        "unescaped {ch:?} in the decoded handler: attr={attr} payload={payload:?} decoded={decoded:?}"
                    );
                }
            }
        }
    }
    // The headline case spelled out exactly, so the shape of the fix is legible and a future
    // change to the escaper has to restate what it now produces.
    let out = with_value(r#"<div @click="f('${v}')">x</div>"#, r#"');alert(1);('"#);
    assert_eq!(
        browser::decoded_attr(&out, "@click").as_deref(),
        Some(r"f('\');alert(1);(\'')"),
        // Before the fix this decoded to `f('');alert(1);('')` - a real call.
        "the decoded handler must keep the payload inside one string literal"
    );
}

/// A slot the author already declared as data (`| js`) must not be escaped a SECOND time - double
/// escaping is how an auto-escaper silently corrupts every value it touches, and it would turn
/// every existing `| js` call site into backslash soup.
#[test]
fn an_already_js_filtered_slot_is_not_escaped_twice() {
    let out = with_value(r#"<div @click="f('${ v | js }')">x</div>"#, "a'b");
    let decoded = browser::decoded_attr(&out, "@click").expect("attribute present");
    assert_eq!(
        decoded, r"f('a\'b')",
        "expected exactly one level of escaping"
    );
}

/// The declared opt-out. A JS-evaluated attribute carries two very different kinds of value, and
/// only the author can tell them apart: DATA (escape it) and JavaScript the THEME wrote and passed
/// in as a component prop (escaping it would corrupt working code - a theme may forward a call
/// such as `fmt(stats.spent, ...)` into `x-text`, or a global lookup into `x-data`).
#[test]
fn unsafe_js_is_the_declared_opt_out_for_theme_written_javascript() {
    let code = "$fmt(stats.spent, window.__siteCurrency)";
    // `x-text="${ v }"` is a WHOLE-attribute slot, i.e. statement position, so the neutralizer is
    // the JSON one: the code becomes an inert string literal rather than a call.
    let escaped = with_value(r#"<div x-text="${ v }">x</div>"#, code);
    assert_eq!(
        browser::decoded_attr(&escaped, "x-text").as_deref(),
        Some(format!("\"{code}\"").as_str()),
        "an undeclared slot must be neutralized by default: {escaped}"
    );
    let declared = with_value(r#"<div x-text="${ v | unsafe_js }">x</div>"#, code);
    assert_eq!(
        browser::decoded_attr(&declared, "x-text").unwrap(),
        code,
        "unsafe_js must pass theme-written JavaScript through untouched"
    );
}

// ── 2b. A slot in JS STATEMENT position ─────────────────────────────────────────────────────────
//
// Auto-escaping with `escape_js_string` is a complete answer INSIDE a JS string literal and no
// answer at all outside one: it escapes quotes, backslash, `$`, `<`, `/` and newlines, but `;` and
// `(` have no JS string escape and none is possible. `<div x-init="n = ${v}">` with `1;alert(1)`
// therefore rendered `n = 1;alert(1)` - live code - and was pinned here as an accepted boundary.
//
// The fix reads WHERE the slot lands (`js_slot::slot_quote_contexts`, a lexical property of the
// author's own attribute text) and picks the encoding that actually fits that position: a JSON
// literal, which is SELF-DELIMITING whatever the value contains, so nothing in the data can
// contribute a `;` or a `(` to the surrounding program.

/// The headline payload, dead. It used to execute.
#[test]
fn a_slot_in_statement_position_cannot_contribute_to_the_program() {
    for payload in [
        "1;alert(1)",
        "1);alert(1);f(",
        "0 || alert(1)",
        "1,alert(1)",
        "`+alert(1)+`",
        "${alert(1)}",
    ] {
        let out = with_value(r#"<div x-init="n = ${v}">x</div>"#, payload);
        let decoded = browser::decoded_attr(&out, "x-init").expect("attribute present");
        // The whole payload must sit inside ONE string literal that the DATA did not open or
        // close: exactly one leading `"`, exactly one trailing `"`, and no unescaped `"` between.
        let body = decoded
            .strip_prefix("n = \"")
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or_else(|| panic!("payload escaped statement position: {decoded:?}"));
        for (i, ch) in body.char_indices() {
            assert!(
                ch != '"' || (i > 0 && body.as_bytes()[i - 1] == b'\\'),
                "unescaped quote closed the literal early: payload={payload:?} decoded={decoded:?}"
            );
        }
    }
}

/// A NON-textual value in statement position stays bare, which is what keeps the rule usable.
/// Ordinary themes carry such lines routinely (`x-show="active == ${loop.index}"`,
/// `x-data="...createController({ limit: ${section.settings.per_page} })"`), and none of them is a
/// bug, so refusing them outright is not an option. A number is already self-delimiting.
#[test]
fn a_numeric_slot_in_statement_position_is_left_bare() {
    let out = render_ctx(
        r#"<div x-show="active == ${n}" x-init="f(${b})">x</div>"#,
        serde_json::json!({ "n": 3, "b": true }),
    );
    assert_eq!(
        browser::decoded_attr(&out, "x-show").as_deref(),
        Some("active == 3")
    );
    assert_eq!(
        browser::decoded_attr(&out, "x-init").as_deref(),
        Some("f(true)")
    );
}

/// `| js` is a CLAIM about context - "this slot is inside a quoted JS string literal, so escaping
/// the delimiters contains it". A declared slot is emitted VERBATIM, so a FALSE claim is strictly
/// worse than no claim at all: `x-init="n = ${ v | js }"` turned the auto-escaper off and put
/// nothing in its place. Refusing a provably false declaration is the honest outcome, and it is the
/// same rule a `<script>` body already applies.
#[test]
fn the_js_filter_is_refused_where_there_is_no_string_literal_to_be_inside_of() {
    let src = r#"<div x-init="n = ${ v | js }">x</div>"#;
    // The COMPILE-TIME gate, isolated. This is the half `slurp validate` runs, and it has to
    // refuse without rendering - a template whose only refusal came from the renderer would pass
    // validation and then fail at publish time, which is the failure mode `check_security` exists
    // to prevent.
    let (doc, _) = slurp_compiler::parse(src, "t.slurp").expect("lex");
    let gate_err = slurp_compiler::check_security(&doc, "t.slurp", false)
        .expect_err("the security walk must refuse a false `| js` declaration");
    assert_eq!(
        gate_err.code,
        slurp_compiler::ErrorCode::UnsafeScriptInterpolation
    );

    let err = compile_err(src, serde_json::json!({ "v": "1;alert(1)" }));
    assert_eq!(
        err.code,
        slurp_compiler::ErrorCode::UnsafeScriptInterpolation
    );
    assert!(
        err.message.contains("statement or expression position"),
        "the diagnostic must say WHY the declaration is wrong: {}",
        err.message
    );
    assert!(
        err.message.contains("| json") && err.message.contains("| unsafe_js"),
        "the diagnostic must name the fix: {}",
        err.message
    );
    // ... and the same filter one character to the left, inside quotes, is still perfectly fine.
    let ok = with_value(r#"<div x-init="n = '${ v | js }'">x</div>"#, "1;alert(1)");
    assert_eq!(
        browser::decoded_attr(&ok, "x-init").as_deref(),
        Some(r"n = '1;alert(1)'")
    );
}

/// The context is read from the author's own text, so each slot is judged on its own and a slot
/// after a CLOSED literal is correctly seen as statement position. A naive "is there a quote
/// somewhere to my left" test gets this exactly backwards.
#[test]
fn each_slot_is_judged_by_the_literal_that_actually_encloses_it() {
    let out = render_ctx(
        r#"<div @click="f('${a}', ${b})">x</div>"#,
        serde_json::json!({ "a": "x');alert(1);('", "b": "1;alert(1)" }),
    );
    assert_eq!(
        browser::decoded_attr(&out, "@click").as_deref(),
        // `a` is inside the literal -> JS-string escaped. `b` is not -> a JSON literal.
        Some(r#"f('x\');alert(1);(\'', "1;alert(1)")"#)
    );
}

/// `| json` is the declared opt-out for statement position, mirroring the `<script>` body rule
/// where `| js` means "inside a string literal" and `| json` means "a bare value".
#[test]
fn the_json_filter_is_the_declared_opt_out_for_statement_position() {
    let out = render_ctx(
        r#"<div x-data="${ v | json }">x</div>"#,
        serde_json::json!({ "v": { "a": 1 } }),
    );
    assert_eq!(
        browser::decoded_attr(&out, "x-data").as_deref(),
        Some(r#"{"a":1}"#),
        "an already-encoded value must not be encoded or escaped again"
    );
}

/// Tag injection through an attribute value cannot close the tag and start a new one.
#[test]
fn data_cannot_close_the_tag_it_is_inside() {
    for payload in [
        r#"" onload="alert(1)"#,
        "><script>alert(1)</script>",
        r#"x" onmouseover="alert(1)"#,
    ] {
        let out = with_value(r#"<img alt="${v}">"#, payload);
        // Asserted after a real parse: the payload must still be ONE attribute value on ONE
        // element, not markup. Substring checks on our own output cannot tell those apart - which
        // is the whole reason this file grew a parser.
        assert_eq!(
            browser::decoded_attr(&out, "alt").as_deref(),
            Some(payload),
            "the payload did not stay inside the attribute: {payload:?} -> {out}"
        );
        // ... and the handler names in the payload must not exist as ATTRIBUTES. Asserting on the
        // substring "onload" instead would fail on `alt="&quot; onload=&quot;..."`, which is inert
        // text inside a value - the distinction a parser makes and a substring check cannot.
        for smuggled in ["onload", "onmouseover"] {
            assert!(
                browser::decoded_attr(&out, smuggled).is_none(),
                "the payload became a real attribute: {payload:?} -> {out}"
            );
        }
        // In ATTRIBUTE position the raw check is genuinely conclusive and is kept: a parser cannot
        // start a tag without a raw `<`, so if none reached the value, no element can exist. (Note
        // a re-serialization check would NOT work here - html5ever does not escape `<`/`>` inside
        // attribute values on the way out, so the round-tripped markup contains a literal
        // `alt="><script>..."` that is inert but reads alarming.)
        let inner = out
            .split_once(r#"alt=""#)
            .and_then(|(_, r)| r.split_once('"'))
            .map(|(v, _)| v)
            .unwrap_or_else(|| panic!("could not read alt back out of {out}"));
        assert!(
            !inner.contains('<'),
            "raw < survived into the attribute: {payload:?} -> {out}"
        );
        assert!(
            !inner.contains('>'),
            "raw > survived into the attribute: {payload:?} -> {out}"
        );
    }
}

// ── 3. Text-position escaping ───────────────────────────────────────────────────────────────────

/// Text position: an executable construct must never appear from data, in any casing or spacing that
/// a browser's tag-soup parser would still accept.
#[test]
fn no_data_payload_yields_an_executable_construct_in_text() {
    for payload in [
        "<script>alert(1)</script>",
        "<ScRiPt>alert(1)</ScRiPt>",
        "<script\n>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "<svg/onload=alert(1)>",
        "<iframe src=javascript:alert(1)>",
        "<!--<script>alert(1)</script>-->",
        "</p><script>alert(1)</script><p>",
    ] {
        let out = with_value("<p>${v}</p>", payload);
        // The single property that matters in text position: no `<` from DATA may reach the output
        // unescaped, so the parser can never start a tag. Checking for "onerror=" instead would be
        // wrong - `&lt;img src=x onerror=alert(1)&gt;` contains that substring and is inert text.
        let inner = out
            .split_once("<p>")
            .and_then(|(_, r)| r.split_once("</p>"))
            .map(|(v, _)| v)
            .unwrap_or_else(|| panic!("could not read the paragraph back out of {out}"));
        assert!(
            !inner.contains('<'),
            "raw < reached text output: {payload:?} -> {out}"
        );
        assert!(
            !inner.contains('>'),
            "raw > reached text output: {payload:?} -> {out}"
        );
        // And after a real parse there is still exactly one paragraph and no element the data
        // brought with it. In TEXT position the raw check above is genuinely sufficient (a parser
        // cannot start a tag without a `<`), so this is confirmation, not the load-bearing claim.
        let parsed = browser::reparse(&out);
        assert!(
            !parsed.contains("<script") && !parsed.contains("<img") && !parsed.contains("<iframe"),
            "data became markup after parsing: {payload:?} -> {parsed}"
        );
    }
}

// ── 5. `${ }` inside a <script> BODY ────────────────────────────────────────────────────────────
//
// A `<script>` is a RAW TEXT element. The HTML parser does not decode character references inside
// one, so entity escaping there is completely inert: backtick, `$`, `\`, `;`, `(`, `)`, `/` and
// newline all reach the JS engine untouched, and escaping `<` (which blocks `</script>` breakout)
// was the only protection the text path ever provided. Interpolation stays alive in a script body
// on purpose (`LexMode::RawScript`), so this had to be closed at the language level.

/// Four payloads that reach the JS engine unaltered, each REFUSED at compile time.
#[test]
fn an_unfiltered_slot_in_a_script_body_is_a_compile_error() {
    let cases: [(&str, &str); 4] = [
        // Statement injection - the headline. `var n = 1;alert(1);`
        ("<script>var n = ${v};</script>", "1;alert(1)"),
        // Quote breakout across two slots.
        (
            r#"<script>var a = "${v}", b = "${v}";</script>"#,
            r#"";alert(1);var c=""#,
        ),
        // Backtick breakout out of a template literal.
        ("<script>var t = `x${v}y`;</script>", "`+alert(1)+`"),
        // A newline terminates a `//` comment, so the next line is code again.
        ("<script>// ${v}\nvar ok = 1;</script>", "\nalert(1)"),
    ];
    for (tpl, payload) in cases {
        let err = compile_err(tpl, serde_json::json!({ "v": payload }));
        assert!(
            err.message.contains("<script> body"),
            "expected the script-context diagnostic, got: {}",
            err.message
        );
        assert!(
            err.message.contains("| js"),
            "the error must NAME the fix, or an author cannot act on it: {}",
            err.message
        );
    }
}

/// The rule is enforced STATICALLY, not just on the code path that happens to render.
///
/// Worth its own test because the two layers produce the same diagnostic, so the tests above pass
/// whichever one fires - proven by mutation: disabling the compile-time gate left them green,
/// because the render-time backstop caught the same templates. A branch that is never rendered is
/// the one case only the static walk can reach, and it is the case that matters: a theme ships
/// every branch to production, not just the one a build-time context happened to take.
#[test]
fn the_script_rule_is_enforced_on_branches_that_never_render() {
    for tpl in [
        "{if show}<script>var n = ${v};</script>{/if}",
        "{each item in items}<script>var n = ${item};</script>{/each}",
        "{if show}{else}<script>var n = ${v};</script>{/if}",
    ] {
        let err = compile_err(tpl, serde_json::json!({ "show": false, "items": [] }));
        assert!(
            err.message.contains("<script> body"),
            "an unrendered branch escaped the static gate: {tpl} -> {}",
            err.message
        );
    }
}

/// The half that makes the rule usable: `| js` output is EXEMPT from the entity pass inside a
/// script body.
///
/// Before this there was no working way to interpolate into a script at all -
/// `var a = "${ v | js }"` rendered a literal `&#39;` INSIDE the JS string, because the entity pass
/// ran after the filter and a raw-text element never decodes it back. So rule (a) on its own would
/// have been a rule with no way to comply.
#[test]
fn js_filter_output_is_emitted_verbatim_inside_a_script_body() {
    let out = render_ctx(
        r#"<script>var a = "${ v | js }";</script>"#,
        serde_json::json!({ "v": "a'b\"c" }),
    );
    assert_eq!(out, r#"<script>var a = "a\'b\"c";</script>"#);
    assert!(
        !out.contains("&#39;"),
        "an entity leaked into a raw-text element: {out}"
    );
    assert!(
        !out.contains("&quot;"),
        "an entity leaked into a raw-text element: {out}"
    );
}

/// ... and the exemption must NOT leak into an HTML context, where `| js` output is still ordinary
/// text and must still be entity-escaped exactly as before.
#[test]
fn the_script_exemption_does_not_leak_into_html_contexts() {
    let ctx = serde_json::json!({ "v": "a'b<c" });
    let text = render_ctx("<p>${ v | js }</p>", ctx.clone());
    assert!(
        text.contains("&#39;"),
        "text position must still entity-escape: {text}"
    );
    assert!(
        !text.contains("a\\'b"),
        "the raw backslash form escaped into text: {text}"
    );

    let attr = render_ctx(r#"<div title="${ v | js }">x</div>"#, ctx);
    assert!(
        attr.contains("&#39;"),
        "attribute position must still entity-escape: {attr}"
    );
}

/// A closing script tag can never form, whichever filter carried the value in.
#[test]
fn no_filtered_value_can_close_the_script_element() {
    let payload = "</script><img src=x onerror=alert(1)>";
    for tpl in [
        r#"<script>var a = "${ v | js }";</script>"#,
        r#"<script>var a = ${ v | json };</script>"#,
    ] {
        let out = render_ctx(tpl, serde_json::json!({ "v": payload }));
        // Counted, not split. Splitting on the FIRST `</script>` would happily read a body that
        // ends at the payload's own injected close tag and then find nothing wrong with it - the
        // test would pass precisely when the escaping had failed.
        assert_eq!(
            out.matches("</script").count(),
            1,
            "the script element was closed more than once: {out}"
        );
        let body = out
            .trim_start_matches("<script>")
            .trim_end_matches("</script>");
        assert!(
            !body.contains('<'),
            "a raw < survived into a script body: {out}"
        );
    }
}

/// `| json` embeds an object or array as a bare JS value, which is the shape `{html data}` was
/// being used for. It stays valid JSON (`\u003c` is a legal JSON escape) so `JSON.parse` still
/// works, and it cannot close the element.
#[test]
fn json_filter_embeds_a_structure_without_closing_the_script() {
    let out = render_ctx(
        "<script>var d = ${ v | json };</script>",
        serde_json::json!({ "v": { "name": "</script>", "n": 42 } }),
    );
    assert!(
        out.contains("\\u003c/script>"),
        "the tag was not neutralized: {out}"
    );
    let body = out
        .trim_start_matches("<script>var d = ")
        .trim_end_matches(";</script>");
    let round_tripped: serde_json::Value =
        serde_json::from_str(body).expect("the json filter must emit valid JSON");
    assert_eq!(round_tripped["name"], "</script>");
    assert_eq!(round_tripped["n"], 42);
}

/// A WELCOME SIDE EFFECT, pinned so it is a decision rather than a surprise.
///
/// Slurp reads `${` as its own interpolation inside a script body, so an ordinary JavaScript
/// template literal was being EATEN silently: `let t = ` + backtick + `${hours}h ${minutes}m` +
/// backtick compiled to a backtick-`h m`-backtick with no error and no diagnostic. Under the rule
/// above that becomes a loud compile error, which is the correct outcome - the author is told to
/// build the string by concatenation instead of shipping a page whose clock renders as "h m".
#[test]
fn a_javascript_template_literal_in_a_script_body_is_now_a_loud_compile_error() {
    let err = compile_err(
        "<script>let t = `${hours}h ${minutes}m`;</script>",
        serde_json::json!({}),
    );
    assert!(
        err.message.contains("template literal"),
        "the diagnostic must explain the template-literal case, not just demand a filter: {}",
        err.message
    );
    // Concatenation is the migration, and it compiles.
    let ok = render_ctx(
        "<script>let t = hours + 'h ' + minutes + 'm';</script>",
        serde_json::json!({}),
    );
    assert!(ok.contains("hours + 'h '"), "{ok}");
}

// ── 6. srcdoc round-trips into a live document ──────────────────────────────────────────────────

/// `srcdoc` is a DOUBLE-DECODE context: entity escaping is correct for an attribute value and is
/// then completely undone, because the parser decodes the value and hands the result to the iframe
/// as its document - same-origin by default. Proven: `<iframe srcdoc="${v}">` with
/// `<script>alert(1)</script>` rendered entity-escaped and decoded back into live markup.
#[test]
fn srcdoc_cannot_round_trip_markup_back_into_a_live_document() {
    for payload in [
        "<script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "<iframe src=javascript:alert(1)></iframe>",
        "<a href=\"javascript:alert(1)\">x</a>",
    ] {
        let out = with_value(r#"<iframe srcdoc="${v}"></iframe>"#, payload);
        // An empty result drops the attribute entirely (an iframe with no document at all), which
        // is the strongest outcome, not a missing assertion.
        let decoded = browser::decoded_attr(&out, "srcdoc").unwrap_or_default();
        // The decoded value IS the iframe's document, so assert on it as markup.
        assert!(
            !decoded.contains("<script"),
            "srcdoc carried a script element: {decoded}"
        );
        assert!(
            !decoded.contains("onerror"),
            "srcdoc carried an event handler: {decoded}"
        );
        assert!(
            !decoded.contains("<iframe"),
            "srcdoc carried a nested frame: {decoded}"
        );
        assert!(
            !decoded.contains("javascript:"),
            "srcdoc carried a script URL: {decoded}"
        );
    }
}

/// ... while srcdoc keeps working for the one thing it exists for. Refusing it outright, or
/// double-escaping it into visible `&amp;lt;` garbage, would have been the easy fixes and would
/// have broken the feature instead of the attack.
#[test]
fn srcdoc_still_carries_ordinary_markup() {
    let out = with_value(
        r#"<iframe srcdoc="${v}"></iframe>"#,
        "<p>Hello <strong>world</strong></p>",
    );
    let decoded = browser::decoded_attr(&out, "srcdoc").expect("srcdoc present");
    assert!(
        decoded.contains("<strong>world</strong>"),
        "legitimate markup was destroyed: {decoded}"
    );
}

// ── 7. CSS in a style attribute ─────────────────────────────────────────────────────────────────

/// `escape_attr` does not escape `(`, `)`, `;` or `:` - correct for HTML, useless for CSS. Proven:
/// `<div style="background:url(${v})">` with `a);color:red;x:url(b` produced
/// `<div style="background:url(a);color:red;x:url(b)">`, handing the attacker arbitrary CSS
/// declarations on that element: off-origin `url()` beacons that leak IP and referrer,
/// `position:fixed` clickjacking overlays, or simply hiding the page's content.
#[test]
fn a_style_attribute_value_cannot_add_its_own_declarations() {
    for payload in [
        "a);color:red;x:url(b",
        "red;position:fixed;top:0;left:0;width:100vw;height:100vh;z-index:99999",
        "red;background:url(https://evil.example/beacon)",
        "red}html{display:none",
    ] {
        let out = with_value(r#"<div style="background:url(${v})">x</div>"#, payload);
        let decoded = browser::decoded_attr(&out, "style").expect("style present");
        let inside = decoded
            .strip_prefix("background:url(")
            .and_then(|r| r.strip_suffix(')'))
            .unwrap_or_else(|| panic!("the declaration shape changed: {decoded:?}"));
        for ch in [';', '(', ')', '{', '}'] {
            assert!(
                !inside.contains(ch),
                "CSS-structural {ch:?} survived into a style value: payload={payload:?} decoded={decoded:?}"
            );
        }
    }
}

/// The CSS neutralizer applies to DATA SLOTS ONLY, never to the author's own text.
///
/// Running the neutralizer over the WHOLE attribute value instead is the tempting simplification,
/// and it corrupts ordinary CSS: `style="display: flex;"` comes out as `style="display: flex"`,
/// and `style="animation-duration: 60s; will-change: transform"` loses its semicolon and therefore
/// its second declaration. Both are shapes any theme writes. A security fix that quietly rewrites
/// every static style attribute in every theme would be reverted, correctly.
#[test]
fn literal_css_written_by_the_theme_is_never_touched() {
    for literal in [
        r#"<div style="display: flex;">x</div>"#,
        r#"<div style="animation-duration: 60s; will-change: transform">x</div>"#,
        r#"<div style="background:url(/img/a.png)">x</div>"#,
    ] {
        let out = render_ctx(literal, serde_json::json!({}));
        assert_eq!(out, literal, "author CSS was rewritten");
    }
    // The author's own structure around a slot survives too - only the slot is neutralized.
    let out = with_value(r#"<div style="width:${v}px; color:red">x</div>"#, "50");
    assert_eq!(
        browser::decoded_attr(&out, "style").as_deref(),
        Some("width:50px; color:red")
    );
}

/// A `<style>` ELEMENT is different and must stay that way: the lexer reads it as pure raw text
/// with NO interpolation, so there is no value slot to attack. Pinned so a future "let's support
/// `${}` in `<style>` too" change has to face this test first.
#[test]
fn style_elements_are_raw_text_and_are_not_interpolated() {
    let out = with_value("<style>.a{color:${v}}</style>", "red;} body{display:none");
    assert!(
        out.contains("color:${v}"),
        "a <style> element must not interpolate; it did: {out}"
    );
}

// ── 4. Documented boundary, so nobody assumes coverage that does not exist ──────────────────────

/// A pure Alpine BINDING (`:href="expr"`) is evaluated in the BROWSER, not by us. The expression is
/// passed through untouched, so a `javascript:` value arriving in client state at runtime is NOT
/// covered by the server-side scheme check - there is nothing for the compiler to inspect.
///
/// This is pinned deliberately as a documented boundary rather than left implicit. `is_url_attr`
/// understands `:href` and `x-bind:href`, which makes it look as though bindings are covered; they
/// are only covered when the value is a SERVER-rendered string. If a runtime guard is ever added,
/// this test should change and say so.
#[test]
fn alpine_bindings_are_evaluated_client_side_and_are_not_scheme_checked_here() {
    let out = with_value(r#"<a :href="v">x</a>"#, "javascript:alert(1)");
    assert!(
        out.contains(r#":href="v""#),
        "the binding expression should pass through verbatim: {out}"
    );
    assert!(
        !out.contains("javascript:"),
        "the compiler must not inline the client value into the attribute: {out}"
    );
}

// ── The attribute-spread path, which the fuzzer could not reach ───────────────────────────────
//
// `{html attrs}` in TAG position renders untrusted data straight into attribute markup, so it is
// the single widest data-to-markup seam in the renderer. It nonetheless survived a 420,000-case
// fuzzing campaign untouched: no seed contained the construct and no alphabet entry could build
// it, so `sanitize_attr_spread` and `is_safe_attr_name` were never once executed with a fuzzed
// value. A CRITICAL panic lived there the whole time.
//
// These are the end-to-end half of that gap (the unit half is in `renderer/mod.rs`). The fuzz
// alphabet and corpus now carry the construct too, so this class is reachable by generation
// rather than only by the cases someone thought to write down.

/// REGRESSION (panic): a name beginning with a multi-byte character.
///
/// `is_safe_attr_name` tested for the `on` prefix with `&bind_target[..2]`, a BYTE slice, and
/// panicked on any name whose first character was not one byte wide. The value comes from the
/// context, so this was reachable from a single hostile record.
#[test]
fn an_attribute_spread_name_can_start_with_a_multibyte_character() {
    for hostile in [
        "\u{20ac}=1",
        "\u{4f60}\u{597d}=1",
        "\u{1f600}nclick=\"alert(1)\"",
        "\u{feff}onclick=\"alert(1)\"",
        "\u{a0}onerror=\"alert(1)\"",
        "\u{202e}onclick=\"alert(1)\"",
    ] {
        let html = render_ctx(
            "<div {html attrs}>hi</div>",
            serde_json::json!({ "attrs": hostile }),
        );
        assert_eq!(
            html, "<div>hi</div>",
            "a hostile spread name was not dropped: {hostile:?} -> {html}"
        );
    }
}

/// The gate is on the NAME, so every alias that reaches a native handler has to be covered, in
/// every casing - the browser lowercases before Alpine reads the attribute.
#[test]
fn an_attribute_spread_cannot_smuggle_a_native_event_handler() {
    for hostile in [
        r#"onclick="alert(1)""#,
        r#"ONCLICK="alert(1)""#,
        r#"OnClIcK="alert(1)""#,
        r#"onerror="alert(1)""#,
        r#":onclick="alert(1)""#,
        r#":ONCLICK="alert(1)""#,
        r#"x-bind:onclick="alert(1)""#,
        r#"X-BIND:ONCLICK="alert(1)""#,
        r#"x-html="payload""#,
        r#"x-data="{}""#,
        r#"x-init="fetch(x)""#,
        r#"x-effect="run()""#,
    ] {
        let html = render_ctx(
            "<div {html attrs}>hi</div>",
            serde_json::json!({ "attrs": hostile }),
        );
        assert_eq!(
            html, "<div>hi</div>",
            "a handler survived the spread: {hostile:?} -> {html}"
        );
    }
}

/// A name is a NAME: it cannot carry the punctuation that would end it and start something else.
/// Each of these would otherwise let one rendered value contribute a second attribute, or close
/// the tag and open an element.
#[test]
fn an_attribute_spread_name_cannot_break_out_of_the_tag() {
    for hostile in [
        r#"a"onclick="alert(1)"#,
        r#"a'onclick='alert(1)"#,
        "a>x<script>alert(1)</script>",
        "a/onclick=alert(1)",
        "a\0onclick=x",
        "a\nonclick=x",
        "a\tonclick=x",
    ] {
        let html = render_ctx(
            "<div {html attrs}>hi</div>",
            serde_json::json!({ "attrs": hostile }),
        );
        let low = html.to_ascii_lowercase();
        assert!(
            !low.contains("onclick=\"") && !low.contains("<script"),
            "a spread name broke out of the tag: {hostile:?} -> {html}"
        );
    }
}

/// Dropping one attribute does not take the whole fragment with it. The parse is per-attribute
/// on purpose: a fragment is a LIST, and one rejected entry must not silently delete the good
/// ones beside it - a failure mode that looks like the theme, not like a defence.
#[test]
fn one_bad_attribute_does_not_discard_the_rest_of_the_spread() {
    let html = render_ctx(
        "<div {html attrs}>hi</div>",
        serde_json::json!({ "attrs": r#"data-ok="1" onclick="alert(1)" aria-label="fine""# }),
    );
    assert!(html.contains(r#"data-ok="1""#), "got: {html}");
    assert!(html.contains(r#"aria-label="fine""#), "got: {html}");
    assert!(!html.contains("onclick"), "the handler survived: {html}");
}

/// A very long attribute NAME is emitted, not refused, and that is the right answer - recorded
/// here because it looks like an oversight and is not.
///
/// There is no length cap because there is nothing to cap: the name came from a data value that
/// was already that long, and the emitted form is the same size, so a long name is not an
/// amplifier. What matters is that length changes nothing about the two properties that do the
/// work - the charset gate still holds, so a long name cannot contain structural punctuation,
/// and the attributes beside it are unaffected. Adding a cap would only invent a new way for a
/// legitimate `data-` attribute to disappear.
#[test]
fn a_very_long_attribute_name_is_bounded_by_its_charset_not_by_a_length_cap() {
    let long_name = "a".repeat(100_000);
    let html = render_ctx(
        "<div {html attrs}>hi</div>",
        serde_json::json!({ "attrs": format!(r#"data-ok="1" {long_name}="x""#) }),
    );
    assert!(
        html.contains(r#"data-ok="1""#),
        "neighbour dropped: got a truncated render"
    );
    assert!(
        html.contains(&long_name),
        "the long name was expected to survive"
    );
    // The output is the same order of magnitude as the input: no amplification.
    assert!(
        html.len() < 200_000,
        "the spread amplified its input: {} bytes out",
        html.len()
    );

    // A long name made of structural characters is still refused, which is the property that
    // actually matters.
    let hostile = format!("{}\"onclick=\"alert(1)", "a".repeat(50_000));
    let html = render_ctx(
        "<div {html attrs}>hi</div>",
        serde_json::json!({ "attrs": hostile }),
    );
    assert!(
        !html.to_ascii_lowercase().contains("onclick=\""),
        "a long hostile name broke out"
    );
}

/// The Alpine directives the spread EXISTS to forward still get through. A gate that refused
/// these would be safe and useless, and the theme's Button/Input/Select components pass exactly
/// this shape through an `attrs` prop.
#[test]
fn the_attribute_spread_still_forwards_the_alpine_directives_it_is_for() {
    let html = render_ctx(
        "<button {html attrs}>go</button>",
        serde_json::json!({
            "attrs": r#"@click="submit()" x-model="email" :disabled="loading" x-on:keydown.enter="go()" data-id="5" disabled"#
        }),
    );
    for expected in [
        r#"@click="submit()""#,
        r#"x-model="email""#,
        r#":disabled="loading""#,
        r#"x-on:keydown.enter="go()""#,
        r#"data-id="5""#,
        "disabled",
    ] {
        assert!(
            html.contains(expected),
            "a legitimate directive was dropped: {expected} -> {html}"
        );
    }
}

/// A script-scheme URL in a spread value is a VALUE-level rejection, and it has to survive the
/// same spelling evasions the bare `href={...}` path does.
#[test]
fn an_attribute_spread_value_cannot_carry_a_script_scheme() {
    for hostile in [
        r#"href="javascript:alert(1)""#,
        r#"href="JaVaScRiPt:alert(1)""#,
        "href=\"java\tscript:alert(1)\"",
        "href=\"\u{feff}javascript:alert(1)\"",
        "href=\"\u{a0}javascript:alert(1)\"",
        r#"href="data:text/html,<script>alert(1)</script>""#,
        r#"href="vbscript:msgbox(1)""#,
    ] {
        let html = render_ctx(
            "<a {html attrs}>x</a>",
            serde_json::json!({ "attrs": hostile }),
        );
        let low = html.to_ascii_lowercase();
        assert!(
            !low.contains("javascript:") && !low.contains("vbscript:") && !low.contains("data:"),
            "a script scheme survived the spread: {hostile:?} -> {html}"
        );
    }
}

/// A BOUND url attribute is scheme-checked like a plain one.
///
/// `:href` and `x-bind:href` were already on `is_url_attr`'s list, so this looked covered. It was
/// not. Their value is the JS source Alpine evaluates rather than a URL, and a slot outside a JS
/// string is encoded as a JSON string literal on the way in, so the hostile value reached
/// `has_forbidden_url_scheme` already wrapped in quotes. That function tests the LEADING
/// character for a scheme, found `"`, and reported none. The rendered attribute was
/// `:href='"javascript:alert(1)"'`, which Alpine evaluates back to the bare string and assigns.
///
/// The plain `href` and `href={ expr }` forms were both neutralised correctly throughout, which
/// is precisely what made this survive: the two forms anyone would test were the two that worked.
#[test]
fn a_script_scheme_cannot_ride_in_through_a_bound_url_attribute() {
    for attr in [":href", "x-bind:href", ":src", "x-bind:src", ":action"] {
        for hostile in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "java\tscript:alert(1)",
            "\u{feff}javascript:alert(1)",
            "\u{a0}javascript:alert(1)",
            "vbscript:msgbox(1)",
        ] {
            let html = render_ctx(
                &format!(r#"<a {attr}="${{ link }}">x</a>"#),
                serde_json::json!({ "link": hostile }),
            );
            let low = html.to_ascii_lowercase();
            assert!(
                !low.contains("javascript:") && !low.contains("vbscript:"),
                "a script scheme survived a bound url attribute: {attr} = {hostile:?} -> {html}"
            );
        }
    }
}

/// The other half of the same fix, and the half that makes it a fix rather than a mute button.
///
/// Neutralising every bound URL attribute would pass the test above and break every real theme,
/// so what has to hold is that only an actual script scheme is removed. A bound attribute whose
/// value is not a whole string literal is deliberately left alone: its value is not knowable
/// here, and guessing at a concatenation or an identifier would be reading the wrong thing.
#[test]
fn an_ordinary_bound_url_attribute_is_left_alone() {
    let html = render_ctx(
        r#"<a :href="${ good }">a</a><a :href="'/p/' + slug">b</a><a :href="safe">c</a>"#,
        serde_json::json!({ "good": "/products/1", "slug": "abc", "safe": "/ok" }),
    );
    // Pinned as OBSERVED, quotes included. On this path `escape_attr` entity-escapes the JS
    // string delimiters, so the concatenation reads `&#39;/p/&#39; + slug` in the output rather
    // than the source spelling. Asserting the source spelling is predicting, and it fails.
    assert!(
        html.contains(r#":href="&quot;/products/1&quot;""#),
        "a benign bound URL was destroyed: {html}"
    );
    assert!(
        html.contains(r#":href="&#39;/p/&#39; + slug""#),
        "a concatenation binding was destroyed: {html}"
    );
    assert!(
        html.contains(r#":href="safe""#),
        "an identifier binding was destroyed: {html}"
    );
}
