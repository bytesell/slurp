/**
 * Client-side navigation for Slurp.
 *
 * Activates when `<body navigate>` is present. Intercepts same-origin `<a>`
 * clicks, fetches the next page, swaps only the page-specific content
 * (everything inside `<main data-slurp-page>`), and updates the browser
 * history via the History API.
 *
 * Supports the View Transitions API when `<body navigate transition>` is set.
 * Re-initialises Alpine on new content via `Alpine.initTree`. Dispatches a
 * `slurp:navigate` CustomEvent on `document` after each successful swap so
 * other modules (e.g. infinite scroll) can clean up.
 */

import Alpine from 'alpinejs'

/** Selector for the page-specific content container injected by the compiler. */
const PAGE_SLOT_SELECTOR = '[data-slurp-page]'

/** Whether the current `<body>` has declared transition support. */
let hasTransition = false

/** Guards against calling initNavigation() more than once per page load. */
let initialized = false

/**
 * Tracks the last URL that was navigated to by client navigation, so popstate
 * can detect genuine back/forward vs hash-only transitions.
 */
let lastNavigatedHref = ''

/** Named listener references so we can remove them in teardown. */
const boundClickHandler = (e: MouseEvent): void => { void handleClick(e) }
const boundPopStateHandler = (e: PopStateEvent): void => { void handlePopState(e) }

/**
 * Tears down navigation listeners and resets state. Used only in tests.
 * @internal
 */
export function _resetNavigationForTesting(): void {
  document.removeEventListener('click', boundClickHandler)
  window.removeEventListener('popstate', boundPopStateHandler)
  initialized = false
  hasTransition = false
  lastNavigatedHref = ''
}

/**
 * Initialises client navigation. Must be called once during boot.
 * Does nothing if `<body>` lacks the `navigate` attribute.
 * Subsequent calls after initialization are no-ops.
 */
export function initNavigation(): void {
  if (initialized) return

  const body = document.body
  if (!body.hasAttribute('navigate')) return

  initialized = true
  hasTransition = body.hasAttribute('transition')
  // Initialise lastNavigatedHref to the current page URL so hash-only
  // popstate events fired on the initial page are correctly suppressed.
  lastNavigatedHref = window.location.href

  document.addEventListener('click', boundClickHandler)
  window.addEventListener('popstate', boundPopStateHandler)
}

/**
 * Determines whether a clicked anchor should trigger client navigation.
 * Returns the resolved href string, or `null` if navigation should be skipped.
 */
function resolveNavigationHref(e: MouseEvent): string | null {
  // Another handler (e.g. an Alpine @click.prevent) already claimed the click.
  if (e.defaultPrevented) return null
  // Ignore modified clicks - let the browser handle them normally.
  if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return null
  if (e.button !== 0) return null

  const anchor = (e.target as Element).closest<HTMLAnchorElement>('a[href]')
  if (!anchor) return null

  if (anchor.getAttribute('navigate') === 'false') return null

  // New-tab/frame targets and downloads must keep native browser behavior.
  if (anchor.target !== '' && anchor.target !== '_self') return null
  if (anchor.hasAttribute('download')) return null

  const href = anchor.href

  if (anchor.origin !== window.location.origin) return null
  if (anchor.pathname === window.location.pathname && anchor.hash !== '') return null

  // Skip non-HTTP(S) protocols (mailto:, javascript:, etc.)
  if (!href.startsWith('http://') && !href.startsWith('https://')) return null

  return href
}

/**
 * Handles document-level click events. Routes qualifying anchor clicks to the
 * client navigation path.
 */
async function handleClick(e: MouseEvent): Promise<void> {
  const href = resolveNavigationHref(e)
  if (!href) return

  e.preventDefault()
  await navigate(href, true)
}

/**
 * Handles the browser `popstate` event (back / forward buttons).
 *
 * Skips navigation when the popstate was triggered by a hash-only change on
 * the same page (the browser updates `location.hash` and fires popstate, but
 * no new document should be fetched).
 */
async function handlePopState(_e: PopStateEvent): Promise<void> {
  const href = window.location.href

  // Skip hash-only popstate: same origin + pathname + query, different hash.
  // The query string MUST participate: back/forward between ?page=1 and
  // ?page=2 is a real navigation, not a hash change.
  try {
    const current = new URL(href)
    const last = lastNavigatedHref ? new URL(lastNavigatedHref) : null
    if (
      last &&
      current.origin === last.origin &&
      current.pathname === last.pathname &&
      current.search === last.search
    ) {
      // Only the hash changed - no fetch needed.
      lastNavigatedHref = href
      return
    }
  } catch {
    // If URL parsing fails for any reason, proceed with navigation.
  }

  await navigate(href, false)
}

/**
 * Fetches the target page, extracts its page slot, swaps the DOM, and updates
 * browser history.
 *
 * Layout HTML (header, footer, sidebar) is intentionally NOT re-downloaded or
 * re-rendered - only the `[data-slurp-page]` subtree is replaced.
 *
 * @param href - Fully resolved URL of the target page.
 * @param push - When `true`, a new history entry is pushed; `false` is used for
 *   popstate (back/forward) where the URL is already correct.
 */
async function navigate(href: string, push: boolean): Promise<void> {
  // Record the target so hash-only popstates can be filtered out.
  lastNavigatedHref = href

  let html: string
  try {
    const res = await fetch(href, {
      headers: { 'X-Slurp-Navigate': '1' },
    })
    if (!res.ok) {
      console.error(`[slurp] navigation fetch failed: ${res.status} ${res.statusText}`)
      return
    }
    html = await res.text()
  } catch (err) {
    console.error('[slurp] navigation fetch error:', err)
    return
  }

  const parser = new DOMParser()
  const incoming = parser.parseFromString(html, 'text/html')

  const newSlot = incoming.querySelector(PAGE_SLOT_SELECTOR)
  const currentSlot = document.querySelector(PAGE_SLOT_SELECTOR)

  if (!newSlot || !currentSlot) {
    // Fallback: hard navigate if page structure doesn't match.
    window.location.assign(href)
    return
  }

  const doSwap = (): void => {
    const newTitle = incoming.querySelector('title')
    if (newTitle) document.title = newTitle.textContent ?? ''

    // Update <meta name="slurp-route"> for parseParams() after navigation.
    const newRouteMeta = incoming.querySelector<HTMLMetaElement>('meta[name="slurp-route"]')
    const currentRouteMeta = document.querySelector<HTMLMetaElement>('meta[name="slurp-route"]')
    if (newRouteMeta && currentRouteMeta) {
      currentRouteMeta.content = newRouteMeta.content
    }

    currentSlot.replaceWith(newSlot)
    Alpine.initTree(newSlot as HTMLElement)

    if (push) {
      window.history.pushState(null, '', href)
    }

    document.dispatchEvent(new CustomEvent('slurp:navigate', { detail: { href } }))
  }

  if (hasTransition && 'startViewTransition' in document) {
    // View Transitions API - browsers that don't support it fall through to
    // the synchronous swap above.
    await (
      document as Document & {
        startViewTransition: (cb: () => void) => { finished: Promise<void> }
      }
    ).startViewTransition(doSwap).finished
  } else {
    doSwap()
  }
}
