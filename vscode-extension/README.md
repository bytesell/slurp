# Slurp for VS Code

Language support for [Slurp](https://github.com/bytesell/slurp) templates
(`.slurp` files): syntax highlighting, completions, hover docs, go to definition
and formatting.

Slurp is a lightweight templating engine. Pages, components, layouts and slots
compile to plain HTML with almost no client JavaScript, and schema-driven
sections let non-technical users edit a page safely.

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

## Features

- **Syntax highlighting.** TextMate grammar covering frontmatter, control
  blocks, inline expressions, filters, components, and HTML attributes carrying
  Slurp directives.
- **Completions** for keywords (`if`, `each`, `fetch`, `match`, `repeat`,
  `try`), filters (`upper`, `currency`, `sort`, ...), loop variables
  (`loop.index`, `loop.first`, ...), globals (`seo.title`, `params`, `env.dev`,
  ...) and common snippets.
- **Hover documentation** on any keyword or filter.
- **Go to definition.** `F12` on a PascalCase component name opens its `.slurp`
  file.
- **Formatting.** Indents control blocks. Runs on save when
  `slurp.formatOnSave` is on.

## Diagnostics do not work in 0.1.0

`slurp.validateOnType` and `slurp.validateOnSave` default to `true` and
`Slurp: Validate File` is in the Command Palette, so the feature looks live. It
is not, and it fails silently: a file full of parse errors shows no squiggles
and nothing is logged.

`getWasmValidator` in `src/extension.ts` reaches the compiler with
`require('@slurp/compiler-wasm')`, which is not a declared dependency of this
extension. The require throws, the `catch` returns a no-op yielding an empty
array, and that no-op is the only fallback path. Nothing else takes over.

Highlighting, completions, hover and go to definition come from
`@slurp/language`, which **is** a declared dependency, so those work. Until
diagnostics are fixed, validate from the command line:

```bash
slurp validate --dir . --warnings
```

Add `--middleware <DIR>` if the project has middleware, or every correct
`request.*` in it reports a scope violation.

## Install

Search for **Slurp** in the Extensions panel, or:

```bash
# from a built package
pnpm run package
code --install-extension slurp-vscode-0.1.0.vsix
```

To hack on it, run `pnpm install && pnpm run build` in `vscode-extension/`, then
press `F5` to launch the Extension Development Host.

## Settings

| Setting | Default | What it does |
|---|---|---|
| `slurp.formatOnSave` | `true` | Format `.slurp` files on save. |
| `slurp.validateOnType` | `true` | Diagnostics as you type, debounced 300 ms. See above: currently inert. |
| `slurp.validateOnSave` | `true` | Diagnostics on save. Currently inert. |
| `slurp.componentsPath` | `"./components"` | Where go to definition looks for components, relative to the workspace root. Absolute paths and paths containing `..` are ignored. |

## Commands

`Slurp: Validate File` and `Slurp: Format Document`, both in the Command
Palette.

## Language reference

The full syntax, including the constructs that compile cleanly and then render
wrong, is in
[SLURP_DOCS.md](https://github.com/bytesell/slurp/blob/main/SLURP_DOCS.md).

The editor tooling and the compiler disagree on a few points of syntax. Where
they differ, the compiler is right.

## Requirements

VS Code `^1.85.0`. Node `>=20` to build from source.

## License

MIT, ByteSell LLC.
