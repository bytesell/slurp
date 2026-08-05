/**
 * @bytesell/slurp-runtime - infinite entry point (dist/slurp.infinite.js)
 *
 * Manages IntersectionObserver-based infinite scroll. The compiler emits a
 * sentinel element at the bottom of every list that uses `infinite` or
 * `paginate`. This module watches that sentinel and calls `loadMore()` on the
 * parent Alpine component when the sentinel enters the viewport.
 *
 * Lifecycle:
 * - Observe sentinels on page load and after each `slurp:navigate` event.
 * - Disconnect the observer for a sentinel when `hasMore` is `false`.
 * - Clean up all observers on `slurp:navigate` (before re-observing new ones).
 *
 * Sentinel markup emitted by compiler:
 * ```html
 * <div data-slurp-sentinel x-ref="sentinel"></div>
 * ```
 * The sentinel must be a descendant of the element with
 * `x-data="slurpFetch(...)"`.
 */

/** Attribute name placed on sentinel elements by the compiler. */
const SENTINEL_ATTR = 'data-slurp-sentinel'

/**
 * Active observers keyed by their sentinel element.
 *
 * A `WeakMap` holds the sentinel weakly, so a sentinel removed from the DOM by
 * anything OTHER than a `slurp:navigate` swap (an Alpine `x-if` teardown, a
 * manual `remove()`, a third-party script) does not keep the element and its
 * observer alive for the lifetime of the page. The parallel `WeakRef` list
 * exists only because a WeakMap is not enumerable and `disconnectAll` needs to
 * iterate; its entries are pruned as they are found empty.
 */
const observers = new WeakMap<Element, IntersectionObserver>()

/** Weak handles on the sentinels currently observed, for iteration only. */
let observed: WeakRef<Element>[] = []

/** True when `sentinel` currently has an observer. */
function isObserved(sentinel: Element): boolean {
  return observers.has(sentinel)
}

/**
 * Initialises infinite scroll sentinel observation. Must be called once -
 * typically by the compiler-emitted bootstrap code in `slurp.core.js` when
 * the page uses infinite or paginate blocks.
 *
 * Automatically re-observes sentinels after client navigation.
 */
export function initInfiniteScroll(): void {
  observeAll()

  document.addEventListener('slurp:navigate', () => {
    disconnectAll()
    observeAll()
  })
}

/**
 * Finds all sentinel elements in the current document and creates an
 * IntersectionObserver for each one that is not already being observed.
 */
function observeAll(): void {
  const sentinels = Array.from(document.querySelectorAll<HTMLElement>(`[${SENTINEL_ATTR}]`))
  for (const sentinel of sentinels) {
    if (isObserved(sentinel)) continue
    observe(sentinel)
  }
}

/**
 * Disconnects and removes all active observers. Called before re-observing
 * after a client navigation swap.
 */
function disconnectAll(): void {
  for (const ref of observed) {
    const sentinel = ref.deref()
    if (!sentinel) continue
    observers.get(sentinel)?.disconnect()
    observers.delete(sentinel)
  }
  observed = []
}

/**
 * Creates an IntersectionObserver for a single sentinel element.
 *
 * When the sentinel enters the viewport, the observer:
 * 1. Calls `loadMore()` on the nearest Alpine component that exposes it.
 * 2. Checks `hasMore` after the call; if `false`, disconnects the observer.
 *
 * @param sentinel - The sentinel DOM element to observe.
 */
function observe(sentinel: HTMLElement): void {
  const observer = new IntersectionObserver(
    (entries: IntersectionObserverEntry[]) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue

        const component = getAlpineComponent(sentinel)
        if (!component) continue

        if (component.hasMore === false) {
          disconnectSentinel(sentinel)
          continue
        }

        if (typeof component.loadMore === 'function') {
          void (component.loadMore as () => Promise<void>)().then(() => {
            if (component.hasMore === false) {
              disconnectSentinel(sentinel)
            }
          })
        }
      }
    },
    {
      rootMargin: '200px',
      threshold: 0,
    },
  )

  observer.observe(sentinel)
  observers.set(sentinel, observer)
  // Drop handles whose element has already been collected, so the list cannot
  // grow without bound across many navigations.
  observed = observed.filter(ref => ref.deref() !== undefined)
  observed.push(new WeakRef(sentinel))
}

/**
 * Disconnects the observer for a specific sentinel and removes it from the
 * tracking map.
 *
 * @param sentinel - The sentinel element whose observer should be disconnected.
 */
function disconnectSentinel(sentinel: Element): void {
  const observer = observers.get(sentinel)
  if (observer) {
    observer.disconnect()
    observers.delete(sentinel)
    observed = observed.filter(ref => {
      const el = ref.deref()
      return el !== undefined && el !== sentinel
    })
  }
}

/**
 * Walks up the DOM from `element` to find the nearest ancestor that owns an
 * Alpine v3 component (`_x_dataStack` set on the `x-data` root). Returns the
 * component's own data scope (`_x_dataStack[0]`), or `null` if none is found.
 * Alpine v2's `__x` property does not exist in v3, so this must read the v3
 * internal directly; the sentinel is always inside the `slurpFetch` x-data
 * root, whose scope carries `hasMore`/`loadMore`.
 *
 * @param element - Starting element for the ancestor walk.
 */
function getAlpineComponent(element: HTMLElement): AlpineComponentData | null {
  let node: HTMLElement | null = element

  while (node !== null) {
    const alpineNode = node as HTMLElement & { _x_dataStack?: AlpineComponentData[] }
    const stack = alpineNode._x_dataStack
    if (stack !== undefined && stack.length > 0) {
      return stack[0] ?? null
    }
    node = node.parentElement
  }

  return null
}

/**
 * Minimal shape of the Alpine component data object we interact with.
 * The full object is provided by `slurpFetch` in `slurp.fetch.js`.
 */
interface AlpineComponentData {
  hasMore: boolean
  loadMore: (() => Promise<void>) | unknown
}

// Alpine has already mounted by this point - slurp.core.js loads first and calls Alpine.start().
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', initInfiniteScroll)
} else {
  initInfiniteScroll()
}

export { disconnectAll as _disconnectAll, observeAll as _observeAll }
