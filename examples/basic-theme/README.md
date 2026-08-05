# Slurp Basic Theme

An e-commerce storefront used as a reference implementation, a starter theme and a test
harness for the Slurp compiler.

Every claim below was checked by rendering it. Where a feature lives in the browser runtime
rather than in `slurp build`, this file says so, because that difference decides whether a
page works with no JavaScript served.

## Structure

```
basic-theme/
  layouts/
    base.slurp            # HTML shell: head, nav, footer, the head + default slots
  components/
    Navbar.slurp          # Sticky nav, cart count, mobile menu markup
    Footer.slurp          # Footer nav columns and social links
    ProductCard.slurp     # Product card: sale badge, prices, AddToCart
    ReviewCard.slurp      # Review: star rating, verified badge, date filter
    Badge.slurp           # Variant badge, mapped from a prop to a class
    AddToCart.slurp       # Button, server-rendered in its resting state
    Shell.slurp           # Content shell: header, optional sidebar, one slot
    SkeletonCard.slurp    # Card skeleton for a fetch {loading} branch
    SkeletonProduct.slurp # Product page skeleton, uses {repeat}
  pages/
    index.slurp           # Homepage: hero, featured sections, fetch, each, filters
    product.slurp         # Product detail
    collection.slurp      # Collection listing
    cart.slurp            # Cart
    account.slurp         # Account
    404.slurp             # Custom 404
  middleware/
    auth.slurp            # Session guard with a redirect
  mock-context.json       # Populated render context for local preview
  .gitignore              # Written by `slurp gitignore`
  README.md
```

## Running locally

There are three entry points, and they take different flags. Run them from the repo root.

```bash
# Validate. --middleware is required for this theme: middleware obeys inverted scope
# rules, and it is a compile option rather than a path convention, so without the flag
# middleware/auth.slurp reports MiddlewareScopeViolation for its own correct request.* use.
slurp validate --dir examples/basic-theme --middleware middleware --warnings

# Build to static HTML. --globals seeds the render context, which is how the mock
# context is fed in. --middleware takes the same meaning here, and a middleware file is
# checked and then skipped rather than emitted, because it is not a page.
slurp build --input examples/basic-theme --output dist \
  --middleware middleware \
  --globals examples/basic-theme/mock-context.json

# Dev server with hot reload, on http://localhost:3000.
# `slurp dev` serves the CURRENT directory and takes only --port, --bind and --fixtures.
cd examples/basic-theme && slurp dev

# The standalone binary is the one that accepts a directory and a context:
slurp-dev --theme-dir examples/basic-theme --globals examples/basic-theme/mock-context.json
```

There is no `slurp watch`. The subcommands are `build`, `validate`, `dev` and `gitignore`.

`--globals` supplies page context. It is a different thing from `--fixtures`, which maps a
request path to a canned JSON response so `{fetch}` resolves with no backend running. The
globals file is read, not copied: it does not appear in the output directory. Every other
non-`.slurp` file in the input tree is copied through, which is how `README.md` and
`.gitignore` end up in `dist/`.

`build` emits pages only. `components/` and `layouts/` are compiled and checked, but they are
not pages, so no HTML is written for them.

## Features demonstrated

### Layouts and slots

`layouts/base.slurp` is entered with the block form:

```slurp
{layout "@layouts/base"}
  {head}<link rel="canonical" href="/x" />{/head}
  <h1>Page</h1>
{/layout}
```

Only two slot names are ever filled: `default` and `head`. The block form above is what
routes a `{head}` block into `<slot name="head" />`. The element form `<layout src="...">`
builds its node with no head slot, so that combination silently cannot work.

`base.slurp` owns `<title>` and the meta description, driven by the `seo` value in the
render context. The head slot is for additional tags. A page that emits its own `<title>`
there would produce two.

There are NO named slots, and `Shell.slurp` is written accordingly: it has one `<slot />`
and takes everything else as props. See "What the language does not have" below.

### Control flow

- `{each item, i in list | limit(n) | sort("field")}` with `loop.index`, `loop.count`,
  `loop.first` and `loop.last`. The item name comes first, the index second.
- `{empty}` inside `{each}` for the zero state.
- `{if}` / `{else if}` / `{else}` / `{/if}`.
- `{match expr}` with `"value" -> body` arms and `_` for the wildcard. There is no
  `{case}` keyword, and a `{match}` cannot be used inside an attribute expression.
- `{repeat n}` for skeletons and star ratings. The count is any expression.
- `{html product.description sanitize}` for raw HTML from an untrusted source.

### Filters

| Filter | Example |
|---|---|
| `currency` | `${ product.price \| currency }` |
| `truncate(n)` | `${ product.description \| truncate(160) }` |
| `date("format")` | `${ review.createdAt \| date("MMMM D, YYYY") }` |
| `upper` | `${ product.status \| upper }` |

There are 13 value filters: `upper`, `lower`, `currency`, `truncate`, `fixed`, `date`,
`default`, `plural`, `int`, `float`, `js`, `json`, `unsafe_js`. Loop filters are a separate,
disjoint set usable only in an `{each}` header: `limit`, `sort`, `reverse`, `filter`.

Filter arguments must be LITERALS. `currency(store.code)` reads its argument as the empty
string and silently emits no currency marker at all. `default` is the only exception.

`date` parses ISO strings, not epoch milliseconds. An input with fewer than three
`-`-separated parts is returned unchanged, so epoch millis render verbatim.

### Class lists

```slurp
class={"product-card", "product-card--featured": featured}
```

A bare entry is emitted unconditionally; an entry written `"name": condition` is emitted
only when the condition is truthy.

A comma entry with no `: condition` is NOT a conditional. `class={"a", cond && "b"}`
stringifies the false result, so it renders `class="a false"` and adds a junk class. This
is not diagnosed.

Template-literal class values work: ``class=`card-${status}` ``.

### Image

```slurp
<Image src={product.image} alt={product.title} width={600} height={400} />
```

`src` is required and is the only error this component raises. `width` and `height` default
to 800 and 600, and MUST be braced: `width=600` lexes as a boolean attribute plus a second
one named `600`, so the value is lost and the default is used instead.

`priority` switches loading from lazy to eager. There is no `lazy` prop: lazy is already the
default, and an unrecognised prop is dropped.

### Middleware

`middleware/auth.slurp` uses `request.*` and `{redirect "/login"}`. Both are legal ONLY in a
file compiled as middleware, which is a compile option, not a filename convention. Pass
`--middleware middleware` to `slurp validate` or the file reports a scope violation.

### Env guards

`{if env.dev}{debug value}{/if}`. `{debug}` renders a pretty-printed value in DEVELOPMENT
mode and is stripped with no trace in production. `slurp build` always uses production mode,
so these produce nothing in a build and are visible only under the dev server.

## What the language does not have

Every item here was tried against the compiler. Most fail SILENTLY, which is why they are
worth listing in a starter theme.

**No callable functions, of any kind.** `math.round(x)`, `math.max(a, b)`, `math.year()`,
`Date.now()`, `String(x)`, `parseInt(x)` all parse, evaluate to null and render the empty
string, with no diagnostic. There are also no array properties: `items.length` is null too.
Compute on the server and pass the result through the render context. This theme passes the
copyright year in as `year` for exactly that reason.

**No `===` or `!==`.** They lex as `==` plus a stray `=` and produce a parse error. `==` is
already type-strict deep equality, so it is what you want.

**No named slots.** A component has exactly one content slot. `<slot name="sidebar" />`
renders as a LITERAL `<slot>` element, and a child written `<nav slot="sidebar">` is not
routed to it: it lands in the default slot with everything else, while the named slot's
fallback content renders as well. You get both copies, in the wrong place, with no
diagnostic. Anything that would have been a second slot has to become a prop or its own
component.

**`{repeat}` binds no loop variables.** `loop.index`, `loop.count`, `loop.first` and
`loop.last` are all null inside a `{repeat}`; they exist only in `{each}`. `ReviewCard`
therefore counts filled and empty stars with two `{repeat}` blocks rather than testing an
index. `loop.even` does not exist in either construct.

**`{$let}` must be in the body, and creates no server binding.** In frontmatter it is an
unrecognised statement and is discarded silently. In the body it compiles to a
`<script type="application/json" data-slurp-let="name">` payload. Either way the name is not
readable in the same template: `${ menuOpen }` renders empty and `{if menuOpen}` is false.

**`{if}` is not a reactive region.** It is evaluated once at render time and the chosen
branch is baked into the HTML. A block written `{if menuOpen}` around a mobile menu emits
nothing at all, so no client toggle can ever reveal it. Render the markup and switch a class
instead, which is what `Navbar.slurp` does.

**A quoted component prop is a literal string.** `<Card title="Hi ${name}" />` passes the
characters through verbatim. Element attributes DO interpolate, which is the asymmetry that
catches everyone. The brace-plus-template-literal form is the one that interpolates:

```slurp
<Card title={`Hi ${name}`} />
```

**`<Head>` is not a construct.** There is no such component, so it renders as a
`<div data-slurp-component="Head">` placeholder in the BODY, silently. Per-page head content
goes in a `{head}` block inside `{layout "..."}`.

**Unquoted attribute values are lost.** `width=100` on an element lexes the value as a
second attribute NAME, so `<img width=100 height=30>` renders as `<img 100 30>`. Quote them
or brace them.

**A `{match}` cannot appear inside an attribute expression.** `class={ {match x} ... {/match} }`
is a hard parse error, and the diagnostic carries no filename, so it is easy to miss in a
multi-file run. Map a prop to a class with a conditional class list instead, as `Badge.slurp`
does.

**Unknown frontmatter statements are skipped silently,** so a misspelled directive produces
no diagnostic. There is no `layout:` directive; use the `{layout "..."}` block tag.

**Types are never enforced.** `type` and `props` annotations are recorded for tooling and
never checked at render time, so a missing required prop is simply null.

## Client-side behaviour

`slurp build` produces plain HTML and injects NOTHING. The runtime is a separate package,
`@bytesell/slurp-runtime`, and a theme loads it by hand. This theme does it in `layouts/base.slurp`:

```slurp
<script src="/slurp.core.js"></script>
```

### What the runtime does implement

- **`<body navigate transition>`.** `navigate` turns on client-side navigation and
  `transition` uses the View Transitions API. Both are real attributes the runtime reads.
  Caveat: the swap targets a `[data-slurp-page]` element, which the compiler never emits, so
  without one the runtime falls back to a full page load.
- **`navigate=false` on a link,** to opt that link out. Write it UNQUOTED: the parser
  recognises only the bare-word form, and it compiles to `navigate="false"`, which is the
  spelling the runtime checks. Quoted, `navigate="false"` compiles to a bare `navigate`
  attribute and the link stays opted IN, which is the opposite of what was written, with no
  diagnostic. Cross-origin links, `target`, `download`, hash links and modified clicks are all
  skipped anyway.
- **`{fetch}` options** `cache(n)`, `retry(n)`, `timeout(n)`, `poll(n)`, `paginate(n)`,
  `infinite(n)` and the bare flag `abort-on-navigate`. Note the CALL syntax: `cache(300)`,
  not `cache:300`, and `paginate(12)`, not `paginate limit=12`. An unrecognised option is
  DROPPED SILENTLY.
- **Pagination and infinite scroll helpers** `page`, `totalPages`, `prev()`, `next()`,
  `loadMore()`, `hasMore` and `items`. Read the next paragraph before using any of them.

They are NOT reachable from a compiled template. They exist only as properties of an Alpine
component the runtime registers, in the separate `slurp.fetch` and `slurp.infinite` bundles
which `slurp.core.js` does not include, and the compiler never emits the
`x-data="slurpFetch(...)"` binding that would create one. Inside a `{fetch}` body the name is
rebound to the ARRAY itself, so `products.next()` is a call on an array: it parses, evaluates
to null and renders empty. `hasPrev`, `hasNext` and `lastPage` do not exist at all; the
nearest real property is `hasMore`.

Server-side, `{fetch}` performs NO network request, and **the source expression is never
evaluated at all**. It is parsed and security-checked and then ignored by the renderer, so
`from "/api/products"` and `from getProducts(slug)` are equally inert: both are a DECLARATION
the compiler records for a host or the runtime to act on. A URL string is the honest spelling,
because a call implies a function-calling capability the language does not have.

What the renderer does instead is read the fetch NAME out of the render context and pick a
branch: absent or null gives `{loading}`, an `__error` key gives `{error}`, an empty array
gives `{empty}`, anything else renders the body with the name rebound. The branch order is
fixed: body, `{loading}`, `{error}`, `{empty}`.

In an `{error}` branch, `error` is bound to the VALUE OF the `__error` key, not to the fetch
result. So `${ error.message }` reads a context shaped like this:

```json
{ "reviews": { "__error": { "message": "Upstream timed out" } } }
```

The fetch name still holds the whole object, so `${ reviews.message }` would reach a
top-level `message` instead.

### What is NOT wired

- **`on:` handlers do nothing.** `on:click`, `on:change`, `on:input` and `on:add-to-cart`
  compile to literal HTML attributes that neither the browser nor the runtime binds, and the
  runtime has no `on:` handling at all. They are a declaration for a host runtime to pick up.
  Unlike an ordinary attribute an `on:` value is also NOT interpolated, so
  `on:click="f(${item.id})"` emits a literal `${item.id}` into the DOM. Publish the value in a
  `data-` attribute instead, which does interpolate, and let the handler read it back.
  `AddToCart.slurp` uses `data-product-id` for exactly this reason.
- **`on:visible` is not a feature.** There is no pseudo-event registry and no
  IntersectionObserver bound to it.
- **`$dispatch` and `$event` are not provided by Slurp.** Alpine ships them, but only inside
  its own `x-on:` / `@` handlers, which `on:` does not compile to.
- **`$cart` has no special meaning.** A `$name` path is a reactive marker for a client
  runtime and resolves server-side exactly like a plain path. There is no built-in cart store.
  Note the matching trap on the context side: the `$` is part of the PATH syntax and is not
  part of the key, so `${ $cart.count }` looks up `cart`. A context keyed `"$cart"` is
  unreachable by either spelling and renders empty, which is why this theme keys it `"cart"`.
- **`{$let}` is not hydrated.** Nothing in the runtime reads `data-slurp-let`, so the payload
  is emitted and never picked up. The toggles in `Navbar.slurp` and `AddToCart.slurp` are
  written so the SERVER-rendered resting state is correct and honest.
- **`{try}` has no error boundary.** The server always renders the body and never the
  `{error}` branch, which is discarded rather than emitted, and no runtime consumes it.
- **`.lastPage` does not exist.** The property is `hasMore`.

Because of the above, this theme is a rendering example. Its interactive parts describe
intent, and driving them needs a host runtime that binds `on:` and hydrates `{$let}`.

## Mock context

`mock-context.json` populates the render context for local preview. Pass it with
`--globals`.

- `product` - full product with specs, images, comparePrice
- `collection` - collection header data
- `featured` - two featured sections with products
- `reviews` - three reviews
- `stock` - `{ "count": 3 }`, which trips the low-stock branch
- `cart` - a two-item cart with subtotal, tax and total
- `profile`, `orders`, `addresses` - account page data
- `year` - the copyright year, because the language cannot compute one
- `seo`, `params`, `env`, `request` - framework context
