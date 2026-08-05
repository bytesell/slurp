/**
 * Every filter, with the behaviour that actually matters when writing one.
 *
 * VERIFICATION. Each entry was checked twice: once by reading
 * `compiler/src/renderer/filters.rs` (the dispatch table is the definitive list,
 * and an unknown name there is a diagnostic), and once by RENDERING the filter
 * through the compiler and recording what came out. Where the two disagreed, the
 * rendered output won and is what is written here. The `nullBehaviour` and
 * `trap` fields exist because those are the parts a reader gets wrong, and they
 * are the parts a summary usually omits.
 *
 * COUNTS, which are load-bearing: 13 value filters and 4 loop filters. Older
 * notes about Slurp say 11 and 4, from before `json` and `unsafe_js` existed.
 */

export interface FilterDoc {
  name: string
  kind: 'value' | 'loop'
  signature: string
  summary: string
  /** What it does with null, and with a type it did not expect. */
  nullBehaviour: string
  /** The thing that surprises people. Omitted only when there genuinely is none. */
  trap?: string
  example?: string
}

export const FILTERS: FilterDoc[] = [
  {
    name: 'upper',
    kind: 'value',
    signature: 'upper',
    summary: 'Uppercases the stringified value, with full Unicode case mapping.',
    nullBehaviour: 'null renders as the empty string. An array or object is first serialised to JSON and then uppercased, which uppercases its KEYS too.',
    example: '${ name | upper }',
  },
  {
    name: 'lower',
    kind: 'value',
    signature: 'lower',
    summary: 'Lowercases the stringified value, with full Unicode case mapping.',
    nullBehaviour: 'null renders as the empty string. Same JSON caveat as upper.',
    example: '${ name | lower }',
  },
  {
    name: 'currency',
    kind: 'value',
    signature: 'currency(code = "USD")',
    summary:
      'Formats a number as money. Known codes and their symbols: USD $, EUR EUR-sign, GBP pound, JPY yen, CAD CA$, AUD AU$, CHF "CHF " (with a trailing space). Only JPY uses 0 decimals; every other code including unknown ones uses 2. EUR groups with "." and separates decimals with ","; CHF groups with an apostrophe; everything else groups with "," and separates with ".".',
    nullBehaviour:
      'A null, array, object or non-numeric string becomes 0, so it renders as the zero amount rather than blank. An UNKNOWN code is not an error: the code plus a space is used as the symbol, so currency("SEK") renders "SEK 10.00".',
    trap:
      'The symbol always PREFIXES, for every code, so EUR renders as EUR-sign then the amount rather than the usual European suffix. A negative renders with the sign outside the symbol (-$5.00). Rounding happens once in minor units, so 19.999 becomes $20.00. And the argument must be a LITERAL: currency(someVar) silently produces a leading space and no currency marker at all.',
    example: '${ price | currency("EUR") }',
  },
  {
    name: 'truncate',
    kind: 'value',
    signature: 'truncate(n = 50, suffix = "...")',
    summary:
      'Shortens a string to n CHARACTERS (not bytes, so it is Unicode-safe). The suffix is appended only when it actually truncated.',
    nullBehaviour: 'null becomes the empty string, which is length 0 and never truncates.',
    trap:
      'If truncation lands inside an unterminated HTML tag, everything from that "<" onward is dropped before the suffix is added. It does not balance tags, so truncating HTML is still unsafe.',
    example: '${ description | truncate(120, "...") }',
  },
  {
    name: 'fixed',
    kind: 'value',
    signature: 'fixed(n = 2)',
    summary: 'Formats a number with exactly n decimal places. Returns a STRING.',
    nullBehaviour: 'A null, array, object or non-numeric string becomes 0, so it renders as 0.00.',
    trap: 'A requested precision above 100 is silently clamped to 100 rather than rejected.',
    example: '${ rating | fixed(1) }',
  },
  {
    name: 'date',
    kind: 'value',
    signature: 'date(format = "MMM DD, YYYY")',
    summary:
      'Formats an ISO date string. The complete token set is YYYY, YY, MMMM, MMM, MM, M, DD, D. There are NO time tokens at all: no hours, minutes, seconds, timezone or day-of-week.',
    nullBehaviour:
      'Parsing takes the text before the first "T" or space and splits it on "-". If that yields fewer than three parts, THE INPUT IS RETURNED UNCHANGED.',
    trap:
      'That passthrough is the trap: epoch milliseconds have no "-", so "1700000000000" renders verbatim as "1700000000000" with no diagnostic. Pass RFC 3339 / ISO 8601 strings, never epoch numbers. There is also no escaping in the format string, so a literal "D" or "M" in surrounding text gets substituted.',
    example: '${ post.published_at | date("MMMM D, YYYY") }',
  },
  {
    name: 'default',
    kind: 'value',
    signature: 'default(fallback?)',
    summary:
      'Substitutes the fallback when the value is empty. "Empty" means null, the empty string, the empty array or the empty object. With no argument it yields the empty string.',
    nullBehaviour: 'This IS the null handler. Note that 0 and false are NOT empty and pass through.',
    trap:
      'This is the ONLY filter whose argument is evaluated as an expression in scope. Every other filter reads its arguments as literals. So default(user.name) works and currency(user.code) does not.',
    example: '${ user.nickname | default(user.name) }',
  },
  {
    name: 'plural',
    kind: 'value',
    signature: 'plural(one = "item", many = one + "s")',
    summary: 'Picks a singular or plural word based on a count.',
    nullBehaviour: 'A null or non-numeric value counts as 0, so the plural word is used.',
    trap:
      'The OUTPUT INCLUDES THE NUMBER. `${ 5 | plural("item") }` renders "5 items", not "items". Writing `${count} ${count | plural("item")}` prints the number twice.',
    example: '${ result_count | plural("result", "results") }',
  },
  {
    name: 'int',
    kind: 'value',
    signature: 'int',
    summary: 'Coerces to a whole number, truncating floats toward zero. Returns a NUMBER.',
    nullBehaviour: 'null, arrays and objects become 0. true becomes 1, false becomes 0.',
    trap: 'String parsing is all-or-nothing, with no prefix parsing: "12abc" becomes 0, not 12.',
    example: '${ raw_count | int }',
  },
  {
    name: 'float',
    kind: 'value',
    signature: 'float',
    summary: 'Coerces to a floating-point number. Returns a NUMBER.',
    nullBehaviour: 'null, arrays and objects become 0.',
    trap: 'Same all-or-nothing string parsing: "1.5x" becomes 0.',
    example: '${ raw_price | float }',
  },
  {
    name: 'js',
    kind: 'value',
    signature: 'js',
    summary:
      'Escapes a value for placement INSIDE a JavaScript string literal you wrote yourself. It escapes quotes, backtick, backslash, "$", newlines, U+2028/U+2029, "<" and "/". It does NOT add the surrounding quotes.',
    nullBehaviour: 'null becomes the empty string.',
    trap:
      'It is a CLAIM about position, and a false claim is refused at compile time. Using it where the slot is NOT inside a quoted string (statement or expression position) is an UnsafeScriptInterpolation error, because escaping quote characters cannot contain a value that lands next to a ";" or "(". Quote the slot, or use | json.',
    example: "<button @click=\"add('${ id | js }')\">Add</button>",
  },
  {
    name: 'json',
    kind: 'value',
    signature: 'json',
    summary:
      'Serialises the value as a complete JSON literal, script-safely (it rewrites "<" and the line separators). A string comes out WITH its quotes.',
    nullBehaviour: 'null serialises as the JSON literal null.',
    trap:
      'Because it is self-delimiting, this is the right choice in JavaScript statement or expression position, where | js is refused. Do not wrap it in your own quotes as well.',
    example: '<script>const config = ${ page.config | json };</script>',
  },
  {
    name: 'unsafe_js',
    kind: 'value',
    signature: 'unsafe_js',
    summary:
      'No escaping at all. It exists so a theme can declare that a value IS JavaScript the theme itself wrote, rather than data.',
    nullBehaviour: 'null becomes the empty string.',
    trap:
      'Never reachable by untrusted data without an XSS. It is deliberately NOT accepted as the script filter in a <script> body: the raw hatch there is {html expr}.',
    example: '<button @click={`${ handlerName | unsafe_js }()`}>Go</button>',
  },
  {
    name: 'limit',
    kind: 'loop',
    signature: 'limit(n)',
    summary: 'Truncates the collection to the first n items.',
    nullBehaviour: 'The argument is REQUIRED. A missing or non-literal argument is an error.',
    trap:
      'A loop-filter error EMPTIES the collection, which then renders the {empty} branch. So a broken limit() looks like "there is no data" rather than like an error.',
    example: '{each p in products | limit(12)}<li>${p.name}</li>{/each}',
  },
  {
    name: 'sort',
    kind: 'loop',
    signature: 'sort(key?, dir?)',
    summary:
      'Sorts ascending by default. Pass "desc" as the second argument to reverse. With a key, sorts by that field on each item, falling back to the whole item when the key is absent. The sort is stable.',
    nullBehaviour:
      'Cross-type pairs are ordered by type rank (null, bool, number, string, array, object), so a mixed collection sorts deterministically rather than erratically.',
    trap: 'Only the exact string "desc" reverses. "DESC" and "descending" both mean ascending.',
    example: '{each p in products | sort("price", "desc")}<li>${p.name}</li>{/each}',
  },
  {
    name: 'reverse',
    kind: 'loop',
    signature: 'reverse',
    summary: 'Reverses the collection in place.',
    nullBehaviour: 'An empty or null collection stays empty.',
    example: '{each p in products | reverse}<li>${p.name}</li>{/each}',
  },
  {
    name: 'filter',
    kind: 'loop',
    signature: 'filter(key, value)',
    summary: 'Keeps items whose item[key] equals value. Both arguments must be literals.',
    nullBehaviour:
      'Items that are not objects, or that lack the key, are DROPPED. Comparison is type-strict, so filter("id", "1") never matches a numeric 1.',
    trap:
      'Passing a variable as the key makes the key the empty string, so every item is dropped and the loop renders its {empty} branch.',
    example: '{each p in products | filter("featured", true)}<li>${p.name}</li>{/each}',
  },
]

export const VALUE_FILTER_COUNT = FILTERS.filter((f) => f.kind === 'value').length
export const LOOP_FILTER_COUNT = FILTERS.filter((f) => f.kind === 'loop').length
