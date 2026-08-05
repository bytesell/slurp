/**
 * What is EDITABLE about a template.
 *
 * A Slurp theme's sections and blocks declare their own editor surface in
 * frontmatter, and that declaration is the contract between the template and
 * whatever renders a settings form over it. An agent asked to "add a subtitle to
 * the hero" needs to know what the hero already declares, what kinds exist, and
 * what the defaults are, before it writes a line.
 *
 * The `section { }` half comes from the compiler's own `extract_schema`. The
 * `block { }` half, the declared `props`, the `using` imports and the
 * `middleware` name have no dedicated export, so they are read off the AST that
 * the compiler's own parser produced. Nothing here re-implements a grammar.
 */
import {
  extractSectionSchema,
  frontmatterField,
  parseSource,
  sortDiagnostics,
  dedupeDiagnostics,
} from '../compiler.js'
import type { Diagnostic } from '../compiler.js'

export interface PropDeclaration {
  name: string
  type: string
  /** False only when the declaration carries a `?`. A `= default` does NOT clear it. */
  required: boolean
  hasDefault: boolean
}

export interface ImportDeclaration {
  path: string
  /** The name the template refers to it by: the alias if given, else the last path segment. */
  localName: string
}

export interface SchemaResultPayload {
  file: string
  /** The `section { }` schema, or null when none is declared. Not an error. */
  section: Record<string, unknown> | null
  /** The `block { }` schema, or null. A block file declares this instead of `section`. */
  block: Record<string, unknown> | null
  props: PropDeclaration[]
  imports: ImportDeclaration[]
  middleware: string | null
  /**
   * Present only for a block schema, and it matters: a block's TYPE is its FILE
   * NAME, not the `name` it declares. `blocks/button.slurp` is the type
   * `button` however its `name:` reads.
   */
  blockTypeNote?: string
  diagnostics: Diagnostic[]
}

function lastSegment(path: string): string {
  const cleaned = path.replace(/\.slurp$/i, '')
  const parts = cleaned.split('/')
  return parts[parts.length - 1] || cleaned
}

function readProps(raw: unknown): PropDeclaration[] {
  if (!Array.isArray(raw)) return []
  const out: PropDeclaration[] = []
  for (const entry of raw) {
    if (!entry || typeof entry !== 'object') continue
    const e = entry as Record<string, unknown>
    out.push({
      name: String(e['name'] ?? ''),
      type: String(e['type_expr'] ?? 'unknown'),
      required: e['required'] === true,
      hasDefault: e['default_value'] != null,
    })
  }
  return out
}

function readImports(raw: unknown): ImportDeclaration[] {
  if (!Array.isArray(raw)) return []
  const out: ImportDeclaration[] = []
  for (const entry of raw) {
    if (!entry || typeof entry !== 'object') continue
    // Serialised as `{ "Default": { path, alias } }`.
    const values = Object.values(entry as Record<string, unknown>)
    const inner = values[0]
    if (!inner || typeof inner !== 'object') continue
    const i = inner as Record<string, unknown>
    const path = String(i['path'] ?? '')
    if (!path) continue
    const alias = i['alias']
    out.push({
      path,
      localName: typeof alias === 'string' && alias ? alias : lastSegment(path),
    })
  }
  return out
}

export function templateSchema(source: string, file: string): SchemaResultPayload {
  const section = extractSectionSchema(source, file)
  const parsed = parseSource(source, file)

  const blockSchema = frontmatterField(parsed.ast, 'block') as Record<string, unknown> | null
  const middlewareRaw = frontmatterField(parsed.ast, 'middleware')

  const payload: SchemaResultPayload = {
    file,
    section: section.schema,
    block: blockSchema,
    props: readProps(frontmatterField(parsed.ast, 'props')),
    imports: readImports(frontmatterField(parsed.ast, 'imports')),
    middleware: typeof middlewareRaw === 'string' ? middlewareRaw : null,
    diagnostics: sortDiagnostics(
      dedupeDiagnostics([...section.diagnostics, ...parsed.diagnostics]),
    ),
  }

  if (blockSchema) {
    payload.blockTypeNote = `A block's TYPE is its file name without the extension (here: "${lastSegment(file)}"), not the "name" it declares. The name is a human label for an editor; the type is what section state and a {blocks} render both key on.`
  }

  return payload
}
