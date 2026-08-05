import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

// ─── IntersectionObserver mock ────────────────────────────────────────────────
// jsdom does not implement IntersectionObserver, so we provide a controllable
// fake that lets tests simulate entries entering the viewport.

type IOCallback = (entries: IntersectionObserverEntry[]) => void

const observedElements = new Map<Element, IOCallback>()

class FakeIntersectionObserver {
  private callback: IOCallback
  private options: IntersectionObserverInit | undefined

  constructor(callback: IOCallback, options?: IntersectionObserverInit) {
    this.callback = callback
    this.options = options
  }

  observe(el: Element): void {
    observedElements.set(el, this.callback)
  }

  disconnect(): void {
    for (const [el, cb] of observedElements) {
      if (cb === this.callback) observedElements.delete(el)
    }
  }

  unobserve(el: Element): void {
    observedElements.delete(el)
  }

  // Vitest requires this to satisfy IntersectionObserver interface.
  takeRecords(): IntersectionObserverEntry[] {
    return []
  }

  get root() { return this.options?.root ?? null }
  get rootMargin() { return this.options?.rootMargin ?? '0px' }
  get thresholds() { return Array.isArray(this.options?.threshold) ? this.options.threshold : [this.options?.threshold ?? 0] }
}

vi.stubGlobal('IntersectionObserver', FakeIntersectionObserver)

/** Simulates a sentinel entering the viewport. */
function triggerIntersect(sentinel: Element): void {
  const callback = observedElements.get(sentinel)
  if (!callback) return
  callback([
    {
      isIntersecting: true,
      target: sentinel,
      // Fill required IntersectionObserverEntry fields with stubs.
      boundingClientRect: new DOMRect(),
      intersectionRatio: 1,
      intersectionRect: new DOMRect(),
      rootBounds: null,
      time: performance.now(),
    } as IntersectionObserverEntry,
  ])
}

// Import after stubbing IntersectionObserver.
const { initInfiniteScroll, _disconnectAll, _observeAll } = await import('./index')

// ─── Helpers ──────────────────────────────────────────────────────────────────

/** Creates a sentinel element and optionally attaches a fake Alpine component. */
function createSentinel(
  hasMore = true,
  loadMoreImpl: () => Promise<void> = async () => { /* no-op */ },
): HTMLElement {
  const wrapper = document.createElement('div')
  // Simulate Alpine v3's _x_dataStack on the wrapper element (x-data root).
  const component = { hasMore, loadMore: loadMoreImpl }
  ;(wrapper as HTMLElement & { _x_dataStack?: (typeof component)[] })._x_dataStack = [component]

  const sentinel = document.createElement('div')
  sentinel.setAttribute('data-slurp-sentinel', '')
  wrapper.appendChild(sentinel)
  document.body.appendChild(wrapper)

  return sentinel
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('initInfiniteScroll', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    observedElements.clear()
  })

  afterEach(() => {
    _disconnectAll()
    document.body.innerHTML = ''
    observedElements.clear()
  })

  it('observes sentinel elements on init', () => {
    const sentinel = createSentinel()
    _observeAll()
    expect(observedElements.has(sentinel)).toBe(true)
  })

  it('calls loadMore() when sentinel intersects and hasMore is true', async () => {
    const loadMore = vi.fn(async () => { /* no-op */ })
    const sentinel = createSentinel(true, loadMore)
    _observeAll()

    triggerIntersect(sentinel)
    await vi.waitFor(() => expect(loadMore).toHaveBeenCalledOnce())
  })

  it('does not call loadMore() when hasMore is false', async () => {
    const loadMore = vi.fn(async () => { /* no-op */ })
    createSentinel(false, loadMore)
    _observeAll()

    // Even if the sentinel intersects, hasMore is false so loadMore skips.
    await new Promise((r) => setTimeout(r, 50))
    expect(loadMore).not.toHaveBeenCalled()
  })

  it('disconnects observer after loadMore sets hasMore to false', async () => {
    let callCount = 0
    let hasMore = true

    const sentinel = createSentinel(hasMore, async () => {
      callCount++
      hasMore = false
      // Update the component's hasMore so the disconnect fires.
      const wrapper = sentinel.parentElement as HTMLElement & {
        _x_dataStack?: { hasMore: boolean; loadMore: () => Promise<void> }[]
      }
      const scope = wrapper._x_dataStack?.[0]
      if (scope) scope.hasMore = false
    })

    _observeAll()
    triggerIntersect(sentinel)

    await vi.waitFor(() => expect(callCount).toBe(1))
    // After loadMore ran, observer should have disconnected.
    expect(observedElements.has(sentinel)).toBe(false)
  })

  it('re-observes sentinels after slurp:navigate', async () => {
    initInfiniteScroll()

    // Create a sentinel after navigation.
    document.body.innerHTML = ''
    observedElements.clear()
    const sentinel = createSentinel()

    document.dispatchEvent(new CustomEvent('slurp:navigate', { detail: { href: '/new' } }))

    await vi.waitFor(() => expect(observedElements.has(sentinel)).toBe(true))
  })

  it('does not double-observe the same sentinel', () => {
    const sentinel = createSentinel()
    _observeAll()
    _observeAll() // second call

    // The sentinel should appear in observedElements only once.
    let count = 0
    for (const [el] of observedElements) {
      if (el === sentinel) count++
    }
    expect(count).toBe(1)
  })
})
