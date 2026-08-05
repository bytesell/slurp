/**
 * @bytesell/slurp-runtime - fetch entry point (dist/slurp.fetch.js)
 *
 * Provides the `slurpFetch` Alpine data factory used by compiled `{fetch}`
 * blocks. Handles:
 *
 * - Initial data load with status transitions (loading → success | error)
 * - Response caching with configurable TTL
 * - A total time budget across all retries (see `timeoutMs`: it stops WAITING,
 *   it does not cancel the underlying request)
 * - Retry with exponential back-off
 * - Fetch interceptor in editor / dev mode
 * - Pagination (`page` / `totalPages` / `prev()` / `next()`)
 * - Infinite scroll accumulation (`items` / `hasMore` / `loadMore()`)
 * - Polling via `setInterval`
 * - Abort on client navigation via the `slurp:navigate` event
 *
 * Compiler emits usage like:
 * ```html
 * <div x-data="slurpFetch({ fn: myFetchFn, options: { cacheTtl: 60 } })">
 *   <template x-if="$data.status === 'success'">...</template>
 * </div>
 * ```
 */

import type { FetchErrorState } from '../types'

/** Options accepted by the `slurpFetch` data factory. */
interface FetchOptions {
  /**
   * Cache time-to-live in seconds. When > 0 the response is stored in an
   * in-memory cache keyed by function name + serialised args. Subsequent calls
   * within the TTL window skip the network entirely.
   */
  cacheTtl?: number

  /**
   * Maximum number of retry attempts on network error or non-2xx status.
   * Retries use exponential back-off: 200 ms, 400 ms, 800 ms...
   * Default: 0 (no retries).
   */
  retry?: number

  /**
   * Total time budget in milliseconds for the whole load, INCLUDING every retry
   * and every back-off sleep. Default: no timeout.
   *
   * Two things this deliberately is not, both easy to assume:
   *
   * 1. It is not per-attempt. The timer is armed once before the retry loop, so
   *    `{ retry: 3, timeoutMs: 1000 }` gives the three attempts 1000 ms between
   *    them, not 1000 ms each.
   * 2. It does not CANCEL the request. `slurpFetch` owns an `AbortController`
   *    but never passes its signal to `config.fn`, because `fn` is compiler-
   *    generated and takes no signal argument. On expiry the pending promise is
   *    abandoned by losing a `Promise.race`, so the component stops waiting and
   *    reports an error while the underlying request runs to completion. That
   *    matters for a non-idempotent `fn`: the work still happens.
   */
  timeoutMs?: number

  /**
   * When `true`, any in-flight request is aborted when a `slurp:navigate`
   * event fires (i.e. the user navigated away before the response arrived).
   */
  abortOnNavigate?: boolean

  /**
   * When set, the component re-fetches every N seconds in the background.
   * The interval is cleared when the component is destroyed.
   */
  pollSeconds?: number

  /**
   * Activates pagination mode.
   *
   * Appends one extra ARGUMENT, `{ page, limit }`, to the arguments passed to
   * `config.fn`. It does not touch a URL and `fn` need not return one - paging
   * is `fn`'s business, and this only tells it which page to get. `fn` is
   * expected to resolve to `{ data, totalPages }`.
   */
  paginate?: { limit: number }

  /**
   * Activates infinite scroll mode. Passes the same `{ page, limit }` argument
   * as `paginate` and accumulates results in `items[]`.
   */
  infinite?: { limit: number }
}

/** Shape passed to the `slurpFetch` factory by compiled templates. */
interface FetchDataOptions {
  /** The async function that fetches data. Must return a value or throw. */
  fn: (...args: unknown[]) => Promise<unknown>
  /** Arguments forwarded to `fn` on every call. */
  args?: unknown[]
  options?: FetchOptions
}

/** A single in-memory cache entry. */
interface CacheEntry {
  data: unknown
  expiresAt: number
}

/** Shape of a paginated API response envelope expected by slurpFetch. */
interface PaginatedResponse {
  data: unknown[]
  totalPages: number
}

/**
 * Shared in-memory response cache.
 *
 * Keyed by a per-FUNCTION identity plus the serialised args. `fn.name` alone is
 * NOT a usable identity: the compiler emits anonymous arrow functions, whose
 * `.name` is the empty string, so two argument-less `{fetch}` blocks on one
 * page both key `":[]"`. With `cacheTtl` set, the second block then renders the
 * FIRST block's response - a cross-endpoint data leak within a page, not merely
 * a stale read.
 */
const cache = new Map<string, CacheEntry>()

/**
 * Largest number of entries kept. Eviction has to be bounded here rather than
 * left to a lazy per-key check on read: `paginate` plus `pollSeconds` mints a
 * fresh key on every tick, so a read-only eviction policy grows the cache
 * without limit for the lifetime of the page.
 */
const MAX_CACHE_ENTRIES = 100

/**
 * Stable per-function cache identity.
 *
 * A WeakMap keyed on the function object distinguishes two anonymous functions
 * that share the empty name, and lets an unreferenced fetch function (and its
 * identity) be collected. `fn.name` is kept in the key purely to keep the key
 * readable when debugging.
 */
const fnIdentities = new WeakMap<object, number>()
let nextFnIdentity = 0

function identityOf(fn: (...args: unknown[]) => Promise<unknown>): number {
  let id = fnIdentities.get(fn)
  if (id === undefined) {
    id = nextFnIdentity++
    fnIdentities.set(fn, id)
  }
  return id
}

/**
 * Builds the cache key for a given function + args combination.
 *
 * @returns The key, or `null` when the args cannot be serialised, in which case
 *   the caller must skip the cache rather than fail the request.
 */
function buildCacheKey(
  fn: (...args: unknown[]) => Promise<unknown>,
  args: unknown[],
): string | null {
  let serialisedArgs: string
  try {
    serialisedArgs = JSON.stringify(args) ?? 'undefined'
  } catch {
    // JSON.stringify throws on a circular structure or a BigInt. Losing the
    // cache is correct here; throwing out of _load would turn a cacheable
    // request into a hard render error.
    return null
  }
  return `${identityOf(fn)}:${fn.name}:${serialisedArgs}`
}

/**
 * Reads a cached response if one exists and has not expired.
 * Returns `null` when the cache is cold or stale.
 */
function readCache(key: string): unknown | null {
  const entry = cache.get(key)
  if (!entry) return null
  if (Date.now() > entry.expiresAt) {
    cache.delete(key)
    return null
  }
  return entry.data
}

/**
 * Writes a response to the cache with an expiry derived from `ttlSeconds`,
 * evicting expired entries and then the oldest key once the cache is full.
 */
function writeCache(key: string, data: unknown, ttlSeconds: number): void {
  if (!cache.has(key) && cache.size >= MAX_CACHE_ENTRIES) {
    const now = Date.now()
    for (const [k, entry] of cache) {
      if (now > entry.expiresAt) cache.delete(k)
    }
    // Map preserves insertion order, so the first remaining key is the oldest.
    while (cache.size >= MAX_CACHE_ENTRIES) {
      const oldest = cache.keys().next()
      if (oldest.done === true) break
      cache.delete(oldest.value)
    }
  }
  cache.set(key, { data, expiresAt: Date.now() + ttlSeconds * 1000 })
}

/**
 * Clears the shared response cache. Test-only.
 * @internal
 */
export function _clearFetchCache(): void {
  cache.clear()
}

/**
 * Calls the supplied fetch function, routing through the editor/dev fetch
 * interceptor when one is present and the runtime is not in production mode.
 */
async function callFn(
  fn: (...args: unknown[]) => Promise<unknown>,
  args: unknown[],
): Promise<unknown> {
  const mode = window.SLURP_MODE
  const interceptor = window.SLURP_FETCH_INTERCEPTOR

  // ALLOW-LIST, never a deny-list.
  //
  // A gate of `mode !== 'production'` is unsafe here. Nothing sets SLURP_MODE
  // on a production build - the only producer is the dev server, which sets
  // 'dev' - so on a real page `mode` is `undefined`, that comparison is true,
  // and the ONLY remaining condition is that something has assigned
  // `window.SLURP_FETCH_INTERCEPTOR`, which any third-party script on the page
  // can do, silently swapping every {fetch} response. Naming the two modes that
  // may intercept makes an absent or unrecognised mode fail CLOSED, which is
  // the safe direction.
  const interceptionAllowed = mode === 'dev' || mode === 'editor'
  if (interceptionAllowed && typeof interceptor === 'function') {
    return interceptor(fn, args)
  }
  return fn(...args)
}

/**
 * Sleeps for `ms` milliseconds. Used for retry back-off.
 */
function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/**
 * Normalises a thrown value into a `FetchErrorState` object.
 */
function toErrorState(err: unknown): FetchErrorState {
  if (err instanceof Response) {
    return { message: err.statusText || 'HTTP error', code: null, status: err.status }
  }
  if (err instanceof Error) {
    return { message: err.message, code: null, status: null }
  }
  return { message: String(err), code: null, status: null }
}

/**
 * Alpine data factory for `{fetch}` blocks.
 *
 * Returns an Alpine-compatible data object with reactive state properties and
 * lifecycle methods (`init`, `destroy`) that Alpine calls automatically.
 *
 * @param config - Function reference, optional args, and fetch options.
 */
export function slurpFetch(config: FetchDataOptions) {
  return {
    status: 'loading' as 'idle' | 'loading' | 'success' | 'error',
    data: null as unknown,
    error: null as FetchErrorState | null,
    page: 1,
    totalPages: 1,
    items: [] as unknown[],
    hasMore: true,
    _pollTimer: null as ReturnType<typeof setInterval> | null,
    _abortController: null as AbortController | null,
    _navigateListener: null as (() => void) | null,

    /** Called automatically by Alpine after the component mounts. */
    async init(): Promise<void> {
      await this._load()

      if (config.options?.pollSeconds) {
        this._startPolling(config.options.pollSeconds)
      }
      if (config.options?.abortOnNavigate) {
        this._setupAbortOnNavigate()
      }
    },

    /** Called automatically by Alpine when the component is destroyed. */
    destroy(): void {
      if (this._pollTimer !== null) {
        clearInterval(this._pollTimer)
        this._pollTimer = null
      }
      if (this._abortController !== null) {
        this._abortController.abort()
        this._abortController = null
      }
      if (this._navigateListener !== null) {
        document.removeEventListener('slurp:navigate', this._navigateListener)
        this._navigateListener = null
      }
    },

    /**
     * Executes the fetch function with retry and timeout logic. Updates
     * reactive state based on outcome. Respects the module-level cache.
     *
     * @param pageOverride - Page number to pass in paginate/infinite mode.
     *   Defaults to `this.page`.
     */
    async _load(pageOverride?: number): Promise<void> {
      const opts = config.options ?? {}
      const args = config.args ?? []
      const currentPage = pageOverride ?? this.page

      let effectiveArgs: unknown[] = args
      if (opts.paginate ?? opts.infinite) {
        const limit = (opts.paginate ?? opts.infinite)!.limit
        effectiveArgs = [...args, { page: currentPage, limit }]
      }

      const ttl = opts.cacheTtl ?? 0
      // `null` means the args are not serialisable, so this request simply is
      // not cacheable. Every cache interaction below is guarded on it.
      const cacheKey = ttl > 0 ? buildCacheKey(config.fn, effectiveArgs) : null

      if (cacheKey !== null) {
        const cached = readCache(cacheKey)
        if (cached !== null) {
          this._applySuccess(cached, currentPage, opts)
          return
        }
      }

      this.status = 'loading'
      this.error = null

      const controller = new AbortController()
      this._abortController = controller

      let timeoutHandle: ReturnType<typeof setTimeout> | null = null
      if (opts.timeoutMs !== undefined) {
        timeoutHandle = setTimeout(() => controller.abort(), opts.timeoutMs)
      }

      const maxAttempts = 1 + (opts.retry ?? 0)
      let lastError: FetchErrorState = { message: 'Unknown error', code: null, status: null }

      for (let attempt = 0; attempt < maxAttempts; attempt++) {
        if (attempt > 0) {
          // Exponential back-off: 200ms, 400ms, 800ms...
          await sleep(200 * 2 ** (attempt - 1))
        }

        if (controller.signal.aborted) break

        try {
          // Race the fn call against the abort signal so a timeout fires even
          // if the underlying fn never rejects on its own.
          const abortPromise = new Promise<never>((_resolve, reject) => {
            if (controller.signal.aborted) {
              reject(new DOMException('Aborted', 'AbortError'))
              return
            }
            controller.signal.addEventListener('abort', () => {
              reject(new DOMException('Aborted', 'AbortError'))
            }, { once: true })
          })

          const result = await Promise.race([callFn(config.fn, effectiveArgs), abortPromise])

          if (timeoutHandle !== null) clearTimeout(timeoutHandle)

          if (cacheKey !== null) writeCache(cacheKey, result, ttl)

          this._applySuccess(result, currentPage, opts)
          return
        } catch (err) {
          lastError = toErrorState(err)

          // Do not retry on abort - it was intentional.
          if (controller.signal.aborted) break
        }
      }

      if (timeoutHandle !== null) clearTimeout(timeoutHandle)

      this.status = 'error'
      this.error = lastError
    },

    /**
     * Applies a successful response to the component state, handling normal,
     * paginated, and infinite modes.
     */
    _applySuccess(result: unknown, currentPage: number, opts: FetchOptions): void {
      if (opts.infinite) {
        const res = result as PaginatedResponse
        const newItems = Array.isArray(res.data) ? res.data : Array.isArray(result) ? (result as unknown[]) : [result]
        if (currentPage === 1) {
          this.items = newItems
        } else {
          this.items = [...this.items, ...newItems]
        }
        this.hasMore = typeof res.totalPages === 'number'
          ? currentPage < res.totalPages
          : newItems.length > 0
        this.data = this.items
        this.status = 'success'
      } else if (opts.paginate) {
        const res = result as PaginatedResponse
        this.data = Array.isArray(res.data) ? res.data : result
        this.totalPages = typeof res.totalPages === 'number' ? res.totalPages : 1
        this.status = 'success'
      } else {
        this.data = result
        this.status = 'success'
      }
    },

    /**
     * Navigates to the previous page. No-ops on the first page.
     */
    async prev(): Promise<void> {
      if (this.page <= 1) return
      this.page -= 1
      await this._load(this.page)
    },

    /**
     * Navigates to the next page. No-ops on the last page.
     */
    async next(): Promise<void> {
      if (this.page >= this.totalPages) return
      this.page += 1
      await this._load(this.page)
    },

    /**
     * Fetches the next batch and appends it to `items`. Called by the
     * IntersectionObserver sentinel in `slurp.infinite.js` or manually.
     */
    async loadMore(): Promise<void> {
      if (!this.hasMore || this.status === 'loading') return
      this.page += 1
      await this._load(this.page)
    },

    /**
     * Starts a polling interval that re-fetches every `seconds` seconds.
     * The interval is cleared in `destroy()`.
     *
     * @param seconds - Poll interval in seconds.
     */
    _startPolling(seconds: number): void {
      this._pollTimer = setInterval(() => {
        void this._load()
      }, seconds * 1000)
    },

    /**
     * Registers a `slurp:navigate` listener that aborts any in-flight request
     * when the user navigates away. Cleaned up in `destroy()`.
     */
    _setupAbortOnNavigate(): void {
      const listener = (): void => {
        if (this._abortController !== null) {
          this._abortController.abort()
          this._abortController = null
        }
      }
      this._navigateListener = listener
      document.addEventListener('slurp:navigate', listener)
    },
  }
}

// Exposed globally for compiled templates referencing `slurpFetch(...)` in x-data without an import.
window.slurpFetch = slurpFetch

document.addEventListener('alpine:init', () => {
  if (window.Alpine !== undefined) {
    // slurpFetch's signature differs from AlpineDataFactory - it receives the
    // x-data argument object rather than an HTMLElement.
    window.Alpine.data('slurpFetch', slurpFetch as unknown as Parameters<typeof window.Alpine.data>[1])
  }
})
