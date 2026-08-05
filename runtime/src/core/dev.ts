/**
 * HMR (Hot Module Replacement) WebSocket client.
 *
 * Only active when `window.SLURP_MODE === 'dev'`. Connects to the Slurp dev
 * server's HMR endpoint, handles reload/error/clear messages, and auto-retries
 * the connection when the socket closes (e.g. during a dev-server restart).
 *
 * The HMR port is read from `<meta name="slurp-hmr-port" content="3001" />`.
 * Falls back to `3001` when the tag is absent.
 */

import type { HmrMessage, SlurpError } from '../types'

/** ID of the error overlay element when present. */
const OVERLAY_ID = 'slurp-error-overlay'

/** Base delay between reconnection attempts, in milliseconds. */
const RECONNECT_BASE_MS = 1000

/** Ceiling for the exponential reconnect back-off, in milliseconds. */
const RECONNECT_MAX_MS = 30000

/** Consecutive failed connection attempts, reset on a successful open. */
let reconnectAttempts = 0

/**
 * Resets the reconnect back-off counter. Test-only.
 * @internal
 */
export function _resetHmrReconnectForTesting(): void {
  reconnectAttempts = 0
}

/**
 * Schedules a reconnect with exponential back-off capped at
 * `RECONNECT_MAX_MS`. A fixed 1s retry would hammer the dev server once a
 * second forever whenever it is down, from every open tab.
 */
function scheduleReconnect(): void {
  const delay = Math.min(RECONNECT_BASE_MS * 2 ** reconnectAttempts, RECONNECT_MAX_MS)
  reconnectAttempts++
  setTimeout(initDevMode, delay)
}

/**
 * Starts the HMR WebSocket connection and registers message handlers.
 * Called once during dev-mode boot. Re-invoked by `scheduleReconnect` on socket
 * close to provide automatic reconnection.
 */
export function initDevMode(): void {
  const port = getHmrPort()
  let ws: WebSocket

  try {
    ws = new WebSocket(`ws://localhost:${port}`)
  } catch (err) {
    console.warn('[slurp:hmr] failed to open WebSocket, retrying shortly', err)
    scheduleReconnect()
    return
  }

  ws.addEventListener('open', () => {
    reconnectAttempts = 0
  })

  ws.addEventListener('message', (event: MessageEvent<string>) => {
    let raw: unknown
    try {
      raw = JSON.parse(event.data)
    } catch {
      console.warn('[slurp:hmr] received non-JSON message:', event.data)
      return
    }

    // PARSE, do not cast. This message crosses a language boundary, so the
    // compiler cannot check it: a blind `as HmrMessage` lets a mismatch such as
    // a differently-cased `severity` value stay invisible indefinitely.
    // Validating here means a producer-side change shows up as a console
    // warning instead of a silently mis-rendered overlay.
    const msg = parseHmrMessage(raw)
    if (!msg) {
      console.warn('[slurp:hmr] ignoring malformed message:', raw)
      return
    }

    switch (msg.type) {
      case 'reload':
        window.location.reload()
        break

      case 'error':
        showErrorOverlay(msg.errors)
        break

      case 'clear':
        removeErrorOverlay()
        break

      default: {
        // Exhaustiveness guard - msg.type is `never` here in strict mode.
        const _exhaustive: never = msg
        console.warn('[slurp:hmr] unknown message type:', _exhaustive)
      }
    }
  })

  ws.addEventListener('close', () => {
    scheduleReconnect()
  })

  ws.addEventListener('error', () => {
    // The 'close' event always fires after 'error', so reconnection is handled
    // there. We only log here to avoid a duplicate setTimeout.
    console.warn('[slurp:hmr] WebSocket error')
  })
}

/** The severity values the wire format is allowed to carry. */
const SEVERITIES: readonly SlurpError['severity'][] = ['error', 'warning', 'info']

/**
 * Validates one error record from the HMR wire format.
 *
 * An unrecognised `severity` is normalised to `'error'` rather than dropped:
 * the compiler said something was wrong, so the loudest treatment is the safe
 * default, and a swallowed diagnostic is worse than a mis-coloured one.
 */
function parseSlurpError(value: unknown): SlurpError | null {
  if (typeof value !== 'object' || value === null) return null
  const v = value as Record<string, unknown>

  const severity = SEVERITIES.includes(v['severity'] as SlurpError['severity'])
    ? (v['severity'] as SlurpError['severity'])
    : 'error'

  return {
    file: typeof v['file'] === 'string' ? v['file'] : '<unknown>',
    line: typeof v['line'] === 'number' ? v['line'] : 0,
    column: typeof v['column'] === 'number' ? v['column'] : 0,
    message: typeof v['message'] === 'string' ? v['message'] : String(v['message'] ?? ''),
    severity,
    code: (typeof v['code'] === 'string' ? v['code'] : 'UnexpectedToken') as SlurpError['code'],
  }
}

/**
 * Validates a decoded HMR message, returning null when it does not match the
 * declared wire format.
 */
export function parseHmrMessage(value: unknown): HmrMessage | null {
  if (typeof value !== 'object' || value === null) return null
  const v = value as Record<string, unknown>

  switch (v['type']) {
    case 'reload':
      return { type: 'reload', pages: Array.isArray(v['pages']) ? (v['pages'] as string[]) : [] }

    case 'error': {
      // `showErrorOverlay` does a `for...of` over this list, so handing it
      // through unchecked lets a non-array throw inside the WebSocket message
      // handler.
      const list = Array.isArray(v['errors']) ? v['errors'] : []
      const errors = list
        .map(parseSlurpError)
        .filter((e): e is SlurpError => e !== null)
      return { type: 'error', errors }
    }

    case 'clear':
      return { type: 'clear' }

    default:
      return null
  }
}

/**
 * Reads the HMR port from `<meta name="slurp-hmr-port">`.
 * Returns `3001` when the tag is absent or unparseable.
 */
function getHmrPort(): number {
  const meta = document.querySelector<HTMLMetaElement>('meta[name="slurp-hmr-port"]')
  if (!meta) return 3001
  const parsed = parseInt(meta.content, 10)
  return Number.isFinite(parsed) ? parsed : 3001
}

/**
 * Creates and mounts a full-screen error overlay displaying each `SlurpError`
 * as a styled card. Replaces any existing overlay.
 *
 * `innerHTML` is NOT used anywhere in this function: every node is built with
 * `document.createElement` and every string is assigned through `textContent`,
 * so an error message cannot inject markup. Do not "simplify" the
 * createElement calls into a template string - a compiler error message can
 * carry arbitrary source text, so that change turns this function into a real
 * XSS sink.
 *
 * @param errors - Array of errors received from the HMR server.
 */
function showErrorOverlay(errors: SlurpError[]): void {
  removeErrorOverlay()

  // Defensive: callers inside this module go through parseHmrMessage, which
  // guarantees an array, but this must never throw inside a socket handler.
  if (!Array.isArray(errors)) return

  const overlay = document.createElement('div')
  overlay.id = OVERLAY_ID
  overlay.style.cssText = [
    'position:fixed',
    'inset:0',
    'z-index:99999',
    'background:rgba(0,0,0,0.85)',
    'color:#fff',
    'font-family:monospace',
    'padding:2rem',
    'overflow:auto',
    'display:flex',
    'flex-direction:column',
    'gap:1rem',
  ].join(';')

  const dismiss = document.createElement('button')
  dismiss.textContent = '✕ Dismiss'
  dismiss.style.cssText = [
    'align-self:flex-end',
    'background:transparent',
    'border:1px solid #555',
    'color:#fff',
    'padding:0.25rem 0.75rem',
    'border-radius:4px',
    'cursor:pointer',
    'font-family:monospace',
    'font-size:0.85rem',
  ].join(';')
  dismiss.addEventListener('click', removeErrorOverlay)
  overlay.appendChild(dismiss)

  for (const err of errors) {
    const card = document.createElement('div')
    card.style.cssText = [
      'background:#1a1a2e',
      'border:1px solid #e63946',
      'border-radius:6px',
      'padding:1.5rem',
      'display:flex',
      'flex-direction:column',
      'gap:0.4rem',
    ].join(';')

    const heading = document.createElement('div')
    heading.style.cssText = 'color:#e63946;font-weight:bold;font-size:1rem'
    heading.textContent = `${err.code}: ${err.message}`

    const location = document.createElement('div')
    location.style.cssText = 'color:#aaa;font-size:0.85rem'
    location.textContent = `${err.file}:${err.line}:${err.column}`

    const severity = document.createElement('div')
    severity.style.cssText = `color:${err.severity === 'error' ? '#e63946' : err.severity === 'warning' ? '#f4a261' : '#90e0ef'};font-size:0.75rem;text-transform:uppercase;letter-spacing:0.05em`
    severity.textContent = err.severity

    card.appendChild(heading)
    card.appendChild(location)
    card.appendChild(severity)
    overlay.appendChild(card)
  }

  document.body.appendChild(overlay)
}

/**
 * Removes the error overlay from the DOM, if present.
 */
function removeErrorOverlay(): void {
  document.getElementById(OVERLAY_ID)?.remove()
}
