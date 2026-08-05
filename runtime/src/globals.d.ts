/**
 * Global window augmentations shared across all Slurp runtime chunks.
 *
 * Declared in a standalone `.d.ts` so all entry points (`core`, `fetch`,
 * `infinite`) can reference `window.SLURP_MODE`, `window.Alpine`, etc.
 * without duplicating the declarations.
 */

import type Alpine from 'alpinejs'
import type { cn } from './core/cn'
import type { parseParams } from './core/params'
import type { slurpFetch } from './fetch/index'

declare global {
  interface Window {
    /**
     * Set by the Slurp server before any runtime script loads. Controls which
     * runtime behaviours are active.
     *
     * - `'editor'` - fetch interceptor may run; no HMR
     * - `'dev'`    - fetch interceptor may run; HMR WebSocket active
     * - `'production'`, ABSENT, or anything unrecognised - neither runs
     *
     * Absent is the normal state on a real page: only the dev server sets this.
     * The two active modes are therefore named explicitly wherever they are
     * tested, so an unset value fails closed rather than opting in.
     */
    SLURP_MODE?: 'production' | 'editor' | 'dev'

    /**
     * Optional fetch interceptor set by the editor or dev environment. When it is
     * consulted, `slurpFetch` calls this instead of the real fetch function,
     * allowing mock data injection.
     *
     * It is consulted only when `SLURP_MODE` is exactly `'dev'` or `'editor'` -
     * an ALLOW-LIST, not `!== 'production'`. Nothing sets `SLURP_MODE` on a
     * production build, so under the old deny-list `mode` was `undefined` on a
     * real page, that test passed, and the only remaining condition was that
     * SOMETHING had assigned this property - which any third-party script on the
     * page can do, silently replacing every `{fetch}` response. Naming the two
     * modes makes an absent or unrecognised mode fail CLOSED. Do not restore the
     * deny-list form.
     */
    SLURP_FETCH_INTERCEPTOR?: (
      fn: (...args: unknown[]) => Promise<unknown>,
      args: unknown[],
    ) => Promise<unknown>

    /**
     * The Alpine.js instance, exposed so compiled templates can access it
     * without a module import.
     */
    Alpine: typeof Alpine

    /**
     * `cn()` exposed globally for use in compiled `class={}` output and
     * inline Alpine expressions.
     */
    cn: typeof cn

    /**
     * Named URL params extracted from the current route pattern at boot time.
     * E.g. for route `/products/:slug` at `/products/foo`: `{ slug: 'foo' }`
     */
    slurpParams: Record<string, string>

    /**
     * The `slurpFetch` data factory, exposed globally so compiled templates
     * can reference it in `x-data="slurpFetch(...)"` without an import.
     */
    slurpFetch: typeof slurpFetch
  }
}
