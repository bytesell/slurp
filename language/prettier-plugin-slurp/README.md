# prettier-plugin-slurp

A [Prettier](https://prettier.io) plugin that formats [Slurp](https://github.com/bytesell/slurp) template files (`.slurp`).

## Install

```bash
pnpm add -D prettier prettier-plugin-slurp
```

Prettier 3 is a peer dependency.

## Use

Add the plugin to your Prettier config:

```json
{
  "plugins": ["prettier-plugin-slurp"]
}
```

Then format as usual:

```bash
npx prettier --write "**/*.slurp"
```

The plugin registers the `slurp` language and the `.slurp` extension, so
Prettier picks those files up on its own once the plugin is listed.

## Scope, and a warning

This formats **whitespace and indentation**. It does not validate, and a file it
formats without complaint can still fail to compile.

Use `slurp validate --dir .` for correctness. That runs the same parse and the
same compile-time security checks a build runs, which is a different question
from the one this plugin answers.

Note also that the editor tooling and the compiler currently disagree on a few
points of syntax. Where they differ, the compiler is right. See
[SLURP_DOCS.md](https://github.com/bytesell/slurp/blob/main/SLURP_DOCS.md).

## License

[MIT](./LICENSE). Copyright (c) 2026 ByteSell LLC.
