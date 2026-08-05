import type { SlurpHoverResult } from '../../shared/interfaces'

const FILTER_DOCS: Record<string, string> = {
  currency: '**currency** `value | currency` or `value | currency("EUR")`\n\nLocale-aware currency formatting. Default currency: USD.',
  upper: '**upper** `value | upper`\n\nConvert string to uppercase.',
  lower: '**lower** `value | lower`\n\nConvert string to lowercase.',
  truncate: '**truncate** `value | truncate(50)` or `value | truncate(50, " more")`\n\nTruncate at n characters (default 50). The suffix is appended ONLY when the value was actually truncated, and defaults to three dots.',
  fixed: '**fixed** `value | fixed(2)`\n\nFormat to n decimal places.',
  date: '**date** `value | date` or `value | date("MMM DD YYYY")`\n\nFormat as a human-readable date.',
  default: '**default** `value | default("fallback")`\n\nReturn fallback when the value is null or empty (an empty string, array or object). Note that `0` and `false` are NOT empty, and slurp has no `undefined`.',
  plural: '**plural** `count | plural("item", "items")`\n\nFormat: "1 item" or "3 items".',
  int: '**int** `value | int`\n\nCast to integer.',
  float: '**float** `value | float`\n\nCast to float.',
  js: '**js** `value | js`\n\nEscape a value for a JavaScript **string literal**, in a `<script>` body or a JS-evaluated attribute.\n\nIt is a CLAIM about context: a slot carrying it is emitted verbatim, so a `| js` outside a string literal is a compile error. Quote the slot, or use `json`.',
  json: '**json** `value | json`\n\nSerialise as a JSON literal, which is self-delimiting and therefore safe in JavaScript **statement or expression** position.\n\nThis or `js` is REQUIRED on any `${ }` in a `<script>` body; an undeclared interpolation there is a compile error.',
  unsafe_js: '**unsafe_js** `value | unsafe_js`\n\nEmit with NO escaping into a JavaScript context. Only for JavaScript the THEME itself wrote (an expression passed to a component as a prop).\n\nNever route data through it: use `js` or `json` instead.',
  limit: '**limit** `collection | limit(10)` _(loop filter)_\n\nTake first n items.',
  sort: '**sort** `collection | sort("price")` _(loop filter)_\n\nSort ascending by key. Use `sort("price", "desc")` for descending.',
  reverse: '**reverse** `collection | reverse` _(loop filter)_\n\nReverse collection order.',
  filter: '**filter** `collection | filter("status", "active")` _(loop filter)_\n\nKeep items where item[key] === value.',
}

const KEYWORD_DOCS: Record<string, string> = {
  if: '**{if condition}** ... **{/if}**\n\nConditional block. Supports `{else if}` and `{else}` branches.',
  each: '**{each item in collection}** ... **{/each}**\n\nLoop over a collection. Available: `loop.index`, `loop.count`, `loop.first`, `loop.last`.\n\nSupports `{empty}` branch for empty collections.',
  match: '**{match value}** ... **{/match}**\n\nPattern match. Arms: `"string" ->`, `42 ->`, `_ ->` (wildcard).',
  fetch: '**{fetch name: Type from fn}** ... **{/fetch}**\n\nAsync data fetch. Supports `{loading}`, `{error}` and `{empty}` branches, in that order.\n\nOptions: `cache:300`, `retry:3`, `timeout:5000`, `poll:10`, `paginate limit=12`, `infinite limit=10`, `abort:navigate`.',
  repeat: '**{repeat n}** ... **{/repeat}**\n\nRepeat body n times.',
  try: '**{try}** ... **{error}** ... **{/try}**\n\nError boundary. Catches render errors.',
}

export function getHover(word: string): SlurpHoverResult | null {
  if (FILTER_DOCS[word]) {
    return { contents: FILTER_DOCS[word] }
  }
  if (KEYWORD_DOCS[word]) {
    return { contents: KEYWORD_DOCS[word] }
  }
  return null
}
