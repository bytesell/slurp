# Contributing to Slurp

Thanks for taking an interest. This document covers how to build the project, what your change has to pass, and the conventions that are specific enough to be worth writing down.

## Before you start

**Slurp is pre-1.0 and parts of it are honestly incomplete.** [SLURP_DOCS.md](./SLURP_DOCS.md) marks every unimplemented and unwired feature inline. If you are about to fix something, read the relevant section first: a surprising number of "bugs" are documented gaps with a known reason, and a few are documented gaps that genuinely want fixing.

For anything larger than a bug fix, open an issue first so the design can be discussed before you write it.

**Security issues do not go through issues or pull requests.** Use GitHub's private vulnerability reporting, as described in [SECURITY.md](./SECURITY.md).

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust | 1.88 or newer | Pinned by `rust-toolchain.toml`. Edition 2024 needs 1.85; the dependency tree pushes the real floor to 1.88 |
| Node.js | 20 or newer | |
| pnpm | 9 or newer | `corepack enable` is the easiest way to get it |
| wasm-pack | 0.15 or newer | Required: `pnpm build` builds `@slurp/compiler-wasm`, and the compiler integration tests import it. `cargo install wasm-pack --locked` |

**If your toolchain is too old you will not get a version message.** Edition 2024 fails on Rust 1.84 and below with `feature 'edition2024' is required`, which reads like a broken project. `rust-toolchain.toml` pins 1.88 so rustup selects the right toolchain automatically; if you see that error anyway, run `rustup update`. The floor is 1.88 rather than the 1.85 edition 2024 needs because the `image` crate requires it.

The toolchain file pins the MSRV so that clippy lints do not churn and so the minimum supported version is what everyone actually builds against. That pin is also what catches MSRV drift: building under it is the only way to notice that a dependency bump has raised the floor, which had already happened silently before the pin existed. To try a newer toolchain locally, prefix a command with `RUSTUP_TOOLCHAIN=stable`.

If `cargo check` reports that a package requires a newer rustc, do not lower the pin. Either hold that dependency back, or raise `channel` in `rust-toolchain.toml` and `rust-version` in `Cargo.toml` together, and note the bump in `CHANGELOG.md`.

## Building

```bash
# Rust workspace: compiler, dev-server, image
cargo build --workspace

# TypeScript workspace: runtime, language, prettier plugin, vscode extension,
# and the WASM build of the compiler (@slurp/compiler-wasm, via wasm-pack)
pnpm install
pnpm build
```

`pnpm build` is what produces `@slurp/compiler-wasm`, and the compiler
integration suite imports it, so **run it before `pnpm test` on a fresh clone**
or those tests cannot resolve the module. `pnpm -r` orders the build ahead of
its dependents on its own.

## The gates

Run these locally before opening a pull request. They are the gates you can drive by hand; the full CI job list, including the two you cannot practically run locally, is below.

```bash
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo audit
cargo deny check

pnpm build
pnpm test
pnpm typecheck
pnpm copy-check
pnpm -C mcp run check:llms
```

**Run `clippy` as its own step, and read its own output.** Chaining it onto another command and reading the second command's result is how warnings reach a main branch.

`cargo fmt --all --check` is blocking. Fix a failure with `cargo fmt --all`, never by relaxing the check.

### What CI actually runs

`.github/workflows/ci.yml` has eight jobs, and they are not all in the list above:

| Job | What it runs |
|---|---|
| `rust` | build, test and clippy on the pinned MSRV (1.88) |
| `rust-stable` | the same three on current stable, to catch newer-toolchain breakage |
| `fmt` | `cargo fmt --all --check` |
| `copy` | `node scripts/check-copy-rule.mjs` |
| `audit` | `cargo audit` |
| `deny` | `cargo deny check` (licences, banned crates, accepted advisories) |
| `node` | `pnpm install --frozen-lockfile`, `pnpm build`, `pnpm typecheck`, the llms-full drift check, then a filtered `test` |
| `fuzz` | the compiler fuzz targets on **both** ubuntu and windows, with a raised iteration budget |

Three of those differ from what a local run does, in ways worth knowing:

- **`cargo build` runs with `--all-targets` in CI**, so it compiles test and bench targets too. A local `cargo build --workspace` can pass while CI fails to compile a test.
- **The `node` job does not run `pnpm test`.** It runs `pnpm --filter @slurp/runtime --filter @slurp/language --filter prettier-plugin-slurp --filter @slurp/integration-tests test`, so `@slurp/mcp` and `vscode-extension` tests do **not** run in CI. `vscode-extension` is excluded because its `test` script downloads and launches a real VS Code instance, which needs a display. `@slurp/mcp` has no such excuse and its exclusion is worth removing. A local `pnpm test` (`pnpm -r run test`) runs both, so run it.
- **The `fuzz` job is a gate, not an optional tool.** `cargo test --workspace` already runs the fuzz targets at their small default budget; the CI job re-runs them with `SLURP_FUZZ_ITERS=150000` on two operating systems. A finding is uploaded as an artifact, and the file is the reproducer.

**`cargo audit` reports advisories that are not currently actionable.** Several come from transitive dependencies of the image codecs and cannot be resolved without an upstream release. New advisories are worth investigating; the existing ones are recorded with a reason in `deny.toml`, which is what the `deny` job re-checks.

## How the pieces fit together

Understanding this saves time, because a change in one place often needs a matching change in another.

```
.slurp source
   -> compiler/src/lexer      tokens (with several context-sensitive lexing modes)
   -> compiler/src/parser     AST
   -> compiler/src/lib.rs     security_check, on the compile* paths ONLY
   -> compiler/src/renderer   one pass over a JSON context -> HTML
```

`shared/interfaces.ts` is the TypeScript view of the compiler's wire shapes, imported by the WASM package, the language server, the runtime and the VS Code extension. It is the only file in `shared/`: it used to sit beside an `ast.rs` and an `errors.rs` described as the Rust half of the same contract, but neither was a workspace member, neither was `include!`d and neither was referenced from any Rust file, so they silently rotted while contributors were told to maintain them. They are gone. **The compiler's own `ast.rs` and `error.rs` are the Rust definitions.** When you add an error code or change a wire shape, update `interfaces.ts` in the same pull request.

`runtime/` layers optional browser behaviour on the emitted HTML via data attributes. `language/` and `vscode-extension/` are editor tooling and are a separate description of the language from the compiler's, which means they can and do drift.

There is an architecture rationale in `compiler/DECISIONS.md`, `image/DECISIONS.md` and `runtime/DECISIONS.md`. Read the relevant one before proposing a structural change: the hand-rolled parser, the allowlist sanitizer and the tsup bundle split were each chosen over alternatives for reasons recorded there.

## Conventions

### The renderer is total, and that is deliberate

Missing data renders `""`. Budgets truncate. Unknown editor settings are dropped and unknown block types are ignored. Slurp renders untrusted templates in shared multi-tenant services, so a bad template must never take a page down.

Most budgets also record a diagnostic, but **the two loop-iteration budgets record one only in `BuildMode::Development`**, and `slurp build` is unconditionally `Production`, so on a build they truncate in total silence. That is a defect, not a design stance; do not copy the pattern into a new budget.

**When you add a failure path, follow that stance:** degrade and record a diagnostic rather than returning an error, unless the failure is a security violation or a build-time mistake the author must fix. If you add a silent behaviour, document it in `SLURP_DOCS.md` in the same pull request. A silent behaviour nobody wrote down is the single most common complaint about this codebase.

### Security-relevant changes

- **Escaping is context-sensitive, and so is slurp's.** `renderer/escape.rs` holds five different escapers and `renderer/js_slot.rs` decides which one a slot needs. Before touching any of them or any call site, read the "How `${ }` is treated in each context" section of [SECURITY.md](./SECURITY.md). A change that looks like a simplification there can silently remove a control: the ordering in `escape_slot` is load-bearing (the CSS strip runs first, because doing it after the JS pass would remove the backslashes that pass added), and the verbatim-emission exemption for a filtered value is reachable only while `script_depth > 0`.
- **Refusing is a legitimate answer, and sometimes the only honest one.** Where no automatic escaper fits a position, slurp raises `UnsafeScriptInterpolation` rather than applying one that does not. Do not "improve" such a refusal into an escaper that leaves the payload intact while looking like a fix.
- **Never describe a protection that the code does not deliver.** If you document a security property, cite the file and the **symbol** that enforces it, never a line number. Line numbers in prose rot by construction, and a `SECURITY.md` promising a control that does not exist is worse than no document, because it is the first thing a researcher will test.
- Sanitizers stay allowlist-based. Do not convert one to a blocklist.
- The compile-time checks in `lib.rs` run in the `compile*` paths, in `slurp validate`, and in the WASM export `validate_full` only. The other WASM exports skip them deliberately. If you add a check, say in `SECURITY.md` which of those paths it reaches.

### The lexer keyword list is one place, and it must stay that way

The block-tag keyword list is `BLOCK_KEYWORDS` in `compiler/src/lexer/mod.rs`, and it is the single source of truth: `lex_template`, `lex_raw_script` and `lex_raw_script_text` all read it, so a new keyword cannot be taught to one and not the others. It used to be three hand-written copies, and they drifted; the symptom was a tag lexing as an expression and **rendering as empty with no diagnostic**, which is a genuinely difficult thing to debug.

The same shape recurs twice more in that file and both are also consolidated: `RAW_TEXT_TAGS` decides both which tags enter a raw-text mode and which close tags leave it, and `raw_brace_starts_slurp` is the one predicate for "this `{` opens a slurp construct". **Do not reintroduce a second copy of any of the three.** If you need the list somewhere else, read the const.

Note that the fuzzer builds its token alphabet from `block_keywords()` rather than from a copy, so a new keyword is fuzzed the moment it is added. A copy there is what let an earlier `{else` bug survive.

### Adding a template feature

A new tag typically touches: `BLOCK_KEYWORDS`, `parser/mod.rs` dispatch, an AST node in `compiler/src/ast.rs`, a renderer arm, a `security_check` arm in `lib.rs` if it can carry an expression, the `language/` completions and grammar, and `SLURP_DOCS.md`. Missing the tooling and docs is how the editor tooling drifted from the compiler in the first place.

### Writing style for all prose

This applies to documentation, comments, commit messages and UI strings alike.

- **Never use an em-dash.** Use a hyphen, a comma, parentheses, or two sentences.
- **Never use an en-dash in a range.** Write `1-10`.
- **Never use the ellipsis character.** Type three literal dots.

The reason is plain readability: the typographic forms read as machine-generated and are inconsistent across the tree.

This is enforced, not merely asked for. `pnpm copy-check` (`scripts/check-copy-rule.mjs`) scans every file git considers part of the repository and fails on any of the three characters; CI runs it as its own job. Ignored build output is out of scope, and binary files are skipped. If a file genuinely needs one of the characters, prefer writing it as an escape (`String.fromCharCode(0x2014)`) over adding an entry to the script's allow list.

## Tests

- Rust unit tests live beside the code in `#[cfg(test)]` modules; integration tests are in `compiler/tests/`.
- TypeScript tests are `*.test.ts` beside the source, run with vitest.
- **`tests/integration/compiler.test.ts` runs against the real WebAssembly build** and is genuine coverage of the six WASM exports. It used to run against a hardcoded stub behind a `SKIPPED_PENDING_WASM` binding, and an earlier revision of this document still said so; that is no longer true. Because it imports `@slurp/compiler-wasm`, it needs `pnpm build` on a fresh clone. Every WASM export returns a JSON **string**, not an object, so assertions go through the `parseJson` helper.

When you fix a bug, add a test that fails before your fix. For anything touching escaping, sanitization or a resource budget, verify the test actually fails when the fix is reverted. A test that passes either way is worse than none, because it looks like coverage.

## Pull requests

- One logical change per pull request.
- Say what you changed and why, and how you verified it. "Tests pass" is not verification for a rendering or escaping change; show the input and the output.
- Update `CHANGELOG.md` under `## [Unreleased]` for anything user-visible.
- Update `SLURP_DOCS.md` in the same pull request as the behaviour change, not afterwards.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE).
