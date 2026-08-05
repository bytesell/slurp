# slurp-image - Architecture Decisions

## Image processing crates

| Format | Crate | Reason |
|--------|-------|--------|
| JPEG, WebP, PNG, GIF - decode/resize/encode | `image` 0.25 | The most widely used Rust image library; actively maintained, with a good record of addressing advisories promptly. |
| AVIF - encode | `ravif` 0.11 | The best quality/performance tradeoff for AVIF encoding in the Rust ecosystem. Wraps `rav1e`, which is the encoder behind most AVIF pipelines. |

The `image` crate is used for all decoding (regardless of output format) and for JPEG/WebP/PNG/GIF encoding. `ravif` is called only for AVIF output, after the image has been decoded and resized by `image`.

**`image` is built with `default-features = false, features = ["jpeg", "webp", "png", "gif"]`.** Its default set links ten further decoders (bmp, dds, exr, ff, hdr, ico, pnm, qoi, tga, tiff), and decoder selection happens by content, so those would be reachable by anything that got past the magic-byte check. The enabled list must stay in step with `security::SourceFormat`, which has exactly the four matching variants. AVIF is an OUTPUT format only: no ISO base media decoder is linked in.

## Security architecture

Security checks run in strict order before any image processing or I/O:

1. **URL validation** (`security::validate_src`) - scheme allowlist (http/https only), control-character rejection, userinfo rejection, length cap.
2. **DNS resolution + SSRF check** (`tokio::net::lookup_host` + `security::is_safe_ip`) - all resolved addresses must be globally routable unicast, checked against an ALLOW-list so unfamiliar address space fails closed; done before the TCP connection is opened.
3. **Connection pinned to the validated addresses** (`resolve_to_addrs`) - the HTTP client never performs its own second lookup, which is what actually closes DNS rebinding.
4. **No redirect following** (`reqwest::redirect::Policy::none()`) - prevents redirect-based SSRF bypass.
5. **Source size cap** - Content-Length checked before download; streamed body capped at 20 MB even when Content-Length is absent or lies (chunked/gzip).
6. **Magic-byte validation** (`security::detect_source_format`) - file header, not extension or Content-Type, governs format acceptance. The ISO base media branch requires the `ftyp` marker, not just the brand.
7. **Decoder-format allowlist** - the format the decoder infers from the content must equal the one the header declared, so a polyglot cannot be routed to a different decoder.
8. **Decompression-bomb check** - declared dimensions times the decoder's REAL bytes per pixel (8 for a 16-bit source, not a hardcoded 4) must not exceed 50 MB, enforced before any pixel buffer is allocated. A decoder allocation limit backs it up.
9. **Output pixel cap** - the post-resize pixel count is capped, because a width cap alone bounds nothing on a very tall source.

**The blocking-pool boundary is between steps 6 and 7, not before step 6.** Steps 1 to 6 all run on the async worker: magic-byte validation is a cheap header check and is deliberately done *before* any permit is taken or any thread is committed, so a rejected request costs neither. `process_image` then acquires the concurrency permit and only afterwards calls `spawn_blocking`, in that order, so a queued request occupies a cheap future rather than a blocking thread. Everything from step 7 onward is CPU-bound and runs on the blocking pool, never on an async worker.

## Testing the fetch path

The guards behind the SSRF check (streaming cap, timeout, redirect refusal, magic rejection, decompression bomb) cannot be exercised from outside the crate, because the SSRF check correctly refuses to fetch from a local test origin. `Config::allow_private_hosts` is a `#[cfg(test)]`-only field, read through `processor::private_hosts_allowed`, which is a compile-time `false` in every other build. Tests spawn a loopback origin and drive the real `process_image`. Do not promote that field to a runtime flag.

## Caching

Two-level cache:

- **Level 1 - moka async in-memory cache** - bounded by entry count, O(1) lookup, TTL managed by moka.
- **Level 2 - on-disk cache** - persistent across restarts; each entry has a sidecar `.meta` file storing the Unix expiry timestamp so TTL is enforced without reading image bytes.

Cache key: `SHA-256(src || NUL || width_le32 || NUL || format_str)` encoded as hex.

## Rate limiting

Per-client **tumbling-window** limiter (default: 100 requests per 60 s), `RateLimiter::check_and_record` in `handler.rs`. It is a fixed window, not a sliding one: the counter is hard-reset once the window has elapsed, so a client can be admitted up to **twice** the nominal rate across a boundary. That is the standard fixed-window tradeoff and is accepted here, because this is implemented in-process as a last line of defence; a reverse proxy (nginx, Caddy, Cloudflare) should be the primary rate-limit layer.

**The key is not the IP address.** `ClientKey` buckets an IPv6 client by its **/64 prefix**, because a routine residential or cloud allocation is a whole /64 and a per-address bucket would let one client mint unlimited distinct keys. An IPv4-mapped IPv6 address collapses into the IPv4 bucket, so a dual-stack listener cannot hand the same client a second allowance. The table is sharded over 16 mutexes with a per-shard entry cap, and pruning runs on a background timer rather than on the request path; a poisoned mutex fails closed.

## Why `anyhow` only in main

`anyhow` is used only in `main.rs` for top-level startup error propagation. All library code uses `thiserror`-derived typed errors so callers can pattern-match on specific failure modes.
