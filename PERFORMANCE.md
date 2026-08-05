# Performance and resource limits

What each subsystem costs, and what bounds it. There is no committed benchmark suite, so treat every timing here as an expectation to be measured on your own deployment rather than as a result. The limits, by contrast, are constants in the source and are exact.

## Compiler

### Parser

The lexer produces a flat `Vec<SpannedToken>` in one forward pass. The parser walks that vector with a cursor and never rewinds, so parsing is O(n) in source length with a small constant, with no backtracking. Errors do not stop the walk: `ErrorAccumulator` collects them and the parser continues.

### Resource limits

Templates can be authored by untrusted parties, so the compiler enforces hard limits rather than trusting input. The renderer charges every loop against a global iteration budget, every intermediate filter result against a memory budget, and every emitted byte against an output budget; the lexer, parser and renderer each cap their own recursion separately.

| Limit | Constant | Value | Where | Purpose |
|---|---|---|---|---|
| Per-loop iterations | `MAX_ITERATIONS` | 1,000 | `renderer/mod.rs` | Bounds a single `{each}` / `{repeat}` |
| Total iterations | `MAX_TOTAL_ITERATIONS` | 1,000,000 | `renderer/mod.rs` | Bounds nested-loop fan-out |
| Single filter result | `MAX_FILTER_VALUE_BYTES` | 8 MiB | `renderer/mod.rs` | Bounds one intermediate value |
| Cumulative filter bytes | `MAX_FILTER_TOTAL_BYTES` | 64 MiB | `renderer/mod.rs` | Bounds all intermediates in one render |
| Output size | `MAX_OUTPUT_BYTES` | 16 MiB | `renderer/mod.rs` | Caps rendered HTML size |
| Render recursion | `MAX_RENDER_DEPTH` | 96 | `renderer/mod.rs` | Bounds component / layout nesting at render time |
| Parse recursion | `MAX_DEPTH` | 64 | `parser/mod.rs` | Rejects deeply nested input before stack exhaustion |
| Lexer token count | `MAX_TOKENS` | 2,000,000 | `lexer/mod.rs` | Bounds a pathological source file |
| Lexer mode nesting | `MAX_MODE_DEPTH` | 512 | `lexer/mod.rs` | Bounds context-sensitive mode stacking |
| Security-walk recursion | `MAX_CHECK_DEPTH` | 512 | `lib.rs` | Defence in depth over the parser's own cap |
| Import chain length | `MAX_IMPORT_CHAIN` | 128 | `lib.rs` | A deep chain is a stack overflow, which aborts rather than erroring |
| Block nesting | `MAX_BLOCK_DEPTH` | 64 | `sections.rs` | Bounds editor-supplied block trees |
| Blocks per type | `BLOCK_HARD_CEILING` | 200 | `sections.rs` | Default when a schema declares no `max` |
| Diagnostics per compile | `ErrorAccumulator::MAX_DIAGNOSTICS` | 1,000 | `error.rs` | Beyond this, diagnostics are dropped |

**`MAX_DEPTH` and `MAX_RENDER_DEPTH` are different constants with different values.** Parse depth is 64; render depth is 96.

**The two filter budgets stop the worst amplification in this codebase, so do not treat them as incidental.** Several filters expand their input: `json` roughly doubles a string, since it re-quotes and re-escapes something already quoted and escaped, and HTML escaping can grow a value sixfold. An unbounded chain of them is therefore exponential in the chain length. Capping any single intermediate is what makes 2^N unreachable, because the first value to cross the cap ends the chain. The cumulative cap closes the other shape, many chains each producing a value just under the per-value cap. Crossing either yields a typed diagnostic (`IterationLimitExceeded`), never a panic.

The three byte budgets are host-tunable through `renderer::MemoryBudget`; the values above are `MemoryBudget::DEFAULT`. They were picked against measured work rather than guessed: a whole site build peaks around 12 MB of live memory, recorded in the `MemoryBudget` doc comment.

### WASM binary size

`compiler-wasm/pkg/` is build output and is not committed, so reproduce these with `node compiler-wasm/build.mjs` and measure `pkg/slurp_compiler_bg.wasm` yourself. A release build comes out around 1.56 MiB raw and 595 KiB gzipped.

```bash
node compiler-wasm/build.mjs                # wasm-pack build --target nodejs --release
node compiler-wasm/build.mjs --target web   # for a browser host with a bundler
```

The default target is `nodejs`, so `pkg/` is CommonJS. **There is no `wasm-opt` step.** `wasm-pack` drives `wasm-bindgen` itself and nothing in this repository runs Binaryen, so the figures above are what a consumer downloads. Running `wasm-opt` over the output is a plausible way to shrink it and is not currently done.

## Image optimizer

### Concurrency

Decode, resize and encode are seconds of CPU work on a large image, so `process_image` dispatches them to `tokio::task::spawn_blocking` and never runs them on an async worker.

**The ordering of the two admission steps matters.** The transform permit (`--max-concurrent-transforms`, defaulting to one in-flight transform per core) is acquired **before** `spawn_blocking`, so a queued request occupies a cheap future rather than a blocking thread. Magic-byte validation runs before either: it is a header check, and rejecting there costs no permit and no thread at all.

### Two-level cache

| Level | Backend | Hit latency |
|---|---|---|
| L1, in-memory | [`moka`](https://crates.io/crates/moka), LRU eviction | < 1 ms |
| L2, disk | Content-addressed files keyed by `sha256(src + params)` | ~5 ms |

Those latencies are order-of-magnitude expectations for a map lookup against a small disk read, not measurements of this code, and an L2 hit scales with entry size.

Cache entries hold the transformed image bytes. The L1 map is `moka::future::Cache<String, Vec<u8>>` and `ImageCache::get` returns `Option<Vec<u8>>`, so **an L1 hit copies the whole entry onto the heap.** It avoids I/O and all codec work, which is where the saving is, but it is a memcpy of the encoded image and not a refcount bump. An L2 hit reads the sidecar `.meta` TTL file and then the data file, both with `std::fs` on `spawn_blocking` rather than `tokio::fs`, and then populates L1 with a further clone.

### Memory caps

Three caps apply and they are easy to conflate. The constants are in `Config::new` (`image/src/config.rs`) and the errors are variants of `SecurityError` (`image/src/security.rs`).

| Cap | Constant | Value | Error on breach |
|---|---|---|---|
| Source bytes fetched over the network | `max_source_bytes` | 20 MiB | `SourceTooLarge` |
| Decoded pixel buffer | `max_decoded_bytes` | 50 MiB | `DecompressionBomb` |
| Output pixel count after resize | `max_output_pixels` | 8,000,000 | `OutputTooLarge` |

The source cap is enforced twice in `fetch_with_limits`: once against the declared `Content-Length`, and again incrementally while streaming, because `Content-Length` can be absent or can lie.

The decoded cap is checked in `transform` **before any pixel buffer is allocated**, from the header's declared dimensions times the decoder's real bytes per pixel (8 for a 16-bit source, not a hardcoded 4), with saturating arithmetic so dimensions near `u32::MAX` cannot wrap to a small product. A decoder allocation limit backs it up in case the header understates the true cost.

### Resampling

Lanczos3 for every resize (`image::imageops::FilterType::Lanczos3`). It is sharper than bilinear and bicubic at equivalent cost and does not ring at the sharpness levels web images use.

## Runtime

### Bundle sizes

Gzipped, measured over a local `pnpm build`. `runtime/dist/` is build output and is not committed.

| Bundle | Budget | Measured |
|---|---|---|
| `slurp.core.js` | < 30 KB | 2,570 bytes |
| `slurp.fetch.js` | < 10 KB | 1,551 bytes |
| `slurp.infinite.js` | < 5 KB | 654 bytes |

**Alpine.js is not bundled.** `slurp.core.js` begins `import M from 'alpinejs'`, so Alpine is an external import that the consumer's bundler or import map resolves, and none of its weight is in these figures. `clsx` and `tailwind-merge` are external on the same footing. So the numbers are slurp's own code only, and the budgets have a great deal of room in them.

### Everything below this line is inert against the 0.1.0 compiler

**Read this before acting on anything in the rest of this section.** Client navigation, `{fetch}` reactivity and infinite scroll are implemented and unit tested in `@bytesell/slurp-runtime`, but they bind to markers the compiler does not generate:

| Feature | Marker it needs | Generated by the compiler? |
|---|---|---|
| Client navigation | `[data-slurp-page]` | No |
| Route params | `<meta name="slurp-route">` | No |
| `{fetch}` reactivity and polling | `x-data="slurpFetch(...)"` | No |
| Infinite scroll | `[data-slurp-sentinel]` | No |

The `data-slurp-*` attributes the renderer does emit are `data-slurp-let`, `data-slurp-debug`, `data-slurp-component` and `data-slurp-props`, plus `data-bs-block` around each child of a `{blocks}` tag. Client navigation additionally needs a bare `navigate` on `<body>`, which is author markup rather than generated (the shipped example theme writes it), but without `[data-slurp-page]` there is nothing to swap. So on a page built by `slurp build`, none of the mechanisms below is invoked. They are available to an **embedding host** that supplies those markers itself, which is the case slurp was extracted from. See `runtime/README.md`.

### Leak prevention

Both of these paths are reached only when a host supplies the markers above.

**IntersectionObserver cleanup.** Infinite scroll registers an `IntersectionObserver` on the sentinel element and disconnects it inside a `slurp:navigate` listener, so stale observers from previous pages do not accumulate.

**Fetch interval clearing.** Poll-mode `{fetch}` blocks register a `setInterval` whose ID is stored in Alpine component data as `_pollTimer` and cleared in **`destroy()` only** (`runtime/src/fetch/index.ts`), which Alpine calls on teardown.

**`slurp:navigate` does not stop polling.** The listener registered by `_setupAbortOnNavigate` aborts the in-flight request and nothing else; it never reads `_pollTimer`. A poll-mode block that survives a client navigation without being unmounted keeps polling.

### Navigation

`initNavigation` fetches the target page and swaps the `[data-slurp-page]` subtree via `doSwap`, so layout HTML (header, sidebar, footer) is not re-parsed into the live document. How much that saves depends on the ratio of page content to chrome in a given theme, and **no measurement of it exists here**. The fetch still transfers the whole document, so the saving is in parsing and re-execution, not in bytes on the wire.

There is no prefetching of any kind. Navigation is click-triggered only: no hover intent, no viewport prefetch, no speculation rules.

## Dev server

### File watcher

Debounced by **time only**. `spawn_watcher` (`dev-server/src/watcher.rs`) collects change events into a `HashSet<PathBuf>` and flushes after `DEBOUNCE_MS` (50 ms) of quiet, short enough to feel instant and long enough to batch the several inotify events one editor save produces.

**Deduplication is by path, not by content.** There is no mtime read, no hashing and no comparison of old against new bytes anywhere in the dev server, so touching a file with no changes still triggers a rebuild and a reload. Candidate paths are filtered by extension only, in `is_watched_extension`.

### Dependency graph

`DependencyGraph` (`dev-server/src/watcher.rs`) maintains forward and reverse edges between pages and the components and layouts they import. `invalidate` re-indexes the changed file and walks the reverse edges depth-first (the worklist is a `Vec` drained with `pop()`), and `process_changes` unions the results into an `all_affected` set. The traversal is exhaustive and `visited`-guarded, so order does not change the result.

What it drives is narrower than "incremental rebuild". Each affected `.slurp` path is run through `check_page`, which *parses* rather than renders, so the graph decides which files get re-checked for diagnostics rather than scoping a recompile of output. An affected path that is not `.slurp` contributes `"*"` to the broadcast page list alongside the specific pages, rather than replacing them.

Using the graph to re-render only the affected pages, and to send a targeted reload instead of a full one, is the unbuilt half.

### WebSocket broadcast

Hot-reload notifications go to every connected tab (`HmrMessage` in `dev-server/src/hmr.rs`, broadcast capacity 64). O(n) in connected clients, which in practice is one.

**The reload is always a full document reload.** The `reload` message carries a `pages` array, and the client (`runtime/src/core/dev.ts`) parses it and then calls `window.location.reload()` regardless of its contents. There is no CSS-only swap and no partial or module-level update, so `pages` is advisory.

## Benchmarks

There is no committed benchmark suite: no `benches/` directory, no criterion dependency and no `#[bench]` anywhere, so **no timing figure in this document is reproducible from anything committed here.** The compiler's cost model is O(n) in source length with no backtracking, and the enforced limits bound worst-case work regardless of input.

Two numbers here are measured rather than estimated: the ~12 MB peak live memory of a whole site build, which is the basis for the filter caps, and the bundle sizes above, taken with `gzip` over a local build.

To profile the compiler with `cargo-flamegraph`:

```bash
cargo flamegraph --package slurp-compiler --bin slurp -- build --input examples/basic-theme
```
