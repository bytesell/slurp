## What this changes

<!-- One or two sentences. If it fixes an issue, write "Fixes #123". -->

## Why

<!-- What was wrong, or what this makes possible. -->

## How it was verified

<!--
"Tests pass" is not verification for a rendering, escaping or sanitization
change. Show the input and the output. For a bug fix, confirm the new test
actually fails when the fix is reverted, and say so.
-->

## Checklist

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` run as its own step, and clean
- [ ] `cargo test --workspace` passes
- [ ] `pnpm build`, `pnpm test` and `pnpm typecheck` pass, if TypeScript changed
- [ ] `SLURP_DOCS.md` updated in this pull request if behaviour changed
- [ ] `CHANGELOG.md` updated under `## [Unreleased]` if the change is user-visible
- [ ] No em-dash, no en-dash in a range, no ellipsis character in any prose I added

## If this touches security

<!-- Delete this section if it does not. -->

- [ ] I did not describe a protection the code does not deliver, and anything I claimed in `docs/security-model.md` cites the file and the **symbol** that enforces it, not a line number
- [ ] Any sanitizer I touched is still allowlist-based
- [ ] If I added a compile-time check, I said which paths it reaches: the `compile*` entry points and `slurp validate` always, the WASM export `validate_full` if it is in the security walk, and no other WASM export

## If this adds a template tag

- [ ] `BLOCK_KEYWORDS` in `compiler/src/lexer/mod.rs` updated, and no second copy of it introduced (it is the single source of truth for all three lexer scanners)
- [ ] Parser arm, AST node in `compiler/src/ast.rs`, and renderer arm
- [ ] `security_check` arm in `compiler/src/lib.rs`, if the tag can carry an expression
- [ ] `shared/interfaces.ts` updated, if this adds an error code or changes a wire shape
- [ ] `language/` completions and grammar updated, so the editor tooling does not drift further from the compiler
