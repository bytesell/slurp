# Fuzzing

The harness lives in `../tests/fuzz_support/` and the targets in `../tests/fuzz_targets.rs`.
Findings are pinned as named tests in `../tests/fuzz_regressions.rs`.

## Corpus policy (read before adding a file)

**Corpus files must be SYNTHETIC or MINIMISED REPRODUCERS. Never copy a file from a private
codebase, a customer template, or any repository other than this one.**

This is a published repository, so a corpus file is published source. Real templates carry
service names, product concepts and comments naming internal files, and scraping them is a
disclosure bug that is easy to introduce because it feels like the obvious way to build a corpus.

It also produces a worse corpus. Real files cluster around whatever one author happened to write,
so they over-represent a few constructs and miss most of the grammar. Hand-written seeds cover the
language systematically.

Three kinds of file belong here, distinguished by their name prefix:

| Prefix | What it is |
|---|---|
| `grammar-*` | Synthetic seeds exercising one area of the grammar: every block tag and its branches, every lexer mode transition, expressions, filters, frontmatter schemas, HTML. |
| `repro-*` | The minimised input for a bug that was actually found, by review or by the fuzzer. Highest-value seeds: they are what stops a fixed bug regressing silently. |
| `audit-*` | Shapes written to answer a specific question raised during a security or correctness audit, kept so the answer stays pinned. |
| everything else (`unterm-*`, `deep-*`, `amp-*`, `mode-*`, `sec-*`, `bytes-*`, `data-*`, `reg-*`) | Hand-written hostile shapes: unterminated constructs, deep nesting, amplification, mode transitions, hostile bytes. |

`corpus/sections_merge/` uses its own prefix, `state-*`, because its inputs are
not templates: each is a section-state JSON document fed to the merge, so the
prefixes above would not describe them.

Before adding anything, check it: no product names, no service names, no internal file paths in
comments, no customer data. Renaming a variable is not enough if the comment around it describes
private architecture.

## Layout

```
corpus/common/         seeds shared by every target
corpus/sections_merge/ seeds for the section-state merge (template @@@ editor JSON)
crashes/               reproducers written by a failing fuzz run; the file IS the input
```

`@@@` splits one flat file into the several inputs a multi-file target needs (source, JSON
context, registry files). A mutator can also produce it by accident, which is intentional.

The corpus is committed on purpose: it is what makes a CI failure reproducible locally and what
makes `every_corpus_input_is_clean_on_every_target` a regression suite. Keep it small; the test
`the_committed_corpus_stays_small` fails above 2 MiB.

## Running

Default sweep, as `cargo test` runs it:

```sh
cargo test -p slurp-compiler --test fuzz_targets
```

A long hunt on one target (use `--release`; the fuzzer's value is iterations per second):

```sh
SLURP_FUZZ_ITERS=200000 SLURP_FUZZ_SECS=600 \
  cargo test -p slurp-compiler --release --test fuzz_targets -- fuzz_lex --nocapture
```

PowerShell sets variables differently:

```powershell
$env:SLURP_FUZZ_ITERS = "200000"; $env:SLURP_FUZZ_SECS = "600"
cargo test -p slurp-compiler --release --test fuzz_targets -- fuzz_lex --nocapture
```

| Variable | Default | Meaning |
|---|---|---|
| `SLURP_FUZZ_ITERS` | 4000 | Iterations per target |
| `SLURP_FUZZ_SECS` | 25 | Wall-clock budget per target; whichever runs out first wins |
| `SLURP_FUZZ_SEED` | fixed | PRNG seed. The run is deterministic, so a failure reproduces |
| `SLURP_FUZZ_TIMEOUT_MS` | 3000 | Per-input watchdog budget |
| `SLURP_FUZZ_CONFIRM_MS` | 60000 | Budget a suspected hang must ALSO miss before it is reported |
| `SLURP_FUZZ_RSS_LIMIT_MB` | 256 | Process-wide memory ceiling; crossing it is a reported finding |

## Why a hang is reported instead of hanging

Every target runs on a worker thread behind a bounded wait. On a timeout the harness returns to
a live caller that prints and saves the offending input, rather than waiting for a call that
never returns. That property is itself tested, by
`watchdog_reports_a_hang_instead_of_hanging`, against a body known to spin.

A suspected hang is then re-run in a SUBPROCESS with a much larger budget before it is reported.
A debug build under a parallel `cargo test` is slow enough that a legitimately bounded
computation can miss the fast budget, and a fuzzer that reports false hangs stops being trusted.
Only an input that also misses the confirmation budget is a real non-termination. The subprocess
path is also what turns a stack overflow from an uncatchable abort of the whole suite into one
attributable input.

Minimisation runs in subprocesses too, because Rust cannot kill a thread and asking a hanging
input the same question 400 times in-process would leak 400 spinning threads.

## Why memory is bounded as well as time

An input that allocates without bound is the same bug class as one that hangs: the process dies
either way, and in a shared render service it takes every other tenant with it. Bounding only TIME
leaves that half uncovered, and an unbounded target can reach several GB before anything notices.
A harness that dies of the failure it is meant to report cannot report it.

So a counting global allocator, `CountingAlloc`, tracks live bytes process-wide.

**Enforcement is on the allocation path, not on the timer, and the distinction is the whole
point.** The watchdog's 50 ms sample can observe a slow leak but cannot contain a fast one: a tight
allocation loop, or a single huge allocation, goes from under the ceiling to many gigabytes between
two samples. So the ceiling is checked inside `CountingAlloc::alloc`, which parks the offending
worker **before** handing out the memory that would cross it. Parking rather than returning null is
deliberate too - a null return routes to `handle_alloc_error`, which aborts the process, which is
the failure this exists to report rather than suffer.

The 50 ms poll's remaining job is classification and reporting: crossing `SLURP_FUZZ_RSS_LIMIT_MB`
is surfaced exactly like a hang or a panic, with the input written to `crashes/`. The ceiling is
PROCESS-wide rather than per-target on purpose: a hang-detected worker is leaked and keeps
allocating, so leaked workers have to be counted too. Note that the ceiling is rebased on live bytes
at initialisation, so the variable is a budget rather than an absolute RSS figure. Confirmation
subprocesses carry the ceiling explicitly and `abort` on their own when they cross it, so a CI
runner attributes the input instead of being OOM-killed with no output.

Both halves are proven by tests, the same way the hang watchdog is:
`allocator_stops_a_fast_runaway_at_the_ceiling` and
`subprocess_enforces_its_own_memory_ceiling`.

**Worst-case footprint of a campaign** is the ceiling plus one confirmation subprocess carrying
the same ceiling, so roughly 2x `SLURP_FUZZ_RSS_LIMIT_MB`, provided parallelism is bounded with
`--test-threads=1`. CI does exactly that.
