# @slurp/mcp

A [Model Context Protocol](https://modelcontextprotocol.io) server for
[Slurp](https://github.com/bytesell/slurp), so a coding agent can validate,
render and reason about `.slurp` templates instead of guessing at them.

It runs the real compiler in-process through `@slurp/compiler-wasm`. There is no
subprocess, no `slurp` binary on your PATH, and no second implementation of any
rule in TypeScript: a diagnostic this server reports is a diagnostic the
compiler produced, with the same code, message, line and column.

## Why this exists

An agent writing a template in an unfamiliar language is guessing until
something tells it otherwise. The usual feedback loop is a build, which is slow,
needs a project on disk, and reports nothing at all about the constructs that
compile cleanly and then render wrong.

This closes that loop in one call, and adds a second one for the silent cases.

## Install

```bash
npm install -g @slurp/mcp
```

Or run it without installing, which is what the configurations below do.

## Register it

### Claude Code

```bash
claude mcp add slurp -- npx -y @slurp/mcp
```

Add `-s user` to make it available in every project rather than just this one.

To check in a shared configuration instead, put a `.mcp.json` at your repository
root. Everyone on the project then gets the server with no per-machine setup:

```json
{
  "mcpServers": {
    "slurp": {
      "command": "npx",
      "args": ["-y", "@slurp/mcp"]
    }
  }
}
```

### Any other MCP client

The server speaks JSON-RPC over stdio, which is what every coding agent uses.
Point your client's server configuration at the same command:

```json
{
  "mcpServers": {
    "slurp": {
      "command": "npx",
      "args": ["-y", "@slurp/mcp"]
    }
  }
}
```

### From a clone of this repository

```bash
cd mcp
pnpm install
pnpm build
```

then register the built entry point directly:

```json
{
  "mcpServers": {
    "slurp": {
      "command": "node",
      "args": ["/absolute/path/to/slurp/mcp/dist/bin.js"]
    }
  }
}
```

### Confirm it connected

```bash
claude mcp list
```

The server also writes one line to stderr on startup:
`@slurp/mcp 0.1.0 listening on stdio`. Nothing is ever written to stdout except
protocol frames, because on stdio, stdout IS the protocol.

## Tools

Five, separated by the QUESTION they answer rather than by compiler entry point.

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

This reports exactly what `slurp build` enforces, which is more than parsing.
The compile-time security walk is included, so an unfiltered `${ }` in a
`<script>` body, a `| js` slot in JavaScript statement position,
`env.SLURP_SECRET_*` access, `request.*` outside middleware and `{redirect}`
outside middleware are all reported here rather than at build time.

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
`is_middleware: true` only for a file that really will be compiled as middleware,
because the rules genuinely differ.

Two limits, both stated in the tool description too. The security walk stops at
its FIRST violation, so at most one security diagnostic appears per call. And
nothing here resolves imports across files, so a missing component or layout is
not reported.

### `slurp_render`

Render against a JSON context, and get the HTML plus every diagnostic.

Rendering uses DEVELOPMENT mode on purpose. Three diagnostics exist only in that
mode and are dropped silently in production: a loop truncated at 1000 items,
CSS-structural characters stripped from a `style` value slot, and an undeclared
slot in a JavaScript-evaluated attribute. `slurp build` uses production mode, so
it will not tell you about any of them.

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

Three, for pulling context in without a tool round-trip:

| URI | Contents |
|---|---|
| `slurp://reference/language` | The complete reference, every topic. |
| `slurp://reference/errors` | Every error code, which ones the compiler emits, and a worked fix for each. |
| `slurp://reference/gotchas` | The silent failures, with the fix and with whether a tool catches it. |

## How the reference is kept honest

Every claim in it was established two ways: read off the compiler source, and
then checked by RENDERING the construct and recording what came out. Where the
two disagreed, the observed output won.

That is not a promise, it is a test. `test/tools.test.ts` re-derives the
documented claims from the compiler in the same run that asserts them, so a
compiler change that invalidates the documentation turns the suite red rather
than leaving a plausible sentence in place. It compiles every "right" example in
the error reference and requires it to validate clean, compiles every "wrong"
example that claims an error and requires that error, and re-checks the
documented truthiness table, operator semantics, loop variables, filter list and
`| js` versus `| json` behaviour against real renders.

Three claims that older documents about Slurp still make were disproved this way
and are NOT in the reference: that a dynamic index `items[i]` always renders
empty, that `<Image width={400}>` ignores a literal, and that the block form of
`{slot}` has unreachable fallback content. All three work. The linter has a
comment block recording them so nobody re-adds a rule for a bug that is fixed.

## Development

```bash
pnpm install
pnpm build       # tsup, ESM, two entries: the library and the executable
pnpm typecheck
pnpm test        # unit tests plus a real stdio round trip against dist/
```

The smoke test runs the BUILT server as a child process over a real pipe, so
`pnpm build` is a prerequisite. That is deliberate: unit tests over the handlers
prove the logic and prove nothing about the server, and `dist/` is what a host
actually executes.

## License

MIT, ByteSell LLC.
