use crate::ast::*;
use crate::error::ErrorCode;
use crate::lexer::{Lexer, token::Token};
use crate::parser::Parser;
use crate::parser::expressions::{parse_class_list, parse_expr};

pub fn parse_element_or_component(p: &mut Parser, tag_name: &str) -> Option<Node> {
    let sp = p.peek_span();
    p.advance();

    // <Image> is treated as a component despite lowercase convention
    let is_component = tag_name
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
        || tag_name == "Image";

    if is_component {
        parse_component(p, tag_name, sp)
    } else {
        parse_element(p, tag_name, sp)
    }
}

fn parse_component(p: &mut Parser, name: &str, sp: crate::lexer::token::Span) -> Option<Node> {
    let ComponentAttrs {
        props,
        spread,
        events,
        slot,
        self_closing,
    } = parse_attrs_and_props(p, name)?;

    if self_closing {
        return Some(Node::Component(ComponentNode {
            name: name.to_string(),
            props,
            spread,
            children: Vec::new(),
            slot,
            events,
            line: sp.line,
            column: sp.column,
        }));
    }

    let children = parse_children_until_close(p, name);

    Some(Node::Component(ComponentNode {
        name: name.to_string(),
        props,
        spread,
        children,
        slot,
        events,
        line: sp.line,
        column: sp.column,
    }))
}

fn parse_element(p: &mut Parser, tag_name: &str, _sp: crate::lexer::token::Span) -> Option<Node> {
    let (attrs, self_closing) = parse_element_attrs(p, tag_name)?;

    let is_void = is_void_element(tag_name);
    if self_closing || is_void {
        return Some(Node::Element(ElementNode {
            tag: tag_name.to_string(),
            attrs,
            children: Vec::new(),
            self_closing: true,
        }));
    }

    let children = parse_children_until_close(p, tag_name);

    Some(Node::Element(ElementNode {
        tag: tag_name.to_string(),
        attrs,
        children,
        self_closing: false,
    }))
}

/// Everything an opening component tag carries, other than its name.
struct ComponentAttrs {
    props: Vec<Prop>,
    /// The `{...expr}` spread, at most one per tag.
    spread: Option<Expression>,
    events: Vec<EventBinding>,
    /// The `slot="name"` this component fills in its parent.
    slot: Option<String>,
    self_closing: bool,
}

fn parse_attrs_and_props(p: &mut Parser, component_name: &str) -> Option<ComponentAttrs> {
    let mut props: Vec<Prop> = Vec::new();
    let mut spread: Option<Expression> = None;
    let mut events: Vec<EventBinding> = Vec::new();
    let mut slot: Option<String> = None;
    let mut self_closing = false;

    loop {
        match p.peek().clone() {
            Token::TagSelfClose => {
                p.advance();
                self_closing = true;
                break;
            }
            Token::TagEnd => {
                p.advance();
                break;
            }
            Token::Eof => {
                let sp = p.peek_span();
                p.push_error(
                    sp,
                    format!("Unclosed component tag <{component_name}>"),
                    ErrorCode::UnclosedBlock,
                );
                return None;
            }
            Token::DotDotDot => {
                p.advance();
                if let Some(expr) = parse_expr(p) {
                    spread = Some(expr);
                }
            }
            Token::OnDirective(event) => {
                let event = event.clone();
                p.advance();
                let handler = if p.eat(&Token::AttrEquals) {
                    match p.peek().clone() {
                        Token::StringLiteral(s) => {
                            let s = s.clone();
                            p.advance();
                            s
                        }
                        _ => String::new(),
                    }
                } else {
                    String::new()
                };
                events.push(EventBinding { event, handler });
            }
            Token::AttrName(ref n) if n == "slot" => {
                p.advance();
                if p.eat(&Token::AttrEquals)
                    && let Token::StringLiteral(s) = p.peek().clone()
                {
                    slot = Some(s.clone());
                    p.advance();
                }
            }
            Token::AttrName(name) => {
                let name = name.clone();
                p.advance();
                let value = if p.eat(&Token::AttrEquals) {
                    parse_prop_value(p)
                } else {
                    PropValue::Boolean
                };
                props.push(Prop { name, value });
            }
            Token::Ident(name) => {
                let name = name.clone();
                p.advance();
                let value = if p.eat(&Token::AttrEquals) {
                    parse_prop_value(p)
                } else {
                    PropValue::Boolean
                };
                props.push(Prop { name, value });
            }
            _ => {
                p.advance();
            }
        }
    }

    Some(ComponentAttrs {
        props,
        spread,
        events,
        slot,
        self_closing,
    })
}

fn parse_prop_value(p: &mut Parser) -> PropValue {
    match p.peek().clone() {
        Token::StringLiteral(s) => {
            let s = s.clone();
            p.advance();
            PropValue::Expr(Expression::new(ExprKind::StringLiteral(s), 0, 0))
        }
        Token::BoolLiteral(b) => {
            p.advance();
            PropValue::Expr(Expression::new(ExprKind::BoolLiteral(b), 0, 0))
        }
        Token::NumberLiteral(n) => {
            p.advance();
            PropValue::Expr(Expression::new(ExprKind::NumberLiteral(n), 0, 0))
        }
        Token::LBrace => {
            p.advance();
            if let Some(expr) = parse_expr(p) {
                p.eat(&Token::ExprClose);
                PropValue::Expr(expr)
            } else {
                PropValue::Boolean
            }
        }
        Token::TemplateLiteralOpen => {
            if let Some(expr) = super::expressions::parse_expr(p) {
                PropValue::Expr(expr)
            } else {
                PropValue::Boolean
            }
        }
        _ => PropValue::Boolean,
    }
}

fn parse_element_attrs(p: &mut Parser, tag_name: &str) -> Option<(Vec<Attribute>, bool)> {
    let mut attrs: Vec<Attribute> = Vec::new();
    let mut self_closing = false;

    loop {
        match p.peek().clone() {
            Token::TagSelfClose => {
                p.advance();
                self_closing = true;
                break;
            }
            Token::TagEnd => {
                p.advance();
                break;
            }
            Token::Eof => {
                let sp = p.peek_span();
                p.push_error(
                    sp,
                    format!("Unclosed element tag <{tag_name}>"),
                    ErrorCode::UnclosedBlock,
                );
                return None;
            }
            Token::OnDirective(event) => {
                let event = event.clone();
                p.advance();
                let handler = if p.eat(&Token::AttrEquals) {
                    match p.peek().clone() {
                        Token::StringLiteral(s) => {
                            let s = s.clone();
                            p.advance();
                            s
                        }
                        _ => String::new(),
                    }
                } else {
                    String::new()
                };
                attrs.push(Attribute {
                    name: format!("on:{event}"),
                    value: AttributeValue::EventHandler(handler),
                });
            }
            Token::NavigateAttr => {
                p.advance();
                if p.eat(&Token::AttrEquals) {
                    // Every way an author can write "false" has to mean the same thing. Anything
                    // this misses falls through to "attribute present", which the runtime reads
                    // as opted IN - the exact opposite of what was asked for, with no
                    // diagnostic. An opt-out that fails to the wrong side is worse than one
                    // that does not exist, so the tolerance here is deliberate.
                    //
                    // The count is how many tokens the spelling occupies: the brace form is
                    // three (`{`, `false`, `}`), everything else is one.
                    let consume = match p.peek().clone() {
                        Token::BoolLiteral(false) => Some(1),
                        // In HtmlTag mode `false` is lexed as AttrName("false")
                        Token::AttrName(ref s) if s == "false" => Some(1),
                        Token::Ident(ref s) if s == "false" => Some(1),
                        // `navigate="false"`, the spelling ordinary HTML leads you to write.
                        Token::StringLiteral(ref s) if s == "false" => Some(1),
                        // `navigate={false}`, the brace form used for every other attribute.
                        Token::LBrace
                            if matches!(p.peek_nth(1), Token::BoolLiteral(false))
                                && matches!(p.peek_nth(2), Token::ExprClose) =>
                        {
                            Some(3)
                        }
                        _ => None,
                    };
                    if let Some(n) = consume {
                        for _ in 0..n {
                            p.advance();
                        }
                        attrs.push(Attribute {
                            name: "navigate".into(),
                            value: AttributeValue::NavigateFalse,
                        });
                    } else {
                        attrs.push(Attribute {
                            name: "navigate".into(),
                            value: AttributeValue::Boolean,
                        });
                    }
                } else {
                    attrs.push(Attribute {
                        name: "navigate".into(),
                        value: AttributeValue::Boolean,
                    });
                }
            }
            Token::AttrName(name) => {
                let name = name.clone();
                p.advance();
                let value = if p.eat(&Token::AttrEquals) {
                    parse_attr_value(p, &name)
                } else {
                    AttributeValue::Boolean
                };
                attrs.push(Attribute { name, value });
            }
            Token::Ident(name) => {
                let name = name.clone();
                p.advance();
                let value = if p.eat(&Token::AttrEquals) {
                    parse_attr_value(p, &name)
                } else {
                    AttributeValue::Boolean
                };
                attrs.push(Attribute { name, value });
            }
            Token::LBrace => {
                p.advance(); // consume {, tokenizer is now in Expr mode
                match p.peek().clone() {
                    Token::Ident(ref kw) if kw == "html" => {
                        p.advance(); // consume "html"
                        if let Some(expr) = parse_expr(p) {
                            attrs.push(Attribute {
                                name: String::new(),
                                value: AttributeValue::RawAttrInjection {
                                    condition: None,
                                    expr: Box::new(expr),
                                },
                            });
                        }
                        p.eat(&Token::ExprClose);
                    }
                    Token::Ident(ref kw) if kw == "if" => {
                        p.advance(); // consume "if"
                        let condition = parse_expr(p);
                        p.eat(&Token::ExprClose);
                        // Expect {html body_expr}
                        if matches!(p.peek(), &Token::LBrace) {
                            p.advance(); // consume {
                            if matches!(p.peek(), Token::Ident(k) if k == "html") {
                                p.advance(); // consume "html"
                                let body = parse_expr(p);
                                p.eat(&Token::ExprClose);
                                // Consume {/if}
                                if matches!(p.peek(), &Token::LBrace) {
                                    p.advance();
                                    while !matches!(p.peek(), &Token::ExprClose | &Token::Eof) {
                                        p.advance();
                                    }
                                    p.eat(&Token::ExprClose);
                                }
                                if let (Some(cond), Some(body)) = (condition, body) {
                                    attrs.push(Attribute {
                                        name: String::new(),
                                        value: AttributeValue::RawAttrInjection {
                                            condition: Some(Box::new(cond)),
                                            expr: Box::new(body),
                                        },
                                    });
                                }
                            }
                        }
                    }
                    Token::Slash => {
                        // {/if} close - skip
                        while !matches!(p.peek(), &Token::ExprClose | &Token::Eof) {
                            p.advance();
                        }
                        p.eat(&Token::ExprClose);
                    }
                    _ => {
                        // Unknown {...} - skip to ExprClose
                        while !matches!(p.peek(), &Token::ExprClose | &Token::Eof) {
                            p.advance();
                        }
                        p.eat(&Token::ExprClose);
                    }
                }
            }
            _ => {
                p.advance();
            }
        }
    }

    Some((attrs, self_closing))
}

fn parse_string_as_template_literal(s: &str, line: usize, col: usize) -> Expression {
    let mut parts: Vec<TemplatePart> = Vec::new();
    let mut rest = s;

    while !rest.is_empty() {
        if let Some(idx) = rest.find("${") {
            let text = &rest[..idx];
            if !text.is_empty() {
                parts.push(TemplatePart::Literal(text.to_string()));
            }
            let after_open = &rest[idx + 2..];
            let expr_src = if let Some(close_idx) = after_open.find('}') {
                let expr_text = &after_open[..close_idx];
                rest = &after_open[close_idx + 1..];
                expr_text
            } else {
                parts.push(TemplatePart::Literal(after_open.to_string()));
                break;
            };

            let wrapped = format!("${{{expr_src}}}");
            let expr_tokens = Lexer::new(wrapped.as_str()).tokenize().unwrap_or_default();
            let mut inner_parser = Parser::new(expr_tokens, "attr");
            inner_parser.advance();
            let expr = parse_expr(&mut inner_parser)
                .unwrap_or_else(|| Expression::new(ExprKind::Null, line, col));
            parts.push(TemplatePart::Expr(expr));
        } else {
            if !rest.is_empty() {
                parts.push(TemplatePart::Literal(rest.to_string()));
            }
            break;
        }
    }

    Expression::new(ExprKind::TemplateLiteral(parts), line, col)
}

/// Warn when an attribute value holds a JavaScript template literal with a `${ }` slot inside it.
///
/// The bug class this catches: the author writes `` :href="`/cart/${item.id}`" `` meaning the
/// client-side framework's `item`. Slurp reads `${ }` as ITS interpolation, evaluates `item.id`
/// against the SERVER context where it does not exist, and renders `` `/cart/` ``. It is a silent
/// failure - the build is clean, nothing errors, and only a browser shows the dead link.
///
/// It is a WARNING, not an error, because slurp CAN legitimately interpolate a server value inside
/// a backtick literal and the result is working JavaScript. The compiler cannot tell the author's
/// `item` from the server's, only that the shape is the one that goes wrong; refusing it would
/// break third-party themes that use it correctly.
///
/// It is raised at PARSE time on purpose. A signal raised while rendering would be invisible to
/// `slurp validate` (which does not render) and to any branch that never renders - exactly the two
/// places this bug hides.
fn warn_js_template_literal(
    p: &mut Parser,
    attr_name: &str,
    expr: &Expression,
    sp: crate::lexer::token::Span,
) {
    let ExprKind::TemplateLiteral(parts) = &expr.kind else {
        return;
    };
    let scan = crate::renderer::js_slot::slot_quote_contexts(parts);
    // An UNCLOSED backtick is prose far more often than it is a template literal
    // (`title="run `npm test` first"` is fine; `title="a ` b ${x}"` is not a literal at all), and a
    // heuristic diagnostic that fires on ordinary markup is one everyone learns to ignore.
    if !scan.unterminated && scan.quotes.contains(&Some('`')) {
        p.push_warning(
            sp,
            crate::renderer::js_slot::template_literal_in_attr(attr_name),
            ErrorCode::JsTemplateLiteralInAttribute,
        );
    }
}

fn parse_attr_value(p: &mut Parser, attr_name: &str) -> AttributeValue {
    match p.peek().clone() {
        Token::StringLiteral(s) => {
            let s = s.clone();
            let sp = p.peek_span();
            p.advance();
            if s.contains("${") {
                let expr = parse_string_as_template_literal(&s, sp.line, sp.column);
                warn_js_template_literal(p, attr_name, &expr, sp);
                // NOT `Expr`: see `AttributeValue::Interpolated`. A quoted string
                // with `${ }` slots is author text around DATA, and in a
                // JS-evaluated attribute the renderer has to escape those slots.
                return AttributeValue::Interpolated(expr);
            }
            AttributeValue::StringLiteral(s)
        }
        Token::BoolLiteral(false) if attr_name == "navigate" => {
            p.advance();
            AttributeValue::NavigateFalse
        }
        Token::BoolLiteral(b) => {
            p.advance();
            AttributeValue::Expr(Expression::new(ExprKind::BoolLiteral(b), 0, 0))
        }
        Token::LBrace => {
            p.advance();
            // class={...} gets ClassList treatment for conditional class merging
            if attr_name == "class" {
                if let Some(cl_expr) = parse_class_list(p) {
                    return AttributeValue::ClassList(cl_expr);
                }
                return AttributeValue::Boolean;
            }
            if let Some(expr) = parse_expr(p) {
                p.eat(&Token::ExprClose);
                AttributeValue::Expr(expr)
            } else {
                AttributeValue::Boolean
            }
        }
        Token::TemplateLiteralOpen => {
            if let Some(expr) = super::expressions::parse_expr(p) {
                AttributeValue::Expr(expr)
            } else {
                AttributeValue::Boolean
            }
        }
        _ => AttributeValue::Boolean,
    }
}

fn parse_children_until_close(p: &mut Parser, tag_name: &str) -> Vec<Node> {
    let mut children = Vec::new();
    loop {
        match p.peek_raw().clone() {
            Token::Eof => break,
            Token::Whitespace(w) => {
                children.push(Node::Text(w));
                p.advance_raw();
                continue;
            }
            Token::Newline => {
                children.push(Node::Text("\n".to_string()));
                p.advance_raw();
                continue;
            }
            _ => {}
        }
        match p.peek().clone() {
            Token::Eof => {
                let sp = p.peek_span();
                p.push_error(
                    sp,
                    format!("Unclosed element <{tag_name}>"),
                    ErrorCode::UnclosedBlock,
                );
                break;
            }
            Token::TagClose(ref name) if name == tag_name => {
                p.advance();
                break;
            }
            // A close tag for some OTHER element. It used to be swallowed by error
            // recovery with no diagnostic at all, so `<div></span>` compiled clean and
            // `<div><p></div></p>` silently re-nested. Deliberately a WARNING and the
            // token is still skipped: the tolerant behaviour is kept (hard-failing here
            // would break themes that already compile), the silence is not.
            Token::TagClose(ref name) => {
                let name = name.clone();
                let sp = p.peek_span();
                p.push_warning(
                    sp,
                    format!("Mismatched closing tag </{name}> inside <{tag_name}> - ignored"),
                    ErrorCode::UnexpectedToken,
                );
                p.advance();
            }
            _ => {
                if let Some(node) = p.parse_node() {
                    children.push(node);
                } else {
                    if p.check(&Token::Eof) {
                        break;
                    }
                    // Check for close tag after failed parse (error recovery).
                    if let Token::TagClose(ref name) = p.peek().clone()
                        && name == tag_name
                    {
                        p.advance();
                        break;
                    }
                    p.advance();
                }
            }
        }
    }
    children
}

fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

#[cfg(test)]
mod tests {
    use crate::ast::*;
    use crate::parser::parse;

    fn parse_ok(src: &str) -> Document {
        let (doc, diag) = parse(src, "test.slurp").expect("lex failed");
        assert!(!diag.has_errors(), "Parse errors: {:?}", diag.errors);
        doc
    }

    #[test]
    fn test_div_element() {
        let doc = parse_ok("<div></div>");
        assert!(matches!(doc.children[0], Node::Element(_)));
    }

    #[test]
    fn test_self_closing_br() {
        let doc = parse_ok("<br />");
        if let Node::Element(el) = &doc.children[0] {
            assert!(el.self_closing);
        } else {
            panic!("expected Element");
        }
    }

    #[test]
    fn test_void_img() {
        let doc = parse_ok(r#"<img src="x.png" />"#);
        if let Node::Element(el) = &doc.children[0] {
            assert!(el.self_closing);
        } else {
            panic!("expected Element");
        }
    }

    #[test]
    fn test_component_uppercase() {
        let doc = parse_ok("<Navbar />");
        assert!(matches!(doc.children[0], Node::Component(_)));
    }

    #[test]
    fn test_element_attrs() {
        let doc = parse_ok(r#"<a href="/home" class="link">text</a>"#);
        if let Node::Element(el) = &doc.children[0] {
            assert_eq!(el.attrs.len(), 2);
        } else {
            panic!("expected Element");
        }
    }

    #[test]
    fn test_boolean_attr() {
        let doc = parse_ok("<input disabled />");
        if let Node::Element(el) = &doc.children[0] {
            let disabled = el.attrs.iter().find(|a| a.name == "disabled");
            assert!(disabled.is_some());
            assert!(matches!(disabled.unwrap().value, AttributeValue::Boolean));
        } else {
            panic!();
        }
    }

    #[test]
    fn test_on_directive() {
        let doc = parse_ok(r#"<button on:click="handleClick">click</button>"#);
        if let Node::Element(el) = &doc.children[0] {
            let handler = el.attrs.iter().find(|a| a.name == "on:click");
            assert!(handler.is_some());
        } else {
            panic!();
        }
    }

    /// Every spelling of `navigate=false` must mean the same thing.
    ///
    /// The quoted form is the one ordinary HTML leads you to write, and it used to fall through
    /// to "attribute present", which the runtime reads as opted IN. An opt-out that silently
    /// does the opposite is worse than no opt-out, so this pins all four spellings.
    #[test]
    fn every_spelling_of_navigate_false_opts_out() {
        for src in [
            r#"<a href="/x" navigate=false>l</a>"#,
            r#"<a href="/x" navigate="false">l</a>"#,
            r#"<a href="/x" navigate='false'>l</a>"#,
            r#"<a href="/x" navigate={false}>l</a>"#,
        ] {
            let doc = parse_ok(src);
            let Node::Element(el) = &doc.children[0] else {
                panic!("not an element: {src}")
            };
            let nav = el.attrs.iter().find(|a| a.name == "navigate").expect(src);
            assert!(
                matches!(nav.value, AttributeValue::NavigateFalse),
                "{src} did not opt out; got {:?}",
                nav.value
            );
        }

        // Presence alone still means opted IN, which is the point of the attribute.
        let doc = parse_ok(r#"<a href="/x" navigate>l</a>"#);
        let Node::Element(el) = &doc.children[0] else {
            panic!()
        };
        let nav = el.attrs.iter().find(|a| a.name == "navigate").unwrap();
        assert!(matches!(nav.value, AttributeValue::Boolean));
    }

    #[test]
    fn test_navigate_false() {
        let doc = parse_ok(r#"<a href="/x" navigate=false>link</a>"#);
        if let Node::Element(el) = &doc.children[0] {
            let nav = el.attrs.iter().find(|a| a.name == "navigate");
            assert!(nav.is_some());
            assert!(matches!(nav.unwrap().value, AttributeValue::NavigateFalse));
        } else {
            panic!();
        }
    }

    #[test]
    fn test_component_props() {
        let doc = parse_ok(r#"<Card title="Hello" featured />"#);
        if let Node::Component(c) = &doc.children[0] {
            assert_eq!(c.props.len(), 2);
            assert!(matches!(c.props[1].value, PropValue::Boolean));
        } else {
            panic!();
        }
    }

    #[test]
    fn test_nested_elements() {
        let doc = parse_ok("<ul><li>a</li><li>b</li></ul>");
        if let Node::Element(ul) = &doc.children[0] {
            assert_eq!(
                ul.children
                    .iter()
                    .filter(|n| matches!(n, Node::Element(_)))
                    .count(),
                2
            );
        } else {
            panic!();
        }
    }

    #[test]
    fn test_expr_in_attr() {
        let doc = parse_ok("<div class={name}></div>");
        if let Node::Element(el) = &doc.children[0] {
            let cls = el.attrs.iter().find(|a| a.name == "class");
            assert!(cls.is_some());
        } else {
            panic!();
        }
    }
}
