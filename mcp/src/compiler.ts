/**
 * The one place this package talks to the Slurp compiler.
 *
 * Everything below goes through `@slurp/compiler-wasm`, which is the real
 * compiler built to WebAssembly. There is no subprocess, no `slurp` binary on
 * PATH and no reimplementation of any rule in TypeScript: a diagnostic this
 * server reports is a diagnostic the compiler produced.
 *
 * TWO TRAPS THIS MODULE EXISTS TO CONTAIN:
 *
 *  1. Every wasm export returns a JSON **string**, not an object. The package
 *     brands those returns as `JsonString<T>` and ships a `parseJson` helper
 *     precisely because the fact keeps getting forgotten. Parsing happens here,
 *     once, so no tool handler can forget it.
 *
 *  2. `validate` is parse-only. The compile-time security walk lives outside the
 *     parser, so `validate` reports a file clean that `slurp build` then
 *     rejects. `validate_full` is the build-parity entry point and is the only
 *     one this server uses for validation. See `validateSource` below.
 */
import {
  parse as wasmParse,
  render_dev as wasmRenderDev,
  validate_full as wasmValidateFull,
  extract_schema as wasmExtractSchema,
  parseJson,
} from '@slurp/compiler-wasm'
import type { ParseResult, RenderResult, SchemaResult, SlurpError } from '@slurp/compiler-wasm'

export type { ParseResult, RenderResult, SchemaResult, SlurpError }

/** Severity as the compiler spells it. `info` is folded into warnings by the accumulator. */
export type Severity = SlurpError['severity']

/**
 * A diagnostic as this server reports it: the compiler's own fields, plus the
 * caller's filename substituted for the compiler's fixed `wasm-input`.
 *
 * The wasm entry points hard-code the file name `wasm-input` (see
 * `compiler/src/wasm/mod.rs`), which is meaningless to a caller that passed its
 * own path. `withFile` rewrites it so line/column land somewhere an agent can
 * open.
 */
export interface Diagnostic {
  code: string
  message: string
  file: string
  line: number
  column: number
  severity: Severity
}

/** The file name the wasm exports stamp on every diagnostic they produce. */
const WASM_INPUT_FILE = 'wasm-input'

function withFile(errors: SlurpError[], file: string): Diagnostic[] {
  return errors.map((e) => ({
    code: e.code as string,
    message: e.message,
    file: e.file === WASM_INPUT_FILE ? file : e.file,
    line: e.line,
    column: e.column,
    severity: e.severity,
  }))
}

/**
 * Diagnostics compare equal when they name the same problem at the same place.
 *
 * `renderTemplate` merges the validation pass and the render pass, and both can
 * legitimately report the same rule: an unfiltered `${ }` in a `<script>` body
 * is refused by the security walk AND again by the renderer, which is
 * deliberate in the compiler (the render-time half covers the section paths
 * that render without the compile gate). Reporting it twice would just be noise.
 */
function diagnosticKey(d: Diagnostic): string {
  return `${d.severity}\u0000${d.code}\u0000${d.file}\u0000${d.line}\u0000${d.column}\u0000${d.message}`
}

export function dedupeDiagnostics(diagnostics: Diagnostic[]): Diagnostic[] {
  const seen = new Set<string>()
  const out: Diagnostic[] = []
  for (const d of diagnostics) {
    const key = diagnosticKey(d)
    if (seen.has(key)) continue
    seen.add(key)
    out.push(d)
  }
  return out
}

export function hasErrors(diagnostics: Diagnostic[]): boolean {
  return diagnostics.some((d) => d.severity === 'error')
}

/** Sort so an agent reads diagnostics in source order rather than pass order. */
export function sortDiagnostics(diagnostics: Diagnostic[]): Diagnostic[] {
  return [...diagnostics].sort(
    (a, b) => a.line - b.line || a.column - b.column || a.code.localeCompare(b.code),
  )
}

/**
 * Validate a template exactly as a build would.
 *
 * Uses `validate_full`, so this covers both parse diagnostics and the
 * compile-time security walk (unfiltered `${ }` in a `<script>` body, a `| js`
 * slot in JavaScript statement position, `env.SLURP_SECRET_*`, `request.*`
 * outside middleware, `{redirect}` outside middleware).
 *
 * KNOWN LIMIT, and it is the compiler's shape not a shortcut here: the security
 * walk returns on its FIRST violation, so at most one security diagnostic
 * appears per call. Parse diagnostics accumulate normally. Fix the reported one
 * and validate again to see the next.
 */
export function validateSource(
  source: string,
  file: string,
  isMiddleware: boolean,
): Diagnostic[] {
  const errors = parseJson<SlurpError[]>(wasmValidateFull(source, isMiddleware))
  return sortDiagnostics(withFile(errors, file))
}

export interface ParsedTemplate {
  ok: boolean
  ast: Record<string, unknown> | null
  diagnostics: Diagnostic[]
}

export function parseSource(source: string, file: string): ParsedTemplate {
  const result = parseJson<ParseResult>(wasmParse(source))
  return {
    ok: result.ok,
    ast: result.ast,
    diagnostics: sortDiagnostics(withFile([...result.errors, ...result.warnings], file)),
  }
}

export interface RenderOutcome {
  /** True when nothing at error severity was reported by any pass. Mirrors `slurp build`. */
  ok: boolean
  html: string
  diagnostics: Diagnostic[]
}

/**
 * Parse then render, in DEVELOPMENT mode.
 *
 * Development mode is not a debug convenience here, it is the point. Two
 * advisories are recorded only in development mode: the CSS-structural strip in
 * a `style` value slot, and the undeclared slot in a JavaScript-evaluated
 * attribute. Both silently change what a production render emits, which is the
 * right trade for a server hot path and the wrong one for an authoring tool.
 * Loop truncation is recorded in BOTH modes.
 *
 * `{debug expr}` also renders only in development mode, which is why a template
 * containing one produces output here and nothing under `slurp build`.
 */
export function renderTemplate(
  source: string,
  file: string,
  context: unknown,
  isMiddleware: boolean,
): RenderOutcome {
  // The validation pass first, because the security walk is not part of
  // rendering and its findings are the ones that fail a real build.
  const validation = validateSource(source, file, isMiddleware)

  const parsed = parseSource(source, file)
  if (parsed.ast === null) {
    return {
      ok: false,
      html: '',
      diagnostics: sortDiagnostics(dedupeDiagnostics([...validation, ...parsed.diagnostics])),
    }
  }

  const rendered = parseJson<RenderResult>(
    wasmRenderDev(JSON.stringify(parsed.ast), JSON.stringify(context ?? {})),
  )
  const renderDiagnostics = withFile([...rendered.errors, ...rendered.warnings], file)

  const all = sortDiagnostics(
    dedupeDiagnostics([...validation, ...parsed.diagnostics, ...renderDiagnostics]),
  )
  return { ok: !hasErrors(all), html: rendered.html, diagnostics: all }
}

/**
 * The `section { }` schema, via the compiler's own extractor.
 *
 * Returns `schema: null` for a template that declares none, which is not an
 * error. A `block { }` schema is NOT returned here, because `extract_schema`
 * reads `frontmatter.section` only; `blockSchemaFromAst` covers that.
 */
export function extractSectionSchema(source: string, file: string): {
  schema: Record<string, unknown> | null
  diagnostics: Diagnostic[]
} {
  const result = parseJson<SchemaResult>(wasmExtractSchema(source))
  return {
    schema: (result.schema as Record<string, unknown> | null) ?? null,
    diagnostics: sortDiagnostics(withFile([...result.errors, ...result.warnings], file)),
  }
}

/**
 * Read a frontmatter key straight off a parsed AST.
 *
 * `block { }` schemas, `props`, `using` imports and `middleware` all live in
 * `frontmatter` and none of them has a dedicated wasm export. Rather than
 * report those as unavailable, this reads the AST the compiler already handed
 * back, so the values are still the compiler's own parse and not a second
 * grammar implemented here.
 */
export function frontmatterField(
  ast: Record<string, unknown> | null,
  key: string,
): unknown {
  if (!ast) return null
  const fm = ast['frontmatter']
  if (!fm || typeof fm !== 'object') return null
  const value = (fm as Record<string, unknown>)[key]
  return value ?? null
}
