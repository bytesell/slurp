//! Where does a `${ ... }` slot LAND inside a JavaScript-evaluated attribute?
//!
//! SECURITY. Auto-escaping an interpolated slot with `escape_js_string` is correct when the slot
//! sits INSIDE a JS string literal - it neutralizes the delimiters, so the value cannot leave the
//! literal it is in. It does NOTHING for a slot in STATEMENT or EXPRESSION position, because `;`
//! and `(` have no JS string escape and none is possible:
//!
//! ```text
//! <div x-init="n = ${v}"></div>   with v = `1;alert(1)`   ->   n = 1;alert(1)
//! ```
//!
//! Telling the two apart is a lexical property of the attribute's own LITERAL text, which the
//! author wrote and the compiler can therefore read at compile time. That is what this module
//! computes: for each slot of an interpolated attribute value, the quote character of the JS
//! string literal enclosing it, or `None` when nothing encloses it.
//!
//! The scan walks the literal segments BETWEEN the slots and carries quote state across them, so
//! `f('${a}', "${b}")` reports `'` and `"`, and a slot inside a nested quote (`'a "x" ${v}'`)
//! reports the OUTER quote that is actually open. A slot inside a backtick template literal
//! reports `` ` `` - which is a string literal for escaping purposes, and separately a strong
//! signal that the author meant a JavaScript template literal and slurp is about to eat it (see
//! [`template_literal_in_attr`]).
//!
//! A slot itself never changes the quote state. Its value is escaped for the context it lands in,
//! so by construction it cannot open or close a literal; assuming otherwise would let a data value
//! decide how the NEXT slot is treated, which is the same class of bug as trusting the value in
//! the first place.

use crate::ast::TemplatePart;

/// What the scan of one interpolated attribute value found.
pub(crate) struct SlotContexts {
    /// For every `TemplatePart::Expr`, in order: the quote character of the JS string literal
    /// enclosing that slot, or `None` if the slot is in statement/expression position.
    pub quotes: Vec<Option<char>>,
    /// A string literal opened somewhere in the value and never closed. The attribute is then
    /// ambiguous - most often the "quote" is ordinary prose (`title="use ` + backtick + `npm` ...")
    /// rather than a literal at all - so a diagnostic that GUESSES at author intent must stay
    /// quiet here, while the escapers stay conservative and treat the slot as quoted.
    pub unterminated: bool,
}

#[must_use]
pub(crate) fn slot_quote_contexts(parts: &[TemplatePart]) -> SlotContexts {
    let mut quotes = Vec::new();
    let mut quote: Option<char> = None;
    for part in parts {
        match part {
            TemplatePart::Expr(_) => quotes.push(quote),
            TemplatePart::Literal(text) => scan_literal(text, &mut quote),
        }
    }
    SlotContexts {
        quotes,
        unterminated: quote.is_some(),
    }
}

/// Advance the quote state across one literal segment of the attribute's text.
fn scan_literal(text: &str, quote: &mut Option<char>) {
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match (*quote, ch) {
            // A backslash escape inside a literal consumes the next character, so `'it\'s'` does
            // not read as a closed literal followed by an unterminated one.
            (Some(_), '\\') => {
                chars.next();
            }
            (Some(open), c) if c == open => *quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"' | '`') => *quote = Some(ch),
            (None, _) => {}
        }
    }
}

/// The diagnostic for `| js` used where there is no string literal to be inside of.
///
/// `| js` is a CLAIM about context: "this value lands inside a quoted JS string literal, so escape
/// the delimiters". When the claim is false the filter is worse than nothing - a declared slot is
/// passed through VERBATIM, so `x-init="n = ${ v | js }"` with `1;alert(1)` used to render live
/// code. Refusing is the honest outcome, exactly as a `<script>` body refuses an undeclared slot.
#[must_use]
pub(crate) fn js_filter_outside_string_literal(attr_name: &str) -> String {
    format!(
        "`| js` declares that this slot sits inside a JavaScript string literal, but in the \
         JavaScript-evaluated attribute `{attr_name}` it does not - it lands in statement or \
         expression position, where escaping the quote characters cannot contain a value (`;` and \
         `(` have no JS string escape). Quote the slot (`'${{ v | js }}'`), or use `| json` for a \
         bare JSON value, or `| unsafe_js` if it is JavaScript the theme itself wrote"
    )
}

/// The diagnostic for a JavaScript template literal written in an HTML attribute.
///
/// The bug class this catches: the author writes `` :href="`/cart/${item.id}`" `` meaning the
/// client-side framework's `item`, slurp reads `${ }` as ITS interpolation, evaluates `item.id`
/// against the SERVER context where it does not exist, and renders `` `/cart/` ``. Nothing fails -
/// the page builds clean and the link is dead.
#[must_use]
pub(crate) fn template_literal_in_attr(attr_name: &str) -> String {
    format!(
        "a JavaScript template literal (backticks) in the attribute `{attr_name}` contains a \
         `${{ }}` slot, and slurp interpolates `${{ }}` in attribute values itself - the slot is \
         evaluated against the SERVER render context, not the browser's, so a client-side variable \
         renders EMPTY with no other sign of failure. Build the string by concatenation \
         (`'/cart/' + item.id`) if the value is client-side, or drop the backticks if it is \
         server-side data"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{ExprKind, Expression};

    fn slot() -> TemplatePart {
        TemplatePart::Expr(Expression::new(ExprKind::Null, 1, 1))
    }

    fn lit(s: &str) -> TemplatePart {
        TemplatePart::Literal(s.to_string())
    }

    /// Just the per-slot quote contexts, which is what most of these assert on.
    fn q(parts: &[TemplatePart]) -> Vec<Option<char>> {
        slot_quote_contexts(parts).quotes
    }

    #[test]
    fn a_slot_between_quotes_reports_that_quote() {
        assert_eq!(q(&[lit("f('"), slot(), lit("')")]), vec![Some('\'')]);
        assert_eq!(q(&[lit("f(\""), slot(), lit("\")")]), vec![Some('"')]);
        assert_eq!(q(&[lit("f(`"), slot(), lit("`)")]), vec![Some('`')]);
    }

    #[test]
    fn a_bare_slot_reports_statement_position() {
        assert_eq!(q(&[lit("n = "), slot()]), vec![None]);
        assert_eq!(q(&[slot()]), vec![None]);
        // ... and so does a slot AFTER a closed literal, which is the case a naive
        // "is there a quote anywhere before me" test gets wrong.
        assert_eq!(q(&[lit("f('a') + "), slot()]), vec![None]);
    }

    #[test]
    fn each_slot_is_judged_on_its_own() {
        let parts = [lit("f('"), slot(), lit("', "), slot(), lit(")")];
        assert_eq!(q(&parts), vec![Some('\''), None]);
    }

    #[test]
    fn a_nested_quote_does_not_close_the_open_one() {
        // The `"` inside a single-quoted literal is ordinary text; the slot is still inside `'`.
        assert_eq!(
            q(&[lit("f('a \"x\" "), slot(), lit("')")]),
            vec![Some('\'')]
        );
    }

    #[test]
    fn an_escaped_quote_does_not_close_the_literal() {
        assert_eq!(q(&[lit(r"f('it\'s "), slot(), lit("')")]), vec![Some('\'')]);
        // A trailing backslash must not read past the end of the segment.
        assert_eq!(q(&[lit(r"f('a\"), slot(), lit("')")]), vec![Some('\'')]);
    }

    #[test]
    fn an_unclosed_literal_is_reported_as_such() {
        // A lone quote is far more often ordinary prose than an opened literal, so the caller has
        // to be able to tell. The scan still treats the slot as quoted (the conservative choice for
        // escaping); only a diagnostic that guesses at INTENT backs off.
        let one = slot_quote_contexts(&[lit("a ` b "), slot()]);
        assert_eq!(one.quotes, vec![Some('`')]);
        assert!(one.unterminated);

        let closed = slot_quote_contexts(&[lit("a `b "), slot(), lit("` c")]);
        assert_eq!(closed.quotes, vec![Some('`')]);
        assert!(!closed.unterminated);
    }

    #[test]
    fn quote_state_carries_across_a_slot() {
        // The slot itself is escaped for its context, so it cannot open or close a literal.
        assert_eq!(
            q(&[lit("f('"), slot(), lit(" and "), slot(), lit("')")]),
            vec![Some('\''), Some('\'')]
        );
    }
}
