/**
 * @slurp/runtime - core entry point (dist/slurp.core.js)
 *
 * Always loaded on every Slurp page. Responsibilities:
 * 1. Bundle and expose Alpine.js
 * 2. Expose `cn()` globally for inline Alpine expressions
 * 3. Parse URL params from the route meta tag
 * 4. Set up client navigation (when `<body navigate>` is present)
 * 5. Start HMR WebSocket in `dev` mode
 * 6. Boot Alpine
 *
 * The fetch interceptor (`window.SLURP_FETCH_INTERCEPTOR`) is set externally
 * by the editor or dev-server environment before this script loads. It is only
 * consulted by `slurp.fetch.js`, and only when `SLURP_MODE` is exactly `'dev'`
 * or `'editor'` - an allow-list. The earlier `!== 'production'` form failed
 * OPEN, because nothing sets `SLURP_MODE` on a production page, so any script
 * that could assign the interceptor could swap every `{fetch}` response. See
 * `callFn` in `fetch/index.ts`.
 */

import Alpine from 'alpinejs'
import { parseParams } from './params'
import { initNavigation } from './navigation'
import { cn } from './cn'
import { initDevMode } from './dev'

function init(): void {
  window.Alpine = Alpine
  window.cn = cn

  // Each optional boot step is isolated so a failure in one cannot prevent
  // Alpine from starting. Anything that throws here unguarded (for example
  // `parseParams` on a malformed percent-escape such as `/products/%`) kills
  // ALL page interactivity, because `Alpine.start()` never runs.
  try {
    window.slurpParams = parseParams()
  } catch (err) {
    console.error('[slurp] failed to parse route params:', err)
    window.slurpParams = {}
  }

  if (window.SLURP_MODE === 'dev') {
    try {
      initDevMode()
    } catch (err) {
      console.error('[slurp] failed to start HMR client:', err)
    }
  }

  try {
    initNavigation()
  } catch (err) {
    console.error('[slurp] failed to initialise client navigation:', err)
  }

  Alpine.start()
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', init)
} else {
  init()
}

export { cn, parseParams }
