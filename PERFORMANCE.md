# Slurp Performance

This document describes the performance characteristics and resource limits of
each subsystem in the Slurp framework. It reflects what the code does today;
anything not yet implemented is called out explicitly under "Roadmap".

---

## Compiler Performance

### Parser

| Property | Detail |
|---|---|
| Algorithm | Hand-rolled recursive descent |
| Time complexity | O(n) - linear in source length |
| Backtracking | None - each token is consumed exactly once |
| Error recovery | Continues after errors via `ErrorAccumulator` |

The lexer produces a flat `Vec<SpannedToken>` in a single forward pass. The
parser walks that vector with a cursor (`pos: usize`) and never rewinds, so the
total parse cost is O(n) with a small constant.

### Resource limits (DoS budgets)

Because templates can be authored by untrusted parties, the compiler enforces
hard limits rather than trusting input. The renderer charges every loop against
a global iteration budget, every intermediate filter result against a memory
budget, and every emitted byte against an output budget; the lexer, parser and
renderer each cap their own recursion separately.

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

**`MAX_DEPTH` and `MAX_RENDER_DEPTH` are two different constants and are not the
same number.** Earlier revisions of this document named `MAX_DEPTH` for both.

**The two filter budgets are the ones that stop the worst amplification found in
this codebase, so do not treat them as incidental.** Several filters expand their
input - `json` roughly doubles a string, since it re-quotes and re-escapes
something already quoted and escaped, and HTML escaping can grow a value sixfold
- so an unbounded chain of them is exponential in the chain length. Capping any
single intermediate is what makes 2^N unreachable: the first value to cross the
cap ends the chain. The cumulative cap closes the other shape, many chains each
producing a value just under the per-value cap. Crossing either yields a typed
diagnostic (`IterationLimitExceeded`), never a panic.

All three of the renderer's byte budgets are host-tunable through
`renderer::MemoryBudget`; the values above are `MemoryBudget::DEFAULT`. They were
picked against measured work rather than guessed: a whole site build peaks around
12 MB of live memory.

### WASM Binary Size

Measured on the committed `compiler-wasm/pkg/slurp_compiler_bg.wasm`:

| Property | Value |
|---|---|
| Binary size (raw) | ~1.56 MiB (1,637,172 bytes) |
| Binary size (gzipped) | ~595 KiB (609,075 bytes at default gzip; 606,661 at `-9`) |
| Default target | `nodejs`, so `pkg/` is CommonJS |
| Post-processing | None |

Build pipeline (`compiler-wasm/build.mjs`, run by that package's `build` script):

```bash
node compiler-wasm/build.mjs             # wasm-pack build compiler --target nodejs --release
node compiler-wasm/build.mjs --target web   # for a browser host with a bundler
```

**There is no `wasm-opt` step and no `wasm-bindgen` invocation of our own.**
`wasm-pack` drives `wasm-bindgen` itself, and nothing in this repository runs
Binaryen. Earlier revisions of this document described a hand-rolled
`cargo build` plus `wasm-bindgen --target web` plus `wasm-opt -O3
--enable-bulk-memory` pipeline that has never existed here, and asserted a
"< 500 KB gzipped" size the artifact does not meet, being about 20 percent over
it. Running `wasm-opt` over
the output is a plausible way to close some of that gap; it is not currently
done, so treat the figures above as what a consumer downloads today.

---

## Image Optimizer Performance

### Parallel Processing

The image optimizer (`slurp-image`) uses Tokio's async runtime to process
multiple images concurrently. Decode, resize and encode are seconds of CPU work
on a large image, so `process_image` dispatches them to
`tokio::task::spawn_blocking` and never runs them on an async worker.

**Concurrency is bounded, and the ordering of the two steps matters.** The
transform permit (`--max-concurrent-transforms`, defaulting to one in-flight
transform per core) is acquired **before** `spawn_blocking`, so a queued request
occupies a cheap future rather than a blocking thread. Magic-byte validation runs
**before** either: it is a cheap header check, and rejecting there means a bad
request costs no permit and no thread at all.

### Two-Level Cache

| Level | Backend | Hit latency |
|---|---|---|
| L1 - in-memory | [`moka`](https://crates.io/crates/moka), LRU eviction | < 1 ms |
| L2 - disk | Content-addressed files keyed by `sha256(src + params)` | ~5 ms |

Cache entries store the transformed image bytes. The L1 map is
`moka::future::Cache<String, Vec<u8>>` and `ImageCache::get` returns
`Option<Vec<u8>>`, so **a hot L1 hit copies the whole entry onto the heap.** It
avoids I/O and all codec work, which is where the saving is, but it is a
memcpy of the encoded image and not a refcount bump. Earlier revisions of this
document called it "a single `Arc<Bytes>` clone"; there is no `Bytes` type
anywhere in the crate, and the only `Arc` on this path wraps the disk-cache
configuration. An L2 hit reads the sidecar `.meta` TTL file and then the data
file, both with `std::fs` on `spawn_blocking` rather than `tokio::fs`, and then
populates L1 with a further clone.

### Memory Safety

Three separate caps apply, and they are easy to conflate. Their constants are in
`Config::new` (`image/src/config.rs`) and their errors are variants of
`SecurityError` (`image/src/security.rs`):

| Cap | Constant | Value | Error on breach |
|---|---|---|---|
| Source bytes fetched over the network | `max_source_bytes` | 20 MiB | `SourceTooLarge` |
| Decoded pixel buffer | `max_decoded_bytes` | 50 MiB | `DecompressionBomb` |
| Output pixel count after resize | `max_output_pixels` | 8,000,000 | `OutputTooLarge` |

The source cap is enforced twice in `fetch_with_limits`: once against the
declared `Content-Length`, and again incrementally while streaming, because
`Content-Length` can be absent or can lie.

The decoded cap is checked in `transform` **before any pixel buffer is
allocated**, from the header's declared dimensions times the decoder's real
bytes per pixel (8 for a 16-bit source, not a hardcoded 4), with saturating
arithmetic so dimensions near `u32::MAX` cannot wrap to a small product. A
decoder allocation limit backs it up in case the header understates the true
cost.

There is no `ImageTooLarge` error. Earlier revisions of this document named one,
and merged the 20 MiB source cap with the 50 MiB decoded cap as though they were
a single limit.

### Codec Selection

The Lanczos3 resampling filter is used for all resize operations. It provides
the best quality/performance tradeoff for photographic web images:

- Sharper than bilinear and bicubic at equivalent cost
- No ringing artifacts at the sharpness levels used in web images
- Supported natively by the `image` crate's `FilterType::Lanczos3`

---

## Runtime Performance

### Bundle Size Targets

| Bundle | Target (gzipped) | Measured (gzipped) |
|---|---|---|
| `slurp.core.js` | < 30 KB | 2,570 bytes |
| `slurp.fetch.js` | < 10 KB | 1,551 bytes |
| `slurp.infinite.js` | < 5 KB | 654 bytes |

**Alpine.js is not bundled.** `runtime/dist/slurp.core.js` begins
`import M from 'alpinejs'`, so Alpine is an external import that the consumer's
bundler or import map resolves, and none of its weight is inside these figures.
The measured numbers above are therefore slurp's own code only, and the budgets
have a great deal of room in them. Earlier revisions of this document said
Alpine was bundled and derived a "< 15 KB budget for Slurp's own core runtime"
by subtracting Alpine's ~15 KB from the 30 KB target; both halves of that were
wrong. `clsx` and `tailwind-merge` are external imports on the same footing.

### Everything below this line is inert against the 0.1.0 compiler

**Read this before acting on any number in the rest of this section.** Client
navigation, `{fetch}` reactivity and infinite scroll are implemented and unit
tested in `@slurp/runtime`, but they bind to markup markers that the 0.1.0
compiler never emits:

| Feature | Marker it needs | Emitted by the compiler? |
|---|---|---|
| Client navigation | `<body navigate>`, `[data-slurp-page]` | No |
| Route params | `<meta name="slurp-route">` | No |
| `{fetch}` reactivity and polling | `x-data="slurpFetch(...)"` | No |
| Infinite scroll | `[data-slurp-sentinel]` | No |

The only `data-slurp-*` attributes the renderer emits are `data-slurp-let`,
`data-slurp-debug`, `data-slurp-component` and `data-slurp-props`. So on a page
built by `slurp build`, none of the mechanisms below is invoked and none of the
savings below is realised. They describe behaviour available today only to an
**embedding host** that supplies those markers itself, which is the case slurp
was extracted from. See `runtime/README.md`.

### Memory Leak Prevention

Two patterns in the runtime are designed to prevent leaks across navigations.
Both are reached only when a host supplies the markers above.

1. **IntersectionObserver cleanup** - infinite scroll registers an
   `IntersectionObserver` on the sentinel element. The observer is disconnected
   inside a `slurp:navigate` event listener so that stale observers from
   previous pages do not accumulate.

2. **Fetch interval clearing** - poll-mode `{fetch}` blocks register a
   `setInterval`. The interval ID is stored in Alpine component data as
   `_pollTimer` and cleared in **`destroy()` only** (`runtime/src/fetch/index.ts`),
   which Alpine calls when the component is torn down.

   **`slurp:navigate` does not stop polling.** The listener registered by
   `_setupAbortOnNavigate` aborts the in-flight request and nothing else; it
   never reads `_pollTimer`. So a poll-mode block that survives a client
   navigation without being unmounted keeps polling. There is also no `x-effect`
   anywhere in `runtime/src`; earlier revisions of this document named an
   `x-effect` cleanup path that has never existed.

### Navigation Performance

Client-side navigation (`initNavigation`) fetches only the target page HTML and
swaps the `[data-slurp-page]` subtree, so layout HTML (header, sidebar, footer)
is not re-parsed into the live document. How much that saves depends entirely on
the ratio of page content to chrome in a given theme, and **no measurement of it
exists in this repository**. Earlier revisions of this document claimed a 60-90%
payload reduction; that figure was not measured and is not reproducible from
anything here. Note also that the fetch still transfers the whole document over
the network - the saving is in parsing and re-execution, not in bytes on the
wire.

There is no prefetching of any kind. Navigation is click-triggered only: no
hover intent, no viewport prefetch, no speculation rules.

---

## Dev Server Performance

### File Watcher

The file watcher is debounced by **time only**. `spawn_watcher`
(`dev-server/src/watcher.rs`) collects change events into a
`HashSet<PathBuf>` and flushes it after `DEBOUNCE_MS` (50 ms) of quiet, which is
short enough to feel instant and long enough to batch the several inotify events
a single editor save produces.

**Deduplication is by path, not by content.** There is no mtime read, no
hashing, and no comparison of old against new bytes anywhere in the dev server,
so touching a file with no changes still triggers a rebuild and a reload.
Earlier revisions of this document claimed an mtime plus content-hash check;
there is none. Candidate paths are filtered by extension only, in
`is_watched_extension`.

### Incremental rebuild via the dependency graph

`DependencyGraph` (`dev-server/src/watcher.rs`) maintains forward and reverse
edges between pages and the components and layouts they import.
`DependencyGraph::invalidate` re-indexes the changed file and then walks the
reverse edges **depth-first** (the worklist is a `Vec` drained with `pop()`,
which is LIFO), and `process_changes` unions those results into an
`all_affected` set. The traversal is exhaustive and `visited`-guarded, so the
order does not change the result set; earlier revisions of this document called
it breadth-first, which is simply the wrong word for the code. **This is wired**, contrary to earlier revisions of this
document, which listed it under "Roadmap" as scaffolded but not connected.

What it drives is narrower than "incremental rebuild", though. Each affected
`.slurp` path is run through `compiler::check_page`, which deliberately *parses*
rather than renders, so the graph decides which files get re-checked for
diagnostics rather than scoping a recompile of output. An affected path that is
not a `.slurp` file short-circuits the whole set to `"*"`.

### WebSocket Broadcast

Hot-reload notifications are broadcast over WebSocket to all connected browser
tabs (`HmrMessage` in `dev-server/src/hmr.rs`, broadcast capacity 64). The
broadcast is O(n) in the number of connected clients, but in practice dev usage
is a single browser tab, making this effectively O(1).

**The reload is always a full document reload.** The `reload` message carries a
`pages` array, and the client (`runtime/src/core/dev.ts`) parses it and then
calls `window.location.reload()` regardless of its contents. There is no
CSS-only swap and no partial or module-level update, so the `pages` field is
currently advisory only.

---

## Benchmarks

There is no committed benchmark suite. No `benches/` directory, no criterion
dependency and no `#[bench]` exists anywhere in this repository, so **no timing
figure in this document is reproducible from anything committed here.** The
compiler's cost model is O(n) in source length with no backtracking (see
"Parser" above), and the enforced DoS budgets bound worst-case work regardless of
input. Numbers should be measured on the target deployment rather than assumed.

Two timing figures are nonetheless quoted, both in the image cache table
("Hit latency": `< 1 ms` for L1, `~5 ms` for L2). They are order-of-magnitude
expectations for an in-memory map lookup versus a small disk read, not
measurements of this code, and an L2 hit in particular scales with the entry
size. Treat them as such. An earlier revision of this section claimed no timing
figure was given at all, which contradicted that table.

Two figures here are not timings and are measured: the ~12 MB peak live memory
of a whole site build, recorded in the source beside `MAX_FILTER_VALUE_BYTES` as
the basis for that cap, and the bundle sizes above, which were taken with `gzip`
over the committed artifacts.

To profile the compiler with `cargo-flamegraph`:

```bash
cargo flamegraph --package slurp-compiler --bin slurp -- build --input examples/basic-theme
```

---

## Roadmap (not yet implemented)

These are candidate optimizations, not current behavior:

- **String interning** for repeated identifiers (component/filter names), e.g.
  via `lasso`, to cut allocation and equality-check cost on large template sets.
- **Arena allocation** of AST nodes (e.g. `bumpalo`) with per-file arenas to
  reduce allocator pressure on large corpora.
- **Scoped rebuilds off the dependency graph.** The graph itself exists and is
  wired (see "Incremental rebuild via the dependency graph" above), but what it
  currently scopes is re-*parsing* for diagnostics. Using it to re-render only
  the affected pages, and to send a targeted reload instead of a full one, is the
  remaining half.
- **A benchmark suite**, so that any performance claim in this document can be
  reproduced rather than asserted.
