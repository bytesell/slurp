import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

// ─── Error overlay helpers ─────────────────────────────────────────────────────
// We test the overlay via the exported functions indirectly through initDevMode.
// For unit clarity, we import the module and drive it via fake WebSocket events.

// We need to control the WebSocket constructor, so we mock it before importing.
const mockWsInstance = {
  addEventListener: vi.fn(),
  close: vi.fn(),
}

// Must be a `function`, not an arrow: `initDevMode` calls `new WebSocket(...)`,
// and as of vitest 4 a `vi.fn()` wrapping an arrow is not constructible
// ("() => mockWsInstance is not a constructor"). A constructor returning an
// object yields that object, so `new MockWebSocket()` is `mockWsInstance`.
const MockWebSocket = vi.fn(function MockWebSocket() {
  return mockWsInstance
})

vi.stubGlobal('WebSocket', MockWebSocket)

// Import after stubbing globals.
const { initDevMode, parseHmrMessage, _resetHmrReconnectForTesting } = await import('./dev')

// ─── Helpers ──────────────────────────────────────────────────────────────────

type WsListener = (event: MessageEvent<string>) => void
type WsCloseListener = () => void

/** Returns the registered 'message' event listener, if any. */
function getMessageListener(): WsListener | undefined {
  const calls = (mockWsInstance.addEventListener as ReturnType<typeof vi.fn>).mock.calls as unknown[][]
  const found = calls.find((args) => args[0] === 'message')
  return found?.[1] as WsListener | undefined
}

/** Returns the registered 'close' event listener, if any. */
function getCloseListener(): WsCloseListener | undefined {
  const calls = (mockWsInstance.addEventListener as ReturnType<typeof vi.fn>).mock.calls as unknown[][]
  const found = calls.find((args) => args[0] === 'close')
  return found?.[1] as WsCloseListener | undefined
}

/** Returns the registered 'open' event listener, if any. */
function getOpenListener(): (() => void) | undefined {
  const calls = (mockWsInstance.addEventListener as ReturnType<typeof vi.fn>).mock.calls as unknown[][]
  const found = calls.find((args) => args[0] === 'open')
  return found?.[1] as (() => void) | undefined
}

/** Creates a fake MessageEvent with a JSON payload. */
function makeMessageEvent(data: unknown): MessageEvent<string> {
  return new MessageEvent('message', { data: JSON.stringify(data) })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('initDevMode', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    document.body.innerHTML = ''
    _resetHmrReconnectForTesting()
  })

  afterEach(() => {
    document.getElementById('slurp-error-overlay')?.remove()
  })

  it('opens a WebSocket to the HMR port', () => {
    initDevMode()
    expect(MockWebSocket).toHaveBeenCalledWith('ws://localhost:3001')
  })

  it('reads HMR port from meta tag', () => {
    vi.clearAllMocks()
    const meta = document.createElement('meta')
    meta.name = 'slurp-hmr-port'
    meta.content = '4242'
    document.head.appendChild(meta)

    initDevMode()
    expect(MockWebSocket).toHaveBeenCalledWith('ws://localhost:4242')
    meta.remove()
  })

  it('calls window.location.reload on reload message', () => {
    initDevMode()
    // jsdom marks window.location.reload as non-configurable, so vi.spyOn
    // cannot patch it. We replace it via Object.defineProperty instead.
    const reloadSpy = vi.fn()
    Object.defineProperty(window, 'location', {
      value: { ...window.location, reload: reloadSpy },
      writable: true,
      configurable: true,
    })
    const listener = getMessageListener()
    listener?.(makeMessageEvent({ type: 'reload', pages: ['/'] }))
    expect(reloadSpy).toHaveBeenCalledOnce()
  })

  it('creates error overlay on error message', () => {
    initDevMode()
    const listener = getMessageListener()
    listener?.(
      makeMessageEvent({
        type: 'error',
        errors: [
          {
            file: 'src/foo.slurp',
            line: 10,
            column: 5,
            message: 'Unexpected token',
            severity: 'error',
            code: 'UnexpectedToken',
          },
        ],
      }),
    )

    const overlay = document.getElementById('slurp-error-overlay')
    expect(overlay).not.toBeNull()
    expect(overlay?.textContent).toContain('UnexpectedToken')
    expect(overlay?.textContent).toContain('Unexpected token')
    expect(overlay?.textContent).toContain('src/foo.slurp')
  })

  it('removes error overlay on clear message', () => {
    // First create an overlay.
    initDevMode()
    const listener = getMessageListener()
    listener?.(
      makeMessageEvent({
        type: 'error',
        errors: [
          {
            file: 'a.slurp',
            line: 1,
            column: 1,
            message: 'err',
            severity: 'error',
            code: 'UnexpectedToken',
          },
        ],
      }),
    )
    expect(document.getElementById('slurp-error-overlay')).not.toBeNull()

    listener?.(makeMessageEvent({ type: 'clear' }))
    expect(document.getElementById('slurp-error-overlay')).toBeNull()
  })

  it('replaces existing overlay on successive error messages', () => {
    initDevMode()
    const listener = getMessageListener()

    const errMsg = (msg: string) =>
      makeMessageEvent({
        type: 'error',
        errors: [{ file: 'f.slurp', line: 1, column: 1, message: msg, severity: 'error', code: 'UnexpectedToken' }],
      })

    listener?.(errMsg('First error'))
    listener?.(errMsg('Second error'))

    const overlays = document.querySelectorAll('#slurp-error-overlay')
    expect(overlays).toHaveLength(1)
    expect(overlays[0]?.textContent).toContain('Second error')
    expect(overlays[0]?.textContent).not.toContain('First error')
  })

  it('does not insert raw HTML from error fields into the overlay', () => {
    initDevMode()
    const listener = getMessageListener()
    listener?.(
      makeMessageEvent({
        type: 'error',
        errors: [
          {
            file: '<img src=x onerror=alert(1)>',
            line: 1,
            column: 1,
            message: '<script>alert(1)</script>',
            severity: 'error',
            code: 'UnexpectedToken',
          },
        ],
      }),
    )

    const overlay = document.getElementById('slurp-error-overlay')
    // The raw HTML should appear as text, not be parsed as markup.
    expect(overlay?.querySelector('img')).toBeNull()
    expect(overlay?.querySelector('script')).toBeNull()
  })

  it('schedules reconnect on socket close', () => {
    vi.useFakeTimers()
    initDevMode()
    const closeSpy = vi.spyOn(globalThis, 'setTimeout')
    const closeListener = getCloseListener()
    closeListener?.()
    expect(closeSpy).toHaveBeenCalledWith(initDevMode, 1000)
    vi.useRealTimers()
  })

  // ── reconnect back-off ─────────────────────────────────────────────────────

  it('backs off exponentially and caps the reconnect delay', () => {
    vi.useFakeTimers()
    const timerSpy = vi.spyOn(globalThis, 'setTimeout')

    // Drive 12 consecutive closes. Uncapped, 1000 * 2^11 would be 2048000 ms.
    for (let i = 0; i < 12; i++) {
      initDevMode()
      getCloseListener()?.()
    }

    const delays = timerSpy.mock.calls.map((c) => c[1])
    expect(delays[0]).toBe(1000)
    expect(delays[1]).toBe(2000)
    expect(delays[2]).toBe(4000)
    // Never exceeds the ceiling, and never returns to hammering once a second.
    expect(Math.max(...(delays as number[]))).toBe(30000)
    vi.useRealTimers()
  })

  it('resets the back-off once the socket opens', () => {
    vi.useFakeTimers()
    const timerSpy = vi.spyOn(globalThis, 'setTimeout')

    initDevMode()
    getCloseListener()?.()
    initDevMode()
    getCloseListener()?.()
    // A successful connection clears the accumulated penalty.
    getOpenListener()?.()
    initDevMode()
    getCloseListener()?.()

    const delays = timerSpy.mock.calls.map((c) => c[1])
    expect(delays[delays.length - 1]).toBe(1000)
    vi.useRealTimers()
  })
})

// ─── the HMR wire format is PARSED, not cast ─────────────────────────────────

describe('parseHmrMessage', () => {
  it('accepts the lowercase severity the wire format declares', () => {
    const msg = parseHmrMessage({
      type: 'error',
      errors: [
        { file: 'a.slurp', line: 1, column: 2, message: 'm', severity: 'warning', code: 'UnclosedBlock' },
      ],
    })
    expect(msg?.type).toBe('error')
    expect(msg?.type === 'error' && msg.errors[0]?.severity).toBe('warning')
  })

  it('normalises an unrecognised severity to error rather than dropping it', () => {
    // PascalCase is what a Rust Severity enum deriving Serialize with no
    // rename_all emits. Both comparisons in showErrorOverlay are then false and
    // every card renders as info, so the value must be normalised here.
    const msg = parseHmrMessage({
      type: 'error',
      errors: [
        { file: 'a.slurp', line: 1, column: 2, message: 'm', severity: 'Error', code: 'UnclosedBlock' },
      ],
    })
    expect(msg?.type === 'error' && msg.errors).toHaveLength(1)
    expect(msg?.type === 'error' && msg.errors[0]?.severity).toBe('error')
  })

  it('returns an empty error list when errors is not an array', () => {
    // showErrorOverlay does a for...of, so an unchecked non-array throws inside
    // the WebSocket message handler.
    const msg = parseHmrMessage({ type: 'error', errors: 'boom' })
    expect(msg?.type === 'error' && msg.errors).toEqual([])
  })

  it('rejects an unknown message type', () => {
    expect(parseHmrMessage({ type: 'nonsense' })).toBeNull()
  })

  it('rejects a non-object payload', () => {
    expect(parseHmrMessage(null)).toBeNull()
    expect(parseHmrMessage('reload')).toBeNull()
    expect(parseHmrMessage(42)).toBeNull()
  })

  it('fills in missing fields instead of throwing', () => {
    const msg = parseHmrMessage({ type: 'error', errors: [{}] })
    expect(msg?.type === 'error' && msg.errors[0]?.file).toBe('<unknown>')
    expect(msg?.type === 'error' && msg.errors[0]?.severity).toBe('error')
  })

  it('accepts reload with no pages array', () => {
    expect(parseHmrMessage({ type: 'reload' })).toEqual({ type: 'reload', pages: [] })
  })
})

describe('error overlay severity colours', () => {
  it('paints a lowercase error severity in the error colour, not info', () => {
    initDevMode()
    getMessageListener()?.(
      makeMessageEvent({
        type: 'error',
        errors: [
          { file: 'a.slurp', line: 1, column: 1, message: 'm', severity: 'error', code: 'UnclosedBlock' },
        ],
      }),
    )
    const overlay = document.getElementById('slurp-error-overlay')
    const severityEl = Array.from(overlay?.querySelectorAll('div') ?? []).find(
      (d) => d.textContent === 'error',
    )
    expect(severityEl).toBeDefined()
    // #e63946 is the error colour; #90e0ef is the info fallback an
    // unnormalised PascalCase wire value lands on.
    expect(severityEl?.style.color).toBe('rgb(230, 57, 70)')
  })
})
