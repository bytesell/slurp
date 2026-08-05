/// AST types for the Slurp template language.
///
/// This module is the SINGLE definition of the AST. It is written out here,
/// rather than pulled from a shared crate, so the compiler is self-contained and
/// WASM-friendly with no extra crate dependencies.
///
/// Do not add a second Rust copy for another consumer to read: an uncompiled
/// mirror drifts silently and buys nothing but a hand-synchronisation chore.
/// `shared/interfaces.ts` is a different case - the TypeScript packages import
/// it, so it is built and type-checked.
pub use slurp_shared_ast::*;

mod slurp_shared_ast {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Document {
        pub frontmatter: Option<Frontmatter>,
        pub children: Vec<Node>,
        pub file: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum Node {
        Text(String),
        Expression(Expression),
        /// `{html expr}` - raw HTML output, NOT escaped.
        RawHtml {
            expr: Expression,
            sanitize: bool,
        },
        If(IfNode),
        Match(MatchNode),
        Each(EachNode),
        Repeat(RepeatNode),
        Fetch(FetchNode),
        Component(ComponentNode),
        Element(ElementNode),
        Layout(LayoutNode),
        Slot(SlotNode),
        /// `{$let name = expr}` - client-side reactive var.
        ClientLet {
            name: String,
            value: Expression,
        },
        Comment(String),
        /// `{debug expr}` - dev only, stripped in prod.
        Debug(Expression),
        Try(TryNode),
        /// `{redirect "path"}` - middleware only.
        Redirect(Expression),
        /// `{next}` - middleware only.
        Next,
        /// `<Head>` block inside a layout.
        Head(Vec<Node>),
        /// `{sections}` - host node replaced by the ordered render of a section list injected by
        /// the SSR host. `None` = the page's own section list (`{sections}`); `Some(name)` = a named
        /// global section GROUP (`{sections "header"}`), used in layouts for header/footer chrome.
        Sections(Option<String>),
        /// `{blocks}` - render this section's (or block's) child blocks, each from its own
        /// `blocks/<type>.slurp` file. The host node for theme-level blocks.
        Blocks,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
    pub struct Frontmatter {
        pub imports: Vec<Import>,
        pub type_defs: Vec<TypeDef>,
        pub props: Vec<PropDef>,
        pub middleware: Option<String>,
        /// `section { ... }` schema for theme-editor sections.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub section: Option<SectionSchema>,
        /// `block { ... }` schema, for a reusable theme block in `blocks/<name>.slurp`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub block: Option<BlockSchema>,
    }

    /// Schema declared by a `section { ... }` frontmatter block. Drives the
    /// theme editor's settings form and the renderer's `section` scope object.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
    pub struct SectionSchema {
        pub name: String,
        #[serde(default)]
        pub settings: Vec<SettingDef>,
        #[serde(default)]
        pub blocks: Vec<BlockDef>,
        /// Theme-level blocks this section accepts (`blocks { @theme }` / `@theme(a, b)`).
        /// `None` = inline `blocks { ... }` only, which is every theme written before this existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub accepts: Option<BlockTargeting>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max_per_page: Option<u32>,
    }

    /// Which THEME-LEVEL blocks (`blocks/<name>.slurp`) may be inserted at a given level.
    ///
    /// Modelled on Shopify's block targeting. Declared inside an existing `blocks { ... }` group so
    /// a section can mix inline block types and theme blocks:
    ///
    /// ```text
    /// blocks {
    ///   @theme(heading, text, button)   // theme blocks, whitelisted
    ///   legacy_item { settings { ... } } // an inline type, still fine
    /// }
    /// ```
    ///
    /// A bare `@theme` accepts EVERY theme block. The whitelist form is the recommended posture:
    /// it is what lets a section stay coherent ("a FAQ takes headings and text, not a countdown")
    /// while still drawing from one shared, growing palette.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
    pub struct BlockTargeting {
        /// Non-empty = ONLY these theme block types. Empty = every theme block (bare `@theme`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub allow: Vec<String>,
    }

    /// Schema declared by a `block { ... }` frontmatter block in `blocks/<name>.slurp` - a REUSABLE
    /// block, usable by any section that targets it, rather than one owned by a single section.
    ///
    /// The alternative is declaring blocks per section, which scopes them to whichever section
    /// happened to define them: `heading`/`text`/`image` would exist only there, and most sections'
    /// "Add" menu could offer nothing but whatever that one section declared.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
    pub struct BlockSchema {
        /// Display name for the editor's palette ("Heading"). The block's TYPE is its file name.
        pub name: String,
        #[serde(default)]
        pub settings: Vec<SettingDef>,
        /// Inline child types, exactly as a section declares them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub blocks: Vec<BlockDef>,
        /// Theme blocks this block accepts as children - how a `columns` or `group` block composes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub accepts: Option<BlockTargeting>,
    }

    /// One setting line: `heading: text = "Welcome" { label: "Heading" }`.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct SettingDef {
        pub key: String,
        #[serde(flatten)]
        pub kind: SettingKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub default: Option<serde_json::Value>,
        /// Free-form editor hints: `label`, `min`, `max`, ...
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        pub meta: serde_json::Map<String, serde_json::Value>,
    }

    /// Serializes flattened into [`SettingDef`] as
    /// `{"type": "select", "options": [...]}` / `{"type": "text"}`.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(tag = "type", content = "options", rename_all = "lowercase")]
    pub enum SettingKind {
        Text,
        Richtext,
        Color,
        Image,
        Link,
        /// A Lucide icon name (kebab-case, e.g. "arrow-right"). Stored as a plain string like
        /// `text`; the editor renders a visual searchable icon picker instead of a raw text field.
        Icon,
        Select(Vec<String>),
        Number,
        Toggle,
    }

    /// One repeatable block type: `faq_item { max: 12 settings { ... } }`. A block may itself
    /// declare a nested `blocks { ... }` group (blocks inside blocks, recursively) - e.g. a layout
    /// block that contains button blocks.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct BlockDef {
        #[serde(rename = "type")]
        pub block_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max: Option<u32>,
        #[serde(default)]
        pub settings: Vec<SettingDef>,
        /// Nested block types this block may contain. Empty for leaf blocks (the common case).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub blocks: Vec<BlockDef>,
        /// Theme-level blocks this block accepts as children.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub accepts: Option<BlockTargeting>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum Import {
        /// `using "@components/Navbar"`
        Default { path: String, alias: Option<String> },
        /// `using type { Review } from "@components/ReviewCard"`
        Types { path: String, names: Vec<String> },
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct TypeDef {
        pub name: String,
        pub fields: Vec<TypeField>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct TypeField {
        pub name: String,
        pub type_expr: String,
        pub optional: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct PropDef {
        pub name: String,
        pub type_expr: String,
        pub required: bool,
        pub default_value: Option<Expression>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Expression {
        pub kind: ExprKind,
        pub filters: Vec<Filter>,
        pub line: usize,
        pub column: usize,
    }

    impl Expression {
        pub fn new(kind: ExprKind, line: usize, column: usize) -> Self {
            Self {
                kind,
                filters: Vec::new(),
                line,
                column,
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum ExprKind {
        /// `var.path.to.value` - safe navigation: null at any depth → `""`.
        Path(Vec<PathSegment>),
        /// Reactive path: `$cart`, `$isActive`.
        ReactivePath(Vec<PathSegment>),
        StringLiteral(String),
        NumberLiteral(f64),
        BoolLiteral(bool),
        Null,
        Undefined,
        TemplateLiteral(Vec<TemplatePart>),
        Ternary {
            condition: Box<Expression>,
            then: Box<Expression>,
            else_: Box<Expression>,
        },
        BinaryOp {
            op: BinaryOp,
            left: Box<Expression>,
            right: Box<Expression>,
        },
        UnaryNot(Box<Expression>),
        UnaryNeg(Box<Expression>),
        Call {
            callee: Box<Expression>,
            args: Vec<Expression>,
        },
        Index {
            object: Box<Expression>,
            index: Box<Expression>,
        },
        /// `class={}` implicit cn() call.
        ClassList(Vec<ClassEntry>),
        Grouped(Box<Expression>),
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum PathSegment {
        Ident(String),
        Index(Box<Expression>),
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum TemplatePart {
        Literal(String),
        Expr(Expression),
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum BinaryOp {
        Add,
        Sub,
        Mul,
        Div,
        Mod,
        Gt,
        Lt,
        Gte,
        Lte,
        Eq,
        Neq,
        And,
        Or,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct ClassEntry {
        pub value: Expression,
        /// If `Some`, this class is only included when condition is truthy.
        pub condition: Option<Expression>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Filter {
        pub name: String,
        pub args: Vec<Expression>,
        pub line: usize,
        pub column: usize,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct IfNode {
        pub condition: Expression,
        pub then_branch: Vec<Node>,
        pub else_if_branches: Vec<(Expression, Vec<Node>)>,
        pub else_branch: Option<Vec<Node>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct MatchNode {
        pub value: Expression,
        pub arms: Vec<MatchArm>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct MatchArm {
        pub pattern: MatchPattern,
        pub body: Vec<Node>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum MatchPattern {
        StringLiteral(String),
        NumberLiteral(f64),
        BoolLiteral(bool),
        Wildcard,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct EachNode {
        pub item: String,
        pub index: Option<String>,
        pub collection: Expression,
        /// Loop-level filters: limit, sort, reverse, filter.
        pub loop_filters: Vec<Filter>,
        pub body: Vec<Node>,
        pub empty_branch: Option<Vec<Node>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct RepeatNode {
        pub count: Expression,
        pub body: Vec<Node>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct FetchNode {
        pub name: String,
        pub type_hint: Option<String>,
        pub is_array: bool,
        pub source: Expression,
        pub options: FetchOptions,
        pub body: Vec<Node>,
        pub loading_branch: Option<Vec<Node>>,
        pub error_branch: Option<Vec<Node>>,
        pub empty_branch: Option<Vec<Node>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
    pub struct FetchOptions {
        pub cache_ttl: Option<u64>,
        pub retry: Option<u32>,
        pub timeout_ms: Option<u64>,
        pub abort_on_navigate: bool,
        pub poll_seconds: Option<u64>,
        pub paginate: Option<PaginateOptions>,
        pub infinite: Option<InfiniteOptions>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct PaginateOptions {
        pub limit: u32,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct InfiniteOptions {
        pub limit: u32,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct ComponentNode {
        pub name: String,
        pub props: Vec<Prop>,
        /// `...spread` props.
        pub spread: Option<Expression>,
        pub children: Vec<Node>,
        /// Named slot assignment: `slot="header"`.
        pub slot: Option<String>,
        /// Event bindings: `on:add-to-cart="handler"`.
        pub events: Vec<EventBinding>,
        pub line: usize,
        pub column: usize,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Prop {
        pub name: String,
        /// Boolean shorthand: `<C featured />` → `featured = true`.
        pub value: PropValue,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum PropValue {
        Boolean,
        Expr(Expression),
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct EventBinding {
        pub event: String,
        pub handler: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct ElementNode {
        pub tag: String,
        pub attrs: Vec<Attribute>,
        pub children: Vec<Node>,
        pub self_closing: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Attribute {
        pub name: String,
        pub value: AttributeValue,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum AttributeValue {
        /// `attr` without value → true.
        Boolean,
        StringLiteral(String),
        /// `attr={ expr }` - a WHOLE expression the template author composed.
        Expr(Expression),
        /// `attr="text ${ slot } text"` - a quoted attribute whose literal text
        /// belongs to the author but whose `${ }` SLOTS are data. Parsed into a
        /// template literal at parse time, exactly like `Expr`, and kept as its
        /// own variant purely so the renderer can still tell the two apart.
        ///
        /// SECURITY: that distinction is what makes correct escaping possible at
        /// all. In a JS-evaluated attribute the slots of an interpolated string
        /// get JS-escaped, because the author wrote `f('...')` around them and
        /// means the slot to be a value; an `attr={ ... }` expression is left
        /// alone, because the author is composing JavaScript
        /// (`@click={ \`${openVar} = !${openVar}\` }`), which is the same trust
        /// level as a `{html attrs}` spread. Collapsing the two variants into one
        /// would erase the only signal telling data from author-written code, and
        /// the renderer could not recover it later.
        Interpolated(Expression),
        /// `class={...}` with implicit cn().
        ClassList(Expression),
        /// `on:click="handler"`.
        EventHandler(String),
        /// `navigate=false`.
        NavigateFalse,
        /// `{html expr}` or `{if cond}{html expr}{/if}` in attribute position.
        /// Injects a raw HTML attribute string (e.g. `required :disabled="loading"`).
        RawAttrInjection {
            condition: Option<Box<Expression>>,
            expr: Box<Expression>,
        },
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct LayoutNode {
        pub src: String,
        pub head: Option<Vec<Node>>,
        pub children: Vec<Node>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct SlotNode {
        pub name: Option<String>,
        pub fallback: Option<Vec<Node>>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct TryNode {
        pub body: Vec<Node>,
        pub error_branch: Vec<Node>,
    }
}
