import type { SlurpCompletionItem } from '../../shared/interfaces'

export const FILTERS: SlurpCompletionItem[] = [
  {
    label: 'currency',
    kind: 'function',
    detail: 'currency | currency("EUR")',
    documentation: 'Format as locale-aware currency. Optionally specify currency code.',
    insertText: 'currency',
  },
  {
    label: 'upper',
    kind: 'function',
    detail: 'upper',
    documentation: 'Convert to uppercase.',
    insertText: 'upper',
  },
  {
    label: 'lower',
    kind: 'function',
    detail: 'lower',
    documentation: 'Convert to lowercase.',
    insertText: 'lower',
  },
  {
    label: 'truncate',
    kind: 'function',
    detail: 'truncate(n) | truncate(n, "suffix")',
    documentation:
      'Truncate at n characters (default 50). The suffix is appended ONLY when the value was actually truncated, and defaults to three dots.',
    insertText: 'truncate(${1:50})',
    insertTextFormat: 'snippet',
  },
  {
    label: 'fixed',
    kind: 'function',
    detail: 'fixed(n)',
    documentation: 'Format to n decimal places.',
    insertText: 'fixed(${1:2})',
    insertTextFormat: 'snippet',
  },
  {
    label: 'date',
    kind: 'function',
    detail: 'date | date("MMM DD YYYY")',
    documentation: 'Format as date. Optionally specify format string.',
    insertText: 'date',
  },
  {
    label: 'default',
    kind: 'function',
    detail: 'default("fallback")',
    documentation:
      'Return fallback when the value is null or empty (an empty string, array or object). 0 and false are NOT empty, and slurp has no undefined.',
    insertText: 'default("${1:fallback}")',
    insertTextFormat: 'snippet',
  },
  {
    label: 'plural',
    kind: 'function',
    detail: 'plural("singular", "plural")',
    documentation: 'Format with singular/plural form: "1 item" or "3 items".',
    insertText: 'plural("${1:item}", "${2:items}")',
    insertTextFormat: 'snippet',
  },
  {
    label: 'int',
    kind: 'function',
    detail: 'int',
    documentation: 'Cast to integer.',
    insertText: 'int',
  },
  {
    label: 'float',
    kind: 'function',
    detail: 'float',
    documentation: 'Cast to float.',
    insertText: 'float',
  },
  {
    label: 'js',
    kind: 'function',
    detail: 'js',
    documentation:
      'Escape for a JavaScript STRING LITERAL, in a <script> body or a JS-evaluated attribute. It is a claim about context: a slot carrying it is emitted verbatim, so a | js outside a string literal is a compile error. Quote the slot, or use json.',
    insertText: 'js',
  },
  {
    label: 'json',
    kind: 'function',
    detail: 'json',
    documentation:
      'Serialise as a JSON literal, which is self-delimiting and therefore safe in JavaScript STATEMENT or EXPRESSION position. This or js is REQUIRED on any interpolation in a <script> body.',
    insertText: 'json',
  },
  {
    label: 'unsafe_js',
    kind: 'function',
    detail: 'unsafe_js',
    documentation:
      'Emit with NO escaping into a JavaScript context. Only for JavaScript the THEME itself wrote, such as an expression passed to a component as a prop. Never route data through it: use js or json.',
    insertText: 'unsafe_js',
  },
  {
    label: 'limit',
    kind: 'function',
    detail: 'limit(n)',
    documentation: '(Loop filter) Take first n items from collection.',
    insertText: 'limit(${1:10})',
    insertTextFormat: 'snippet',
  },
  {
    label: 'sort',
    kind: 'function',
    detail: 'sort("key") | sort("key", "desc")',
    documentation: '(Loop filter) Sort collection by key, ascending or descending.',
    insertText: 'sort("${1:field}")',
    insertTextFormat: 'snippet',
  },
  {
    label: 'reverse',
    kind: 'function',
    detail: 'reverse',
    documentation: '(Loop filter) Reverse collection order.',
    insertText: 'reverse',
  },
  {
    label: 'filter',
    kind: 'function',
    detail: 'filter("key", "value")',
    documentation: '(Loop filter) Keep items where item[key] === value.',
    insertText: 'filter("${1:key}", "${2:value}")',
    insertTextFormat: 'snippet',
  },
]

export const LOOP_VARS: SlurpCompletionItem[] = [
  { label: 'loop.index', kind: 'variable', detail: 'number', documentation: '0-based loop index' },
  { label: 'loop.count', kind: 'variable', detail: 'number', documentation: '1-based loop count' },
  { label: 'loop.first', kind: 'variable', detail: 'boolean', documentation: 'True on first iteration' },
  { label: 'loop.last', kind: 'variable', detail: 'boolean', documentation: 'True on last iteration' },
]

export const GLOBALS: SlurpCompletionItem[] = [
  { label: 'seo.title', kind: 'variable', detail: 'string', documentation: 'Page SEO title' },
  { label: 'seo.description', kind: 'variable', detail: 'string', documentation: 'Page SEO description' },
  { label: 'seo.image', kind: 'variable', detail: 'string', documentation: 'Page SEO image URL' },
  { label: 'params', kind: 'variable', detail: 'Record<string, string>', documentation: 'URL route params' },
  { label: 'env.dev', kind: 'variable', detail: 'boolean', documentation: 'True in development mode' },
  { label: 'env.prod', kind: 'variable', detail: 'boolean', documentation: 'True in production mode' },
  { label: 'error.message', kind: 'variable', detail: 'string', documentation: 'Error message (in {error} block)' },
  { label: 'error.code', kind: 'variable', detail: 'string', documentation: 'Error code (in {error} block)' },
  { label: 'error.status', kind: 'variable', detail: 'number', documentation: 'HTTP status (in {error} block)' },
  { label: 'request.cookies', kind: 'variable', detail: 'Record<string, string>', documentation: 'Request cookies (middleware only)' },
  { label: 'request.headers', kind: 'variable', detail: 'Record<string, string>', documentation: 'Request headers (middleware only)' },
  { label: 'request.method', kind: 'variable', detail: 'string', documentation: 'HTTP method (middleware only)' },
  { label: 'request.path', kind: 'variable', detail: 'string', documentation: 'Request path (middleware only)' },
]

export const SNIPPETS: SlurpCompletionItem[] = [
  {
    label: 'if',
    kind: 'snippet',
    insertText: '{if ${1:condition}}\n\t$0\n{/if}',
    insertTextFormat: 'snippet',
    documentation: 'Conditional block',
  },
  {
    label: 'each',
    kind: 'snippet',
    insertText: '{each ${1:item} in ${2:items}}\n\t$0\n{/each}',
    insertTextFormat: 'snippet',
    documentation: 'Loop over collection',
  },
  {
    label: 'fetch',
    kind: 'snippet',
    insertText: '{fetch ${1:data}: ${2:Type} from ${3:fn}}\n\t$0\n{loading}\n\t<p>Loading...</p>\n{/loading}\n{error}\n\t<p>${ error.message }</p>\n{/error}\n{/fetch}',
    insertTextFormat: 'snippet',
    documentation:
      'Async data fetch block. Accepts three optional branches, which the parser reads positionally in this order: {loading}, {error}, {empty}.',
  },
  {
    label: 'match',
    kind: 'snippet',
    insertText: '{match ${1:value}}\n\t"${2:case1}" -> $0\n\t_ -> <span>default</span>\n{/match}',
    insertTextFormat: 'snippet',
    documentation: 'Pattern match block',
  },
  {
    label: 'frontmatter',
    kind: 'snippet',
    insertText: '---\nusing "@components/${1:Component}"\n\nprops {\n\t${2:name}: ${3:string}\n}\n---\n\n$0',
    insertTextFormat: 'snippet',
    documentation: 'Frontmatter block',
  },
  {
    label: 'layout',
    kind: 'snippet',
    insertText: '<layout src="@layouts/${1:base}">\n\t<Head>\n\t\t<title>${ seo.title }</title>\n\t</Head>\n\t$0\n</layout>',
    insertTextFormat: 'snippet',
    documentation: 'Layout wrapper',
  },
]

export const FETCH_OPTIONS: SlurpCompletionItem[] = [
  { label: 'cache', kind: 'keyword', detail: 'cache:300', documentation: 'Cache results for N seconds', insertText: 'cache:${1:300}', insertTextFormat: 'snippet' },
  { label: 'retry', kind: 'keyword', detail: 'retry:3', documentation: 'Retry failed fetch up to N times', insertText: 'retry:${1:3}', insertTextFormat: 'snippet' },
  { label: 'timeout', kind: 'keyword', detail: 'timeout:5000', documentation: 'Abort after N milliseconds', insertText: 'timeout:${1:5000}', insertTextFormat: 'snippet' },
  { label: 'poll', kind: 'keyword', detail: 'poll:10', documentation: 'Re-fetch every N seconds', insertText: 'poll:${1:10}', insertTextFormat: 'snippet' },
  { label: 'paginate', kind: 'keyword', detail: 'paginate limit=12', documentation: 'Enable pagination', insertText: 'paginate limit=${1:12}', insertTextFormat: 'snippet' },
  { label: 'infinite', kind: 'keyword', detail: 'infinite limit=10', documentation: 'Enable infinite scroll', insertText: 'infinite limit=${1:10}', insertTextFormat: 'snippet' },
  { label: 'abort:navigate', kind: 'keyword', documentation: 'Abort in-flight fetch on navigation', insertText: 'abort:navigate' },
]

// Returns all completion categories merged - the IDE filters by typed prefix.
export function getCompletions(
  _source: string,
  _line: number,
  _character: number,
): SlurpCompletionItem[] {
  return [...FILTERS, ...LOOP_VARS, ...GLOBALS, ...SNIPPETS, ...FETCH_OPTIONS]
}
