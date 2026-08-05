/**
 * Compiler integration tests, run against the REAL WebAssembly build.
 *
 * These exercise the six exports of `compiler/src/wasm/mod.rs` end to end:
 * `parse`, `render`, `render_dev`, `validate`, `extract_schema` and
 * `render_section`. That is the framework's browser and editor API, and until
 * the WASM package existed it had no end-to-end coverage at all.
 *
 * ── HISTORY, because it explains the shape of this file ──────────────────────
 * This suite used to run against a hardcoded stub that returned
 * `{ ast: {}, errors: [], warnings: [] }` for every input, and the whole file
 * was skipped behind a `SKIPPED_PENDING_WASM` binding. 62 of its tests could not
 * pass against that stub and the rest only asserted that an empty array was
 * empty. It was a specification, not coverage.
 *
 * Wiring the real module up showed that a number of those assertions described
 * an API that never existed: `{case}` arms, `math.*` helpers, `cache:60` option
 * syntax, spread props, `{slot name="x"}` fallbacks, an `<Image>` except-list.
 * Each is now written against what the compiler actually does, with a comment
 * naming what the old assertion assumed and why it was wrong. Where the real
 * behaviour is a documented gap rather than a bug, the test pins the gap so a
 * future change to it is a deliberate one.
 *
 * ── THE CONTRACT THAT CATCHES EVERYONE ───────────────────────────────────────
 * Every WASM export returns a JSON **string**, not an object. See `parseJson`
 * below and `compiler-wasm/README.md`.
 */

import { describe, test, expect } from 'vitest'
import {
  parse as wasmParse,
  render as wasmRender,
  render_dev as wasmRenderDev,
  validate as wasmValidate,
  extract_schema as wasmExtractSchema,
  render_section as wasmRenderSection,
  parseJson,
} from '@slurp/compiler-wasm'
import type { ParseResult, RenderResult, SchemaResult, SlurpError } from '../../shared/interfaces'

// ─── Typed wrappers ───────────────────────────────────────────────────────────

const parse = (source: string): ParseResult => parseJson(wasmParse(source))
const render = (ast: unknown, context: unknown = {}): RenderResult =>
  parseJson(wasmRender(JSON.stringify(ast), JSON.stringify(context)))
const renderDev = (ast: unknown, context: unknown = {}): RenderResult =>
  parseJson(wasmRenderDev(JSON.stringify(ast), JSON.stringify(context)))
const validate = (source: string): SlurpError[] => parseJson(wasmValidate(source))
const extractSchema = (source: string): SchemaResult => parseJson(wasmExtractSchema(source))
const renderSection = (ast: unknown, state: unknown, context: unknown = {}): RenderResult =>
  parseJson(wasmRenderSection(JSON.stringify(ast), JSON.stringify(state), JSON.stringify(context)))

// ─── Helpers ─────────────────────────────────────────────────────────────────

/** Parse source and assert no errors. Returns the AST. */
function parseOk(source: string): unknown {
  const result = parse(source)
  expect(result.errors, `Parse errors: ${JSON.stringify(result.errors)}`).toHaveLength(0)
  expect(result.ok).toBe(true)
  return result.ast
}

/** Parse source and assert at least one error carries the given code. */
function parseExpectError(source: string, code: string): SlurpError[] {
  const result = parse(source)
  expect(
    result.errors.map((e) => e.code),
    `Expected a ${code} error`,
  ).toContain(code)
  expect(result.ok).toBe(false)
  return result.errors
}

/** Render an AST with a context and return HTML. Asserts no render errors. */
function renderOk(ast: unknown, context: unknown = {}): string {
  const result = render(ast, context)
  expect(result.errors, `Render errors: ${JSON.stringify(result.errors)}`).toHaveLength(0)
  return result.html
}

/** Full pipeline: parse then render with a context. */
function compile(source: string, context: unknown = {}): string {
  return renderOk(parseOk(source), context)
}

/** Collapse runs of whitespace, for comparing output that differs only in indentation. */
const squash = (html: string): string => html.replace(/\s+/g, ' ').trim()

// The AST serialises Rust enums in serde's default external tagging, so a node
// is `{ VariantName: { ...fields } }` and a unit variant is the bare string
// `"VariantName"`. The old tests assumed flat objects with a `type` field.
type Node = Record<string, Record<string, unknown>>
const nodesOf = (ast: unknown): Node[] => (ast as { children: Node[] }).children
const firstNode = (ast: unknown): Node => nodesOf(ast)[0]!

// ─── The JSON-string contract ────────────────────────────────────────────────

describe('Compiler - WASM calling convention', () => {
  test('every export returns a JSON string, not an object', () => {
    // The single most repeated mistake against this API. `JsValue::from_str` in
    // compiler/src/wasm/mod.rs means the caller must JSON.parse the result; a
    // property read on the raw return value silently yields undefined.
    const ast = JSON.stringify(parse('<p>hi</p>').ast)
    expect(typeof wasmParse('<p>hi</p>')).toBe('string')
    expect(typeof wasmRender(ast, '{}')).toBe('string')
    expect(typeof wasmRenderDev(ast, '{}')).toBe('string')
    expect(typeof wasmValidate('<p>hi</p>')).toBe('string')
    expect(typeof wasmExtractSchema('<p>hi</p>')).toBe('string')
    expect(typeof wasmRenderSection(ast, '{}', '{}')).toBe('string')
  })

  test('results carry an ok flag that tracks error severity', () => {
    // `ok` is emitted by three of the result types and was declared by no
    // TypeScript interface until this suite was wired up.
    expect(parse('<p>fine</p>').ok).toBe(true)
    expect(parse('{if unclosed}x').ok).toBe(false)
    expect(extractSchema('<p>fine</p>').ok).toBe(true)
    expect(render(parseOk('<p>fine</p>')).ok).toBe(true)
  })

  test('malformed AST and context JSON are reported, never thrown', () => {
    const badAst = parseJson<RenderResult>(wasmRender('not json', '{}'))
    expect(badAst.ok).toBe(false)
    expect(badAst.errors.map((e) => e.code)).toContain('InvalidAst')
    expect(badAst.html).toBe('')

    const badCtx = parseJson<RenderResult>(
      wasmRender(JSON.stringify(parseOk('<p>x</p>')), 'not json'),
    )
    expect(badCtx.ok).toBe(false)
    expect(badCtx.errors.map((e) => e.code)).toContain('InvalidContext')

    const badState = parseJson<RenderResult>(
      wasmRenderSection(JSON.stringify(parseOk('<p>x</p>')), 'not json', '{}'),
    )
    expect(badState.ok).toBe(false)
    expect(badState.errors.map((e) => e.code)).toContain('InvalidSectionState')
  })

  test('html is always a string, empty rather than null on failure', () => {
    // The TypeScript interface used to declare `html: string | null`. The Rust
    // field is a `String` and has no null path.
    const failed = parseJson<RenderResult>(wasmRender('not json', '{}'))
    expect(failed.html).toBe('')
    expect(failed.html).not.toBeNull()
  })
})

// ─── Frontmatter ─────────────────────────────────────────────────────────────

describe('Compiler - Frontmatter', () => {
  // `imports` is a Rust enum, so each entry is `{ Default: {...} }` or
  // `{ Types: {...} }`. The old tests read `i.path` off the entry itself, which
  // is always undefined.
  type ImportEntry = {
    Default?: { path: string; alias: string | null }
    Types?: { path: string; names: string[] }
  }
  const importsOf = (ast: unknown): ImportEntry[] =>
    (ast as { frontmatter: { imports: ImportEntry[] } }).frontmatter.imports

  test('parses using import', () => {
    const ast = parseOk('---\nusing "@components/Navbar"\n---\n<Navbar />')
    const imp = importsOf(ast).find((i) => i.Default?.path === '@components/Navbar')
    expect(imp).toBeDefined()
    expect(imp?.Default?.alias).toBeNull()
  })

  test('parses using with alias', () => {
    const ast = parseOk('---\nusing "@components/Button" as Btn\n---\n<Btn />')
    const imp = importsOf(ast).find((i) => i.Default?.path === '@components/Button')
    expect(imp?.Default?.alias).toBe('Btn')
  })

  test('parses type import', () => {
    const ast = parseOk(
      '---\nusing type { Review } from "@components/ReviewCard"\n---\n<p>ok</p>',
    )
    const imp = importsOf(ast).find((i) => i.Types?.path === '@components/ReviewCard')
    expect(imp?.Types?.names).toContain('Review')
  })

  test('parses type definitions', () => {
    const source = `---
type Product {
  id: string
  price: number
  featured?: boolean
}
---
<p>ok</p>`
    const ast = parseOk(source) as {
      frontmatter: { type_defs: Array<{ name: string; fields: Array<Record<string, unknown>> }> }
    }
    const def = ast.frontmatter.type_defs.find((t) => t.name === 'Product')
    expect(def).toBeDefined()
    expect(def?.fields).toHaveLength(3)
    expect(def?.fields.map((f) => f['name'])).toEqual(['id', 'price', 'featured'])
    // `?` on a field records optionality.
    expect(def?.fields.map((f) => f['optional'])).toEqual([false, false, true])
  })

  test('parses props block', () => {
    const source = `---
props {
  title: string
  count: number
}
---
<h1>\${title}</h1>`
    const ast = parseOk(source) as {
      frontmatter: { props: Array<{ name: string; type_expr: string }> }
    }
    expect(ast.frontmatter.props).toHaveLength(2)
    expect(ast.frontmatter.props.map((p) => p.name)).toEqual(['title', 'count'])
    expect(ast.frontmatter.props.map((p) => p.type_expr)).toEqual(['string', 'number'])
  })

  test('a prop default is recorded, and `required` tracks `?` rather than the default', () => {
    // WAS WRONG: the old test asserted `required === false` for a prop that has
    // a default. The parser sets `required = !eat(Question)`
    // (compiler/src/parser/frontmatter.rs), so a default does not make a prop
    // optional; only a `?` does. Nothing in the compiler reads `required` - it
    // is metadata for editor tooling - so this pins the rule as it stands
    // rather than asserting the semantics one might expect.
    const source = `---
props {
  label: string = "Submit"
  disabled?: boolean
}
---
<button>\${label}</button>`
    const ast = parseOk(source) as {
      frontmatter: {
        props: Array<{ name: string; default_value: unknown; required: boolean }>
      }
    }
    const label = ast.frontmatter.props.find((p) => p.name === 'label')
    const disabled = ast.frontmatter.props.find((p) => p.name === 'disabled')

    expect(label?.default_value).not.toBeNull()
    expect(label?.required).toBe(true)

    expect(disabled?.default_value).toBeNull()
    expect(disabled?.required).toBe(false)
  })

  test('parses middleware declaration', () => {
    const ast = parseOk('---\nmiddleware "auth"\n---\n{next}') as {
      frontmatter: { middleware: string }
    }
    expect(ast.frontmatter.middleware).toBe('auth')
  })
})

// ─── Expressions ──────────────────────────────────────────────────────────────

describe('Compiler - Expressions', () => {
  test('simple variable', () => {
    expect(compile('${name}', { name: 'Alice' })).toBe('Alice')
  })

  test('nested path a.b.c', () => {
    expect(compile('${user.profile.name}', { user: { profile: { name: 'Bob' } } })).toBe('Bob')
  })

  test('safe navigation: null at any depth gives an empty string', () => {
    // No error, no crash, and critically no "null"/"undefined" leaking into HTML.
    expect(compile('${user.profile.name}', { user: null })).toBe('')
    expect(compile('${missing.deeply.nested}', {})).toBe('')
  })

  test('filter: currency', () => {
    expect(compile('${price | currency}', { price: 1234.5 })).toBe('$1,234.50')
  })

  test('filter chaining: value | upper | truncate(10)', () => {
    // truncate(10) keeps 10 characters and appends an ellipsis of three dots.
    expect(compile('${description | upper | truncate(10)}', {
      description: 'hello world this is long',
    })).toBe('HELLO WORL...')
  })

  test('ternary expression', () => {
    expect(compile('${isActive ? "yes" : "no"}', { isActive: true })).toBe('yes')
    expect(compile('${isActive ? "yes" : "no"}', { isActive: false })).toBe('no')
  })

  test('arithmetic: price * 1.1', () => {
    // IEEE-754: 100 * 1.1 is 110.00000000000001, and the renderer does not round.
    expect(compile('${price * 1.1}', { price: 100 })).toMatch(/^110(\.0+1)?$/)
    expect(compile('${a + b}', { a: 2, b: 3 })).toBe('5')
    expect(compile('${a - b}', { a: 9, b: 4 })).toBe('5')
  })

  test('there is no math namespace: a call renders empty and raises no error', () => {
    // WAS WRONG: the old test asserted `math.min`, `math.max` and `math.round`
    // worked. Slurp has no callable functions at all. SLURP_DOCS.md section 2:
    // "f(a, b) parses, and then always evaluates to null, which renders as an
    // empty string. There is no Math, no math, no user-defined functions."
    // Silent-empty is the documented behaviour, so pin it: the failure mode
    // this guards against is a future change that starts erroring, or one that
    // starts resolving `math` from the context and shadows a real variable.
    for (const src of ['${math.min(a, b)}', '${math.max(a, b)}', '${math.round(val)}']) {
      const result = render(parseOk(src), { a: 3, b: 7, val: 3.7 })
      expect(result.html, `${src} should render empty`).toBe('')
      expect(result.errors, `${src} should not error`).toHaveLength(0)
    }
  })

  test('template literal string', () => {
    expect(compile('${`Hello, ${name}!`}', { name: 'Carol' })).toBe('Hello, Carol!')
  })

  test('a reactive $path resolves from the render context and emits no client directive', () => {
    // WAS WRONG: the old test asserted the output contained `x-text`.
    // `ExprKind::ReactivePath` is handled by the same renderer arm as
    // `ExprKind::Path` (compiler/src/renderer/mod.rs), so at render time `$cart`
    // is just the context key `cart` with the sigil dropped, and nothing
    // client-side is emitted. SLURP_DOCS.md section 15 confirms the runtime
    // registers zero Alpine directives, so no consumer exists for one.
    const ast = parseOk('<span>${$cart.count}</span>')

    expect(renderOk(ast, { cart: { count: 3 } })).toBe('<span>3</span>')
    // The sigil is not part of the key: a context key that literally contains
    // `$cart` is NOT what a reactive path reads.
    expect(renderOk(ast, { $cart: { count: 7 } })).toBe('<span></span>')
    expect(renderOk(ast, { cart: { count: 3 } })).not.toContain('x-text')
  })
})

// ─── Whitespace agnosticism ───────────────────────────────────────────────────

describe('Compiler - Whitespace agnosticism', () => {
  const twoSpaceSource = '{if featured}\n  <span>Featured</span>\n{/if}'
  const fourSpaceSource = '{if featured}\n    <span>Featured</span>\n{/if}'
  const tabSource = '{if featured}\n\t<span>Featured</span>\n{/if}'

  // NOTE ON WHAT "AGNOSTIC" MEANS HERE. The old tests asserted only that both
  // inputs parsed and that the AST was not the stub's `{}`. That was a
  // stub-detector, not a test. Indentation is NOT stripped: it survives as Text
  // nodes, so the ASTs and the raw HTML genuinely differ. What holds is that
  // indentation never changes the STRUCTURE, so the outputs are equal once
  // whitespace is collapsed. Asserting AST equality here would be false.

  test('2-space and 4-space indent produce the same structure', () => {
    const two = parseOk(twoSpaceSource)
    const four = parseOk(fourSpaceSource)

    expect(squash(renderOk(two, { featured: true }))).toBe('<span>Featured</span>')
    expect(squash(renderOk(four, { featured: true }))).toBe(
      squash(renderOk(two, { featured: true })),
    )
    // Both are a single If node whose body holds one element.
    expect(Object.keys(firstNode(two))).toEqual(['If'])
    expect(Object.keys(firstNode(four))).toEqual(['If'])
  })

  test('tab indent produces the same structure as spaces', () => {
    const two = parseOk(twoSpaceSource)
    const tabbed = parseOk(tabSource)
    expect(squash(renderOk(tabbed, { featured: true }))).toBe(
      squash(renderOk(two, { featured: true })),
    )
    // The tab really is preserved rather than normalised to spaces.
    expect(renderOk(tabbed, { featured: true })).toContain('\t')
  })

  test('single-line {if} renders the same as multiline', () => {
    const inline = parseOk('{if x}<p>yes</p>{/if}')
    const multiline = parseOk('{if x}\n<p>yes</p>\n{/if}')
    expect(squash(renderOk(inline, { x: true }))).toBe('<p>yes</p>')
    expect(squash(renderOk(multiline, { x: true }))).toBe('<p>yes</p>')
    expect(renderOk(inline, { x: false })).toBe('')
  })

  test('single-line {each} renders the same as multiline', () => {
    const inline = parseOk('{each item in items}<li>${item}</li>{/each}')
    const multiline = parseOk('{each item in items}\n  <li>${item}</li>\n{/each}')
    const ctx = { items: ['a', 'b'] }
    expect(squash(renderOk(inline, ctx))).toBe('<li>a</li><li>b</li>')
    expect(squash(renderOk(multiline, ctx))).toBe('<li>a</li> <li>b</li>')
  })
})

// ─── HTML Output ──────────────────────────────────────────────────────────────

describe('Compiler - HTML Output', () => {
  test('expression output is HTML-escaped', () => {
    const html = compile('${value}', { value: '<script>alert(1)</script>' })
    expect(html).toBe('&lt;script&gt;alert(1)&lt;/script&gt;')
  })

  test('& < > " \' all escaped in expressions', () => {
    expect(compile('${value}', { value: '& < > " \'' })).toBe('&amp; &lt; &gt; &quot; &#39;')
  })

  test('{html} outputs raw unescaped HTML', () => {
    const html = compile('{html content}', { content: '<strong>bold</strong>' })
    expect(html).toBe('<strong>bold</strong>')
  })

  // THE SYNTAX THE OLD TESTS USED DOES NOT EXIST. `{html sanitize content}` is a
  // parse error ("Expected } after {html sanitize expression}"). The sanitize
  // marker is a SUFFIX: `{html expr sanitize}`. SLURP_DOCS.md section 3.9.
  test('{html expr sanitize} strips <script>', () => {
    const html = compile('{html content sanitize}', {
      content: '<p>Hello</p><script>alert(1)</script>',
    })
    expect(html).toBe('<p>Hello</p>')
    expect(html).not.toContain('alert(1)')
  })

  test('{html expr sanitize} strips on* attributes', () => {
    // The allowlist sanitizer drops the whole <button> element, not just the
    // handler, because <button> is not on ammonia's allowlist. The old test
    // asserted the <button> survived, which it does not.
    const html = compile('{html content sanitize}', {
      content: '<button onclick="evil()">Click</button>',
    })
    expect(html).toBe('Click')
    expect(html).not.toContain('onclick')
    expect(html).not.toContain('evil()')
  })

  test('{html expr sanitize} strips an on* attribute from an allowed element', () => {
    // The case above cannot distinguish "attribute stripped" from "element
    // dropped". <a> IS allowlisted, so this isolates attribute stripping.
    const html = compile('{html content sanitize}', {
      content: '<a href="/ok" onclick="evil()">go</a>',
    })
    expect(html).toContain('href="/ok"')
    expect(html).toContain('go')
    expect(html).not.toContain('onclick')
  })

  test('{html expr sanitize} strips href="javascript:"', () => {
    const html = compile('{html content sanitize}', {
      content: '<a href="javascript:void(0)">link</a>',
    })
    expect(html).toContain('<a')
    expect(html).toContain('link')
    expect(html).not.toContain('javascript:')
    expect(html).not.toContain('href=')
  })

  test('{html} without sanitize does NOT strip a script, by design', () => {
    // The counterpart to the three above: `{html}` is the documented escape
    // hatch for trusted server-controlled content, so it must stay raw. A change
    // that started sanitizing unconditionally would break trusted embeds; a
    // change that made `sanitize` a no-op would be an XSS hole. Only one of
    // those directions is caught by the tests above.
    const html = compile('{html content}', { content: '<script>alert(1)</script>' })
    expect(html).toBe('<script>alert(1)</script>')
  })
})

// ─── Control Flow ─────────────────────────────────────────────────────────────

describe('Compiler - Control Flow', () => {
  test('{if} renders then branch when true', () => {
    expect(compile('{if show}<p>Visible</p>{/if}', { show: true })).toBe('<p>Visible</p>')
  })

  test('{if} renders nothing when false', () => {
    expect(compile('{if show}<p>Visible</p>{/if}', { show: false })).toBe('')
  })

  test('{if}{else}', () => {
    const source = '{if show}<p>Yes</p>{else}<p>No</p>{/if}'
    expect(compile(source, { show: true })).toBe('<p>Yes</p>')
    expect(compile(source, { show: false })).toBe('<p>No</p>')
  })

  test('{if}{else if}{else}', () => {
    const source =
      '{if status == "active"}<p>Active</p>{else if status == "pending"}<p>Pending</p>{else}<p>Inactive</p>{/if}'
    expect(compile(source, { status: 'active' })).toBe('<p>Active</p>')
    expect(compile(source, { status: 'pending' })).toBe('<p>Pending</p>')
    expect(compile(source, { status: 'archived' })).toBe('<p>Inactive</p>')
  })

  // THERE IS NO {case} TAG. An arm is `pattern -> body`, and the old `{case "x"}`
  // form is a parse error. SLURP_DOCS.md section 3.6 calls this out by name,
  // including the exact diagnostic ("Expected match pattern, found LBrace"),
  // because earlier revisions of the docs showed the wrong form too.
  test('{match} string patterns', () => {
    const source = `{match status}
  "active"   -> <span>Active</span>
  "inactive" -> <span>Inactive</span>
{/match}`
    expect(squash(compile(source, { status: 'active' }))).toBe('<span>Active</span>')
    expect(squash(compile(source, { status: 'inactive' }))).toBe('<span>Inactive</span>')
  })

  test('{match} number patterns', () => {
    const source = `{match code}
  200 -> <span>OK</span>
  404 -> <span>Not Found</span>
{/match}`
    expect(squash(compile(source, { code: 200 }))).toBe('<span>OK</span>')
    expect(squash(compile(source, { code: 404 }))).toBe('<span>Not Found</span>')
  })

  test('{match} is type-strict and renders nothing when no arm matches', () => {
    const source = '{match code}\n  200 -> <span>OK</span>\n{/match}'
    // A "200" string never matches a numeric 200 pattern, and with no `_` arm
    // the block renders nothing rather than falling through.
    expect(squash(compile(source, { code: '200' }))).toBe('')
    expect(squash(compile(source, { code: 500 }))).toBe('')
  })

  test('{match} wildcard', () => {
    const source = `{match status}
  "active" -> <span>Active</span>
  _        -> <span>Other</span>
{/match}`
    expect(squash(compile(source, { status: 'active' }))).toBe('<span>Active</span>')
    expect(squash(compile(source, { status: 'archived' }))).toBe('<span>Other</span>')
    expect(squash(compile(source, { status: 'pending' }))).toBe('<span>Other</span>')
  })

  test('{match} multi-line arm with a multi-element body', () => {
    // Arms are self-delimiting: a body runs to the next pattern token, so there
    // is no per-arm closing tag.
    const source = `{match role}
  "admin" -> <p>Admin</p>
             <a href="/admin">Dashboard</a>
  _       -> <p>User</p>
{/match}`
    const html = squash(compile(source, { role: 'admin' }))
    expect(html).toContain('<p>Admin</p>')
    expect(html).toContain('<a href="/admin">Dashboard</a>')
    expect(html).not.toContain('<p>User</p>')
    expect(squash(compile(source, { role: 'buyer' }))).toBe('<p>User</p>')
  })
})

// ─── Loops ────────────────────────────────────────────────────────────────────

describe('Compiler - Loops', () => {
  test('{each} iterates collection', () => {
    expect(
      compile('{each item in items}<li>${item}</li>{/each}', {
        items: ['apple', 'banana', 'cherry'],
      }),
    ).toBe('<li>apple</li><li>banana</li><li>cherry</li>')
  })

  test('{each} with index variable', () => {
    expect(
      compile('{each item, i in items}<li>${i}: ${item}</li>{/each}', { items: ['a', 'b'] }),
    ).toBe('<li>0: a</li><li>1: b</li>')
  })

  test('{each} with empty branch on empty collection', () => {
    const source = '{each item in items}<li>${item}</li>{empty}<li>No items</li>{/each}'
    expect(compile(source, { items: [] })).toBe('<li>No items</li>')
    expect(compile(source, { items: ['a'] })).toBe('<li>a</li>')
  })

  test('{each} loop.index, loop.count, loop.first, loop.last', () => {
    expect(
      compile(
        '{each item in items}<span>${loop.index}-${loop.count}-${loop.first}-${loop.last}</span>{/each}',
        { items: ['x', 'y'] },
      ),
    ).toBe('<span>0-2-true-false</span><span>1-2-false-true</span>')
  })

  test('{each} | limit(n) filter', () => {
    expect(
      compile('{each item in items | limit(2)}<li>${item}</li>{/each}', {
        items: ['a', 'b', 'c', 'd'],
      }),
    ).toBe('<li>a</li><li>b</li>')
  })

  test('{each} | sort("key") filter', () => {
    expect(
      compile('{each p in products | sort("price")}<li>${p.name}</li>{/each}', {
        products: [
          { name: 'Expensive', price: 100 },
          { name: 'Cheap', price: 10 },
          { name: 'Mid', price: 50 },
        ],
      }),
    ).toBe('<li>Cheap</li><li>Mid</li><li>Expensive</li>')
  })

  test('{each} | sort("key", "desc") filter', () => {
    expect(
      compile('{each p in products | sort("price", "desc")}<li>${p.name}</li>{/each}', {
        products: [
          { name: 'Expensive', price: 100 },
          { name: 'Cheap', price: 10 },
        ],
      }),
    ).toBe('<li>Expensive</li><li>Cheap</li>')
  })

  test('{each} | filter("key", "value") filter', () => {
    expect(
      compile('{each p in products | filter("featured", true)}<li>${p.name}</li>{/each}', {
        products: [
          { name: 'Featured Product', featured: true },
          { name: 'Normal Product', featured: false },
        ],
      }),
    ).toBe('<li>Featured Product</li>')
  })

  test('{each} | reverse filter', () => {
    expect(
      compile('{each item in items | reverse}<li>${item}</li>{/each}', {
        items: ['first', 'second', 'third'],
      }),
    ).toBe('<li>third</li><li>second</li><li>first</li>')
  })

  test('{repeat n} repeats body n times', () => {
    expect(compile('{repeat 3}<span>x</span>{/repeat}')).toBe(
      '<span>x</span><span>x</span><span>x</span>',
    )
  })

  test('nested loops', () => {
    expect(
      compile(
        '{each category in categories}<h2>${category.name}</h2>{each item in category.items}<li>${item}</li>{/each}{/each}',
        {
          categories: [
            { name: 'Fruits', items: ['apple', 'banana'] },
            { name: 'Veggies', items: ['carrot'] },
          ],
        },
      ),
    ).toBe('<h2>Fruits</h2><li>apple</li><li>banana</li><h2>Veggies</h2><li>carrot</li>')
  })

  test('max iteration cap at 1000, reported in both modes', () => {
    // THE CAP AND THE DIAGNOSTIC ARE BOTH REAL IN BOTH MODES. Truncation leaves
    // no trace in the rendered page - the rows are simply absent - so a render
    // that drops half a list while reporting nothing is indistinguishable from
    // a correct one. `render_each` therefore warns in production too, matching
    // the output-byte and render-depth budgets.
    //
    // `render_dev` still differs, on the diagnostics whose effect IS visible in
    // the markup: the CSS-structural strip and the undeclared JS attribute slot.
    const items = Array.from({ length: 2000 }, (_, i) => i)
    const ast = parseOk('{each item in items}<span>${item}</span>{/each}')

    for (const result of [render(ast, { items }), renderDev(ast, { items })]) {
      expect((result.html.match(/<span>/g) ?? []).length).toBe(1000)
      expect(result.warnings.map((w) => w.code)).toContain('IterationLimitExceeded')
      // A truncation is advisory, not a failure: the page still renders.
      expect(result.errors).toHaveLength(0)
      expect(result.ok).toBe(true)
    }
  })

  test('a collection at exactly the cap is not truncated and raises nothing', () => {
    // Pins the boundary, so a future off-by-one in the cap is a red test rather
    // than a thousand silently-dropped rows.
    const items = Array.from({ length: 1000 }, (_, i) => i)
    const dev = renderDev(parseOk('{each item in items}<span>x</span>{/each}'), { items })
    expect((dev.html.match(/<span>/g) ?? []).length).toBe(1000)
    expect(dev.warnings).toHaveLength(0)
  })
})

// ─── Fetch ────────────────────────────────────────────────────────────────────

describe('Compiler - Fetch', () => {
  // WHAT {fetch} ACTUALLY DOES, because every old test in this suite assumed
  // otherwise. It is SERVER-SIDE: the compiler performs no network call and
  // emits no client-side component. The host populates the context key before
  // rendering and the renderer picks a branch from that value
  // (SLURP_DOCS.md section 3.8). The `x-data="slurpFetch(...)"` root,
  // `data-slurp-sentinel` and pagination controls the old tests looked for have
  // NO PRODUCER anywhere in the compiler, which SLURP_DOCS.md section 15 states
  // outright. Those assertions described a feature that does not exist.
  const fetchSource = `{fetch data from "/api/items"}
  <p>Loaded \${data.name}</p>
{loading}
  <p>Loading...</p>
{error}
  <p>Failed: \${error.message}</p>
{empty}
  <p>Nothing</p>
{/fetch}`

  test('{fetch} renders its body when the host supplied the value', () => {
    expect(squash(compile(fetchSource, { data: { name: 'Widget' } }))).toBe('<p>Loaded Widget</p>')
  })

  test('{fetch} renders the loading branch when the key is absent', () => {
    expect(squash(compile(fetchSource, {}))).toBe('<p>Loading...</p>')
    expect(squash(compile(fetchSource, { data: null }))).toBe('<p>Loading...</p>')
  })

  test('{fetch} renders the error branch for a value carrying __error', () => {
    // `__error` is the host's convention for reporting a failure, and
    // `error.message` works only because the host put a `message` field there.
    expect(squash(compile(fetchSource, { data: { __error: { message: 'boom' } } }))).toBe(
      '<p>Failed: boom</p>',
    )
  })

  test('{fetch} renders the empty branch for an empty array', () => {
    expect(squash(compile(fetchSource, { data: [] }))).toBe('<p>Nothing</p>')
  })

  test('{fetch} emits no client-side runtime markers', () => {
    // Pins the documented gap. If a future change starts emitting an
    // `x-data="slurpFetch(...)"` root, this test should be updated deliberately
    // and the runtime's markers wired up in the same change - not discovered by
    // a theme author whose infinite scroll silently never fires.
    const html = compile(fetchSource, { data: { name: 'x' } })
    expect(html).not.toContain('x-data')
    expect(html).not.toContain('slurpFetch')
    expect(html).not.toContain('data-slurp-sentinel')
  })

  test('{fetch} parses the parenthesised option form', () => {
    // WAS WRONG: the old test used `cache:60 retry:3 timeout:5000 abort:navigate`.
    // Only the call form parses. SLURP_DOCS.md section 3.8: "Only the
    // parenthesised call form parses."
    const source = `{fetch data from "/api/items" cache(60) retry(3) timeout(5000) abortOnNavigate}
  <p>\${data.name}</p>
{/fetch}`
    const node = firstNode(parseOk(source))
    const options = node['Fetch']!['options'] as Record<string, unknown>
    expect(options['cache_ttl']).toBe(60)
    expect(options['retry']).toBe(3)
    expect(options['timeout_ms']).toBe(5000)
    expect(options['abort_on_navigate']).toBe(true)
  })

  test('only the camelCase abortOnNavigate spelling works, not the hyphenated one', () => {
    // DOCS-VS-CODE DEFECT, pinned here rather than fixed because the parser is
    // out of scope for this change. SLURP_DOCS.md section 3.8 lists
    // `abort-on-navigate` as the option's PRIMARY spelling and `abortOnNavigate`
    // as an alternative. Only the second one is recognised: the hyphenated form
    // lexes as separate tokens and is then discarded by the same
    // silently-drop-unknown-options path as `cache:60`.
    //
    // So a theme author following the documented spelling gets no abort, no
    // error and no warning. Reported; the fix is either to accept the
    // hyphenated form in the parser or to correct the table.
    const optionsFor = (opt: string): Record<string, unknown> =>
      firstNode(parseOk(`{fetch d from "/api/i" ${opt}}<p>x</p>{/fetch}`))['Fetch']![
        'options'
      ] as Record<string, unknown>

    expect(optionsFor('abortOnNavigate')['abort_on_navigate']).toBe(true)
    expect(optionsFor('abort-on-navigate')['abort_on_navigate']).toBe(false)
    // And it really is silent, which is what makes it costly.
    expect(parse('{fetch d from "/api/i" abort-on-navigate}<p>x</p>{/fetch}').errors).toHaveLength(0)
  })

  test('{fetch} poll, paginate and infinite options', () => {
    const opts = (src: string): Record<string, unknown> =>
      firstNode(parseOk(src))['Fetch']!['options'] as Record<string, unknown>

    expect(opts('{fetch d from "/api/s" poll(30)}<p>x</p>{/fetch}')['poll_seconds']).toBe(30)
    expect(opts('{fetch d from "/api/s" paginate(20)}<p>x</p>{/fetch}')['paginate']).toEqual({
      limit: 20,
    })
    expect(opts('{fetch d from "/api/s" infinite(20)}<p>x</p>{/fetch}')['infinite']).toEqual({
      limit: 20,
    })
  })

  test('the colon option form is silently dropped, with no diagnostic', () => {
    // This is documented behaviour and a genuine footgun: `cache:300` looks
    // plausible, parses cleanly, and does nothing. Pinned here so that if
    // anyone makes it an error or makes it work, a test says so.
    const result = parse('{fetch d from "/api/i" cache:60 bogus(9)}<p>x</p>{/fetch}')
    expect(result.errors).toHaveLength(0)
    expect(result.warnings).toHaveLength(0)
    const options = firstNode(result.ast)['Fetch']!['options'] as Record<string, unknown>
    expect(options['cache_ttl']).toBeNull()
  })

  test('the type hint parses and does not affect rendering', () => {
    // DOCS-VS-CODE DEFECT. SLURP_DOCS.md section 3.8 shows the array form as
    // `{fetch products[] from "/api/products"}`, with the brackets on the NAME.
    // That is a parse error ("Expected 'from' in {fetch}"). The brackets belong
    // to a TYPE, so an array hint requires the named-type form. Reported.
    parseExpectError('{fetch products[] from "/api/products"}<p>x</p>{/fetch}', 'UnexpectedToken')

    const arrayForm = parseOk('{fetch products: Product[] from "/api/products"}<p>x</p>{/fetch}')
    expect(firstNode(arrayForm)['Fetch']!['is_array']).toBe(true)
    expect(firstNode(arrayForm)['Fetch']!['type_hint']).toBe('Product')

    const named = parseOk('{fetch product: Product from "/api/p"}<p>x</p>{/fetch}')
    expect(firstNode(named)['Fetch']!['type_hint']).toBe('Product')
    // Nothing is validated against the hint: a mismatched value still renders.
    expect(
      squash(compile('{fetch product: Product from "/api/p"}<p>${product.n}</p>{/fetch}', {
        product: { n: 'anything' },
      })),
    ).toBe('<p>anything</p>')
  })
})

// ─── Components & Slots ───────────────────────────────────────────────────────

describe('Compiler - Components & Slots', () => {
  test('an unresolved component renders a placeholder carrying its props', () => {
    // WAS VACUOUS: the old test asserted only `typeof html === 'string'`, which
    // the stub satisfied with an empty string. The WASM entry point has no
    // component registry, so every component renders the placeholder form that
    // a host with a registry would replace.
    const html = compile('<Card title="Hello" />')
    expect(html).toContain('data-slurp-component="Card"')
    expect(html).toContain('data-slurp-props=')
    // Props are serialised into the attribute, attribute-escaped.
    expect(html).toContain('&quot;title&quot;:&quot;Hello&quot;')
  })

  test('component with boolean prop shorthand', () => {
    // `PropValue::Boolean` is a unit enum variant, so it serialises as the bare
    // string "Boolean" rather than an object.
    const node = firstNode(parseOk('<Button disabled />'))
    const props = node['Component']!['props'] as Array<{ name: string; value: unknown }>
    const disabled = props.find((p) => p.name === 'disabled')
    expect(disabled).toBeDefined()
    expect(disabled?.value).toBe('Boolean')
    // It reaches the rendered placeholder as a real `true`.
    expect(compile('<Button disabled />')).toContain('&quot;disabled&quot;:true')
  })

  test('spread props are a parse error', () => {
    // WAS WRONG: the old test asserted `ast.children[0].spread` was defined.
    // Both `<Card ...cardProps />` and `<Card ...spread={x} />` are parse
    // errors, because in HTML tag lexing the identifier after the dots lexes as
    // an attribute name. SLURP_DOCS.md section 5 documents this as a limitation.
    parseExpectError('<ProductCard ...item />', 'UnexpectedToken')
    parseExpectError('<ProductCard ...item={x} />', 'UnexpectedToken')
  })

  test('component with children (default slot)', () => {
    const node = firstNode(parseOk('<Card><p>Inner content</p></Card>'))
    const children = node['Component']!['children'] as unknown[]
    expect(children.length).toBeGreaterThan(0)
    expect(compile('<Card><p>Inner content</p></Card>')).toContain('<p>Inner content</p>')
  })

  test('slot="name" reaches the AST as an attribute and is then ignored', () => {
    // WAS WRONG: the old test read `child.slot`. A plain element carries
    // `slot="header"` as an ordinary attribute; only a Component node has a
    // `slot` field. Either way nothing reads it: SLURP_DOCS.md section 5 states
    // that `slot="name"` routing is not implemented, and the attribute passes
    // through to the output verbatim.
    const node = firstNode(parseOk('<Card><h1 slot="header">Title</h1></Card>'))
    const children = node['Component']!['children'] as Node[]
    const attrs = children[0]!['Element']!['attrs'] as Array<{ name: string; value: unknown }>
    expect(attrs.find((a) => a.name === 'slot')?.value).toEqual({ StringLiteral: 'header' })

    expect(compile('<Card><h1 slot="header">Title</h1></Card>')).toContain('<h1 slot="header">')
  })

  test('{slot "name"} is a void block; the fallback-content form is a parse error', () => {
    // WAS WRONG: the old test used `{slot name="cta"}<button>...</button>{/slot}`
    // and expected a `fallback` array. The block form takes no children and no
    // `name=` attribute. SLURP_DOCS.md section 6: "Fallback content needs the
    // element form. {slot}fallback{/slot} is a parse error."
    parseExpectError('{slot name="cta"}<button>Default CTA</button>{/slot}', 'UnexpectedToken')

    // The real forms parse. Only `default` and `head` are ever filled; any other
    // name renders a literal <slot> element.
    parseOk('{slot}')
    parseOk('{slot "head"}')
    expect(compile('{slot "sidebar"}')).toContain('<slot name="sidebar">')
  })
})

// ─── Image ────────────────────────────────────────────────────────────────────

describe('Compiler - Image', () => {
  test('<Image> outputs <picture> with avif, webp and a jpeg fallback', () => {
    const html = compile('<Image src="/hero.jpg" alt="Hero" width={1200} height={600} />')
    expect(html).toContain('<picture')
    expect(html).toContain('type="image/avif"')
    expect(html).toContain('type="image/webp"')
    expect(html).toContain('f=jpeg')
    expect(html).toContain('width="1200"')
    expect(html).toContain('height="600"')
    expect(html).toContain('alt="Hero"')
  })

  test('loading is controlled by `priority`; there is no `lazy` prop', () => {
    // WAS MISLEADING, IN BOTH DIRECTIONS. One old test passed `lazy` and
    // asserted `loading="lazy"` - but lazy is the DEFAULT, so that test would
    // have passed with the prop deleted; it proved nothing. The other asserted
    // `priority` produced `fetchpriority="high"`, which the compiler never
    // emits. SLURP_DOCS.md section 5: "There is no `lazy` prop; `priority` is
    // the only loading control", and truthy `priority` gives `loading="eager"`.
    const plain = compile('<Image src="/img.jpg" alt="Alt" width={800} height={400} />')
    expect(plain).toContain('loading="lazy"')

    const priority = compile(
      '<Image src="/hero.jpg" alt="Hero" width={1200} height={630} priority />',
    )
    expect(priority).toContain('loading="eager"')
    expect(priority).not.toContain('loading="lazy"')
    expect(priority).not.toContain('fetchpriority')

    // An unrecognised `lazy` prop changes nothing.
    const withLazy = compile('<Image src="/img.jpg" alt="Alt" width={800} height={400} lazy />')
    expect(withLazy).toBe(plain)
  })

  test('a missing width or height falls back to a default, and is NOT an error', () => {
    // There is no missing-dimension error, despite what the code used to be
    // called: width defaults to 800 and height to 600. The code is now
    // `MissingImageSrc`, which is the only case it ever fired for.
    const noWidth = compile('<Image src="/img.jpg" alt="Alt" height={400} />')
    expect(noWidth).toContain('w=800')
    expect(noWidth).toContain('height="400"')

    const noHeight = compile('<Image src="/img.jpg" alt="Alt" width={800} />')
    expect(noHeight).toContain('height="600"')
  })

  test('<Image> without a src raises MissingImageSrc and renders nothing', () => {
    const result = render(parseOk('<Image alt="Alt" width={800} height={400} />'))
    expect(result.errors.map((e) => e.code)).toContain('MissingImageSrc')
    expect(result.html).toBe('')
  })

  test('every src is routed through the optimiser; there is no except-list', () => {
    // WAS WRONG: the old test expected an external domain to bypass the
    // optimiser and emit a plain <img> with the original URL. `ImageConfig.except`
    // exists in shared/interfaces.ts as host configuration, but the compiler
    // reads no config and has no bypass path, so an external host is rewritten
    // exactly like a local one.
    const html = compile(
      '<Image src="https://cdn.partner.com/logo.png" alt="Logo" width={200} height={50} />',
    )
    expect(html).toContain('/_slurp/image?src=https://cdn.partner.com/logo.png')
    expect(html).toContain('<picture')
  })
})

// ─── Security ─────────────────────────────────────────────────────────────────

describe('Compiler - Security', () => {
  // THE WASM EXPORTS DELIBERATELY DO NOT RUN `security_check`. The note at the
  // top of compiler/src/wasm/mod.rs says so: SLURP_SECRET_ access, middleware
  // scope and redirect restrictions are server-side concerns, enforced by the
  // host. The old tests asserted this API rejected them, which it never has.
  //
  // These tests therefore pin the OMISSION. That is worth pinning precisely
  // because it is security-relevant: a host that assumed `validate` was a
  // security gate would be wrong, and if these checks are ever moved into the
  // WASM path that should be a deliberate change with a test update.
  test('SLURP_SECRET_* parses cleanly here: the WASM path runs no security check', () => {
    const result = parse('${SLURP_SECRET_API_KEY}')
    expect(result.errors).toHaveLength(0)
    expect(result.ok).toBe(true)
    expect(validate('${SLURP_SECRET_API_KEY}')).toHaveLength(0)
    // It renders as an ordinary context lookup, so an absent key is empty.
    expect(compile('${SLURP_SECRET_API_KEY}')).toBe('')
  })

  test('request.cookies parses cleanly here: middleware scope is a host concern', () => {
    const result = parse('${request.cookies.session}')
    expect(result.errors).toHaveLength(0)
    expect(validate('${request.cookies.session}')).toHaveLength(0)
  })

  test('validate is single-file: it resolves no imports and reports no CircularImport', () => {
    // Import resolution needs a file system and a registry, neither of which
    // this entry point has. A self-import is therefore not detectable here, and
    // reporting it is the host build's job. The old test asserted only
    // `Array.isArray(errors)`, which the stub satisfied trivially.
    const errors = validate('---\nusing "@pages/self"\n---\n<p>Self-import</p>')
    expect(errors).toHaveLength(0)
    expect(errors.map((e) => e.code)).not.toContain('CircularImport')
  })

  test('escaping still applies to values that look like markup', () => {
    // The one security property this entry point DOES enforce end to end.
    const html = compile('<div title="${v}">${v}</div>', { v: '"><script>alert(1)</script>' })
    expect(html).not.toContain('<script>')
    expect(html).toContain('&lt;script&gt;')
  })
})

// ─── Errors ───────────────────────────────────────────────────────────────────

describe('Compiler - Errors', () => {
  test('an error carries file, line, column, message, code and severity', () => {
    // The old test wrapped every assertion in `if (result.errors.length > 0)`
    // with an `expect(true).toBe(true)` else-branch, so it passed against a stub
    // that never errored. The real parser errors here, unconditionally.
    const result = parse('{if unclosed}content')
    expect(result.errors.length).toBeGreaterThan(0)
    const err = result.errors[0]!
    expect(err.file).toBe('wasm-input')
    expect(typeof err.line).toBe('number')
    expect(typeof err.column).toBe('number')
    expect(err.message.length).toBeGreaterThan(0)
    expect(typeof err.code).toBe('string')
    expect(err.severity).toBe('error')
    // Positions are 1-based and point at the offending construct.
    expect(err.line).toBe(1)
    expect(err.column).toBe(1)
  })

  test('unclosed {if} block gives an UnclosedBlock error', () => {
    const errors = parseExpectError('{if isVisible}<p>Hello</p>', 'UnclosedBlock')
    expect(errors[0]?.message).toMatch(/unclosed/i)
  })

  test('validate returns the same diagnostics as parse, as a flat array', () => {
    const source = '{if isVisible}<p>Hello</p>'
    const fromValidate = validate(source)
    const fromParse = parse(source)
    expect(fromValidate.length).toBeGreaterThan(0)
    expect(fromValidate.map((e) => e.code)).toEqual(
      [...fromParse.errors, ...fromParse.warnings].map((e) => e.code),
    )
    expect(validate('<p>clean</p>')).toEqual([])
  })

  test('an unknown component is NOT an error: it renders a placeholder', () => {
    // WAS WRONG: the old test expected `UnknownComponent`. SLURP_DOCS.md section
    // 12 lists it among the codes that "exist only for host integrations to
    // reuse" and are never emitted by the compiler: "an unresolvable component
    // renders a placeholder rather than raising UnknownComponent".
    const result = parse('---\n---\n<NonExistentComponent />')
    expect(result.errors).toHaveLength(0)
    const rendered = render(result.ast)
    expect(rendered.errors).toHaveLength(0)
    expect(rendered.html).toContain('data-slurp-component="NonExistentComponent"')
  })

  test('a missing required prop is NOT an error: prop types are never checked', () => {
    // WAS WRONG, same reason as above. SLURP_DOCS.md section 5: "Prop types are
    // never validated at render time. MissingRequiredProp, UnknownProp and
    // TypeMismatch exist as error codes but the compiler does not emit them."
    const result = parse('<Card />')
    expect(result.errors).toHaveLength(0)
    expect(render(result.ast).errors).toHaveLength(0)

    // Not merely unemitted for this input: no source in the compiler emits it.
    const withDeclaredProps = parse('---\nprops {\n  title: string\n}\n---\n<Card />')
    expect(withDeclaredProps.errors.map((e) => e.code)).not.toContain('MissingRequiredProp')
  })
})

// ─── Editor schema surface ────────────────────────────────────────────────────

// `extract_schema` and `render_section` are two of the six WASM exports and had
// no coverage of any kind before this file was wired to the real module.

describe('Compiler - extract_schema', () => {
  const sectionSource = `---
section {
  name: "Hero"
  settings {
    heading:    text = "Welcome"
    max_items:  number = 8 { min: 1, max: 24 }
    show_badge: toggle = true
  }
}
---
<h1>\${ section.settings.heading }</h1>`

  test('extracts settings with their kinds, defaults and metadata', () => {
    const result = extractSchema(sectionSource)
    expect(result.ok).toBe(true)
    expect(result.errors).toHaveLength(0)

    const schema = result.schema as { name: string; settings: Array<Record<string, unknown>> }
    expect(schema.name).toBe('Hero')
    expect(schema.settings.map((s) => s['key'])).toEqual(['heading', 'max_items', 'show_badge'])
    expect(schema.settings.map((s) => s['type'])).toEqual(['text', 'number', 'toggle'])
    expect(schema.settings.map((s) => s['default'])).toEqual(['Welcome', 8, true])
    expect(schema.settings[1]!['meta']).toEqual({ min: 1, max: 24 })
  })

  test('a template with no section block yields a null schema, not an error', () => {
    const result = extractSchema('<p>just a template</p>')
    expect(result.ok).toBe(true)
    expect(result.errors).toHaveLength(0)
    expect(result.schema).toBeNull()
  })

  test('a section block without a name is an error', () => {
    const result = extractSchema('---\nsection {\n  settings {\n    a: text = "x"\n  }\n}\n---\n<p>x</p>')
    expect(result.ok).toBe(false)
    expect(result.errors.map((e) => e.code)).toContain('InvalidFrontmatter')
  })
})

describe('Compiler - render_section', () => {
  const sectionSource = `---
section {
  name: "Hero"
  settings {
    heading:   text = "Welcome"
    max_items: number = 8 { min: 1, max: 24 }
  }
}
---
<h1>\${ section.settings.heading }</h1><span>\${ section.settings.max_items }</span>`

  const ast = (): unknown => parseOk(sectionSource)

  test('renders schema defaults when the saved state is empty', () => {
    const result = renderSection(ast(), {})
    expect(result.ok).toBe(true)
    expect(squash(result.html)).toBe('<h1>Welcome</h1><span>8</span>')
  })

  test('saved state overrides a default, leaving the others alone', () => {
    const result = renderSection(ast(), { settings: { heading: 'Custom' } })
    expect(squash(result.html)).toBe('<h1>Custom</h1><span>8</span>')
  })

  test('the merge is tolerant: out-of-range numbers clamp and unknown keys drop', () => {
    // Tolerance is the point of this design: saved editor state must survive a
    // template change rather than failing the render.
    const clamped = renderSection(ast(), { settings: { max_items: 9999 } })
    expect(squash(clamped.html)).toBe('<h1>Welcome</h1><span>24</span>')
    expect(clamped.errors).toHaveLength(0)

    const unknown = renderSection(ast(), { settings: { bogus: 'x', heading: 'H' } })
    expect(squash(unknown.html)).toBe('<h1>H</h1><span>8</span>')
    expect(unknown.errors).toHaveLength(0)
  })
})
