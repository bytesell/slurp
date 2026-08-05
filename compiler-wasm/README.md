# @slurp/compiler-wasm

The WebAssembly build of the Slurp compiler. It exposes the seven entry points in
`compiler/src/wasm/mod.rs` to JavaScript hosts: an editor, a preview pane, a
language server, or a browser.

## Build

From a fresh clone, from the repository root:

```bash
pnpm install
pnpm --filter @slurp/compiler-wasm build
```

`pnpm build` at the root also builds it, ahead of the packages that consume it.

The build needs [`wasm-pack`](https://rustwasm.github.io/wasm-pack/installer/)
on `PATH` (`cargo install wasm-pack --locked`). The `wasm32-unknown-unknown`
rustup target is installed automatically if it is missing. Output lands in
`pkg/`, which is a build artifact and is not committed.

To build for a browser bundler instead of Node:

```bash
node build.mjs --target web
```

## Everything returns a JSON string

`JsValue::from_str` is how each export serialises, so **every function returns a
`string` that you must `JSON.parse`**. It does not return an object. Older
documentation in this repo claimed otherwise and the VS Code extension carries a
comment about being bitten by it.

The TypeScript declarations encode this: an export returns `JsonString<T>`, a
branded `string` that will not typecheck as a `T`. Cross the boundary with the
exported `parseJson` helper.

```ts
import { parse, parseJson } from '@slurp/compiler-wasm'

const result = parseJson(parse('<h1>${title}</h1>'))
//    ^ ParseResult
if (!result.ok) console.error(result.errors)
```

The parsed shapes (`ParseResult`, `RenderResult`, `SchemaResult`, `SlurpError`)
come from `shared/interfaces.ts`, so there is one definition of each rather than
a copy per consumer.

## Exports

| Export | Returns | Notes |
|---|---|---|
| `parse(source)` | `ParseResult` | `ast` is `null` only when lexing failed outright |
| `render(astJson, contextJson)` | `RenderResult` | production mode |
| `render_dev(astJson, contextJson)` | `RenderResult` | development mode; see below |
| `validate(source)` | `SlurpError[]` | parse diagnostics only, a flat array of errors and warnings |
| `validate_full(source, isMiddleware)` | `SlurpError[]` | `validate` **plus the compile-time security walk**; see below |
| `extract_schema(source)` | `SchemaResult` | `schema` is `null` when none is declared, which is not an error |
| `render_section(astJson, stateJson, contextJson)` | `RenderResult` | merges saved editor state over schema defaults |

### `render` vs `render_dev`

Three of the renderer's diagnostics are recorded only in development mode: a
loop truncated at the 1,000-item cap, CSS-structural characters stripped from a
`style` value, and an un-annotated interpolation in a JavaScript-evaluated
attribute. In production mode all three happen silently.

That is the right trade for server-side rendering and the wrong one for an
editor, where a loop quietly losing its tail is precisely what a preview should
surface. Editors and preview panes should call `render_dev`. `render` is
unchanged, so existing hosts keep production behaviour.

### `validate` vs `validate_full`

**Prefer `validate_full` if you are building an authoring tool, a publish gate or
a CI check.** These two do not report the same thing, and the difference is not
cosmetic.

`validate` parses. `validate_full` parses **and** runs the compiler's
compile-time security walk (`slurp_compiler::check_security`), which is where
several rules live that the parser knows nothing about:

- an unfiltered `${ }` in a `<script>` body,
- a `| js` slot in JavaScript statement position, where escaping the quote
  characters cannot contain a value,
- `env.SLURP_SECRET_*`, in any file,
- `request.*` outside middleware,
- `{redirect}` and `{next}` outside middleware.

A tool that calls only `validate` therefore tells an author their file is clean
and then `slurp build` rejects it. The two are kept separate rather than merged
so that pure editor-preview hosts stay unaffected.

Two things to know at the call site:

- **`isMiddleware` must match the value the eventual compile will use.** The
  rules genuinely differ: `request.*` and `{redirect}` are legal only in
  middleware, and most other path roots are illegal there. A caller with no way
  to know passes `false`, which is what `compile`, `compile_with_registry` and
  the `slurp validate` CLI all do.
- **The security walk returns on its first violation** (it is a `Result`, not an
  accumulator), so at most one security diagnostic is appended per call, while
  parse diagnostics accumulate. Fix the reported one and re-run to see the next.

## Security

**`parse`, `render`, `render_dev`, `validate`, `extract_schema` and
`render_section` do not run `security_check`; `validate_full` does.**
`SLURP_SECRET_*` access, middleware scope and redirect restrictions are
server-side concerns enforced by the host, and a template that violates them
parses cleanly through the first six. That is deliberate, because a preview is
not a deploy.

A WASM **render** enforces none of those rules either. It does still apply the
renderer's own script-context backstops, so an undeclared `${ }` in a `<script>`
body and a false `| js` claim are caught there, but do not use a WASM render as a
production render path.

See the note at the top of `compiler/src/wasm/mod.rs` and `SECURITY.md`.
