/**
 * Static checks for the things Slurp compiles cleanly and then gets WRONG at
 * run time, silently.
 *
 * This is the one tool here that is not a view onto compiler diagnostics, and
 * the reason it exists is narrow: every rule below describes a construct that
 * `slurp validate` and `slurp build` both accept with zero output, and that then
 * renders the empty string or the wrong string with no trace. An agent cannot
 * discover any of them from a failing build, because the build does not fail.
 *
 * THE BAR FOR ADDING A RULE HERE. Three tests, all of them:
 *   1. The compiler emits NO diagnostic for it. Anything the compiler already
 *      reports belongs in `slurp_validate`, not here, or it is reported twice.
 *   2. It is detectable from the parsed AST (or, for exactly one rule, from the
 *      source) without guessing at intent.
 *   3. It is wrong essentially always, not merely unusual.
 *
 * Every rule below was confirmed against the real compiler by rendering the
 * construct and observing the output, not inferred from reading the source. The
 * observed output is quoted in each rule's comment. Two candidate rules were
 * REMOVED after that check disproved them, and they are recorded at the bottom
 * of this file so nobody re-adds them from a stale document.
 */
import { parseSource, sortDiagnostics } from '../compiler.js'
import type { Diagnostic } from '../compiler.js'

export interface LintFinding {
  rule: string
  message: string
  fix: string
  file: string
  line: number
  column: number
  severity: 'warning' | 'info'
}

export interface LintResult {
  clean: boolean
  findings: LintFinding[]
  /** Parse diagnostics, so a file that does not parse says so instead of linting nothing. */
  diagnostics: Diagnostic[]
}

/** Filter arguments are read as literals. `default` is the one exception. */
const FILTER_WITH_EVALUATED_ARG = 'default'

const LITERAL_KINDS = ['StringLiteral', 'NumberLiteral', 'BoolLiteral'] as const

interface Positioned {
  line?: number
  column?: number
}

function kindOf(expr: unknown): string | null {
  if (!expr || typeof expr !== 'object') return null
  const kind = (expr as Record<string, unknown>)['kind']
  if (typeof kind === 'string') return kind
  if (kind && typeof kind === 'object') {
    const keys = Object.keys(kind as Record<string, unknown>)
    return keys[0] ?? null
  }
  return null
}

function kindPayload(expr: unknown): unknown {
  if (!expr || typeof expr !== 'object') return null
  const kind = (expr as Record<string, unknown>)['kind']
  if (kind && typeof kind === 'object') {
    const keys = Object.keys(kind as Record<string, unknown>)
    return keys.length ? (kind as Record<string, unknown>)[keys[0]!] : null
  }
  return null
}

function posOf(node: unknown, fallback: Positioned = {}): { line: number; column: number } {
  const n = (node ?? {}) as Record<string, unknown>
  const line = typeof n['line'] === 'number' ? (n['line'] as number) : (fallback.line ?? 0)
  const column =
    typeof n['column'] === 'number' ? (n['column'] as number) : (fallback.column ?? 0)
  return { line, column }
}

/**
 * The AST serialises Rust enums as either `"Variant"` or `{ "Variant": payload }`.
 * This normalises both so a walker does not have to branch on the encoding.
 */
function asVariant(node: unknown): { name: string; payload: unknown } | null {
  if (typeof node === 'string') return { name: node, payload: null }
  if (!node || typeof node !== 'object' || Array.isArray(node)) return null
  const keys = Object.keys(node as Record<string, unknown>)
  if (keys.length !== 1) return null
  const name = keys[0]!
  return { name, payload: (node as Record<string, unknown>)[name] }
}

class Linter {
  readonly findings: LintFinding[] = []
  /** `{$let}` names seen, so a later read of one can be reported. */
  private readonly letNames = new Map<string, { line: number; column: number }>()
  private readonly letReads: Array<{ name: string; line: number; column: number }> = []

  constructor(private readonly file: string) {}

  add(
    rule: string,
    severity: 'warning' | 'info',
    at: { line: number; column: number },
    message: string,
    fix: string,
  ): void {
    this.findings.push({
      rule,
      severity,
      message,
      fix,
      file: this.file,
      line: at.line,
      column: at.column,
    })
  }

  /** Report reads of `{$let}` names last, so the declaration is always seen first. */
  finish(): void {
    for (const read of this.letReads) {
      const declared = this.letNames.get(read.name)
      if (!declared) continue
      this.add(
        'let-binding-not-readable',
        'warning',
        { line: read.line, column: read.column },
        `\`${read.name}\` is read as a template value but was introduced by \`{$let ${read.name} = ...}\` on line ${declared.line}. A \`{$let}\` seeds CLIENT state only: server-side it emits a <script type="application/json"> payload and creates no binding the template can read, so \`\${ ${read.name} }\` renders the empty string.`,
        `If the value is needed in the HTML, pass it through the render context and read it directly. Keep the \`{$let}\` only for state the browser runtime will pick up.`,
      )
    }
  }

  walkNodes(nodes: unknown): void {
    if (!Array.isArray(nodes)) return
    for (const node of nodes) this.walkNode(node)
  }

  walkNode(node: unknown): void {
    const variant = asVariant(node)
    if (!variant) return
    const { name, payload } = variant
    const p = (payload ?? {}) as Record<string, unknown>

    switch (name) {
      case 'Component':
        this.checkComponent(p)
        this.walkNodes(p['children'])
        return
      case 'Element':
        for (const attr of (p['attrs'] as unknown[]) ?? []) this.walkAttr(attr)
        this.walkNodes(p['children'])
        return
      case 'Expression':
      case 'RawHtml':
        this.walkExpr(name === 'Expression' ? payload : p['expr'])
        return
      // The AST calls a `{$let}` node `ClientLet`, which is the name that
      // matters: it is client state, not a template binding.
      case 'ClientLet': {
        // A ClientLet node carries no span of its own, only a name and a value,
        // so the value expression's position is the closest honest answer.
        const at = posOf(p['value'], posOf(p))
        const letName = p['name']
        if (typeof letName === 'string' && !this.letNames.has(letName)) {
          this.letNames.set(letName, at)
        }
        this.walkExpr(p['value'])
        return
      }
      default:
        break
    }

    // Generic descent. Every remaining node variant carries its children under a
    // handful of well-known field names and its expressions under others, so a
    // structural walk covers `{if}`, `{each}`, `{match}`, `{fetch}`, `{repeat}`,
    // `{try}`, `{layout}`, `{slot}` and `{head}` without a case for each. A node
    // variant added to the compiler is therefore still descended into, rather
    // than silently skipped.
    this.walkGeneric(payload)
  }

  private walkGeneric(payload: unknown): void {
    if (!payload || typeof payload !== 'object') return
    if (Array.isArray(payload)) {
      for (const item of payload) this.walkNode(item)
      return
    }
    const p = payload as Record<string, unknown>
    for (const [key, value] of Object.entries(p)) {
      if (value === null || typeof value !== 'object') continue
      if (EXPRESSION_FIELDS.has(key)) {
        this.walkExpr(value)
      } else if (Array.isArray(value)) {
        // Could be child nodes, arms, branches or filters. Arms and branch pairs
        // are arrays of their own, so recurse rather than assume a shape.
        for (const item of value) {
          if (Array.isArray(item)) {
            for (const inner of item) this.walkNode(inner)
          } else {
            this.walkNode(item)
          }
        }
      } else {
        this.walkGeneric(value)
      }
    }
  }

  private walkAttr(attr: unknown): void {
    if (!attr || typeof attr !== 'object') return
    const a = attr as Record<string, unknown>
    const value = a['value']
    const variant = asVariant(value)
    if (!variant) return
    // `Interpolated`, `Expr` and `ClassList` all carry a single expression.
    // `RawAttrInjection` carries two.
    if (variant.name === 'RawAttrInjection') {
      const payload = (variant.payload ?? {}) as Record<string, unknown>
      this.walkExpr(payload['condition'])
      this.walkExpr(payload['expr'])
      return
    }
    this.walkExpr(variant.payload)
  }

  /**
   * RULE: a quoted component prop is a LITERAL string.
   *
   * Verified: `<Card title="Hi ${name}" />` with `name = "Bo"` renders
   * `data-slurp-props="{"title":"Hi ${name}"}"`, the `${name}` intact. The same
   * text on an ELEMENT attribute interpolates correctly. The asymmetry is real
   * and the compiler reports nothing.
   *
   * RULE: an unquoted number in a prop position lexes as an attribute NAME.
   *
   * Verified: `<Card count=123 />` parses to TWO boolean props, one named
   * `count` and one named `123`. The number is gone.
   */
  private checkComponent(component: Record<string, unknown>): void {
    const at = posOf(component)
    const componentName = String(component['name'] ?? 'component')
    for (const prop of (component['props'] as unknown[]) ?? []) {
      if (!prop || typeof prop !== 'object') continue
      const propRecord = prop as Record<string, unknown>
      const propName = String(propRecord['name'] ?? '')

      if (/^[0-9]/.test(propName)) {
        this.add(
          'component-prop-unquoted-number',
          'warning',
          at,
          `<${componentName}> has a prop that parsed as the attribute NAME \`${propName}\`. An unquoted number in a component prop position is lexed as a name, not a value, so the intended prop became a bare boolean \`true\` and a second boolean prop named \`${propName}\` appeared beside it.`,
          `Wrap the number in braces: \`prop={${propName}}\`. Braces are what make a prop an expression.`,
        )
        continue
      }

      const value = asVariant(propRecord['value'])
      if (!value || value.name !== 'Expr') continue
      if (kindOf(value.payload) !== 'StringLiteral') continue
      const literal = kindPayload(value.payload)
      if (typeof literal !== 'string' || !literal.includes('${')) continue

      this.add(
        'component-prop-literal-interpolation',
        'warning',
        at,
        `<${componentName} ${propName}="${literal}"> passes the text ${JSON.stringify(literal)} through verbatim, interpolation slots and all. A QUOTED component prop is a literal string and is never interpolated, unlike a quoted attribute on a plain HTML element, which is. This asymmetry is the most common mistake in the language and the compiler reports nothing.`,
        `Use a template literal in braces: ${propName}={\`${literal}\`}. If the whole value is a single expression, ${propName}={expr} is simpler.`,
      )
    }
  }

  private walkExpr(expr: unknown): void {
    if (!expr || typeof expr !== 'object') return
    if (Array.isArray(expr)) {
      for (const item of expr) this.walkExpr(item)
      return
    }
    const e = expr as Record<string, unknown>
    const at = posOf(e)
    const kind = kindOf(e)
    const payload = kindPayload(e)

    /**
     * RULE: `f(...)` parses and always evaluates to null.
     *
     * Verified: `${ Math.max(1,2) }` renders `` (empty). There are no callable
     * functions in the language at all, and no diagnostic is emitted, so JS
     * habits like `items.map(...)` or `Math.round(x)` compile and vanish.
     */
    if (kind === 'Call') {
      const callee = (payload as Record<string, unknown> | null)?.['callee']
      const calleeText = pathText(callee) ?? 'the callee'
      this.add(
        'call-always-null',
        'warning',
        at,
        `\`${calleeText}(...)\` parses, but Slurp has NO callable functions: every call evaluates to null and renders the empty string, with no diagnostic.`,
        `Compute the value on the server and pass it through the render context, or use a filter. Filters are the only call-shaped construct in the language, and they are written \`\${ value | name(args) }\`.`,
      )
    }

    /**
     * RULE: a path segment `.length`.
     *
     * Verified: `${ items.length }` with `items = [1,2,3]` renders `` (empty).
     * An identifier segment applied to an array yields null. Reported at `info`
     * rather than `warning`, because a context OBJECT with a genuine `length`
     * key resolves normally and this cannot tell the two apart.
     */
    if (kind === 'Path' || kind === 'ReactivePath') {
      const text = pathText(e)
      if (text && /\.length$/.test(text)) {
        this.add(
          'array-length-property',
          'info',
          at,
          `\`${text}\` resolves to null and renders empty if the value is an ARRAY. Slurp has no array properties or methods; an identifier segment applied to an array is always null. (If the value is an object with a real \`length\` key, this reads fine and you can ignore this.)`,
          `To count items, pass the count through the render context, or use the \`{each}\` header's \`loop.count\` inside the loop.`,
        )
      }
      const root = pathRoot(e)
      if (root) this.letReads.push({ name: root, ...at })
    }

    /**
     * RULE: a filter argument that is not a literal.
     *
     * Verified: `${ price | currency(cur) }` with `cur = "EUR"` renders
     * ` 1,234.50` (a leading space and NO currency marker), where
     * `currency("EUR")` on 49.99 renders `€49,99`: the euro SIGN, and a comma
     * decimal separator (re-measured 2026-08-05; this comment previously
     * claimed `EUR1.234,50`, which the compiler does not produce). Only the
     * broken form above matters to the rule, but a wrong "correct" example is
     * how a reader concludes the rule itself is wrong. Filter arguments are read as
     * literals; a non-literal yields the empty string inside the filter, so each
     * filter falls back to its default or produces nothing. `default` is the one
     * filter whose argument the renderer evaluates in scope.
     */
    for (const filter of (e['filters'] as unknown[]) ?? []) {
      if (!filter || typeof filter !== 'object') continue
      const f = filter as Record<string, unknown>
      const filterName = String(f['name'] ?? '')
      const args = (f['args'] as unknown[]) ?? []
      for (const arg of args) {
        this.walkExpr(arg)
        if (filterName === FILTER_WITH_EVALUATED_ARG) continue
        const argKind = kindOf(arg)
        if (argKind && (LITERAL_KINDS as readonly string[]).includes(argKind)) continue
        const argText = pathText(arg) ?? 'that expression'
        this.add(
          'filter-arg-not-literal',
          'warning',
          posOf(arg, at),
          `\`| ${filterName}(${argText})\` passes a non-literal argument. Filter arguments are read as LITERALS, so \`${argText}\` is seen as the empty string and \`${filterName}\` silently falls back to its default or emits nothing. Nothing is reported at build time.`,
          `Pass a literal: \`| ${filterName}("USD")\`. If the argument genuinely varies, resolve it on the server and pass the finished string through the context instead. \`default\` is the only filter whose argument is evaluated in scope.`,
        )
      }
    }

    // Descend into every nested expression field.
    for (const [key, value] of Object.entries(e)) {
      if (key === 'filters' || key === 'kind') continue
      if (value && typeof value === 'object') this.walkExpr(value)
    }
    if (payload && typeof payload === 'object') this.walkExpr(payload)
  }
}

/** Expression-bearing field names across every AST node variant. */
const EXPRESSION_FIELDS = new Set([
  'condition',
  'collection',
  'value',
  'expr',
  'count',
  'target',
  'default_value',
])

function segments(expr: unknown): unknown[] | null {
  const kind = kindOf(expr)
  if (kind !== 'Path' && kind !== 'ReactivePath') return null
  const payload = kindPayload(expr)
  return Array.isArray(payload) ? payload : null
}

/** Render a path expression back to source-ish text for a message. */
function pathText(expr: unknown): string | null {
  const segs = segments(expr)
  if (!segs) {
    const kind = kindOf(expr)
    if (kind === 'StringLiteral') return JSON.stringify(kindPayload(expr))
    if (kind === 'NumberLiteral' || kind === 'BoolLiteral') return String(kindPayload(expr))
    return null
  }
  const parts: string[] = []
  for (const seg of segs) {
    const variant = asVariant(seg)
    if (!variant) continue
    if (variant.name === 'Ident') {
      parts.push(parts.length ? `.${String(variant.payload)}` : String(variant.payload))
    } else {
      parts.push(`[${pathText(variant.payload) ?? '...'}]`)
    }
  }
  return parts.length ? parts.join('') : null
}

function pathRoot(expr: unknown): string | null {
  const segs = segments(expr)
  if (!segs || segs.length === 0) return null
  const first = asVariant(segs[0])
  if (!first || first.name !== 'Ident') return null
  return String(first.payload)
}

/**
 * Is the byte at `index` inside an open HTML tag, rather than in text or in a
 * `<script>` / `<style>` body?
 *
 * THIS GUARD IS NOT DEFENSIVE PADDING, it is a fix for a real false positive.
 * Running the rule below over 130 template files from two shipped themes
 * produced exactly one finding, and it was wrong: a JavaScript assignment inside
 * a `<script>` body,
 *
 *     const accentName = "${ theme.settings.accent | default("indigo") | js }";
 *
 * matches `= "` just as an attribute does, and the inner `"` of `default("...")`
 * looks exactly like the attribute-truncating quote the rule hunts for. Inside a
 * script body it is not a bug at all: slurp lexes a script body differently, and
 * the compiler accepts that line.
 *
 * The test is positional: scanning backwards, we are inside a tag when the
 * nearest `<` is nearer than the nearest `>`. An attribute value containing a
 * literal `>` can defeat it, but only by SUPPRESSING a finding, which is the
 * safe direction for a linter.
 */
function insideOpenTag(source: string, index: number): boolean {
  return source.lastIndexOf('<', index) > source.lastIndexOf('>', index)
}

/**
 * RULE (source-level, the only one): a nested `"` inside `${ }` in a
 * double-quoted attribute ends the attribute early.
 *
 * This one cannot be an AST rule, because by the time the AST exists the damage
 * is done and the evidence is gone. Verified: `<div title="${ a["k"] }">t</div>`
 * renders `<div title=" a[" k>t</div>` with no diagnostic at all. The lexer
 * closes the attribute at the inner quote, so the rest becomes stray attributes.
 *
 * The test is deliberately narrow: a `${` inside a double-quoted ATTRIBUTE value
 * with no `}` before the closing quote. That is always broken, so it does not
 * fire on well-formed markup.
 */
function findUnterminatedAttributeSlots(source: string, file: string): LintFinding[] {
  const findings: LintFinding[] = []
  const attrStart = /=\s*"/g
  let match: RegExpExecArray | null
  while ((match = attrStart.exec(source)) !== null) {
    if (!insideOpenTag(source, match.index)) continue
    const valueStart = match.index + match[0].length
    const closing = source.indexOf('"', valueStart)
    if (closing === -1) break
    const value = source.slice(valueStart, closing)
    const slot = value.indexOf('${')
    if (slot === -1) continue
    if (value.indexOf('}', slot) !== -1) continue

    const before = source.slice(0, valueStart + slot)
    const line = before.split('\n').length
    const lastNewline = before.lastIndexOf('\n')
    const column = valueStart + slot - lastNewline
    findings.push({
      rule: 'unterminated-attribute-interpolation',
      severity: 'warning',
      file,
      line,
      column,
      message: `A \`\${\` in this double-quoted attribute value has no closing \`}\` before the attribute's own closing quote. The lexer ends an attribute at the first \`"\`, so a nested double quote inside \`\${ ... }\` (for example \`title="\${ a["k"] }"\`) truncates the attribute and turns the remainder into stray attributes. Nothing is reported at build time.`,
      fix: `Use the brace form, which is not quote-delimited: \`title={ a["k"] }\`. Single quotes inside the expression also work: \`title="\${ a['k'] }"\`.`,
    })
  }
  return findings
}

export function lintTemplate(source: string, file: string): LintResult {
  const parsed = parseSource(source, file)
  const findings = findUnterminatedAttributeSlots(source, file)

  if (parsed.ast) {
    const linter = new Linter(file)
    linter.walkNodes((parsed.ast as Record<string, unknown>)['children'])
    linter.finish()
    findings.push(...linter.findings)
  }

  findings.sort((a, b) => a.line - b.line || a.column - b.column || a.rule.localeCompare(b.rule))
  return {
    clean: findings.length === 0,
    findings,
    diagnostics: sortDiagnostics(parsed.diagnostics),
  }
}

/*
 * RULES DELIBERATELY NOT IMPLEMENTED, because checking disproved them. Older
 * notes about Slurp still assert all four; they are wrong against this compiler.
 *
 *  - "A dynamic index `items[i]` always renders empty." It does not. Verified:
 *    `${ items[i] }` with `items = ['a','b','c'], i = 1` renders `b`, and the
 *    same inside an `{each}` using the loop index renders correctly. The
 *    renderer pre-evaluates non-constant subscripts before path resolution.
 *  - "`<Image width={400}>` ignores a literal and needs a context value." It does
 *    not. Verified: both a literal `{400}` and a context value produce
 *    `w=400` in the emitted URL and `width="400"` on the `<img>`.
 *  - "The block form `{slot}fallback{/slot}` has unreachable fallback content."
 *    It does not. Verified: both the block form and the element form render
 *    `<slot name="default">fallback here</slot>`.
 *  - "An unfiltered `${ }` in a `<script>` body is silent." It is not, and it
 *    belongs to `slurp_validate` regardless: the compile-time security walk
 *    refuses it with `UnsafeScriptInterpolation`.
 */
