pub mod blocks;
pub mod expressions;
pub mod frontmatter;
pub mod html;

use crate::ast::*;
use crate::error::{CompileError, CompileResult, ErrorAccumulator, ErrorCode};
use crate::lexer::SpannedToken;
use crate::lexer::token::{Span, Token};

/// SECURITY: hard cap on nesting depth for every recursive-descent path (node,
/// expression, and frontmatter `blocks {}` recursion). Untrusted templates may
/// render in a shared multi-tenant service; without this a deeply
/// nested input (e.g. thousands of `<div>` or `(((...)))`) overflows the stack
/// and aborts the whole process. A clean parse error is returned instead.
pub const MAX_DEPTH: usize = 64;

pub struct Parser {
    tokens: Vec<SpannedToken>,
    pos: usize,
    pub(crate) file: String,
    pub(crate) diagnostics: ErrorAccumulator,
    /// Current recursion depth; guarded against `MAX_DEPTH` at every recursive
    /// entry point. `depth_exceeded` latches once the cap is hit so the error is
    /// reported once and parsing unwinds cleanly.
    depth: usize,
    depth_exceeded: bool,
}

impl Parser {
    pub fn new(tokens: Vec<SpannedToken>, file: impl Into<String>) -> Self {
        Self {
            tokens,
            pos: 0,
            file: file.into(),
            diagnostics: ErrorAccumulator::new(),
            depth: 0,
            depth_exceeded: false,
        }
    }

    /// Enter one level of recursion. Returns `false` (and records a single error)
    /// when `MAX_DEPTH` would be exceeded; callers must then bail without
    /// recursing further. Pair every successful `enter_depth()` with
    /// `exit_depth()`.
    pub(crate) fn enter_depth(&mut self) -> bool {
        if self.depth >= MAX_DEPTH {
            if !self.depth_exceeded {
                self.depth_exceeded = true;
                let sp = self.peek_span();
                self.push_error(
                    sp,
                    format!("Maximum nesting depth ({MAX_DEPTH}) exceeded"),
                    ErrorCode::UnexpectedToken,
                );
            }
            return false;
        }
        self.depth += 1;
        true
    }

    pub(crate) fn exit_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub fn parse_document(&mut self) -> Document {
        let frontmatter = self.try_parse_frontmatter();
        let children = self.parse_nodes_until_eof();
        Document {
            frontmatter,
            children,
            file: self.file.clone(),
        }
    }

    /// Current absolute token cursor (trivia included). Loop bodies that call
    /// fallible sub-parsers use this to detect a lack of forward progress and
    /// bail instead of spinning forever on a token nothing can consume.
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub fn peek(&self) -> &Token {
        self.peek_nth(0)
    }

    pub fn peek_nth(&self, n: usize) -> &Token {
        let mut count = 0;
        let mut i = self.pos;
        loop {
            if i >= self.tokens.len() {
                return &Token::Eof;
            }
            if !self.tokens[i].0.is_trivia() {
                if count == n {
                    return &self.tokens[i].0;
                }
                count += 1;
            }
            i += 1;
        }
    }

    pub fn peek_span(&self) -> Span {
        let mut i = self.pos;
        loop {
            if i >= self.tokens.len() {
                return Span::new(0, 0);
            }
            if !self.tokens[i].0.is_trivia() {
                return self.tokens[i].1;
            }
            i += 1;
        }
    }

    pub fn advance(&mut self) -> (Token, Span) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].0.is_trivia() {
            self.pos += 1;
        }
        if self.pos >= self.tokens.len() {
            return (Token::Eof, Span::new(0, 0));
        }
        let tok = self.tokens[self.pos].clone();
        self.pos += 1;
        tok
    }

    /// Is the next non-trivia token on a LATER line than the cursor?
    ///
    /// `peek_raw()` alone is not enough to answer this: trailing spaces before the
    /// newline mean the raw stream reads [Whitespace, Newline, ...], so a bare
    /// `peek_raw() == Newline` test sees only the Whitespace and misses the line
    /// break entirely. That is how one trailing space after a prop's type silently
    /// swallowed the NEXT declaration.
    pub(crate) fn at_line_end(&self) -> bool {
        let mut i = self.pos;
        while i < self.tokens.len() {
            match &self.tokens[i].0 {
                Token::Newline => return true,
                t if t.is_trivia() => i += 1,
                _ => return false,
            }
        }
        // End of the token stream also terminates the line.
        true
    }

    /// Every token from the cursor onward, trivia included. For the rare
    /// lookahead that has to answer a question about the whole remaining stream
    /// (does a matching close tag exist at all?) rather than about the next few
    /// tokens.
    pub(crate) fn remaining_tokens(&self) -> &[SpannedToken] {
        self.tokens.get(self.pos..).unwrap_or(&[])
    }

    pub fn peek_raw(&self) -> &Token {
        if self.pos >= self.tokens.len() {
            return &Token::Eof;
        }
        &self.tokens[self.pos].0
    }

    pub fn advance_raw(&mut self) -> (Token, Span) {
        if self.pos >= self.tokens.len() {
            return (Token::Eof, Span::new(0, 0));
        }
        let tok = self.tokens[self.pos].clone();
        self.pos += 1;
        tok
    }

    pub fn expect(&mut self, expected: &Token) -> Option<(Token, Span)> {
        let sp = self.peek_span();
        let current = self.peek().clone();
        if Self::token_matches(&current, expected) {
            Some(self.advance())
        } else {
            self.push_error(
                sp,
                format!("Expected {expected:?}, found {current:?}"),
                ErrorCode::UnexpectedToken,
            );
            None
        }
    }

    pub fn check(&self, tok: &Token) -> bool {
        Self::token_matches(self.peek(), tok)
    }

    pub fn eat(&mut self, tok: &Token) -> bool {
        if self.check(tok) {
            self.advance();
            true
        } else {
            false
        }
    }

    // Discriminant-only: payload values (String, f64) are not compared.
    fn token_matches(a: &Token, b: &Token) -> bool {
        std::mem::discriminant(a) == std::mem::discriminant(b)
    }

    pub fn push_error(&mut self, span: Span, message: impl Into<String>, code: ErrorCode) {
        self.diagnostics.push(CompileError::parse(
            self.file.clone(),
            span.line,
            span.column,
            message,
            code,
        ));
    }

    pub fn push_warning(&mut self, span: Span, message: impl Into<String>, code: ErrorCode) {
        self.diagnostics.push(CompileError::warn(
            self.file.clone(),
            span.line,
            span.column,
            message,
            code,
        ));
    }

    pub fn parse_nodes_until_eof(&mut self) -> Vec<Node> {
        let mut nodes = Vec::new();
        loop {
            // Check raw stream first so whitespace/newlines become text nodes.
            match self.peek_raw().clone() {
                Token::Eof => break,
                Token::Whitespace(w) => {
                    nodes.push(Node::Text(w));
                    self.advance_raw();
                    continue;
                }
                Token::Newline => {
                    nodes.push(Node::Text("\n".to_string()));
                    self.advance_raw();
                    continue;
                }
                _ => {}
            }
            match self.peek() {
                Token::Eof => break,
                _ => {
                    if let Some(node) = self.parse_node() {
                        nodes.push(node);
                    } else {
                        // Error-recovery: skip one token to avoid infinite loop
                        if self.peek() == &Token::Eof {
                            break;
                        }
                        self.advance();
                    }
                }
            }
        }
        nodes
    }

    pub fn parse_nodes_until_close(&mut self, close_name: &str) -> Vec<Node> {
        let mut nodes = Vec::new();
        loop {
            // Check raw stream first so whitespace/newlines become text nodes.
            match self.peek_raw().clone() {
                Token::Eof => break,
                Token::Whitespace(w) => {
                    nodes.push(Node::Text(w));
                    self.advance_raw();
                    continue;
                }
                Token::Newline => {
                    nodes.push(Node::Text("\n".to_string()));
                    self.advance_raw();
                    continue;
                }
                _ => {}
            }
            match self.peek().clone() {
                Token::Eof => {
                    let sp = self.peek_span();
                    self.push_error(
                        sp,
                        format!("Unclosed block - expected {{/{close_name}}}"),
                        ErrorCode::UnclosedBlock,
                    );
                    break;
                }
                Token::BlockClose(ref name) if name == close_name => break,
                Token::BlockElse | Token::BlockElseIf | Token::BlockEmpty => break,
                _ => {
                    if let Some(node) = self.parse_node() {
                        nodes.push(node);
                    } else {
                        if matches!(self.peek(), Token::Eof) {
                            break;
                        }
                        self.advance();
                    }
                }
            }
        }
        nodes
    }

    pub fn parse_node(&mut self) -> Option<Node> {
        // SECURITY: guard the node-recursion entry point (covers element/block
        // children, which loop back here). Bail by consuming one token so the
        // surrounding loop still makes progress and terminates.
        if !self.enter_depth() {
            if !matches!(self.peek(), Token::Eof) {
                self.advance();
            }
            return None;
        }
        // Element parsing has large stack frames; grow the stack on native targets
        // so MAX_DEPTH (not the raw 2 MB worker/wasm stack) is the real bound.
        let node = crate::stack::with_stack(|| self.parse_node_inner());
        self.exit_depth();
        node
    }

    fn parse_node_inner(&mut self) -> Option<Node> {
        match self.peek().clone() {
            Token::Text(t) => {
                let t = t.clone();
                self.advance();
                Some(Node::Text(t))
            }
            Token::Whitespace(w) => {
                let w = w.clone();
                self.advance();
                Some(Node::Text(w))
            }
            Token::Newline => {
                self.advance();
                Some(Node::Text("\n".to_string()))
            }
            // Literal tokens that appear in text content and should render as-is.
            Token::BoolLiteral(b) => {
                self.advance();
                Some(Node::Text(if b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }))
            }
            Token::Null => {
                self.advance();
                Some(Node::Text("null".to_string()))
            }
            Token::NumberLiteral(n) => {
                let s = format!("{n}");
                self.advance();
                Some(Node::Text(s))
            }
            Token::StringLiteral(s) => {
                let s = format!("\"{s}\"");
                self.advance();
                Some(Node::Text(s))
            }
            Token::Comment(c) => {
                let c = c.clone();
                self.advance();
                Some(Node::Comment(c))
            }
            Token::ExprOpen => {
                self.advance();
                let expr = self.parse_expression()?;
                let close_sp = self.peek_span();
                if !self.eat(&Token::ExprClose) {
                    self.push_error(
                        close_sp,
                        "Expected } to close expression",
                        ErrorCode::UnclosedBlock,
                    );
                }
                Some(Node::Expression(expr))
            }
            Token::LBrace => {
                let sp = self.peek_span();
                self.advance();
                match self.peek().clone() {
                    Token::Ident(_)
                    | Token::NumberLiteral(_)
                    | Token::StringLiteral(_)
                    | Token::BoolLiteral(_)
                    | Token::Null
                    | Token::Bang
                    | Token::Minus
                    | Token::LParen
                    | Token::Dollar => {
                        let expr = self.parse_expression()?;
                        let close_sp = self.peek_span();
                        if !self.eat(&Token::ExprClose) {
                            self.push_error(
                                close_sp,
                                "Expected } to close expression",
                                ErrorCode::UnclosedBlock,
                            );
                        }
                        Some(Node::Expression(expr))
                    }
                    _ => {
                        self.push_error(
                            sp,
                            "Unexpected { - did you mean ${...}?",
                            ErrorCode::UnexpectedToken,
                        );
                        None
                    }
                }
            }
            Token::RawHtml => {
                self.advance();
                let expr = self.parse_expression()?;
                let close_sp = self.peek_span();
                if !self.eat(&Token::ExprClose) {
                    self.push_error(
                        close_sp,
                        "Expected } after {html expression}",
                        ErrorCode::UnclosedBlock,
                    );
                }
                Some(Node::RawHtml {
                    expr,
                    sanitize: false,
                })
            }
            Token::RawHtmlSanitize => {
                self.advance();
                let expr = self.parse_expression()?;
                // Consume optional trailing `sanitize` keyword before }.
                if let Token::Ident(ref kw) = self.peek().clone()
                    && kw == "sanitize"
                {
                    self.advance();
                }
                let close_sp = self.peek_span();
                if !self.eat(&Token::ExprClose) {
                    self.push_error(
                        close_sp,
                        "Expected } after {html sanitize expression}",
                        ErrorCode::UnclosedBlock,
                    );
                }
                Some(Node::RawHtml {
                    expr,
                    sanitize: true,
                })
            }
            Token::BlockOpen(ref name) => {
                let name = name.clone();
                self.parse_block_node(&name)
            }
            Token::ClientLet => self.parse_client_let(),
            Token::Debug => self.parse_debug(),
            Token::Redirect => self.parse_redirect(),
            Token::Next => {
                self.advance();
                Some(Node::Next)
            }
            Token::TagOpen(ref name) => {
                let name = name.clone();
                html::parse_element_or_component(self, &name)
            }
            Token::TagClose(_) => None,
            Token::Eof => None,
            other => {
                let sp = self.peek_span();
                self.push_error(
                    sp,
                    format!("Unexpected token {other:?} in template"),
                    ErrorCode::UnexpectedToken,
                );
                self.advance();
                None
            }
        }
    }

    fn parse_block_node(&mut self, name: &str) -> Option<Node> {
        match name {
            "if" => blocks::parse_if(self).map(Node::If),
            "each" => blocks::parse_each(self).map(Node::Each),
            "match" => blocks::parse_match(self).map(Node::Match),
            "fetch" => blocks::parse_fetch(self).map(Node::Fetch),
            "repeat" => blocks::parse_repeat(self).map(Node::Repeat),
            "try" => blocks::parse_try(self).map(Node::Try),
            "slot" => blocks::parse_slot(self).map(Node::Slot),
            "layout" => blocks::parse_layout(self).map(Node::Layout),
            "head" => blocks::parse_head(self).map(Node::Head),
            "blocks" => {
                // `{blocks}` - a void host node. Renders the child blocks of the nearest enclosing
                // section or block, each through its own `blocks/<type>.slurp` file.
                self.advance();
                let close_sp = self.peek_span();
                if !self.eat(&Token::ExprClose) {
                    self.push_error(
                        close_sp,
                        "Expected } to close {blocks}",
                        ErrorCode::UnclosedBlock,
                    );
                }
                Some(Node::Blocks)
            }
            "sections" => {
                // `{sections}` (page list) or `{sections "header"}` (a named global group), a void
                // host node like `{slot}` without fallback.
                self.advance();
                let group = if let Token::StringLiteral(s) = self.peek() {
                    let s = s.clone();
                    self.advance();
                    Some(s)
                } else {
                    None
                };
                let close_sp = self.peek_span();
                if !self.eat(&Token::ExprClose) {
                    self.push_error(
                        close_sp,
                        "Expected } to close {sections}",
                        ErrorCode::UnclosedBlock,
                    );
                }
                Some(Node::Sections(group))
            }
            _ => {
                let sp = self.peek_span();
                self.push_error(
                    sp,
                    format!("Unknown block: {{{name}}}"),
                    ErrorCode::UnexpectedToken,
                );
                self.advance();
                None
            }
        }
    }

    fn parse_client_let(&mut self) -> Option<Node> {
        let sp = self.peek_span();
        self.advance();
        let name = match self.peek().clone() {
            Token::Ident(n) => {
                self.advance();
                n
            }
            other => {
                self.push_error(
                    sp,
                    format!("Expected variable name after {{$let, found {other:?}"),
                    ErrorCode::UnexpectedToken,
                );
                return None;
            }
        };
        let eq_sp = self.peek_span();
        if !self.eat(&Token::Equals) {
            self.push_error(
                eq_sp,
                "Expected = after variable name in {$let",
                ErrorCode::UnexpectedToken,
            );
            return None;
        }
        let value = self.parse_expression()?;
        let close_sp = self.peek_span();
        if !self.eat(&Token::ExprClose) {
            self.push_error(
                close_sp,
                "Expected } to close {$let",
                ErrorCode::UnclosedBlock,
            );
        }
        Some(Node::ClientLet { name, value })
    }

    fn parse_debug(&mut self) -> Option<Node> {
        self.advance();
        let expr = self.parse_expression()?;
        let close_sp = self.peek_span();
        if !self.eat(&Token::ExprClose) {
            self.push_error(
                close_sp,
                "Expected } to close {debug",
                ErrorCode::UnclosedBlock,
            );
        }
        Some(Node::Debug(expr))
    }

    fn parse_redirect(&mut self) -> Option<Node> {
        self.advance();
        let expr = self.parse_expression()?;
        let close_sp = self.peek_span();
        if !self.eat(&Token::ExprClose) {
            self.push_error(
                close_sp,
                "Expected } to close {redirect",
                ErrorCode::UnclosedBlock,
            );
        }
        Some(Node::Redirect(expr))
    }

    pub fn parse_expression(&mut self) -> Option<Expression> {
        expressions::parse_expr(self)
    }
}

pub fn parse(source: &str, file: impl Into<String>) -> CompileResult<(Document, ErrorAccumulator)> {
    let tokens = crate::lexer::Lexer::new(source).tokenize()?;
    let file = file.into();
    let mut parser = Parser::new(tokens, file);
    let doc = parser.parse_document();
    Ok((doc, parser.diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Document {
        let (doc, diag) = parse(src, "test.slurp").expect("lex failed");
        assert!(!diag.has_errors(), "Parse errors: {:?}", diag.errors);
        doc
    }

    #[test]
    fn test_parse_empty() {
        let doc = parse_ok("");
        assert!(doc.children.is_empty());
        assert!(doc.frontmatter.is_none());
    }

    #[test]
    fn test_parse_text() {
        let doc = parse_ok("hello world");
        let text: String = doc
            .children
            .iter()
            .filter_map(|n| {
                if let Node::Text(t) = n {
                    Some(t.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(text.contains("hello"));
    }

    #[test]
    fn test_parse_comment() {
        let doc = parse_ok("{* a comment *}");
        assert!(matches!(doc.children[0], Node::Comment(_)));
    }

    #[test]
    fn test_parse_expression() {
        let doc = parse_ok("${name}");
        assert!(matches!(doc.children[0], Node::Expression(_)));
    }

    #[test]
    fn test_parse_if_block() {
        let doc = parse_ok("{if x}hello{/if}");
        assert!(matches!(doc.children[0], Node::If(_)));
    }

    #[test]
    fn test_parse_if_else() {
        let doc = parse_ok("{if x}a{else}b{/if}");
        if let Node::If(n) = &doc.children[0] {
            assert!(n.else_branch.is_some());
        } else {
            panic!("expected If node");
        }
    }

    #[test]
    fn test_parse_each_block() {
        let doc = parse_ok("{each item in items}${item}{/each}");
        assert!(matches!(doc.children[0], Node::Each(_)));
    }

    #[test]
    fn test_parse_next() {
        let doc = parse_ok("{next}");
        assert!(matches!(doc.children[0], Node::Next));
    }

    #[test]
    fn test_parse_client_let() {
        let doc = parse_ok("{$let count = 0}");
        assert!(matches!(doc.children[0], Node::ClientLet { .. }));
    }

    #[test]
    fn test_parse_raw_html() {
        let doc = parse_ok("{html content}");
        assert!(matches!(
            doc.children[0],
            Node::RawHtml {
                sanitize: false,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_element() {
        let doc = parse_ok("<div>hello</div>");
        assert!(matches!(doc.children[0], Node::Element(_)));
    }

    #[test]
    fn test_parse_sections_node() {
        let doc = parse_ok("<main>{sections}</main>");
        if let Node::Element(el) = &doc.children[0] {
            assert!(
                el.children.iter().any(|n| matches!(n, Node::Sections(_))),
                "{:?}",
                el.children
            );
        } else {
            panic!("expected Element");
        }
    }

    #[test]
    fn test_regression_dollar_sections_still_expression() {
        // `${sections}` must stay a plain path expression - only bare `{sections}`
        // is the host node.
        let doc = parse_ok("${sections}");
        assert!(
            matches!(doc.children[0], Node::Expression(_)),
            "{:?}",
            doc.children
        );
    }

    #[test]
    fn test_regression_each_over_sections_ident() {
        let doc = parse_ok("{each section in items}${section.title}{/each}");
        assert!(matches!(doc.children[0], Node::Each(_)));
    }
}
