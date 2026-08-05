/**
 * The committed `llms-full.txt` must match what the reference data produces.
 *
 * This is the mechanism that stops the two from drifting. `llms-full.txt` is
 * read by agents that have no MCP connection, so a stale copy is exactly the
 * failure mode this project has just spent a session removing elsewhere: a
 * confident document nobody re-runs. Generating it makes the reference tests,
 * which re-derive their claims from the real compiler, cover this file too.
 *
 * `llms.txt` is hand-written and is checked differently: its claims are
 * asserted against the compiler here, one at a time, rather than compared to a
 * generated string.
 */
import { readFileSync, existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

import { renderLlmsFull } from '../src/llms-data.js'
import { validateSource, renderTemplate } from '../src/compiler.js'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = resolve(here, '..', '..')
const llmsFull = resolve(repoRoot, 'llms-full.txt')
const llms = resolve(repoRoot, 'llms.txt')

describe('llms-full.txt', () => {
  it('exists', () => {
    expect(existsSync(llmsFull), `expected ${llmsFull}`).toBe(true)
  })

  it('matches the reference data exactly', () => {
    // Read as-is. The generator writes with \n, and a normalising read here
    // would hide a committed file with the wrong line endings.
    expect(readFileSync(llmsFull, 'utf8')).toBe(renderLlmsFull())
  })

  // The top-level heading each topic actually renders, which is not always its
  // menu title (the `expressions` topic is titled "Expression grammar" and
  // renders "# Expressions").
  it('contains every topic body, one top-level heading each', () => {
    const text = readFileSync(llmsFull, 'utf8')
    for (const heading of [
      '# Slurp',
      '# Block tags',
      '# Expressions',
      '# Filters',
      '# Elements, attributes and components',
      '# Script bodies and JavaScript attributes',
      '# Frontmatter',
      '# Section and block schemas',
      '# Error codes',
      '# Gotchas',
      '# Limits',
    ]) {
      expect(text, heading).toContain(`
${heading}
`)
    }
  })
})

describe('llms.txt', () => {
  const text = () => readFileSync(llms, 'utf8')

  it('exists and leads with the summary blockquote the convention expects', () => {
    expect(existsSync(llms), `expected ${llms}`).toBe(true)
    const lines = text().split('\n')
    expect(lines[0]).toBe('# Slurp')
    expect(lines.find((l) => l.startsWith('> '))).toBeTruthy()
  })

  it('points at llms-full.txt and at the MCP server', () => {
    expect(text()).toContain('llms-full.txt')
    expect(text()).toContain('@bytesell/slurp-mcp')
  })

  it('uses the real repository URL and no placeholder', () => {
    expect(text()).toContain('github.com/bytesell/slurp')
    expect(text()).not.toContain('disell-org')
    expect(text()).not.toContain('slurp.dev')
  })

  /*
   * The counts and lists llms.txt states are the parts most likely to rot, so
   * each is re-derived from the compiler here rather than eyeballed.
   */
  it('the filter names it lists are all real filters', () => {
    const valueFilters = 'upper lower currency truncate fixed date default plural int float js json unsafe_js'.split(' ')
    // The list is line-wrapped in the file, so check each name rather than the
    // joined string.
    const listed = text()
    for (const name of valueFilters) {
      expect(listed, `llms.txt should list the ${name} filter`).toContain(name)
    }
    for (const name of valueFilters) {
      const call = ['currency', 'truncate', 'fixed', 'date', 'plural', 'default'].includes(name)
        ? `${name}("x")`
        : name
      const out = renderTemplate(`\${ v | ${call} }`, 'f.slurp', { v: 1 }, false)
      expect(out.diagnostics.filter((d) => d.code === 'UnknownFilter'), name).toEqual([])
    }
  })

  it('is right that {with} and {case} do not exist', () => {
    expect(text()).toContain('There is NO `{with}` and no `{case}`')
    expect(validateSource('{with x}y{/with}', 'f.slurp', false).length).toBeGreaterThan(0)
    expect(
      validateSource('{match s}{case "a"}A{/case}{/match}', 'f.slurp', false).length,
    ).toBeGreaterThan(0)
  })

  it('is right that there is no `layout` frontmatter directive', () => {
    expect(text()).toContain('There is NO `layout` directive')
    // A `layout` line in frontmatter is silently skipped, not an error, which
    // is precisely why the claim needs stating.
    const source = '---\nlayout "@layouts/base"\n---\n<p>hi</p>'
    expect(validateSource(source, 'f.slurp', false)).toEqual([])
    // The directive is skipped with NO diagnostic and no layout is applied: the
    // body renders on its own. That silence is exactly why the claim is stated.
    // (The leading newline is the frontmatter terminator, not an artefact.)
    expect(renderTemplate(source, 'f.slurp', {}, false).html.trim()).toBe('<p>hi</p>')
  })

  it('is right that the script-body rule is a hard error', () => {
    expect(text()).toContain('This one IS a hard compile error')
    const diagnostics = validateSource('<script>${ v }</script>', 'f.slurp', false)
    expect(diagnostics.some((d) => d.severity === 'error')).toBe(true)
  })

  it('is right that the attribute template-literal case is only a warning', () => {
    expect(text()).toContain('Reported as a warning, not an error')
    const diagnostics = validateSource('<a :href="`/p/${s}`">x</a>', 'f.slurp', false)
    const found = diagnostics.find((d) => d.code === 'JsTemplateLiteralInAttribute')
    expect(found?.severity).toBe('warning')
  })

  it('is right that `items.length` is null and `Math.max` is null', () => {
    expect(renderTemplate('[${ items.length }]', 'f.slurp', { items: [1, 2] }, false).html).toBe('[]')
    expect(renderTemplate('[${ Math.max(1,2) }]', 'f.slurp', {}, false).html).toBe('[]')
  })

  it('is right about arithmetic coercing while comparisons do not', () => {
    expect(renderTemplate('${ "a" + 1 }', 'f.slurp', {}, false).html).toBe('a1')
    expect(renderTemplate('${ 1 / 0 }', 'f.slurp', {}, false).html).toBe('0')
    expect(renderTemplate('${ a > b }', 'f.slurp', { a: '10', b: 9 }, false).html).toBe('false')
    expect(renderTemplate('${ a == b }', 'f.slurp', { a: 1, b: '1' }, false).html).toBe('false')
  })
})

/**
 * House rules for everything this package writes, including the text it hands
 * to an agent. Asserting them beats remembering them.
 */
describe('copy style', () => {
  const files = [llms, llmsFull]
  it('uses no em-dash, en-dash or ellipsis character', () => {
    for (const file of files) {
      const content = readFileSync(file, 'utf8')
      for (const [name, ch] of [
        ['em-dash', '\u2014'],
        ['en-dash', '\u2013'],
        ['ellipsis', '\u2026'],
      ] as const) {
        const at = content.indexOf(ch)
        expect(at, `${name} in ${file} near: ${content.slice(Math.max(0, at - 60), at + 60)}`).toBe(
          -1,
        )
      }
    }
  })
})

/**
 * Slurp is a public repository, and this reference is one of its most widely
 * read surfaces: every agent using the MCP server reads it, and `llms-full.txt`
 * is generated from the same data.
 *
 * So it must not describe the private product slurp was extracted from, or any
 * incident from it. That is not a hypothetical: a specific production incident,
 * with counts, sat in the gotcha for `JsTemplateLiteralInAttribute` and was
 * carried straight into the generated artefact. The engineering lesson was
 * worth keeping and is still there, stated as a failure MODE. The incident is
 * not, and the terms below make sure it cannot return by hand or by
 * regeneration.
 *
 * `ByteSell LLC` is deliberately absent from this list: it is the licence and
 * author attribution and belongs in the package metadata.
 */
describe('no private-product disclosure', () => {
  const forbidden = [
    // Incident specifics.
    'dead links and 10',
    'in a real theme',
    // The private product's domain vocabulary.
    'merchant',
    'storefront',
    'gift card',
    'payout',
    'shopping cart',
    // The private monorepo and the developer's machine.
    'xelle',
    'C:\\Users',
    'bytesell-',
  ]

  for (const file of [llms, llmsFull]) {
    it(`${file.split(/[\\/]/).pop()} names no private product detail`, () => {
      const content = readFileSync(file, 'utf8').toLowerCase()
      for (const term of forbidden) {
        const at = content.indexOf(term.toLowerCase())
        expect(
          at,
          `found ${JSON.stringify(term)} near: ${content.slice(Math.max(0, at - 100), at + 100)}`,
        ).toBe(-1)
      }
    })
  }
})
