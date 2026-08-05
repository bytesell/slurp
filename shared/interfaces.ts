export interface SlurpError {
  file: string
  line: number
  column: number
  message: string
  severity: 'error' | 'warning' | 'info'
  code: SlurpErrorCode
}

export type SlurpDiagnostic = SlurpError

export type SlurpErrorCode =
  | 'UnexpectedToken'
  | 'UnclosedBlock'
  | 'InvalidFrontmatter'
  | 'CircularImport'
  | 'MissingFile'
  | 'MissingRequiredProp'
  | 'UnknownProp'
  | 'TypeMismatch'
  | 'SecretEnvInTemplate'
  | 'MiddlewareScopeViolation'
  | 'IterationLimitExceeded'
  | 'MissingImageSrc'
  | 'LayoutDepthExceeded'
  | 'UnknownComponent'
  | 'UnknownSlot'
  | 'UnknownFilter'
  | 'InvalidFilterArgs'
  | 'RedirectOutsideMiddleware'
  | 'InfiniteLoop'
  /** A `${ }` in a `<script>` body or JS-evaluated attribute that does not declare its escaping. */
  | 'UnsafeScriptInterpolation'
  /** A JavaScript template literal in an HTML attribute, whose `${ }` slurp interpolates itself. */
  | 'JsTemplateLiteralInAttribute'

export interface SlurpBuildOutput {
  pages: Record<string, string>
  assets: Record<string, Uint8Array>
  manifest: SlurpManifest
  errors: SlurpError[]
  warnings: SlurpError[]
}

export interface SlurpManifest {
  version: string
  routes: Record<string, RouteManifest>
  components: Record<string, ComponentManifest>
}

export interface RouteManifest {
  file: string
  /** Named URL params: { slug: 'string' } */
  params: Record<string, string>
  /** JS chunk filenames needed by this page */
  chunks: string[]
  /** Cache TTL in seconds, 0 = no cache */
  cache: number
  /** Middleware names applied to this route */
  middleware: string[]
}

export interface ComponentManifest {
  file: string
  props: Record<string, PropManifest>
  types: Record<string, TypeManifest>
  slots: string[]
}

export interface PropManifest {
  type: string
  required: boolean
  default?: unknown
}

export interface TypeManifest {
  fields: Record<string, string>
}

export interface SlurpConfig {
  image: ImageConfig
  dev: DevConfig
}

export interface ImageConfig {
  /** Source domains to bypass the optimizer */
  except: string[]
  /** Output formats to generate (in order of preference) */
  formats: ('avif' | 'webp' | 'jpeg')[]
  /** Cache TTL in seconds */
  cacheTtl: number
}

export interface DevConfig {
  port: number
  hmrPort: number
}

export interface SlurpRuntimeOptions {
  mode: 'production' | 'editor' | 'dev'
  /**
   * Editor/dev-only: intercept all fetch block function calls.
   * Return mock data from the editor environment.
   */
  fetchInterceptor?: (fn: (...args: unknown[]) => Promise<unknown>, args: unknown[]) => Promise<unknown>
}

/*
 * The three result types below are what the WASM exports in
 * `compiler/src/wasm/mod.rs` actually emit, field for field. They are the
 * PARSED shapes: every export returns a JSON string, so a caller parses first.
 * See `compiler-wasm/index.d.ts`, which imports these rather than redeclaring
 * them so the two cannot drift.
 */

export interface ParseResult {
  /** `false` when parsing produced at least one error-severity diagnostic. */
  ok: boolean
  /** `null` only when lexing failed outright; a recovered parse still returns an AST. */
  ast: SerializedAST | null
  errors: SlurpError[]
  warnings: SlurpError[]
}

export interface RenderResult {
  /** `false` when rendering produced at least one error-severity diagnostic. */
  ok: boolean
  /**
   * Always a string, empty on failure. It is never null: the Rust field is a
   * `String`, and an input that could not be decoded renders as `""` alongside
   * an `InvalidAst`, `InvalidContext` or `InvalidSectionState` error.
   */
  html: string
  errors: SlurpError[]
  warnings: SlurpError[]
}

/** Result of `extract_schema`: the `section { }` editor schema, if the template declares one. */
export interface SchemaResult {
  ok: boolean
  /** `null` when the template declares no `section { }` block, which is not an error. */
  schema: SerializedSectionSchema | null
  errors: SlurpError[]
  warnings: SlurpError[]
}

/** Opaque serialized editor schema - treated as a JSON blob by TypeScript consumers. */
export type SerializedSectionSchema = Record<string, unknown>

export interface BuildResult {
  output: SlurpBuildOutput | null
  errors: SlurpError[]
  warnings: SlurpError[]
}

/** Opaque serialized AST - treated as JSON blob by TypeScript consumers */
export type SerializedAST = Record<string, unknown>

export interface SlurpPosition {
  line: number
  character: number
}

export interface SlurpRange {
  start: SlurpPosition
  end: SlurpPosition
}

export interface SlurpCompletionItem {
  label: string
  kind: CompletionKind
  detail?: string
  documentation?: string
  insertText?: string
  insertTextFormat?: 'plaintext' | 'snippet'
  sortText?: string
}

export type CompletionKind =
  | 'keyword'
  | 'snippet'
  | 'variable'
  | 'property'
  | 'function'
  | 'class'
  | 'module'
  | 'value'

export interface SlurpHoverResult {
  contents: string
  range?: SlurpRange
}

export type HmrMessage =
  | { type: 'reload'; pages: string[] }
  | { type: 'error'; errors: SlurpError[] }
  | { type: 'clear' }

export interface FetchState<T> {
  status: 'idle' | 'loading' | 'success' | 'error'
  data: T | null
  error: FetchErrorState | null
}

export interface FetchErrorState {
  message: string
  code: string | null
  status: number | null
}

export interface PaginatedFetchState<T> extends FetchState<T[]> {
  page: number
  totalPages: number
  firstPage: boolean
  lastPage: boolean
}

export interface InfiniteFetchState<T> extends FetchState<T[]> {
  items: T[]
  hasMore: boolean
}
