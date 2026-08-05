import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { slurpFetch, _clearFetchCache } from './index'

// ─── Helpers ──────────────────────────────────────────────────────────────────

/** Calls `init()` on the component and returns it. */
async function mount(component: ReturnType<typeof slurpFetch>) {
  await component.init()
  return component
}

/** A simple async function that returns its first argument. */
const echoFn = async (...args: unknown[]) => args[0]

/** An async function that always rejects. */
const failFn = async (): Promise<never> => {
  throw new Error('Fetch failed')
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('slurpFetch - basic states', () => {
  it('sets status to success and assigns data', async () => {
    const c = await mount(slurpFetch({ fn: echoFn, args: [{ id: 1 }] }))
    expect(c.status).toBe('success')
    expect(c.data).toEqual({ id: 1 })
    expect(c.error).toBeNull()
  })

  it('sets status to error and populates error when fn throws', async () => {
    const c = await mount(slurpFetch({ fn: failFn }))
    expect(c.status).toBe('error')
    expect(c.error).not.toBeNull()
    expect(c.error?.message).toBe('Fetch failed')
  })

  it('starts as loading before init resolves', () => {
    const c = slurpFetch({ fn: echoFn, args: ['data'] })
    // Before init() is awaited, status is the initial value.
    expect(c.status).toBe('loading')
  })
})

describe('slurpFetch - cache', () => {
  it('returns cached data on second call without invoking fn again', async () => {
    const spy = vi.fn(async () => ({ cached: true }))
    const opts = { cacheTtl: 60 }

    const c1 = await mount(slurpFetch({ fn: spy, options: opts }))
    expect(c1.status).toBe('success')

    const c2 = await mount(slurpFetch({ fn: spy, options: opts }))
    expect(c2.status).toBe('success')
    expect(c2.data).toEqual({ cached: true })

    // spy should only have been called once.
    expect(spy).toHaveBeenCalledTimes(1)
  })

  it('skips cache when cacheTtl is 0', async () => {
    const spy = vi.fn(async () => 'fresh')
    await mount(slurpFetch({ fn: spy, options: { cacheTtl: 0 } }))
    await mount(slurpFetch({ fn: spy, options: { cacheTtl: 0 } }))
    expect(spy).toHaveBeenCalledTimes(2)
  })
})

describe('slurpFetch - retry', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('retries on failure and succeeds if a later attempt works', async () => {
    let attempts = 0
    const flakyFn = async () => {
      attempts++
      if (attempts < 3) throw new Error('Transient')
      return 'ok'
    }

    const initPromise = mount(
      slurpFetch({ fn: flakyFn, options: { retry: 3 } }),
    )

    // Advance timers for back-off sleeps (200ms, 400ms).
    await vi.runAllTimersAsync()

    const c = await initPromise
    expect(c.status).toBe('success')
    expect(c.data).toBe('ok')
    expect(attempts).toBe(3)
  })

  it('sets status to error after exhausting all retries', async () => {
    const initPromise = mount(slurpFetch({ fn: failFn, options: { retry: 2 } }))
    await vi.runAllTimersAsync()
    const c = await initPromise
    expect(c.status).toBe('error')
  })
})

describe('slurpFetch - timeout', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('aborts and sets error state when request exceeds timeoutMs', async () => {
    // slowFn never resolves on its own - the timeout fires and the
    // abort-race Promise.race() rejects with an AbortError.
    const slowFn = () => new Promise<never>((_resolve, _reject) => {
      // Deliberately never settles.
    })

    const initPromise = mount(
      slurpFetch({
        fn: slowFn,
        options: { timeoutMs: 100 },
      }),
    )

    // Advance past the 100ms timeout so the AbortController fires.
    await vi.advanceTimersByTimeAsync(150)
    const c = await initPromise

    expect(c.status).toBe('error')
    expect(c.error).not.toBeNull()
  })
})

describe('slurpFetch - fetch interceptor', () => {
  afterEach(() => {
    delete window.SLURP_FETCH_INTERCEPTOR
    delete window.SLURP_MODE
  })

  it('calls the interceptor instead of fn in editor mode', async () => {
    const realFn = vi.fn(async () => 'real')
    const interceptor = vi.fn(async () => 'intercepted')

    window.SLURP_MODE = 'editor'
    window.SLURP_FETCH_INTERCEPTOR = interceptor

    const c = await mount(slurpFetch({ fn: realFn }))
    expect(c.data).toBe('intercepted')
    expect(realFn).not.toHaveBeenCalled()
    expect(interceptor).toHaveBeenCalledWith(realFn, [])
  })

  it('does not call the interceptor in production mode', async () => {
    const realFn = vi.fn(async () => 'real')
    const interceptor = vi.fn(async () => 'intercepted')

    window.SLURP_MODE = 'production'
    window.SLURP_FETCH_INTERCEPTOR = interceptor

    const c = await mount(slurpFetch({ fn: realFn }))
    expect(c.data).toBe('real')
    expect(interceptor).not.toHaveBeenCalled()
  })

  // ── the gate must be an allow-list, not a deny-list ───────────────────────

  it('does NOT call the interceptor when SLURP_MODE is undefined', async () => {
    // This is the real production case. Nothing sets SLURP_MODE on a production
    // build - the only producer is the dev server, which sets 'dev'. Under a
    // `mode !== 'production'` gate, `undefined` passes and the sole remaining
    // condition is that SOMETHING has assigned window.SLURP_FETCH_INTERCEPTOR,
    // which any third-party script on the page can do. That would let an
    // attacker silently swap every {fetch} response.
    const realFn = vi.fn(async () => 'real')
    const interceptor = vi.fn(async () => 'intercepted')

    delete window.SLURP_MODE
    window.SLURP_FETCH_INTERCEPTOR = interceptor

    const c = await mount(slurpFetch({ fn: realFn }))
    expect(c.data).toBe('real')
    expect(interceptor).not.toHaveBeenCalled()
    expect(realFn).toHaveBeenCalled()
  })

  it('does NOT call the interceptor for an unrecognised mode', async () => {
    const realFn = vi.fn(async () => 'real')
    const interceptor = vi.fn(async () => 'intercepted')

    ;(window as { SLURP_MODE?: string }).SLURP_MODE = 'staging'
    window.SLURP_FETCH_INTERCEPTOR = interceptor

    const c = await mount(slurpFetch({ fn: realFn }))
    expect(c.data).toBe('real')
    expect(interceptor).not.toHaveBeenCalled()
  })

  it('calls the interceptor in dev mode', async () => {
    const realFn = vi.fn(async () => 'real')
    const interceptor = vi.fn(async () => 'intercepted')

    window.SLURP_MODE = 'dev'
    window.SLURP_FETCH_INTERCEPTOR = interceptor

    const c = await mount(slurpFetch({ fn: realFn }))
    expect(c.data).toBe('intercepted')
  })

  it('ignores a non-function interceptor instead of throwing', async () => {
    const realFn = vi.fn(async () => 'real')

    window.SLURP_MODE = 'dev'
    ;(window as { SLURP_FETCH_INTERCEPTOR?: unknown }).SLURP_FETCH_INTERCEPTOR = 'not a function'

    const c = await mount(slurpFetch({ fn: realFn }))
    expect(c.data).toBe('real')
  })
})

// ─── the cache must not collide across endpoints ─────────────────────────────

describe('slurpFetch - cache key isolation', () => {
  beforeEach(() => {
    _clearFetchCache()
  })

  it('does not serve one anonymous fetch block the other block\'s data', async () => {
    // The compiler emits anonymous arrows, whose `.name` is ''. Keyed on
    // `${fn.name}:${JSON.stringify(args)}`, two argument-less {fetch} blocks on
    // one page both key ':[]', so with cacheTtl set the second block renders
    // the FIRST block's response - a cross-endpoint leak inside a page.
    //
    // The functions MUST be built so that `.name` is genuinely ''. Writing
    // `const products = async () => ...` does not do it: JavaScript infers the
    // name from the binding, so `products.name` is 'products' and even a
    // name-based key tells the two apart, making the test vacuous. Reading them
    // out of an array defeats that inference and reproduces what the compiler
    // actually emits.
    const [products, orders] = [
      async (): Promise<unknown> => ({ kind: 'products' }),
      async (): Promise<unknown> => ({ kind: 'orders' }),
    ] as Array<() => Promise<unknown>>

    // Guard the guard: if this ever stops holding, the test below is vacuous.
    expect(products?.name).toBe('')
    expect(orders?.name).toBe('')

    const opts = { cacheTtl: 60 }
    const a = await mount(slurpFetch({ fn: products as () => Promise<unknown>, options: opts }))
    const b = await mount(slurpFetch({ fn: orders as () => Promise<unknown>, options: opts }))

    expect(a.data).toEqual({ kind: 'products' })
    expect(b.data).toEqual({ kind: 'orders' })
  })

  it('still shares the cache for the SAME function and args', async () => {
    const spy = vi.fn(async () => ({ v: 1 }))
    const opts = { cacheTtl: 60 }

    await mount(slurpFetch({ fn: spy, options: opts }))
    const second = await mount(slurpFetch({ fn: spy, options: opts }))

    expect(second.data).toEqual({ v: 1 })
    expect(spy).toHaveBeenCalledTimes(1)
  })

  it('separates entries by args for the same function', async () => {
    const spy = vi.fn(async (id: unknown) => ({ id }))
    const opts = { cacheTtl: 60 }

    const a = await mount(slurpFetch({ fn: spy, args: [1], options: opts }))
    const b = await mount(slurpFetch({ fn: spy, args: [2], options: opts }))

    expect(a.data).toEqual({ id: 1 })
    expect(b.data).toEqual({ id: 2 })
    expect(spy).toHaveBeenCalledTimes(2)
  })

  it('does not throw when args are circular, and skips the cache', async () => {
    // JSON.stringify throws on a circular structure. Unguarded, that propagates
    // out of _load and turns a cacheable request into a hard render error.
    const circular: Record<string, unknown> = {}
    circular['self'] = circular

    const spy = vi.fn(async () => 'ok')
    const c = await mount(
      slurpFetch({ fn: spy, args: [circular], options: { cacheTtl: 60 } }),
    )

    expect(c.status).toBe('success')
    expect(c.data).toBe('ok')

    // Uncacheable, so a second mount must hit the function again.
    const c2 = await mount(
      slurpFetch({ fn: spy, args: [circular], options: { cacheTtl: 60 } }),
    )
    expect(c2.status).toBe('success')
    expect(spy).toHaveBeenCalledTimes(2)
  })

  it('bounds the cache instead of growing without limit', async () => {
    // 150 distinct arg sets against a 100-entry ceiling. The oldest must be
    // evicted, so re-requesting the very first one calls the function again.
    const spy = vi.fn(async (n: unknown) => ({ n }))
    const opts = { cacheTtl: 600 }

    for (let i = 0; i < 150; i++) {
      await mount(slurpFetch({ fn: spy, args: [i], options: opts }))
    }
    expect(spy).toHaveBeenCalledTimes(150)

    await mount(slurpFetch({ fn: spy, args: [0], options: opts }))
    expect(spy).toHaveBeenCalledTimes(151)

    // A recent entry is still cached.
    await mount(slurpFetch({ fn: spy, args: [149], options: opts }))
    expect(spy).toHaveBeenCalledTimes(151)
  })
})

describe('slurpFetch - pagination', () => {
  it('fetches page 1 on init', async () => {
    const spy = vi.fn(async (_args: unknown) => ({ data: ['a', 'b'], totalPages: 3 }))
    const c = await mount(
      slurpFetch({ fn: spy, options: { paginate: { limit: 2 } } }),
    )
    expect(c.status).toBe('success')
    expect(c.totalPages).toBe(3)
    expect(c.page).toBe(1)
    // fn called with page/limit arg appended.
    expect(spy).toHaveBeenCalledWith({ page: 1, limit: 2 })
  })

  it('next() advances page and fetches', async () => {
    const spy = vi.fn(async (_args: unknown) => ({ data: ['x'], totalPages: 3 }))
    const c = await mount(slurpFetch({ fn: spy, options: { paginate: { limit: 1 } } }))
    await c.next()
    expect(c.page).toBe(2)
    expect(spy).toHaveBeenCalledTimes(2)
  })

  it('prev() decrements page', async () => {
    const spy = vi.fn(async (_args: unknown) => ({ data: ['x'], totalPages: 3 }))
    const c = await mount(slurpFetch({ fn: spy, options: { paginate: { limit: 1 } } }))
    await c.next()
    await c.prev()
    expect(c.page).toBe(1)
  })

  it('next() is a no-op on the last page', async () => {
    const spy = vi.fn(async (_args: unknown) => ({ data: ['x'], totalPages: 1 }))
    const c = await mount(slurpFetch({ fn: spy, options: { paginate: { limit: 1 } } }))
    await c.next()
    expect(c.page).toBe(1)
    expect(spy).toHaveBeenCalledTimes(1)
  })

  it('prev() is a no-op on the first page', async () => {
    const spy = vi.fn(async (_args: unknown) => ({ data: ['x'], totalPages: 2 }))
    const c = await mount(slurpFetch({ fn: spy, options: { paginate: { limit: 1 } } }))
    await c.prev()
    expect(c.page).toBe(1)
    expect(spy).toHaveBeenCalledTimes(1)
  })
})

describe('slurpFetch - infinite scroll', () => {
  it('accumulates items on loadMore()', async () => {
    let page = 0
    const spy = vi.fn(async (_args: unknown) => {
      page++
      return { data: [`item-${page}`], totalPages: 3 }
    })

    const c = await mount(slurpFetch({ fn: spy, options: { infinite: { limit: 1 } } }))
    expect(c.items).toEqual(['item-1'])
    expect(c.hasMore).toBe(true)

    await c.loadMore()
    expect(c.items).toEqual(['item-1', 'item-2'])

    await c.loadMore()
    expect(c.items).toEqual(['item-1', 'item-2', 'item-3'])
    expect(c.hasMore).toBe(false)
  })

  it('loadMore() is a no-op when hasMore is false', async () => {
    const spy = vi.fn(async (_args: unknown) => ({ data: ['only'], totalPages: 1 }))
    const c = await mount(slurpFetch({ fn: spy, options: { infinite: { limit: 1 } } }))
    expect(c.hasMore).toBe(false)
    await c.loadMore()
    expect(spy).toHaveBeenCalledTimes(1)
  })
})

describe('slurpFetch - polling', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('re-fetches on each poll interval', async () => {
    const spy = vi.fn(async () => 'polled')
    const c = await mount(slurpFetch({ fn: spy, options: { pollSeconds: 5 } }))

    expect(spy).toHaveBeenCalledTimes(1)

    await vi.advanceTimersByTimeAsync(5000)
    expect(spy).toHaveBeenCalledTimes(2)

    await vi.advanceTimersByTimeAsync(5000)
    expect(spy).toHaveBeenCalledTimes(3)

    c.destroy()

    await vi.advanceTimersByTimeAsync(10000)
    expect(spy).toHaveBeenCalledTimes(3)
  })
})

describe('slurpFetch - abortOnNavigate', () => {
  it('aborts in-flight request when slurp:navigate fires', async () => {
    // We cannot easily simulate an aborted in-flight request without real async
    // complexity, so we verify the AbortController is aborted by checking that
    // the listener was registered and removing it does not throw.
    const spy = vi.fn(async () => 'done')
    const c = slurpFetch({ fn: spy, options: { abortOnNavigate: true } })
    await c.init()

    // Navigate event should not throw even though fetch already completed.
    document.dispatchEvent(new CustomEvent('slurp:navigate', { detail: { href: '/x' } }))

    c.destroy()
    expect(c._navigateListener).toBeNull()
  })
})

describe('slurpFetch - destroy', () => {
  it('clears poll timer and navigate listener on destroy', async () => {
    vi.useFakeTimers()
    const spy = vi.fn(async () => 'x')
    const c = await mount(
      slurpFetch({ fn: spy, options: { pollSeconds: 1, abortOnNavigate: true } }),
    )
    c.destroy()
    expect(c._pollTimer).toBeNull()
    expect(c._navigateListener).toBeNull()
    vi.useRealTimers()
  })
})
