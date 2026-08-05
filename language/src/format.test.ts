/**
 * Direct unit tests for the shared Slurp formatter.
 *
 * These exercise `formatSlurp` itself, which is what the VS Code extension
 * calls. The prettier-plugin tests go through Prettier, and Prettier applies
 * `endOfLine` normalisation AFTER the printer runs, so the line-ending
 * behaviour below is only observable here.
 */

import { describe, test, expect } from 'vitest'
import { formatSlurp, BLOCK_KEYWORDS } from './format'

const OPTS = { indentUnit: '  ' }

// ─── frontmatter delimiter promotion ─────────────────────────────────────────

describe('an indented --- is never promoted to column 1', () => {
  test('indented dashes keep their original indentation', () => {
    const out = formatSlurp('<div>\n  --- not frontmatter ---\n</div>', OPTS)
    expect(out.split('\n')[1]).toBe('  --- not frontmatter ---')
  })

  test('a long dash run stays indented', () => {
    const out = formatSlurp('<div>\n\t--------\n</div>', OPTS)
    expect(out.split('\n')[1]).toBe('\t--------')
  })

  test('a column-1 delimiter opens frontmatter and stays at column 1', () => {
    const out = formatSlurp('---\nprops {\n  a: string\n}\n---\n<p>x</p>', OPTS)
    expect(out.split('\n')).toEqual([
      '---',
      'props {',
      '  a: string',
      '}',
      '---',
      '<p>x</p>',
    ])
  })

  test('content after frontmatter is still indented normally', () => {
    const out = formatSlurp('---\nx\n---\n{if a}\n<p>y</p>\n{/if}', OPTS)
    expect(out.split('\n')).toEqual(['---', 'x', '---', '{if a}', '  <p>y</p>', '{/if}'])
  })

  test('an indented --- inside a block does not swallow the rest of the file', () => {
    const out = formatSlurp('{if a}\n  --- x ---\n  <p>body</p>\n{/if}', OPTS)
    expect(out).toContain('<p>body</p>')
    expect(out.split('\n')[1]).toBe('  --- x ---')
  })
})

// ─── expression bodies ───────────────────────────────────────────────────────

describe('expression bodies are never rewritten', () => {
  test('a brace inside a string literal survives', () => {
    const src = '<p>${ label | default("}") }</p>'
    expect(formatSlurp(src, OPTS)).toBe(src)
  })

  test('expression spacing is left exactly as authored', () => {
    expect(formatSlurp('<p>${x}</p>', OPTS)).toBe('<p>${x}</p>')
    expect(formatSlurp('<p>${    x    }</p>', OPTS)).toBe('<p>${    x    }</p>')
  })

  test('only leading whitespace ever changes', () => {
    const out = formatSlurp('{if a}\n<p>${ a|b("}") }</p>\n{/if}', OPTS)
    expect(out.split('\n')[1]).toBe('  <p>${ a|b("}") }</p>')
  })
})

// ─── linear-time formatting ──────────────────────────────────────────────────

describe('formatting is linear in input size', () => {
  test('a long unclosed ${ run is formatted promptly', () => {
    const src = '${' + ' '.repeat(50000) + 'x'
    const start = Date.now()
    formatSlurp(src, OPTS)
    expect(Date.now() - start).toBeLessThan(1000)
  })
})

// ─── whitespace-significant elements ─────────────────────────────────────────

describe('whitespace-significant elements are verbatim', () => {
  test('<pre> body keeps leading whitespace and blank lines', () => {
    const out = formatSlurp('<pre>\n    a\n\n        b\n</pre>', OPTS)
    expect(out.split('\n')).toEqual(['<pre>', '    a', '', '        b', '</pre>'])
  })

  test('<textarea> body keeps leading whitespace', () => {
    const out = formatSlurp('<textarea>\n   v\n</textarea>', OPTS)
    expect(out.split('\n')[1]).toBe('   v')
  })

  test('a <pre> nested inside a block still shields its body', () => {
    const out = formatSlurp('{if a}\n<pre>\n   keep\n</pre>\n{/if}', OPTS)
    expect(out.split('\n')).toEqual(['{if a}', '  <pre>', '   keep', '</pre>', '{/if}'])
  })

  test('a one-line <pre> does not open a verbatim region', () => {
    const out = formatSlurp('<pre>x</pre>\n{if a}\n<p>y</p>\n{/if}', OPTS)
    expect(out.split('\n')[2]).toBe('  <p>y</p>')
  })

  test('<presentation> is not mistaken for <pre>', () => {
    const out = formatSlurp('<presentation>\n      <p>x</p>\n</presentation>', OPTS)
    expect(out.split('\n')[1]).toBe('<p>x</p>')
  })
})

// ─── one keyword set for open and close ──────────────────────────────────────

describe('one keyword set drives open and close', () => {
  test('the keyword list mirrors the compiler lexer BLOCK_KEYWORDS', () => {
    // compiler/src/lexer/mod.rs BLOCK_KEYWORDS, verbatim.
    expect([...BLOCK_KEYWORDS].sort()).toEqual(
      [
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
      ].sort(),
    )
  })

  test('{loading} and {error} indent symmetrically inside {fetch}', () => {
    const src = [
      '{fetch d from fn}',
      '<p>a</p>',
      '{loading}',
      '<p>l</p>',
      '{/loading}',
      '{error}',
      '<p>e</p>',
      '{/error}',
      '{/fetch}',
    ].join('\n')
    expect(formatSlurp(src, OPTS).split('\n')).toEqual([
      '{fetch d from fn}',
      '  <p>a</p>',
      '  {loading}',
      '    <p>l</p>',
      '  {/loading}',
      '  {error}',
      '    <p>e</p>',
      '  {/error}',
      '{/fetch}',
    ])
  })

  test('an unknown close tag neither dedents nor throws', () => {
    const out = formatSlurp('{if a}\n{/nonsense}\n<p>x</p>\n{/if}', OPTS)
    expect(out.split('\n')).toEqual(['{if a}', '  {/nonsense}', '  <p>x</p>', '{/if}'])
  })

  test('a block opened and closed on one line does not indent', () => {
    const out = formatSlurp('{if a}<p>x</p>{/if}\n<p>y</p>', OPTS)
    expect(out.split('\n')[1]).toBe('<p>y</p>')
  })

  test('depth never goes negative on an unbalanced close', () => {
    const out = formatSlurp('{/if}\n{/if}\n<p>x</p>', OPTS)
    expect(out.split('\n')[2]).toBe('<p>x</p>')
  })
})

// ─── line endings ────────────────────────────────────────────────────────────

describe('endOfLine is honoured instead of silently normalising', () => {
  test('lf is the default and strips CR from a CRLF source', () => {
    expect(formatSlurp('<p>a</p>\r\n<p>b</p>', OPTS)).toBe('<p>a</p>\n<p>b</p>')
  })

  test('crlf emits CRLF', () => {
    expect(formatSlurp('<p>a</p>\n<p>b</p>', { ...OPTS, endOfLine: 'crlf' })).toBe(
      '<p>a</p>\r\n<p>b</p>',
    )
  })

  test('auto preserves a CRLF source', () => {
    expect(formatSlurp('<p>a</p>\r\n<p>b</p>', { ...OPTS, endOfLine: 'auto' })).toBe(
      '<p>a</p>\r\n<p>b</p>',
    )
  })

  test('auto preserves an LF source', () => {
    expect(formatSlurp('<p>a</p>\n<p>b</p>', { ...OPTS, endOfLine: 'auto' })).toBe(
      '<p>a</p>\n<p>b</p>',
    )
  })

  test('a stray CR does not survive as trailing whitespace under lf', () => {
    const out = formatSlurp('{if a}\r\n<p>x</p>\r\n{/if}', OPTS)
    expect(out).toBe('{if a}\n  <p>x</p>\n{/if}')
    expect(out).not.toContain('\r')
  })
})

// ─── Indent unit ─────────────────────────────────────────────────────────────

describe('indent unit', () => {
  test('tabs are used when requested', () => {
    const out = formatSlurp('{if a}\n<p>x</p>\n{/if}', { indentUnit: '\t' })
    expect(out.split('\n')[1]).toBe('\t<p>x</p>')
  })

  test('a four-space unit is used when requested', () => {
    const out = formatSlurp('{if a}\n<p>x</p>\n{/if}', { indentUnit: '    ' })
    expect(out.split('\n')[1]).toBe('    <p>x</p>')
  })

  test('blank lines are emitted empty, not padded', () => {
    const out = formatSlurp('{if a}\n\n<p>x</p>\n{/if}', OPTS)
    expect(out.split('\n')[1]).toBe('')
  })
})
