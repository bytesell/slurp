/// HTML sanitizer for `{html sanitize}` nodes.
///
/// Uses the `ammonia` crate with a strict allowlist configuration.  The
/// allowlist approach is safer than a blocklist because new dangerous tags
/// added to future HTML specs are denied by default rather than allowed.
///
/// Specifically denied regardless of any allowlist:
/// - `<script>`, `<iframe>`, `<object>`, `<embed>`, `<applet>`, `<form>`,
///   `<input>`, `<button>`, `<select>`, `<textarea>`, `<link>`, `<style>`,
///   `<base>`, `<meta>`
/// - All `on*` event attributes (`onclick`, `onload`, ...)
/// - `href="javascript:..."` and `src="javascript:..."`
/// - `data:` URIs in any URL attribute
use ammonia::Builder;
use std::collections::HashSet;

// Only called for `{html sanitize}` - raw `{html}` nodes bypass this entirely.
pub fn sanitize_html(input: &str) -> String {
    // Configured explicitly rather than relying on Ammonia defaults for clarity and future-proofing.
    let mut builder = Builder::new();
    builder.tags(allowed_tags());
    builder.generic_attributes(allowed_generic_attrs());
    builder.url_schemes(allowed_url_schemes());
    // SECURITY: any anchor that opens a new context (`target="_blank"`) must carry
    // `rel="noopener noreferrer"` so the opened page cannot reach back via
    // `window.opener` (reverse tabnabbing) and does not leak the referrer.
    //
    // Ammonia sets this rel on EVERY `<a>` it emits, with no test for `target` - it is
    // STRICTER than the requirement above, not equal to it. Worth knowing before someone
    // "fixes" a same-tab link that carries a rel it appears not to need: removing the rel
    // means writing a target test ammonia does not have.
    builder.link_rel(Some("noopener noreferrer"));
    builder.clean(input).to_string()
}

fn allowed_tags() -> HashSet<&'static str> {
    [
        "p",
        "br",
        "hr",
        "span",
        "div",
        "section",
        "article",
        "aside",
        "header",
        "footer",
        "main",
        "nav",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "ul",
        "ol",
        "li",
        "dl",
        "dt",
        "dd",
        "a",
        "strong",
        "em",
        "b",
        "i",
        "u",
        "s",
        "strike",
        "del",
        "ins",
        "mark",
        "sup",
        "sub",
        "small",
        "abbr",
        "cite",
        "code",
        "pre",
        "kbd",
        "samp",
        "var",
        "q",
        "blockquote",
        // no script-capable src
        "img",
        "figure",
        "figcaption",
        "picture",
        "source",
        "table",
        "thead",
        "tbody",
        "tfoot",
        "tr",
        "th",
        "td",
        "caption",
        "colgroup",
        "col",
        "time",
        "data",
        "address",
        "details",
        "summary",
    ]
    .iter()
    .cloned()
    .collect()
}

fn allowed_generic_attrs() -> HashSet<&'static str> {
    [
        "class",
        "id",
        "title",
        "lang",
        "dir",
        "aria-label",
        "aria-describedby",
        "aria-hidden",
        "aria-live",
        "aria-atomic",
        "aria-relevant",
        "role",
        "href",
        "target",
        // SECURITY NOTE: `src` on the allowed `img`/`source`/`picture` tags lets
        // sanitized content load OFF-ORIGIN resources, which can act as a
        // tracking/beacon (the request leaks IP + a referrer). This is kept
        // intentionally because richtext legitimately embeds images, and
        // the URL is already constrained to the `allowed_url_schemes()` allowlist
        // (http/https/mailto - no `data:`/`javascript:`). If off-origin beacons
        // become a concern, drop `src` here or add an origin allowlist.
        //
        // `srcset` is NOT allowed, and that is the whole point of this note.
        // Ammonia's URL-scheme check covers `href`, `xlink:href`, `src` and a few
        // tag-specific pairs - it does NOT parse `srcset`, whose value is a
        // comma-separated list of `url descriptor` candidates. So a
        // `data:text/html` candidate passed `{html v sanitize}` UNCHANGED, which
        // made the module doc above ("data: URIs in any URL attribute" are
        // denied) false. The choice is between teaching a sanitizer the srcset
        // grammar and dropping one responsive-image nicety from editor-supplied
        // richtext; dropping it costs nothing, because `src` still renders the
        // image. `sizes` stays - it carries no URL.
        "src",
        "alt",
        "width",
        "height",
        "loading",
        "decoding",
        "sizes",
        "colspan",
        "rowspan",
        "scope",
        "headers",
        "datetime",
        "value",
        "open",
    ]
    .iter()
    .cloned()
    .collect()
}

fn allowed_url_schemes() -> HashSet<&'static str> {
    ["http", "https", "mailto"].iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strips_script() {
        let out = sanitize_html("<script>alert(1)</script>safe");
        assert!(!out.contains("<script"), "script tag not stripped: {out}");
        assert!(out.contains("safe"));
    }

    #[test]
    fn test_strips_iframe() {
        let out = sanitize_html("<iframe src='evil.com'></iframe>safe");
        assert!(!out.contains("iframe"), "iframe not stripped: {out}");
    }

    #[test]
    fn test_strips_object() {
        let out = sanitize_html("<object data='x.swf'></object>");
        assert!(!out.contains("object"), "object not stripped: {out}");
    }

    #[test]
    fn test_strips_onerror() {
        let out = sanitize_html(r#"<img src="x" onerror="alert(1)">"#);
        assert!(!out.contains("onerror"), "onerror not stripped: {out}");
    }

    #[test]
    fn test_strips_javascript_href() {
        let out = sanitize_html(r#"<a href="javascript:alert(1)">click</a>"#);
        assert!(
            !out.contains("javascript:"),
            "javascript: href not stripped: {out}"
        );
    }

    #[test]
    fn test_allows_safe_href() {
        let out = sanitize_html(r#"<a href="https://example.com">link</a>"#);
        assert!(
            out.contains("https://example.com"),
            "safe href was stripped: {out}"
        );
    }

    #[test]
    fn test_allows_strong_em() {
        let out = sanitize_html("<p>Hello <strong>world</strong></p>");
        assert!(out.contains("<strong>"), "strong was stripped: {out}");
    }

    #[test]
    fn test_strips_form() {
        let out = sanitize_html("<form action='/evil'><input type='text'/></form>");
        assert!(!out.contains("<form"), "form not stripped: {out}");
        assert!(!out.contains("<input"), "input not stripped: {out}");
    }

    #[test]
    fn test_strips_data_uri() {
        let out = sanitize_html(r#"<img src="data:text/html,<script>alert(1)</script>">"#);
        assert!(!out.contains("data:"), "data: URI not stripped: {out}");
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(sanitize_html(""), "");
    }

    #[test]
    fn test_plain_text_passthrough() {
        let out = sanitize_html("Hello, world!");
        assert!(out.contains("Hello, world!"));
    }

    #[test]
    fn test_target_link_gets_noopener_rel() {
        let out = sanitize_html(r#"<a href="https://example.com" target="_blank">x</a>"#);
        assert!(out.contains("noopener"), "rel not injected: {out}");
        assert!(out.contains("noreferrer"), "rel not injected: {out}");
    }

    /// Ammonia's `link_rel` is UNCONDITIONAL: it stamps the rel on every `<a>` it emits, with no
    /// test for `target`. Pinned so that nobody "corrects" it by adding a target check, which
    /// would leave targeted links relying on a rule the sanitizer no longer applies.
    ///
    /// Stricter than the requirement, and harmless: `noopener noreferrer` on a same-tab link
    /// changes nothing except suppressing the referrer.
    #[test]
    fn every_sanitized_link_gets_the_rel_not_just_targeted_ones() {
        let out = sanitize_html(r#"<a href="https://example.com">x</a>"#);
        assert!(
            out.contains("noopener") && out.contains("noreferrer"),
            "ammonia is documented as stamping rel on EVERY anchor; got: {out}"
        );

        let same_tab = sanitize_html(r#"<a href="/local" target="_self">x</a>"#);
        assert!(
            same_tab.contains("noopener"),
            "rel missing on a _self link: {same_tab}"
        );
    }
}
