/**
 * Regression tests for prettier-plugin-slurp.
 *
 * Formatting runs on save by default, so each property below guards against the
 * formatter changing what a template MEANS rather than how it reads:
 *
 * - An indented `---` must never reach column 1. The lexer would read it as a
 *   frontmatter delimiter and swallow the rest of the file.
 * - The inside of a `${ }` expression must never be rewritten, including string
 *   literals that contain a brace.
 * - Formatting must stay linear in input size, so no input shape can hang
 *   `prettier --check` in CI.
 * - Whitespace-significant elements must survive verbatim.
 * - The caller's `endOfLine` must be honoured rather than silently normalised.
 */

import { describe, test, expect } from 'vitest'
import * as prettier from 'prettier'
import plugin from './index'

/** Runs the source through prettier using this plugin. */
async function format(source: string, options: prettier.Options = {}): Promise<string> {
  return prettier.format(source, {
    parser: 'slurp',
    plugins: [plugin],
    ...options,
  })
}

// ─── frontmatter delimiter promotion ─────────────────────────────────────────

describe('never promotes an indented --- to column 1', () => {
  test('an indented `---` stays indented', async () => {
    const source = ['<div>', '  --- not frontmatter ---', '</div>', ''].join('\n')
    const out = await format(source)
    expect(out.split('\n')[1]).toBe('  --- not frontmatter ---')
  })

  test('an indented horizontal rule of dashes stays indented', async () => {
    const source = ['<section>', '    --------', '</section>', ''].join('\n')
    const out = await format(source)
    expect(out.split('\n')[1]).toBe('    --------')
  })

  test('the page body survives an indented `---` inside a block', async () => {
    const source = [
      '{if show}',
      '  --- Section ---',
      '  <p>body text</p>',
      '{/if}',
      '',
    ].join('\n')
    const out = await format(source)
    // The body must still be present and must not have been absorbed.
    expect(out).toContain('<p>body text</p>')
    // And the dashes must not have reached column 1.
    expect(out).not.toMatch(/^--- Section ---$/m)
  })

  test('a real frontmatter delimiter at column 1 is preserved at column 1', async () => {
    const source = [
      '---',
      'props {',
      '  name: string',
      '}',
      '---',
      '',
      '<p>hi</p>',
      '',
    ].join('\n')
    const out = await format(source)
    const lines = out.split('\n')
    expect(lines[0]).toBe('---')
    expect(lines[4]).toBe('---')
  })

  test('frontmatter body indentation is preserved verbatim', async () => {
    const source = ['---', 'props {', '  name: string', '}', '---', '', ''].join('\n')
    const out = await format(source)
    expect(out.split('\n')[2]).toBe('  name: string')
  })
})

// ─── string literals inside ${ } ─────────────────────────────────────────────

describe('expression contents are never rewritten', () => {
  test('a closing brace inside a string literal is left alone', async () => {
    const source = '<p>${ label | default("}") }</p>\n'
    const out = await format(source)
    expect(out).toBe('<p>${ label | default("}") }</p>\n')
  })

  test('a single-quoted string containing a brace is left alone', async () => {
    const source = "<p>${ label | default('}') }</p>\n"
    const out = await format(source)
    expect(out).toBe("<p>${ label | default('}') }</p>\n")
  })

  test('spacing inside an expression is not normalised away', async () => {
    const source = '<p>${x}</p>\n'
    const out = await format(source)
    expect(out).toBe('<p>${x}</p>\n')
  })

  test('formatting is idempotent on an expression with a braced literal', async () => {
    const source = '<p>${ label | default("}") }</p>\n'
    const once = await format(source)
    const twice = await format(once)
    expect(twice).toBe(once)
  })
})

// ─── polynomial backtracking ─────────────────────────────────────────────────

describe('pathological input does not hang the formatter', () => {
  // A brace-unaware normalising regex is O(n^3) on exactly this shape, measured
  // at N=500 -> 91ms, N=1000 -> 844ms, N=2000 -> 6.98s, N=3000 -> 24.4s. N is
  // held at 3000 so that reintroducing such a regex makes this test FAIL in
  // roughly 24s rather than hanging the whole run, while a linear
  // implementation formats it instantly.
  test('an unclosed ${ followed by 3000 spaces formats promptly', async () => {
    const source = '${' + ' '.repeat(3000) + 'x\n'
    const start = Date.now()
    await format(source)
    const elapsed = Date.now() - start
    expect(elapsed).toBeLessThan(2000)
  })
})

// ─── whitespace-significant elements ─────────────────────────────────────────

describe('<pre> and <textarea> content is not trimmed', () => {
  test('leading whitespace inside <pre> is preserved', async () => {
    const source = ['<pre>', '    indented line', '        deeper', '</pre>', ''].join('\n')
    const out = await format(source)
    const lines = out.split('\n')
    expect(lines[1]).toBe('    indented line')
    expect(lines[2]).toBe('        deeper')
  })

  test('blank lines inside <pre> are preserved', async () => {
    const source = ['<pre>', '  a', '', '  b', '</pre>', ''].join('\n')
    const out = await format(source)
    expect(out.split('\n').slice(0, 5)).toEqual(['<pre>', '  a', '', '  b', '</pre>'])
  })

  test('<textarea> content is preserved', async () => {
    const source = ['<textarea>', '   spaced default', '</textarea>', ''].join('\n')
    const out = await format(source)
    expect(out.split('\n')[1]).toBe('   spaced default')
  })

  test('an element merely starting with "pre" is still formatted', async () => {
    const source = ['<presentation>', '      <p>x</p>', '</presentation>', ''].join('\n')
    const out = await format(source)
    expect(out.split('\n')[1]).toBe('<p>x</p>')
  })
})

// ─── agreement with the VS Code formatter ────────────────────────────────────

describe('{fetch} sub-blocks indent symmetrically', () => {
  test('{loading} and {error} open and close a level', async () => {
    const source = [
      '{fetch data from fn}',
      '<p>${ data }</p>',
      '{loading}',
      '<p>Loading</p>',
      '{/loading}',
      '{error}',
      '<p>Failed</p>',
      '{/error}',
      '{/fetch}',
      '',
    ].join('\n')
    const out = await format(source)
    expect(out.split('\n')).toEqual([
      '{fetch data from fn}',
      '  <p>${ data }</p>',
      '  {loading}',
      '    <p>Loading</p>',
      '  {/loading}',
      '  {error}',
      '    <p>Failed</p>',
      '  {/error}',
      '{/fetch}',
      '',
    ])
  })
})

// ─── end-of-line handling ────────────────────────────────────────────────────

describe('options.endOfLine is honoured', () => {
  test('endOfLine crlf emits CRLF', async () => {
    const out = await format('<p>a</p>\n<p>b</p>\n', { endOfLine: 'crlf' })
    expect(out).toContain('\r\n')
  })

  test('endOfLine lf strips CR from a CRLF source', async () => {
    const out = await format('<p>a</p>\r\n<p>b</p>\r\n', { endOfLine: 'lf' })
    expect(out).not.toContain('\r')
  })

  test('endOfLine auto preserves a CRLF source', async () => {
    const out = await format('<p>a</p>\r\n<p>b</p>\r\n', { endOfLine: 'auto' })
    expect(out).toContain('\r\n')
  })
})
