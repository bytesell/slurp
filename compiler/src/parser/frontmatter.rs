/// Frontmatter parser: parses the `---` ... `---` block at the top of a .slurp file.
///
/// Grammar (informal):
///
/// ```text
/// frontmatter ::= "---" newline statement* "---" newline?
///
/// statement ::= using_stmt
///             | type_stmt
///             | props_stmt
///             | middleware_stmt
///
/// using_stmt  ::= "using" "type"? "{" ident+ "}" "from" string_literal
///               | "using" string_literal ("as" ident)?
///
/// type_stmt   ::= "type" ident "{" field* "}"
/// field       ::= ident ("?")? ":" type_expr newline
///
/// props_stmt  ::= "props" "{" prop_def* "}"
/// prop_def    ::= ident ("?")? ":" type_expr ("=" expr)? newline
///
/// middleware_stmt ::= "middleware" string_literal
///
/// section_stmt ::= "section" "{" section_entry* "}"
/// section_entry ::= "name" ":" string_literal
///                 | "settings" "{" setting_def* "}"
///                 | "blocks" "{" block_def* "}"
///                 | "max_per_page" ":" number
/// setting_def ::= ident ":" setting_kind ("=" setting_value)? ("{" meta_entry* "}")?
/// setting_kind ::= "text" | "richtext" | "color" | "image" | "link"
///                | "number" | "toggle" | "select" "(" option ("," option)* ")"
/// block_def   ::= ident "{" ("max" ":" number)? ("settings" "{" setting_def* "}")? "}"
///
/// NOTE: `section`, `settings`, `blocks`, `name`, `max`, `max_per_page` and the
/// setting kinds are CONTEXTUAL - they lex as plain identifiers and only gain
/// meaning at this exact structural position. A `section` statement is only
/// recognized when immediately followed by `{`; templates using these words as
/// prop names, type names, or expression identifiers are unaffected.
/// ```
use crate::ast::*;
use crate::error::ErrorCode;
use crate::lexer::token::Token;
use crate::parser::Parser;
use crate::sections::json_number;

impl Parser {
    pub fn try_parse_frontmatter(&mut self) -> Option<Frontmatter> {
        if !self.check(&Token::FrontmatterDelimiter) {
            return None;
        }
        self.advance();

        let mut fm = Frontmatter::default();

        loop {
            match self.peek().clone() {
                Token::FrontmatterDelimiter | Token::Eof => {
                    if self.check(&Token::FrontmatterDelimiter) {
                        self.advance();
                    }
                    break;
                }
                Token::Using => {
                    self.advance();
                    if let Some(import) = self.parse_import() {
                        fm.imports.push(import);
                    }
                }
                Token::Type => {
                    self.advance();
                    if let Some(td) = self.parse_type_def() {
                        fm.type_defs.push(td);
                    }
                }
                Token::Props => {
                    self.advance();
                    let defs = self.parse_props_block();
                    fm.props.extend(defs);
                }
                Token::Middleware => {
                    self.advance();
                    match self.peek().clone() {
                        Token::StringLiteral(s) => {
                            fm.middleware = Some(s.clone());
                            self.advance();
                        }
                        Token::Ident(s) => {
                            fm.middleware = Some(s.clone());
                            self.advance();
                        }
                        other => {
                            let sp = self.peek_span();
                            self.push_error(
                                sp,
                                format!("Expected middleware name, found {other:?}"),
                                ErrorCode::InvalidFrontmatter,
                            );
                        }
                    }
                }
                Token::Ident(ref name)
                    if name == "block" && matches!(self.peek_nth(1), Token::LBrace) =>
                {
                    self.advance();
                    if let Some(schema) = self.parse_block_schema() {
                        if fm.block.is_some() {
                            let sp = self.peek_span();
                            self.push_error(
                                sp,
                                "Duplicate block schema in frontmatter",
                                ErrorCode::InvalidFrontmatter,
                            );
                        } else {
                            fm.block = Some(schema);
                        }
                    }
                }
                Token::Ident(ref name)
                    if name == "section" && matches!(self.peek_nth(1), Token::LBrace) =>
                {
                    self.advance();
                    if let Some(schema) = self.parse_section_block() {
                        if fm.section.is_some() {
                            let sp = self.peek_span();
                            self.push_error(
                                sp,
                                "Duplicate section block in frontmatter",
                                ErrorCode::InvalidFrontmatter,
                            );
                        } else {
                            fm.section = Some(schema);
                        }
                    }
                }
                // Skip unknown tokens / blank lines gracefully
                _ => {
                    self.advance();
                }
            }
        }

        Some(fm)
    }

    /// `block { name: "Heading" settings { ... } blocks { @theme } }` - the frontmatter of a
    /// REUSABLE theme block (`blocks/<name>.slurp`). Deliberately the same grammar as `section`
    /// minus `max_per_page`, so a theme author who can write one can write the other.
    fn parse_block_schema(&mut self) -> Option<BlockSchema> {
        let open_sp = self.peek_span();
        if !self.eat(&Token::LBrace) {
            self.push_error(
                open_sp,
                "Expected { after block",
                ErrorCode::InvalidFrontmatter,
            );
            return None;
        }
        let mut schema = BlockSchema::default();
        loop {
            match self.peek().clone() {
                Token::RBrace | Token::Eof => {
                    self.eat(&Token::RBrace);
                    break;
                }
                Token::Comma => {
                    self.advance();
                }
                Token::Ident(kw) => match kw.as_str() {
                    "name" => {
                        self.advance();
                        let colon_sp = self.peek_span();
                        if !self.eat(&Token::Colon) {
                            self.push_error(
                                colon_sp,
                                "Expected : after block name",
                                ErrorCode::InvalidFrontmatter,
                            );
                            continue;
                        }
                        match self.peek().clone() {
                            Token::StringLiteral(s) | Token::Ident(s) => {
                                schema.name = s;
                                self.advance();
                            }
                            other => {
                                let sp = self.peek_span();
                                self.push_error(
                                    sp,
                                    format!("Expected block name string, found {other:?}"),
                                    ErrorCode::InvalidFrontmatter,
                                );
                            }
                        }
                    }
                    "settings" if matches!(self.peek_nth(1), Token::LBrace) => {
                        self.advance();
                        schema.settings = self.parse_settings_block();
                    }
                    "blocks" if matches!(self.peek_nth(1), Token::LBrace) => {
                        self.advance();
                        let (defs, accepts) = self.parse_blocks_block();
                        schema.blocks = defs;
                        schema.accepts = accepts;
                    }
                    other => {
                        let sp = self.peek_span();
                        self.push_error(sp, format!("Unexpected '{other}' in block schema (expected name, settings or blocks)"), ErrorCode::InvalidFrontmatter);
                        self.advance();
                    }
                },
                other => {
                    let sp = self.peek_span();
                    self.push_error(
                        sp,
                        format!("Unexpected {other:?} in block schema"),
                        ErrorCode::InvalidFrontmatter,
                    );
                    self.advance();
                }
            }
        }
        Some(schema)
    }

    fn parse_section_block(&mut self) -> Option<SectionSchema> {
        let open_sp = self.peek_span();
        if !self.eat(&Token::LBrace) {
            self.push_error(
                open_sp,
                "Expected { after section",
                ErrorCode::InvalidFrontmatter,
            );
            return None;
        }

        let mut schema = SectionSchema::default();
        loop {
            match self.peek().clone() {
                Token::RBrace | Token::Eof => {
                    self.eat(&Token::RBrace);
                    break;
                }
                Token::Comma => {
                    self.advance();
                }
                Token::Ident(kw) => {
                    match kw.as_str() {
                        "name" => {
                            self.advance();
                            let colon_sp = self.peek_span();
                            if !self.eat(&Token::Colon) {
                                self.push_error(
                                    colon_sp,
                                    "Expected : after section name",
                                    ErrorCode::InvalidFrontmatter,
                                );
                                continue;
                            }
                            match self.peek().clone() {
                                Token::StringLiteral(s) | Token::Ident(s) => {
                                    schema.name = s;
                                    self.advance();
                                }
                                other => {
                                    let sp = self.peek_span();
                                    self.push_error(
                                        sp,
                                        format!("Expected section name string, found {other:?}"),
                                        ErrorCode::InvalidFrontmatter,
                                    );
                                }
                            }
                        }
                        "settings" if matches!(self.peek_nth(1), Token::LBrace) => {
                            self.advance();
                            schema.settings = self.parse_settings_block();
                        }
                        "blocks" if matches!(self.peek_nth(1), Token::LBrace) => {
                            self.advance();
                            let (defs, accepts) = self.parse_blocks_block();
                            schema.blocks = defs;
                            schema.accepts = accepts;
                        }
                        "max_per_page" => {
                            self.advance();
                            let colon_sp = self.peek_span();
                            if !self.eat(&Token::Colon) {
                                self.push_error(
                                    colon_sp,
                                    "Expected : after max_per_page",
                                    ErrorCode::InvalidFrontmatter,
                                );
                                continue;
                            }
                            match self.peek().clone() {
                                Token::NumberLiteral(n) if n >= 1.0 => {
                                    schema.max_per_page = Some(n as u32);
                                    self.advance();
                                }
                                other => {
                                    let sp = self.peek_span();
                                    self.push_error(sp, format!("Expected positive number for max_per_page, found {other:?}"), ErrorCode::InvalidFrontmatter);
                                    self.advance();
                                }
                            }
                        }
                        other => {
                            let sp = self.peek_span();
                            self.push_error(sp, format!("Unexpected '{other}' in section block (expected name, settings, blocks, or max_per_page)"), ErrorCode::InvalidFrontmatter);
                            self.advance();
                        }
                    }
                }
                other => {
                    let sp = self.peek_span();
                    self.push_error(
                        sp,
                        format!("Unexpected {other:?} in section block"),
                        ErrorCode::InvalidFrontmatter,
                    );
                    self.advance();
                }
            }
        }

        if schema.name.is_empty() {
            self.push_error(
                open_sp,
                "section block requires a name",
                ErrorCode::InvalidFrontmatter,
            );
        }
        Some(schema)
    }

    fn parse_settings_block(&mut self) -> Vec<SettingDef> {
        let open_sp = self.peek_span();
        if !self.eat(&Token::LBrace) {
            self.push_error(
                open_sp,
                "Expected { after settings",
                ErrorCode::InvalidFrontmatter,
            );
            return Vec::new();
        }

        let mut defs: Vec<SettingDef> = Vec::new();
        loop {
            match self.peek().clone() {
                Token::RBrace | Token::Eof => {
                    self.eat(&Token::RBrace);
                    break;
                }
                Token::Comma => {
                    self.advance();
                }
                Token::Ident(_) | Token::Type => {
                    let key_sp = self.peek_span();
                    let key = match self.advance().0 {
                        Token::Type => "type".to_string(),
                        Token::Ident(n) => n,
                        _ => unreachable!(),
                    };
                    let colon_sp = self.peek_span();
                    if !self.eat(&Token::Colon) {
                        self.push_error(
                            colon_sp,
                            format!("Expected : after setting key '{key}'"),
                            ErrorCode::InvalidFrontmatter,
                        );
                        continue;
                    }
                    let Some(kind) = self.parse_setting_kind() else {
                        continue;
                    };
                    let default = if self.eat(&Token::Equals) {
                        self.parse_setting_value()
                    } else {
                        None
                    };
                    if let SettingKind::Select(ref opts) = kind
                        && let Some(serde_json::Value::String(ref d)) = default
                        && !opts.iter().any(|o| o == d)
                    {
                        self.push_error(
                            key_sp,
                            format!(
                                "Default '{d}' for setting '{key}' is not one of the select options"
                            ),
                            ErrorCode::InvalidFrontmatter,
                        );
                    }
                    let meta = if self.check(&Token::LBrace) {
                        self.parse_meta_object()
                    } else {
                        serde_json::Map::new()
                    };
                    if defs.iter().any(|d| d.key == key) {
                        self.push_error(
                            key_sp,
                            format!("Duplicate setting key '{key}'"),
                            ErrorCode::InvalidFrontmatter,
                        );
                        continue;
                    }
                    defs.push(SettingDef {
                        key,
                        kind,
                        default,
                        meta,
                    });
                }
                other => {
                    let sp = self.peek_span();
                    self.push_error(
                        sp,
                        format!("Expected setting key, found {other:?}"),
                        ErrorCode::InvalidFrontmatter,
                    );
                    self.advance();
                }
            }
        }
        defs
    }

    fn parse_setting_kind(&mut self) -> Option<SettingKind> {
        let sp = self.peek_span();
        let name = match self.peek().clone() {
            Token::Ident(n) => {
                self.advance();
                n
            }
            other => {
                self.push_error(
                    sp,
                    format!("Expected setting type, found {other:?}"),
                    ErrorCode::InvalidFrontmatter,
                );
                return None;
            }
        };

        match name.as_str() {
            "text" => Some(SettingKind::Text),
            "richtext" => Some(SettingKind::Richtext),
            "color" => Some(SettingKind::Color),
            "image" => Some(SettingKind::Image),
            "icon" => Some(SettingKind::Icon),
            "link" => Some(SettingKind::Link),
            "number" => Some(SettingKind::Number),
            "toggle" => Some(SettingKind::Toggle),
            "select" => {
                let paren_sp = self.peek_span();
                if !self.eat(&Token::LParen) {
                    self.push_error(
                        paren_sp,
                        "select requires options: select(a, b, c)",
                        ErrorCode::InvalidFrontmatter,
                    );
                    return None;
                }
                let mut opts: Vec<String> = Vec::new();
                loop {
                    match self.peek().clone() {
                        Token::RParen => {
                            self.advance();
                            break;
                        }
                        Token::Eof => break,
                        Token::Ident(o) | Token::StringLiteral(o) => {
                            self.advance();
                            opts.push(o);
                            self.eat(&Token::Comma);
                        }
                        other => {
                            let osp = self.peek_span();
                            self.push_error(
                                osp,
                                format!("Expected select option, found {other:?}"),
                                ErrorCode::InvalidFrontmatter,
                            );
                            self.advance();
                        }
                    }
                }
                if opts.is_empty() {
                    self.push_error(
                        paren_sp,
                        "select needs at least one option",
                        ErrorCode::InvalidFrontmatter,
                    );
                    return None;
                }
                Some(SettingKind::Select(opts))
            }
            other => {
                self.push_error(sp, format!("Unknown setting type '{other}' (expected text, richtext, color, image, link, select, number, or toggle)"), ErrorCode::InvalidFrontmatter);
                None
            }
        }
    }

    fn parse_setting_value(&mut self) -> Option<serde_json::Value> {
        match self.peek().clone() {
            Token::StringLiteral(s) | Token::Ident(s) => {
                self.advance();
                Some(serde_json::Value::String(s))
            }
            Token::NumberLiteral(n) => {
                self.advance();
                Some(json_number(n))
            }
            Token::Minus => {
                self.advance();
                match self.peek().clone() {
                    Token::NumberLiteral(n) => {
                        self.advance();
                        Some(json_number(-n))
                    }
                    other => {
                        let sp = self.peek_span();
                        self.push_error(
                            sp,
                            format!("Expected number after -, found {other:?}"),
                            ErrorCode::InvalidFrontmatter,
                        );
                        None
                    }
                }
            }
            Token::BoolLiteral(b) => {
                self.advance();
                Some(serde_json::Value::Bool(b))
            }
            Token::Null => {
                self.advance();
                Some(serde_json::Value::Null)
            }
            other => {
                let sp = self.peek_span();
                self.push_error(
                    sp,
                    format!("Expected default value, found {other:?}"),
                    ErrorCode::InvalidFrontmatter,
                );
                None
            }
        }
    }

    fn parse_meta_object(&mut self) -> serde_json::Map<String, serde_json::Value> {
        let mut meta = serde_json::Map::new();
        self.eat(&Token::LBrace);
        loop {
            match self.peek().clone() {
                Token::RBrace | Token::Eof => {
                    self.eat(&Token::RBrace);
                    break;
                }
                Token::Comma => {
                    self.advance();
                }
                Token::Ident(_) | Token::Type => {
                    let key = match self.advance().0 {
                        Token::Type => "type".to_string(),
                        Token::Ident(n) => n,
                        _ => unreachable!(),
                    };
                    let colon_sp = self.peek_span();
                    if !self.eat(&Token::Colon) {
                        self.push_error(
                            colon_sp,
                            format!("Expected : after meta key '{key}'"),
                            ErrorCode::InvalidFrontmatter,
                        );
                        continue;
                    }
                    if let Some(v) = self.parse_setting_value() {
                        meta.insert(key, v);
                    }
                }
                other => {
                    let sp = self.peek_span();
                    self.push_error(
                        sp,
                        format!("Expected meta key, found {other:?}"),
                        ErrorCode::InvalidFrontmatter,
                    );
                    self.advance();
                }
            }
        }
        meta
    }

    /// A `blocks { ... }` group: inline block definitions plus, optionally, `@theme` targeting.
    fn parse_blocks_block(&mut self) -> (Vec<BlockDef>, Option<BlockTargeting>) {
        // SECURITY: nested `blocks { ... }` groups recurse here; bound the depth.
        if !self.enter_depth() {
            return (Vec::new(), None);
        }
        let out = self.parse_blocks_block_inner();
        self.exit_depth();
        out
    }

    /// `@theme` / `@theme(a, b, c)` - which THEME-LEVEL blocks are insertable here. A bare `@theme`
    /// accepts every theme block; the parenthesised form is a whitelist.
    fn parse_theme_target(&mut self, existing: Option<BlockTargeting>) -> Option<BlockTargeting> {
        let at_sp = self.peek_span();
        self.advance(); // consume `@theme`
        // `Some` with an EMPTY allow-list means a bare `@theme` was already declared, i.e. "every
        // theme block". That is the widest possible statement, so a whitelist that follows it cannot
        // narrow it - the states are distinguishable only because `accepts` is an Option.
        let already_unrestricted = existing.as_ref().is_some_and(|t| t.allow.is_empty());
        let mut target = existing.unwrap_or_default();
        if already_unrestricted {
            // Consume and discard any argument list so the rest of the group still parses.
            if self.check(&Token::LParen) {
                while !matches!(self.peek(), Token::RParen | Token::Eof) {
                    self.advance();
                }
                self.eat(&Token::RParen);
            }
            return Some(target);
        }
        if !self.check(&Token::LParen) {
            // Bare `@theme` widens to "every theme block", discarding any earlier whitelist.
            target.allow.clear();
            return Some(target);
        }
        self.advance(); // (
        let mut named: Vec<String> = Vec::new();
        loop {
            match self.peek().clone() {
                Token::RParen => {
                    self.advance();
                    break;
                }
                Token::Eof => break,
                Token::Ident(o) | Token::StringLiteral(o) => {
                    self.advance();
                    if !named.contains(&o) {
                        named.push(o);
                    }
                    self.eat(&Token::Comma);
                }
                other => {
                    let sp = self.peek_span();
                    self.push_error(
                        sp,
                        format!("Expected a theme block name in @theme(...), found {other:?}"),
                        ErrorCode::InvalidFrontmatter,
                    );
                    self.advance();
                }
            }
        }
        if named.is_empty() {
            self.push_error(at_sp, "@theme(...) needs at least one block name - write a bare @theme to accept every theme block", ErrorCode::InvalidFrontmatter);
            return Some(target);
        }
        // Reached only in whitelist mode: the `already_unrestricted` branch above returned early.
        target.allow.extend(named);
        Some(target)
    }

    fn parse_blocks_block_inner(&mut self) -> (Vec<BlockDef>, Option<BlockTargeting>) {
        let open_sp = self.peek_span();
        if !self.eat(&Token::LBrace) {
            self.push_error(
                open_sp,
                "Expected { after blocks",
                ErrorCode::InvalidFrontmatter,
            );
            return (Vec::new(), None);
        }

        let mut blocks: Vec<BlockDef> = Vec::new();
        let mut accepts: Option<BlockTargeting> = None;
        loop {
            match self.peek().clone() {
                Token::RBrace | Token::Eof => {
                    self.eat(&Token::RBrace);
                    break;
                }
                Token::Comma => {
                    self.advance();
                }
                // `@theme` lexes as a plain frontmatter ident (the reader already accepts '@').
                Token::Ident(ref n) if n == "@theme" => {
                    accepts = self.parse_theme_target(accepts.take());
                }
                Token::Ident(ref n) if n.starts_with('@') => {
                    let sp = self.peek_span();
                    self.push_error(
                        sp,
                        format!("Unknown block target '{n}' (only @theme is supported)"),
                        ErrorCode::InvalidFrontmatter,
                    );
                    self.advance();
                }
                Token::Ident(_) | Token::Type => {
                    let type_sp = self.peek_span();
                    let block_type = match self.advance().0 {
                        Token::Type => "type".to_string(),
                        Token::Ident(n) => n,
                        _ => unreachable!(),
                    };
                    let brace_sp = self.peek_span();
                    if !self.eat(&Token::LBrace) {
                        self.push_error(
                            brace_sp,
                            format!("Expected {{ after block type '{block_type}'"),
                            ErrorCode::InvalidFrontmatter,
                        );
                        continue;
                    }
                    let mut max: Option<u32> = None;
                    let mut settings: Vec<SettingDef> = Vec::new();
                    let mut nested: Vec<BlockDef> = Vec::new();
                    let mut nested_accepts: Option<BlockTargeting> = None;
                    loop {
                        match self.peek().clone() {
                            Token::RBrace | Token::Eof => {
                                self.eat(&Token::RBrace);
                                break;
                            }
                            Token::Comma => {
                                self.advance();
                            }
                            Token::Ident(kw) => match kw.as_str() {
                                "max" => {
                                    self.advance();
                                    let colon_sp = self.peek_span();
                                    if !self.eat(&Token::Colon) {
                                        self.push_error(
                                            colon_sp,
                                            "Expected : after max",
                                            ErrorCode::InvalidFrontmatter,
                                        );
                                        continue;
                                    }
                                    match self.peek().clone() {
                                        Token::NumberLiteral(n) if n >= 1.0 => {
                                            max = Some(n as u32);
                                            self.advance();
                                        }
                                        other => {
                                            let sp = self.peek_span();
                                            self.push_error(sp, format!("Expected positive number for max, found {other:?}"), ErrorCode::InvalidFrontmatter);
                                            self.advance();
                                        }
                                    }
                                }
                                "settings" if matches!(self.peek_nth(1), Token::LBrace) => {
                                    self.advance();
                                    settings = self.parse_settings_block();
                                }
                                // Nested blocks: a block may declare its own `blocks { ... }` group
                                // (blocks inside blocks). Parsed by the same routine, recursively.
                                "blocks" if matches!(self.peek_nth(1), Token::LBrace) => {
                                    self.advance();
                                    let (n, a) = self.parse_blocks_block();
                                    nested = n;
                                    nested_accepts = a;
                                }
                                other => {
                                    let sp = self.peek_span();
                                    self.push_error(sp, format!("Unexpected '{other}' in block definition (expected max, settings or blocks)"), ErrorCode::InvalidFrontmatter);
                                    self.advance();
                                }
                            },
                            other => {
                                let sp = self.peek_span();
                                self.push_error(
                                    sp,
                                    format!("Unexpected {other:?} in block definition"),
                                    ErrorCode::InvalidFrontmatter,
                                );
                                self.advance();
                            }
                        }
                    }
                    if blocks.iter().any(|b| b.block_type == block_type) {
                        self.push_error(
                            type_sp,
                            format!("Duplicate block type '{block_type}'"),
                            ErrorCode::InvalidFrontmatter,
                        );
                        continue;
                    }
                    blocks.push(BlockDef {
                        block_type,
                        max,
                        settings,
                        blocks: nested,
                        accepts: nested_accepts,
                    });
                }
                other => {
                    let sp = self.peek_span();
                    self.push_error(
                        sp,
                        format!("Expected block type name, found {other:?}"),
                        ErrorCode::InvalidFrontmatter,
                    );
                    self.advance();
                }
            }
        }
        (blocks, accepts)
    }

    fn parse_import(&mut self) -> Option<Import> {
        // `using type { Name, Name2 } from "path"`
        if self.check(&Token::Type) {
            self.advance();
            let mut names = Vec::new();
            if self.eat(&Token::LBrace) {
                loop {
                    match self.peek().clone() {
                        Token::RBrace => {
                            self.advance();
                            break;
                        }
                        Token::Ident(n) => {
                            names.push(n.clone());
                            self.advance();
                            self.eat(&Token::Comma);
                        }
                        Token::Eof => {
                            break;
                        }
                        _ => {
                            self.advance();
                        }
                    }
                }
            }
            let from_sp = self.peek_span();
            if !self.eat(&Token::From) {
                self.push_error(
                    from_sp,
                    "Expected 'from' after type import names",
                    ErrorCode::InvalidFrontmatter,
                );
                return None;
            }
            let path = self.parse_string_or_ident()?;
            return Some(Import::Types { path, names });
        }

        // `using "path"` or `using "@alias/path"` with optional `as Name`
        let path = self.parse_string_or_ident()?;
        let alias = if self.check(&Token::As) {
            self.advance();
            match self.peek().clone() {
                Token::Ident(n) => {
                    let n = n.clone();
                    self.advance();
                    Some(n)
                }
                _ => None,
            }
        } else {
            None
        };
        Some(Import::Default { path, alias })
    }

    fn parse_type_def(&mut self) -> Option<TypeDef> {
        let name_sp = self.peek_span();
        let name = match self.peek().clone() {
            Token::Ident(n) => {
                let n = n.clone();
                self.advance();
                n
            }
            other => {
                self.push_error(
                    name_sp,
                    format!("Expected type name, found {other:?}"),
                    ErrorCode::InvalidFrontmatter,
                );
                return None;
            }
        };

        let open_sp = self.peek_span();
        if !self.eat(&Token::LBrace) {
            self.push_error(
                open_sp,
                "Expected { after type name",
                ErrorCode::InvalidFrontmatter,
            );
            return None;
        }

        let mut fields = Vec::new();
        loop {
            match self.peek().clone() {
                Token::RBrace | Token::Eof => {
                    self.eat(&Token::RBrace);
                    break;
                }
                Token::Ident(field_name) => {
                    let field_name = field_name.clone();
                    self.advance();
                    let optional = self.eat(&Token::Question);
                    let colon_sp = self.peek_span();
                    if !self.eat(&Token::Colon) {
                        self.push_error(
                            colon_sp,
                            "Expected : after field name",
                            ErrorCode::InvalidFrontmatter,
                        );
                        continue;
                    }
                    let type_expr = self.read_type_expr();
                    fields.push(TypeField {
                        name: field_name,
                        type_expr,
                        optional,
                    });
                }
                _ => {
                    self.advance();
                }
            }
        }

        Some(TypeDef { name, fields })
    }

    fn parse_props_block(&mut self) -> Vec<PropDef> {
        let open_sp = self.peek_span();
        if !self.eat(&Token::LBrace) {
            self.push_error(
                open_sp,
                "Expected { after props",
                ErrorCode::InvalidFrontmatter,
            );
            return Vec::new();
        }

        let mut defs = Vec::new();
        loop {
            match self.peek().clone() {
                Token::RBrace | Token::Eof => {
                    self.eat(&Token::RBrace);
                    break;
                }
                Token::Ident(_) | Token::Type => {
                    let name = match self.peek().clone() {
                        Token::Type => {
                            self.advance();
                            "type".to_string()
                        }
                        Token::Ident(n) => {
                            self.advance();
                            n.clone()
                        }
                        _ => unreachable!(),
                    };
                    let required = !self.eat(&Token::Question);
                    let colon_sp = self.peek_span();
                    if !self.eat(&Token::Colon) {
                        self.push_error(
                            colon_sp,
                            "Expected : after prop name",
                            ErrorCode::InvalidFrontmatter,
                        );
                        continue;
                    }
                    let type_expr = self.read_type_expr();
                    let default_value = if self.eat(&Token::Equals) {
                        self.parse_expression()
                    } else {
                        None
                    };
                    defs.push(PropDef {
                        name,
                        type_expr,
                        required,
                        default_value,
                    });
                    self.eat(&Token::Comma);
                }
                _ => {
                    self.advance();
                }
            }
        }
        defs
    }

    fn read_type_expr(&mut self) -> String {
        // Types are stored as raw strings rather than parsed deeply - used by IDE tooling.
        // Raw-token peeking is needed so newlines terminate the expression (normally skipped).
        let mut parts: Vec<String> = Vec::new();
        loop {
            // A newline TERMINATES the type expression, and it has to be looked for in
            // the raw stream (trivia included) because `peek()` skips right past it.
            // Trailing spaces put a Whitespace token in front of the Newline, so this
            // must scan the trivia run rather than test only the immediate next token.
            if self.at_line_end() {
                break;
            }
            match self.peek().clone() {
                Token::Comma | Token::RBrace | Token::Equals | Token::Eof => break,
                Token::Ident(n) => {
                    parts.push(n.clone());
                    self.advance();
                }
                Token::LBracket => {
                    parts.push("[".into());
                    self.advance();
                }
                Token::RBracket => {
                    parts.push("]".into());
                    self.advance();
                }
                Token::Pipe => {
                    parts.push("|".into());
                    self.advance();
                }
                Token::Question => {
                    parts.push("?".into());
                    self.advance();
                }
                Token::LessThan => {
                    parts.push("<".into());
                    self.advance();
                }
                Token::GreaterThan => {
                    parts.push(">".into());
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }
        parts.join(" ")
    }

    fn parse_string_or_ident(&mut self) -> Option<String> {
        match self.peek().clone() {
            Token::StringLiteral(s) => {
                let s = s.clone();
                self.advance();
                Some(s)
            }
            Token::Ident(s) => {
                let s = s.clone();
                self.advance();
                Some(s)
            }
            other => {
                let sp = self.peek_span();
                self.push_error(
                    sp,
                    format!("Expected string path, found {other:?}"),
                    ErrorCode::InvalidFrontmatter,
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::parse;

    #[test]
    fn test_parse_no_frontmatter() {
        let (doc, diag) = parse("<div>hello</div>", "test").unwrap();
        assert!(!diag.has_errors());
        assert!(doc.frontmatter.is_none());
    }

    #[test]
    fn test_parse_empty_frontmatter() {
        let src = "---\n---\n<div>body</div>";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors());
        assert!(doc.frontmatter.is_some());
        let fm = doc.frontmatter.unwrap();
        assert!(fm.imports.is_empty());
    }

    #[test]
    fn test_parse_default_import() {
        let src = "---\nusing \"@components/Nav\"\n---\n";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors());
        let fm = doc.frontmatter.unwrap();
        assert_eq!(fm.imports.len(), 1);
        matches!(&fm.imports[0], crate::ast::Import::Default { path, .. } if path == "@components/Nav");
    }

    #[test]
    fn test_parse_import_with_alias() {
        let src = "---\nusing \"@components/Nav\" as Navigation\n---\n";
        let (doc, _) = parse(src, "test").unwrap();
        let fm = doc.frontmatter.unwrap();
        if let crate::ast::Import::Default { alias, .. } = &fm.imports[0] {
            assert_eq!(alias.as_deref(), Some("Navigation"));
        } else {
            panic!("Expected Default import");
        }
    }

    #[test]
    fn test_parse_type_import() {
        let src = "---\nusing type { Review } from \"@components/ReviewCard\"\n---\n";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let fm = doc.frontmatter.unwrap();
        assert!(
            matches!(&fm.imports[0], crate::ast::Import::Types { names, .. } if names.contains(&"Review".to_string()))
        );
    }

    #[test]
    fn test_parse_props() {
        let src = "---\nprops {\n  title: string\n  count?: number\n}\n---\n";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let fm = doc.frontmatter.unwrap();
        assert_eq!(fm.props.len(), 2);
        assert!(fm.props[0].required);
        assert!(!fm.props[1].required);
    }

    #[test]
    fn test_parse_middleware() {
        let src = "---\nmiddleware auth\n---\n";
        let (doc, _) = parse(src, "test").unwrap();
        let fm = doc.frontmatter.unwrap();
        assert_eq!(fm.middleware.as_deref(), Some("auth"));
    }

    // ---- section {} schema ----

    use crate::ast::SettingKind;

    const FULL_SECTION: &str = r##"---
section {
  name: "Hero"
  settings {
    heading:    text = "Welcome" { label: "Heading" }
    background: color = "#0a0a0a"
    align:      select(left, center, right) = center
    max_items:  number = 8 { min: 1, max: 24 }
    show_badge: toggle = true
    image:      image
    cta_link:   link
    blurb:      richtext
  }
  blocks { faq_item { max: 12 settings { question: text  answer: richtext } } }
  max_per_page: 3
}
---
<div>${section.settings.heading}</div>
"##;

    #[test]
    fn test_parse_section_full() {
        let (doc, diag) = parse(FULL_SECTION, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let schema = doc.frontmatter.unwrap().section.expect("no section schema");
        assert_eq!(schema.name, "Hero");
        assert_eq!(schema.max_per_page, Some(3));
        assert_eq!(schema.settings.len(), 8);

        let heading = &schema.settings[0];
        assert_eq!(heading.key, "heading");
        assert_eq!(heading.kind, SettingKind::Text);
        assert_eq!(heading.default, Some(serde_json::json!("Welcome")));
        assert_eq!(
            heading.meta.get("label"),
            Some(&serde_json::json!("Heading"))
        );

        let align = &schema.settings[2];
        assert_eq!(
            align.kind,
            SettingKind::Select(vec!["left".into(), "center".into(), "right".into()])
        );
        assert_eq!(align.default, Some(serde_json::json!("center")));

        let max_items = &schema.settings[3];
        assert_eq!(max_items.kind, SettingKind::Number);
        assert_eq!(max_items.default, Some(serde_json::json!(8)));
        assert_eq!(max_items.meta.get("min"), Some(&serde_json::json!(1)));
        assert_eq!(max_items.meta.get("max"), Some(&serde_json::json!(24)));

        assert_eq!(schema.settings[4].kind, SettingKind::Toggle);
        assert_eq!(schema.settings[4].default, Some(serde_json::json!(true)));
        assert_eq!(schema.settings[5].kind, SettingKind::Image);
        assert_eq!(schema.settings[5].default, None);
        assert_eq!(schema.settings[6].kind, SettingKind::Link);
        assert_eq!(schema.settings[7].kind, SettingKind::Richtext);

        assert_eq!(schema.blocks.len(), 1);
        let faq = &schema.blocks[0];
        assert_eq!(faq.block_type, "faq_item");
        assert_eq!(faq.max, Some(12));
        assert_eq!(faq.settings.len(), 2);
        assert_eq!(faq.settings[1].kind, SettingKind::Richtext);
    }

    // ── Theme blocks ────────────────────────────────────────────────────────────────────────

    #[test]
    fn theme_targeting_whitelist_and_inline_blocks_coexist() {
        let src = r#"---
section {
  name: "FAQ"
  blocks {
    @theme(heading, text, button)
    faq_item { settings { q: text } }
  }
}
---
<div></div>"#;
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let schema = doc.frontmatter.unwrap().section.unwrap();
        // Inline types survive untouched - adding targeting must not break an existing theme.
        assert_eq!(schema.blocks.len(), 1);
        assert_eq!(schema.blocks[0].block_type, "faq_item");
        let accepts = schema.accepts.expect("targeting");
        assert_eq!(accepts.allow, vec!["heading", "text", "button"]);
    }

    #[test]
    fn a_bare_theme_target_accepts_everything() {
        let src = "---
section { name: \"Blank\" blocks { @theme } }
---
<div></div>";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let accepts = doc
            .frontmatter
            .unwrap()
            .section
            .unwrap()
            .accepts
            .expect("targeting");
        assert!(
            accepts.allow.is_empty(),
            "empty allow-list means every theme block"
        );
    }

    /// A bare `@theme` is WIDER than a whitelist, so it must win regardless of declaration order -
    /// silently narrowing an author's "accept everything" would be the worst possible reading.
    #[test]
    fn a_bare_theme_target_is_not_narrowed_by_a_whitelist() {
        for src in [
            "---
section { name: \"S\" blocks { @theme @theme(heading) } }
---
<i></i>",
            "---
section { name: \"S\" blocks { @theme(heading) @theme } }
---
<i></i>",
        ] {
            let (doc, _) = parse(src, "test").unwrap();
            let accepts = doc
                .frontmatter
                .unwrap()
                .section
                .unwrap()
                .accepts
                .expect("targeting");
            assert!(accepts.allow.is_empty(), "bare @theme must win in: {src}");
        }
    }

    #[test]
    fn targeting_nests_onto_inline_blocks() {
        let src = r#"---
section {
  name: "Layout"
  blocks {
    column { blocks { @theme(heading, image) } }
  }
}
---
<div></div>"#;
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let schema = doc.frontmatter.unwrap().section.unwrap();
        let col = &schema.blocks[0];
        assert_eq!(col.block_type, "column");
        assert_eq!(
            col.accepts.as_ref().expect("nested targeting").allow,
            vec!["heading", "image"]
        );
        assert!(
            schema.accepts.is_none(),
            "the section itself targeted nothing"
        );
    }

    #[test]
    fn a_theme_block_file_parses_its_own_schema() {
        let src = r#"---
block {
  name: "Heading"
  settings {
    text: text = "Your heading"
    size: select(sm, md, lg) = md
  }
}
---
<h2>${ block.settings.text }</h2>"#;
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let fm = doc.frontmatter.unwrap();
        assert!(fm.section.is_none(), "a block file declares no section");
        let block = fm.block.expect("block schema");
        assert_eq!(block.name, "Heading");
        assert_eq!(block.settings.len(), 2);
        assert_eq!(
            block.settings[0].default,
            Some(serde_json::json!("Your heading"))
        );
    }

    #[test]
    fn a_theme_block_can_accept_other_theme_blocks() {
        let src = "---
block { name: \"Group\" blocks { @theme } }
---
<div></div>";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let block = doc.frontmatter.unwrap().block.expect("block schema");
        assert!(
            block.accepts.is_some(),
            "composition is what makes the palette feel unlimited"
        );
    }

    /// The parser must stay TOTAL: bad targeting is an error message, never a panic, and never a
    /// silently different meaning.
    #[test]
    fn malformed_targeting_is_reported_not_guessed() {
        let empty = "---
section { name: \"S\" blocks { @theme() } }
---
<i></i>";
        let (_, diag) = parse(empty, "test").unwrap();
        assert!(diag.has_errors(), "@theme() with no names must be an error");

        let unknown = "---
section { name: \"S\" blocks { @app } }
---
<i></i>";
        let (_, diag) = parse(unknown, "test").unwrap();
        assert!(diag.has_errors(), "an unknown @target must be reported");
    }

    /// Every theme published before this existed has no `accepts` and must serialize without it,
    /// so an older editor sees exactly the payload it saw yesterday.
    #[test]
    fn absent_targeting_is_absent_from_the_payload() {
        let (doc, _) = parse(FULL_SECTION, "test").unwrap();
        let schema = doc.frontmatter.unwrap().section.unwrap();
        let json = serde_json::to_value(&schema).unwrap();
        assert!(
            json.get("accepts").is_none(),
            "no targeting = no key: {json}"
        );
        assert!(json["blocks"][0].get("accepts").is_none(), "{json}");
    }

    #[test]
    fn test_section_schema_json_shape() {
        // The serialized shape is the editor contract - keep it stable.
        let (doc, _) = parse(FULL_SECTION, "test").unwrap();
        let schema = doc.frontmatter.unwrap().section.unwrap();
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["name"], "Hero");
        assert_eq!(json["max_per_page"], 3);
        assert_eq!(json["settings"][0]["key"], "heading");
        assert_eq!(json["settings"][0]["type"], "text");
        assert_eq!(json["settings"][0]["default"], "Welcome");
        assert_eq!(json["settings"][0]["meta"]["label"], "Heading");
        assert_eq!(json["settings"][2]["type"], "select");
        assert_eq!(
            json["settings"][2]["options"],
            serde_json::json!(["left", "center", "right"])
        );
        assert_eq!(json["blocks"][0]["type"], "faq_item");
        assert_eq!(json["blocks"][0]["max"], 12);
        // Round-trips (the AST travels as JSON through WASM).
        let back: crate::ast::SectionSchema = serde_json::from_value(json).unwrap();
        assert_eq!(back, schema);
    }

    #[test]
    fn test_section_negative_number_default() {
        let src = "---\nsection {\n  name: \"X\"\n  settings { offset: number = -10 }\n}\n---\n";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let schema = doc.frontmatter.unwrap().section.unwrap();
        assert_eq!(schema.settings[0].default, Some(serde_json::json!(-10)));
    }

    #[test]
    fn test_section_unknown_kind_is_error() {
        let src = "---\nsection {\n  name: \"X\"\n  settings { thing: blob }\n}\n---\n";
        let (_, diag) = parse(src, "test").unwrap();
        assert!(diag.has_errors(), "unknown setting kind accepted");
    }

    #[test]
    fn test_section_select_default_must_be_option() {
        let src = "---\nsection {\n  name: \"X\"\n  settings { align: select(left, right) = middle }\n}\n---\n";
        let (_, diag) = parse(src, "test").unwrap();
        assert!(diag.has_errors(), "out-of-options select default accepted");
    }

    #[test]
    fn test_section_duplicate_setting_key_is_error() {
        let src = "---\nsection {\n  name: \"X\"\n  settings { a: text  a: number }\n}\n---\n";
        let (_, diag) = parse(src, "test").unwrap();
        assert!(diag.has_errors(), "duplicate setting key accepted");
    }

    #[test]
    fn test_duplicate_section_block_is_error() {
        let src = "---\nsection { name: \"A\" }\nsection { name: \"B\" }\n---\n";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(diag.has_errors(), "duplicate section block accepted");
        assert_eq!(doc.frontmatter.unwrap().section.unwrap().name, "A");
    }

    #[test]
    fn test_section_requires_name() {
        let src = "---\nsection { settings { a: text } }\n---\n";
        let (_, diag) = parse(src, "test").unwrap();
        assert!(diag.has_errors(), "nameless section accepted");
    }

    // ---- contextual-keyword regressions: section/settings/blocks stay valid identifiers ----

    #[test]
    fn test_regression_section_as_prop_name() {
        let src = "---\nprops {\n  section: string\n  settings: object\n  blocks: string[]\n}\n---\n${section} ${settings.theme}";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let fm = doc.frontmatter.unwrap();
        assert!(fm.section.is_none());
        assert_eq!(fm.props.len(), 3);
        assert_eq!(fm.props[0].name, "section");
        assert_eq!(fm.props[1].name, "settings");
        assert_eq!(fm.props[2].name, "blocks");
    }

    #[test]
    fn test_regression_section_ident_without_brace_skipped() {
        // A stray `section` token at statement level (no `{`) keeps the old
        // skip-unknown behavior instead of erroring.
        let src = "---\nsection\nprops { title: string }\n---\n";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let fm = doc.frontmatter.unwrap();
        assert!(fm.section.is_none());
        assert_eq!(fm.props.len(), 1);
    }

    #[test]
    fn test_regression_section_as_type_name() {
        let src = "---\ntype section {\n  id: string\n}\n---\n";
        let (doc, diag) = parse(src, "test").unwrap();
        assert!(!diag.has_errors(), "{:?}", diag.errors);
        let fm = doc.frontmatter.unwrap();
        assert!(fm.section.is_none());
        assert_eq!(fm.type_defs[0].name, "section");
    }
}
