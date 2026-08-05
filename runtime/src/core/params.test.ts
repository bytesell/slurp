import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { parseParams, matchRoute } from './params'

// ─── matchRoute (pure, no DOM) ────────────────────────────────────────────────

describe('matchRoute', () => {
  it('returns empty object for root → root', () => {
    expect(matchRoute('/', '/')).toEqual({})
  })

  it('extracts a single named param', () => {
    expect(matchRoute('/products/:slug', '/products/my-widget')).toEqual({ slug: 'my-widget' })
  })

  it('extracts multiple named params', () => {
    expect(matchRoute('/blog/:year/:month/:slug', '/blog/2024/03/hello-world')).toEqual({
      year: '2024',
      month: '03',
      slug: 'hello-world',
    })
  })

  it('returns empty object when segment count differs', () => {
    expect(matchRoute('/a/:b', '/a/b/c')).toEqual({})
  })

  it('returns empty object when a static segment does not match', () => {
    expect(matchRoute('/products/:slug', '/orders/my-widget')).toEqual({})
  })

  it('decodes percent-encoded param values', () => {
    expect(matchRoute('/products/:slug', '/products/hello%20world')).toEqual({
      slug: 'hello world',
    })
  })

  it('matches a route with no params (all static)', () => {
    expect(matchRoute('/about/team', '/about/team')).toEqual({})
  })

  it('returns empty object when static route has wrong value', () => {
    expect(matchRoute('/about/team', '/about/contact')).toEqual({})
  })

  it('handles a param at the root level', () => {
    expect(matchRoute('/:lang', '/en')).toEqual({ lang: 'en' })
  })

  it('handles adjacent params', () => {
    expect(matchRoute('/:a/:b', '/foo/bar')).toEqual({ a: 'foo', b: 'bar' })
  })
})

// ─── parseParams (DOM-dependent) ─────────────────────────────────────────────

describe('parseParams', () => {
  let meta: HTMLMetaElement

  beforeEach(() => {
    meta = document.createElement('meta')
    meta.name = 'slurp-route'
    document.head.appendChild(meta)
  })

  afterEach(() => {
    meta.remove()
    vi.restoreAllMocks()
  })

  it('returns empty object when meta tag is absent', () => {
    meta.remove()
    expect(parseParams()).toEqual({})
  })

  it('returns params matched against location.pathname', () => {
    meta.content = '/products/:slug'
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      pathname: '/products/cool-thing',
    })
    expect(parseParams()).toEqual({ slug: 'cool-thing' })
  })

  it('returns empty object when pathname does not match pattern', () => {
    meta.content = '/products/:slug'
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      pathname: '/orders/cool-thing',
    })
    expect(parseParams()).toEqual({})
  })

  it('returns empty object for empty content', () => {
    meta.content = ''
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      pathname: '',
    })
    // Both are '', split('/') => ['', ''], so they match with no params.
    expect(parseParams()).toEqual({})
  })
})

// ─── a malformed percent-escape must not kill the page ───────────────────────

describe('matchRoute - malformed percent-escapes', () => {
  // `decodeURIComponent` throws URIError on these. matchRoute runs during boot,
  // BEFORE Alpine.start(), so an uncaught throw here leaves the whole page
  // without any interactivity.

  it('does not throw on a bare percent sign', () => {
    expect(() => matchRoute('/products/:slug', '/products/%')).not.toThrow()
  })

  it('passes a bare percent through verbatim', () => {
    expect(matchRoute('/products/:slug', '/products/%')).toEqual({ slug: '%' })
  })

  it('does not throw on an invalid hex escape', () => {
    expect(matchRoute('/products/:slug', '/products/%zz')).toEqual({ slug: '%zz' })
  })

  it('does not throw on a truncated escape', () => {
    expect(matchRoute('/products/:slug', '/products/%E0%A4')).toEqual({ slug: '%E0%A4' })
  })

  it('still decodes the other segments when one is malformed', () => {
    expect(matchRoute('/b/:a/:b', '/b/%/hello%20world')).toEqual({
      a: '%',
      b: 'hello world',
    })
  })

  it('still decodes a valid escape', () => {
    expect(matchRoute('/s/:q', '/s/hello%20world')).toEqual({ q: 'hello world' })
  })
})
