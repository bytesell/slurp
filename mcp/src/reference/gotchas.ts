/**
 * The mistakes that cost real time on a real project.
 *
 * The `detectedBy` field is the point of this file. A gotcha an agent will be
 * told about by a tool is a different kind of problem from one that ships
 * silently, and conflating the two is how a "here are the pitfalls" list stops
 * being useful. Each entry says exactly which of the three states it is in:
 * the compiler reports it, `slurp_lint` reports it, or nothing reports it at
 * all and it has to be known in advance.
 *
 * Every entry was confirmed against the compiler by writing the wrong form,
 * rendering it, and recording the output.
 */

export interface Gotcha {
  title: string
  explanation: string
  wrong?: string
  right?: string
  detectedBy: string
}

export const GOTCHAS: Gotcha[] = [
  {
    title: 'A quoted component prop is a LITERAL string',
    explanation:
      'Element attributes interpolate; component props do not. The same quoted text means two different things depending on whether the tag starts with an uppercase letter. This is the single most common mistake in the language, and the compiler says nothing about it: the prop simply arrives with the interpolation syntax intact as literal characters.',
    wrong: '<Card title="Hi ${name}" />',
    right: '<Card title={`Hi ${name}`} />',
    detectedBy: '`slurp_lint`, rule `component-prop-literal-interpolation`. The compiler does not.',
  },
  {
    title: 'A `${ }` inside a `<script>` body must end in `| js` or `| json`',
    explanation:
      'A script body is a raw-text element, so the HTML parser hands it to the JavaScript engine without decoding character references. The entity escaping that protects ordinary text is inert there. Use `| js` for a value sitting inside a string literal you wrote, and `| json` for a bare value that needs its own delimiters. `| unsafe_js` is deliberately not accepted.',
    wrong: '<script>var n = ${ user.name };</script>',
    right: '<script>var n = ${ user.name | json };</script>',
    detectedBy: '`slurp_validate`, as an `UnsafeScriptInterpolation` error. This is a hard build failure.',
  },
  {
    title: 'A JavaScript template literal in an attribute gets eaten',
    explanation:
      'Slurp interpolates `${ }` in attribute values itself, so a backtick template written for the browser has its slots consumed by the template engine and evaluated against the SERVER render context instead. A client-side variable therefore renders EMPTY, with no runtime error and no other sign of failure: you get dead links and unstyled elements on a page that otherwise looks fine. Write string concatenation instead.',
    wrong: '<a :href="`/product/${item.slug}`">View</a>',
    right: "<a :href=\"'/product/' + item.slug\">View</a>",
    detectedBy: '`slurp_validate`, as a `JsTemplateLiteralInAttribute` WARNING. It is a warning, not an error, so a build still succeeds.',
  },
  {
    title: 'In JavaScript statement position, `| js` is an ERROR',
    explanation:
      '`| js` is a CLAIM that the slot sits inside a quoted JavaScript string, and a declared slot is then emitted verbatim. Outside a string that turns the auto-escaper off and puts nothing in its place, and escaping quote characters cannot contain a value landing next to a `;` or a `(`. An UNDECLARED slot in the same position is fine: it is auto-encoded as a self-delimiting JSON literal.',
    wrong: '<button @click="add(${ id | js })">Add</button>',
    right: '<button @click="add(${ id | json })">Add</button>',
    detectedBy: '`slurp_validate`, as an `UnsafeScriptInterpolation` error.',
  },
  {
    title: 'Filter arguments must be LITERALS',
    explanation:
      'Filter arguments are read as literals, never evaluated. A variable argument is seen as the empty string, so the filter falls back to its default or emits nothing. `currency(cur)` with `cur = "EUR"` renders a leading space and NO currency symbol at all. `default` is the sole exception: the renderer evaluates its argument in scope.',
    wrong: '${ price | currency(settings.currency) }',
    right: '${ price | currency("EUR") }',
    detectedBy: '`slurp_lint`, rule `filter-arg-not-literal`. The compiler does not, except for `limit()` and `filter()` where the argument is required.',
  },
  {
    title: '`| date` parses ISO strings, not epoch milliseconds',
    explanation:
      'The parser takes the text before the first "T" or space and splits it on "-". Fewer than three parts and THE INPUT IS RETURNED UNCHANGED. Epoch milliseconds have no "-", so they render verbatim as a long number. There are also no time tokens at all: only YYYY, YY, MMMM, MMM, MM, M, DD and D exist.',
    wrong: '${ 1700000000000 | date("MMM DD, YYYY") }',
    right: '${ "2026-08-03T10:00:00Z" | date("MMM DD, YYYY") }',
    detectedBy: 'Nothing. Return RFC 3339 dates from the server and this cannot arise.',
  },
  {
    title: 'Money arrives as a STRING',
    explanation:
      'A server decimal serialises to JSON as a string such as "49.990000", not as a number. In a template that is usually fine, because filters coerce. It matters in the JavaScript you emit: calling .toFixed on that value throws, because a string has no such method. Coerce first, or format server-side and pass the finished string.',
    wrong: '<script>var t = ${ record.amount | json }.toFixed(2);</script>',
    right: '<script>var t = Number(${ record.amount | json }).toFixed(2);</script>',
    detectedBy: 'Nothing. The compiler does not read the JavaScript you emit.',
  },
  {
    title: 'A nested double quote inside `${ }` ends the attribute',
    explanation:
      'The lexer closes an attribute value at its first matching quote, before interpolation is considered. So a double-quoted string key inside a double-quoted attribute truncates the attribute and turns the remainder into stray attributes. Either use the brace form, which is not quote-delimited, or use single quotes inside the expression.',
    wrong: '<div title="${ a["k"] }">',
    right: '<div title={ a["k"] }>',
    detectedBy: '`slurp_lint`, rule `unterminated-attribute-interpolation`. The compiler does not.',
  },
  {
    title: 'There are NO callable functions',
    explanation:
      'A call parses and always evaluates to null, with no diagnostic, so it renders the empty string. Every JavaScript habit lands here: Math.max, items.map, String(x), parseInt. The only call-shaped construct in the language is a filter. There are also no array properties, so items.length is null too.',
    wrong: '<p>${ Math.max(a, b) }</p>',
    right: '<p>${ a > b ? a : b }</p>',
    detectedBy: '`slurp_lint`, rules `call-always-null` and `array-length-property`. The compiler does not.',
  },
  {
    title: 'Components and layouts get their own root scope',
    explanation:
      'A component renders with the page globals overlaid by its explicit props, with props winning on collision. That means it CAN read a page global it was never passed, which is often convenient. What it cannot see is the caller LOCAL bindings: an {each} loop variable and loop.index are invisible inside a component unless passed as props.',
    wrong: '{each p in products}<Card />{/each}   {* Card cannot see p *}',
    right: '{each p in products}<Card product={p} />{/each}',
    detectedBy: 'Nothing. It renders as empty values inside the component.',
  },
  {
    title: '`{$let}` creates no template binding',
    explanation:
      'A {$let} seeds CLIENT state. Server-side it emits only a <script type="application/json"> payload, so reading the name back in the same template renders the empty string.',
    wrong: '{$let count = 0}<p>${ count }</p>',
    right: 'Pass the value through the render context and read it directly.',
    detectedBy: '`slurp_lint`, rule `let-binding-not-readable`.',
  },
  {
    title: 'An unresolvable component renders a placeholder, not an error',
    explanation:
      'A component whose import did not resolve does NOT fail the build. It renders a <div data-slurp-component="Name" data-slurp-props="..."> placeholder, so a typo in an import path looks like a clean build with a missing card. The same is true of this MCP server, which has no cross-file resolution at all: every component renders as a placeholder here.',
    detectedBy: 'Nothing. Check the rendered HTML for `data-slurp-component` attributes.',
  },
  {
    title: 'A truncated loop still exits 0, and -v is what prints the warning',
    explanation:
      'Every budget truncates rather than failing, so a page that loses the tail of a list still builds successfully. The loop cap warns in both modes, but `slurp build` prints warnings only with `-v`, so the default console is quiet. Two advisories ARE development-only: the CSS-structural strip and the undeclared-JS-slot note. The `slurp_render` tool here uses development mode so those two are visible.',
    detectedBy: '`slurp_render`. On the CLI, `slurp build -v` reports the loop cap; without `-v` it is silent.',
  },
  {
    title: 'Comparisons do not coerce, but arithmetic does',
    explanation:
      'Arithmetic coerces freely: a non-numeric string, null, an array or an object all become 0, and `+` concatenates whenever either side is a string. Comparisons do the opposite: only number-to-number and string-to-string actually compare, and every other pairing compares Equal, so `>` is false and `>=` is true. So `"10" > 9` is FALSE. Division by zero yields 0 rather than an error or Infinity.',
    wrong: '{if product.stock_string > 0}',
    right: '{if (product.stock_string | int) > 0}',
    detectedBy: 'Nothing. Coerce explicitly with `| int` or `| float` when a value may be a string.',
  },
  {
    title: '`{with}` does not exist, and `{match}` has no `{case}`',
    explanation:
      'The lexer recognises `with` as a block keyword but no parser supports it, so every `{with}` is a hard error. Inside `{match}`, arms are a bare pattern then `->` then a body; there is no `{case}` keyword. Also note that `{each}` puts the ITEM name first and the index second.',
    wrong: '{match s}{case "a"}A{/case}{/match}',
    right: '{match s}\n  "a" -> A\n  _   -> other\n{/match}',
    detectedBy: '`slurp_validate`, as `UnexpectedToken` errors.',
  },
  {
    title: 'Unknown frontmatter directives are silently skipped',
    explanation:
      'A misspelled or non-existent frontmatter directive produces no diagnostic at all. There is no `layout` directive, for instance: use the `{layout "..."}` block tag in the body. Note also that `props` entries must be one per line.',
    detectedBy: 'Nothing. Use `slurp_schema` to see what the compiler actually read out of your frontmatter.',
  },
  {
    title: 'A @theme block with no catalog is dropped from the render',
    explanation:
      'Without a block catalog, a @theme-targeted block is an unknown type and is dropped at merge time while surviving intact in storage. The page renders as though nothing was added, and the data is not lost. This is the most surprising behaviour in the schema subsystem.',
    detectedBy: 'Nothing. It is a host integration concern, not a template one.',
  },
]
