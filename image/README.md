# slurp-image

The on-demand image service for [Slurp](https://github.com/bytesell/slurp)
themes. The `<Image>` component emits URLs that point at it.

```bash
cargo install slurp-image
slurp-image --port 3001
```

One route, `GET /_slurp/image`, taking `src` (an absolute `http`/`https` URL),
`w` (target width) and `f` (`avif`, `webp` or `jpeg`). The only transform is a
width-only resize that preserves aspect ratio and never upscales. Results are
cached in memory and on disk.

It fetches attacker-influenced URLs by design, so it carries a matching set of
guards: a scheme allowlist, rejection of userinfo, DNS resolved up front with
every returned address required to be globally routable unicast, those
addresses pinned into the HTTP client to defeat rebinding, redirects disabled
entirely, magic-byte validation of the response body backed by a decoder-format
allowlist, and caps on source size, decoded size and output pixels. Transform
work runs off the async workers behind a concurrency limit. See
[SECURITY.md](https://github.com/bytesell/slurp/blob/main/SECURITY.md).

It has no authentication, so it binds loopback by default and sends no CORS
headers. Pass `--bind 0.0.0.0` and `--cors-allow-origin <origin>` to widen that,
and put it behind a reverse proxy.

**Status: 0.1.0, pre-1.0.** The CLI surface is unstable.

Licensed under MIT.
