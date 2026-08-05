# slurp-dev-server

The local development server for [Slurp](https://github.com/bytesell/slurp)
themes. It watches a theme directory, rebuilds on change, and pushes a reload
over a WebSocket.

```bash
cargo install slurp-dev-server
slurp-dev --theme-dir . --port 3000
```

Features:

- file watching with a debounce, and a full `slurp build` re-run when
  `--source-dir` is set,
- hot reload over a WebSocket at `/_slurp/ws`,
- `--fixtures`, a flat JSON file mapping a request path to a canned response, so
  a theme can be previewed with no backend running,
- an optional reverse proxy to a real backend via `--backend-url`.

**This server is for local development only.** It binds `127.0.0.1`
unconditionally and is not hardened for network exposure.

**Status: 0.1.0, pre-1.0.** The CLI surface is unstable.

Licensed under MIT.
