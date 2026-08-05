# @bytesell/slurp-runtime

The optional browser runtime for [Slurp](https://github.com/bytesell/slurp) themes.

It is optional in the strong sense: a page that does not use client navigation,
`{fetch}` refetching or infinite scroll needs no JavaScript from slurp at all.
The compiler's output is plain HTML.

```bash
pnpm add @bytesell/slurp-runtime
```

```html
<script type="module" src="/path/to/slurp.core.js"></script>
```

ESM only. These modules are browser chunks that mutate `window` on import, so
`require()` is not supported and there is no CommonJS build. See
[DECISIONS.md](./DECISIONS.md) for why.

## What works against the 0.1.0 compiler

`@bytesell/slurp-runtime` (the `.` entry, `dist/slurp.core.js`) does four things on load:

| Feature | State |
|---|---|
| Bundles and boots Alpine.js (`window.Alpine`) | Works |
| Exposes `cn()` globally for inline Alpine expressions | Works |
| Dev-mode HMR client (when `window.SLURP_MODE === 'dev'`) | Works, paired with `slurp-dev` |
| `parseParams()` -> `window.slurpParams` | Implemented, but see below |
| Client navigation | Implemented, but see below |

Each boot step is isolated in a `try`/`catch`, so a failure in one cannot stop
Alpine from starting.

## What is implemented but NOT wired to the compiler

**Read this before you file a bug that says nothing happens.**

Several features are fully implemented and unit-tested on the client side, but
the 0.1.0 compiler does not emit the markup markers they bind to. Against the
shipped compiler they attach to nothing and silently do no work.

| Feature | Marker it needs | Emitted by the compiler? |
|---|---|---|
| Client navigation | `<body navigate>` and `<main data-slurp-page>` | No |
| Route params (`parseParams`) | `<meta name="slurp-route">` | No |
| `{fetch}` reactivity, caching, retry, polling | `x-data="slurpFetch(...)"` | No |
| Infinite scroll | `[data-slurp-sentinel]` | No |

The server side of client navigation is likewise unimplemented: the runtime
sends `X-Slurp-Navigate: 1` on a client navigation request, and nothing in this
repository reads that header.

Because of this, the two extra entry points are published under an
`experimental/` prefix rather than as `./fetch` and `./infinite`:

```js
import '@bytesell/slurp-runtime/experimental/fetch'      // window.slurpFetch
import '@bytesell/slurp-runtime/experimental/infinite'   // IntersectionObserver sentinels
```

The prefix is the warning. It is deliberately not something you can reach by
accident, because the failure mode is silence rather than an error.

### Why ship them at all

They are useful today to an **embedding host**, which is the case slurp was
extracted from. A host that renders slurp output and injects these markers
itself gets working client navigation, `{fetch}` reactivity and infinite
scroll now. If you control the HTML around the compiled output, supply the
markers in the table above and the modules do their jobs.

What is missing is only the compiler codegen that would emit those markers
automatically from `{fetch}`, `paginate`, `infinite` and route declarations.
That codegen is a feature, not a bug fix, and it is not in 0.1.0. When it
lands, these move to `./fetch` and `./infinite` and this section goes away.

Related gaps, all documented inline in
[SLURP_DOCS.md](../SLURP_DOCS.md#typescript-runtime): `{$let}` has no hydrator
(nothing reads `data-slurp-let`), and the runtime registers zero Alpine magics
and zero Alpine directives.

One trap that is not a gap. `navigate=false` on a link, unquoted, compiles to
`navigate="false"`, which is the spelling `navigation.ts` checks, and it works.
The **quoted** spelling `navigate="false"` does not: the parser only recognises
the bare-word form, so the quoted one compiles to a bare `navigate` attribute
and the link stays opted in. Write it unquoted.

## Development

```bash
pnpm install
pnpm --filter @bytesell/slurp-runtime build       # tsup -> dist/
pnpm --filter @bytesell/slurp-runtime test        # vitest
pnpm --filter @bytesell/slurp-runtime typecheck   # tsc --noEmit
```

## License

[MIT](./LICENSE). Copyright (c) 2026 ByteSell LLC.
