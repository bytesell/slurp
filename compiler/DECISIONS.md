# Compiler Architecture Decisions

## 1. Parser Approach: Hand-rolled Recursive Descent

**Decision:** Implement the parser as a hand-rolled recursive descent parser
rather than using a parser generator library such as `nom`, `pest`, or
`chumsky`.

**Reasons:**

1. **Multi-error collection.** A parser generator produces one error per parse
   attempt and typically aborts on the first failure. A hand-rolled parser can
   accumulate errors via `ErrorAccumulator` and continue parsing after a
   recovery point, giving developers actionable lists of all problems at once
   rather than forcing repeated compile cycles.

2. **WASM binary size.** `nom` and `chumsky` pull in substantial code size.
   `pest` generates a grammar-based state machine. A hand-rolled parser has no
   runtime grammar representation - the parser logic compiles down to direct
   branch instructions. This is critical for the WASM target where every
   kilobyte counts.

3. **Best error message quality.** Because we control every branch of the
   parser, we can write messages like `"Expected } to close {if condition}"`
   rather than the generic parser-combinator messages like `"expected one of:
   Ident, LBrace, ..."`. Error messages are a first-class concern in Slurp
   because developers editing templates expect IDE-quality feedback.

4. **Whitespace-agnostic parsing is natural in recursive descent.** The lexer
   emits whitespace tokens so it is lossless, but the parser skips all trivia
   tokens before each `peek()` call, making it automatically whitespace-
   agnostic. This would require special handling in most grammar-based tools.

5. **Custom grammar.** The Slurp syntax is a hybrid of HTML and a custom
   expression language with context-sensitive rules (frontmatter keywords,
   `{else if}` as a compound token, loop filters vs value filters). Recursive
   descent handles context naturally via the call stack; grammar-based tools
   require awkward state flags.

**Alternatives considered:**
- `pest` with a `.pest` grammar: rejected because it produces a single error,
  has no incremental parse support, and generates large WASM binaries.
- `nom` combinator: rejected because error messages are cryptic and it does not
  naturally support multi-error collection.
- `chumsky`: good design, but still heavier than needed for this grammar, and
  has WASM size implications from ariadne integration.

---

## 2. Escaping: hand-rolled, one escaper per output context

**Decision:** Implement the escapers directly in `renderer/escape.rs` rather
than delegating to a general-purpose crate, and choose between them from the
context a value lands in.

Every escaper on the output path is local. The `html-escape` crate is still a
dependency, used for exactly one thing: `decode_html_entities` in
`sections.rs`.

**Reasons:**

- **One crate cannot serve five contexts.** `escape_html` covers HTML text.
  `escape_attr` additionally escapes backtick, `<` and `>` as defence in depth
  against attribute-value breakout. `escape_js_string` neutralises the JS string
  delimiters, `$`, the line terminators and `<` / `/`. `escape_css_value`
  **removes** structural characters rather than escaping any, because CSS escapes
  are only valid inside identifiers and strings while a value slot in a `style`
  attribute can sit in a dozen grammatical positions. `escape_script_safe`
  escapes only what can terminate a `<script>` element, precisely so that a JSON
  payload survives as valid JSON.
- **The policy decisions live in the escaper's own doc comment**, next to the
  code, where a reviewer meets them. `/` is not escaped in HTML text because it
  has no meaning there and escaping it corrupts URLs and dates; `:` and `/`
  survive the CSS strip because `background:url(${path})` needs them and neither
  can open a declaration alone. Those are judgement calls a general crate cannot
  make for us.
- **Fewer dependencies on the WASM target**, which is a stated constraint of
  section 5 below.

**Escaping policy:** `${expr}` output is escaped for its context, never left
raw. `{html expr}` is the only opt-out. `{html expr sanitize}` runs the Ammonia
sanitizer first, which is the recommended pattern for user-generated content. In
a `<script>` body no escaper is a general answer, so slurp raises a compile error
instead unless the author declares the context with `| js` or `| json` - see
`docs/security-model.md`.

---

## 3. Sanitizer: `ammonia` with allowlist

**Decision:** Use the `ammonia` crate with an explicit allowlist of safe tags
and attributes for `{html sanitize}` nodes.

**Reasons:**
- An allowlist approach is safer than a blocklist: any future dangerous HTML
  spec additions are denied by default.
- `ammonia` is the most widely used, actively maintained HTML sanitizer in the
  Rust ecosystem.
- It uses an internal HTML5 parser (via `html5ever`) so it correctly handles
  malformed HTML that human-visible parsers would interpret differently -
  closing the parser differential attack vector.
- The allowlist we configure permits standard rich-text tags (headings, lists,
  tables, inline semantics) while stripping `<script>`, `<iframe>`, `<object>`,
  `<embed>`, `<applet>`, `<form>`, `<input>`, `<button>`, `<select>`,
  `<textarea>`, `<link>`, `<style>`, `<base>`, `<meta>`, all `on*` event
  attributes, `javascript:` URIs, and `data:` URIs.

---

## 4. Filter Design: Two-tier (value vs loop)

**Decision:** Filters are split into two categories: *value filters* that
transform a single `serde_json::Value`, and *loop filters* that transform an
`Vec<Value>` collection before iteration.

**Reasons:**
- Loop filters (`limit`, `sort`, `reverse`, `filter`) are semantically
  different: they mutate the iteration set, not an individual value. Treating
  them as the same kind of filter would require the renderer to detect which
  pipe-position a filter is in at runtime - fragile and error-prone.
- The parser distinguishes them at parse time: filters in `{each ... | limit(n)}`
  are stored in `EachNode::loop_filters`; filters in `${expr | upper}` are
  stored in `Expression::filters`.
- This separation enables static validation (e.g. "sort" is invalid on a string
  expression but valid on a collection) without runtime type inspection.

---

## 5. WASM Binary Size Strategy

- No `tokio` on WASM (`target.'cfg(not(target_arch = "wasm32"))'`).
- No file I/O on WASM - the WASM entry points accept source strings directly.
- Currency formatting is done manually rather than pulling in ICU4X or similar.
- Date formatting uses a simple token-replacement strategy rather than
  `chrono`/`time`, which avoids locale tables entirely.
- The sanitizer (`ammonia` + `html5ever`) is the largest binary contributor
  (~300KB after Brotli). This is a justified cost for security-critical
  functionality.
- JSON serialisation uses `serde_json::to_string` over a string bridge rather
  than `serde-wasm-bindgen` (saves one dependency).

---

## 6. Safe Navigation Semantics

**Decision:** Path lookups (`a.b.c`) never produce errors - null at any depth
returns `Value::Null`, which renders as the empty string `""`.

**Rationale:** Template authors should not need to guard every access with
`{if user}{if user.address}{if user.address.city}...{/if}{/if}{/if}`. The JS
optional-chaining pattern (`a?.b?.c`) is the model. A missing or null segment
yields silence, not a crash.

This is implemented in `renderer/scope.rs` `resolve_path()`.

---

## 7. Error Recovery

The parser accumulates errors via `ErrorAccumulator` and continues parsing
after failures. Recovery strategies used:

- **Unclosed block:** emits `ErrorCode::UnclosedBlock` and stops collecting
  children for that block; continues at the next sibling.
- **Unexpected token:** emits `ErrorCode::UnexpectedToken` and advances one
  token (skip-token recovery).
- **Unknown filter:** emits `ErrorCode::UnknownFilter` and uses the previous
  value (identity filter fallback).

This ensures that a single typo does not prevent the rest of the file from
being analysed - important for IDE language-server use cases.
