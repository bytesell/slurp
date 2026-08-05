/**
 * The single Slurp source formatter.
 *
 * Both formatting surfaces call THIS function and nothing else:
 * - `prettier-plugin-slurp` (used by `prettier --write` / `--check`)
 * - the VS Code extension's `DocumentFormattingEditProvider`
 *
 * Two independent line-based implementations with different dedent rules make
 * `format-on-save` and `prettier --check` disagree permanently on any file
 * containing a `{fetch}` block. Sharing one implementation makes that class of
 * divergence unrepresentable rather than merely fixed.
 *
 * ── What this formatter deliberately does NOT do ────────────────────────────
 *
 * It does not rewrite the inside of `${ ... }` expressions. Normalising that
 * spacing with a lazy, brace-unaware regex is unsafe: the Slurp lexer accepts a
 * `}` inside a string literal, so `${ label | default("}") }` comes back out as
 * `${ label | default(" }") }` - a silent change to author-supplied fallback
 * text that still compiles, and that is idempotent, so `prettier --check` stays
 * green on the corrupted file. That same regex shape is also a polynomial
 * (O(n^3)) hang on a single unclosed `${` followed by a long run of spaces.
 *
 * Cosmetic whitespace inside an expression is not worth ANY risk of silently
 * editing template content on a default-on save hook, so the normalisation is
 * omitted entirely rather than reimplemented with a scanner. Formatting only
 * ever changes leading indentation.
 */

/**
 * Names accepted directly after `{` as a block opener.
 *
 * MIRRORS `BLOCK_KEYWORDS` in `compiler/src/lexer/mod.rs`. Keep the two in
 * lockstep: a keyword the compiler treats as a block but this list omits gets
 * no indentation, and one this list invents but the compiler does not indents
 * a level that never closes.
 */
export const BLOCK_KEYWORDS: readonly string[] = [
  'if',
  'each',
  'match',
  'fetch',
  'repeat',
  'try',
  'slot',
  'layout',
  'head',
  'with',
  'error',
  'loading',
  'sections',
  'blocks',
]

/**
 * Elements whose text content is whitespace-significant.
 *
 * The compiler turns template whitespace into a literal text node and has no
 * special case for these, so trimming their lines changes what the page
 * RENDERS, not just how the source reads.
 */
const RAW_TEXT_ELEMENTS: readonly string[] = ['pre', 'textarea']

/** Line ending to emit. `auto` keeps whatever the source predominantly used. */
export type EndOfLine = 'lf' | 'crlf' | 'cr' | 'auto'

export interface SlurpFormatOptions {
  /** The string used for ONE indent level, e.g. two spaces or a tab. */
  indentUnit: string
  /** Which line ending to emit. Defaults to `lf`, matching Prettier. */
  endOfLine?: EndOfLine
}

const EOL_STRING: Record<Exclude<EndOfLine, 'auto'>, string> = {
  lf: '\n',
  crlf: '\r\n',
  cr: '\r',
}

/**
 * Splits `source` into lines while remembering which line ending dominated, so
 * `endOfLine: 'auto'` can put it back. Splitting on `\n` alone would leave a
 * stray `\r` on every line which `.trim()` then silently eats, converting CRLF
 * files to LF regardless of the caller's preference.
 */
function splitLines(source: string): { lines: string[]; dominant: string } {
  const crlf = (source.match(/\r\n/g) ?? []).length
  const bareCr = (source.match(/\r(?!\n)/g) ?? []).length
  const bareLf = (source.match(/(?<!\r)\n/g) ?? []).length

  let dominant = '\n'
  if (crlf > bareLf && crlf >= bareCr) dominant = '\r\n'
  else if (bareCr > bareLf && bareCr > crlf) dominant = '\r'

  return { lines: source.split(/\r\n|\r|\n/), dominant }
}

/**
 * True when `line` (already known to be non-blank) begins a frontmatter
 * delimiter AT COLUMN 1.
 *
 * The lexer only treats `---` as a delimiter when it starts at column 1
 * (`compiler/src/lexer/mod.rs`), and `peek_str` is a prefix test, so `-------`
 * and `--- Section ---` are delimiters too.
 */
function isFrontmatterDelimiterAtColumnOne(rawLine: string): boolean {
  return rawLine.startsWith('---')
}

/** True when the trimmed line begins with three dashes, wherever it sits. */
function startsWithDashes(trimmed: string): boolean {
  return trimmed.startsWith('---')
}

/** Counts unclosed `<pre>` / `<textarea>` openings contributed by one line. */
function rawTextDelta(line: string): number {
  let delta = 0
  for (const tag of RAW_TEXT_ELEMENTS) {
    // `(?=[\s/>])` keeps `<presentation>` from matching `<pre`.
    const opens = line.match(new RegExp(`<${tag}(?=[\\s/>])`, 'gi')) ?? []
    const closes = line.match(new RegExp(`</${tag}\\s*>`, 'gi')) ?? []
    const selfClosing = line.match(new RegExp(`<${tag}[^>]*/>`, 'gi')) ?? []
    delta += opens.length - closes.length - selfClosing.length
  }
  return delta
}

/** Matches a block close such as `{/if}` at the very start of a line. */
function closesBlock(trimmed: string): boolean {
  const m = /^\{\/([A-Za-z0-9_-]+)\}/.exec(trimmed)
  return m !== null && BLOCK_KEYWORDS.includes(m[1] as string)
}

/**
 * Returns the keyword when `trimmed` OPENS a block that is still open at the
 * end of the line, otherwise null.
 */
function opensBlock(trimmed: string): string | null {
  const m = /^\{([A-Za-z0-9_-]+)\b/.exec(trimmed)
  if (!m) return null
  const keyword = m[1] as string
  if (!BLOCK_KEYWORDS.includes(keyword)) return null
  // A block opened and closed on one line does not change the depth.
  if (trimmed.includes(`{/${keyword}}`)) return null
  return keyword
}

/**
 * Re-indents Slurp source. Only leading whitespace is ever changed, and only on
 * lines the formatter positively understands.
 *
 * @param source - The full document text.
 * @param options - Indent unit and desired line ending.
 */
export function formatSlurp(source: string, options: SlurpFormatOptions): string {
  const { indentUnit } = options
  const requested = options.endOfLine ?? 'lf'
  const { lines, dominant } = splitLines(source)
  const eol = requested === 'auto' ? dominant : EOL_STRING[requested]

  const out: string[] = []
  let depth = 0
  let inFrontmatter = false
  let rawTextDepth = 0

  for (const rawLine of lines) {
    // ── Verbatim: inside <pre> / <textarea>. Leading whitespace is content. ──
    if (rawTextDepth > 0) {
      out.push(rawLine)
      rawTextDepth = Math.max(0, rawTextDepth + rawTextDelta(rawLine))
      continue
    }

    // ── Verbatim: inside frontmatter. It is a different grammar entirely, and
    //    re-indenting it flattens `props { ... }` bodies to column 1. ──
    if (inFrontmatter) {
      out.push(rawLine)
      if (isFrontmatterDelimiterAtColumnOne(rawLine)) inFrontmatter = false
      continue
    }

    const trimmed = rawLine.trim()

    if (!trimmed) {
      out.push('')
      continue
    }

    // ── A `---` line is NEVER re-indented. ──
    //
    // Trimming every line would move an indented `---` (or `--------`, or
    // `--- Section ---`) onto column 1. The lexer then switches to
    // LexMode::Frontmatter and swallows the remainder of the file as
    // whitespace, with no lexer error and no diagnostic - and formatOnSave
    // defaults to true, so merely saving the file would destroy the page.
    //
    // Emitting the ORIGINAL line keeps a real column-1 delimiter at column 1
    // and leaves an indented one exactly where the author put it.
    if (startsWithDashes(trimmed)) {
      out.push(rawLine)
      if (isFrontmatterDelimiterAtColumnOne(rawLine)) inFrontmatter = true
      continue
    }

    if (closesBlock(trimmed)) {
      depth = Math.max(0, depth - 1)
    }

    out.push(indentUnit.repeat(depth) + trimmed)

    if (opensBlock(trimmed) !== null) {
      depth++
    }

    rawTextDepth = Math.max(0, rawTextDepth + rawTextDelta(trimmed))
  }

  return out.join(eol)
}
