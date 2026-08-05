/**
 * URL parameter extraction from a route pattern.
 *
 * The compiler embeds the route pattern into the page HTML as:
 * ```html
 * <meta name="slurp-route" content="/products/:slug" />
 * ```
 *
 * At runtime, `parseParams()` reads that tag and matches it against the current
 * `window.location.pathname` to extract named segments.
 */

/**
 * Reads the `<meta name="slurp-route">` tag and returns named URL params
 * matched against `window.location.pathname`.
 *
 * Returns an empty object when:
 * - The meta tag is absent
 * - The pathname segment count does not match the pattern
 * - Any static segment does not match the actual pathname segment
 *
 * @returns A plain object mapping param names to their decoded string values.
 *
 * @example
 * // URL: /products/my-cool-widget
 * // <meta name="slurp-route" content="/products/:slug" />
 * parseParams() // => { slug: 'my-cool-widget' }
 */
export function parseParams(): Record<string, string> {
  const meta = document.querySelector<HTMLMetaElement>('meta[name="slurp-route"]')
  if (!meta) return {}

  const pattern = meta.content
  const pathname = window.location.pathname

  return matchRoute(pattern, pathname)
}

/**
 * Matches a route pattern containing `:param` segments against a concrete
 * pathname. Returns extracted params or an empty object on mismatch.
 *
 * @param pattern - Route pattern, e.g. `/blog/:year/:slug`
 * @param pathname - Concrete pathname, e.g. `/blog/2024/hello-world`
 * @returns Named params extracted from matching segments.
 */
export function matchRoute(pattern: string, pathname: string): Record<string, string> {
  const patternParts = pattern.split('/')
  const pathParts = pathname.split('/')

  if (patternParts.length !== pathParts.length) return {}

  const params: Record<string, string> = {}

  for (let i = 0; i < patternParts.length; i++) {
    const part = patternParts[i]
    const actual = pathParts[i]

    if (part === undefined || actual === undefined) return {}

    if (part.startsWith(':')) {
      params[part.slice(1)] = safeDecode(actual)
    } else if (part !== actual) {
      return {}
    }
  }

  return params
}

/**
 * Percent-decodes ONE path segment, falling back to the raw segment when the
 * escape sequence is malformed.
 *
 * `decodeURIComponent` throws `URIError` on input like `%` or `%zz`. This runs
 * during boot, before `Alpine.start()`, so an unguarded decode of a URL such as
 * `/products/%` throws out of `init()` and leaves the entire page without any
 * interactivity. A param that cannot be decoded is passed through verbatim: the
 * page stays alive and the template sees exactly what the URL contained.
 */
function safeDecode(segment: string): string {
  try {
    return decodeURIComponent(segment)
  } catch {
    return segment
  }
}
