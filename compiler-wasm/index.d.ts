/**
 * Type declarations for the WASM build of the Slurp compiler.
 *
 * THE ONE THING TO KNOW: every export here returns a JSON **string**, not an
 * object. `compiler/src/wasm/mod.rs` serialises with `JsValue::from_str`, so the
 * caller must `JSON.parse` the result. Documentation elsewhere in this repo has
 * claimed otherwise in the past, and the VS Code extension carries a comment
 * about the same trap, so the return types below are branded: `JsonString<T>`
 * is a `string` that is known to parse to `T`. It cannot be used as a `T` by
 * accident, and `parseJson` is the one way to cross that boundary.
 *
 * The parsed shapes are imported from `shared/interfaces.ts` rather than
 * redeclared, so there is exactly one definition of each and it cannot drift
 * away from what the Rust actually emits.
 */
import type { ParseResult, RenderResult, SchemaResult, SlurpError } from '../shared/interfaces'

export type { ParseResult, RenderResult, SchemaResult, SlurpError }

/** A JSON string known to parse to `T`. See the note above. */
export type JsonString<T> = string & { readonly __parsesTo?: T }

/** `JSON.parse`, typed for the branded strings these exports return. */
export declare function parseJson<T>(json: JsonString<T>): T

/**
 * Parse a Slurp template source string.
 *
 * A lexer failure yields `ok: false` with a single error and `ast: null`; a
 * parse that recovers yields an AST alongside whatever diagnostics it collected.
 */
export declare function parse(source: string): JsonString<ParseResult>

/**
 * Render a previously-parsed AST against a JSON context. Both arguments are
 * JSON strings; malformed input comes back as an `InvalidAst` or
 * `InvalidContext` error rather than a thrown exception.
 *
 * Renders in production mode: `{debug}` emits nothing and the advisory
 * truncation warnings (loop cap, CSS-stripped, undeclared JS slot) are not
 * recorded. Use `render_dev` in an editor, where those diagnostics are the
 * point.
 */
export declare function render(
  astJson: string,
  contextJson: string,
): JsonString<RenderResult>

/**
 * `render`, in development mode: `{debug}` renders and the advisory warnings
 * above are recorded. This is the entry point an editor or preview pane wants,
 * because production mode truncates a >1000-item loop with no diagnostic at all.
 */
export declare function render_dev(
  astJson: string,
  contextJson: string,
): JsonString<RenderResult>

/**
 * Parse and return only the diagnostics, as a flat array of errors and warnings.
 *
 * PARSE ONLY. It does not run the compile-time security walk, so it reports a file
 * clean that `slurp build` then rejects. Use `validate_full` for build parity.
 */
export declare function validate(source: string): JsonString<SlurpError[]>

/**
 * `validate`, plus the compile-time security walk that `slurp build` and `slurp
 * validate` run. This is the entry point an authoring or agent tool wants.
 *
 * It is the only export here that enforces the rules living outside the parser: an
 * unfiltered `${ }` in a `<script>` body, a `| js` slot in JavaScript statement
 * position, `env.SLURP_SECRET_*`, `request.*` outside middleware and `{redirect}`
 * outside middleware.
 *
 * Pass `is_middleware: true` only for a file that will be compiled as middleware;
 * the rules genuinely differ. `false` matches the CLI and both `compile` entry points.
 *
 * The walk stops at its FIRST violation, so at most one security diagnostic appears
 * per call while parse diagnostics accumulate normally.
 */
export declare function validate_full(
  source: string,
  is_middleware: boolean,
): JsonString<SlurpError[]>

/**
 * Extract the `section { }` editor schema from a template.
 *
 * `schema` is `null` when the template declares none, which is not an error.
 */
export declare function extract_schema(source: string): JsonString<SchemaResult>

/**
 * Render a section template with saved editor state merged over its schema
 * defaults. The merge is tolerant: unknown keys are dropped and out-of-range
 * numbers are clamped to the schema's `min`/`max` rather than rejected.
 */
export declare function render_section(
  astJson: string,
  sectionStateJson: string,
  contextJson: string,
): JsonString<RenderResult>
