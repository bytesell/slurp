# Runtime Build Tool Decisions

## Bundler: tsup (wraps esbuild)

**Decision date:** 2026-03-27

### Why tsup over alternatives

| Concern | tsup | rollup | webpack | vite (lib) |
|---|---|---|---|---|
| Tree-shaking for chunked output | Excellent | Excellent | Good | Good |
| TypeScript support | Zero-config | Requires plugin | Requires plugin | Zero-config |
| Build speed | Fastest (esbuild) | Slow | Slowest | Fast (esbuild) |
| ESM + CJS dual output | First-class | Requires config | Complex | Limited |
| DTS generation | Built-in | Requires plugin | N/A | Separate step |

**Reasons:**

1. **Best tree-shaking for chunked output** - Each of the three output chunks (`slurp.core`, `slurp.fetch`, `slurp.infinite`) is independently tree-shaken, and tsup's per-entry splitting keeps them genuinely separate rather than emitting one bundle with dead branches. That matters because a page that uses no `{fetch}` should never pay for `slurp.fetch.js`.

2. **Zero-config TypeScript support** - No `ts-loader`, no `@rollup/plugin-typescript`. tsup calls esbuild's TypeScript transpiler natively, with `tsc` used separately only for type declarations (`dts: true`).

3. **Faster than rollup for this use case** - esbuild is 10-100x faster than rollup's JS-based transform pipeline. For a runtime that ships to end users' browsers, fast iteration matters in development.

4. **First-class ESM + CJS dual output** - Although this runtime only ships ESM (`format: ['esm']`), tsup makes adding CJS a one-line change if a server-side rendering consumer needs it.

### Output chunk strategy

| Chunk | Contents |
|---|---|
| `dist/slurp.core.js` | Alpine.js, `cn()`, `parseParams()`, client navigation, HMR, Alpine init |
| `dist/slurp.fetch.js` | `slurpFetch()` Alpine data factory, cache, retry, polling |
| `dist/slurp.infinite.js` | IntersectionObserver sentinel management |

**Which chunks a page loads is the host's decision, not the compiler's.** The compiler performs no AST analysis for chunk selection and injects no `<script>` tags: it emits plain HTML, and a consumer adds the `<script type="module">` tags it wants. An earlier revision of this document described build-time chunk analysis recorded in a `RouteManifest.chunks` field, with the renderer injecting only the necessary tags. **None of that is implemented.** `RouteManifest` exists only as a TypeScript interface declaration in `shared/interfaces.ts`; no Rust code constructs it and no runtime code reads it.

That is consistent with the gap recorded further down this file: the compiler emits none of the markers `slurp.fetch.js` and `slurp.infinite.js` bind to, so there is nothing for a chunk analysis to detect yet. Selective injection is a natural companion to that codegen when it lands, not a thing that already works.

## Published package surface

**Decision date:** 2026-08-01

### ESM only - `require()` is not supported

`format: ['esm']` and `"type": "module"`, and every `exports` entry offers only
an `import` condition. A CommonJS consumer calling
`require('@slurp/runtime')` gets `ERR_REQUIRE_ESM`. That is deliberate: these
modules are browser chunks loaded with `<script type="module">`, and they mutate
`window` on import. Adding a CJS build would create a second copy of that
global state in any bundle that mixed the two.

For the same reason `"sideEffects"` is deliberately **absent** rather than set
to `false`. `slurp.core.js` starts Alpine, `slurp.fetch.js` assigns
`window.slurpFetch`, and `slurp.infinite.js` starts observing sentinels, all as
a side effect of being imported. Declaring `"sideEffects": false` would license
a bundler to drop an entry point that was imported purely for its effect.

### `types` and `files`

Each `exports` entry carries a `types` condition, and there is a top-level
`types` for older resolvers. Without them, `dts: true` still emitted the
declaration files but no TypeScript consumer could find them, so every import
resolved to "could not find a declaration file".

`files` is an allowlist (`dist`, `README.md`, `LICENSE`). Before it, `npm pack`
shipped `src/`, `tsconfig.json` and the `.map` files. That mattered beyond size:
`src/types.ts` re-exports from `../../shared/interfaces`, which is OUTSIDE the
package root, so a consumer resolving from source would hit a missing module.
`tsup`'s dts rollup inlines those shared types into `dist/*.d.ts`, so the built
artifact is self-contained - shipping `dist` only is what makes that guarantee
hold. If `src/` is ever added back to `files`, `shared/` has to become a real
package first.

### `experimental/` entry points, and why they are not `./fetch` and `./infinite`

**Decision date:** 2026-08-01

`slurp.fetch.js` and `slurp.infinite.js` are published as
`@slurp/runtime/experimental/fetch` and `@slurp/runtime/experimental/infinite`,
not as the shorter `./fetch` and `./infinite` the build output would suggest.

The reason is that the client half and the compiler half of these features do
not meet. Both modules are implemented and unit-tested, but they bind to markup
markers (`x-data="slurpFetch(...)"`, `[data-slurp-sentinel]`) that the 0.1.0
compiler never emits. Client navigation and `parseParams` in `slurp.core.js`
are in the same position, needing `<body navigate>`, `[data-slurp-page]` and
`<meta name="slurp-route">`.

So against the shipped compiler these modules import cleanly, register their
globals, observe nothing and do nothing. **The failure mode is silence**, which
is the worst kind to ship on a public package: there is no error to search for
and no way to tell a broken install from a feature that was never wired.

Three options were considered:

1. **Delete the modules.** Rejected. They work, they are tested, and they are
   the basis of the codegen that will wire them up. Deleting tested code to
   express a documentation fact is a bad trade.
2. **Build the codegen.** That is the real fix, and it is a feature, not a
   release chore. Out of scope for 0.1.0.
3. **Rename the entry points so the gap is impossible to miss.** Chosen.

A consumer who types `experimental/` has been told. An embedding host that
supplies the markers itself - the case slurp was extracted from - still gets
working `{fetch}` reactivity and infinite scroll, so nothing is taken away.

The modules, their tests and the tsup entry names are all unchanged; this is
purely the `exports` map. When the codegen lands, promote them back to `./fetch`
and `./infinite`, which is an additive change for anyone already on the
`experimental/` path only if the old paths are kept as aliases for a release.
