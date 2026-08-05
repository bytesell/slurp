/**
 * Runtime integration tests.
 *
 * These import the REAL runtime modules from `runtime/src` and exercise them
 * through the DOM. They used to inline COPY-PASTED reimplementations of
 * `matchRoute`, `parseParams`, `cn` and the navigation module and assert
 * against those copies, which is testing a fake: the copies could not fail when
 * the real modules broke, and in fact `cn`'s stand-in did not use
 * `tailwind-merge` at all while claiming to verify Tailwind conflict
 * resolution.
 *
 * Imports are by relative source path rather than through the `@bytesell/slurp-runtime`
 * package export map on purpose. The published entry points are side-effectful
 * bundles - importing `@bytesell/slurp-runtime` boots Alpine and starts client
 * navigation - so pulling one into a test would start the framework before the
 * test could set the page up.
 *
 * Canonical unit tests: runtime/src/core/{params,cn,navigation}.test.ts
 * This file is the cross-package check that those modules are importable and
 * behave the same when consumed from outside their own package.
 */

import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest'
import { parseParams, matchRoute } from '../../runtime/src/core/params'
import { cn } from '../../runtime/src/core/cn'
import {
  initNavigation,
  _resetNavigationForTesting,
} from '../../runtime/src/core/navigation'

// ─── Helpers ─────────────────────────────────────────────────────────────────

function buildPageHtml(content: string, title = 'New Page'): string {
  return `<!DOCTYPE html><html><head><title>${title}</title></head><body><div data-slurp-page>${content}</div></body></html>`
}

function createAnchor(href: string, attrs: Record<string, string> = {}): HTMLAnchorElement {
  const a = document.createElement('a')
  a.href = href
  for (const [k, v] of Object.entries(attrs)) a.setAttribute(k, v)
  document.body.appendChild(a)
  return a
}

function createPageSlot(content = '<p>Old</p>'): HTMLElement {
  const el = document.createElement('div')
  el.setAttribute('data-slurp-page', '')
  el.innerHTML = content
  document.body.appendChild(el)
  return el
}

// ─── parseParams ──────────────────────────────────────────────────────────────

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

  test('extracts slug from /products/:slug', () => {
    meta.content = '/products/:slug'
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      pathname: '/products/my-widget',
    })
    expect(parseParams()).toEqual({ slug: 'my-widget' })
  })

  test('extracts multiple params', () => {
    meta.content = '/blog/:year/:month/:slug'
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      pathname: '/blog/2024/03/hello-world',
    })
    expect(parseParams()).toEqual({ year: '2024', month: '03', slug: 'hello-world' })
  })

  test('returns empty object on no match', () => {
    meta.content = '/products/:slug'
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      pathname: '/orders/something',
    })
    expect(parseParams()).toEqual({})
  })

  test('returns empty if no meta tag present', () => {
    meta.remove()
    expect(parseParams()).toEqual({})
  })

  test('decodes percent-encoded param values', () => {
    meta.content = '/search/:query'
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      pathname: '/search/hello%20world',
    })
    expect(parseParams()).toEqual({ query: 'hello world' })
  })

  test('matches route with no dynamic segments', () => {
    meta.content = '/about/team'
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      pathname: '/about/team',
    })
    expect(parseParams()).toEqual({})
  })

  test('returns empty when static segment does not match', () => {
    meta.content = '/shop/:id'
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      pathname: '/blog/42',
    })
    expect(parseParams()).toEqual({})
  })
})

// ─── cn() ─────────────────────────────────────────────────────────────────────

describe('cn()', () => {
  test('combines class names', () => {
    const result = cn('px-4', 'py-2', 'bg-blue-500')
    expect(result).toContain('px-4')
    expect(result).toContain('py-2')
    expect(result).toContain('bg-blue-500')
  })

  test('filters falsy values', () => {
    const result = cn('px-4', false, null, undefined, 'py-2')
    expect(result).toContain('px-4')
    expect(result).toContain('py-2')
    expect(result).not.toContain('false')
    expect(result).not.toContain('null')
    expect(result).not.toContain('undefined')
  })

  test('merges tailwind conflicts - last class wins', () => {
    // px-4 followed by px-6: only px-6 should survive
    const result = cn('px-4', 'px-6')
    expect(result).toContain('px-6')
    expect(result).not.toContain('px-4')
  })

  test('handles arrays', () => {
    const result = cn(['px-4', 'py-2'], 'text-sm')
    expect(result).toContain('px-4')
    expect(result).toContain('py-2')
    expect(result).toContain('text-sm')
  })

  test('handles objects', () => {
    const result = cn({
      'bg-blue-500': true,
      'bg-red-500': false,
      'text-white': true,
    })
    expect(result).toContain('bg-blue-500')
    expect(result).not.toContain('bg-red-500')
    expect(result).toContain('text-white')
  })

  test('handles empty input', () => {
    expect(cn()).toBe('')
  })

  test('handles all falsy input', () => {
    expect(cn(false, null, undefined)).toBe('')
  })

  test('handles mixed array and object', () => {
    const result = cn(['text-sm', 'font-bold'], { 'text-blue-500': true, 'text-red-500': false })
    expect(result).toContain('font-bold')
    expect(result).toContain('text-blue-500')
    expect(result).not.toContain('text-red-500')
  })
})

// ─── Client navigation ────────────────────────────────────────────────────────

describe('Client navigation', () => {
  let fetchMock: ReturnType<typeof vi.fn>

  beforeEach(() => {
    // The real module guards re-entry with a module-level `initialized` flag and
    // keeps its listeners on `document`, so without this reset a listener from
    // an earlier test keeps handling clicks and the "should be ignored" cases
    // see a navigation they did not trigger. The inlined stand-in this file used
    // to test had no such state, which is precisely the kind of behaviour a
    // reimplementation silently fails to model.
    _resetNavigationForTesting()
    document.body.innerHTML = ''
    document.body.removeAttribute('navigate')
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  afterEach(() => {
    _resetNavigationForTesting()
    document.body.innerHTML = ''
    document.body.removeAttribute('navigate')
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  test('intercepts <a> clicks when body has navigate attribute', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      text: async () => buildPageHtml('<p>New</p>'),
    })
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(`${window.location.origin}/new-page`)
    a.click()

    await vi.waitFor(() => {
      expect(fetchMock).toHaveBeenCalledOnce()
    })

    document.body.removeAttribute('navigate')
  })

  test('ignores clicks when body lacks navigate attribute', async () => {
    // No 'navigate' attribute on body
    fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    initNavigation()

    const a = createAnchor(`${window.location.origin}/page`)
    a.click()

    await new Promise((r) => setTimeout(r, 50))
    expect(fetchMock).not.toHaveBeenCalled()
  })

  test('ignores navigate=false links', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(`${window.location.origin}/page`, { navigate: 'false' })
    a.click()

    await new Promise((r) => setTimeout(r, 50))
    expect(fetchMock).not.toHaveBeenCalled()

    document.body.removeAttribute('navigate')
  })

  test('ignores external links', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor('https://external-site.com/page')
    a.click()

    await new Promise((r) => setTimeout(r, 50))
    expect(fetchMock).not.toHaveBeenCalled()

    document.body.removeAttribute('navigate')
  })

  test('ignores ctrl+click', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(`${window.location.origin}/ctrl`)
    const event = new MouseEvent('click', { bubbles: true, ctrlKey: true })
    a.dispatchEvent(event)

    await new Promise((r) => setTimeout(r, 50))
    expect(fetchMock).not.toHaveBeenCalled()

    document.body.removeAttribute('navigate')
  })

  test('ignores meta+click (⌘ on macOS)', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(`${window.location.origin}/meta`)
    const event = new MouseEvent('click', { bubbles: true, metaKey: true })
    a.dispatchEvent(event)

    await new Promise((r) => setTimeout(r, 50))
    expect(fetchMock).not.toHaveBeenCalled()

    document.body.removeAttribute('navigate')
  })

  test('ignores shift+click', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(`${window.location.origin}/shift`)
    const event = new MouseEvent('click', { bubbles: true, shiftKey: true })
    a.dispatchEvent(event)

    await new Promise((r) => setTimeout(r, 50))
    expect(fetchMock).not.toHaveBeenCalled()

    document.body.removeAttribute('navigate')
  })

  test('fires slurp:navigate event after successful navigation', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      text: async () => buildPageHtml('<p>New content</p>'),
    })
    vi.stubGlobal('fetch', fetchMock)

    const events: CustomEvent[] = []
    document.addEventListener('slurp:navigate', (e) => events.push(e as CustomEvent))

    const a = createAnchor(`${window.location.origin}/destination`)
    a.click()

    await vi.waitFor(() => {
      expect(events).toHaveLength(1)
    })

    expect(events[0]?.detail?.href).toContain('/destination')

    document.body.removeAttribute('navigate')
  })

  test('swaps [data-slurp-page] content after navigation', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot('<p>Old</p>')
    initNavigation()

    fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      text: async () => buildPageHtml('<p>Injected</p>'),
    })
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(`${window.location.origin}/next`)
    a.click()

    await vi.waitFor(() => {
      expect(document.querySelector('[data-slurp-page]')?.textContent).toContain('Injected')
    })

    document.body.removeAttribute('navigate')
  })

  test('updates document.title after navigation', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      text: async () => buildPageHtml('<p>x</p>', 'Updated Title'),
    })
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(`${window.location.origin}/title-page`)
    a.click()

    await vi.waitFor(() => {
      expect(document.title).toBe('Updated Title')
    })

    document.body.removeAttribute('navigate')
  })

  test('does not crash when fetch throws a network error', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn().mockRejectedValue(new Error('Network error'))
    vi.stubGlobal('fetch', fetchMock)
    const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    const a = createAnchor(`${window.location.origin}/bad`)
    a.click()

    await vi.waitFor(() => {
      expect(consoleSpy).toHaveBeenCalled()
    })

    document.body.removeAttribute('navigate')
  })

  test('does not navigate when fetch returns non-ok status', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot('<p>Original</p>')
    initNavigation()

    fetchMock = vi.fn().mockResolvedValue({ ok: false, status: 404, statusText: 'Not Found' })
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(`${window.location.origin}/missing`)
    a.click()

    await new Promise((r) => setTimeout(r, 100))
    // Page slot content should remain unchanged
    expect(document.querySelector('[data-slurp-page]')?.textContent).toContain('Original')

    document.body.removeAttribute('navigate')
  })

  test('ignores hash-only navigations on the same page', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(
      `${window.location.origin}${window.location.pathname}#section`,
    )
    a.click()

    await new Promise((r) => setTimeout(r, 50))
    expect(fetchMock).not.toHaveBeenCalled()

    document.body.removeAttribute('navigate')
  })

  test('sends X-Slurp-Navigate header with navigation requests', async () => {
    document.body.setAttribute('navigate', '')
    createPageSlot()
    initNavigation()

    const capturedCalls: RequestInit[] = []
    fetchMock = vi.fn().mockImplementation((_url: string, init: RequestInit) => {
      capturedCalls.push(init)
      return Promise.resolve({
        ok: true,
        text: async () => buildPageHtml('<p>ok</p>'),
      })
    })
    vi.stubGlobal('fetch', fetchMock)

    const a = createAnchor(`${window.location.origin}/some-page`)
    a.click()

    await vi.waitFor(() => {
      expect(capturedCalls.length).toBeGreaterThan(0)
    })

    const headers = capturedCalls[0]?.headers as Record<string, string>
    expect(headers?.['X-Slurp-Navigate']).toBe('1')

    document.body.removeAttribute('navigate')
  })
})
