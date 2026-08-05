use crate::ast::*;
use crate::error::ErrorCode;
use crate::lexer::token::Token;
use crate::parser::Parser;
use crate::parser::expressions::parse_expr;

pub fn parse_if(p: &mut Parser) -> Option<IfNode> {
    let sp = p.peek_span();
    p.advance();

    let condition = p.parse_expression()?;
    let close_sp = p.peek_span();
    if !p.eat(&Token::ExprClose) {
        p.push_error(
            close_sp,
            "Expected } to close {if condition}",
            ErrorCode::UnclosedBlock,
        );
    }

    let then_branch = p.parse_nodes_until_close("if");
    let mut else_if_branches: Vec<(Expression, Vec<Node>)> = Vec::new();
    let mut else_branch: Option<Vec<Node>> = None;

    loop {
        match p.peek().clone() {
            Token::BlockElseIf => {
                p.advance();
                let cond = p.parse_expression()?;
                let ec_sp = p.peek_span();
                if !p.eat(&Token::ExprClose) {
                    p.push_error(
                        ec_sp,
                        "Expected } to close {else if condition}",
                        ErrorCode::UnclosedBlock,
                    );
                }
                let body = p.parse_nodes_until_close("if");
                else_if_branches.push((cond, body));
            }
            Token::BlockElse => {
                p.advance();
                let body = p.parse_nodes_until_close("if");
                else_branch = Some(body);
                break;
            }
            Token::BlockClose(ref name) if name == "if" => break,
            Token::Eof => {
                p.push_error(sp, "Unclosed {if} block", ErrorCode::UnclosedBlock);
                break;
            }
            _ => break,
        }
    }

    if p.check(&Token::BlockClose("if".to_string())) {
        p.advance();
    }

    Some(IfNode {
        condition,
        then_branch,
        else_if_branches,
        else_branch,
    })
}

pub fn parse_each(p: &mut Parser) -> Option<EachNode> {
    let sp = p.peek_span();
    p.advance();

    let item = match p.peek().clone() {
        Token::Ident(n) => {
            let n = n.clone();
            p.advance();
            n
        }
        other => {
            p.push_error(
                sp,
                format!("Expected item name in {{each}}, found {other:?}"),
                ErrorCode::UnexpectedToken,
            );
            return None;
        }
    };

    let index = if p.eat(&Token::Comma) {
        match p.peek().clone() {
            Token::Ident(n) => {
                let n = n.clone();
                p.advance();
                Some(n)
            }
            _ => None,
        }
    } else {
        None
    };

    let in_sp = p.peek_span();
    match p.peek().clone() {
        Token::Ident(ref kw) if kw == "in" => {
            p.advance();
        }
        _ => {
            p.push_error(
                in_sp,
                "Expected 'in' keyword in {each}",
                ErrorCode::UnexpectedToken,
            );
            return None;
        }
    }

    let mut collection = parse_expr(p)?;
    // `parse_expr` consumed any `| filter` pipes onto the expression; move them to loop_filters
    let loop_filters: Vec<Filter> = collection.filters.drain(..).collect();

    let close_sp = p.peek_span();
    if !p.eat(&Token::ExprClose) {
        p.push_error(
            close_sp,
            "Expected } to close {each}",
            ErrorCode::UnclosedBlock,
        );
    }

    let body = p.parse_nodes_until_close("each");

    let empty_branch = if p.check(&Token::BlockEmpty) {
        p.advance();
        Some(p.parse_nodes_until_close("each"))
    } else {
        None
    };

    if p.check(&Token::BlockClose("each".to_string())) {
        p.advance();
    } else {
        p.push_error(sp, "Unclosed {each} block", ErrorCode::UnclosedBlock);
    }

    Some(EachNode {
        item,
        index,
        collection,
        loop_filters,
        body,
        empty_branch,
    })
}

pub fn parse_match(p: &mut Parser) -> Option<MatchNode> {
    let sp = p.peek_span();
    p.advance();

    let value = p.parse_expression()?;
    let close_sp = p.peek_span();
    if !p.eat(&Token::ExprClose) {
        p.push_error(
            close_sp,
            "Expected } to close {match expr}",
            ErrorCode::UnclosedBlock,
        );
    }

    let mut arms: Vec<MatchArm> = Vec::new();

    loop {
        match p.peek().clone() {
            Token::BlockClose(ref name) if name == "match" => {
                p.advance();
                break;
            }
            Token::Eof => {
                p.push_error(sp, "Unclosed {match} block", ErrorCode::UnclosedBlock);
                break;
            }
            _ => {
                if let Some(arm) = parse_match_arm(p) {
                    arms.push(arm);
                } else if !p.check(&Token::Eof) {
                    p.advance();
                }
            }
        }
    }

    Some(MatchNode { value, arms })
}

fn parse_match_arm(p: &mut Parser) -> Option<MatchArm> {
    let sp = p.peek_span();

    let pattern = match p.peek().clone() {
        Token::StringLiteral(s) => {
            p.advance();
            MatchPattern::StringLiteral(s)
        }
        Token::NumberLiteral(n) => {
            p.advance();
            MatchPattern::NumberLiteral(n)
        }
        Token::BoolLiteral(b) => {
            p.advance();
            MatchPattern::BoolLiteral(b)
        }
        Token::Underscore => {
            p.advance();
            MatchPattern::Wildcard
        }
        Token::Ident(ref n) if n == "_" => {
            p.advance();
            MatchPattern::Wildcard
        }
        other => {
            p.push_error(
                sp,
                format!("Expected match pattern, found {other:?}"),
                ErrorCode::UnexpectedToken,
            );
            return None;
        }
    };

    if !p.eat(&Token::Arrow) {
        // Produce an empty-body arm rather than an error for trailing `_ {/match}`.
        match p.peek() {
            Token::BlockClose(n) if n == "match" => {
                return Some(MatchArm {
                    pattern,
                    body: Vec::new(),
                });
            }
            Token::Eof => {
                return Some(MatchArm {
                    pattern,
                    body: Vec::new(),
                });
            }
            _ => {
                let arrow_sp = p.peek_span();
                p.push_error(
                    arrow_sp,
                    "Expected -> after match pattern",
                    ErrorCode::UnexpectedToken,
                );
                return None;
            }
        }
    }

    let body = parse_match_arm_body(p);

    Some(MatchArm { pattern, body })
}

fn parse_match_arm_body(p: &mut Parser) -> Vec<Node> {
    let mut nodes = Vec::new();
    loop {
        match p.peek().clone() {
            Token::Eof => break,
            Token::BlockClose(ref name) if name == "match" => break,
            // Next arm pattern - stop collecting body nodes
            Token::StringLiteral(_)
            | Token::NumberLiteral(_)
            | Token::BoolLiteral(_)
            | Token::Underscore => break,
            Token::Ident(ref n) if n == "_" => break,
            _ => {
                if let Some(node) = p.parse_node() {
                    nodes.push(node);
                } else {
                    if p.check(&Token::Eof) {
                        break;
                    }
                    p.advance();
                }
            }
        }
    }
    nodes
}

pub fn parse_fetch(p: &mut Parser) -> Option<FetchNode> {
    let sp = p.peek_span();
    p.advance();

    let name = match p.peek().clone() {
        Token::Ident(n) => {
            let n = n.clone();
            p.advance();
            n
        }
        other => {
            p.push_error(
                sp,
                format!("Expected fetch name, found {other:?}"),
                ErrorCode::UnexpectedToken,
            );
            return None;
        }
    };

    let (type_hint, is_array) = if p.eat(&Token::Colon) {
        let hint = match p.peek().clone() {
            Token::Ident(t) => {
                let t = t.clone();
                p.advance();
                t
            }
            _ => String::new(),
        };
        let is_arr = if p.eat(&Token::LBracket) {
            p.eat(&Token::RBracket);
            true
        } else {
            false
        };
        (Some(hint), is_arr)
    } else {
        (None, false)
    };

    let from_sp = p.peek_span();
    match p.peek().clone() {
        Token::Ident(ref kw) if kw == "from" => {
            p.advance();
        }
        _ => {
            p.push_error(
                from_sp,
                "Expected 'from' in {fetch}",
                ErrorCode::UnexpectedToken,
            );
            return None;
        }
    }

    let source = parse_expr(p)?;
    let options = parse_fetch_options(p);

    let close_sp = p.peek_span();
    if !p.eat(&Token::ExprClose) {
        p.push_error(
            close_sp,
            "Expected } to close {fetch}",
            ErrorCode::UnclosedBlock,
        );
    }

    let body = parse_fetch_branch(p, "fetch");
    let loading_branch = parse_optional_named_branch(p, "loading", "fetch");
    let error_branch = parse_optional_named_branch(p, "error", "fetch");
    let empty_branch = if p.check(&Token::BlockEmpty) {
        p.advance();
        Some(p.parse_nodes_until_close("fetch"))
    } else {
        None
    };

    if p.check(&Token::BlockClose("fetch".to_string())) {
        p.advance();
    } else {
        p.push_error(sp, "Unclosed {fetch} block", ErrorCode::UnclosedBlock);
    }

    Some(FetchNode {
        name,
        type_hint,
        is_array,
        source,
        options,
        body,
        loading_branch,
        error_branch,
        empty_branch,
    })
}

// collapsible_match suppressed: using match guards with side-effectful p.eat() is confusing.
#[allow(clippy::collapsible_match)]
fn parse_fetch_options(p: &mut Parser) -> FetchOptions {
    let mut opts = FetchOptions::default();

    loop {
        match p.peek().clone() {
            Token::ExprClose | Token::Eof => break,
            Token::Ident(kw) => {
                let kw = kw.clone();
                p.advance();
                match kw.as_str() {
                    "cache" => {
                        if p.eat(&Token::LParen) {
                            if let Token::NumberLiteral(n) = p.peek().clone() {
                                opts.cache_ttl = Some(n as u64);
                                p.advance();
                            }
                            p.eat(&Token::RParen);
                        }
                    }
                    "retry" => {
                        if p.eat(&Token::LParen) {
                            if let Token::NumberLiteral(n) = p.peek().clone() {
                                opts.retry = Some(n as u32);
                                p.advance();
                            }
                            p.eat(&Token::RParen);
                        }
                    }
                    "timeout" => {
                        if p.eat(&Token::LParen) {
                            if let Token::NumberLiteral(n) = p.peek().clone() {
                                opts.timeout_ms = Some(n as u64);
                                p.advance();
                            }
                            p.eat(&Token::RParen);
                        }
                    }
                    "abort-on-navigate" | "abortOnNavigate" => {
                        opts.abort_on_navigate = true;
                    }
                    "poll" => {
                        if p.eat(&Token::LParen) {
                            if let Token::NumberLiteral(n) = p.peek().clone() {
                                opts.poll_seconds = Some(n as u64);
                                p.advance();
                            }
                            p.eat(&Token::RParen);
                        }
                    }
                    "paginate" => {
                        if p.eat(&Token::LParen) {
                            if let Token::NumberLiteral(n) = p.peek().clone() {
                                opts.paginate = Some(PaginateOptions { limit: n as u32 });
                                p.advance();
                            }
                            p.eat(&Token::RParen);
                        }
                    }
                    "infinite" => {
                        if p.eat(&Token::LParen) {
                            if let Token::NumberLiteral(n) = p.peek().clone() {
                                opts.infinite = Some(InfiniteOptions { limit: n as u32 });
                                p.advance();
                            }
                            p.eat(&Token::RParen);
                        }
                    }
                    _ => {}
                }
            }
            _ => {
                p.advance();
            }
        }
    }

    opts
}

fn parse_fetch_branch(p: &mut Parser, close: &str) -> Vec<Node> {
    let mut nodes = Vec::new();
    loop {
        match p.peek().clone() {
            Token::Eof => break,
            Token::BlockClose(ref n) if n == close => break,
            Token::BlockOpen(ref n) if n == "loading" || n == "error" => break,
            Token::BlockEmpty => break,
            _ => {
                if let Some(n) = p.parse_node() {
                    nodes.push(n);
                } else {
                    if p.check(&Token::Eof) {
                        break;
                    }
                    p.advance();
                }
            }
        }
    }
    nodes
}

fn parse_optional_named_branch(
    p: &mut Parser,
    branch_name: &str,
    close: &str,
) -> Option<Vec<Node>> {
    if let Token::BlockOpen(ref n) = p.peek().clone()
        && n == branch_name
    {
        p.advance();
        p.eat(&Token::ExprClose);
        let nodes = parse_fetch_branch(p, close);
        return Some(nodes);
    }
    None
}

pub fn parse_repeat(p: &mut Parser) -> Option<RepeatNode> {
    let sp = p.peek_span();
    p.advance();
    let count = p.parse_expression()?;
    let close_sp = p.peek_span();
    if !p.eat(&Token::ExprClose) {
        p.push_error(
            close_sp,
            "Expected } to close {repeat}",
            ErrorCode::UnclosedBlock,
        );
    }
    let body = p.parse_nodes_until_close("repeat");
    if p.check(&Token::BlockClose("repeat".to_string())) {
        p.advance();
    } else {
        p.push_error(sp, "Unclosed {repeat} block", ErrorCode::UnclosedBlock);
    }
    Some(RepeatNode { count, body })
}

pub fn parse_try(p: &mut Parser) -> Option<TryNode> {
    let sp = p.peek_span();
    p.advance();
    p.eat(&Token::ExprClose);

    let body = parse_try_body(p);
    let error_branch = if let Token::BlockOpen(ref n) = p.peek().clone() {
        if n == "error" {
            p.advance();
            p.eat(&Token::ExprClose);
            p.parse_nodes_until_close("try")
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    if p.check(&Token::BlockClose("try".to_string())) {
        p.advance();
    } else {
        p.push_error(sp, "Unclosed {try} block", ErrorCode::UnclosedBlock);
    }

    Some(TryNode { body, error_branch })
}

fn parse_try_body(p: &mut Parser) -> Vec<Node> {
    let mut nodes = Vec::new();
    loop {
        match p.peek().clone() {
            Token::Eof => break,
            Token::BlockClose(ref n) if n == "try" => break,
            Token::BlockOpen(ref n) if n == "error" => break,
            _ => {
                if let Some(n) = p.parse_node() {
                    nodes.push(n);
                } else {
                    if p.check(&Token::Eof) {
                        break;
                    }
                    p.advance();
                }
            }
        }
    }
    nodes
}

pub fn parse_slot(p: &mut Parser) -> Option<SlotNode> {
    p.advance();

    let name = match p.peek().clone() {
        Token::StringLiteral(s) => {
            let s = s.clone();
            p.advance();
            Some(s)
        }
        Token::Ident(s) => {
            let s = s.clone();
            p.advance();
            Some(s)
        }
        Token::ExprClose => None,
        _ => None,
    };

    // `{slot}` and `{slot "name"}` both arrive with the closing `}` still in the
    // stream as an ExprClose, so the header always ends here.
    //
    // Returning as soon as that `}` is eaten is wrong: well-formed
    // `{slot}fallback{/slot}` then produces `fallback: None` and leaves the body
    // as SIBLING nodes of the slot, so the fallback renders UNCONDITIONALLY -
    // including when the slot is filled - which is the exact opposite of what a
    // fallback is. It also makes `SlotNode.fallback` reachable only from
    // malformed source, so the security walk's coverage of it can only be tested
    // against a hand-built AST.
    //
    // A bare `{slot}` with no `{/slot}` is still legal and must keep meaning "no
    // fallback", so the body is only taken when a matching close actually exists.
    // Scanning for it (rather than parsing and rolling back) keeps the parser's
    // error accumulator out of it: `parse_nodes_until_close` reports an unclosed
    // block at EOF, and a speculative parse would have to un-report that.
    if p.eat(&Token::ExprClose) && !has_block_close(p, "slot") {
        return Some(SlotNode {
            name,
            fallback: None,
        });
    }

    let nodes = p.parse_nodes_until_close("slot");
    if p.check(&Token::BlockClose("slot".to_string())) {
        p.advance();
    }

    Some(SlotNode {
        name,
        fallback: Some(nodes),
    })
}

/// Is there a `{/name}` anywhere ahead in the token stream?
///
/// Deliberately NOT nesting-aware: `{slot}` cannot meaningfully contain another
/// `{slot}`, and the only decision this drives is "does a body exist at all".
fn has_block_close(p: &Parser, name: &str) -> bool {
    p.remaining_tokens()
        .iter()
        .any(|(t, _)| matches!(t, Token::BlockClose(n) if n == name))
}

pub fn parse_layout(p: &mut Parser) -> Option<LayoutNode> {
    let sp = p.peek_span();
    p.advance();

    let src = match p.peek().clone() {
        Token::StringLiteral(s) => {
            let s = s.clone();
            p.advance();
            s
        }
        Token::Ident(s) => {
            let s = s.clone();
            p.advance();
            s
        }
        other => {
            p.push_error(
                sp,
                format!("Expected layout path, found {other:?}"),
                ErrorCode::UnexpectedToken,
            );
            return None;
        }
    };

    let close_sp = p.peek_span();
    if !p.eat(&Token::ExprClose) {
        p.push_error(
            close_sp,
            "Expected } to close {layout}",
            ErrorCode::UnclosedBlock,
        );
    }

    let children = p.parse_nodes_until_close("layout");
    if p.check(&Token::BlockClose("layout".to_string())) {
        p.advance();
    } else {
        p.push_error(sp, "Unclosed {layout} block", ErrorCode::UnclosedBlock);
    }

    let mut head_nodes: Option<Vec<Node>> = None;
    let mut body_nodes: Vec<Node> = Vec::new();
    for node in children {
        match node {
            Node::Head(h) => {
                head_nodes = Some(h);
            }
            n => body_nodes.push(n),
        }
    }

    Some(LayoutNode {
        src,
        head: head_nodes,
        children: body_nodes,
    })
}

pub fn parse_head(p: &mut Parser) -> Option<Vec<Node>> {
    let sp = p.peek_span();
    p.advance();
    p.eat(&Token::ExprClose);

    let nodes = p.parse_nodes_until_close("head");
    if p.check(&Token::BlockClose("head".to_string())) {
        p.advance();
    } else {
        p.push_error(sp, "Unclosed {head} block", ErrorCode::UnclosedBlock);
    }
    Some(nodes)
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
    fn test_if_then_else() {
        let doc = parse_ok("{if show}yes{else}no{/if}");
        if let Node::If(n) = &doc.children[0] {
            assert!(n.else_branch.is_some());
        } else {
            panic!("expected If");
        }
    }

    #[test]
    fn test_if_else_if() {
        let doc = parse_ok("{if a}A{else if b}B{else}C{/if}");
        if let Node::If(n) = &doc.children[0] {
            assert_eq!(n.else_if_branches.len(), 1);
            assert!(n.else_branch.is_some());
        } else {
            panic!("expected If");
        }
    }

    #[test]
    fn test_each_basic() {
        let doc = parse_ok("{each item in items}${item.name}{/each}");
        if let Node::Each(n) = &doc.children[0] {
            assert_eq!(n.item, "item");
            assert!(n.index.is_none());
        } else {
            panic!("expected Each");
        }
    }

    #[test]
    fn test_each_with_index() {
        let doc = parse_ok("{each item, i in list}${i}{/each}");
        if let Node::Each(n) = &doc.children[0] {
            assert_eq!(n.index.as_deref(), Some("i"));
        } else {
            panic!("expected Each");
        }
    }

    #[test]
    fn test_each_empty_branch() {
        let doc = parse_ok("{each x in xs}${x}{empty}none{/each}");
        if let Node::Each(n) = &doc.children[0] {
            assert!(n.empty_branch.is_some());
        } else {
            panic!("expected Each");
        }
    }

    #[test]
    fn test_match_basic() {
        let doc = parse_ok(r#"{match status}"open" -> open"closed" -> closed_ -> {/match}"#);
        if let Node::Match(n) = &doc.children[0] {
            assert_eq!(n.arms.len(), 3);
        } else {
            panic!("expected Match");
        }
    }

    #[test]
    fn test_repeat() {
        let doc = parse_ok("{repeat 5}*{/repeat}");
        assert!(matches!(doc.children[0], Node::Repeat(_)));
    }

    #[test]
    fn test_try_error() {
        let doc = parse_ok("{try}content{error}fallback{/try}");
        if let Node::Try(n) = &doc.children[0] {
            assert!(!n.error_branch.is_empty());
        } else {
            panic!("expected Try");
        }
    }

    #[test]
    fn test_slot_default() {
        let doc = parse_ok("{slot}");
        assert!(matches!(doc.children[0], Node::Slot(_)));
    }

    #[test]
    fn test_slot_named() {
        let doc = parse_ok(r#"{slot "header"}"#);
        if let Node::Slot(s) = &doc.children[0] {
            assert_eq!(s.name.as_deref(), Some("header"));
        } else {
            panic!("expected Slot");
        }
    }

    #[test]
    fn test_layout() {
        let doc = parse_ok(r#"{layout "base.slurp"}content{/layout}"#);
        if let Node::Layout(l) = &doc.children[0] {
            assert_eq!(l.src, "base.slurp");
        } else {
            panic!("expected Layout");
        }
    }

    #[test]
    fn test_nested_if() {
        let doc = parse_ok("{if a}{if b}inner{/if}{/if}");
        if let Node::If(outer) = &doc.children[0] {
            assert!(matches!(outer.then_branch[0], Node::If(_)));
        } else {
            panic!("expected If");
        }
    }

    #[test]
    fn test_multiline_each() {
        let src = "{each item in products}\n  <div>${item.name}</div>\n{/each}";
        let doc = parse_ok(src);
        assert!(matches!(doc.children[0], Node::Each(_)));
    }
}
