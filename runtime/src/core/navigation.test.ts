import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { initNavigation, _resetNavigationForTesting } from './navigation'

// ─── Helpers ──────────────────────────────────────────────────────────────────

/** Creates a minimal page slot in the document body. */
function createPageSlot(content = '<p>Old content</p>'): HTMLElement {
  const slot = document.createElement('div')
  slot.setAttribute('data-slurp-page', '')
  slot.innerHTML = content
  document.body.appendChild(slot)
  return slot
}

/** Builds a full HTML document string with a page slot containing `content`. */
function buildPageHtml(content: string, title = 'New Page'): string {
  return `<!DOCTYPE html><html><head><title>${title}</title></head><body><div data-slurp-page>${content}</div></body></html>`
}

/** Creates an anchor element, appends to body, and returns it. */
function createAnchor(href: string, attrs: Record<string, string> = {}): HTMLAnchorElement {
  const a = document.createElement('a')
  a.href = href
  for (const [k, v] of Object.entries(attrs)) {
    a.setAttribute(k, v)
  }
  document.body.appendChild(a)
  return a
}

// ─── Shared fetch mock (reset per test) ──────────────────────────────────────

let fetchMock: ReturnType<typeof vi.fn>

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('initNavigation', () => {
  beforeEach(() => {
    _resetNavigationForTesting()
    document.body.innerHTML = ''
  })

  afterEach(() => {
    _resetNavigationForTesting()
    document.body.innerHTML = ''
  })

  it('does nothing when <body> lacks the navigate attribute', () => {
    const spy = vi.spyOn(document, 'addEventListener')
    initNavigation()
    const clickCalls = spy.mock.calls.filter(([event]) => event === 'click')
    expect(clickCalls).toHaveLength(0)
    spy.mockRestore()
  })

  it('adds click listener when <body> has navigate attribute', () => {
    document.body.setAttribute('navigate', '')
    const spy = vi.spyOn(document, 'addEventListener')
    initNavigation()
    const clickCalls = spy.mock.calls.filter(([event]) => event === 'click')
    expect(clickCalls).toHaveLength(1)
    spy.mockRestore()
    document.body.removeAttribute('navigate')
  })

  it('does not register duplicate listeners on repeated calls', () => {
    document.body.setAttribute('navigate', '')
    const spy = vi.spyOn(document, 'addEventListener')
    initNavigation()
    initNavigation()
    initNavigation()
    const clickCalls = spy.mock.calls.filter(([event]) => event === 'click')
    expect(clickCalls).toHaveLength(1)
    spy.mockRestore()
    document.body.removeAttribute('navigate')
  })
})

describe('click interception', () => {
  beforeEach(() => {
    _resetNavigationForTesting()
    document.body.innerHTML = ''
    document.body.setAttribute('navigate', '')
    createPageSlot()

    // Give every test a fresh fetch mock. Unhandled calls return a rejected
    // promise to avoid silent bleed from jsdom's stray popstate events.
    fetchMock = vi.fn().mockRejectedValue(new Error('fetch not mocked for this call'))
    vi.stubGlobal('fetch', fetchMock)

    initNavigation()
  })

  afterEach(() => {
    _resetNavigationForTesting()
    document.body.removeAttribute('navigate')
    document.body.innerHTML = ''
    vi.unstubAllGlobals()
    vi.clearAllMocks()
  })

  it('intercepts same-origin anchor clicks and calls fetch', async () => {
    fetchMock.mockResolvedValueOnce({
      ok: true,
      text: async () => buildPageHtml('<p>New content</p>'),
    })

    const a = createAnchor(`${window.location.origin}/new-page`)
    a.click()

    // Wait until fetch has been called exactly once.
    await vi.waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        `${window.location.origin}/new-page`,
        expect.objectContaining({ headers: { 'X-Slurp-Navigate': '1' } }),
      )
    })
  })

  it('does not intercept clicks on anchors with navigate="false"', async () => {
    const a = createAnchor(`${window.location.origin}/new-page`, { navigate: 'false' })
    a.click()

    await new Promise((r) => setTimeout(r, 50))
    // fetch should not have been called with the navigate URL.
    const navigateCalls = fetchMock.mock.calls.filter(
      ([url]) => typeof url === 'string' && (url as string).includes('/new-page'),
    )
    expect(navigateCalls).toHaveLength(0)
  })

  // REGRESSION: the compiler emitted `data-navigate="false"` while this file read `navigate`,
  // so the documented opt-out did nothing on every page that used it. The names have to agree,
  // and this is the runtime half of that agreement (the compiler half is
  // `test_element_navigate_false`). The `data-` form must stay INERT: if it ever starts working
  // here, both spellings are live and the next person to change one will fix half the feature.
  it('reads the navigate attribute, not data-navigate', async () => {
    const dataForm = createAnchor(`${window.location.origin}/data-form`, {
      'data-navigate': 'false',
    })
    dataForm.click()
    await new Promise((r) => setTimeout(r, 50))
    expect(
      fetchMock.mock.calls.filter(
        ([url]) => typeof url === 'string' && (url as string).includes('/data-form'),
      ).length,
    ).toBeGreaterThan(0)
  })

  it('does not intercept external links', async () => {
    const a = createAnchor('https://example.com/external')
    a.click()

    await new Promise((r) => setTimeout(r, 50))
    const externalCalls = fetchMock.mock.calls.filter(
      ([url]) => typeof url === 'string' && (url as string).includes('example.com'),
    )
    expect(externalCalls).toHaveLength(0)
  })

  it('does not intercept hash-only navigations on the same page', async () => {
    // Use the current pathname so the guard fires correctly.
    const currentPath = window.location.pathname
    const a = createAnchor(`${window.location.origin}${currentPath}#section`)
    a.click()

    await new Promise((r) => setTimeout(r, 50))
    const hashCalls = fetchMock.mock.calls.filter(
      ([url]) => typeof url === 'string' && (url as string).includes('#section'),
    )
    expect(hashCalls).toHaveLength(0)
  })

  it('swaps page slot content after successful fetch', async () => {
    fetchMock.mockResolvedValueOnce({
      ok: true,
      text: async () => buildPageHtml('<p>Injected</p>'),
    })

    const a = createAnchor(`${window.location.origin}/next`)
    a.click()

    await vi.waitFor(() => {
      expect(document.querySelector('[data-slurp-page]')?.textContent).toContain('Injected')
    })
  })

  it('dispatches slurp:navigate CustomEvent after swap', async () => {
    fetchMock.mockResolvedValueOnce({
      ok: true,
      text: async () => buildPageHtml('<p>New</p>'),
    })

    const events: CustomEvent[] = []
    const listener = (e: Event): void => { events.push(e as CustomEvent) }
    document.addEventListener('slurp:navigate', listener)

    try {
      const a = createAnchor(`${window.location.origin}/event-test`)
      a.click()

      await vi.waitFor(() => {
        expect(events.filter((e) => (e.detail as { href: string }).href.includes('/event-test'))).toHaveLength(1)
      })
    } finally {
      document.removeEventListener('slurp:navigate', listener)
    }
  })

  it('updates document.title after navigation', async () => {
    fetchMock.mockResolvedValueOnce({
      ok: true,
      text: async () => buildPageHtml('<p>x</p>', 'My New Title'),
    })

    const a = createAnchor(`${window.location.origin}/title-test`)
    a.click()

    await vi.waitFor(() => {
      expect(document.title).toBe('My New Title')
    })
  })

  it('logs an error when fetch fails', async () => {
    fetchMock.mockRejectedValueOnce(new Error('Network error'))
    const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined)

    try {
      const a = createAnchor(`${window.location.origin}/bad`)
      a.click()

      await vi.waitFor(() => {
        expect(consoleSpy).toHaveBeenCalled()
      })
    } finally {
      consoleSpy.mockRestore()
    }
  })

  it('ignores ctrl+click', async () => {
    const a = createAnchor(`${window.location.origin}/ctrl`)
    const event = new MouseEvent('click', { bubbles: true, ctrlKey: true })
    a.dispatchEvent(event)

    await new Promise((r) => setTimeout(r, 30))
    const ctrlCalls = fetchMock.mock.calls.filter(
      ([url]) => typeof url === 'string' && (url as string).includes('/ctrl'),
    )
    expect(ctrlCalls).toHaveLength(0)
  })

  it('ignores meta+click', async () => {
    const a = createAnchor(`${window.location.origin}/meta`)
    const event = new MouseEvent('click', { bubbles: true, metaKey: true })
    a.dispatchEvent(event)

    await new Promise((r) => setTimeout(r, 30))
    const metaCalls = fetchMock.mock.calls.filter(
      ([url]) => typeof url === 'string' && (url as string).includes('/meta'),
    )
    expect(metaCalls).toHaveLength(0)
  })
})
