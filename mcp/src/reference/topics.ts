/**
 * The language reference, by topic.
 *
 * HOW THIS WAS VERIFIED, because a reference nobody checked is worse than none.
 * Every statement below was established one of two ways, and mostly both:
 *
 *   (a) Read off the compiler source. The keyword table, the operator binding
 *       powers, the setting kinds, the void-element list and the numeric limits
 *       are all single definitions in Rust and are quoted from them.
 *   (b) Rendered through the compiler and observed. Anything with an OBSERVED
 *       note was run: the source was compiled and the actual output recorded.
 *
 * Where a pre-existing document about Slurp disagreed with the compiler, the
 * compiler won and the correction is called out inline, because those stale
 * claims are still in circulation and an agent that half-remembers one needs to
 * see it contradicted rather than merely omitted.
 */
import { FILTERS, LOOP_FILTER_COUNT, VALUE_FILTER_COUNT } from './filters.js'
import { DECLARED_ONLY_ERROR_CODES, EMITTED_ERROR_CODES, ERROR_CODES } from './errors.js'
import { GOTCHAS } from './gotchas.js'

export interface Topic {
  id: string
  title: string
  summary: string
  render: () => string
}

function filterTable(kind: 'value' | 'loop'): string {
  return FILTERS.filter((f) => f.kind === kind)
    .map((f) => {
      const lines = [`### \`${f.signature}\``, '', f.summary, '', `Null and wrong types: ${f.nullBehaviour}`]
      if (f.trap) lines.push('', `TRAP: ${f.trap}`)
      if (f.example) lines.push('', '```slurp', f.example, '```')
      return lines.join('\n')
    })
    .join('\n\n')
}

const OVERVIEW = `# Slurp

Slurp is a lightweight, agent-first templating engine. You write pages,
components and layouts; it compiles them to plain HTML with almost no client
JavaScript. Sections and blocks additionally declare a typed schema, so a
non-technical person can edit a page through a form without touching the
template.

The mental model closest to it is a small Astro: file-based pages, components
with props and slots, layouts, and a build step that produces static HTML.

## What a file looks like

\`\`\`slurp
---
using "@components/ProductCard"
section {
  name: "Featured"
  settings {
    heading: text = "Featured products"
    limit:   number = 8 { min: 1, max: 24 }
  }
}
---
<section>
  <h2>\${ section.settings.heading }</h2>
  {each product in products | limit(8)}
    <ProductCard product={product} />
  {empty}
    <p>Nothing here yet.</p>
  {/each}
</section>
\`\`\`

## The shape of the language

- \`\${ expr }\` interpolates, HTML-escaped. \`{ expr }\` is the same thing.
- Block tags are brace-delimited: \`{if}\`, \`{each}\`, \`{match}\`, \`{fetch}\`,
  \`{repeat}\`, \`{try}\`, \`{slot}\`, \`{layout}\`, \`{head}\`, \`{sections}\`,
  \`{blocks}\`.
- \`{* comments *}\` are stripped from the output.
- Frontmatter between \`---\` lines declares imports, props, types, middleware
  and the section or block schema.
- Everything is rendered on the server. Rendering is deterministic and has no
  side effects: the compiler performs no network request, reads no file the host
  did not hand it, and given the same source and context produces the same bytes.

## Four things that will save you time

1. A quoted COMPONENT prop is a literal string; a quoted ELEMENT attribute
   interpolates. This asymmetry is the most common mistake in the language.
2. There are no callable functions. \`Math.max(a, b)\` parses and evaluates to
   null.
3. Filter arguments must be literals. \`default\` is the only exception.
4. A \`\${ }\` inside a \`<script>\` body must end in \`| js\` or \`| json\`.

Ask for the \`gotchas\` topic for the full list, each with the fix.`

const TAGS = `# Block tags

The lexer recognises 14 block keywords. 11 are usable tags, 2 are branch markers
valid only inside another tag, and 1 (\`with\`) is lexed but has no parser support
at all, so every \`{with}\` is a hard error. That last one is worth knowing: it
looks like a language feature and is not one.

## \`{if}\`

\`\`\`slurp
{if notifications.count > 0}
  <a href="/inbox">Inbox (\${ notifications.count })</a>
{else if user.is_guest}
  <a href="/login">Sign in</a>
{else}
  <span>Nothing new</span>
{/if}
\`\`\`

Any number of \`{else if}\`, at most one \`{else}\`. Neither takes a closing tag.

Truthiness: \`null\`, \`false\`, the number \`0\`, the empty string, the empty
array and the empty object are falsy. Everything else is truthy, INCLUDING the
strings \`"0"\` and \`"false"\`. Note that an empty array is falsy here and truthy
in JavaScript.

## \`{each}\`

\`\`\`slurp
{each product, i in products | limit(12) | sort("price")}
  <li>\${ i }: \${ product.name }</li>
{empty}
  <li>Nothing here yet.</li>
{/each}
\`\`\`

THE ITEM NAME COMES FIRST, the index second. The \`in\` keyword is required.
\`{empty}\` takes no closing tag; \`{/each}\` is required.

Four loop variables are bound automatically:

| Variable | Meaning |
|---|---|
| \`loop.index\` | 0-based position |
| \`loop.count\` | the TOTAL number of items, not index+1 |
| \`loop.first\` | index == 0 |
| \`loop.last\` | index == count-1 |

Filters in an \`{each}\` header are LOOP filters, a different table from the
value filters. Using a value filter name there is an error.

A non-array, non-null collection is wrapped and iterated once. A null collection
is empty and takes the \`{empty}\` branch. Each loop is capped at 1000 items.

## \`{match}\`

\`\`\`slurp
{match post.status}
  "published" -> <span class="ok">Published</span>
  "archived"  -> <span class="warn">Archived</span>
  _           -> <span>\${ post.status }</span>
{/match}
\`\`\`

THERE IS NO \`{case}\` KEYWORD. Arms are a bare pattern, then \`->\`, then a body.
Patterns are string, number or boolean literals, or \`_\` for the wildcard.

Matching is type-strict: a \`"1"\` pattern never matches a numeric \`1\`. With no
matching arm and no \`_\`, the block renders nothing.

## \`{fetch}\`

\`\`\`slurp
{fetch products: Product[] from "/api/products" cache(300) retry(2)}
  {each p in products}<li>\${ p.name }</li>{/each}
{loading}
  {repeat 6}<div class="skeleton"></div>{/repeat}
{error}
  <p>Could not load products.</p>
{empty}
  <p>No products yet.</p>
{/fetch}
\`\`\`

THE BRANCH ORDER IS FIXED: body, then \`{loading}\`, then \`{error}\`, then
\`{empty}\`. Out of order is not recovered and reports an unclosed block.

Options, all optional: \`cache(n)\`, \`retry(n)\`, \`timeout(n)\`, \`poll(n)\`,
\`paginate(n)\`, \`infinite(n)\`, and the bare flag \`abort-on-navigate\` (also
spelled \`abortOnNavigate\`). An unrecognised option is SILENTLY DROPPED.

Server-side the compiler performs NO network request. It reads the name out of
the render context and picks a branch: absent or null gives \`{loading}\`; an
object carrying an \`__error\` key gives \`{error}\`; an empty array gives
\`{empty}\`; anything else renders the body with the name rebound. The type hint
is recorded and has no effect on rendering.

## \`{repeat}\`

\`\`\`slurp
{repeat 8}<div class="skeleton-card"></div>{/repeat}
\`\`\`

The count is any expression; non-numeric means 0, and it is capped at 1000.

## \`{try}\`

\`\`\`slurp
{try}<LiveFeed items={$feed.items} />{error}<p>Feed unavailable.</p>{/try}
\`\`\`

THE SERVER ALWAYS RENDERS THE BODY AND NEVER THE \`{error}\` BRANCH. This is
purely a client-side boundary and needs the browser runtime to do anything.

## \`{html expr}\` and \`{html expr sanitize}\`

Raw, unescaped output; the only way to bypass escaping.

\`\`\`slurp
{html product.description}        {* trusted only *}
{html review.body sanitize}       {* safe for untrusted input *}
\`\`\`

\`sanitize\` runs a strict tag and attribute allowlist and permits only the
http, https and mailto URL schemes. Any \`<a target=...>\` gains
\`rel="noopener noreferrer"\`.

\`{html}\` also works in ATTRIBUTE position as an attribute spread, including a
conditional form:

\`\`\`slurp
<input {html attrs} {if is_required}{html "required"}{/if} />
\`\`\`

That path is sanitized differently: the spread is re-parsed into whole
attributes and any with an unsafe name (\`on*\`, \`x-html\`, \`x-data\`,
\`x-init\`, \`x-effect\`) or a script-scheme value is dropped.

## \`{$let name = expr}\`

Seeds CLIENT state. Server-side it emits ONLY a JSON payload:

\`\`\`html
<script type="application/json" data-slurp-let="count">0.0</script>
\`\`\`

IT CREATES NO TEMPLATE BINDING. Writing \`{$let n = 5}\` and then \`\${ n }\`
renders the empty string. (OBSERVED.) Object and array literals are not valid
values, because the language has no such literals; pass them through context.

## \`{slot}\` and \`{layout}\`

Only two slot names are ever filled: \`default\` and \`head\`. Any other name
renders as a literal \`<slot name="x">\` element.

\`\`\`slurp
{layout "@layouts/base"}
  {head}<title>Products</title>{/head}
  <h1>Products</h1>
{/layout}
\`\`\`

The two layout forms are NOT equivalent. With \`{layout "..."}\` the \`{head}\`
children are extracted into the layout's head slot. With the element form
\`<layout src="...">\` they are not, and render inline in the body.

There is no \`layout\` frontmatter directive. A \`layout:\` line in frontmatter is
silently ignored.

## \`{sections}\` and \`{blocks}\`

Host integration points. \`{sections}\` emits pre-rendered section HTML the host
injected under a reserved context key; \`{sections "header"}\` does the same for a
named group, which is how global chrome lives in a layout. \`{blocks}\` renders
the child blocks of the nearest enclosing section or block, each from its own
\`blocks/<type>.slurp\` file.

## \`{debug expr}\`, \`{redirect}\`, \`{next}\`

\`{debug}\` renders a pretty-printed value in DEVELOPMENT mode and is stripped
with no trace in production, which is what \`slurp build\` always uses.

\`{redirect}\` and \`{next}\` are middleware-only signals that render nothing.
Using either outside middleware is a compile error.`

const EXPRESSIONS = `# Expressions

## Precedence, loosest first

| # | Construct | Associativity |
|---|---|---|
| 1 | \`\\|\` filter application | left, and LOOSEST of all |
| 2 | \`? :\` ternary | RIGHT |
| 3 | \`\\|\\|\` | left |
| 4 | \`&&\` | left |
| 5 | \`==\` \`!=\` | left |
| 6 | \`<\` \`>\` \`<=\` \`>=\` | left |
| 7 | \`+\` \`-\` | left |
| 8 | \`*\` \`/\` \`%\` | left |
| 9 | \`!x\` \`-x\` prefix | right |
| 10 | \`f(...)\` \`x[...]\` postfix | left |
| 11 | literals, paths, \`(...)\`, template literals | tightest |

FILTERS BIND LOOSEST. \`\${ a ? "yes" : "no" | upper }\` applies \`upper\` to the
whole ternary result, not to \`"no"\`. Parenthesise if you meant otherwise.

A filter cannot be followed by a binary operator: \`\${ a | upper + b }\` is a
parse error. Parenthesise instead.

All binary operators are left-associative, including the comparisons, so
\`a < b < c\` parses as \`(a < b) < c\`.

## Operators that do NOT exist

\`===\` \`!==\` \`??\` \`?.\` \`in\` \`**\` \`typeof\` \`instanceof\`, every bitwise
operator, every assignment operator, and arrow functions. Writing any of them is
a parse or lex error, not a silent no-op.

There is no \`?.\` because none is needed: \`.\` is ALWAYS null-safe.
For a null fallback use \`| default(...)\` rather than \`??\`.

## Operator semantics

| Op | Behaviour |
|---|---|
| \`+\` | STRING CONCATENATION if either side is a string, otherwise numeric. \`"a" + 1\` is \`a1\`. |
| \`-\` \`*\` | Numeric. null, arrays, objects and non-numeric strings all coerce to 0. |
| \`/\` \`%\` | Numeric, and DIVISION BY ZERO YIELDS 0, not an error and not Infinity. |
| \`==\` \`!=\` | Type-strict deep equality. \`1 == "1"\` is false; \`0 == false\` is false. Numbers use an epsilon, so \`0.1 + 0.2 == 0.3\` is true. Arrays and objects compare structurally. |
| \`<\` \`>\` \`<=\` \`>=\` | Only number-to-number and string-to-string actually compare. EVERY other pairing compares Equal, so \`>\` is false and \`>=\` is true. No coercion, so \`"10" > 9\` is false. |
| \`&&\` \`\\|\\|\` | Short-circuit and return THE OPERAND VALUE, JavaScript-style, not a coerced boolean. |
| \`!x\` | Always a real boolean. |
| \`-x\` | Only a NUMBER is negated. \`-"5"\` is 0, which disagrees with \`0 - "5"\` being -5. |

Any arithmetic result that is NaN or infinite silently becomes 0.

## Literals

| Form | Notes |
|---|---|
| \`"str"\` / \`'str'\` | Escapes: \`\\n \\t \\r \\\\ \\' \\" \\0 \\uXXXX\`. An unknown escape is preserved as backslash plus the character. |
| \`123\`, \`1.5\`, \`1e3\` | All f64. \`.5\` is NOT valid, a digit must lead. \`1.\` is not a number either. No hex, no separators, no unary plus. |
| \`true\` \`false\` \`null\` | Word-boundary matched. There is no \`undefined\` keyword. |
| \`\\\`text \${expr}\\\`\` | Template literal. Escapes are a SMALLER set than in a quoted string: \`\\n \\t \\r \\\\ \\\` \\$\` and NO \`\\uXXXX\`. |
| \`[1,2]\`, \`{a:1}\` | DO NOT EXIST. Both are parse errors. Pass collections through the render context. |

## Paths

\`\`\`slurp
\${ user.address.city }      {* null-safe at every depth: missing gives "" *}
\${ items[0].title }         {* literal index *}
\${ config["feature-flag"] } {* literal string key *}
\${ items[i] }               {* dynamic index: WORKS *}
\`\`\`

Navigation is null-safe at every depth, so a missing intermediate yields the
empty string rather than an error.

A dynamic index WORKS. (OBSERVED: with \`items = ["a","b","c"]\` and \`i = 1\`,
\`\${ items[i] }\` renders \`b\`, including when \`i\` is an \`{each}\` index.)
Older notes claim dynamic indexes always render empty; that was true of an
earlier compiler and is not true now.

After a \`.\` only an identifier is accepted, so \`a.0\` is a parse error and
\`a[0]\` is the only form.

There are NO array properties or methods. \`items.length\` resolves to null and
renders empty. (OBSERVED.) Pass a count through the render context, or use
\`loop.count\` inside the loop.

\`$name\` marks a reactive path for the client runtime. Server-side it resolves
exactly like a plain path.

## Function calls

\`f(a, b)\` PARSES and ALWAYS EVALUATES TO NULL. There are no built-in callable
functions of any kind. (OBSERVED: \`\${ Math.max(1,2) }\` renders the empty
string, with no diagnostic.) The only call-shaped construct in the language is a
filter.

## Scoping

The page context is the root scope. \`{each}\` and \`{fetch}\` create child scopes,
and a child shadows its parent.

Components and layouts render in their OWN root scope: the page globals, overlaid
by whatever was passed explicitly, with the explicit values winning on collision.
So a component CAN read a page global it was never passed. What it cannot see is
the caller's local bindings, which notably means an \`{each}\` loop variable and
\`loop.index\` are invisible inside a component unless passed as props.`

const COMPONENTS = `# Elements, attributes and components

## What makes a component

A tag is a COMPONENT if its first character is uppercase. Everything else is a
plain HTML element. There is no registration step beyond a \`using\` import.

## THE ASYMMETRY THAT CATCHES EVERYONE

\`\`\`slurp
<span data-x="Hi \${name}"></span>   {* ELEMENT attribute: interpolates. Renders "Hi World" *}
<Card title="Hi \${name}" />        {* COMPONENT prop: LITERAL. Renders "Hi \${name}" *}
<Card title={\\\`Hi \${name}\\\`} />      {* COMPONENT prop, correct: renders "Hi World" *}
\`\`\`

(OBSERVED, all three.) A quoted element attribute is re-lexed as a template
literal and its slots are evaluated. A quoted component prop is not: the
\`\${...}\` characters are carried through as literal text. Nothing is reported.

If you write one thing from this page down, write this one.

## Element attribute forms

| Form | Meaning |
|---|---|
| \`attr\` | Boolean attribute, emitted bare. |
| \`attr="text"\` | Literal, and \`\${...}\` inside IS interpolated. |
| \`attr={expr}\` | Expression. An empty result OMITS the attribute entirely. |
| \`attr={\\\`tpl \${x}\\\`}\` | Template literal. |
| \`class={"a": cond, "b": true}\` | Conditional class list, joined with spaces. |
| \`on:click="handler"\` | NOT A BINDING. Emitted as an inert literal attribute, and never interpolated. See below. |
| \`{html expr}\` | Raw attribute spread, sanitized. |

### \`on:\` binds nothing

\`on:event="handler"\` parses and is emitted verbatim. \`<button on:click="go()">\`
compiles to \`<button on:click=go()>\`, which is not an Alpine directive
(\`@click\` / \`x-on:click\`) and not a DOM handler, so nothing runs. Neither the
browser nor \`@bytesell/slurp-runtime\` looks at it.

It is also NOT interpolated, unlike every other quoted element attribute:
\`on:click="f(\${item.id})"\` emits the literal text \`\${item.id}\` into the DOM.

To bind a handler, write the Alpine attribute you want:

\`\`\`slurp
<button @click="addToCart(1)">Add to cart</button>
\`\`\`

## Component prop forms

| Form | Result |
|---|---|
| \`prop={expr}\` | The evaluated expression. This is the form you want. |
| \`prop="text"\` | A LITERAL string. Never interpolated. |
| \`prop={\\\`text \${x}\\\`}\` | Template literal. The correct way to interpolate. |
| \`prop\` | Boolean \`true\`. |
| \`prop=123\` | TWO boolean props. An unquoted number lexes as an attribute NAME, so you get \`prop = true\` and a second prop literally named \`123\`. Write \`prop={123}\`. |
| \`...spread\` | A PARSE ERROR. Both \`<Card ...cardProps />\` and \`<Card ...spread={cardProps} />\` fail with \`error[UnexpectedToken]\`. Pass props explicitly. |

An unresolvable component does NOT fail the build. It renders a placeholder:

\`\`\`html
<div data-slurp-component="Card" data-slurp-props="{...}">children</div>
\`\`\`

which is why a broken import can look like a clean build with a missing card.

## URL attributes are scheme-checked

\`href src xlink:href poster formaction action cite background data ping srcset\`,
plus their \`:href\` and \`x-bind:href\` forms, are checked. A \`javascript:\`,
\`data:\` or \`vbscript:\` value collapses to an EMPTY value; the attribute is not
dropped. Detection first strips control characters, Unicode whitespace,
zero-width and bidi characters, so obfuscation does not get past it.

## A nested double quote inside \`\${ }\` ends the attribute

\`\`\`slurp
<div title="\${ a["k"] }">   {* BROKEN: renders <div title=" a[" k> *}
<div title={ a["k"] }>      {* correct *}
<div title="\${ a['k'] }">   {* also correct *}
\`\`\`

(OBSERVED.) The lexer closes an attribute at its first matching quote, before
interpolation is ever considered. There is no diagnostic.

## \`<Image>\`

\`\`\`slurp
<Image src={product.image} alt={product.name} width={400} height={300} priority />
\`\`\`

Emits a \`<picture>\` with AVIF, WebP and JPEG sources. \`src\` is required and
omitting it is the only error this component raises. \`width\` defaults to 800
and \`height\` to 600; both accept a literal, contrary to older notes claiming a
literal is ignored. (OBSERVED: \`width={400}\` produces \`w=400\`.) \`priority\`
switches loading from lazy to eager. \`height\` is only used for the \`<img>\`
attribute and is never sent to the image service.`

const SCRIPTS = `# Script bodies and JavaScript attributes

This is the part of Slurp with the strictest rules, and the reason is worth one
paragraph. A \`<script>\` body is a RAW TEXT element: the HTML parser hands it to
the JavaScript engine WITHOUT decoding character references. So the entity
escaping that makes \`\${ }\` safe in ordinary text is completely inert there,
and every JavaScript metacharacter except \`<\` passes through untouched. The
compiler therefore refuses to guess.

## Rule 1: every \`\${ }\` in a \`<script>\` body must declare its escaping

\`\`\`slurp
<script>var n = \${ user.name };</script>          {* ERROR *}
<script>var n = \${ user.name | json };</script>   {* correct: emits "Bo" with quotes *}
<script>var n = '\${ user.name | js }';</script>   {* correct: you supply the quotes *}
\`\`\`

Only \`| js\` and \`| json\` satisfy the rule. \`| unsafe_js\` deliberately does
NOT; the raw hatch inside a script body is \`{html expr}\`.

The difference between them: \`| js\` escapes the value for placement inside a
string literal YOU wrote, and adds no quotes. \`| json\` emits a complete,
self-delimiting JSON literal, so a string arrives with its quotes.
(OBSERVED: with \`name = "Bo"\`, \`| js\` gives \`Bo\` and \`| json\` gives \`"Bo"\`.)

## Rule 2: a JS template literal cannot be written in a \`<script>\` body at all

Slurp claims the \`\${\` sequence itself, so there is no way to write a backtick
template in a script body. Build the string by concatenation instead.

## Rule 3: in a JavaScript-evaluated ATTRIBUTE, position decides the filter

An attribute counts as JavaScript-evaluated if it starts with \`@\` or \`x-on:\`,
starts with \`x-bind:\` or \`:\`, is longer than two characters and starts with
\`on\`, or is one of \`x-data x-init x-effect x-show x-text x-html x-if x-for
x-model\`.

Inside such an attribute the compiler tracks whether each slot sits inside a
quote YOU opened:

\`\`\`slurp
<button @click="add('\${ id | js }')">   {* STRING position: | js is correct *}
<button @click="add(\${ id | json })">   {* STATEMENT position: | json is correct *}
<button @click="add(\${ id | js })">     {* ERROR: a false claim about position *}
\`\`\`

(OBSERVED: the third form is refused with UnsafeScriptInterpolation.)

Why the third is an error rather than merely redundant: \`| js\` is a CLAIM that
the slot is inside a string literal, and a declared slot is then passed through
VERBATIM. Outside a literal that turns the auto-escaper off and puts nothing in
its place, and escaping quote characters cannot contain a value sitting next to
a \`;\` or a \`(\`.

An UNDECLARED slot in statement position is not an error. It is auto-encoded as
a JSON literal, which is a complete answer, and development mode records an
advisory warning suggesting you declare it.

## Rule 4: a backtick template in a quoted attribute is a warning

\`\`\`slurp
<a :href="\\\`/product/\${item.slug}\\\`">   {* WARNING: renders EMPTY *}
<a :href="'/product/' + item.slug">    {* correct *}
\`\`\`

The template engine consumes the \`\${ }\` slots itself and evaluates them against
the SERVER render context, not the browser's, so a client-side variable renders
EMPTY with no runtime error and no other sign of failure. This is the
\`JsTemplateLiteralInAttribute\` warning. It has a dedicated code because of how
it fails: the page still builds and still looks built, while carrying dead links
and unstyled elements wherever a slot was silently emptied.

## Rule 5: a \`style\` value slot loses CSS-structural characters

Semicolons, parentheses, braces, quotes and \`@\` are stripped from a slot
interpolated into a style attribute. Development mode records a warning saying
so; production mode strips silently.`

const FRONTMATTER = `# Frontmatter

Delimited by \`---\` at column 1. Every directive is optional.

UNKNOWN STATEMENTS ARE SILENTLY SKIPPED. A misspelled directive produces no
diagnostic at all, which is why a \`layout:\` line (there is no such directive)
appears to do nothing rather than to fail.

| Directive | Syntax |
|---|---|
| \`using\` | \`using "@components/Card"\` or \`using "./Card" as Renamed\` |
| \`using type\` | \`using type { Post, Author } from "@types/content"\` (tooling only, no render effect) |
| \`type\` | \`type Post { id: string  views?: number }\` (stored as raw strings, NEVER enforced) |
| \`props\` | \`props { title: string  count?: number  featured: bool = false }\` |
| \`middleware\` | \`middleware "auth"\` or \`middleware auth\` |
| \`section { }\` | see the \`schema\` topic |
| \`block { }\` | see the \`schema\` topic |

## props

ONE DECLARATION PER LINE. (OBSERVED: putting three on one line parses as a
single prop whose type expression swallows the rest.)

\`\`\`slurp
---
props {
  title: string
  count?: number
  featured: bool = false
}
---
\`\`\`

\`?\` marks a prop optional. A \`= default\` is supported and is evaluated in the
CALLER's scope. Prop types are recorded and NEVER validated at render time, so a
missing required prop is simply null.

## Import names

The local name is the alias if one is given, otherwise the last path segment. So
\`using "@components/Card"\` introduces \`Card\`.

## middleware

This directive only RECORDS a name. Whether a file is compiled as middleware is a
caller-supplied compile option, not a filename or frontmatter convention. The
CLI and this MCP server both treat every file as non-middleware unless told
otherwise, which is why \`request.*\` reports a scope violation by default.`

const SCHEMA = `# Section and block schemas

A section or block declares its own editor surface in frontmatter. That
declaration is the contract between the template and whatever renders a settings
form over it, which is how a non-technical person edits a page safely.

\`\`\`slurp
---
section {
  name: "Hero"
  settings {
    heading:    text = "Welcome" { label: "Heading" }
    background: color = "#0a0a0a"
    align:      select(left, center, right) = center
    max_items:  number = 8 { min: 1, max: 24 }
    show_badge: toggle = true
    blurb:      richtext
    logo:       image
    cta:        link
    marker:     icon
  }
  blocks {
    @theme(heading, text, button)
    faq_item { max: 12 settings { question: text  answer: richtext } }
  }
  max_per_page: 3
}
---
<h1>\${ section.settings.heading }</h1>
{blocks}
\`\`\`

## The nine setting kinds

| Kind | Default when unset | Validation when merging saved state |
|---|---|---|
| \`text\` | \`""\` | must be a string |
| \`richtext\` | \`""\` | string, run through the sanitizer allowlist |
| \`color\` | \`""\` | \`#hex\` of 3, 4, 6 or 8 digits, or a named CSS colour; otherwise the default is kept |
| \`image\` | \`""\` | string, URL-scheme filtered |
| \`icon\` | \`""\` | \`[a-z0-9-]+\` only |
| \`link\` | \`""\` | relative, anchor and query URLs kept; an absolute one must be http, https, mailto or tel, else it collapses to \`""\` |
| \`select(a, b, c)\` | the FIRST option | must be exactly one of the options |
| \`number\` | \`0\` | must be numeric, and is CLAMPED to \`meta.min\` and \`meta.max\` |
| \`toggle\` | \`false\` | must be a boolean |

Options in \`select(...)\` may be identifiers OR string literals. A common claim
that they must be identifiers is false.

Each setting takes an optional \`= default\` and an optional trailing
\`{ meta }\` object of arbitrary keys, of which \`label\`, \`min\` and \`max\`
have meaning.

## section vs block

\`block { }\` is the same grammar minus \`max_per_page\`, and it does not require
a \`name\`. \`section { }\` does require one.

A BLOCK'S TYPE IS ITS FILE NAME, not the \`name\` it declares.
\`blocks/social_row.slurp\` is the type \`social_row\` whatever its \`name:\`
says. The name is only a human label for an editor.

## @theme targeting

Inside a \`blocks { }\` group, a bare \`@theme\` accepts every theme block and
\`@theme(a, b)\` whitelists specific ones.

A BARE \`@theme\` ALWAYS WINS over a whitelist regardless of the order they
appear in. Widening is never silently narrowed.

An INLINE block definition always beats the shared theme catalog, so adding to
the palette can never change an existing section.

## Merge semantics: deliberately tolerant

Saved editor state is merged over the schema so a theme upgrade can never break
it. Unknown setting keys are dropped, type mismatches keep the default, numbers
are clamped, unknown block types are dropped, per-type block counts are capped,
and nesting is bounded.

THE MOST SURPRISING CONSEQUENCE: without a block catalog, a \`@theme\`-targeted
block is an unknown type and is DROPPED FROM THE RENDER while surviving intact
in storage. The page renders as though nothing was added.`

const LIMITS = `# Limits

Exceeding a budget TRUNCATES the output. It never aborts the render, so a page
over budget still ships, quietly shortened.

EVERY budget records a diagnostic, in both modes. What differs is whether the
build FAILS: the two filter budgets are errors, everything else is a warning and
the build still exits 0. So a 1500-item loop emits 1000 items, warns, and
succeeds. On the CLI, \`-v\` is what prints warnings; without it the console is
quiet even though the diagnostic exists.

| Limit | Value |
|---|---|
| Parser nesting depth | 64 |
| Render depth | 96 |
| Iterations per loop | 1000 |
| Total iterations per render | 1000000 |
| Output size | 16 MiB |
| Single filter value | 8 MiB |
| Cumulative filter work | 64 MiB |
| Security walk depth | 512 |
| Lexer token count | 2000000 |
| Lexer mode-stack depth | 512 |
| \`{html ... sanitize}\` keyword lookahead | 4096 characters |
| Import chain depth | 128 |
| Block merge depth | 64 |
| Blocks per type with no declared max | 200 |
| \`fixed(n)\` precision | 100 |
| Accumulated diagnostics | 1000, then further ones are dropped |

The three memory caps are tunable per host; the rest are compiled in.

CRITICAL FOR AUTHORING: the CSS-strip and undeclared-JS-slot advisories are
recorded in DEVELOPMENT mode ONLY, so a production render applies both and says
nothing. Loop truncation is NOT one of these - it warns in both modes. The
\`slurp_render\` tool here uses development mode so the dev-only pair is visible.`

function renderErrors(): string {
  const section = (title: string, list: typeof ERROR_CODES): string => {
    if (list.length === 0) return ''
    const body = list
      .map((e) => {
        const lines = [`### \`${e.code}\``]
        if (e.cause) lines.push('', e.cause)
        if (e.fix) lines.push('', `FIX: ${e.fix}`)
        if (e.wrong) lines.push('', 'Wrong:', '```slurp', e.wrong, '```')
        if (e.right) lines.push('', 'Right:', '```slurp', e.right, '```')
        if (e.note) lines.push('', `NOTE: ${e.note}`)
        return lines.join('\n')
      })
      .join('\n\n')
    return `## ${title}\n\n${body}`
  }

  return [
    '# Error codes',
    '',
    `The compiler declares ${ERROR_CODES.length} codes and EMITS ${EMITTED_ERROR_CODES.length} of them. The remaining ${DECLARED_ONLY_ERROR_CODES.length} are declared for host integrations that add their own checks; the compiler will never produce them, so do not go hunting for the condition.`,
    '',
    'Every diagnostic carries a code, a message, a file, a line, a column and a severity, so they are machine-consumable rather than prose to be pattern-matched.',
    '',
    section('Codes the compiler emits', EMITTED_ERROR_CODES),
    '',
    section('Codes declared but never emitted by the compiler', DECLARED_ONLY_ERROR_CODES),
  ].join('\n')
}

function renderGotchas(): string {
  const body = GOTCHAS.map((g, i) => {
    const lines = [`## ${i + 1}. ${g.title}`, '', g.explanation]
    if (g.wrong) lines.push('', 'Wrong:', '```slurp', g.wrong, '```')
    if (g.right) lines.push('', 'Right:', '```slurp', g.right, '```')
    lines.push('', `Detected by: ${g.detectedBy}`)
    return lines.join('\n')
  }).join('\n\n')

  return [
    '# Gotchas',
    '',
    'Every entry here has cost a human real time on a real project. They are ordered by how often they bite, and each says whether a tool will catch it for you or whether nothing will.',
    '',
    body,
  ].join('\n')
}

function renderFilters(): string {
  return [
    '# Filters',
    '',
    `There are ${VALUE_FILTER_COUNT} value filters and ${LOOP_FILTER_COUNT} loop filters. THE TWO TABLES ARE DISJOINT: using one where the other belongs is an UnknownFilter error.`,
    '',
    'FILTER ARGUMENTS MUST BE LITERALS. A non-literal argument is read as the empty string, so the filter silently falls back to its default or emits nothing, with no diagnostic. `default` is the ONLY filter whose argument is evaluated in scope.',
    '',
    'Chained left to right: `${ name | lower | truncate(20) }`.',
    '',
    'An unknown filter records a diagnostic AND passes the value through unfiltered, so the page renders looking nearly right.',
    '',
    '## Value filters',
    '',
    'Usable anywhere an expression is.',
    '',
    filterTable('value'),
    '',
    '## Loop filters',
    '',
    'Usable ONLY in an `{each}` header. A loop-filter error EMPTIES the collection, which then renders the `{empty}` branch, so a broken loop filter looks like missing data rather than like an error.',
    '',
    filterTable('loop'),
  ].join('\n')
}

export const TOPICS: Topic[] = [
  {
    id: 'overview',
    title: 'Overview',
    summary: 'What Slurp is, a complete example, and the four rules that save the most time.',
    render: () => OVERVIEW,
  },
  {
    id: 'tags',
    title: 'Block tags',
    summary: 'Every block tag with its branches, branch order and closing rules.',
    render: () => TAGS,
  },
  {
    id: 'expressions',
    title: 'Expression grammar',
    summary: 'Precedence, operators that exist and do not, semantics, literals, paths, scoping.',
    render: () => EXPRESSIONS,
  },
  {
    id: 'filters',
    title: 'Filters',
    summary: 'All value and loop filters, with signatures, null behaviour and traps.',
    render: renderFilters,
  },
  {
    id: 'components',
    title: 'Elements, attributes and components',
    summary: 'The literal-prop asymmetry, every attribute and prop form, URL checks, <Image>.',
    render: () => COMPONENTS,
  },
  {
    id: 'scripts',
    title: 'Script bodies and JS attributes',
    summary: 'When | js applies, when | json does, and why position decides.',
    render: () => SCRIPTS,
  },
  {
    id: 'frontmatter',
    title: 'Frontmatter',
    summary: 'Imports, props, types, middleware, and what is silently ignored.',
    render: () => FRONTMATTER,
  },
  {
    id: 'schema',
    title: 'Section and block schemas',
    summary: 'The nine setting kinds, @theme targeting, and the tolerant merge rules.',
    render: () => SCHEMA,
  },
  {
    id: 'errors',
    title: 'Error codes',
    summary: 'All 21 codes, which 13 the compiler emits, and a worked fix for each.',
    render: renderErrors,
  },
  {
    id: 'gotchas',
    title: 'Gotchas',
    summary: 'The silent failures, each with what catches it and what does not.',
    render: renderGotchas,
  },
  {
    id: 'limits',
    title: 'Limits',
    summary: 'Every compiled-in budget, which ones fail a build, and the two advisories that exist only in development mode.',
    render: () => LIMITS,
  },
]

export const TOPIC_IDS = TOPICS.map((t) => t.id)

export function topicById(id: string): Topic | undefined {
  return TOPICS.find((t) => t.id === id)
}
