# @bytesell/slurp-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) server for
[Slurp](https://github.com/bytesell/slurp), so a coding agent can validate,
render and reason about `.slurp` templates instead of guessing at them.

It runs the real compiler in-process through `@bytesell/slurp-compiler-wasm`: no
subprocess, no `slurp` binary on your PATH, and no second implementation of any
rule in TypeScript. A diagnostic this server reports is a diagnostic the
compiler produced, with the same code, message, line and column.

## Install

```bash
npm install -g @bytesell/slurp-mcp
```

Or skip the install and let the client fetch it, which is what the
configurations below do.

## Register it

Claude Code:

```bash
claude mcp add slurp -- npx -y @bytesell/slurp-mcp
```

Add `-s user` to get it in every project rather than just this one.

Any other client, or a shared `.mcp.json` at your repository root:

```json
{
  "mcpServers": {
    "slurp": {
      "command": "npx",
      "args": ["-y", "@bytesell/slurp-mcp"]
    }
  }
}
```

The server speaks JSON-RPC over stdio. To point at a clone instead, run
`pnpm install && pnpm build` in `mcp/` and use
`{"command": "node", "args": ["/abs/path/to/slurp/mcp/dist/bin.js"]}`.

Check the connection with `claude mcp list`. The server writes one line to
stderr on startup (`@bytesell/slurp-mcp 0.1.0 listening on stdio`) and nothing at all to
stdout except protocol frames, because on stdio stdout IS the protocol.

## Tools

| Tool | Question it answers |
|---|---|
| `slurp_validate` | Is this correct? |
| `slurp_render` | What does it produce? |
| `slurp_lint` | Is it silently wrong? |
| `slurp_schema` | What is editable here? |
| `slurp_reference` | How do I write this? |

### `slurp_validate`

Compile-check a template and get back structured diagnostics: code, message,
file, line, column, severity.

This reports what `slurp build` enforces, which is more than parsing. The
compile-time security walk is included, so an unfiltered `${ }` in a `<script>`
body, a `| js` slot in JavaScript statement position, `env.SLURP_SECRET_*`
access, `request.*` outside middleware and `{redirect}` outside middleware are
all reported here rather than at build time.

```jsonc
// in
{ "source": "<script>var x = ${ name };</script>", "file": "hero.slurp" }

// out
{
  "ok": false,
  "errors": 1,
  "warnings": 0,
  "diagnostics": [{
    "code": "UnsafeScriptInterpolation",
    "message": "an expression in a <script> body must end with the `| js` filter ...",
    "file": "hero.slurp",
    "line": 1,
    "column": 20,
    "severity": "error"
  }]
}
```

Pass `files` instead of `source` to check several templates in one call. Pass
`is_middleware: true` only for a file that really will be compiled as
middleware, because the rules genuinely differ.

The security walk stops at its first violation, so at most one security
diagnostic appears per call. Nothing here resolves imports across files either,
so a missing component or layout is not reported.

### `slurp_render`

Render against a JSON context, and get the HTML plus every diagnostic.

Rendering uses development mode, where two advisories are recorded that
production drops silently: CSS-structural characters stripped from a `style`
value slot, and an un-annotated interpolation in a JavaScript-evaluated
attribute. `{debug expr}` also renders only here. The budget warnings (loop
truncation at 1000 items, the output-byte budget, the render-depth budget) are
reported in both modes, but `slurp build` prints warnings only under
`--verbose`.

Rendering is deterministic and side-effect free. No network request is made and
no file is read; `{fetch}` picks a branch by reading its name out of the context
you passed.

### `slurp_lint`

The one tool here that is not a view onto compiler diagnostics. Every rule
describes a construct that both `slurp validate` and `slurp build` accept with
zero output, and that then renders the empty string or the wrong string.

| Rule | Catches |
|---|---|
| `component-prop-literal-interpolation` | A quoted component prop carrying interpolation syntax. It is a literal string, unlike an element attribute. |
| `filter-arg-not-literal` | A filter argument that is not a literal, so the filter falls back to its default. |
| `call-always-null` | A function call. There are no callable functions; every call is null. |
| `let-binding-not-readable` | A `{$let}` name read back as a template value. |
| `array-length-property` | `.length` on what is probably an array. |
| `component-prop-unquoted-number` | `prop=123`, which lexes as an attribute NAME. |
| `unterminated-attribute-interpolation` | A nested `"` inside `${ }` that ends its attribute early. |

Each finding carries a concrete fix, not just a complaint.

### `slurp_schema`

What a template declares in frontmatter: its `section { }` or `block { }` schema
with every setting, kind, default and nested block, plus its `props`, its
imports and its middleware name.

A `null` schema is not an error. Most templates declare none.

### `slurp_reference`

The language reference, queryable by topic: `overview`, `tags`, `expressions`,
`filters`, `components`, `scripts`, `frontmatter`, `schema`, `errors`,
`gotchas`, `limits`.

Pass an error code or a filter name as `query` with no topic to jump straight to
it, which is what you want when holding a diagnostic:

```jsonc
{ "query": "UnsafeScriptInterpolation" }
```

## Resources

For pulling context in without a tool round-trip:

| URI | Contents |
|---|---|
| `slurp://reference/language` | The complete reference, every topic. |
| `slurp://reference/errors` | Every error code, which ones the compiler emits, and a worked fix for each. |
| `slurp://reference/gotchas` | The silent failures, with the fix and with whether a tool catches it. |

## How the reference stays correct

`test/tools.test.ts` re-derives the documented claims from the compiler in the
same run that asserts them, so a compiler change that invalidates the
documentation turns the suite red rather than leaving a plausible sentence in
place. It compiles every "right" example in the error reference and requires it
to validate clean, compiles every "wrong" example that claims an error and
requires that error, and re-checks the truthiness table, operator semantics,
loop variables, filter list and `| js` versus `| json` behaviour against real
renders.

## Development

```bash
pnpm install
pnpm build       # tsup -> dist/, then regenerates llms-full.txt
pnpm typecheck
pnpm test        # unit tests plus a real stdio round trip against dist/
```

The smoke test runs the built server as a child process over a real pipe, so
`pnpm build` is a prerequisite. Unit tests over the handlers prove the logic and
prove nothing about the server, and `dist/` is what a host executes.

## License

MIT, ByteSell LLC.
