use crate::ast::*;
use crate::error::ErrorCode;
use crate::lexer::token::Token;
use crate::parser::Parser;

fn infix_bp(tok: &Token) -> Option<(u8, u8)> {
    match tok {
        Token::Or => Some((10, 11)),
        Token::And => Some((20, 21)),
        Token::DoubleEquals | Token::BangEquals => Some((30, 31)),
        Token::LessThan | Token::GreaterThan | Token::LessThanEquals | Token::GreaterThanEquals => {
            Some((40, 41))
        }
        Token::Plus | Token::Minus => Some((50, 51)),
        Token::Star | Token::Slash | Token::Percent => Some((60, 61)),
        _ => None,
    }
}

#[must_use]
pub fn parse_expr(p: &mut Parser) -> Option<Expression> {
    // SECURITY: every expression sub-parser routes back through `parse_expr`
    // (grouped, ternary, call args, index, filter args, ...), so guarding here
    // bounds the whole expression recursion against stack-overflow DoS.
    if !p.enter_depth() {
        return None;
    }
    let r = parse_expr_inner(p);
    p.exit_depth();
    r
}

fn parse_expr_inner(p: &mut Parser) -> Option<Expression> {
    let mut expr = parse_ternary(p)?;

    // Pipe-chained value filters: expr | filter | filter(arg)
    while p.check(&Token::Pipe) {
        p.advance();
        let filter = parse_filter(p)?;
        expr.filters.push(filter);
    }

    Some(expr)
}

fn parse_ternary(p: &mut Parser) -> Option<Expression> {
    // SECURITY: the `then`/`else` operands recurse straight back into
    // `parse_ternary` (not through the depth-guarded `parse_expr`/`parse_binary`),
    // so a chain like `1?1:1?1:...` would otherwise exhaust the native stack
    // while the depth counter stayed flat. Guard here too.
    if !p.enter_depth() {
        return None;
    }
    let r = parse_ternary_inner(p);
    p.exit_depth();
    r
}

fn parse_ternary_inner(p: &mut Parser) -> Option<Expression> {
    let condition = parse_binary(p, 0)?;

    if p.check(&Token::Question) {
        let sp = p.peek_span();
        p.advance();
        let then = parse_ternary(p)?;
        let colon_sp = p.peek_span();
        if !p.eat(&Token::Colon) {
            p.push_error(
                colon_sp,
                "Expected : in ternary expression",
                ErrorCode::UnexpectedToken,
            );
            return None;
        }
        let else_ = parse_ternary(p)?;
        let line = sp.line;
        let column = sp.column;
        return Some(Expression {
            kind: ExprKind::Ternary {
                condition: Box::new(condition),
                then: Box::new(then),
                else_: Box::new(else_),
            },
            filters: Vec::new(),
            line,
            column,
        });
    }

    Some(condition)
}

// Pratt precedence climbing.
fn parse_binary(p: &mut Parser, min_bp: u8) -> Option<Expression> {
    // SECURITY: rhs recursion (`a op b op c ...`) is depth-guarded here.
    if !p.enter_depth() {
        return None;
    }
    let r = parse_binary_inner(p, min_bp);
    p.exit_depth();
    r
}

fn parse_binary_inner(p: &mut Parser, min_bp: u8) -> Option<Expression> {
    let mut lhs = parse_unary(p)?;

    loop {
        let op_tok = p.peek().clone();
        let (l_bp, r_bp) = match infix_bp(&op_tok) {
            Some(bp) => bp,
            None => break,
        };
        if l_bp < min_bp {
            break;
        }

        let sp = p.peek_span();
        p.advance();

        let rhs = parse_binary(p, r_bp)?;

        let op = match op_tok {
            Token::Plus => BinaryOp::Add,
            Token::Minus => BinaryOp::Sub,
            Token::Star => BinaryOp::Mul,
            Token::Slash => BinaryOp::Div,
            Token::Percent => BinaryOp::Mod,
            Token::GreaterThan => BinaryOp::Gt,
            Token::LessThan => BinaryOp::Lt,
            Token::GreaterThanEquals => BinaryOp::Gte,
            Token::LessThanEquals => BinaryOp::Lte,
            Token::DoubleEquals => BinaryOp::Eq,
            Token::BangEquals => BinaryOp::Neq,
            Token::And => BinaryOp::And,
            Token::Or => BinaryOp::Or,
            _ => unreachable!(),
        };

        lhs = Expression {
            kind: ExprKind::BinaryOp {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            },
            filters: Vec::new(),
            line: sp.line,
            column: sp.column,
        };
    }

    Some(lhs)
}

fn parse_unary(p: &mut Parser) -> Option<Expression> {
    // SECURITY: prefix-operator chains (`!!!...`, `---...`) recurse here.
    if !p.enter_depth() {
        return None;
    }
    let r = parse_unary_inner(p);
    p.exit_depth();
    r
}

fn parse_unary_inner(p: &mut Parser) -> Option<Expression> {
    let sp = p.peek_span();
    if p.check(&Token::Bang) {
        p.advance();
        let operand = parse_unary(p)?;
        return Some(Expression {
            kind: ExprKind::UnaryNot(Box::new(operand)),
            filters: Vec::new(),
            line: sp.line,
            column: sp.column,
        });
    }
    if p.check(&Token::Minus) {
        p.advance();
        let operand = parse_unary(p)?;
        return Some(Expression {
            kind: ExprKind::UnaryNeg(Box::new(operand)),
            filters: Vec::new(),
            line: sp.line,
            column: sp.column,
        });
    }
    parse_postfix(p)
}

fn parse_postfix(p: &mut Parser) -> Option<Expression> {
    let mut expr = parse_primary(p)?;

    loop {
        if p.check(&Token::LParen) {
            let sp = p.peek_span();
            p.advance();
            let args = parse_arg_list(p)?;
            expr = Expression {
                kind: ExprKind::Call {
                    callee: Box::new(expr),
                    args,
                },
                filters: Vec::new(),
                line: sp.line,
                column: sp.column,
            };
        } else if p.check(&Token::LBracket) {
            let sp = p.peek_span();
            p.advance();
            let index = parse_expr(p)?;
            let close_sp = p.peek_span();
            if !p.eat(&Token::RBracket) {
                p.push_error(
                    close_sp,
                    "Expected ] to close index expression",
                    ErrorCode::UnclosedBlock,
                );
                return None;
            }
            expr = Expression {
                kind: ExprKind::Index {
                    object: Box::new(expr),
                    index: Box::new(index),
                },
                filters: Vec::new(),
                line: sp.line,
                column: sp.column,
            };
        } else {
            break;
        }
    }

    Some(expr)
}

fn parse_primary(p: &mut Parser) -> Option<Expression> {
    let sp = p.peek_span();

    match p.peek().clone() {
        Token::LParen => {
            p.advance();
            let inner = parse_expr(p)?;
            let close_sp = p.peek_span();
            if !p.eat(&Token::RParen) {
                p.push_error(
                    close_sp,
                    "Expected ) to close grouped expression",
                    ErrorCode::UnclosedBlock,
                );
                return None;
            }
            Some(Expression {
                kind: ExprKind::Grouped(Box::new(inner)),
                filters: Vec::new(),
                line: sp.line,
                column: sp.column,
            })
        }

        Token::StringLiteral(s) => {
            let s = s.clone();
            p.advance();
            Some(Expression::new(
                ExprKind::StringLiteral(s),
                sp.line,
                sp.column,
            ))
        }

        Token::NumberLiteral(n) => {
            p.advance();
            Some(Expression::new(
                ExprKind::NumberLiteral(n),
                sp.line,
                sp.column,
            ))
        }

        Token::BoolLiteral(b) => {
            p.advance();
            Some(Expression::new(
                ExprKind::BoolLiteral(b),
                sp.line,
                sp.column,
            ))
        }

        Token::Null => {
            p.advance();
            Some(Expression::new(ExprKind::Null, sp.line, sp.column))
        }

        // Template literal `` `hello ${name}` ``
        Token::TemplateLiteralOpen => parse_template_literal(p),

        // Reactive path: $name
        Token::Dollar => {
            p.advance();
            let name_sp = p.peek_span();
            match p.peek().clone() {
                Token::Ident(name) => {
                    let name = name.clone();
                    p.advance();
                    let segments = parse_path_tail(p, name);
                    Some(Expression::new(
                        ExprKind::ReactivePath(segments),
                        sp.line,
                        sp.column,
                    ))
                }
                other => {
                    p.push_error(
                        name_sp,
                        format!("Expected identifier after $, found {other:?}"),
                        ErrorCode::UnexpectedToken,
                    );
                    None
                }
            }
        }

        Token::Ident(name) => {
            let name = name.clone();
            p.advance();
            let segments = parse_path_tail(p, name);
            Some(Expression::new(
                ExprKind::Path(segments),
                sp.line,
                sp.column,
            ))
        }

        // Wildcard (only meaningful inside match arms but handled here for completeness)
        Token::Underscore => {
            p.advance();
            Some(Expression::new(
                ExprKind::Path(vec![PathSegment::Ident("_".to_string())]),
                sp.line,
                sp.column,
            ))
        }

        other => {
            p.push_error(
                sp,
                format!("Unexpected token in expression: {other:?}"),
                ErrorCode::UnexpectedToken,
            );
            None
        }
    }
}

fn parse_path_tail(p: &mut Parser, first: String) -> Vec<PathSegment> {
    let mut segments = vec![PathSegment::Ident(first)];
    loop {
        if p.check(&Token::Dot) {
            p.advance();
            match p.peek().clone() {
                Token::Ident(name) => {
                    let name = name.clone();
                    p.advance();
                    segments.push(PathSegment::Ident(name));
                }
                _ => {
                    // Allow trailing dot to be lenient (error recovery)
                    break;
                }
            }
        } else if p.check(&Token::LBracket) {
            p.advance();
            if let Some(idx_expr) = parse_expr(p) {
                segments.push(PathSegment::Index(Box::new(idx_expr)));
            }
            p.eat(&Token::RBracket);
        } else {
            break;
        }
    }
    segments
}

fn parse_template_literal(p: &mut Parser) -> Option<Expression> {
    let sp = p.peek_span();
    p.advance();
    let mut parts: Vec<TemplatePart> = Vec::new();

    loop {
        match p.peek().clone() {
            Token::Eof => {
                p.push_error(
                    sp,
                    "Unterminated template literal",
                    ErrorCode::UnclosedBlock,
                );
                break;
            }
            Token::TemplateLiteralClose => {
                p.advance();
                break;
            }
            Token::TemplateLiteralText(t) => {
                let t = t.clone();
                p.advance();
                parts.push(TemplatePart::Literal(t));
            }
            Token::TemplateExprOpen => {
                p.advance();
                if let Some(expr) = parse_expr(p) {
                    parts.push(TemplatePart::Expr(expr));
                }
                let close_sp = p.peek_span();
                if !p.eat(&Token::ExprClose) {
                    p.push_error(
                        close_sp,
                        "Expected } to close template expression",
                        ErrorCode::UnclosedBlock,
                    );
                }
            }
            other => {
                let s = format!("{other:?}");
                p.advance();
                parts.push(TemplatePart::Literal(s));
            }
        }
    }

    Some(Expression::new(
        ExprKind::TemplateLiteral(parts),
        sp.line,
        sp.column,
    ))
}

/// Parse a comma-separated argument list (already past opening `(`).
fn parse_arg_list(p: &mut Parser) -> Option<Vec<Expression>> {
    let mut args = Vec::new();
    loop {
        if p.check(&Token::RParen) {
            p.advance();
            break;
        }
        if p.check(&Token::Eof) {
            let sp = p.peek_span();
            p.push_error(sp, "Unterminated argument list", ErrorCode::UnclosedBlock);
            return None;
        }
        let before = p.position();
        if let Some(expr) = parse_expr(p) {
            args.push(expr);
        }
        if !p.eat(&Token::Comma) && !p.check(&Token::RParen) {
            let sp = p.peek_span();
            p.push_error(
                sp,
                "Expected , or ) in argument list",
                ErrorCode::UnexpectedToken,
            );
        }
        // Forward-progress guard: if this iteration consumed no tokens (e.g. a
        // token `parse_expr` cannot start on, like `*`), bail rather than loop
        // forever pushing the same error and exhausting memory.
        if p.position() == before {
            return None;
        }
    }
    Some(args)
}

#[must_use]
pub fn parse_filter(p: &mut Parser) -> Option<Filter> {
    let sp = p.peek_span();
    let name = match p.peek().clone() {
        Token::Ident(n) => {
            p.advance();
            n
        }
        other => {
            p.push_error(
                sp,
                format!("Expected filter name, found {other:?}"),
                ErrorCode::UnexpectedToken,
            );
            return None;
        }
    };

    let mut args = Vec::new();
    if p.check(&Token::LParen) {
        p.advance();
        loop {
            if p.check(&Token::RParen) {
                p.advance();
                break;
            }
            if p.check(&Token::Eof) {
                let sp = p.peek_span();
                p.push_error(
                    sp,
                    "Unterminated filter argument list",
                    ErrorCode::UnclosedBlock,
                );
                return None;
            }
            let before = p.position();
            if let Some(expr) = parse_expr(p) {
                args.push(expr);
            }
            p.eat(&Token::Comma);
            // Forward-progress guard (see `parse_arg_list`): bail if the
            // iteration consumed nothing, so `${ x | f(*) }` cannot spin.
            if p.position() == before {
                return None;
            }
        }
    }

    Some(Filter {
        name,
        args,
        line: sp.line,
        column: sp.column,
    })
}

// Parses `{ "cls": cond, "cls2" }` for `class={...}`. Already past the opening `{`.
#[must_use]
pub fn parse_class_list(p: &mut Parser) -> Option<Expression> {
    let sp = p.peek_span();
    let mut entries: Vec<ClassEntry> = Vec::new();

    loop {
        if p.check(&Token::RBrace) || p.check(&Token::ExprClose) {
            p.advance();
            break;
        }
        if p.check(&Token::Eof) {
            p.push_error(sp, "Unterminated class list", ErrorCode::UnclosedBlock);
            return None;
        }

        // Each entry is either:
        //   "class"           → always-on
        //   "class": cond     → conditional
        //   expr: cond        → dynamic class name with condition
        let value = parse_expr(p)?;
        let condition = if p.eat(&Token::Colon) {
            Some(parse_expr(p)?)
        } else {
            None
        };
        entries.push(ClassEntry { value, condition });
        p.eat(&Token::Comma);
    }

    Some(Expression::new(
        ExprKind::ClassList(entries),
        sp.line,
        sp.column,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn parse_expr_from(src: &str) -> Expression {
        // Wrap in ${ } so the tokeniser enters expression mode
        let wrapped = format!("${{{src}}}");
        let (doc, diag) = parse(&wrapped, "test").expect("lex error");
        assert!(!diag.has_errors(), "Parse errors: {:?}", diag.errors);
        match &doc.children[0] {
            crate::ast::Node::Expression(e) => e.clone(),
            other => panic!("Expected Expression, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_string_literal() {
        let e = parse_expr_from(r#""hello""#);
        assert_eq!(e.kind, ExprKind::StringLiteral("hello".into()));
    }

    #[test]
    fn test_parse_number() {
        let e = parse_expr_from("42");
        assert_eq!(e.kind, ExprKind::NumberLiteral(42.0));
    }

    #[test]
    fn test_parse_float() {
        let e = parse_expr_from("1.25");
        assert_eq!(e.kind, ExprKind::NumberLiteral(1.25));
    }

    #[test]
    fn test_parse_bool_true() {
        let e = parse_expr_from("true");
        assert_eq!(e.kind, ExprKind::BoolLiteral(true));
    }

    #[test]
    fn test_parse_bool_false() {
        let e = parse_expr_from("false");
        assert_eq!(e.kind, ExprKind::BoolLiteral(false));
    }

    #[test]
    fn test_parse_null() {
        let e = parse_expr_from("null");
        assert_eq!(e.kind, ExprKind::Null);
    }

    #[test]
    fn test_parse_path() {
        let e = parse_expr_from("user.name");
        assert!(matches!(e.kind, ExprKind::Path(_)));
        if let ExprKind::Path(segs) = &e.kind {
            assert_eq!(segs.len(), 2);
        }
    }

    #[test]
    fn test_parse_deep_path() {
        let e = parse_expr_from("a.b.c.d");
        if let ExprKind::Path(segs) = &e.kind {
            assert_eq!(segs.len(), 4);
        } else {
            panic!("Expected Path");
        }
    }

    #[test]
    fn test_parse_reactive_path() {
        let e = parse_expr_from("$cart");
        assert!(matches!(e.kind, ExprKind::ReactivePath(_)));
    }

    #[test]
    fn test_parse_binary_add() {
        let e = parse_expr_from("a + b");
        assert!(matches!(
            e.kind,
            ExprKind::BinaryOp {
                op: BinaryOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_binary_comparison() {
        let e = parse_expr_from("x == y");
        assert!(matches!(
            e.kind,
            ExprKind::BinaryOp {
                op: BinaryOp::Eq,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_unary_not() {
        let e = parse_expr_from("!active");
        assert!(matches!(e.kind, ExprKind::UnaryNot(_)));
    }

    #[test]
    fn test_parse_unary_neg() {
        let e = parse_expr_from("-5");
        assert!(matches!(e.kind, ExprKind::UnaryNeg(_)));
    }

    #[test]
    fn test_parse_ternary() {
        let e = parse_expr_from("a ? b : c");
        assert!(matches!(e.kind, ExprKind::Ternary { .. }));
    }

    #[test]
    fn test_parse_grouped() {
        let e = parse_expr_from("(a + b)");
        assert!(matches!(e.kind, ExprKind::Grouped(_)));
    }

    #[test]
    fn test_parse_pipe_filter() {
        let e = parse_expr_from("name | upper");
        assert_eq!(e.filters.len(), 1);
        assert_eq!(e.filters[0].name, "upper");
    }

    #[test]
    fn test_parse_pipe_filter_with_args() {
        let e = parse_expr_from(r#"name | truncate(50)"#);
        assert_eq!(e.filters[0].name, "truncate");
        assert_eq!(e.filters[0].args.len(), 1);
    }

    #[test]
    fn test_parse_multiple_filters() {
        let e = parse_expr_from("name | upper | truncate(10)");
        assert_eq!(e.filters.len(), 2);
    }

    #[test]
    fn test_parse_template_literal() {
        let e = parse_expr_from("`hello ${name}`");
        assert!(matches!(e.kind, ExprKind::TemplateLiteral(_)));
    }

    #[test]
    fn test_precedence_mul_over_add() {
        let e = parse_expr_from("a + b * c");
        // Top level should be Add, with right being Mul
        if let ExprKind::BinaryOp { op, right, .. } = &e.kind {
            assert_eq!(*op, BinaryOp::Add);
            assert!(matches!(
                right.kind,
                ExprKind::BinaryOp {
                    op: BinaryOp::Mul,
                    ..
                }
            ));
        } else {
            panic!("Expected BinaryOp");
        }
    }

    /// Parse `src` on a worker thread and fail if it does not finish within
    /// `secs`. Used to pin the parser-DoS regressions: without the
    /// forward-progress guard these inputs spin forever, so a timeout here is a
    /// real failure rather than a hang of the whole test binary.
    fn parse_within(src: &str, secs: u64) -> bool {
        use std::sync::mpsc;
        use std::time::Duration;
        let owned = src.to_string();
        let worker_src = owned.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // We only care that it TERMINATES and reports errors, not the AST.
            let has_err = crate::parser::parse(&worker_src, "dos")
                .map(|(_, diag)| diag.has_errors())
                .unwrap_or(true);
            let _ = tx.send(has_err);
        });
        match rx.recv_timeout(Duration::from_secs(secs)) {
            Ok(has_err) => has_err,
            Err(_) => panic!("parser did not terminate within {secs}s for input: {owned}"),
        }
    }

    #[test]
    fn arg_list_no_progress_terminates() {
        // Regression: `${ foo(*) }` once looped forever in parse_arg_list - `*`
        // is a token parse_expr cannot start on, and the old loop pushed an
        // error without consuming it.
        assert!(parse_within("${ foo(*) }", 5), "expected parse errors");
    }

    #[test]
    fn filter_arg_list_no_progress_terminates() {
        // Regression: `${ x | f(*) }` once looped forever in parse_filter for
        // the same reason.
        assert!(parse_within("${ x | f(*) }", 5), "expected parse errors");
    }

    #[test]
    fn parser_dos_fuzz_seeds_terminate() {
        // A seed corpus of pathological argument/filter lists that all must
        // terminate quickly. Any regression that reintroduces a
        // no-forward-progress loop trips the per-input timeout.
        let seeds = [
            "${ foo(*) }",
            "${ foo(*,*,*) }",
            "${ foo(**) }",
            "${ x | f(*) }",
            "${ x | f(*) | g(*) }",
            "${ foo(bar(*)) }",
            "${ foo(1, *, 2) }",
            "${ x | f(1, *) }",
            "${ foo(@) }",
            "${ foo(#) }",
        ];
        for s in seeds {
            assert!(parse_within(s, 5), "input did not terminate: {s}");
        }
    }
}
