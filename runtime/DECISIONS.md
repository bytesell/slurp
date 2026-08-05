# Runtime build decisions

## Bundler: tsup

**Decision date:** 2026-03-27

tsup wraps esbuild and needs no configuration to emit typed ESM from
TypeScript. That is the whole reason: this package is three small browser
entry points, and every alternative wanted a plugin to do the same job.

The config that matters is in `tsup.config.ts`:

- Three entries (`slurp.core`, `slurp.fetch`, `slurp.infinite`), each bundled
  and tree-shaken on its own.
- `splitting: false`, deliberately. A page loads these with a plain
  `<script type="module">`, so each file has to be self-contained. Code
  splitting would emit a shared chunk that nothing on the page fetches.
- `format: ['esm']` only. See below.
- `dts: true`, so declarations come from `tsc` while the transpile stays on
  esbuild.

## Chunk contents

| Chunk | Contents |
|---|---|
| `dist/slurp.core.js` | Alpine.js, `cn()`, `parseParams()`, client navigation, HMR, Alpine init |
| `dist/slurp.fetch.js` | `slurpFetch()` Alpine data factory, cache, retry, polling |
| `dist/slurp.infinite.js` | IntersectionObserver sentinel management |

**Which chunks a page loads is the host's decision, not the compiler's.** The
compiler performs no AST analysis for chunk selection and injects no `<script>`
tags: it emits plain HTML and a consumer adds the tags it wants.
`RouteManifest` in `shared/interfaces.ts` describes a build-time chunk manifest,
but it is an interface declaration and nothing else. No Rust code constructs it
and no runtime code reads it.

There is nothing for a chunk analysis to detect yet anyway: the compiler emits
none of the markers `slurp.fetch.js` and `slurp.infinite.js` bind to. Selective
injection is a companion to that codegen when it lands.

## ESM only, `require()` is not supported

**Decision date:** 2026-08-01

`format: ['esm']` and `"type": "module"`, and every `exports` entry offers only
an `import` condition. A CommonJS consumer calling `require('@bytesell/slurp-runtime')`
gets `ERR_REQUIRE_ESM`. These modules are browser chunks loaded with
`<script type="module">` and they mutate `window` on import, so a CJS build
would create a second copy of that global state in any bundle mixing the two.

For the same reason `"sideEffects"` is deliberately **absent** rather than set
to `false`. `slurp.core.js` starts Alpine, `slurp.fetch.js` assigns
`window.slurpFetch`, and `slurp.infinite.js` starts observing sentinels, all as
a side effect of being imported. Declaring `"sideEffects": false` would license
a bundler to drop an entry point that was imported purely for its effect.

## `types` and `files`

Each `exports` entry carries a `types` condition, and there is a top-level
`types` for older resolvers. Without them `dts: true` still emitted the
declaration files but no TypeScript consumer could find them, so every import
resolved to "could not find a declaration file".

`files` is an allowlist (`dist`, `README.md`, `LICENSE`). Before it, `npm pack`
shipped `src/`, `tsconfig.json` and the `.map` files. That mattered beyond size:
`src/types.ts` re-exports from `../../shared/interfaces`, which is outside the
package root, so a consumer resolving from source would hit a missing module.
tsup's dts rollup inlines those shared types into `dist/*.d.ts`, so the built
artifact is self-contained, and shipping `dist` only is what makes that hold. If
`src/` is ever added back to `files`, `shared/` has to become a real package
first.

## `experimental/fetch` and `experimental/infinite`

**Decision date:** 2026-08-01

`slurp.fetch.js` and `slurp.infinite.js` are published as
`@bytesell/slurp-runtime/experimental/fetch` and `@bytesell/slurp-runtime/experimental/infinite`,
not as the shorter `./fetch` and `./infinite` the build output suggests.

The client half and the compiler half of these features do not meet. Both
modules are implemented and unit-tested, but they bind to markup markers
(`x-data="slurpFetch(...)"`, `[data-slurp-sentinel]`) that the 0.1.0 compiler
never emits. Client navigation and `parseParams` in `slurp.core.js` are in the
same position, needing `<body navigate>`, `[data-slurp-page]` and
`<meta name="slurp-route">`.

Against the shipped compiler these modules import cleanly, register their
globals, observe nothing and do nothing. The failure mode is silence, which is
the worst kind to ship on a public package: no error to search for, and no way
to tell a broken install from a feature that was never wired.

Deleting them was rejected. They work, they are tested, and they are the basis
of the codegen that will wire them up. Building that codegen is the real fix and
is a feature, not a release chore. Renaming the entry points was the option that
made the gap impossible to miss without throwing away working code.

A consumer who types `experimental/` has been told. An embedding host that
supplies the markers itself, which is the case slurp was extracted from, still
gets working `{fetch}` reactivity and infinite scroll, so nothing is taken away.

The modules, their tests and the tsup entry names are all unchanged; this is
purely the `exports` map. When the codegen lands, promote them to `./fetch` and
`./infinite`, keeping the `experimental/` paths as aliases for one release.
