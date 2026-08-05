pub mod token;

use token::{Span, Token};

use crate::error::{CompileError, CompileResult};

pub type SpannedToken = (Token, Span);

/// Whether a `{html ...}` opener (the text AFTER `{html`) carries the `sanitize`
/// keyword before its closing `}`. The lexer consumes only `{html` here, so this
/// lookahead must be bounded: scanning to the next `}` (or EOF) would be O(n) per
/// token, making a file of many unterminated `{html` starts O(n^2). A `{html ...}`
/// OPENING tag is short, so we only scan a generous window - large enough never
/// to miss `sanitize` on a real opener, small enough to keep the scan O(1).
fn html_open_has_sanitize(after_html: &str) -> bool {
    const HTML_OPEN_SCAN_LIMIT: usize = 4096;
    // ONE pass that stops at the first `}`, rather than materialising the whole 4096-char window
    // and only then searching it. Materialising costs the full window for every `{html` in the
    // file while `lex_template` consumes just the five bytes `{html`, so an input of nothing but
    // `{html}` pays ~800 char-steps per input byte. Stopping at the first `}` makes the common
    // case one step, and a scan that does run to the limit had to be paid for with 4096 real
    // input bytes, so there is no amplification.
    //
    // KEEP THE RESULT EXACTLY AS IT IS: same limit, same "first `}` within the window", same
    // `contains`. That matters more than the speed. Widening this predicate is XSS-neutral, but
    // NARROWING it silently turns `{html x sanitize}` into the raw form, so any change here needs
    // the two tests in `fuzz_regressions.rs` that pin both forms.
    //
    // `n` counts CHARACTERS, not bytes: the limit is a character count, and a byte index would
    // shorten the window on any non-ASCII opener, which is the narrowing direction.
    for (n, (i, c)) in after_html.char_indices().enumerate() {
        if n >= HTML_OPEN_SCAN_LIMIT {
            return false;
        }
        if c == '}' {
            return after_html[..i].contains("sanitize");
        }
    }
    false
}

/// Names accepted directly after `{` as a BLOCK opener (`{if ...}`, `{each ...}`, ...).
/// Single source of truth: `lex_template`, `lex_raw_script` and `lex_raw_script_text` all
/// read this list, so a new block keyword cannot be taught to one and not the others.
///
/// It is `pub` (via [`block_keywords`]) so the fuzzer builds its token alphabet FROM this
/// table rather than from a copy. A copy is how the `{else` CRITICAL survived: the fuzzer
/// could not generate a keyword it had not been told about, so a keyword added here but
/// missed elsewhere stayed unreachable by generated input. Reading the real table means a
/// new keyword is fuzzed the moment it is added.
pub(crate) const BLOCK_KEYWORDS: &[&str] = &[
    "if", "each", "match", "fetch", "repeat", "try", "slot", "layout", "head", "with", "error",
    "loading", "sections", "blocks",
];

/// The block-opener keyword table, exposed for the fuzzer's alphabet. See [`BLOCK_KEYWORDS`].
#[doc(hidden)]
pub fn block_keywords() -> &'static [&'static str] {
    BLOCK_KEYWORDS
}

/// Every `{`-prefixed construct the raw-text scanners must stop before, other than the
/// [`BLOCK_KEYWORDS`] openers.
///
/// [`raw_brace_starts_slurp`] READS this table rather than repeating it, so the predicate and
/// the published list cannot drift; the test `raw_openers_are_all_recognised` pins that, and
/// pins that nothing outside this table plus the keyword set is accepted. Re-inlining any of
/// these into the predicate reintroduces exactly the drift that caused the `{else` lexer hang.
pub(crate) const RAW_BRACE_OPENERS: &[&str] = &[
    "{*",
    "{/",
    "{$let",
    "{else if",
    "{else}",
    "{empty}",
    "{next}",
    "{html",
    "{debug",
    "{redirect",
];

/// The raw-brace opener table, exposed for the fuzzer's alphabet. See [`RAW_BRACE_OPENERS`].
#[doc(hidden)]
pub fn raw_brace_openers() -> &'static [&'static str] {
    RAW_BRACE_OPENERS
}

/// Tag names whose bodies the lexer treats as RAW TEXT, paired with the mode each enters.
/// SINGLE source of truth: `tokenize` reads this to decide both when to ENTER a raw mode and
/// which close tag LEAVES it, so the enter-set and the leave-set cannot drift apart. They were
/// two separate literal matches before; that is the same shape as the `{else` drift bug.
fn raw_text_mode_for(tag: &str) -> Option<LexMode> {
    RAW_TEXT_TAGS
        .iter()
        .find(|(name, _)| *name == tag)
        .map(|(_, m)| m.clone())
}

/// The one table. `raw_text_mode_for` reads it forwards, `LexMode::raw_tag` backwards.
const RAW_TEXT_TAGS: &[(&str, LexMode)] =
    &[("script", LexMode::RawScript), ("style", LexMode::RawStyle)];

/// The identifier-shaped run directly after a `{` (used to test for a block keyword).
fn brace_ident(after_brace: &str) -> String {
    after_brace
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

fn is_block_keyword(name: &str) -> bool {
    BLOCK_KEYWORDS.contains(&name)
}

/// SINGLE source of truth for "the text at `rest` (which starts with `{`) opens a Slurp
/// construct". `lex_raw_script` CONSUMES exactly this set; `lex_raw_script_text` must STOP
/// before exactly this set.
///
/// The two used to carry independent hand-written tables and they drifted: the consumer
/// matched only the exact strings `{else if` and `{else}` while the text scanner broke on the
/// PREFIX `{else`. Ordinary JavaScript such as `{elseBranch: 1}` therefore hit neither - the
/// text scanner returned `Text("")` without advancing `self.pos`, and `tokenize` re-entered
/// forever, pushing an empty token each pass until the allocator aborted. Keep them reading
/// this one predicate; do not re-inline either table.
///
/// The construct set itself lives in exactly two tables - [`RAW_BRACE_OPENERS`] and
/// [`BLOCK_KEYWORDS`] - which this reads. Writing a literal `starts_with` here instead of
/// extending the table is what recreates the drift, so do not.
fn raw_brace_starts_slurp(rest: &str) -> bool {
    RAW_BRACE_OPENERS.iter().any(|o| rest.starts_with(o))
        || rest
            .strip_prefix('{')
            .is_some_and(|after| is_block_keyword(&brace_ident(after)))
}

#[derive(Debug, Clone, PartialEq)]
enum LexMode {
    Template,
    /// Content INSIDE a `{match}` block. Same as `Template`, except bare literals (string / number /
    /// bool / `->` / `_`) are tokenized so the parser can read match-arm patterns. In plain `Template`
    /// those characters are literal text - otherwise a prose apostrophe ("we'll") would be lexed as an
    /// unterminated string literal.
    Match,
    Expr,
    TemplateLiteral,
    HtmlTag,
    Frontmatter,
    RawScript,
    RawStyle,
}

impl LexMode {
    /// The tag name that opens (and therefore closes) this raw-text mode. Panics only on a
    /// non-raw mode, which is a programming error rather than an input-reachable state.
    fn raw_tag(&self) -> &'static str {
        RAW_TEXT_TAGS
            .iter()
            .find(|(_, m)| m == self)
            .map(|(name, _)| *name)
            .expect("raw_tag called on a non raw-text mode")
    }
}

/// Ceiling on the number of tokens one source file may produce.
///
/// SECURITY (memory): a byte that starts no construct becomes its own `Token::Text` of one
/// character, and a `SpannedToken` costs roughly 70 bytes once the `String` allocation and the
/// `Span` are counted. A file of `<` therefore expands about 70x into resident memory with
/// nothing to stop it, so an untrusted template a few tens of MB long could exhaust a shared
/// service and take every other tenant's render down with it. A host's upload path will normally
/// reject files far smaller than this, so no realistic theme can reach it: a large hand-written
/// page lexes to a few thousand tokens.
const MAX_TOKENS: usize = 2_000_000;

/// Ceiling on lexer mode nesting (`${`, backticks, tags, `{if}` and friends each push a mode).
///
/// The parser has its own `MAX_DEPTH`, but it only applies once the whole token stream has been
/// built. Refusing here means a 600 KB file of `${\`` costs a bounded amount of work instead of
/// a multi-million-token stream that the parser then walks only to reject. Set far above any
/// nesting a human or a generator would write.
const MAX_MODE_DEPTH: usize = 512;

pub struct Lexer<'src> {
    src: &'src str,
    pos: usize,
    line: usize,
    col: usize,
}

impl<'src> Lexer<'src> {
    pub fn new(src: &'src str) -> Self {
        Self {
            src,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    #[must_use = "lexer result must be checked for errors"]
    pub fn tokenize(mut self) -> CompileResult<Vec<SpannedToken>> {
        let mut tokens = Vec::new();
        let mut mode_stack: Vec<LexMode> = vec![LexMode::Template];
        let mut in_frontmatter = false;
        let mut template_expr_depth: usize = 0;

        loop {
            if self.pos >= self.src.len() {
                tokens.push((Token::Eof, self.span()));
                break;
            }

            // SECURITY (memory): see `MAX_TOKENS`. Checked before the branch that would push,
            // so the ceiling is on what we have already allocated.
            if tokens.len() >= MAX_TOKENS {
                return Err(CompileError::lex(
                    "unknown",
                    self.line,
                    self.col,
                    format!("Template is too complex: more than {MAX_TOKENS} tokens"),
                ));
            }
            // SECURITY (memory + parser depth): see `MAX_MODE_DEPTH`.
            if mode_stack.len() >= MAX_MODE_DEPTH {
                return Err(CompileError::lex(
                    "unknown",
                    self.line,
                    self.col,
                    format!("Template nesting is too deep (limit {MAX_MODE_DEPTH})"),
                ));
            }

            let mode = mode_stack.last().cloned().unwrap_or(LexMode::Template);
            // SECURITY: forward-progress invariant, see the check after the match. Every
            // lexer branch below must consume at least one byte; a branch that does not
            // turns this loop into an infinite loop that allocates a token per pass.
            let pos_before = self.pos;
            let tok = match mode {
                LexMode::Frontmatter => self.lex_frontmatter()?,
                // `literals` is true only inside a {match} block, where bare string/number/bool/`->`/`_`
                // are match-arm syntax. In ordinary template content they are plain text.
                LexMode::Template => self.lex_template(false)?,
                LexMode::Match => self.lex_template(true)?,
                LexMode::HtmlTag => self.lex_html_tag()?,
                LexMode::Expr => self.lex_expr()?,
                LexMode::TemplateLiteral => self.lex_template_literal()?,
                LexMode::RawScript => self.lex_raw_script()?,
                // The close tag is derived from the mode, not written a third time.
                LexMode::RawStyle => self.lex_raw_text_until(LexMode::RawStyle.raw_tag())?,
            };

            // SECURITY: the durable guard against a lexer branch that emits a token without
            // consuming input. `pos >= len` already broke out above, so at this point the
            // input is non-empty and any token that left `pos` untouched means the loop
            // would spin forever. Fail with a clean error instead of hanging the process -
            // this compiler runs on untrusted templates in a shared service.
            //
            // `Token::Eof` is deliberately NOT exempt. Three helpers can return it
            // (`lex_frontmatter`, `lex_html_tag`, `lex_expr`), but all three do so only when
            // `peek()` is `None`, i.e. `pos >= len` - which the check at the top of the loop
            // already handled, so any `Eof` reaching this point has necessarily advanced (for
            // example `lex_html_tag` consuming the trailing space of `"<a "`). An exemption
            // would therefore be dead code today while silently un-guarding tomorrow: a future
            // helper that returned `Eof` mid-input would spin with no diagnostic.
            if self.pos == pos_before {
                return Err(CompileError::lex(
                    "unknown",
                    self.line,
                    self.col,
                    "Internal lexer error: no progress made while tokenizing",
                ));
            }

            match &tok.0 {
                Token::FrontmatterDelimiter => {
                    if !in_frontmatter {
                        in_frontmatter = true;
                        mode_stack.push(LexMode::Frontmatter);
                    } else {
                        if mode_stack.last() == Some(&LexMode::Frontmatter) {
                            mode_stack.pop();
                        }
                        in_frontmatter = false;
                    }
                }
                Token::TagOpen(_) => {
                    mode_stack.push(LexMode::HtmlTag);
                }
                Token::TagEnd => {
                    if mode_stack.last() == Some(&LexMode::HtmlTag) {
                        mode_stack.pop();
                    }
                    // After closing a <script> or <style> tag, switch to raw text mode
                    // so JS/CSS isn't parsed as Slurp template syntax.
                    let last_tag = tokens.iter().rev().find_map(|(t, _)| {
                        if let Token::TagOpen(name) = t {
                            Some(name.clone())
                        } else {
                            None
                        }
                    });
                    if let Some(m) = last_tag.as_deref().and_then(raw_text_mode_for) {
                        mode_stack.push(m);
                    }
                }
                Token::TagSelfClose => {
                    if mode_stack.last() == Some(&LexMode::HtmlTag) {
                        mode_stack.pop();
                    }
                }
                Token::TagClose(name) => {
                    // Pop the mode a close tag ends: an HTML tag, or the raw-text mode of a
                    // <script>/<style> we entered on its matching open tag.
                    // The leave-set is DERIVED from the same table as the enter-set above:
                    // a close tag ends a raw mode exactly when that tag is the one that
                    // would have entered it. Writing the two sets independently is the
                    // drift shape that produced the `{else` hang.
                    let closes_mode = match mode_stack.last() {
                        Some(&LexMode::HtmlTag) => true,
                        Some(m @ (&LexMode::RawScript | &LexMode::RawStyle)) => {
                            raw_text_mode_for(name).as_ref() == Some(m)
                        }
                        _ => false,
                    };
                    if closes_mode {
                        mode_stack.pop();
                    }
                }
                Token::ExprOpen | Token::LBrace => {
                    // Braces inside frontmatter delimit type-import lists, not expressions.
                    if mode != LexMode::Frontmatter {
                        mode_stack.push(LexMode::Expr);
                    }
                }
                Token::BlockOpen(name) => {
                    // The {match} BODY is lexed in Match mode (literals on, for arm patterns). Push it
                    // UNDER the header expression's Expr mode, so when the header's `}` pops Expr we land
                    // in Match and stay there until {/match}.
                    if name == "match" {
                        mode_stack.push(LexMode::Match);
                    }
                    mode_stack.push(LexMode::Expr);
                }
                Token::BlockElseIf
                | Token::RawHtml
                | Token::RawHtmlSanitize
                | Token::ClientLet
                | Token::Debug
                | Token::Redirect => {
                    mode_stack.push(LexMode::Expr);
                }
                Token::BlockClose(name) => {
                    if name == "match" && mode_stack.last() == Some(&LexMode::Match) {
                        mode_stack.pop();
                    }
                }
                Token::BlockElse | Token::BlockEmpty | Token::Next => {}
                Token::ExprClose | Token::RBrace => {
                    if mode_stack.last() == Some(&LexMode::Expr) {
                        mode_stack.pop();
                    }
                    if template_expr_depth > 0 {
                        template_expr_depth -= 1;
                        if mode_stack.last() != Some(&LexMode::TemplateLiteral) {
                            mode_stack.push(LexMode::TemplateLiteral);
                        }
                    }
                }
                Token::TemplateLiteralOpen => {
                    mode_stack.push(LexMode::TemplateLiteral);
                }
                Token::TemplateLiteralClose => {
                    if mode_stack.last() == Some(&LexMode::TemplateLiteral) {
                        mode_stack.pop();
                    }
                }
                Token::TemplateExprOpen => {
                    template_expr_depth += 1;
                    mode_stack.push(LexMode::Expr);
                }
                _ => {}
            }

            tokens.push(tok);
        }

        Ok(tokens)
    }

    fn lex_template(&mut self, literals: bool) -> CompileResult<SpannedToken> {
        let sp = self.span();

        if self.peek_str("---") && self.col == 1 {
            self.advance_by_str("---");
            self.eat_line_remainder(); // eat trailing spaces on the same line
            return Ok((Token::FrontmatterDelimiter, sp));
        }

        if let Some(t) = self.try_lex_newline(sp) {
            return Ok(t);
        }

        if self.peek().map(|c| c == ' ' || c == '\t').unwrap_or(false) {
            return Ok(self.lex_whitespace(sp));
        }

        let rest = &self.src[self.pos..];

        if rest.starts_with("{*") {
            return self.lex_comment(sp);
        }

        if rest.starts_with("{/") {
            return self.lex_block_close(sp);
        }

        if rest.starts_with("{$let") {
            self.advance_by_str("{$let");
            return Ok((Token::ClientLet, sp));
        }

        if rest.starts_with("{else if") {
            self.advance_by_str("{else if");
            return Ok((Token::BlockElseIf, sp));
        }

        if rest.starts_with("{else}") {
            self.advance_by_str("{else}");
            return Ok((Token::BlockElse, sp));
        }

        if rest.starts_with("{empty}") {
            self.advance_by_str("{empty}");
            return Ok((Token::BlockEmpty, sp));
        }

        if rest.starts_with("{next}") {
            self.advance_by_str("{next}");
            return Ok((Token::Next, sp));
        }

        // Peek ahead to determine if "sanitize" appears before } - sets the right token.
        if rest.starts_with("{html") {
            let after_html = rest.strip_prefix("{html").unwrap_or_default();
            let has_sanitize = html_open_has_sanitize(after_html);
            self.advance_by_str("{html");
            if has_sanitize {
                return Ok((Token::RawHtmlSanitize, sp));
            }
            return Ok((Token::RawHtml, sp));
        }

        if rest.starts_with("{debug") {
            self.advance_by_str("{debug");
            return Ok((Token::Debug, sp));
        }

        if rest.starts_with("{redirect") {
            self.advance_by_str("{redirect");
            return Ok((Token::Redirect, sp));
        }

        if rest.starts_with('{') {
            let after_brace = rest.strip_prefix('{').unwrap_or_default();
            let name = brace_ident(after_brace);

            if is_block_keyword(&name) {
                self.advance(); // {
                for _ in 0..name.chars().count() {
                    self.advance(); // name chars
                }
                // Do NOT consume the trailing } - it will be emitted as ExprClose
                // from within the Expr mode that we push after returning this token.
                return Ok((Token::BlockOpen(name), sp));
            }

            // ${  - expression open
            self.advance(); // {
            return Ok((Token::LBrace, sp));
        }

        if rest.starts_with("${") {
            self.advance(); // $
            self.advance(); // {
            return Ok((Token::ExprOpen, sp));
        }

        if self.peek() == Some('`') {
            self.advance();
            return Ok((Token::TemplateLiteralOpen, sp));
        }

        if rest.starts_with("</") {
            self.advance(); // <
            self.advance(); // /
            let name = self.read_tag_name();
            self.skip_inline_ws();
            if self.peek() == Some('>') {
                self.advance();
            }
            return Ok((Token::TagClose(name), sp));
        }

        if self.peek() == Some('<') {
            let after_lt = &rest[1..];
            // Must be followed by a letter or uppercase (component) or lowercase (element)
            if after_lt.starts_with(|c: char| c.is_alphabetic()) {
                self.advance(); // <
                let name = self.read_tag_name();
                return Ok((Token::TagOpen(name), sp));
            }
            // HTML comment <!-- ... --> - consume it whole and emit as literal text.
            if after_lt.starts_with("!--") {
                let comment_end = rest.find("-->").map(|i| i + 3).unwrap_or(rest.len());
                let comment = rest[..comment_end].to_string();
                self.advance_by_str(&comment);
                return Ok((Token::Text(comment), sp));
            }
            // Bare < not followed by alphabetic (e.g. <!DOCTYPE ...) - emit as literal text
            // so lex_text() is never called on this character (it breaks on < without advancing).
            self.advance();
            return Ok((Token::Text("<".to_string()), sp));
        }

        // Literals (string / number / `->` / `_` / true|false|null) are match-arm SYNTAX and are only
        // tokenized inside a {match} block. In ordinary template content they are plain text - so prose
        // like "we'll" or "$5 off" or the word "false" does not get mis-lexed (a lone apostrophe used to
        // start an unterminated string literal here).
        if literals {
            if self.peek() == Some('"') || self.peek() == Some('\'') {
                let q = self.peek().unwrap();
                let s = self.read_string(q)?;
                return Ok((Token::StringLiteral(s), sp));
            }

            if self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                let n = self.read_number()?;
                return Ok((Token::NumberLiteral(n), sp));
            }

            if self.peek_str("->") {
                self.advance_by_str("->");
                return Ok((Token::Arrow, sp));
            }

            if self.peek() == Some('_')
                && self
                    .peek2()
                    .map(|c| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(true)
            {
                self.advance();
                return Ok((Token::Underscore, sp));
            }

            // Emitted as structured tokens so they can be used as match arm patterns;
            // parse_node() converts them back to text nodes in non-pattern contexts.
            for (keyword, tok) in &[
                ("true", Token::BoolLiteral(true)),
                ("false", Token::BoolLiteral(false)),
                ("null", Token::Null),
            ] {
                if self.peek_str(keyword) {
                    let after = self.src[self.pos + keyword.len()..].chars().next();
                    // Only emit as keyword if followed by non-alphanumeric (word boundary)
                    if after
                        .map(|c| !c.is_alphanumeric() && c != '_')
                        .unwrap_or(true)
                    {
                        self.advance_by_str(keyword);
                        return Ok((tok.clone(), sp));
                    }
                }
            }
        }

        self.lex_text(sp, literals)
    }

    fn lex_frontmatter(&mut self) -> CompileResult<SpannedToken> {
        let sp = self.span();

        if self.peek_str("---") && self.col == 1 {
            self.advance_by_str("---");
            self.eat_line_remainder();
            return Ok((Token::FrontmatterDelimiter, sp));
        }

        if let Some(t) = self.try_lex_newline(sp) {
            return Ok(t);
        }

        if self.peek().map(|c| c == ' ' || c == '\t').unwrap_or(false) {
            return Ok(self.lex_whitespace(sp));
        }

        // `{* ... *}` is a comment in frontmatter too. Without this the comment BODY is
        // lexed as frontmatter tokens, so prose containing the word "props" (or "using",
        // "type", ...) is read as a real declaration and the file fails to compile.
        if self.peek_str("{*") {
            return self.lex_comment(sp);
        }

        match self.peek() {
            None => Ok((Token::Eof, sp)),
            Some('"') | Some('\'') => {
                let q = self.peek().unwrap();
                let s = self.read_string(q)?;
                Ok((Token::StringLiteral(s), sp))
            }
            Some('0'..='9') => {
                let n = self.read_number()?;
                Ok((Token::NumberLiteral(n), sp))
            }
            Some(':') => {
                self.advance();
                Ok((Token::Colon, sp))
            }
            Some(',') => {
                self.advance();
                Ok((Token::Comma, sp))
            }
            Some('{') => {
                self.advance();
                Ok((Token::LBrace, sp))
            }
            Some('}') => {
                self.advance();
                Ok((Token::RBrace, sp))
            }
            Some('[') => {
                self.advance();
                Ok((Token::LBracket, sp))
            }
            Some(']') => {
                self.advance();
                Ok((Token::RBracket, sp))
            }
            Some('(') => {
                self.advance();
                Ok((Token::LParen, sp))
            }
            Some(')') => {
                self.advance();
                Ok((Token::RParen, sp))
            }
            Some('?') => {
                self.advance();
                Ok((Token::Question, sp))
            }
            Some('=') => {
                self.advance();
                Ok((Token::Equals, sp))
            }
            Some('-') => {
                self.advance();
                Ok((Token::Minus, sp))
            }
            Some('@') | Some('_') | Some('a'..='z') | Some('A'..='Z') => {
                let ident = self.read_frontmatter_ident();
                Ok((self.keyword_or_ident(ident), sp))
            }
            Some(c) => {
                self.advance();
                // Unknown characters in frontmatter are treated as whitespace so
                // YAML-style comments and other content don't break the lexer.
                Ok((Token::Whitespace(c.to_string()), sp))
            }
        }
    }

    fn lex_html_tag(&mut self) -> CompileResult<SpannedToken> {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
                self.advance();
            } else {
                break;
            }
        }

        let sp = self.span();

        match self.peek() {
            None => Ok((Token::Eof, sp)),
            Some('/') if self.peek2() == Some('>') => {
                self.advance();
                self.advance();
                Ok((Token::TagSelfClose, sp))
            }
            Some('>') => {
                self.advance();
                Ok((Token::TagEnd, sp))
            }
            Some('=') => {
                self.advance();
                Ok((Token::AttrEquals, sp))
            }
            Some('"') | Some('\'') => {
                let q = self.peek().unwrap();
                let s = self.read_string(q)?;
                Ok((Token::StringLiteral(s), sp))
            }
            Some('{') => {
                self.advance();
                Ok((Token::LBrace, sp))
            }
            Some('`') => {
                self.advance();
                Ok((Token::TemplateLiteralOpen, sp))
            }
            Some(c) if c.is_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '@' => {
                let name = self.read_attr_name();

                if let Some(event) = name.strip_prefix("on:") {
                    return Ok((Token::OnDirective(event.to_string()), sp));
                }

                if name == "slot" {
                    return Ok((Token::AttrName("slot".to_string()), sp));
                }

                if name == "navigate" {
                    return Ok((Token::NavigateAttr, sp));
                }

                Ok((Token::AttrName(name), sp))
            }
            Some('.') if self.peek_str("...") => {
                self.advance_by_str("...");
                Ok((Token::DotDotDot, sp))
            }
            Some(c) => {
                self.advance();
                Err(CompileError::lex(
                    "unknown",
                    self.line,
                    self.col,
                    format!("Unexpected character in HTML tag: {c:?}"),
                ))
            }
        }
    }

    fn lex_expr(&mut self) -> CompileResult<SpannedToken> {
        while self.peek().map(|c| c == ' ' || c == '\t').unwrap_or(false) {
            self.advance();
        }
        let sp = self.span();

        if let Some(t) = self.try_lex_newline(sp) {
            return Ok(t);
        }

        match self.peek() {
            None => Ok((Token::Eof, sp)),
            Some('}') => {
                self.advance();
                Ok((Token::ExprClose, sp))
            }
            Some('|') if self.peek2() == Some('|') => {
                self.advance_by_str("||");
                Ok((Token::Or, sp))
            }
            Some('|') => {
                self.advance();
                Ok((Token::Pipe, sp))
            }
            Some('&') if self.peek2() == Some('&') => {
                self.advance_by_str("&&");
                Ok((Token::And, sp))
            }
            Some('!') if self.peek2() == Some('=') => {
                self.advance_by_str("!=");
                Ok((Token::BangEquals, sp))
            }
            Some('!') => {
                self.advance();
                Ok((Token::Bang, sp))
            }
            Some('=') if self.peek2() == Some('=') => {
                self.advance_by_str("==");
                Ok((Token::DoubleEquals, sp))
            }
            Some('=') => {
                self.advance();
                Ok((Token::Equals, sp))
            }
            Some('<') if self.peek2() == Some('=') => {
                self.advance_by_str("<=");
                Ok((Token::LessThanEquals, sp))
            }
            Some('<') => {
                self.advance();
                Ok((Token::LessThan, sp))
            }
            Some('>') if self.peek2() == Some('=') => {
                self.advance_by_str(">=");
                Ok((Token::GreaterThanEquals, sp))
            }
            Some('>') => {
                self.advance();
                Ok((Token::GreaterThan, sp))
            }
            Some('+') => {
                self.advance();
                Ok((Token::Plus, sp))
            }
            Some('-') if self.peek2() == Some('>') => {
                self.advance_by_str("->");
                Ok((Token::Arrow, sp))
            }
            Some('-') => {
                self.advance();
                Ok((Token::Minus, sp))
            }
            Some('*') => {
                self.advance();
                Ok((Token::Star, sp))
            }
            Some('/') => {
                self.advance();
                Ok((Token::Slash, sp))
            }
            Some('%') => {
                self.advance();
                Ok((Token::Percent, sp))
            }
            Some('?') => {
                self.advance();
                Ok((Token::Question, sp))
            }
            Some(':') => {
                self.advance();
                Ok((Token::Colon, sp))
            }
            Some(',') => {
                self.advance();
                Ok((Token::Comma, sp))
            }
            Some('.') if self.peek_str("...") => {
                self.advance_by_str("...");
                Ok((Token::DotDotDot, sp))
            }
            Some('.') => {
                self.advance();
                Ok((Token::Dot, sp))
            }
            Some('[') => {
                self.advance();
                Ok((Token::LBracket, sp))
            }
            Some(']') => {
                self.advance();
                Ok((Token::RBracket, sp))
            }
            Some('(') => {
                self.advance();
                Ok((Token::LParen, sp))
            }
            Some(')') => {
                self.advance();
                Ok((Token::RParen, sp))
            }
            Some('$') if self.peek2() == Some('{') => {
                self.advance_by_str("${");
                Ok((Token::TemplateExprOpen, sp))
            }
            Some('$') => {
                self.advance();
                Ok((Token::Dollar, sp))
            }
            Some('`') => {
                self.advance();
                Ok((Token::TemplateLiteralOpen, sp))
            }
            Some('"') | Some('\'') => {
                let q = self.peek().unwrap();
                let s = self.read_string(q)?;
                Ok((Token::StringLiteral(s), sp))
            }
            Some('0'..='9') => {
                let n = self.read_number()?;
                Ok((Token::NumberLiteral(n), sp))
            }
            Some('_')
                if self
                    .peek2()
                    .map(|c| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(true) =>
            {
                self.advance();
                Ok((Token::Underscore, sp))
            }
            Some(c) if c.is_alphabetic() || c == '_' => {
                let ident = self.read_ident();
                Ok((self.expr_keyword_or_ident(ident), sp))
            }
            Some(c) => {
                self.advance();
                Err(CompileError::lex(
                    "unknown",
                    self.line,
                    self.col,
                    format!("Unexpected character in expression: {c:?}"),
                ))
            }
        }
    }

    fn lex_template_literal(&mut self) -> CompileResult<SpannedToken> {
        let sp = self.span();

        match self.peek() {
            None => {
                return Err(CompileError::lex(
                    "unknown",
                    self.line,
                    self.col,
                    "Unterminated template literal",
                ));
            }
            Some('`') => {
                self.advance();
                return Ok((Token::TemplateLiteralClose, sp));
            }
            Some('$') if self.peek2() == Some('{') => {
                self.advance_by_str("${");
                return Ok((Token::TemplateExprOpen, sp));
            }
            _ => {}
        }

        let mut text = String::new();
        loop {
            match self.peek() {
                None | Some('`') => break,
                Some('$') if self.peek2() == Some('{') => break,
                Some('\\') => {
                    self.advance(); // backslash
                    match self.advance() {
                        Some('n') => text.push('\n'),
                        Some('t') => text.push('\t'),
                        Some('r') => text.push('\r'),
                        Some('\\') => text.push('\\'),
                        Some('`') => text.push('`'),
                        Some('$') => text.push('$'),
                        Some(c) => {
                            text.push('\\');
                            text.push(c);
                        }
                        None => break,
                    }
                }
                Some(c) => {
                    text.push(c);
                    self.advance();
                }
            }
        }
        Ok((Token::TemplateLiteralText(text), sp))
    }

    /// Raw text mode for `<script>` - like template mode but bare `{` is literal text,
    /// not a Slurp expression start. `${...}` and `{if}` blocks still work.
    fn lex_raw_script(&mut self) -> CompileResult<SpannedToken> {
        let sp = self.span();

        if let Some(t) = self.try_lex_newline(sp) {
            return Ok(t);
        }
        if self.peek().map(|c| c == ' ' || c == '\t').unwrap_or(false) {
            return Ok(self.lex_whitespace(sp));
        }

        let rest = &self.src[self.pos..];

        if rest.starts_with("{*") {
            return self.lex_comment(sp);
        }
        if rest.starts_with("{/") {
            return self.lex_block_close(sp);
        }
        if rest.starts_with("{$let") {
            self.advance_by_str("{$let");
            return Ok((Token::ClientLet, sp));
        }
        if rest.starts_with("{else if") {
            self.advance_by_str("{else if");
            return Ok((Token::BlockElseIf, sp));
        }
        if rest.starts_with("{else}") {
            self.advance_by_str("{else}");
            return Ok((Token::BlockElse, sp));
        }
        if rest.starts_with("{empty}") {
            self.advance_by_str("{empty}");
            return Ok((Token::BlockEmpty, sp));
        }
        if rest.starts_with("{next}") {
            self.advance_by_str("{next}");
            return Ok((Token::Next, sp));
        }

        if rest.starts_with("{html") {
            let after_html = rest.strip_prefix("{html").unwrap_or_default();
            let has_sanitize = html_open_has_sanitize(after_html);
            self.advance_by_str("{html");
            if has_sanitize {
                return Ok((Token::RawHtmlSanitize, sp));
            }
            return Ok((Token::RawHtml, sp));
        }

        if rest.starts_with("{debug") {
            self.advance_by_str("{debug");
            return Ok((Token::Debug, sp));
        }
        if rest.starts_with("{redirect") {
            self.advance_by_str("{redirect");
            return Ok((Token::Redirect, sp));
        }

        if let Some(after_brace) = rest.strip_prefix('{') {
            let name = brace_ident(after_brace);
            if is_block_keyword(&name) {
                self.advance(); // {
                for _ in 0..name.chars().count() {
                    self.advance();
                }
                return Ok((Token::BlockOpen(name), sp));
            }
            // Bare `{` not a Slurp keyword - emit as raw text and continue reading.
            return self.lex_raw_script_text(sp);
        }

        if rest.starts_with("${") {
            self.advance(); // $
            self.advance(); // {
            return Ok((Token::ExprOpen, sp));
        }

        if rest.starts_with("</") {
            self.advance(); // <
            self.advance(); // /
            let name = self.read_tag_name();
            self.skip_inline_ws();
            if self.peek() == Some('>') {
                self.advance();
            }
            return Ok((Token::TagClose(name), sp));
        }

        self.lex_raw_script_text(sp)
    }

    fn lex_raw_script_text(&mut self, sp: Span) -> CompileResult<SpannedToken> {
        let mut text = String::new();
        loop {
            let rest = &self.src[self.pos..];
            if rest.is_empty() {
                break;
            }
            if rest.starts_with("${") || rest.starts_with("</") {
                break;
            }
            match self.peek() {
                Some('{') => {
                    // Stop only on what `lex_raw_script` will actually consume. Using a
                    // wider test here (it once broke on the bare prefix `{else`) strands
                    // the lexer: this returns Text("") and nothing advances `self.pos`.
                    if raw_brace_starts_slurp(rest) {
                        break;
                    }
                    text.push('{');
                    self.advance();
                }
                Some('\n') | Some('\r') | Some(' ') | Some('\t') => break,
                Some(c) => {
                    text.push(c);
                    self.advance();
                }
                None => break,
            }
        }
        Ok((Token::Text(text), sp))
    }

    /// Raw text mode for `<style>` - reads everything until `</style>` as literal text.
    fn lex_raw_text_until(&mut self, close_tag: &str) -> CompileResult<SpannedToken> {
        let sp = self.span();
        let close_str = format!("</{close_tag}>");
        let rest = &self.src[self.pos..];
        if rest.starts_with(&close_str) {
            // CHARS, not bytes. `advance()` consumes one CHARACTER, so counting `close_str.len()`
            // (a byte count) over-advances the moment a raw-text tag name is not pure ASCII.
            // Correct today only because `RAW_TEXT_TAGS` holds `script` and `style`; the two
            // sibling loops in `lex_template` and `lex_raw_script` already use `.chars().count()`,
            // and this one being different was the drift.
            for _ in 0..close_str.chars().count() {
                self.advance();
            }
            return Ok((Token::TagClose(close_tag.to_string()), sp));
        }
        // Handle newlines and whitespace before the text loop to avoid returning empty
        // Token::Text("") without advancing (which causes an infinite tokenize loop).
        if let Some(t) = self.try_lex_newline(sp) {
            return Ok(t);
        }
        if self.peek().map(|c| c == ' ' || c == '\t').unwrap_or(false) {
            return Ok(self.lex_whitespace(sp));
        }
        // Read non-whitespace chars until the closing tag
        let mut text = String::new();
        loop {
            let rest = &self.src[self.pos..];
            if rest.is_empty() || rest.starts_with(&close_str) {
                break;
            }
            match self.peek() {
                Some('\n') | Some('\r') => break,
                Some(' ') | Some('\t') => break,
                Some(c) => {
                    text.push(c);
                    self.advance();
                }
                None => break,
            }
        }
        Ok((Token::Text(text), sp))
    }

    fn span(&self) -> Span {
        Span::new(self.line, self.col)
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek2(&self) -> Option<char> {
        let mut it = self.src[self.pos..].chars();
        it.next();
        it.next()
    }

    fn peek_str(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.src[self.pos..].chars().next()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn advance_by_str(&mut self, s: &str) {
        for _ in s.chars() {
            self.advance();
        }
    }

    fn try_lex_newline(&mut self, sp: Span) -> Option<SpannedToken> {
        if self.peek_str("\r\n") {
            self.advance();
            self.advance();
            return Some((Token::Newline, sp));
        }
        if self.peek().map(|c| c == '\n' || c == '\r').unwrap_or(false) {
            self.advance();
            return Some((Token::Newline, sp));
        }
        None
    }

    fn lex_whitespace(&mut self, sp: Span) -> SpannedToken {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        (Token::Whitespace(s), sp)
    }

    // `literals` mirrors lex_template: only inside a {match} block do quotes / digits / `->` / `_` end a
    // text run (so the following match-arm literal can be tokenized). In ordinary template content they
    // are part of the text - otherwise an apostrophe or digit in prose would be split off and mis-lexed.
    fn lex_text(&mut self, sp: Span, literals: bool) -> CompileResult<SpannedToken> {
        let mut text = String::new();
        loop {
            match self.peek() {
                None => break,
                // Structural boundaries
                Some('{') | Some('<') | Some('`') => break,
                Some('$') if self.peek2() == Some('{') => break,
                Some('\n') | Some('\r') => break,
                Some(' ') | Some('\t') => break,
                // String delimiters - stop so lex_template can emit StringLiteral (match arms only)
                Some('"') | Some('\'') if literals => break,
                // Digits - stop so lex_template can emit NumberLiteral (match arms only)
                Some('0'..='9') if literals => break,
                // Arrow operator (match arms only)
                Some('-') if literals && self.peek2() == Some('>') => break,
                // Underscore wildcard - standalone, not part of an identifier (match arms only)
                Some('_')
                    if literals
                        && self
                            .peek2()
                            .map(|c| !c.is_alphanumeric() && c != '_')
                            .unwrap_or(true) =>
                {
                    break;
                }
                Some(c) => {
                    text.push(c);
                    self.advance();
                }
            }
        }
        Ok((Token::Text(text), sp))
    }

    fn lex_comment(&mut self, sp: Span) -> CompileResult<SpannedToken> {
        self.advance_by_str("{*");
        let mut content = String::new();
        loop {
            if self.pos >= self.src.len() {
                return Err(CompileError::lex(
                    "unknown",
                    self.line,
                    self.col,
                    "Unclosed comment block - expected *}",
                ));
            }
            if self.peek_str("*}") {
                self.advance_by_str("*}");
                break;
            }
            if let Some(c) = self.advance() {
                content.push(c);
            }
        }
        Ok((Token::Comment(content), sp))
    }

    fn lex_block_close(&mut self, sp: Span) -> CompileResult<SpannedToken> {
        self.advance_by_str("{/");
        let name = self.read_ident();
        self.skip_inline_ws();
        if self.peek() == Some('}') {
            self.advance();
        }
        Ok((Token::BlockClose(name), sp))
    }

    fn read_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn read_frontmatter_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '@' || c == '.' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn read_tag_name(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn read_attr_name(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '@' || c == '.' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn read_string(&mut self, quote: char) -> CompileResult<String> {
        self.advance();
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    return Err(CompileError::lex(
                        "unknown",
                        self.line,
                        self.col,
                        "Unterminated string literal",
                    ));
                }
                Some(c) if c == quote => break,
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some('\'') => s.push('\''),
                    Some('"') => s.push('"'),
                    Some('0') => s.push('\0'),
                    Some('u') => {
                        // Validate BEFORE consuming: a short escape such as `"\u00"` used to
                        // eat the closing quote (and whatever followed it) as if it were hex,
                        // then report "Unterminated string literal" somewhere far away.
                        let hex: String = self.src[self.pos..].chars().take(4).collect();
                        if hex.chars().count() < 4 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                            return Err(CompileError::lex(
                                "unknown",
                                self.line,
                                self.col,
                                format!(
                                    "Invalid unicode escape: \\u must be followed by exactly 4 hex digits, found \"{}\"",
                                    hex.escape_debug()
                                ),
                            ));
                        }
                        self.advance_by_str(&hex);
                        let code = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
                        s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    Some(c) => {
                        s.push('\\');
                        s.push(c);
                    }
                    None => {
                        return Err(CompileError::lex(
                            "unknown",
                            self.line,
                            self.col,
                            "Unterminated escape sequence",
                        ));
                    }
                },
                Some(c) => s.push(c),
            }
        }
        Ok(s)
    }

    fn read_number(&mut self) -> CompileResult<f64> {
        let start = self.pos;
        while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            self.advance();
        }
        if self.peek() == Some('.') && self.peek2().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            self.advance();
            while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                self.advance();
            }
        }
        if self.peek().map(|c| c == 'e' || c == 'E').unwrap_or(false) {
            self.advance();
            if self.peek().map(|c| c == '+' || c == '-').unwrap_or(false) {
                self.advance();
            }
            while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                self.advance();
            }
        }
        let num_str = &self.src[start..self.pos];
        num_str.parse::<f64>().map_err(|_| {
            CompileError::lex(
                "unknown",
                self.line,
                self.col,
                format!("Invalid number: {num_str}"),
            )
        })
    }

    fn skip_inline_ws(&mut self) {
        while self.peek().map(|c| c == ' ' || c == '\t').unwrap_or(false) {
            self.advance();
        }
    }

    fn eat_line_remainder(&mut self) {
        while self.peek().map(|c| c != '\n' && c != '\r').unwrap_or(false) {
            self.advance();
        }
    }

    fn keyword_or_ident(&self, ident: String) -> Token {
        match ident.as_str() {
            "using" => Token::Using,
            "as" => Token::As,
            "type" => Token::Type,
            "from" => Token::From,
            "props" => Token::Props,
            "middleware" => Token::Middleware,
            "true" => Token::BoolLiteral(true),
            "false" => Token::BoolLiteral(false),
            "null" => Token::Null,
            _ => Token::Ident(ident),
        }
    }

    fn expr_keyword_or_ident(&self, ident: String) -> Token {
        match ident.as_str() {
            "true" => Token::BoolLiteral(true),
            "false" => Token::BoolLiteral(false),
            "null" => Token::Null,
            _ => Token::Ident(ident),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use token::Token;

    fn lex(src: &str) -> Vec<Token> {
        Lexer::new(src)
            .tokenize()
            .expect("lex failed")
            .into_iter()
            .map(|(t, _)| t)
            .filter(|t| !t.is_trivia())
            .collect()
    }

    #[test]
    fn html_open_sanitize_lookahead_is_bounded_and_correct() {
        // sanitize is still detected on a normal opener.
        assert!(html_open_has_sanitize(" x sanitize}"));
        assert!(!html_open_has_sanitize(" x}"));
        // detected even after a long-but-valid expression (within the window).
        let long = format!(" {} sanitize}}", "a".repeat(1000));
        assert!(html_open_has_sanitize(&long));
        // an unterminated opener (no `}`) does not scan unboundedly and reports
        // no sanitize, rather than hanging.
        assert!(!html_open_has_sanitize(&"{html".repeat(100_000)));
        // multibyte chars at the window edge must not panic (char-safe slice).
        assert!(!html_open_has_sanitize(&"é".repeat(5000)));
    }

    fn lex_all(src: &str) -> Vec<Token> {
        Lexer::new(src)
            .tokenize()
            .expect("lex failed")
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    #[test]
    fn test_lex_plain_text() {
        let tokens = lex("hello world");
        assert_eq!(
            tokens,
            vec![
                Token::Text("hello".into()),
                Token::Text("world".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn test_lex_comment() {
        let tokens = lex("{* this is a comment *}");
        assert_eq!(
            tokens,
            vec![Token::Comment(" this is a comment ".into()), Token::Eof]
        );
    }

    #[test]
    fn test_lex_block_open_close() {
        let tokens = lex("{if}{/if}");
        assert!(
            tokens.contains(&Token::BlockOpen("if".into())),
            "Missing BlockOpen: {tokens:?}"
        );
        assert!(
            tokens.contains(&Token::BlockClose("if".into())),
            "Missing BlockClose: {tokens:?}"
        );
    }

    #[test]
    fn test_lex_else_else_if() {
        let tokens = lex("{else}{else if}");
        assert!(
            tokens.contains(&Token::BlockElse),
            "Missing BlockElse: {tokens:?}"
        );
        assert!(
            tokens.contains(&Token::BlockElseIf),
            "Missing BlockElseIf: {tokens:?}"
        );
    }

    #[test]
    fn test_lex_empty() {
        let tokens = lex("{empty}");
        assert_eq!(tokens, vec![Token::BlockEmpty, Token::Eof]);
    }

    #[test]
    fn test_lex_next() {
        let tokens = lex("{next}");
        assert_eq!(tokens, vec![Token::Next, Token::Eof]);
    }

    #[test]
    fn test_lex_raw_html() {
        let tokens = lex("{html");
        assert!(tokens.contains(&Token::RawHtml));
    }

    #[test]
    fn test_lex_raw_html_sanitize() {
        let tokens = lex("{html sanitize}");
        assert!(tokens.contains(&Token::RawHtmlSanitize));
    }

    #[test]
    fn test_lex_debug() {
        let tokens = lex("{debug");
        assert!(tokens.contains(&Token::Debug));
    }

    #[test]
    fn test_lex_redirect() {
        let tokens = lex("{redirect");
        assert!(tokens.contains(&Token::Redirect));
    }

    #[test]
    fn test_lex_client_let() {
        let tokens = lex("{$let");
        assert!(tokens.contains(&Token::ClientLet));
    }

    #[test]
    fn test_lex_expr_open_close() {
        let tokens = lex("${x}");
        // ExprOpen, Ident(x), ExprClose
        assert!(tokens.contains(&Token::ExprOpen));
        assert!(tokens.contains(&Token::Ident("x".into())));
        assert!(tokens.contains(&Token::ExprClose));
    }

    #[test]
    fn test_lex_string_literals() {
        // String literals are tokenized inside expressions (and match arms), NOT in plain template text.
        let tokens = lex(r#"${ "hello" }"#);
        assert!(tokens.contains(&Token::StringLiteral("hello".into())));
    }

    #[test]
    fn test_lex_match_arm_string_pattern() {
        // A match-arm string pattern is still tokenized as a StringLiteral (Match mode keeps literals).
        let tokens = lex(r#"{match x}'world' -> hi{/match}"#);
        assert!(tokens.contains(&Token::StringLiteral("world".into())));
    }

    #[test]
    fn test_apostrophe_in_prose_is_text() {
        // Regression: a lone apostrophe in template prose must NOT start a (here, unterminated) string
        // literal. It used to error with "Unterminated string literal"; now it is plain text.
        let tokens = lex("we'll be fine");
        assert!(!tokens.iter().any(|t| matches!(t, Token::StringLiteral(_))));
        assert!(
            tokens
                .iter()
                .any(|t| matches!(t, Token::Text(s) if s.contains('\'')))
        );
    }

    #[test]
    fn test_quotes_in_prose_are_text() {
        // Double quotes in prose are also plain text outside expressions/match arms.
        let tokens = lex(r#"the "best" deal"#);
        assert!(!tokens.iter().any(|t| matches!(t, Token::StringLiteral(_))));
    }

    #[test]
    fn test_lex_number() {
        let tokens = lex("${42.5}");
        assert!(tokens.contains(&Token::NumberLiteral(42.5)));
    }

    #[test]
    fn test_lex_bool_literals() {
        let tokens = lex("${true}${false}");
        assert!(tokens.contains(&Token::BoolLiteral(true)));
        assert!(tokens.contains(&Token::BoolLiteral(false)));
    }

    #[test]
    fn test_lex_null() {
        let tokens = lex("${null}");
        assert!(tokens.contains(&Token::Null));
    }

    #[test]
    fn test_lex_operators() {
        let tokens = lex("${a == b}");
        assert!(tokens.contains(&Token::DoubleEquals));
    }

    #[test]
    fn test_lex_pipe() {
        let tokens = lex("${x | upper}");
        assert!(tokens.contains(&Token::Pipe));
        assert!(tokens.contains(&Token::Ident("upper".into())));
    }

    #[test]
    fn test_lex_arrow() {
        let tokens = lex("${a -> b}");
        assert!(tokens.contains(&Token::Arrow));
    }

    #[test]
    fn test_lex_dot() {
        let tokens = lex("${a.b}");
        assert!(tokens.contains(&Token::Dot));
    }

    #[test]
    fn test_lex_spread() {
        let tokens = lex("${...props}");
        assert!(tokens.contains(&Token::DotDotDot));
    }

    #[test]
    fn test_lex_template_literal() {
        let tokens = lex("`hello ${name}`");
        assert!(tokens.contains(&Token::TemplateLiteralOpen));
        assert!(tokens.contains(&Token::TemplateExprOpen));
        assert!(tokens.contains(&Token::TemplateLiteralClose));
    }

    #[test]
    fn test_lex_html_tag() {
        let tokens = lex("<div>");
        assert!(tokens.contains(&Token::TagOpen("div".into())));
        assert!(tokens.contains(&Token::TagEnd));
    }

    #[test]
    fn test_lex_html_closing_tag() {
        let tokens = lex("</div>");
        assert!(tokens.contains(&Token::TagClose("div".into())));
    }

    #[test]
    fn test_lex_self_closing() {
        let tokens = lex("<br />");
        assert!(tokens.contains(&Token::TagOpen("br".into())));
        assert!(tokens.contains(&Token::TagSelfClose));
    }

    #[test]
    fn test_lex_on_directive() {
        let tokens = lex("<button on:click>");
        assert!(tokens.contains(&Token::OnDirective("click".into())));
    }

    #[test]
    fn test_lex_navigate_attr() {
        let tokens = lex("<a navigate>");
        assert!(tokens.contains(&Token::NavigateAttr));
    }

    #[test]
    fn test_lex_frontmatter_keywords() {
        let src = "---\nusing as type from props middleware\n---\n";
        let tokens = lex(src);
        assert!(tokens.contains(&Token::Using));
        assert!(tokens.contains(&Token::As));
        assert!(tokens.contains(&Token::Type));
        assert!(tokens.contains(&Token::From));
        assert!(tokens.contains(&Token::Props));
        assert!(tokens.contains(&Token::Middleware));
    }

    #[test]
    fn test_lex_sections_block() {
        let tokens = lex("{sections}");
        assert!(
            tokens.contains(&Token::BlockOpen("sections".into())),
            "{tokens:?}"
        );
    }

    #[test]
    fn test_lex_underscore() {
        let tokens = lex("${_}");
        assert!(tokens.contains(&Token::Underscore));
    }

    #[test]
    fn test_whitespace_preserved() {
        let tokens = lex_all("  hello");
        assert!(tokens.iter().any(|t| matches!(t, Token::Whitespace(_))));
    }

    #[test]
    fn test_never_panics_on_bad_input() {
        // The lexer should return an error, not panic.
        let _result = Lexer::new("${@@@}").tokenize();
        // If we get here, no panic occurred - the result may be Ok or Err
    }

    #[test]
    fn test_span_tracking() {
        let tokens = Lexer::new("ab\ncd").tokenize().unwrap();
        // "cd" should be on line 2
        let cd = tokens.iter().find(|(t, _)| *t == Token::Text("cd".into()));
        assert!(cd.is_some());
        assert_eq!(cd.unwrap().1.line, 2);
    }

    /// Is `cand` accepted for a reason the two PUBLISHED tables explain?
    ///
    /// This is the reverse direction of the invariant: not "does the predicate know about the
    /// table" but "does the table know about everything the predicate does". A clause written
    /// straight into `raw_brace_starts_slurp` and never added to a table would pass the forward
    /// check and fail here.
    fn explained_by_the_tables(cand: &str) -> bool {
        if RAW_BRACE_OPENERS.iter().any(|o| cand.starts_with(o)) {
            return true;
        }
        cand.strip_prefix('{')
            .is_some_and(|after| BLOCK_KEYWORDS.contains(&brace_ident(after).as_str()))
    }

    /// THE drift test named in the docs on `RAW_BRACE_OPENERS` and `raw_brace_starts_slurp`.
    ///
    /// `lex_raw_script` CONSUMES the set of `{`-constructs the raw-text scanners must stop
    /// before; `lex_raw_script_text` must STOP before exactly the same set. When those two
    /// carried independent literal tables they drifted, and ordinary JavaScript
    /// (`{elseBranch: 1}`) hit neither: the text scanner returned an empty token without
    /// advancing, `tokenize` re-entered forever, and the process allocated a token per pass
    /// until it aborted. So the tables and the predicate must be provably the same set.
    ///
    /// Both directions are asserted, because each catches a different mistake:
    ///   * FORWARD - every published opener, and every block keyword, is recognised. Catches a
    ///     table entry the predicate does not honour.
    ///   * REVERSE - nothing the predicate accepts is unexplained by the tables. Catches a
    ///     literal `starts_with` added to the predicate and to nothing else, which is the exact
    ///     shape of the original bug.
    #[test]
    fn raw_openers_are_all_recognised() {
        // FORWARD: the published openers, bare and followed by content.
        for opener in RAW_BRACE_OPENERS {
            assert!(
                raw_brace_starts_slurp(opener),
                "{opener:?} is published in RAW_BRACE_OPENERS but the predicate does not recognise it"
            );
            for tail in ["", " x", "}", " x}", "\n", "abc"] {
                let cand = format!("{opener}{tail}");
                assert!(
                    raw_brace_starts_slurp(&cand),
                    "{cand:?} starts with the published opener {opener:?} but was not recognised"
                );
            }
        }

        // FORWARD: every block keyword opens a construct too.
        for kw in BLOCK_KEYWORDS {
            for tail in ["", " x", "}", " x}"] {
                let cand = format!("{{{kw}{tail}");
                assert!(
                    raw_brace_starts_slurp(&cand),
                    "{cand:?} opens with the block keyword {kw:?} but was not recognised"
                );
            }
        }

        // REVERSE, by exhaustion over short inputs. Every `{`-prefixed string up to three
        // trailing characters over an alphabet that covers the punctuation the openers use plus
        // ASCII identifier characters. That is ~250k candidates and runs in well under a second.
        let alphabet: Vec<char> =
            "abcdefghijklmnopqrstuvwxyz0123456789*/$!@#%^&-_.:;,?<>[]()|+=~ \t\n{}\"'"
                .chars()
                .collect();
        let mut candidates: Vec<String> = vec!["{".to_string(), String::new()];
        for a in &alphabet {
            candidates.push(format!("{{{a}"));
            for b in &alphabet {
                candidates.push(format!("{{{a}{b}"));
                for c in &alphabet {
                    candidates.push(format!("{{{a}{b}{c}"));
                }
            }
        }

        // Longer candidates: mutations of the published entries, so a NEW multi-character clause
        // (which brute force at length three cannot reach) is still probed. Truncations, altered
        // final characters and appended characters around every opener and keyword.
        let mut longer: Vec<String> = Vec::new();
        for base in RAW_BRACE_OPENERS
            .iter()
            .map(|s| (*s).to_string())
            .chain(BLOCK_KEYWORDS.iter().map(|k| format!("{{{k}")))
        {
            for cut in 1..=base.chars().count() {
                let trunc: String = base.chars().take(cut).collect();
                longer.push(trunc.clone());
                for a in &alphabet {
                    longer.push(format!("{trunc}{a}"));
                }
            }
        }
        candidates.extend(longer);

        for cand in &candidates {
            if raw_brace_starts_slurp(cand) {
                assert!(
                    explained_by_the_tables(cand),
                    "the predicate accepts {cand:?}, but neither RAW_BRACE_OPENERS nor \
                     BLOCK_KEYWORDS explains it - a clause was added to raw_brace_starts_slurp \
                     without extending a table, which is the drift that hung the lexer"
                );
            }
        }
    }
}
