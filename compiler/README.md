# slurp-compiler

The compiler for the [Slurp](https://github.com/bytesell/slurp) template
language. It lexes and parses `.slurp` files into an AST and renders that AST to
HTML against a JSON context, in a single server-side pass.

This crate ships three things:

- a Rust library (`compile`, `parse`, `render`, `check_security`,
  `extract_schema`, ...),
- the `slurp` command line binary (`build`, `validate`, `dev`, `gitignore`),
- WASM bindings: `parse`, `render`, `render_dev`, `validate`, `validate_full`,
  `extract_schema` and `render_section`. All but `validate_full` are for editor
  previews and skip the compile-time security walk; `validate_full` runs it, so
  it is what a tool needing parity with `slurp build` should call.

```bash
cargo install slurp-compiler
slurp build --input . --output dist
```

> The binary is named `slurp`, not `slurp-compiler`.

Rendering is escape-by-default, and the escaper is chosen from the context the
value lands in: HTML entities in text and plain attributes, JS-literal escaping
or a self-delimiting JSON literal in a JavaScript-evaluated attribute, removal of
structural characters in a `style` value, and the allowlist sanitizer in
`srcdoc`. An interpolation in a `<script>` body is a compile error unless it
declares its context with `| js` or `| json`.

[The security model](https://github.com/bytesell/slurp/blob/main/docs/security-model.md)
has the threat model and the known gaps.

**Status: 0.1.0, pre-1.0.** The API and the template language are both unstable.

Licensed under MIT.
