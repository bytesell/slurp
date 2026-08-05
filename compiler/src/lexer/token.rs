use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub line: usize,
    pub column: usize,
}

impl Span {
    #[must_use]
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Token {
    FrontmatterDelimiter,
    Using,
    As,
    Type,
    From,
    Props,
    Middleware,

    /// `{if}`, `{each}`, `{match}`, ... - carries the bare name without braces.
    BlockOpen(String),
    /// `{/if}`, `{/each}`, ... - carries the bare name without braces.
    BlockClose(String),
    BlockElse,
    /// `{else if ...}` - condition tokens follow.
    BlockElseIf,
    /// `{empty}` - empty-state branch of each/fetch.
    BlockEmpty,

    ExprOpen,
    /// `}` - closes an expression (shared with block close).
    ExprClose,
    RawHtml,
    RawHtmlSanitize,
    /// `{* ... *}` - template comment; carries inner text.
    Comment(String),

    TemplateLiteralOpen,
    TemplateLiteralClose,
    TemplateLiteralText(String),
    TemplateExprOpen,

    StringLiteral(String),
    NumberLiteral(f64),
    BoolLiteral(bool),
    Null,

    Ident(String),
    Dot,
    DotDotDot,
    Pipe,
    Arrow,
    Comma,
    Colon,
    Equals,
    BangEquals,
    DoubleEquals,
    Bang,
    And,
    Or,
    LessThan,
    GreaterThan,
    LessThanEquals,
    GreaterThanEquals,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Question,
    /// `$` - reactive-variable prefix.
    Dollar,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,

    TagOpen(String),
    TagClose(String),
    TagSelfClose,
    TagEnd,
    AttrName(String),
    AttrEquals,

    /// `on:click`, `on:visible`, ... - carries the event name.
    OnDirective(String),
    NavigateAttr,
    NavigateFalse,
    SlotAttr(String),

    ClientLet,
    Underscore,
    Debug,
    Redirect,
    Next,

    Whitespace(String),
    Newline,
    Eof,

    /// Raw text between Slurp constructs - converted to [`crate::ast::Node::Text`] by the parser.
    Text(String),
}

impl Token {
    #[must_use]
    pub fn is_trivia(&self) -> bool {
        matches!(self, Token::Whitespace(_) | Token::Newline)
    }
}
