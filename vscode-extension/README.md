# Slurp - VS Code Extension

Language support for [Slurp](https://github.com/bytesell/slurp) templates (`.slurp` files).

---

## What is Slurp?

Slurp is a lightweight, agent-first templating engine. Pages, components, layouts and slots compile to plain HTML with almost no client JavaScript, and schema-driven sections let non-technical users edit a page safely. The nearest mental model is a lightweight Astro.

Templates are plain text files with a `.slurp` extension. They combine HTML with a block-tag syntax for control flow, async data fetching, pattern matching, and typed component composition - without JavaScript in the template itself.

Slurp is at `0.1.0` and its API is not yet stable.

```slurp
---
using "@components/Card"

props {
  title: string
}
---

<h1>${ title }</h1>

{fetch products: Product[] from "/api/products" cache(300)}
  {each p in products | sort("price")}
    <Card title={p.name} price={p.price} />
  {/each}
{loading}
  <p>Loading...</p>
{error}
  <p>Error: ${ error.message }</p>
{/fetch}
```

Two things that catch everyone in that snippet: `{loading}` and `{error}` are
branch markers, not blocks, so they take **no** closing tag and each runs to the
enclosing `{/fetch}`. And `{fetch}` options parse only in the parenthesised call
form: `cache(300)`, never `cache:300`. A colon-form option is silently dropped
with no error.

---

## Features

- **Syntax highlighting** - Full TextMate grammar covering frontmatter, control blocks, inline expressions, filters, components, and HTML attributes with Slurp directives.
- **Completions** - IntelliSense for keywords (`if`, `each`, `fetch`, `match`, `repeat`, `try`), filters (`| upper`, `| currency`, `| sort`, ...), loop variables (`loop.index`, `loop.first`, ...), global variables (`seo.title`, `params`, `env.dev`, ...), and common snippets.
- **Hover documentation** - Hover over any keyword or filter to see its syntax and description.
- **Go to Definition** - `Cmd+Click` (or `F12`) on a PascalCase component name to jump to its `.slurp` file.
- **Diagnostics** - Error and warning squiggles as you type and on save, driven by the real compiler through `@slurp/compiler-wasm`. **This is currently inert in a published build.** See the note below.
- **Document formatting** - Auto-indents control blocks correctly. Runs on save when `slurp.formatOnSave` is enabled.
- **Commands** - `Slurp: Validate File` and `Slurp: Format Document` available in the Command Palette.

### Known: diagnostics report nothing

`slurp.validateOnType` and `slurp.validateOnSave` both default to `true`, and
`Slurp: Validate File` is in the Command Palette, so the feature looks live. It
is not. `getWasmValidator` in `src/extension.ts` reaches the compiler with
`require('@slurp/compiler-wasm')`, but **the extension does not declare that
package as a dependency**, so the require throws, the `catch` returns a no-op
that yields an empty array, and you get zero squiggles on every file forever
with nothing logged. A file full of parse errors looks clean.

**There is no fallback to a second parser.** The no-op IS the fallback path, and
it cannot report anything. Earlier revisions of this file described "WASM when
available, with graceful fallback", which reads as though something else takes
over. Nothing does.

Syntax highlighting, completions, hover and Go to Definition come from
`@slurp/language`, which **is** a declared dependency, so those work. Until this
is fixed, validate from the CLI: `slurp validate --dir . --warnings`, adding
`--middleware <DIR>` if the project has middleware.

---

## Installation

### From the Marketplace

Search for **Slurp** in the VS Code Extensions panel, or install from the [Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=slurp.slurp-vscode).

### From VSIX (manual)

```bash
# Build the extension package
pnpm run package

# Install into VS Code
code --install-extension slurp-vscode-0.1.0.vsix
```

### From source (development)

```bash
cd vscode-extension
pnpm install
pnpm run build

# Open in Extension Development Host
# Press F5 in VS Code with this folder open
```

---

## Configuration

All settings live under the `slurp` namespace in VS Code settings (`Cmd+,`).

| Setting | Type | Default | Description |
|---|---|---|---|
| `slurp.formatOnSave` | `boolean` | `true` | Auto-format `.slurp` files when you save. |
| `slurp.validateOnType` | `boolean` | `true` | Show diagnostics as you type (debounced 300 ms). |
| `slurp.validateOnSave` | `boolean` | `true` | Run validation when you save. |
| `slurp.componentsPath` | `string` | `"./components"` | Path to your components directory, used by Go to Definition. Relative to the workspace root. |

Example `settings.json`:

```json
{
  "slurp.formatOnSave": true,
  "slurp.validateOnType": true,
  "slurp.componentsPath": "./src/components"
}
```

---

## Commands

Open the Command Palette (`Cmd+Shift+P`) and search for:

| Command | Description |
|---|---|
| `Slurp: Validate File` | Run diagnostics on the active `.slurp` file immediately. |
| `Slurp: Format Document` | Format the active `.slurp` file. |

---

## Screenshots

> _Screenshots coming soon._

---

## Language Syntax Quick Reference

### Frontmatter

```slurp
---
using "@components/Card"
using "@components/Button" as Btn

type Product {
  id: number
  name: string
  price: number
}

props {
  heading: string
  products: Product[]
}
---
```

### Control flow

```slurp
{if user.isLoggedIn}
  <p>Welcome, ${ user.name | upper }!</p>
{else if user.isPending}
  <p>Account pending.</p>
{else}
  <p>Please sign in.</p>
{/if}
```

### Loops

```slurp
{each product in products | sort("price") | limit(10)}
  <p>${ loop.count }. ${ product.name } - ${ product.price | currency }</p>
{empty}
  <p>No products found.</p>
{/each}
```

### Async data

```slurp
{fetch orders: Order[] from "/api/orders" cache(60) retry(3)}
  {each o in orders}
    <li>${ o.id }</li>
  {/each}
{loading}
  <p>Loading orders...</p>
{error}
  <p>Failed: ${ error.message } (${ error.status })</p>
{empty}
  <p>No orders yet.</p>
{/fetch}
```

Branch order is fixed: body, then `{loading}`, then `{error}`, then `{empty}`.
Branches written out of order are not recovered, and `{/loading}`, `{/error}`
and `{/empty}` are all parse errors.

### Pattern matching

```slurp
{match order.status}
  "shipped"   -> <span class="green">Shipped</span>
  "pending"   -> <span class="yellow">Pending</span>
  "cancelled" -> <span class="red">Cancelled</span>
  _           -> <span>Unknown</span>
{/match}
```

### Components

```slurp
<Card title="Hello" count={42} />

<Modal>
  <p>Modal body content here.</p>
</Modal>
```

---

## Requirements

- VS Code `^1.85.0`
- Node.js `>=20` (for the build toolchain)
- `@slurp/language` - bundled automatically via the workspace dependency

---

## Contributing

See [CONTRIBUTING.md](https://github.com/bytesell/slurp/blob/main/CONTRIBUTING.md) in the Slurp repository.

---

## License

MIT
