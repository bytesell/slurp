# Slurp Template Language - Reference Documentation

## Table of Contents

1. [Overview](#1-overview)
2. [Quick Start](#2-quick-start)
3. [Template Syntax](#3-template-syntax)
   - [Text and Comments](#31-text-and-comments)
   - [Expressions](#32-expressions)
   - [Template Literals](#33-template-literals)
   - [Conditionals](#34-conditionals)
   - [Loops](#35-loops)
   - [Match](#36-match)
   - [Repeat](#37-repeat)
   - [Fetch](#38-fetch)
   - [Raw HTML](#39-raw-html)
   - [Client State](#310-client-state)
   - [Try / Error Boundary](#311-try--error-boundary)
   - [Debug](#312-debug)
   - [Redirect and Next (Middleware)](#313-redirect-and-next-middleware)
   - [Sections and Blocks](#314-sections-and-blocks)
4. [HTML Elements and Attributes](#4-html-elements-and-attributes)
5. [Components](#5-components)
6. [Layouts and Slots](#6-layouts-and-slots)
7. [Frontmatter](#7-frontmatter)
8. [Built-in Filters](#8-built-in-filters)
9. [Loop Filters](#9-loop-filters)
10. [Escaping and Security](#10-escaping-and-security)
11. [Error Handling](#11-error-handling)
12. [Configuration](#12-configuration)
13. [Rust API Reference](#13-rust-api-reference)
14. [WASM / TypeScript Integration](#14-wasm--typescript-integration)
15. [Image Optimisation Service](#15-image-optimisation-service)
16. [Limitations and Known Issues](#16-limitations-and-known-issues)

---

## 1. Overview

> **Status: 0.1.0.** This is a reference for what the compiler does today, not
> for what it is meant to do. Where a feature is absent, incomplete or wired
> only halfway, it says so. Anything not described here does not exist.

Slurp is a server-side HTML templating engine written in Rust. Templates are compiled to plain HTML strings before being sent to the browser. An optional TypeScript runtime layers client behaviour on top of the emitted HTML.

**Key properties:**

- Rendering is a **single server-side pass over a JSON context**. There is no reactivity, no function evaluation and no user code execution at render time.
- Expression output is HTML-escaped by default. `{html}` is the only way to emit raw output. See [Section 10](#10-escaping-and-security), including the contexts where that escaping is not sufficient.
- Compile-time security checks gate `env.SLURP_SECRET_*`, `request.*` scope, `{redirect}` and `{next}`. **They run in the `compile*` entry points, in `slurp validate`, and in the WASM export `validate_full`. Every other WASM export skips them.**
- The renderer is **total and tolerant**: missing data renders `""`, budgets truncate, and unknown keys are dropped. Almost nothing raises. Totality is the property that makes rendering untrusted templates in a shared service safe, and it is also why most mistakes are silent rather than loud. The resource budgets record a warning either way, but `slurp build` prints warnings only under `--verbose`, so without `-v` a build that dropped the tail of a list is quiet and still exits 0. See "Resource budgets" in Section 11.
- The compiler is available as a native Rust library, a WASM module, a CLI binary (`slurp`), and a dev server with hot reload.

---

## 2. Quick Start

### CLI

```sh
# Build a directory tree of templates (--input is a FLAG, not a positional)
slurp build --input . --output dist

# Validate without building
slurp validate --dir .

# Start the dev server with hot reload
slurp dev --port 3000

# Write a theme .gitignore
slurp gitignore
```

### Rust library

```rust
use slurp_compiler::{compile_template, CompileOptions};
use serde_json::json;

let options = CompileOptions {
    context_json: Some(json!({"name": "Alice", "count": 42}).to_string()),
    ..Default::default()
};

let html = compile_template("<h1>Hello, ${name}!</h1>", options)?;
// -> "<h1>Hello, Alice!</h1>"
```

### Simple template

```slurp
---
props {
    title: string
    items: any
}
---
<h1>${ title }</h1>
<ul>
  {each item in items}
    <li>${ item.name }</li>
  {empty}
    <li>Nothing here yet.</li>
  {/each}
</ul>
```

---

## 3. Template Syntax

### 3.1 Text and Comments

Any text outside a Slurp tag is emitted verbatim (after HTML escaping of expressions within it).

**Block comments** - stripped from output entirely:

```slurp
{* This is a comment and will not appear in the HTML *}
```

---

### 3.2 Expressions

Expressions interpolate a value into the HTML. All output is **HTML-escaped** automatically.

**Syntax:** `${expr}` or `{expr}` (both forms are accepted)

```slurp
<p>${user.name}</p>
<p>{product.price | currency("USD")}</p>
```

#### Path navigation

Paths use dot notation. Access is **safe**: if any segment is null or undefined, the entire expression produces an empty string instead of an error.

```slurp
${user.address.city}          <!-- "" if address is null -->
${items[0].title}             <!-- index access -->
${config["feature-flag"]}     <!-- bracket notation -->
```

#### Reactive paths

Prefix with `$` to mark a path as client-side reactive. The runtime watches the referenced Alpine.js store and re-renders the expression when it changes.

```slurp
${$cart.total}      <!-- updates live when cart changes -->
${$isOpen}          <!-- watches a reactive boolean -->
```

#### Operators

| Category | Operators |
|---|---|
| Arithmetic | `+` `-` `*` `/` `%` |
| Comparison | `==` `!=` `<` `>` `<=` `>=` |
| Logical | `&&` `\|\|` `!` |
| Unary | `-expr` `!expr` |
| Ternary | `cond ? then : else` |

```slurp
${price * quantity}
${user.isAdmin ? "Admin" : "Guest"}
${count > 0 && !loading}
```

#### Function calls - NOT IMPLEMENTED

**Slurp has no callable functions.** `f(a, b)` parses, and then always evaluates to `null`, which renders as an empty string. There is no `Math`, no `math`, no user-defined functions, and no way to register any. There is no error either, so a call is a silent empty string.

```slurp
${ Math.max(a, b) }   {* parses, renders "" *}
${ formatDate(x) }    {* parses, renders "" *}
```

Compute the value on the server and pass it in through the render context, or use a filter (see [Section 8](#8-built-in-filters)).

#### Operators that do not exist

There is no `===`, `!==`, `??`, `?.`, `in`, and no bitwise operator. `===` is a parse error (`Unexpected token in expression: Equals`); use `==`. Path navigation is null-safe at every depth, so `?.` is not needed and writing it is a parse error.

There are also **no array or object literals**. `[1, 2]` and `{a: 1}` are parse errors. Pass structured data in through the context.

#### Filters

Filters are applied with `|`:

```slurp
${title | upper}
${price | currency("EUR")}
${body | truncate(200, "...")}
${date | date("MMMM DD, YYYY")}
```

Multiple filters chain left-to-right:

```slurp
${name | lower | truncate(20)}
```

See [Section 8](#8-built-in-filters) for the full filter reference.

---

### 3.3 Template Literals

Backtick strings interpolate expressions inline, useful for attribute values:

```slurp
<a href="`/products/${product.slug}`">View</a>
<img alt="`${product.name} photo`">
```

Escape a literal backtick with `\``:

```slurp
{$let label = `Press \` to continue`}
```

Supported escape sequences: `\n` `\t` `\r` `\\` `` \` `` `\$`

---

### 3.4 Conditionals

```slurp
{if user.isLoggedIn}
  <p>Welcome back, ${user.name}!</p>
{else if user.isGuest}
  <p>You are browsing as a guest.</p>
{else}
  <p>Please log in.</p>
{/if}
```

The condition is any expression. Falsy values: `false`, `null`, `0`, `""`, empty array, empty object.

---

### 3.5 Loops

#### Basic `{each}`

```slurp
{each item in products}
  <div class="product">
    <h2>${item.name}</h2>
    <p>${item.price | currency("USD")}</p>
  </div>
{/each}
```

#### With index

**The item name comes first and the index second.**

```slurp
{each item, i in products}
  <p>${i}: ${item.name}</p>
{/each}
```

#### Empty branch

```slurp
{each item in cart.items}
  <li>${item.name}</li>
{empty}
  <li>Your cart is empty.</li>
{/each}
```

#### Loop variables

Inside an `{each}` body, four read-only variables are available:

| Variable | Type | Description |
|---|---|---|
| `loop.index` | number | Zero-based position of the current item |
| `loop.count` | number | **Total number of items in the collection.** It is not `loop.index + 1` |
| `loop.first` | boolean | `true` for the first iteration |
| `loop.last` | boolean | `true` for the last iteration |

```slurp
{each item, i in items}
  {if loop.first}<ul>{/if}
  <li>${ i + 1 } of ${ loop.count }: ${ item.name }</li>
  {if loop.last}</ul>{/if}
{/each}
```

These are resolved by a guarded fast path, so a real `loop` object in your render context is not shadowed by them.

#### Semantics worth knowing

- A **non-array, non-null** collection is wrapped and iterated exactly once.
- `null` counts as empty and selects the `{empty}` branch.
- `{empty}` takes **no closing tag**. `{/empty}` is a parse error.
- Each loop is capped at **1,000 items**; the excess is truncated and a warning is recorded. A render is additionally capped at 1,000,000 iterations in total, which is what bounds nested loops.

#### Loop filters

Loop filters transform the collection before iteration:

```slurp
{each item in products | limit(10) | sort("price") | filter("inStock", true)}
  <li>${item.name}</li>
{/each}
```

See [Section 9](#9-loop-filters) for the full loop filter reference.

---

### 3.6 Match

Pattern-match a value against string, number, or boolean literals. `_` is the wildcard arm.

> **There is no `{case}` tag.** An arm is written `pattern -> body`; `{case "x"}`
> is a parse error (`Expected match pattern, found LBrace`).

```slurp
{match order.status}
  "pending"   -> <span>Pending</span>
  "shipped"   -> <span>Shipped</span>
  "delivered" -> <span class="success">Delivered!</span>
  _           -> <span>Unknown</span>
{/match}
```

Number and boolean patterns:

```slurp
{match response.code}
  200 -> <p>OK</p>
  404 -> <p>Not found</p>
  _   -> <p>Error ${response.code}</p>
{/match}

{match feature.enabled}
  true  -> <p>Feature is on</p>
  false -> <p>Feature is off</p>
{/match}
```

Arms are self-delimiting: an arm body ends at the next literal or `_` token, so no closing tag is needed per arm.

Matching is **type-strict**. A `"1"` pattern never matches a numeric `1`. If no arm matches and there is no `_` arm, the block renders nothing.

Inside `{match}` the lexer treats bare quotes, digits, `->` and `_` as tokens rather than text. A stray quote inside an arm body can therefore misparse, even though the same quote is harmless elsewhere in a template.

---

### 3.7 Repeat

Renders its body a fixed number of times. Useful for skeleton loaders and placeholder UIs.

```slurp
{repeat 3}
  <div class="skeleton-card"></div>
{/repeat}
```

The count can be an expression:

```slurp
{repeat config.columnCount}
  <col>
{/repeat}
```

Float counts are truncated to an integer. A count of `0` renders nothing.

---

### 3.8 Fetch

> **`{fetch}` is server-side, and the compiler never performs a network
> request.** It reads `name` out of the render context and picks a branch. The
> HOST is responsible for populating that key before rendering; `eval_fetch_urls`
> exists so a host can discover which URLs a page wants. The optional client
> runtime is what performs live refetching, polling and pagination on top of the
> rendered markup.

Declares a named piece of data and renders one of several branches depending on what the context holds for it.

#### Minimal form

```slurp
{fetch products from "/api/products"}
  {each item in products}
    <li>${item.name}</li>
  {/each}
{/fetch}
```

#### With all branches

```slurp
{fetch post from "/api/posts/${slug}"}
  <article>
    <h1>${post.title}</h1>
    <div>{html post.body sanitize}</div>
  </article>
{loading}
  <p>Loading...</p>
{error}
  <p>Failed to load post.</p>
{empty}
  <p>Post not found.</p>
{/fetch}
```

**Branch order is fixed: body, then `{loading}`, then `{error}`, then `{empty}`.** Branches written out of order are not recovered. None of them takes a closing tag: `{/loading}`, `{/error}` and `{/empty}` are all parse errors, and each branch runs to the enclosing `{/fetch}`.

Which branch the server renders:

| Context value at `name` | Branch rendered |
|---|---|
| absent or `null` | `{loading}`, or nothing if there is none |
| an object containing the key `__error` | `{error}`, with `error` bound to that value |
| an empty array | `{empty}`, or nothing if there is none |
| anything else | the body, with `name` bound to it |

The `__error` key is the convention a host uses to report a failure; the dev server does this for a failed proxy or a missing fixture. `error.message` in the `{error}` branch works because the host put a `message` field there, not because slurp defines one.

The type hint (`: Type` and `[]`) is recorded in the AST and **has no effect on rendering**. Nothing is validated against it.

#### Options

Options appear after the URL expression. **Only the parenthesised call form parses.** A colon form such as `cache:300`, and any unrecognised option, is **silently dropped** with no error, so check this table before copying an example from elsewhere.

| Option | Example | Description |
|---|---|---|
| `cache(n)` | `cache(300)` | Cache response for `n` seconds |
| `retry(n)` | `retry(3)` | Retry failed requests up to `n` times |
| `timeout(n)` | `timeout(5000)` | Abort after `n` milliseconds |
| `abort-on-navigate` | `abort-on-navigate` | Cancel in-flight requests on page navigation. Also spelled `abortOnNavigate` |
| `poll(n)` | `poll(30)` | Re-fetch every `n` seconds |
| `paginate(n)` | `paginate(20)` | Enable pagination with `n` items per page |
| `infinite(n)` | `infinite(20)` | Enable infinite scroll with `n` items per page |

```slurp
{fetch orders from "/api/orders" cache(60) retry(2) poll(30)}
  {each order in orders}
    <tr><td>${order.id}</td><td>${order.total | currency}</td></tr>
  {/each}
{/fetch}
```

#### Array vs object fetch

The type hint `[]` tells the compiler the response is an array:

```slurp
{fetch products[] from "/api/products"}
  ...
{/fetch}
```

Without `[]`, the response is treated as a single object.

#### Named type hint

```slurp
{fetch product: Product from "/api/products/${id}"}
  ...
{/fetch}
```

---

### 3.9 Raw HTML

Emits a value as **raw, unescaped HTML**. Use only with fully-trusted content.

```slurp
{html richTextContent}
```

#### Sanitized raw HTML

Runs the content through an `ammonia`-based allowlist sanitizer before emitting. Safe for user-provided content that may contain markup.

```slurp
{html comment.body sanitize}
```

**Allowlisted tags:** `p`, `br`, `hr`, `span`, `div`, `section`, `article`, `aside`, `header`, `footer`, `main`, `nav`, `h1`-`h6`, `ul`, `ol`, `li`, `dl`, `dt`, `dd`, `a`, `strong`, `em`, `b`, `i`, `u`, `s`, `strike`, `del`, `ins`, `mark`, `sup`, `sub`, `small`, `abbr`, `cite`, `code`, `pre`, `kbd`, `samp`, `var`, `q`, `blockquote`, `img`, `figure`, `figcaption`, `picture`, `source`, `table`, `thead`, `tbody`, `tfoot`, `tr`, `th`, `td`, `caption`, `colgroup`, `col`, `time`, `data`, `address`, `details`, `summary`

Everything not on that list is removed, so `<script>`, `<iframe>`, `<object>`, `<embed>`, `<form>`, `<input>`, `<style>` and the rest are stripped by absence rather than as special cases. Attributes work the same way: only an explicit allowlist survives, which is what removes every `on*` handler. URL schemes are limited to `http`, `https` and `mailto`. Any `<a>` with a `target` gets `rel="noopener noreferrer"` added.

The full attribute list and the tracking caveat about allowed `img`/`srcset` are in [docs/security-model.md](./docs/security-model.md).

#### `{html}` in attribute position

`{html expr}` also works as an attribute spread, including a conditional form:

```slurp
<input {html attrs} {if isRequired}{html "required"}{/if} />
```

This path is sanitized differently from the element form: the spread is re-parsed into whole attributes, and any attribute with an unsafe name (`on*`, `:onclick`, `x-bind:onclick`, `x-html`, `x-data`, `x-init`, `x-effect`) or a script-scheme value is dropped.

---

### 3.10 Client State

`{$let}` seeds a client-side value. Server-side it emits **only a JSON payload**; it does not create a template binding you can read on the same page.

```slurp
{$let count = 0}
{$let user = currentUser}
```

Emits:

```html
<script type="application/json" data-slurp-let="count">0.0</script>
```

Note `0.0`: a numeric literal round-trips through f64, so it comes back out as a float.

**Object and array literals are not valid values** here, because slurp has no such literals. `{$let config = {"theme": "dark"}}` is a parse error. Pass the object in through the render context and reference it by name.

**Security:** the payload is escaped with a script-aware escaper. Every `<` becomes `<`, which kills both a `</script>` breakout and an `<!--` comment breakout, and U+2028/U+2029 are escaped as well.

> **Nothing in this repository reads `data-slurp-let`.** The serialiser exists;
> the hydrator does not. Until an embedding host supplies one, `{$let}` emits a
> payload that nothing consumes.

---

### 3.11 Try / Error Boundary

Wraps client-side code in a try/error boundary. If an error occurs during client-side rendering or a `{fetch}` inside the body throws, the `{error}` branch is shown instead.

```slurp
{try}
  <ShoppingCart items={$cart.items} />
{error}
  <p>Something went wrong. <a href="/">Reload</a></p>
{/try}
```

The `{error}` branch is **client-side only** - the server renders the `{try}` body unconditionally.

---

### 3.12 Debug

Emits a visible debug representation of a value. Stripped in `BuildMode::Production` (rendered as an empty string).

```slurp
{debug request.headers}
{debug user}
```

In development mode, the value is pretty-printed into a visible element (not an HTML comment):

```html
<pre data-slurp-debug>{
  "id": 1,
  "name": "Alice"
}</pre>
```

`slurp build` always renders in Production mode, so `{debug}` never appears in a build.

---

### 3.13 Redirect and Next (Middleware)

These nodes are only valid inside middleware files. Both render to the empty string; they are signals for the host runtime, and slurp does not act on them itself. Using either outside a middleware file is a compile error (`RedirectOutsideMiddleware`).

**"Middleware" is a compile option, not a path convention.** It is `CompileOptions.is_middleware`, supplied by whoever invokes the compiler. Putting a file in a `middleware/` directory does nothing on its own.

#### `{redirect}`

Signals the runtime to redirect to another URL. No HTML is output.

```slurp
{if !request.user}
  {redirect "/login"}
{/if}
```

#### `{next}`

Passes control to the next middleware in the chain. The request continues processing.

```slurp
{if request.headers["x-api-key"] == env.API_KEY}
  {next}
{else}
  {redirect "/unauthorized"}
{/if}
```

---

### 3.14 Sections and Blocks

These two tags are the host side of the editor-schema system declared by `section { }` and `block { }` frontmatter (see [Section 7](#7-frontmatter)). Both are void tags with no closing form.

#### `{sections}` and `{sections "group"}`

A host pre-renders a page's ordered section list and injects the resulting HTML under a reserved context key; this tag emits it raw.

- `{sections}` reads `__slurp_sections_html`.
- `{sections "header"}` reads `__slurp_section_group_header_html`, which is how a layout hosts global chrome such as a header or footer.

The page-level key is stripped when entering a component or layout scope, so a stray `{sections}` inside a component cannot duplicate the page. Group keys are not stripped, so layouts can host groups. `${sections}` with a dollar sign remains an ordinary path expression and has nothing to do with this tag.

#### `{blocks}`

Renders the child blocks of the nearest enclosing section or block, each from its own `blocks/<type>.slurp` file. It prefers an enclosing `block` over an enclosing `section` when resolving the list, which is what makes nesting work.

```slurp
<div class="hero">
  <h1>${ section.settings.heading }</h1>
  {blocks}
</div>
```

Each child is wrapped so the editor can address it:

```html
<div data-bs-block="{id}" style="display:contents">...</div>
```

Guards: a block type must match `[A-Za-z0-9_-]+`, because it is used as a registry path and arrives from saved editor state; a block that hosts itself is skipped by a cycle guard; and every block is charged against the shared iteration budget.

> **The single most surprising behaviour in this subsystem:** without a block
> catalog, a `@theme`-targeted block is an unknown type and is **dropped from the
> render while surviving in the database**. The page renders as though the block
> was never added, and no error is reported.

---

## 4. HTML Elements and Attributes

Standard HTML tags are written normally. Slurp adds several attribute extensions.

### Dynamic attribute values

```slurp
<div class="${activeClass}" data-id="${item.id}">
```

### Boolean attributes

Attribute with no value - emitted as a bare attribute:

```slurp
<input disabled>
<details open>
```

### Expression attributes

```slurp
<a href="${product.url}" target="_blank">
<img src="${img.src}" alt="${img.alt}" width="${img.width}" height="${img.height}">
```

### Class lists

The `class={...}` syntax conditionally includes class names:

```slurp
<div class={"active": isActive, "featured": product.isFeatured, "sale": product.onSale}>
```

Only class names whose condition is truthy are included. Multiple unconditional classes can be provided:

```slurp
<div class={"btn": true, "btn-primary": isPrimary}>
```

### URL attributes are scheme-checked

Eleven attribute names are checked for a script scheme: `href`, `src`, `xlink:href`, `poster`, `formaction`, `action`, `cite`, `background`, `data`, `ping` and `srcset`, plus their `:` and `x-bind:` forms. A `javascript:`, `data:` or `vbscript:` value collapses to `""`. Detection strips control characters, Unicode whitespace, zero-width and bidi characters first, so obfuscated variants do not slip through.

`srcset` is checked as a single opaque value rather than parsed into its candidate list, so a script scheme in a **later** candidate is not caught. `is_url_attr` records that limitation: candidates are fetched as images, which makes it a resource-load question and not a script-execution one.

### Attribute value forms

| Form | Meaning |
|---|---|
| `attr` | boolean attribute, emitted bare |
| `attr="text"` | literal text; **`${...}` inside IS interpolated** |
| `attr={expr}` | expression; an empty result **omits the attribute entirely** |
| `` attr={`tpl ${x}`} `` | template literal |
| `class={"a": cond, "b": true}` | conditional class list, joined with spaces |
| `{html expr}` | raw attribute spread, see Section 3.9 |

**Gotcha in attribute interpolation:** the lexer ends a double-quoted attribute at its closing quote before interpolation runs, so a nested `"` inside `${...}` terminates the attribute early. Use the expression form instead.

```slurp
<div title="${ a["k"] }">   {* breaks   *}
<div title={ a["k"] }>      {* correct  *}
```

### Event handlers - NOT WIRED

`on:event="handler"` is parsed and emitted as a **literal, inert attribute**. `<button on:click="go()">` compiles to `<button on:click="go()">`, which is neither an Alpine directive (`@click` / `x-on:click`) nor a DOM handler, so nothing runs.

To bind a handler, write the Alpine attribute you actually want:

```slurp
<button @click="addToCart(1)">Add to cart</button>
```

Before putting a value into any Alpine attribute, read the JS-evaluated-attribute warning in [docs/security-model.md](./docs/security-model.md); those attributes are a JavaScript context and `${ }` escaping is not sufficient in them.

### `navigate=false`

Opts a link out of client navigation, so it takes a full page load. All three spellings mean the same thing and all emit `navigate="false"`, which is what the runtime reads (`runtime/src/core/navigation.ts`):

```slurp
<a href="/a" navigate=false>
<a href="/b" navigate="false">
<a href="/c" navigate={false}>
```

**Bare `navigate` is the opposite: it means opted IN.** That is the spelling a page puts on `<body>` to turn client navigation on at all, so a bare `navigate` on an anchor does not opt it out.

The attribute only does anything on a page where client navigation is running, which needs `navigate` on `<body>` and a `[data-slurp-page]` container. The compiler generates neither; a theme or host supplies them. See [Section 14](#14-wasm--typescript-integration).

---

## 5. Components

A tag is a component if it starts with an uppercase letter, or is exactly `Image`. Components are resolved at compile time from imports declared in frontmatter.

```slurp
<ProductCard product={currentProduct} featured />
```

A component that cannot be resolved does not fail the build. It renders a placeholder instead, which is easy to miss:

```html
<div data-slurp-component="Card" data-slurp-props="{...}">children</div>
```

### Props

| Syntax | Result |
|---|---|
| `prop={expr}` | evaluated expression |
| `prop="text"` | **literal string, NOT interpolated** |
| ``prop={`text ${x}`}`` | template literal, the correct way to interpolate |
| `prop` (bare) | boolean `true` |
| `prop=123` | boolean `true`, because an unquoted number lexes as an attribute name |
| `...spread` / `...spread={x}` | **parse error, see below** |

#### The single most important gotcha in the language

**Element attributes interpolate `${...}`. Component props do not.** The same syntax means two different things depending on the tag.

```slurp
<Card title="Hi ${name}" />        {* renders literally: Hi ${name} *}
<Card title={`Hi ${name}`} />      {* renders: Hi World             *}
<span data-x="Hi ${name}"></span>  {* element attrs DO interpolate  *}
```

There is no error or warning for the first line. Use the template-literal form for any prop that needs a value in it.

#### Spread props - NOT IMPLEMENTED

Both `<Card ...spread={cardProps} />` and `<Card ...cardProps />` are **parse errors**. In HTML tag lexing the identifier after the dots lexes as an attribute name, which no expression can start on. Pass props explicitly.

#### Prop defaults

A `props` block may declare a default with `= expr`, and the default is evaluated in the **caller's** scope:

```slurp
props {
  featured: bool = false
  heading: string = "Untitled"
}
```

Prop types are never validated at render time. `MissingRequiredProp`, `UnknownProp` and `TypeMismatch` exist as error codes but the compiler does not emit them.

### Events on components - NOT WIRED

`on:event="handler"` on a component is parsed into the AST and **never read by the renderer**. It has no effect.

### Slots

Component children become the `default` slot content.

```slurp
<Modal title="Confirm">
  <p>Are you sure?</p>
</Modal>
```

**`slot="name"` routing is NOT implemented.** It is parsed into the AST and never read. Only two slots are ever filled anywhere in slurp: `default` and `head`. A `{slot "sidebar"}` in a component or layout renders a literal `<slot name="sidebar">` element into the output.

### `<Image>` built-in component

The `<Image>` component routes through the image optimisation service. It emits a `<picture>` with AVIF, WebP and JPEG sources plus an `<img>` fallback.

```slurp
<Image src={product.imageUrl} alt={product.name} width={800} height={600} />
<Image src="/hero.jpg" alt="Hero" width={1200} height={400} priority />
```

| Prop | Default | Notes |
|---|---|---|
| `src` | none | **Required.** Empty or missing raises `MissingImageSrc` and nothing is rendered |
| `alt` | `""` | attribute-escaped |
| `width` | `800` | used in the service URL and on the `<img>` |
| `height` | `600` | used on the `<img>` only, never sent to the service |
| `priority` | `false` | truthy gives `loading="eager"`, otherwise `loading="lazy"` |

Remember `src="${product.imageUrl}"` is a prop, so it does **not** interpolate. Use `src={product.imageUrl}`.

The emitted service URL has the shape `/_slurp/image?src={urlencoded}&w={width}&f={avif|webp|jpeg}`. There is no `lazy` prop; `priority` is the only loading control.

---

## 6. Layouts and Slots

> **There is no frontmatter `layout` directive.** A page wraps itself in a
> layout using the `{layout}` block or the `<layout>` element, both shown below.
> A `layout "..."` line in frontmatter parses without error and does nothing,
> because frontmatter silently skips tokens it does not recognise, so the page
> renders with no layout applied.

### Defining a layout

**`layouts/base.slurp`:**

```slurp
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <slot name="head"></slot>
</head>
<body>
  <nav><Navbar /></nav>
  <main>
    {slot}
  </main>
  <footer><Footer /></footer>
</body>
</html>
```

**`pages/home.slurp`:**

```slurp
{layout "@layouts/base"}
  {head}<title>Home</title>{/head}

  <h1>Welcome!</h1>
  <p>This content goes into the default slot.</p>
{/layout}
```

A `src` without a leading `@` gets one prepended. A layout that cannot be resolved falls back to an HTML comment wrapper rather than failing the build, and re-entrant layouts are blocked by a rendering stack.

### The two layout forms are NOT equivalent

| Form | `{head}` handling |
|---|---|
| `{layout "@layouts/base"}...{/layout}` | `{head}` children are **extracted** into the layout's `head` slot |
| `<layout src="@layouts/base">...</layout>` | `{head}` is **not** extracted; it renders inline in the body |

Use the block form whenever a page contributes to `<head>`.

### `{slot}` and `{slot "name"}`

`{slot}` marks where the page or component body is inserted. **Only `default` and `head` are ever filled.** Any other slot name renders a literal `<slot name="x">` element into the output.

```slurp
{slot}          {* the page or component body *}
{slot "head"}   {* content from a {head} block *}
```

**Fallback content needs the element form.** `{slot}fallback{/slot}` is a parse error; the block form takes no children.

```slurp
<slot>Nothing provided.</slot>
<slot name="head"></slot>
```

### `{head}` block

Contributes content to the layout's `head` slot. It is only meaningful as a direct child of a `{layout}` block; anywhere else its children simply render inline.

```slurp
{head}
  <link rel="stylesheet" href="/page.css">
  <meta name="description" content="${meta.description}">
{/head}
```

---

## 7. Frontmatter

Frontmatter appears between `---` delimiters at the top of a template file. All frontmatter fields are optional.

```slurp
---
using "@components/Navbar"
using "@components/ProductCard"
using type { Product, Review } from "@types/shop"

type Address {
    street: String
    city: String
    zip: String
}

props {
    product: Product
    reviews: Review[]
    currentUser: Object?
}

middleware "auth/require-login"
---
```

> **Unknown frontmatter tokens are skipped silently.** A misspelled directive,
> or one that does not exist such as `layout`, produces no error and no warning.
> It simply does nothing. Check this section before assuming a directive is
> taking effect.

### `using` - imports

Import a component file:

```slurp
using "@components/Button"
using "../../shared/Footer"
```

Import type definitions only (no component):

```slurp
using type { Product } from "@types/product"
using type { User, Role } from "@types/auth"
```

### `type` - local type definitions

Define a shape for use in `props` or type hints:

```slurp
type CartItem {
    id: String
    name: String
    price: Float
    quantity: Int
}
```

### `props` - component props contract

Declare the expected incoming props. Required props have no `?`; optional ones do.

```slurp
props {
    title: String
    count: Int
    isAdmin: Bool?
    config: Object?
}
```

Prop types: `String`, `Int`, `Float`, `Bool`, `Object`, `Array`, `Any`, or any user-defined type name. Append `[]` for arrays of a type.

### `middleware` - attach a middleware file

Both the quoted and the bare form work:

```slurp
middleware "auth/session-check"
middleware auth
```

This only marks intent. What actually makes a file middleware is `CompileOptions.is_middleware`, set by whoever calls the compiler.

### `section { }` and `block { }` - editor schemas

A template can declare an editable surface for a visual editor. This is a large subsystem: nine setting kinds (`text`, `richtext`, `color`, `image`, `icon`, `link`, `select(...)`, `number`, `toggle`), nested block definitions, `@theme` block targeting, and tolerant merge rules that clamp and drop rather than reject so that saved editor state can survive a template change.

```slurp
---
section {
  name: "Hero"
  settings {
    heading:    text = "Welcome" { label: "Heading" }
    background: color = "#0a0a0a"
    align:      select(left, center, right) = center
    max_items:  number = 8 { min: 1, max: 24 }
    show_badge: toggle = true
  }
  blocks {
    @theme(heading, text, button)
    faq_item { max: 12 settings { question: text  answer: richtext } }
  }
  max_per_page: 3
}
---
<h1>${ section.settings.heading }</h1>
{blocks}
```

`section` and `block` are contextual keywords, special only when immediately followed by `{`, so they remain usable as prop and type names. A `block { }` schema lives in `blocks/<name>.slurp` and takes the same grammar minus `max_per_page`; **a block's type is its file name**, not its declared `name`.

### `layout` - NOT A DIRECTIVE

There is no frontmatter `layout` directive. See [Section 6](#6-layouts-and-slots).

---

## 8. Built-in Filters

Filters transform expression values before output. Apply with `|`:

```slurp
${value | filterName}
${value | filterName(arg1, arg2)}
```

Before the table:

> **Filter arguments must be literals.** A filter argument is read only as a
> string, number or boolean literal; anything else yields `""` with no error.
> `${ 1234.5 | currency(cur) }` with `cur = "EUR"` renders `" 1,234.50"`, silently
> without a symbol. The **only** exception is `default`, whose argument the
> renderer evaluates in scope.

> **Filters bind loosest of all operators.** `${ a ? "yes" : "no" | upper }`
> applies `upper` to the ternary result, not to `"no"`. Parenthesise if you meant
> otherwise.

An unknown filter name **fails the build**.

### `upper`

Convert string to uppercase.

```slurp
${"hello" | upper}   -> "HELLO"
```

### `lower`

Convert string to lowercase.

```slurp
${"WORLD" | lower}   -> "world"
```

### `currency(code?)`

Format a number as a currency string. Defaults to USD.

Supported codes: `USD` `$`, `EUR` `€`, `GBP` `£`, `JPY` `¥`, `CAD` `CA$`, `AUD` `AU$`, `CHF` `CHF `. An **unknown code is used as a prefix**, so `${ 10 | currency("SEK") }` renders `SEK 10.00`, not a bare number.

JPY uses 0 decimals; EUR uses `.` for thousands and `,` for decimals; CHF uses `'` for thousands. The symbol always prefixes, and a negative renders as `-$5.00`. Rounding happens in minor units, so `19.999` becomes `$20.00`. A non-numeric value becomes `0`.

```slurp
${1234.56 | currency("USD")}   -> "$1,234.56"
${1000.00 | currency("EUR")}   -> "€1.000,00"
${5000    | currency("JPY")}   -> "¥5,000"
${-50.00  | currency("GBP")}   -> "-£50.00"
```

### `truncate(n, suffix?)`

Truncate a string to at most `n` characters. Appends `suffix` (default: `"..."`) when truncated.

```slurp
${"Hello, world!" | truncate(5)}         -> "Hello..."
${"Hello, world!" | truncate(5, " [more]")} -> "Hello [more]"
${"Hi"            | truncate(100)}       -> "Hi"
```

Character count is Unicode-aware.

### `fixed(n)`

Format a number to `n` decimal places.

```slurp
${3.14159 | fixed(2)}   -> "3.14"
${42      | fixed(2)}   -> "42.00"
```

### `date(format?)`

Format an ISO date string (`YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SSZ`). Default format: `"MMM DD, YYYY"`.

| Token | Example | Description |
|---|---|---|
| `YYYY` | `2024` | Full year |
| `YY` | `24` | Two-digit year |
| `MMMM` | `January` | Full month name |
| `MMM` | `Jan` | Short month name |
| `MM` | `01` | Zero-padded month |
| `M` | `1` | Month without padding |
| `DD` | `05` | Zero-padded day |
| `D` | `5` | Day without padding |

```slurp
${"2024-01-15"          | date}                  -> "Jan 15, 2024"
${"2024-03-05"          | date("MM/DD/YYYY")}     -> "03/05/2024"
${"2024-12-25"          | date("D MMMM YYYY")}    -> "25 December 2024"
```

**There are no time tokens.** The filter splits on `T` or a space, then on `-`. If that yields fewer than three parts the **input is returned unchanged**, so an epoch-millisecond timestamp passes straight through looking like a number. Pass dates as RFC 3339 or `YYYY-MM-DD` strings.

### `default(fallback)`

Return `fallback` when the value is null, an empty string, an empty array, or an empty object. With no argument it yields `""`.

This is the one filter whose argument is evaluated as an expression rather than read as a literal, so `${ x | default(fallbackVar) }` works. Note there are no array literals, so `default([])` is a parse error.

```slurp
${user.bio | default("No bio yet.")}
```

### `plural(singular, plural?)`

Format a count word. If `plural` is omitted, appends `"s"` to `singular`.

```slurp
${5 | plural("item")}              -> "5 items"
${1 | plural("item")}              -> "1 item"
${3 | plural("child", "children")} -> "3 children"
${0 | plural("result")}            -> "0 results"
```

### `int`

Convert to integer. Truncates floats; parses numeric strings. Returns `0` on failure.

```slurp
${"42.7" | int}   -> 42
${3.9    | int}   -> 3
${null   | int}   -> 0
```

### `float`

Convert to floating-point number. Returns `0.0` on failure.

```slurp
${"3.14" | float}   -> 3.14
${null   | float}   -> 0.0
```

Neither `int` nor `float` does prefix parsing: `"12abc"` is `0`, not `12`.

### `js`

Escape a value for embedding inside a JavaScript **string literal**. It escapes quotes, backtick, backslash, `$`, newlines, U+2028, U+2029, `<` and `/`.

```slurp
<button @click="addToCart('${ id | js }')">Add</button>
<script>var id = "${ id | js }";</script>
```

`js` is a **claim about context**: "this slot sits inside a quoted JS string literal". A slot that declares it is emitted verbatim, which is exactly right inside a literal and wrong outside one, so the claim is checked:

- In a JS-evaluated attribute, a `| js` slot that is **not** inside a string literal is a compile error (`UnsafeScriptInterpolation`). Quote the slot, or use `json` for a bare value.
- In a `<script>` body, `| js` (or `json`) is **required** - an undeclared interpolation there is a compile error.

You rarely need to write `js` for its escaping alone: in a JS-evaluated attribute the renderer applies the same escaper automatically to an undeclared slot inside a string literal. Writing it silences the development-mode advisory warning and states the intent.

### `json`

Serialise the value as JSON. The result is a self-delimiting JavaScript literal - a string becomes a quoted string, a number stays a bare number, an object or array becomes its literal form - so it cannot contribute structure to the surrounding program whatever it contains.

```slurp
<script>var cfg = ${ config | json };</script>
<div x-data="{ n: ${ count | json } }"></div>
```

This is the filter to reach for when a slot lands in JavaScript **statement or expression** position, where quote escaping is no help. `<` is encoded as `<` on the way out, so the value cannot form `</script>` or `<!--`, and U+2028 / U+2029 are escaped; the output is still valid JSON.

### `unsafe_js`

Emit the value with no escaping at all, into a JavaScript context. It is the declaration "this is JavaScript the **theme itself** wrote", for the case where a theme composes an expression and passes it to a component as a prop:

```slurp
<span x-text="${ valueExpression | unsafe_js }"></span>
```

**Never route data through it.** Escaping theme-written code would corrupt it, which is the only reason this exists; a value from the render context belongs in `js` or `json`. The name is a warning, not a description of the output.

---

## 9. Loop Filters

Loop filters are applied to the **collection** before iteration, inside the `{each}` tag.

```slurp
{each item in products | limit(10) | sort("price", "desc") | filter("available", true)}
```

Multiple loop filters chain left-to-right.

### `limit(n)`

Keep only the first `n` items.

```slurp
{each item in posts | limit(5)}
```

### `sort(key?, dir?)`

Sort items. `key` is a field name (required for objects). `dir` is `"asc"` (default) or `"desc"`.

```slurp
{each p in products | sort("price")}          <!-- ascending by price -->
{each p in products | sort("price", "desc")}  <!-- descending by price -->
{each n in numbers  | sort}                   <!-- sort primitives -->
```

### `reverse`

Reverse the order of items.

```slurp
{each item in timeline | reverse}
```

### `filter(key, value)`

Keep only items where `item[key] == value`. Supports string, number, boolean, and null comparisons.

```slurp
{each item in orders   | filter("status", "shipped")}
{each item in products | filter("featured", true)}
{each item in users    | filter("roleId", 2)}
```

---

## 10. Escaping and Security

### Default HTML escaping

All `${expr}` outputs are HTML-escaped. The following characters are replaced:

| Character | Escaped as |
|---|---|
| `&` | `&amp;` |
| `<` | `&lt;` |
| `>` | `&gt;` |
| `"` | `&quot;` |
| `'` | `&#39;` |

`/` is not escaped.

### THE ESCAPER IS CHOSEN BY CONTEXT

HTML entity escaping is right in HTML text and in a plain attribute value, and wrong everywhere else, so slurp does not apply it everywhere. The renderer picks its treatment from where the slot lands:

- **Inside a `<script>` body: a compile error unless you declare the context.** `<script>` is a raw-text element and the HTML parser never decodes entities there, so entity escaping stops nothing - and no automatic escaper is correct either, because a JS-string escaper would leave `1;alert(1)` intact in `var n = ${ count }`. So `<script>var n = ${ count };</script>` fails the build with `error[UnsafeScriptInterpolation]`. Add `| js` (data inside a quoted JS string literal) or `| json` (a bare JSON value), or use `{html expr}` for raw bytes. A filtered value is emitted verbatim inside a script body, which is what makes `| js` usable there.
- **Inside a JS-evaluated attribute** (`on*`, `@click`, `x-on:`, `x-data`, `x-init`, `x-effect`, `x-show`, `x-text`, `x-html`, `x-if`, `x-for`, `x-model`, `:bind`, `x-bind:`) the value is escaped **before** the attribute escaper runs, so the browser decoding the entities does not undo it. Which escaper depends on position, read from the author's own text: inside a JS string literal the slot is JS-escaped, so `@click="add('${ v }')"` with `v = ');alert(1);('` renders `add('\');alert(1);(\'')` and the value stays one literal. Outside a string literal, in statement or expression position, it is encoded as a self-delimiting JSON literal instead, because `;` and `(` have no JS string escape. Declare intent with `| js`, `| json` or `| unsafe_js`; a `| js` that is false about its position is a compile error.
- **Inside a `style` or `:style` attribute** the CSS-structural characters `; { } ( ) " ' \ @ * < >` are removed from the value. `:` and `/` are kept, so `background:url(${ path })` still works.
- **Inside `srcdoc`** the value is run through the same ammonia allowlist sanitizer that `{html expr sanitize}` uses, at the layer the iframe will see it.

A **`<style>` element body** and a `{html attrs}` **attribute spread** have no protection at all. The CSS strip is routed by attribute name, so nothing reaches text inside `<style>`; the spread's whole purpose is forwarding Alpine directives, which Alpine evaluates as JavaScript, so it can never be made safe for untrusted data. Do not interpolate untrusted data into either. [docs/security-model.md](./docs/security-model.md) has the worked examples and the known gaps.

### A trap: JavaScript template literals

Because `${ }` is interpolated inside script bodies and attributes too, an ordinary JavaScript template literal can be eaten by the compiler. Both places now diagnose it.

```slurp
<script>let t = `${hours}h`;</script>
```

is a **compile error** (it is the `<script>` body rule above; the message names template literals explicitly). In an attribute:

```slurp
<a :href="`/cart/${item.id}`">
```

is a **warning**, `JsTemplateLiteralInAttribute`: slurp evaluates `item.id` against the server context, where it does not exist, and renders `` `/cart/` `` - a dead link with a clean build. It is a warning rather than an error because interpolating a genuinely server-side value inside a backtick literal is legal and works. Build the string by concatenation (`'/cart/' + item.id`) if the value is client-side. Warnings are hidden unless you pass `--warnings` to `slurp validate` or `--verbose` to `slurp build`.

### Attribute escaping

Attribute values are always emitted double-quoted and escaped for that context: beyond `&` and `"`, the escaper also handles `'`, backtick, `<` and `>` as defence against attribute-value breakout.

### `{html}` - raw output (XSS risk)

Only use `{html expr}` when `expr` is fully trusted server-controlled content. User-provided content must use `{html expr sanitize}`.

### `{$let}` JSON safety

Values passed to `{$let}` are serialised as JSON inside a `<script type="application/json">` tag. **Every** `<` in the payload is escaped, which blocks both a `</script>` breakout and an `<!--` comment breakout, and U+2028 and U+2029 are escaped too.

### Template security checks

The compiler enforces the following at compile time:

| Rule | Scope | Error code |
|---|---|---|
| An interpolation in a `<script>` body must carry `js` or `json` | All files | `UnsafeScriptInterpolation` |
| A `\| js` slot in a JS-evaluated attribute must sit inside a string literal | All files | `UnsafeScriptInterpolation` |
| `env.SLURP_SECRET_*` is blocked, dotted and subscript forms alike | **All files, middleware included** | `SecretEnvInTemplate` |
| `env[...]` must be indexed by a literal key | All files | `SecretEnvInTemplate` |
| `request.*` is only accessible in middleware | Non-middleware files | `MiddlewareScopeViolation` |
| Inside middleware, only `request`, `env` and `loop` roots are allowed | Middleware files | `MiddlewareScopeViolation` |
| `{redirect}` is only valid in middleware | Non-middleware files | `RedirectOutsideMiddleware` |
| `{next}` is only valid in middleware | Non-middleware files | `RedirectOutsideMiddleware` |
| Circular imports | Virtual-file-system compiles | `CircularImport` |

Plus one **warning**, raised at parse time so `slurp validate` sees it: a JavaScript template literal in an HTML attribute whose `${ }` slurp would interpolate itself (`JsTemplateLiteralInAttribute`).

**These run in the `compile*` entry points, in `slurp validate`, and in the WASM export `validate_full`.** The other WASM exports (`parse`, `render`, `render_dev`, `validate`, `extract_schema`, `render_section`) do not run them, so a template that passes plain `validate` in an editor can still fail to compile on the server. An authoring tool that wants build parity should call `validate_full`. Note that the walk stops at its **first** violation, so at most one of these is reported per run.

The two script-context rules above additionally have render-time backstops in the renderer, so they are enforced even on the paths that render without the compile-time walk.

The `SLURP_SECRET_` check matches a **literal name prefix**, and it is refused in every file - middleware is not an exception. The compiler never reads the process environment; `env` is just a context root supplied by the host. `${ env.API_KEY }` compiles cleanly and renders whatever the host put there. Getting this right is the host's job, and [docs/security-model.md](./docs/security-model.md) says exactly what that job is.

---

## 11. Error Handling

### Compile errors

The Rust API returns `Result<String, CompileError>`. A `CompileError` contains:

| Field | Type | Description |
|---|---|---|
| `file` | `String` | Template file path |
| `line` | `usize` | 1-based line number |
| `column` | `usize` | 1-based column number |
| `message` | `String` | Human-readable description |
| `severity` | `Severity` | `Error`, `Warning`, or `Info` |
| `code` | `ErrorCode` | Machine-readable enum variant |

### Error codes

| Code | Description |
|---|---|
| `UnexpectedToken` | Lexer or parser saw an unexpected character/token |
| `UnclosedBlock` | A block tag was not closed |
| `InvalidFrontmatter` | Frontmatter could not be parsed |
| `CircularImport` | Component import cycle detected |
| `MissingFile` | A referenced component file was not found |
| `MissingRequiredProp` | A required prop was not passed to a component |
| `UnknownProp` | An unknown prop was passed to a component |
| `TypeMismatch` | A value's type does not match the declared prop type |
| `SecretEnvInTemplate` | `env.SLURP_SECRET_*` accessed in a template |
| `MiddlewareScopeViolation` | `request.*` or `{redirect}` used outside middleware |
| `IterationLimitExceeded` | A `{repeat}` or `{each}` exceeded the safety limit |
| `MissingImageSrc` | `<Image>` used without a `src`. `width` and `height` are optional and default to 800 and 600 |
| `LayoutDepthExceeded` | Layout nesting too deep |
| `UnknownComponent` | A PascalCase tag could not be resolved |
| `UnknownSlot` | A `slot="name"` references a slot that doesn't exist in the layout |
| `UnknownFilter` | A filter name is not recognised |
| `InvalidFilterArgs` | Wrong number or type of filter arguments |
| `RedirectOutsideMiddleware` | `{redirect}` used in a non-middleware file |
| `InfiniteLoop` | Circular data reference detected at render time |
| `UnsafeScriptInterpolation` | A `${ }` in a `<script>` body or a JS-evaluated attribute that does not declare its escaping. Also the code carried by the development-mode advisory warnings for an undeclared JS slot and a CSS-stripped `style` value |
| `JsTemplateLiteralInAttribute` | Warning: a JavaScript template literal in an HTML attribute, whose `${ }` slurp interpolates itself |

**Several of these are never emitted by the compiler** and exist only for host integrations to reuse: `MissingRequiredProp`, `UnknownProp`, `TypeMismatch`, `LayoutDepthExceeded`, `UnknownComponent`, `UnknownSlot`, `InfiniteLoop` and `MissingFile`. In particular, an unresolvable component renders a placeholder rather than raising `UnknownComponent`, and prop types are never checked.

At most 1,000 diagnostics are collected per compile; beyond that they are silently dropped.

### Resource budgets

Every limit below truncates rather than aborting the render. Whether you are TOLD is a separate question, and it differs per budget, so the table says so for each one. Every warning listed needs `-v` to be printed: `slurp build` produces the diagnostic either way, but only prints it when asked.

| Limit | Value | Reported on a `slurp build`? |
|---|---|---|
| Parser nesting depth | 64 | Yes, as a parse error |
| Render recursion depth | 96 | Warning, once per render, shown with `-v` |
| Iterations per `{each}` or `{repeat}` | 1,000 | Warning, once per loop, shown with `-v` |
| Total iterations per render | 1,000,000 | Warning, once per render, shown with `-v` |
| Output bytes per render | 16 MiB | Warning, once per render, shown with `-v` |
| Single filter result | 8 MiB | Yes, as an error; the build fails |
| Cumulative filter bytes per render | 64 MiB | Yes, as an error; the build fails |
| Security-walk depth | 512 | Yes, as an error |
| Blocks per type in a section | `max`, else 200 | No, extra blocks are dropped |
| Block nesting depth | 64 | No, deeper blocks are dropped |
| `fixed(n)` precision | 100 | Clamped |

#### What truncation looks like

A `{each}` over a 1,500-item array renders exactly 1,000 items. With `-v` the build
also prints:

```
warning[IterationLimitExceeded]: Iteration limit: collection has 1500 items, capped at 1000
Build complete. 1 file(s) compiled.
```

The build still SUCCEEDS. Truncating and continuing is the correct behaviour for a
service rendering an untrusted template: a page that loses its tail is better than a
page that fails. What you are protected from is not noticing.

Note the exit code is 0, so a script that only checks the status will not catch this.
Look for the warning, or count what you expected to render.

### `ErrorAccumulator`

Some APIs (`compile()`) return an accumulator holding both errors and warnings:

```rust
let (html, diag) = compile(source, file, &context, BuildMode::Production);
if diag.has_errors() {
    for err in &diag.errors {
        eprintln!("[{}:{}] {}", err.line, err.column, err.message);
    }
}
```

---

## 12. Configuration

### `slurp gitignore`

Writes a `.gitignore` for a theme into the current directory.

```
slurp gitignore [OPTIONS]

Options:
  -p, --path <DIR>   Directory to write into [default: .]
      --append       Add the rules to an existing .gitignore
      --force        Replace an existing .gitignore
      --stdout       Print the template, write nothing
```

It refuses rather than clobbering: if a `.gitignore` already exists you get the
three options above and nothing is written.

The template's useful content is the part naming what must NOT be ignored. A
theme's compiled stylesheet is committed alongside its source, and a rule that
excludes it produces a theme which publishes, renders, and serves unstyled HTML
because the stylesheet 404s, with every static check still green. The same
applies to the `--globals` file, which is committed and, since it is no longer
copied into the output, is not published with the site.

### `slurp build` CLI options

**The input is a flag, not a positional argument.**

```
slurp build [OPTIONS]

Options:
  -i, --input <DIR>      Source file or directory [default: .]
  -o, --output <DIR>     Output directory [default: dist]
      --globals <FILE>   JSON object seeded as the render context
      --middleware <DIR> Directory whose .slurp files are middleware. Repeatable
  -v, --verbose          Print each compiled file and its diagnostics
```

`build` walks the input tree for `*.slurp`, skipping symlinks and any directory named `node_modules`, `dist`, `target`, or starting with `.`. Every non-`.slurp` file is copied to the output tree verbatim, except the `--globals` file, which is build input rather than a site asset.

**`components/` and `layouts/` are checked but never emitted.** Every `.slurp` file is compiled, so an error in a component that no page imports yet still fails the build. Only those two directories are held back from the output, because a component or layout rendered on its own is a fragment rather than a document - a layout comes out carrying literal `<slot>` elements. Everything else is written, so a single `index.slurp` at the root builds to `dist/index.html`.

- **Output is always minified**, with `minify-html` in spec-compliant mode. There is no flag to turn it off. Spec-compliant minification removes optional tags and attribute quotes, so built HTML will not look like your source. The library API does not minify; only the CLI does.
- **`slurp build` truncates a loop and still exits 0.** Every budget in Section 11 truncates rather than failing, so a build that drops the tail of a list reports success. The warning is produced, but `-v` is what prints it. See "Resource budgets" in Section 11.
- **A `--globals` file that is unreadable, malformed, or not a JSON object is fatal**, not a silent fallback to an empty context. The reason to pass it is that some attribute cannot be written without it, and a quiet fallback builds clean and breaks in the browser.

### `slurp validate` CLI options

```
slurp validate [OPTIONS]

Options:
  -d, --dir <DIR>        Directory to validate [default: .]
  -w, --warnings         Include warnings
      --middleware <DIR> Directory whose .slurp files are middleware. Repeatable
```

**It parses and runs the security walk, but it does not render and produces no output.** `cmd_validate` calls `check_security` on every file, so the compile-time rules in Section 10 (including `SecretEnvInTemplate`, the middleware scope rules and the script-context rules) are enforced here. What a clean `validate` does **not** prove is that the file renders: nothing that only the renderer can discover, such as a missing component file or a budget truncation, is reached.

`--middleware` names the directories whose files are middleware, relative to the scanned directory, and is repeatable. The security walk applies two inverted rule sets and has no other way to learn which one a file wants, so **without the flag a correct middleware file reports `MiddlewareScopeViolation` and a broken one is never caught.** `slurp build` takes the same flag, where middleware is checked and then skipped rather than emitted.

### `slurp dev` CLI options

```
slurp dev [OPTIONS]

Options:
  -p, --port <PORT>   Dev server port [default: 3000]
  -b, --bind <ADDR>   Accepted and IGNORED
      --fixtures <F>  Fixture file or directory
```

`slurp dev` is a **launcher**: it shells out to the separate `slurp-dev` binary and forwards only `--port` and `--fixtures`. `slurp-dev`'s other flags (`--theme-dir`, `--source-dir`, `--backend-url`, `--css-watch-cmd`, `--globals`) are unreachable this way, so invoke `slurp-dev` directly if you need them. `--bind` is accepted and ignored: the dev server binds `127.0.0.1` unconditionally.

**Exit codes:** 0 on success or when no files were found; 1 on compile or validate errors, or a bad globals file; 127 if `slurp-dev` cannot be launched; 2 from clap on bad arguments.

### Image service CLI options

```
slurp-image [OPTIONS]

Options:
  --port <PORT>                    TCP port [default: 3001]
  --bind <ADDR>                    Bind address [default: 127.0.0.1]
  --cors-allow-origin <ORIGIN>     Allowed CORS origin, repeatable, `*` for any
                                   [default: send no CORS headers]
  --max-concurrent-transforms <N>  In-flight transforms [default: core count]
  --request-timeout-secs <N>       Whole-request deadline [default: 30]
  --max-output-pixels <N>          Output pixels after resize [default: 8000000]
  --cache-dir <DIR>                On-disk cache directory [default: /tmp/slurp-image-cache]
  --cache-ttl <SECS>               Cache TTL in seconds [default: 3600]
  --memory-cache-size <N>          In-memory cache entries [default: 512]
  --max-disk-cache-bytes <N>       Disk cache ceiling [default: 2 GiB]
  --disk-sweep-interval-secs <N>   Disk cache sweep interval [default: 300]
```

**The defaults are closed.** The service fetches attacker-supplied URLs, so it binds loopback and sends no CORS headers until you ask otherwise.

Not configurable by flag or environment variable: max source size 20 MiB, max decoded size 50 MiB, max width 8192, fetch timeout 10 s, rate limit 100 requests per 60 s per IP.

### Environment variables

**`RUST_LOG` is the only environment variable slurp reads**, in both the dev server and the image service. The compiler reads none at all. Note in particular that no `SLURP_PUBLIC_*` or `SLURP_SECRET_*` mechanism reads the environment; see [Section 10](#10-escaping-and-security).

---

## 13. Rust API Reference

### `compile_template`

```rust
pub fn compile_template(
    source: &str,
    options: CompileOptions,
) -> Result<String, CompileError>
```

Compile a template string. The `file` name used in error messages is `"template"`.

### `compile_with_options`

```rust
pub fn compile_with_options(
    options: CompileOptions,
) -> Result<String, CompileError>
```

Compile from a virtual file system (`options.virtual_files`). Resolves imports against the virtual FS. Detects circular imports before compiling.

### `compile`

```rust
pub fn compile(
    source: &str,
    file: impl Into<String>,
    context: &serde_json::Value,
    mode: BuildMode,
) -> (String, ErrorAccumulator)
```

Lower-level API used by the CLI. Returns `(html, diagnostics)` rather than `Result`. Security checks run. On any error, `html` is empty.

### `CompileOptions`

```rust
pub struct CompileOptions {
    /// JSON string used as the template context.
    pub context_json: Option<String>,
    /// Set to true for middleware files.
    pub is_middleware: bool,
    /// Virtual file system for multi-file compilation.
    pub virtual_files: Option<HashMap<String, String>>,
    /// Entry file name within virtual_files.
    pub entry_file: Option<String>,
    /// Renderer byte budgets. Defaults to `MemoryBudget::DEFAULT`.
    pub budget: MemoryBudget,
}
```

### `BuildMode`

```rust
pub enum BuildMode {
    Development,
    Production,
}
```

In `Development` mode, `{debug}` nodes emit a visible `<pre data-slurp-debug>` element. In `Production` mode they are stripped.

### `parse`

```rust
pub fn parse(
    source: &str,
    file: impl Into<String>,
) -> CompileResult<(Document, ErrorAccumulator)>
```

Lex and parse a template into an AST. Does not run security checks or render.

### `render`

```rust
pub fn render(
    doc: &Document,
    context: &serde_json::Value,
    mode: BuildMode,
) -> (String, ErrorAccumulator)
```

Render a parsed `Document` to HTML against a context JSON value. Three arguments; there is no `file` parameter. Like `compile`, it returns diagnostics alongside the output rather than a `Result`, because rendering is total.

### Other exports

`compile_with_registry`, `render_with_registry`, `extract_schema`, `extract_block_schema`, `eval_fetch_urls`, `sanitize_html`, and the `sections::*` family for host-side section and block rendering.

---

## 14. WASM / TypeScript Integration

### Building the WASM module

```sh
node compiler-wasm/build.mjs                # --target nodejs, the default
node compiler-wasm/build.mjs --target web   # for a browser host with a bundler
```

`pkg/` is build output and is not committed. `pnpm build` runs this as part of the `@bytesell/slurp-compiler-wasm` package.

### WASM exports

Seven functions are exported. There is no `compile_wasm`, `parse_wasm` or `get_errors_wasm`, whatever an older integration may call.

```typescript
function parse(source: string): string
function render(ast_json: string, context_json: string): string
function render_dev(ast_json: string, context_json: string): string
function validate(source: string): string
function validate_full(source: string, is_middleware: boolean): string
function extract_schema(source: string): string
function render_section(ast_json: string, section_state_json: string, context_json: string): string
```

What every embedder needs to know:

1. **Each returns a JSON string, not an object.** Callers must `JSON.parse()` the result.
2. **`validate_full` is the one that runs the compile-time security walk**, and it is a separate export rather than a change to `validate` so that editor-preview hosts are unaffected. Rules such as "an unfiltered `${ }` may not appear in a `<script>` body" live in the walk, not in the parser, so a tool that calls only `validate` reports a file clean that `slurp build` then rejects. Pass `is_middleware` matching the eventual compile; `false` is what the CLI uses. The walk returns on its first violation, so at most one security diagnostic comes back per call.
3. **A WASM `render` still enforces nothing at compile time.** The secret-env, middleware-scope and `{redirect}` / `{next}` rules are unenforced on the render exports. The renderer's own script-context backstops do run there, but do not use a WASM render as a production render path.
4. **`render_dev` records advisory warnings that `render` suppresses**: CSS-structural characters stripped from a `style` value, and an undeclared interpolation in a JavaScript-evaluated attribute. Loop truncation is NOT among them - the resource budgets warn in both modes. An editor or preview pane still wants `render_dev` for the two above.

### TypeScript runtime

The TypeScript runtime is published as **`@bytesell/slurp-runtime`**. It is optional: a page using none of `{fetch}`, `{$let}`, `{try}` or client navigation does not need it.

```typescript
import '@bytesell/slurp-runtime'
```

It ships three ESM bundles: `slurp.core.js` (`cn`, `parseParams`, navigation, Alpine boot, dev-mode HMR), `slurp.fetch.js` (`slurpFetch`) and `slurp.infinite.js` (`initInfiniteScroll`).

Only the first is exported as `@bytesell/slurp-runtime`. Because of the unwired markers described just below, the other two are published under an `experimental/` prefix - `@bytesell/slurp-runtime/experimental/fetch` and `@bytesell/slurp-runtime/experimental/infinite` - so that a consumer cannot import them without being told. See `runtime/README.md`.

#### What the runtime does NOT do

Be careful here, because the runtime's capability and the compiler's output do not currently meet in the middle.

- **It registers zero Alpine magics and zero Alpine directives.** There is no `$reactive`, no `$money`, nothing of that shape. It touches Alpine in exactly three places: `Alpine.start()`, `Alpine.initTree()` after a client navigation, and `Alpine.data('slurpFetch', ...)`.
- **`{$let}` has no hydrator.** Nothing reads `data-slurp-let`.
- Several markers the runtime looks for have **no producer in the compiler**: `[data-slurp-page]`, `<meta name="slurp-route">`, `x-data="slurpFetch(...)"` and `[data-slurp-sentinel]`. In practice that means `slurp.fetch.js` and `slurp.infinite.js` are not reachable from compiled output today, and client navigation needs a theme that supplies `[data-slurp-page]` and a `navigate` attribute on `<body>` itself.

An embedding host can supply any of the above itself. This section exists so that you find that out here rather than by debugging.

---

## 15. Image Optimisation Service

The `slurp-image` binary is a standalone HTTP microservice. The `<Image>` component in templates generates URLs that point to it.

### Endpoint

```
GET /_slurp/image?src=<url>&w=<width>&f=<format>
```

| Parameter | Required | Default | Allowed |
|---|---|---|---|
| `src` | yes | none | absolute `http`/`https`, at most 4096 bytes, no control characters, no userinfo |
| `w` | yes | none | 1 to 8192 |
| `f` | no | `webp` | `avif`, `webp`, `jpeg` (or `jpg`) |

**There is no `h`, no `q`, and the format parameter is `f`, not `fmt`.** PNG and GIF are accepted as input formats and are never emitted.

The only transform is a **width-only resize that preserves aspect ratio and never upscales**, using Lanczos3. Quality is fixed (AVIF q=80 speed=6; JPEG and WebP use the `image` crate defaults). Height on the `<Image>` component is used for the `<img>` attribute only and is never sent to this service.

Errors are JSON `{"error": "..."}`: 400 bad parameters or URL, 403 blocked, 413 too large, 422 undecodable, 429 rate limited, 500 encode failure, 502 DNS or fetch failure.

### Security

- SSRF protection: every hostname is DNS-resolved before connecting and every returned address must be globally routable unicast. The connection is then pinned to those exact addresses, so a second lookup cannot reach a private one.
- Scheme allowlist: only `http://` and `https://` are permitted as source URLs.
- Magic byte validation: the response body must begin with a JPEG, PNG, GIF or WebP header, and the format the decoder infers from the content must match the one the header declared. An AVIF or HEIC `ftyp` header is recognised only so such a file is reported as an unsupported source rather than as corrupt data; there is no ISO base media decoder linked in, and AVIF is an output format only.
- Source size limit: enforced against `Content-Length` and again while streaming, because the header can be absent or can lie.
- Decompression bomb protection: declared width times height times the decoder's real bytes per pixel must not exceed the limit, checked before any pixel buffer is allocated.
- No redirects followed: redirect policy is `none()`.
- Rate limiting is an in-memory fixed-window counter per client. It resets on restart and is not shared between instances, so put a reverse proxy in front of it in production.

The full treatment is in [docs/security-model.md](./docs/security-model.md).

### Caching

Two-level cache:

1. **In-memory** (moka LRU): configurable entry count, default 512
2. **Disk**: stored in `--cache-dir` as `<sha256-of-params>.{ext}` + `.meta` sidecar with expiry timestamp

---

## 16. Limitations and Known Issues

Security limitations are separate and live in [docs/security-model.md](./docs/security-model.md).

### No object or array literals in templates

`{key: value}` and `[1, 2, 3]` are parse errors. Pass structured data in through the render context.

```slurp
{* NOT supported: *}
{$let config = {theme: "dark"}}

{* Use context binding instead: *}
{$let config = siteConfig}
```

### No callable functions, and no `===`, `??`, `?.`, `in` or bitwise operators

`f(a, b)` parses and evaluates to `null`, rendering empty with no error; there is no way to register a function. The missing operators are parse errors, except that path access is null-safe at every depth, so `?.` is not needed. See [Section 3.2](#32-expressions).

### `{with}` is a keyword with no implementation

`with` is in the lexer's block-tag keyword list but the parser has no arm for it, so `{with ...}` always fails with `Unknown block: {with}`. It is dead surface area.

### Unicode escapes: `\uXXXX` yes, `\u{XXXX}` no

`\uXXXX` works inside quoted string literals. The braced form `\u{XXXX}` is not supported anywhere, and **template literals support only** `\n` `\t` `\r` `\\` `` \` `` `\$`, so no Unicode escape works inside a backtick string. Use the character directly or pass it through the context.

### `{debug}` stripped silently in production

`{debug}` nodes emit nothing in `BuildMode::Production`. That is correct, and it can surprise developers who expect an error if `{debug}` is left in production templates.

### Currency formatting is manual (no ICU)

The `currency` filter uses a manual lookup table for symbols and separators to keep the WASM binary small. A currency not in the table (USD, EUR, GBP, JPY, CAD, AUD, CHF) gets its **code used as the prefix**, so `SEK 10.00`.

### `{try}` error branch is server-side no-op

The `{error}` branch inside `{try}` is never rendered on the server. It is client-side only. The server always renders the `{try}` body.

### Parts of the compiler-to-runtime contract are unwired

`{$let}` has no hydrator, `on:` handlers compile to inert literal attributes, and several markers the runtime looks for have no producer in the compiler. See [Section 14](#14-wasm--typescript-integration).

### Component AST fields that are parsed and never read

`spread`, `events` and `slot` on a component node all reach the AST and are then ignored by the renderer.

### Validating a theme that has middleware needs `--middleware`

The security walk applies inverted scope rules to middleware and has no way to infer which files those are, so both `validate` and `build` take a repeatable `--middleware <DIR>`. Validate the shipped example with:

```bash
slurp validate --dir examples/basic-theme --middleware middleware
```

Without the flag, `middleware/auth.slurp` reports a spurious `MiddlewareScopeViolation` for its perfectly legal `request.*` access.

### `detect_circular_dfs` re-parses files on each traversal

Circular import detection re-parses every referenced file on every DFS pass, so the cost is O(n * depth). Fine at typical project sizes; a parse cache would fix it for larger ones.
