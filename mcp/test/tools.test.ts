/**
 * Unit tests over each tool's handler.
 *
 * THE RULE THESE FOLLOW: assert on what the COMPILER produced, never on data
 * this file constructed. A test that hand-builds a diagnostic and then checks
 * the shape of its own object proves the shape and nothing about the wiring, and
 * this repo has shipped a whole feature disconnected with every such test green.
 * So every expectation below is reached by running real source through the real
 * compiler, and the reference assertions compare the documentation against
 * values re-derived from the compiler in the same run.
 */
import { describe, expect, it } from 'vitest'

import { renderTemplate, validateSource } from '../src/compiler.js'
import { lintTemplate } from '../src/tools/lint.js'
import { templateSchema } from '../src/tools/schema.js'
import { referenceLookup } from '../src/tools/reference.js'
import { TOPICS, TOPIC_IDS } from '../src/reference/topics.js'
import { ERROR_CODES } from '../src/reference/errors.js'
import { FILTERS } from '../src/reference/filters.js'

const FILE = 'test.slurp'

describe('slurp_validate', () => {
  it('reports nothing for a valid template', () => {
    const diagnostics = validateSource('<p>${ user.name }</p>', FILE, false)
    expect(diagnostics).toEqual([])
  })

  it('returns a structured diagnostic, not a string message', () => {
    const [first] = validateSource('{each p in products}<li></li>', FILE, false)
    expect(first).toBeDefined()
    expect(first!.code).toBe('UnclosedBlock')
    expect(first!.severity).toBe('error')
    expect(first!.file).toBe(FILE)
    expect(typeof first!.line).toBe('number')
    expect(typeof first!.column).toBe('number')
  })

  it('substitutes the caller file name for the compiler placeholder', () => {
    const diagnostics = validateSource('{with x}y{/with}', 'sections/hero.slurp', false)
    expect(diagnostics.length).toBeGreaterThan(0)
    for (const d of diagnostics) {
      expect(d.file).toBe('sections/hero.slurp')
      expect(d.file).not.toBe('wasm-input')
    }
  })

  // The whole reason this server calls validate_full and not validate: the
  // security walk lives outside the parser, so a parse-only check reports this
  // file clean and the build then rejects it.
  it('includes the SECURITY WALK, not only parse diagnostics', () => {
    const diagnostics = validateSource('<script>var x = ${ name };</script>', FILE, false)
    const codes = diagnostics.map((d) => d.code)
    expect(codes).toContain('UnsafeScriptInterpolation')
  })

  it('reports a false | js position claim as an error', () => {
    const diagnostics = validateSource('<button @click="add(${ id | js })">x</button>', FILE, false)
    expect(diagnostics.map((d) => d.code)).toContain('UnsafeScriptInterpolation')
  })

  it('accepts | js when the slot really is inside a string literal', () => {
    const diagnostics = validateSource(
      "<button @click=\"add('${ id | js }')\">x</button>",
      FILE,
      false,
    )
    expect(diagnostics).toEqual([])
  })

  it('reports secret env access', () => {
    const diagnostics = validateSource('<p>${ env.SLURP_SECRET_KEY }</p>', FILE, false)
    expect(diagnostics.map((d) => d.code)).toContain('SecretEnvInTemplate')
  })

  it('honours is_middleware, because the rules genuinely differ', () => {
    const source = '<p>${ request.path }</p>'
    expect(validateSource(source, FILE, false).map((d) => d.code)).toContain(
      'MiddlewareScopeViolation',
    )
    expect(validateSource(source, FILE, true)).toEqual([])
  })

  it('reports the JS-template-literal-in-attribute case as a WARNING', () => {
    const diagnostics = validateSource(
      '<a :href="`/p/${item.slug}`">x</a>',
      FILE,
      false,
    )
    const found = diagnostics.find((d) => d.code === 'JsTemplateLiteralInAttribute')
    expect(found).toBeDefined()
    expect(found!.severity).toBe('warning')
  })
})

describe('slurp_render', () => {
  it('renders against a context', () => {
    const out = renderTemplate('<p>${ user.name }</p>', FILE, { user: { name: 'Bo' } }, false)
    expect(out.ok).toBe(true)
    expect(out.html).toBe('<p>Bo</p>')
  })

  it('HTML-escapes interpolated values', () => {
    const out = renderTemplate('<p>${ v }</p>', FILE, { v: '<script>x</script>' }, false)
    expect(out.html).not.toContain('<script>x')
    expect(out.html).toContain('&lt;script&gt;')
  })

  it('is deterministic: the same input twice gives identical bytes', () => {
    const source = '{each p in products}<li>${p.name}</li>{/each}'
    const ctx = { products: [{ name: 'a' }, { name: 'b' }] }
    expect(renderTemplate(source, FILE, ctx, false).html).toBe(
      renderTemplate(source, FILE, ctx, false).html,
    )
  })

  // Development mode is the point of this tool. Production mode drops this
  // warning entirely and silently loses the loop's tail.
  it('uses DEVELOPMENT mode, so the loop-truncation warning is visible', () => {
    const items = Array.from({ length: 1200 }, (_, i) => i)
    const out = renderTemplate('{each i in items}x{/each}', FILE, { items }, false)
    const codes = out.diagnostics.map((d) => d.code)
    expect(codes).toContain('IterationLimitExceeded')
    // Capped at 1000, so the tail really was dropped.
    expect(out.html.length).toBe(1000)
  })

  it('renders {debug}, which production mode strips with no trace', () => {
    const out = renderTemplate('{debug order}', FILE, { order: { id: 1 } }, false)
    expect(out.html).toContain('data-slurp-debug')
  })

  it('reports errors from the security walk and still returns the render', () => {
    const out = renderTemplate('<script>var x = ${ name };</script>', FILE, { name: 'Bo' }, false)
    expect(out.ok).toBe(false)
    expect(out.diagnostics.map((d) => d.code)).toContain('UnsafeScriptInterpolation')
  })

  // The security walk and the renderer both refuse an unfiltered script slot, on
  // purpose. Reporting it twice would be noise.
  it('deduplicates a diagnostic reported by both passes', () => {
    const out = renderTemplate('<script>var x = ${ name };</script>', FILE, {}, false)
    const unsafe = out.diagnostics.filter((d) => d.code === 'UnsafeScriptInterpolation')
    expect(unsafe.length).toBe(1)
  })

  it('emits a component placeholder rather than failing on an unresolvable component', () => {
    const out = renderTemplate('<Card title={t} />', FILE, { t: 'Hi' }, false)
    expect(out.html).toContain('data-slurp-component="Card"')
  })

  it('performs no network request for {fetch}: the branch comes from the context', () => {
    const source = '{fetch products from "/api/products"}HAS{loading}LOADING{/fetch}'
    expect(renderTemplate(source, FILE, {}, false).html).toBe('LOADING')
    expect(renderTemplate(source, FILE, { products: [{ n: 1 }] }, false).html).toBe('HAS')
  })
})

describe('slurp_lint', () => {
  it('catches a quoted component prop carrying interpolation syntax', () => {
    const result = lintTemplate('<Card title="Hi ${name}" />', FILE)
    expect(result.clean).toBe(false)
    expect(result.findings.map((f) => f.rule)).toContain('component-prop-literal-interpolation')
  })

  // Proves the rule is about the ASYMMETRY and not just about seeing "${" in a
  // quoted value. The identical text on an element is correct and must not fire.
  it('does NOT fire on the same text in an element attribute, which interpolates', () => {
    const result = lintTemplate('<span data-x="Hi ${name}"></span>', FILE)
    expect(result.findings.map((f) => f.rule)).not.toContain(
      'component-prop-literal-interpolation',
    )
  })

  it('does not fire on the correct template-literal prop form', () => {
    const result = lintTemplate('<Card title={`Hi ${name}`} />', FILE)
    expect(result.clean).toBe(true)
  })

  it('catches a non-literal filter argument', () => {
    const result = lintTemplate('<p>${ price | currency(cur) }</p>', FILE)
    expect(result.findings.map((f) => f.rule)).toContain('filter-arg-not-literal')
  })

  it('does not fire on a literal filter argument', () => {
    const result = lintTemplate('<p>${ price | currency("EUR") }</p>', FILE)
    expect(result.clean).toBe(true)
  })

  it('exempts `default`, the one filter whose argument is evaluated in scope', () => {
    const result = lintTemplate('<p>${ nickname | default(user.name) }</p>', FILE)
    expect(result.findings.map((f) => f.rule)).not.toContain('filter-arg-not-literal')
  })

  it('catches a function call, which always evaluates to null', () => {
    const result = lintTemplate('<p>${ Math.max(a, b) }</p>', FILE)
    expect(result.findings.map((f) => f.rule)).toContain('call-always-null')
  })

  it('catches a {$let} name read back as a template value', () => {
    const result = lintTemplate('{$let count = 0}<p>${ count }</p>', FILE)
    expect(result.findings.map((f) => f.rule)).toContain('let-binding-not-readable')
  })

  it('does not fire on a {$let} whose name is never read back', () => {
    const result = lintTemplate('{$let count = 0}<p>ok</p>', FILE)
    expect(result.clean).toBe(true)
  })

  it('catches .length on a path', () => {
    const result = lintTemplate('<p>${ items.length }</p>', FILE)
    expect(result.findings.map((f) => f.rule)).toContain('array-length-property')
  })

  it('catches an unquoted number in a component prop position', () => {
    const result = lintTemplate('<Card count=123 />', FILE)
    expect(result.findings.map((f) => f.rule)).toContain('component-prop-unquoted-number')
  })

  it('catches a nested double quote that ends its attribute early', () => {
    const result = lintTemplate('<div title="${ a["k"] }">t</div>', FILE)
    expect(result.findings.map((f) => f.rule)).toContain('unterminated-attribute-interpolation')
  })

  it('does not fire on the brace form of the same expression', () => {
    const result = lintTemplate('<div title={ a["k"] }>t</div>', FILE)
    expect(result.findings.map((f) => f.rule)).not.toContain(
      'unterminated-attribute-interpolation',
    )
  })

  /*
   * A regression from a real theme file, and the reason the rule now checks that
   * it is inside an open tag. A JavaScript assignment in a <script> body matches
   * `= "` exactly as an attribute does, and the inner quote of `default("...")`
   * looks exactly like the attribute-truncating quote the rule hunts for. It is
   * not a bug: the compiler accepts this line, which is asserted below so the
   * two claims cannot come apart.
   */
  it('does not fire on a JavaScript assignment inside a <script> body', () => {
    const source = [
      '<script>',
      '  const accentName = "${theme.settings.accent | default("indigo") | js}";',
      '</script>',
    ].join('\n')
    expect(lintTemplate(source, FILE).findings.map((f) => f.rule)).not.toContain(
      'unterminated-attribute-interpolation',
    )
    expect(validateSource(source, FILE, false).filter((d) => d.severity === 'error')).toEqual([])
  })

  it('every finding carries a concrete fix and a position', () => {
    const result = lintTemplate('<Card title="Hi ${name}" />', FILE)
    for (const f of result.findings) {
      expect(f.fix.length).toBeGreaterThan(20)
      expect(f.file).toBe(FILE)
      expect(typeof f.line).toBe('number')
    }
  })

  it('stays quiet on an ordinary correct template', () => {
    const source = [
      '{each product, i in products | limit(12)}',
      '  <li>${ i }: ${ product.name | upper }</li>',
      '{empty}',
      '  <li>None</li>',
      '{/each}',
    ].join('\n')
    expect(lintTemplate(source, FILE).clean).toBe(true)
  })

  /*
   * These four are the rules this linter deliberately does NOT have, because
   * checking against the compiler disproved the claim. They are asserted here so
   * that anyone re-adding a rule from a stale document is told immediately.
   */
  describe('constructs that older documents call broken and that actually work', () => {
    it('a dynamic index resolves correctly', () => {
      const out = renderTemplate('${ items[i] }', FILE, { items: ['a', 'b', 'c'], i: 1 }, false)
      expect(out.html).toBe('b')
      expect(lintTemplate('${ items[i] }', FILE).clean).toBe(true)
    })

    it('an <Image> honours a literal width', () => {
      const out = renderTemplate(
        '<Image src={s} alt="a" width={400} height={300} />',
        FILE,
        { s: 'https://example.com/a.png' },
        false,
      )
      expect(out.html).toContain('w=400')
      expect(out.html).toContain('width="400"')
    })

    it('the block form of {slot} renders its fallback', () => {
      const out = renderTemplate('{slot}fallback here{/slot}', FILE, {}, false)
      expect(out.html).toContain('fallback here')
    })
  })
})

describe('slurp_schema', () => {
  const SECTION = [
    '---',
    'using "@components/Card"',
    'using "./Btn" as Button',
    'section {',
    '  name: "Hero"',
    '  settings {',
    '    heading: text = "Hi" { label: "Heading" }',
    '    n: number = 8 { min: 1, max: 24 }',
    '    align: select(left, center) = center',
    '  }',
    '  blocks {',
    '    @theme(heading, button)',
    '    faq { max: 12 settings { q: text } }',
    '  }',
    '  max_per_page: 3',
    '}',
    '---',
    '<h1>${ section.settings.heading }</h1>',
  ].join('\n')

  it('extracts the section schema with settings, defaults and meta', () => {
    const result = templateSchema(SECTION, 'sections/hero.slurp')
    expect(result.section).not.toBeNull()
    const schema = result.section as Record<string, unknown>
    expect(schema['name']).toBe('Hero')
    expect(schema['max_per_page']).toBe(3)
    const settings = schema['settings'] as Array<Record<string, unknown>>
    expect(settings.map((s) => s['key'])).toEqual(['heading', 'n', 'align'])
    expect(settings[1]!['meta']).toEqual({ min: 1, max: 24 })
    expect(settings[2]!['options']).toEqual(['left', 'center'])
  })

  it('extracts nested blocks and @theme targeting', () => {
    const schema = templateSchema(SECTION, FILE).section as Record<string, unknown>
    expect(schema['accepts']).toEqual({ allow: ['heading', 'button'] })
    const blocks = schema['blocks'] as Array<Record<string, unknown>>
    expect(blocks[0]!['type']).toBe('faq')
    expect(blocks[0]!['max']).toBe(12)
  })

  it('resolves import local names, honouring an alias', () => {
    const result = templateSchema(SECTION, FILE)
    expect(result.imports).toEqual([
      { path: '@components/Card', localName: 'Card' },
      { path: './Btn', localName: 'Button' },
    ])
  })

  // extract_schema reads only frontmatter.section, so a block file comes back
  // null there. This is why the tool also reads the parsed AST.
  it('extracts a block schema, which extract_schema alone does not return', () => {
    const source = [
      '---',
      'block {',
      '  name: "Button"',
      '  settings { label: text = "Click" }',
      '}',
      '---',
      '<button>${ block.settings.label }</button>',
    ].join('\n')
    const result = templateSchema(source, 'blocks/button.slurp')
    expect(result.section).toBeNull()
    expect(result.block).not.toBeNull()
    expect((result.block as Record<string, unknown>)['name']).toBe('Button')
    expect(result.blockTypeNote).toContain('button')
  })

  it('reads props, marking optionality and defaults separately', () => {
    const source = [
      '---',
      'props {',
      '  title: string',
      '  count?: number',
      '  featured: bool = false',
      '}',
      '---',
      '<p>${title}</p>',
    ].join('\n')
    expect(templateSchema(source, FILE).props).toEqual([
      { name: 'title', type: 'string', required: true, hasDefault: false },
      { name: 'count', type: 'number', required: false, hasDefault: false },
      { name: 'featured', type: 'bool', required: true, hasDefault: true },
    ])
  })

  it('returns nulls rather than an error for a template with no schema', () => {
    const result = templateSchema('<p>hello</p>', FILE)
    expect(result.section).toBeNull()
    expect(result.block).toBeNull()
    expect(result.diagnostics).toEqual([])
  })
})

describe('slurp_reference', () => {
  it('returns the index plus the overview with no arguments', () => {
    const out = referenceLookup()
    expect(out).toContain('Slurp reference')
    for (const id of TOPIC_IDS) expect(out).toContain(`\`${id}\``)
  })

  it('returns each topic non-empty', () => {
    for (const topic of TOPICS) {
      const out = referenceLookup(topic.id)
      expect(out.length, `topic ${topic.id}`).toBeGreaterThan(200)
      expect(out.startsWith('#'), `topic ${topic.id}`).toBe(true)
    }
  })

  it('answers an error code with no topic, which is what a diagnostic gives you', () => {
    const out = referenceLookup(undefined, 'UnsafeScriptInterpolation')
    expect(out).toContain('# UnsafeScriptInterpolation')
    expect(out).toContain('Fix')
  })

  it('answers a filter name with no topic', () => {
    const out = referenceLookup(undefined, 'currency')
    expect(out).toContain('currency(code')
  })

  it('says plainly when a declared code is never emitted', () => {
    const out = referenceLookup(undefined, 'UnknownComponent')
    expect(out).toContain('NEVER EMITS')
  })

  it('narrows within a topic', () => {
    const full = referenceLookup('filters')
    const narrowed = referenceLookup('filters', 'truncate')
    expect(narrowed.length).toBeLessThan(full.length)
    expect(narrowed).toContain('truncate')
  })

  it('names the unknown topic rather than failing silently', () => {
    expect(referenceLookup('nope')).toContain('Unknown topic')
  })
})

/*
 * These are the tests that keep the REFERENCE honest rather than the code. Each
 * one re-derives a documented claim from the real compiler in this same run, so
 * a compiler change that invalidates the documentation turns the suite red
 * instead of leaving a plausible sentence in place.
 */
describe('the reference agrees with the compiler', () => {
  it('every documented value filter is accepted by the compiler', () => {
    for (const filter of FILTERS.filter((f) => f.kind === 'value')) {
      // Give each one a literal argument where its signature takes one.
      const call = filter.signature.includes('(')
        ? `${filter.name}(${filter.name === 'plural' ? '"item"' : filter.name === 'date' ? '"YYYY"' : filter.name === 'currency' ? '"USD"' : '2'})`
        : filter.name
      const out = renderTemplate(`\${ v | ${call} }`, FILE, { v: 1 }, false)
      const unknown = out.diagnostics.filter((d) => d.code === 'UnknownFilter')
      expect(unknown, `value filter ${filter.name}`).toEqual([])
    }
  })

  it('every documented loop filter is accepted by the compiler', () => {
    const args: Record<string, string> = {
      limit: '(2)',
      sort: '("k")',
      reverse: '',
      filter: '("k", 1)',
    }
    for (const filter of FILTERS.filter((f) => f.kind === 'loop')) {
      const out = renderTemplate(
        `{each x in items | ${filter.name}${args[filter.name] ?? ''}}y{/each}`,
        FILE,
        { items: [{ k: 1 }] },
        false,
      )
      const unknown = out.diagnostics.filter((d) => d.code === 'UnknownFilter')
      expect(unknown, `loop filter ${filter.name}`).toEqual([])
    }
  })

  it('a value filter name in an {each} header really is an error', () => {
    const out = renderTemplate('{each x in items | upper}y{/each}', FILE, { items: [1] }, false)
    expect(out.diagnostics.map((d) => d.code)).toContain('UnknownFilter')
  })

  it('documents 13 value filters and 4 loop filters', () => {
    expect(FILTERS.filter((f) => f.kind === 'value')).toHaveLength(13)
    expect(FILTERS.filter((f) => f.kind === 'loop')).toHaveLength(4)
  })

  it('documents 21 error codes, of which 13 are marked emitted', () => {
    expect(ERROR_CODES).toHaveLength(21)
    expect(ERROR_CODES.filter((e) => e.emitted)).toHaveLength(13)
  })

  it('every error code marked emitted carries a fix', () => {
    for (const e of ERROR_CODES.filter((c) => c.emitted)) {
      expect(e.cause.length, e.code).toBeGreaterThan(20)
      expect(e.fix, e.code).toBeDefined()
    }
  })

  // The documented example pairs are worth nothing if the "right" one is also
  // broken. Each is compiled here.
  it('every "right" example in the error reference validates clean', () => {
    for (const e of ERROR_CODES) {
      // Some "right" entries are prose advice rather than source. Compile only
      // the ones that are actually template syntax.
      if (!e.right || !/[<${]/.test(e.right)) continue
      const diagnostics = validateSource(e.right, FILE, false)
      const errors = diagnostics.filter((d) => d.severity === 'error')
      expect(errors, `right-hand example for ${e.code}: ${e.right}`).toEqual([])
    }
  })

  it('every "wrong" example that claims an error really produces one', () => {
    const shouldError = ['UnclosedBlock', 'UnsafeScriptInterpolation', 'SecretEnvInTemplate', 'MiddlewareScopeViolation']
    for (const e of ERROR_CODES) {
      if (!e.wrong || !shouldError.includes(e.code)) continue
      const diagnostics = validateSource(e.wrong, FILE, false)
      expect(diagnostics.map((d) => d.code), `wrong example for ${e.code}`).toContain(e.code)
    }
  })

  it('the documented truthiness rule matches the compiler', () => {
    const cases: Array<[unknown, boolean]> = [
      [null, false],
      [false, false],
      [0, false],
      ['', false],
      [[], false],
      [{}, false],
      ['0', true],
      ['false', true],
      [[1], true],
      [{ a: 1 }, true],
    ]
    for (const [value, truthy] of cases) {
      const out = renderTemplate('{if v}Y{else}N{/if}', FILE, { v: value }, false)
      expect(out.html, `truthiness of ${JSON.stringify(value)}`).toBe(truthy ? 'Y' : 'N')
    }
  })

  it('the documented operator semantics match the compiler', () => {
    const check = (expr: string, ctx: Record<string, unknown>, expected: string) => {
      expect(renderTemplate(`\${ ${expr} }`, FILE, ctx, false).html, expr).toBe(expected)
    }
    check('2 + 3 * 4', {}, '14')                 // precedence
    check('"a" + 1', {}, 'a1')                   // + concatenates when either side is a string
    check('1 / 0', {}, '0')                      // division by zero yields 0
    check('a == b', { a: 1, b: '1' }, 'false')   // == is type-strict
    check('a > b', { a: '10', b: 9 }, 'false')   // comparisons do not coerce
    check('a || b', { a: '', b: 'fallback' }, 'fallback') // returns the operand
  })

  it('the documented loop variables match the compiler', () => {
    const out = renderTemplate(
      '{each x, i in items}${i}/${loop.index}/${loop.count}/${loop.first}/${loop.last} {/each}',
      FILE,
      { items: ['a', 'b'] },
      false,
    )
    // loop.count is the TOTAL, not index+1.
    expect(out.html).toBe('0/0/2/true/false 1/1/2/false/true ')
  })

  it('the documented | js and | json difference matches the compiler', () => {
    expect(renderTemplate('<script>${ v | js }</script>', FILE, { v: 'Bo' }, false).html).toBe(
      '<script>Bo</script>',
    )
    expect(renderTemplate('<script>${ v | json }</script>', FILE, { v: 'Bo' }, false).html).toBe(
      '<script>"Bo"</script>',
    )
  })

  it('the documented `date` passthrough on non-ISO input matches the compiler', () => {
    const out = renderTemplate('${ v | date("MMM DD, YYYY") }', FILE, { v: '1700000000000' }, false)
    expect(out.html).toBe('1700000000000')
  })

  it('the documented `plural` output really includes the number', () => {
    expect(renderTemplate('${ n | plural("item") }', FILE, { n: 5 }, false).html).toBe('5 items')
    expect(renderTemplate('${ n | plural("item") }', FILE, { n: 1 }, false).html).toBe('1 item')
  })

  it('the documented `{with}` and `{case}` claims match the compiler', () => {
    expect(validateSource('{with x}y{/with}', FILE, false).length).toBeGreaterThan(0)
    expect(validateSource('{match s}{case "a"}A{/case}{/match}', FILE, false).length).toBeGreaterThan(0)
    expect(validateSource('{match s}\n"a" -> A\n_ -> B\n{/match}', FILE, false)).toEqual([])
  })

  it('the documented "no array literal" claim matches the compiler', () => {
    expect(validateSource('${ [1, 2] }', FILE, false).length).toBeGreaterThan(0)
  })
})
