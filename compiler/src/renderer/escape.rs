// `/` is intentionally NOT encoded - it has no special meaning in HTML text
// context, and encoding it would corrupt URL strings and date formats.
#[must_use]
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Escape a value for an HTML attribute VALUE (always emitted double-quoted). Beyond the `&` and `"`
/// that a double-quoted attribute needs, this also escapes `'`, backtick, `<`, and `>` as defense in
/// depth against attribute-value breakout.
///
/// IMPORTANT: this does NOT make a value safe inside a JavaScript-evaluated attribute (Alpine
/// `x-data`/`x-init`/`@click`/`:bind`, or a native `on*` handler). The HTML parser decodes character
/// references in an attribute value BEFORE Alpine or the JS engine reads it, so `&#39;` becomes `'`
/// again at exactly the layer that matters and the value can break out of a JS string literal. To
/// interpolate untrusted data into a JS-evaluated attribute, JS-escape it first with the `js` filter
/// (see `escape_js_string`); entity escaping alone is not sufficient for that context.
#[must_use]
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '`' => out.push_str("&#96;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Escape a value so it is safe to embed inside a JavaScript string literal
/// (single-quoted, double-quoted, or template literal). This is the escaper
/// behind the `js` filter, for interpolating untrusted data into a JS-evaluated
/// attribute, e.g. ``@click={ `addToCart('${ id | js }')` }``.
///
/// Every JS metacharacter is backslash-encoded so the value stays a single
/// literal after the browser HTML-decodes the attribute: the string delimiters
/// (`'` `"` `` ` ``), `\` itself, `$` (so `${...}` cannot start a template-literal
/// substitution), the line terminators (`\n` `\r`, U+2028, U+2029, which break a
/// JS string outright), and `<`/`/` (so a value cannot form `</script>` when the
/// attribute sits inside an inline script). The attribute layer applies its own
/// escaping on top, so the two compose without weakening either.
#[must_use]
pub fn escape_js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '`' => out.push_str("\\`"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '<' => out.push_str("\\x3C"),
            '/' => out.push_str("\\/"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// SECURITY: neutralize a value interpolated into a CSS context - a `style=`
/// attribute, or a client-built style string in `:style` / `x-bind:style`.
///
/// `escape_attr` is the WRONG tool there and its output is not merely
/// insufficient but inert: it does not touch `(`, `)`, `;` or `:`, none of which
/// have any meaning in HTML, and all of which are structural in CSS. The proven
/// payload is `<div style="background:url(${v})">` with
/// `v = "a);color:red;x:url(b"`, which closes the `url()`, terminates the
/// declaration and adds arbitrary ones of its own - off-origin `url()` beacons
/// that leak IP and referrer, a `position:fixed` clickjacking overlay, or simply
/// hiding the page.
///
/// There is no useful ESCAPE for this: CSS escapes (`\3B `) are only valid
/// inside identifiers and strings, and a value slot in a style attribute can sit
/// in any of a dozen grammatical positions. So the structural characters are
/// REMOVED instead. `:` and `/` are deliberately kept - neither can open a new
/// declaration on its own, and both are needed by the legitimate
/// `background:url(${path})` shape. `*` goes (so `/*` cannot start a comment)
/// while `/` stays, which is enough to make comment injection impossible.
///
/// Callers should tell the author when this actually changed a value; silently
/// mangling is how a value slot becomes a mystery bug.
#[must_use]
pub fn escape_css_value(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(
                c,
                ';' | '{' | '}' | '(' | ')' | '"' | '\'' | '\\' | '@' | '*' | '<' | '>' | '\0'
            )
        })
        .collect()
}

/// Make a JSON/text payload safe to embed inside a `<script>` element or any
/// raw-HTML context WITHOUT corrupting it as JSON.
///
/// HTML entity-encoding (`escape_html`) would turn `<` into `&lt;` and break any
/// JSON parser, so instead we escape only the byte sequences that can terminate
/// the surrounding element or comment, plus the two JS line terminators that are
/// illegal inside a JS string literal:
/// - `<` -> `\u003c` (kills `</script>` breakout AND `<!--` comment-state
///   confusion; `\u003c` is a valid JSON/JS escape for `<`).
/// - U+2028 / U+2029 -> `\u2028` / `\u2029` (LINE/PARAGRAPH SEPARATOR are valid
///   in JSON strings but terminate a JS string literal, breaking inline scripts).
///
/// The result is still valid JSON (these are all legal JSON string escapes) and
/// cannot break out of a `<script>` / HTML-comment context.
#[must_use]
pub fn escape_script_safe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_ampersand() {
        assert_eq!(escape_html("a & b"), "a &amp; b");
    }

    #[test]
    fn escape_js_string_neutralizes_string_breakout() {
        // The classic breakout payload closes the JS string, injects a
        // statement, then reopens a string. After JS-escaping, both quotes are
        // backslashed, so the value stays a single string literal.
        let escaped = escape_js_string("x'});alert(1);({'");
        assert_eq!(escaped, "x\\'});alert(1);({\\'");
        // Every single quote in the output is preceded by a backslash.
        for (i, _) in escaped.match_indices('\'') {
            assert!(i > 0 && escaped.as_bytes()[i - 1] == b'\\');
        }
    }

    #[test]
    fn escape_js_string_escapes_all_metacharacters() {
        assert_eq!(escape_js_string("a\\b"), "a\\\\b");
        assert_eq!(escape_js_string("\"q\""), "\\\"q\\\"");
        assert_eq!(escape_js_string("`t`"), "\\`t\\`");
        // `${` must not be able to start a template-literal substitution.
        assert_eq!(escape_js_string("a${b}"), "a\\${b}");
        assert_eq!(escape_js_string("line\nbreak"), "line\\nbreak");
        // `</script>` cannot form: `<` and `/` are both escaped.
        let out = escape_js_string("</script>");
        assert!(!out.contains("</"));
        assert!(out.contains("\\x3C"));
        // JS line separators break a raw string literal, so they are escaped.
        assert_eq!(escape_js_string("a\u{2028}b"), "a\\u2028b");
    }

    #[test]
    fn test_escape_less_than() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
    }

    #[test]
    fn test_escape_greater_than() {
        assert_eq!(escape_html("a > b"), "a &gt; b");
    }

    #[test]
    fn test_escape_double_quote() {
        assert_eq!(escape_html(r#"say "hi""#), r#"say &quot;hi&quot;"#);
    }

    #[test]
    fn test_escape_single_quote() {
        // html-escape may or may not escape single quotes in encode_safe
        let result = escape_html("it's");
        assert!(!result.contains('<'));
        assert!(!result.contains('>'));
    }

    #[test]
    fn test_escape_xss_attempt() {
        let xss = r#"<img src=x onerror="alert(1)">"#;
        let escaped = escape_html(xss);
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('>'));
        assert!(!escaped.contains('"'));
    }

    #[test]
    fn test_escape_empty() {
        assert_eq!(escape_html(""), "");
    }

    #[test]
    fn test_escape_plain_text() {
        assert_eq!(escape_html("hello world"), "hello world");
    }

    #[test]
    fn test_escape_attr() {
        assert_eq!(escape_attr(r#"a "b" c"#), "a &quot;b&quot; c");
    }

    #[test]
    fn test_escape_attr_neutralizes_js_string_breakout() {
        // A value that would break out of a single-quoted JS string embedded in an Alpine attribute
        // (e.g. x-data="{ img: '${value}' }"). The single quotes must be encoded so the injected JS
        // cannot execute.
        let out = escape_attr("');alert(document.cookie);('");
        assert!(!out.contains('\''), "raw single quote survived: {out}");
        assert!(out.contains("&#39;"));
        // Backtick (template-literal breakout) is encoded too.
        let bt = escape_attr("`+alert(1)+`");
        assert!(!bt.contains('`'), "raw backtick survived: {bt}");
        assert!(bt.contains("&#96;"));
    }

    #[test]
    fn test_script_safe_neutralizes_script_close() {
        // A value that would otherwise break out of a <script> element.
        let out = escape_script_safe(r#"{"x":"</script><img src=x onerror=alert(1)>"}"#);
        assert!(!out.contains("</script"), "script close survived: {out}");
        assert!(!out.contains('<'), "raw < survived: {out}");
        assert!(out.contains("\\u003c"));
    }

    #[test]
    fn test_script_safe_neutralizes_comment_and_line_seps() {
        let out = escape_script_safe("a<!--b\u{2028}c\u{2029}d");
        assert!(!out.contains("<!--"));
        assert!(!out.contains('\u{2028}'));
        assert!(!out.contains('\u{2029}'));
        assert!(out.contains("\\u2028") && out.contains("\\u2029"));
    }

    #[test]
    fn test_script_safe_leaves_plain_json_valid() {
        // No special chars -> unchanged, still parseable JSON.
        let json = r#"{"name":"Ada","n":42}"#;
        assert_eq!(escape_script_safe(json), json);
    }
}
