# Slurp

**Slurp turns `.slurp` template files into plain HTML, on the server, in one pass, with everything escaped by default.**

It is a lightweight, agent-first templating engine. You write pages, components and layouts; it compiles them to plain HTML with almost no client JavaScript. Sections and blocks additionally declare a typed schema, so a non-technical person can edit a page through a form without touching the template. The mental model closest to it is a small Astro.

It exists for the case where you want a real templating language for user-supplied themes, but you do not want to ship a JavaScript framework to do it, and you cannot fully trust the person writing the template. The output is HTML, and the browser runtime is optional.

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

## Status

**0.1.0. Pre-1.0, and the API is unstable.** Both the Rust API and the template language will change without a deprecation cycle until 1.0.

Slurp was extracted from a production system where it renders multi-tenant sites, so the core (parsing, rendering, escaping, the resource budgets) is exercised daily. The edges are less settled. [SLURP_DOCS.md](./SLURP_DOCS.md) marks every such gap inline rather than leaving you to find it by parse error, and the short version is below.

### Known gaps in 0.1.0

Read this before you conclude something is broken. None of it is a bug you need to report.

- **Parts of the browser runtime are not wired to the compiler.** Client navigation, route params, `{fetch}` reactivity and infinite scroll are implemented and tested in `@slurp/runtime`, but the compiler emits none of the markers they bind to (`[data-slurp-page]`, `<meta name="slurp-route">`, `x-data="slurpFetch(...)"`, `[data-slurp-sentinel]`). Against the 0.1.0 compiler they attach to nothing and do nothing, silently. The two affected entry points are published as `@slurp/runtime/experimental/*` so this cannot be discovered at runtime; [runtime/README.md](./runtime/README.md) has the full table. An embedding host that supplies those markers itself does get working behaviour, which is the case slurp was extracted from. The missing piece is compiler codegen, which is a feature and is not in this release.
- **The example theme under `examples/basic-theme/` needs `--middleware` to validate.** It has a `middleware/` directory, and the security walk applies inverted scope rules there that it cannot infer, so validate it with `slurp validate --dir examples/basic-theme --middleware middleware`. Without the flag its middleware file reports a spurious `MiddlewareScopeViolation`. (An earlier revision of this file said the theme did not compile at all and produced 209 errors, blaming `{loading}` and `{fetch ... paginate=}`. That was repaired; do not act on it.)
- **The editor tooling and the compiler disagree on a few points of syntax.** Where they differ the compiler is right.
- **`{$let}` has no hydrator, and the runtime registers no Alpine magics or directives.**

## Why it is shaped this way

Three decisions explain most of the behaviour you will meet:

1. **One server-side pass over a JSON context.** No reactivity, no function evaluation, no user code execution at render time. What comes out is HTML.
2. **Escaped by default, and the escaper is chosen by context.** In HTML text and plain attributes `${ expr }` is HTML-escaped. A slot in a `style` attribute has its CSS-structural characters removed, a slot in `srcdoc` is run through the allowlist sanitizer, and a slot in a JavaScript-evaluated attribute is escaped for the JS string literal it sits in or encoded as a self-delimiting JSON literal if it sits outside one. In a `<script>` body slurp refuses instead: an interpolation there is a compile error unless you declare its context with `| js` or `| json`, because no automatic escaper is correct in every position and a fix-shaped non-fix is worse than an honest failure. `{html}` is the only way to emit raw output, and `{html expr sanitize}` runs an allowlist sanitizer. [SECURITY.md](./SECURITY.md) has the full treatment, with worked examples and the limitations that remain.
3. **Total and tolerant.** Missing data renders `""`. Budgets truncate rather than aborting. Unknown editor settings are dropped rather than rejected. That is what makes it safe to render an untrusted template in a shared service, and it is also why mistakes tend to be silent.

   **Some of those truncations are silent even with `--verbose`.** `slurp build` always renders in production mode, and the iteration-limit warnings are development-mode only, so a `{each}` over 1,500 items emits 1,000 of them and reports nothing. `--verbose` prints diagnostics that exist; it does not cause these to be produced. Use `slurp dev`, or a library or WASM entry point that takes `BuildMode::Development`, when something renders short. [SLURP_DOCS.md](./SLURP_DOCS.md) has the per-budget table of which limits report and which do not.

## Install

### The compiler and CLI

```bash
cargo install slurp-compiler
```

That installs a binary named **`slurp`**, not `slurp-compiler`.

```bash
slurp build --input . --output dist   # compile a tree of templates to HTML
slurp validate --dir .                # check without building
slurp dev --port 3000                 # dev server with hot reload
```

Note that `--input` is a flag, not a positional argument.

`slurp dev` is a launcher, not a server: it runs the `slurp-dev` binary from the separate `slurp-dev-server` crate. If you installed only `slurp-compiler` it exits with "Could not launch the dev server binary". Install it too (see [The optional pieces](#the-optional-pieces)).

## Your first template

```bash
mkdir hello && cd hello
```

Create `index.slurp`:

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

Create `data.json`:

```json
{
  "site": { "title": "My Shop" },
  "products": [
    { "name": "Notebook", "price": 12.5 },
    { "name": "Pen", "price": 3 }
  ]
}
```

Build it:

```bash
slurp validate --dir .
slurp build --input . --output dist --globals data.json --verbose
cat dist/index.html
```

You get this, minified, with the products sorted by price:

```html
<!doctype html><body><h1>My Shop</h1><p>Pen - $3.00<p>Notebook - $12.50
```

Two things worth noticing straight away. `${ }` is always HTML-escaped, so a product named `<script>` renders as text rather than running. And `--globals` is how you feed data to a **build**; in the system slurp was extracted from, data arrives per request from the host instead, and `--globals` covers the values that must be resolved at compile time. There is no `--data` flag.

`slurp validate` runs the same parse and the same compile-time security checks a
build runs, so a clean `validate` means a build will not fail on either. It does
not render, so it cannot report a render-time diagnostic. Add `--warnings` to see
warnings as well as errors.

### As a Rust library

```toml
[dependencies]
slurp-compiler = "0.1"
```

```rust
use slurp_compiler::{compile_template, CompileOptions};
use serde_json::json;

let html = compile_template(
    "<h1>Hello, ${ name }!</h1>",
    CompileOptions {
        context_json: Some(json!({ "name": "Alice" }).to_string()),
        ..Default::default()
    },
)?;
```

### The optional pieces

```bash
cargo install slurp-dev-server   # `slurp-dev`, the hot-reload dev server
cargo install slurp-image        # `slurp-image`, the on-demand image service
pnpm add @slurp/runtime          # the optional browser runtime
```

A page needs no JavaScript from slurp at all. What `@slurp/runtime` adds today is Alpine.js, a global `cn()`, and the dev-mode hot-reload client. Its client navigation, `{fetch}` reactivity and infinite scroll are implemented but **not wired to the compiler in 0.1.0** - see [Known gaps](#known-gaps-in-010) below and [runtime/README.md](./runtime/README.md).

## Documentation

| Document | What is in it |
|---|---|
| [SLURP_DOCS.md](./SLURP_DOCS.md) | The language reference: every tag, filter, operator and CLI flag, with the gaps marked |
| [SECURITY.md](./SECURITY.md) | Threat model, what the escaping does and does not cover, and how to report a vulnerability |
| [PERFORMANCE.md](./PERFORMANCE.md) | Performance characteristics and the enforced resource budgets |
| [CONTRIBUTING.md](./CONTRIBUTING.md) | How to build it, the gates a change has to pass, and the conventions |
| [CHANGELOG.md](./CHANGELOG.md) | What changed, per release |

## Repository layout

```
compiler/          Rust: lexer, parser, renderer, WASM bindings, the `slurp` CLI
dev-server/        Rust: file watcher, WebSocket hot reload, fixtures, backend proxy
image/             Rust: on-demand image service with SSRF protection
runtime/           TypeScript: @slurp/runtime, the optional browser bundle
language/          TypeScript: TextMate grammar, completions, hover, Prettier plugin
vscode-extension/  TypeScript: the VS Code extension
shared/            interfaces.ts, the TypeScript view of the compiler's wire shapes
examples/          Sample themes (see the status note above)
tests/integration/ Cross-package tests
```

## Building from source

**Prerequisites: Rust 1.88 or newer, Node.js 20 or newer, pnpm 9 or newer.**

The Rust floor is worth stating plainly, because a too-old toolchain does not give you a version message. Slurp uses edition 2024, and on Rust 1.84 or below `cargo build` fails with `feature 'edition2024' is required`, which reads like a broken project rather than an old compiler. `rust-toolchain.toml` pins 1.88, so if you use rustup the right toolchain is selected for you. If you get that error, run `rustup update`.

1.88 rather than the 1.85 that edition 2024 needs, because the `image` crate that the image service depends on requires it.

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

pnpm install
pnpm build
pnpm test
```

`pnpm build` includes the WebAssembly build of the compiler, so it needs
[`wasm-pack`](https://rustwasm.github.io/wasm-pack/installer/) on your `PATH`
(`cargo install wasm-pack --locked`). The `wasm32-unknown-unknown` target is
added for you if it is missing. To build only that package:

```bash
pnpm --filter @slurp/compiler-wasm build
```

The result is `@slurp/compiler-wasm`, the browser and editor API: `parse`,
`render`, `render_dev`, `validate`, `validate_full`, `extract_schema` and
`render_section`. Every one of them returns a JSON **string** that the caller
parses. `validate_full` is the one that also runs the compile-time security
walk, so it is what an authoring tool should call for parity with `slurp build`.
See [compiler-wasm/README.md](./compiler-wasm/README.md).

## Editor support

`vscode-extension/` holds a VS Code extension providing syntax highlighting, inline diagnostics, completions, hover docs and formatting. It is not on the Marketplace yet; build it locally with `pnpm -C vscode-extension build`.

Be aware that the editor tooling and the compiler currently disagree on a few points of syntax. Where they differ, the compiler is right and [SLURP_DOCS.md](./SLURP_DOCS.md) follows the compiler.

## Contributing

Bug reports and pull requests are welcome. Start with [CONTRIBUTING.md](./CONTRIBUTING.md), and please read [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md). For anything security-related, do not open a public issue: use GitHub's private vulnerability reporting, as described in [SECURITY.md](./SECURITY.md).

## Getting help

1. **Check [Known gaps in 0.1.0](#known-gaps-in-010) first.** Most surprises in this release are listed there, and the runtime ones fail silently rather than with an error, so they are hard to guess at.
2. **[SLURP_DOCS.md](./SLURP_DOCS.md) is the reference**, and it marks the gaps inline. Its "Gotchas" section covers the traps that produce confusing errors, such as a comment inside frontmatter breaking the file.
3. **A compile error you cannot place** is usually one of: a quoted component prop being a literal string (components take `prop={expr}`), a `"` nested inside a `${ }` in an attribute, or `select(...)` options being given as literals rather than identifiers. All three are in the docs.
4. **Otherwise open an issue** at [github.com/bytesell/slurp/issues](https://github.com/bytesell/slurp/issues), with the template that reproduces it. A reduced `.slurp` file is worth more than a description, because the compiler is total and tolerant, so most bugs show up as unexpected output rather than as a message.

## A note on names

The crates.io name `slurp` and the npm name `slurp` are both taken by unrelated packages. This project publishes as `slurp-compiler` on crates.io and `@slurp/runtime` on npm.

**The installed binary is still called `slurp`, deliberately.** `cargo install slurp-compiler` puts a `slurp` on your PATH.

That is safe rather than merely convenient, because the crates.io `slurp` crate is a **library with no binary target**: 42 lines of file-reading helpers, last published in 2017. `cargo install slurp` on it fails with "there is nothing to install", so nothing that crate can do will ever put a competing `slurp` on your PATH. The name is only taken as a *registry* name, which is why the crate here is `slurp-compiler`; the *binary* name was never contested.

The npm `slurp` package is a separate ecosystem and cannot collide with a PATH binary either.

The residual risk is an unrelated local tool of the same name, which is true of any short binary name and is the user's to resolve. If you hit it, `cargo install --bin slurp slurp-compiler` and a shell alias, or rename on your own PATH.

## License

[MIT](./LICENSE).
