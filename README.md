# Slurp

**Lightweight, agent-first templating engine.**

[Documentation](https://docs.bytesell.io/slurp)

Slurp compiles `.slurp` files to plain HTML on the server, in one pass, with everything escaped by default. You write pages, components and layouts; what ships is HTML, and the browser runtime is optional.

It exists for the case where you need a real templating language for themes you did not write and cannot fully trust. Sections and blocks declare a typed schema on top of that, so a non-technical person can edit a page through a form without touching the template.

```slurp
---
using "@components/ProductCard"
props {
  products: any
}
---
{each product, i in products | limit(8) | sort("price")}
  <ProductCard product={product} featured={loop.first} />
{empty}
  <p>No products found.</p>
{/each}
```

## Install

```bash
cargo install slurp-compiler
```

That gives you a binary named `slurp`.

```bash
slurp build      # compile a tree of templates to HTML
slurp validate   # check without building
slurp dev        # dev server with hot reload
slurp gitignore  # write a .gitignore for a theme
```

Optional pieces:

```bash
cargo install slurp-dev-server        # the hot-reload server `slurp dev` launches
cargo install slurp-image             # on-demand image resizing and transcoding
pnpm add @bytesell/slurp-runtime      # the optional browser runtime
pnpm add -D @bytesell/slurp-mcp       # MCP server, so an agent can write Slurp correctly
```

## Your first template

```bash
mkdir hello && cd hello
```

`index.slurp`:

```slurp
<!doctype html>
<html>
  <body>
    <h1>${ site.title }</h1>
    {each product in products | sort("price")}
      <p>${ product.name } - ${ product.price | currency("USD") }</p>
    {empty}
      <p>Nothing for sale yet.</p>
    {/each}
  </body>
</html>
```

`data.json`:

```json
{
  "site": { "title": "My Shop" },
  "products": [
    { "name": "Notebook", "price": 12.5 },
    { "name": "Pen", "price": 3 }
  ]
}
```

```bash
slurp build --globals data.json
cat dist/index.html
```

```html
<!doctype html><body><h1>My Shop</h1><p>Pen - $3.00<p>Notebook - $12.50
```

Output is minified, and the products came out sorted by price. `${ }` is HTML-escaped everywhere, so a product named `<script>` renders as text.

`--globals` is build-time data. In a hosted setup the data arrives per request from your server instead, and `--globals` covers only the values that have to be resolved at compile time.

## Using it as a library

```toml
[dependencies]
slurp-compiler = "0.1"
```

```rust
use serde_json::json;
use slurp_compiler::{compile_template, CompileOptions};

let html = compile_template(
    "<h1>Hello, ${ name }!</h1>",
    CompileOptions {
        context_json: Some(json!({ "name": "Alice" }).to_string()),
        ..Default::default()
    },
)?;
```

There are also WebAssembly bindings for browser and editor hosts. See [compiler-wasm/README.md](./compiler-wasm/README.md).

## For agents

`@bytesell/slurp-mcp` is an MCP server that gives a coding agent the language reference plus tools to validate, render, lint and inspect schemas. Slurp is small and unusual enough that an agent will invent syntax without it.

[`llms.txt`](./llms.txt) and [`llms-full.txt`](./llms-full.txt) serve the same reference to anything that reads that convention. Both are generated from the data the MCP server uses, so they cannot drift from it.

## How it behaves

**One server-side pass over a JSON context.** No reactivity, no function calls, no user code execution at render time.

**Escaped by default, and the escaper follows the context.** HTML text and plain attributes are HTML-escaped; a `style` attribute has its CSS-structural characters removed; `srcdoc` goes through an allowlist sanitizer; a JavaScript-evaluated attribute is escaped for the string literal it sits in, or encoded as a JSON literal if it sits outside one. Inside a `<script>` body Slurp refuses instead: an interpolation there is a compile error unless you declare the context with `| js` or `| json`, because no automatic escaper is correct in every position.

**Total and tolerant.** Missing data renders as the empty string, budgets truncate rather than aborting, and unknown editor settings are dropped. That makes an untrusted template safe to render in a shared service, and it means a mistake usually produces wrong output rather than an error. Build with `--verbose`.

## Documentation

| | |
|---|---|
| [SLURP_DOCS.md](./SLURP_DOCS.md) | Language reference: every tag, filter, operator and CLI flag |
| [SECURITY.md](./SECURITY.md) | How to report a vulnerability |
| [CONTRIBUTING.md](./CONTRIBUTING.md) | Building it, and the gates a change has to pass |
| [CHANGELOG.md](./CHANGELOG.md) | What changed, per release |

## Status

**0.1.0, pre-1.0.** The Rust API and the template language will both change without a deprecation cycle until 1.0.

Slurp was extracted from a production system where it renders multi-tenant sites, so parsing, rendering, escaping and the resource budgets are exercised daily. The edges are less settled. Parts of `@bytesell/slurp-runtime` are not wired to compiler codegen, and those entry points ship under `experimental/`. The editor tooling and the compiler disagree on a few points of syntax; the compiler is right. [runtime/README.md](./runtime/README.md) has the detail.

## Contributing

Bug reports and pull requests are welcome. Start with [CONTRIBUTING.md](./CONTRIBUTING.md), and please read [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).

For anything security-related, do not open a public issue. Use GitHub's private vulnerability reporting, as described in [SECURITY.md](./SECURITY.md).

## License

[MIT](./LICENSE)
