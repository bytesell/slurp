//! Fuzz targets for the slurp compiler.
//!
//! Run the default (CI-sized) sweep:
//!
//! ```text
//! cargo test -p slurp-compiler --test fuzz_targets
//! ```
//!
//! Run a long hunt on one target:
//!
//! ```text
//! SLURP_FUZZ_ITERS=200000 SLURP_FUZZ_SECS=600 cargo test -p slurp-compiler \
//!     --test fuzz_targets -- fuzz_lex --nocapture
//! ```
//!
//! On Windows PowerShell, set the variables with `$env:SLURP_FUZZ_ITERS = "200000"` first.
//!
//! Every target runs under the watchdog in `fuzz_support`, so a hang FAILS the test with the
//! minimised input printed and saved, instead of hanging the suite. That property is itself
//! tested by `watchdog_reports_a_hang_instead_of_hanging`.

mod fuzz_support;

/// The counting allocator that enforces the memory ceiling. It must be installed at the crate
/// root of the test BINARY, and it covers leaked workers and subprocess replays alike.
#[global_allocator]
static ALLOC: fuzz_support::CountingAlloc = fuzz_support::CountingAlloc;

use std::time::Duration;

use fuzz_support::{
    Outcome, SubOutcome, Target, corpus_root, fuzz, load_corpus, replay_worker_if_requested,
    run_guarded, run_in_subprocess, run_in_thread,
};

fn report(stats: fuzz_support::Stats) {
    println!(
        "[fuzz] {:<22} iterations={:<7} elapsed={:>7.2}s corpus={} panics={} hangs={} ooms={} slow={} peak={}MiB",
        stats.target.name(),
        stats.iterations,
        stats.elapsed.as_secs_f64(),
        stats.corpus_grown_to,
        stats.panics,
        stats.hangs,
        stats.ooms,
        stats.slow,
        fuzz_support::peak_bytes() / (1024 * 1024),
    );
}

// -------------------------------------------------------------------------------------------
// The watchdog's own proof
// -------------------------------------------------------------------------------------------

/// The harness's central property, asserted directly: given a body that provably never returns,
/// the watchdog returns `Hung` to a LIVE caller within the budget.
///
/// Anything that measures elapsed time AFTER the call returns cannot observe a hang, because the
/// call never returns and the measurement never runs. This test is what keeps that distinction
/// from being a claim in a comment.
///
/// The spinning body is released through a flag afterwards so the test does not leave a thread
/// burning a core for the rest of the run.
#[test]
fn watchdog_reports_a_hang_instead_of_hanging() {
    let _serial = fuzz_support::serial_guard();
    fuzz_support::init_memory_limit();
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let stop = Arc::new(AtomicBool::new(false));
    let s = Arc::clone(&stop);

    let started = std::time::Instant::now();
    let outcome = run_guarded("selftest-hang", Duration::from_millis(300), move || {
        while !s.load(Ordering::Relaxed) {
            std::hint::spin_loop();
        }
    });
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);

    assert_eq!(
        outcome,
        Outcome::Hung,
        "the watchdog must report a hang, not wait for it"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the watchdog returned only after {elapsed:?}; it must return at its budget"
    );
}

/// The memory half of the same property, and the one a polled ceiling cannot satisfy.
///
/// A tight allocation loop goes from under the ceiling to many gigabytes between two 50 ms
/// samples, so a POLLED ceiling cannot contain it - under one, a mutated input can reach tens of
/// gigabytes. Enforcement therefore lives inside `CountingAlloc::alloc`, which parks the worker at
/// the ceiling before handing out the memory.
///
/// This body allocates as fast as it can with no sleeps and no flag to release it, which is what
/// makes it a fair test: nothing but the allocator itself can stop it. The assertion is that the
/// harness REPORTS it, the process SURVIVES, and the peak stays near the ceiling rather than
/// running away from it.
#[test]
fn allocator_stops_a_fast_runaway_at_the_ceiling() {
    let _serial = fuzz_support::serial_guard();
    // A small BUDGET so the test is quick and the worker it permanently parks is small.
    // Legitimate work is tens of megabytes: a full build of a real theme peaks at ~12 MB.
    unsafe { std::env::set_var("SLURP_FUZZ_RSS_LIMIT_MB", "64") };
    fuzz_support::init_memory_limit();
    fuzz_support::reset_memory_breach();

    let outcome = run_guarded("selftest-fast-oom", Duration::from_secs(600), || {
        // No release flag, no sleep. Only the allocator can stop this.
        let mut hog: Vec<Vec<u8>> = Vec::new();
        loop {
            hog.push(vec![0u8; 1024 * 1024]);
            std::hint::black_box(&hog);
        }
    });

    assert!(
        matches!(outcome, Outcome::OutOfMemory(_)),
        "a fast runaway must be reported, got {outcome:?}"
    );
    assert!(
        fuzz_support::memory_breached(),
        "the allocator must be what stopped it, not the poll"
    );
    // Containment: the worker was parked ON the allocation path, so the peak sits just above the
    // ceiling instead of gigabytes past it. The margin covers the main thread's own allocations.
    let peak_mb = fuzz_support::peak_bytes() / (1024 * 1024);
    assert!(
        peak_mb < 512,
        "peak reached {peak_mb} MiB against a 64 MiB budget; containment failed"
    );

    fuzz_support::reset_memory_breach();
    unsafe { std::env::remove_var("SLURP_FUZZ_RSS_LIMIT_MB") };
    fuzz_support::init_memory_limit();
}

/// A panic inside a target is reported as a finding, not propagated into the harness.
#[test]
fn watchdog_reports_a_panic_with_its_message() {
    let _serial = fuzz_support::serial_guard();
    fuzz_support::init_memory_limit();
    let outcome = run_guarded("selftest-panic", Duration::from_secs(5), || {
        panic!("deliberate canary panic");
    });
    match outcome {
        Outcome::Panicked(msg) => assert!(msg.contains("deliberate canary panic"), "got: {msg}"),
        other => panic!("expected a reported panic, got {other:?}"),
    }
}

/// A body that completes is reported as completed. Without this, a watchdog that reported
/// everything as a hang would still pass the two tests above.
#[test]
fn watchdog_reports_completion() {
    let _serial = fuzz_support::serial_guard();
    fuzz_support::init_memory_limit();
    let outcome = run_guarded("selftest-ok", Duration::from_secs(5), || {
        std::hint::black_box(1 + 1);
    });
    assert_eq!(outcome, Outcome::Completed);
}

/// The subprocess watchdog, which is what minimisation runs on and what makes a stack overflow
/// attributable rather than an abort of the whole suite.
#[test]
fn subprocess_watchdog_runs_a_real_input() {
    let _serial = fuzz_support::serial_guard();
    let out = run_in_subprocess(Target::Parse, b"<p>${x}</p>", Duration::from_secs(60));
    assert_eq!(
        out,
        SubOutcome::Completed,
        "a healthy input must replay cleanly in a subprocess"
    );
}

/// The single-input replay entry point that `run_in_subprocess` re-executes. On an ordinary
/// `cargo test` run no replay is requested and this is a no-op.
#[test]
fn zzz_fuzz_replay_worker() {
    let _serial = fuzz_support::serial_guard();
    let _ = replay_worker_if_requested();
}

// -------------------------------------------------------------------------------------------
// Corpus health
// -------------------------------------------------------------------------------------------

/// Every committed corpus file must run cleanly through every target. This is the regression
/// suite: each bug found by the fuzzer has its minimised reproducer added to the corpus, so a
/// reintroduced bug fails here immediately rather than waiting for the fuzzer to rediscover it.
#[test]
fn every_corpus_input_is_clean_on_every_target() {
    let _serial = fuzz_support::serial_guard();
    // Generous on purpose. This is a wall-clock budget, and the thing it exists to catch is a
    // hang, which is unbounded: 60s and 20s both catch that, but only 20s also catches a healthy
    // input that lost its CPU slice to whatever else the machine is doing. That is not
    // hypothetical, it fires on a loaded runner and reads as a real regression. 60s matches the
    // subprocess budget below.
    let budget = Duration::from_secs(60);
    let mut checked = 0usize;
    for &target in Target::ALL {
        for input in load_corpus(target) {
            match run_in_thread(target, input.clone(), budget) {
                Outcome::Completed => {}
                other => panic!(
                    "corpus input failed on target {}: {other:?}\ninput: {:?}",
                    target.name(),
                    String::from_utf8_lossy(&input)
                ),
            }
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "the corpus is empty; it must be committed alongside the harness"
    );
    println!("[fuzz] corpus checks: {checked}");
}

/// The corpus is committed, so it must stay small enough to be reviewable. A fuzzer that drags
/// gigabytes into the repository is not one anybody keeps maintaining.
#[test]
fn the_committed_corpus_stays_small() {
    let _serial = fuzz_support::serial_guard();
    fn total(dir: &std::path::Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        rd.flatten()
            .map(|e| {
                let p = e.path();
                if p.is_dir() {
                    total(&p)
                } else {
                    p.metadata().map(|m| m.len()).unwrap_or(0)
                }
            })
            .sum()
    }
    let bytes = total(&corpus_root());
    assert!(
        bytes < 2 * 1024 * 1024,
        "corpus is {bytes} bytes; keep it under 2 MiB"
    );
}

// -------------------------------------------------------------------------------------------
// The targets
// -------------------------------------------------------------------------------------------

#[test]
fn fuzz_lex() {
    let _serial = fuzz_support::serial_guard();
    report(fuzz(Target::Lex));
}

#[test]
fn fuzz_parse() {
    let _serial = fuzz_support::serial_guard();
    report(fuzz(Target::Parse));
}

#[test]
fn fuzz_compile_template() {
    let _serial = fuzz_support::serial_guard();
    report(fuzz(Target::CompileTemplate));
}

#[test]
fn fuzz_compile_with_registry() {
    let _serial = fuzz_support::serial_guard();
    report(fuzz(Target::CompileWithRegistry));
}

#[test]
fn fuzz_extract_schema() {
    let _serial = fuzz_support::serial_guard();
    report(fuzz(Target::ExtractSchema));
}

#[test]
fn fuzz_sections_merge() {
    let _serial = fuzz_support::serial_guard();
    report(fuzz(Target::SectionsMerge));
}

/// The subprocess half of the memory ceiling. A CI runner has far less RAM than a developer
/// machine, so the child must die of its own accord and be classified, rather than being
/// OOM-killed by the kernel with no attribution.
///
/// Driven by giving the child an absurdly small ceiling, so ordinary compiler work crosses it.
/// That proves the monitor thread fires and the parent reads the death as a finding, without
/// needing an input that genuinely allocates gigabytes.
#[test]
fn subprocess_enforces_its_own_memory_ceiling() {
    let _serial = fuzz_support::serial_guard();
    let input = b"{repeat 1000}{repeat 1000}hello{/repeat}{/repeat}";
    let out = fuzz_support::run_in_subprocess_with_limit(
        Target::CompileTemplate,
        input,
        Duration::from_secs(120),
        1, // 1 MiB: the child cannot even finish start-up work inside this
    );
    assert!(
        matches!(out, SubOutcome::Crashed(_)),
        "the child must abort on its own ceiling, got {out:?}"
    );
}

/// The harness's own constants have to be valid before any finding they produce can be believed.
///
/// A malformed `DEFAULT_CONTEXT` panics on EVERY input, so the fuzzer reports a panic and the
/// minimiser shrinks the reproducer to a single arbitrary byte - a "finding" that points at the
/// compiler and is really a typo in a test fixture. That happened while the attribute-spread
/// values were being added (a raw NUL, which JSON cannot carry unescaped). This turns the same
/// mistake into a named failure that says so.
#[test]
fn default_context_is_valid_json() {
    let ctx = fuzz_support::default_context();
    let parsed: serde_json::Value = serde_json::from_str(ctx)
        .unwrap_or_else(|e| panic!("DEFAULT_CONTEXT is not valid JSON: {e}"));
    assert!(
        parsed.is_object(),
        "DEFAULT_CONTEXT must be a JSON object, got {parsed}"
    );
    // The attribute-spread keys specifically: a `{html attrs}` whose expression resolves to
    // nothing returns before the sanitizer runs, so a missing key here silently un-covers the
    // path this context exists to reach.
    for key in ["attrs", "attrs2", "more", "flag"] {
        assert!(
            parsed.get(key).is_some(),
            "DEFAULT_CONTEXT lost the `{key}` binding; the attribute-spread alphabet entries              would compile to an empty spread and never reach sanitize_attr_spread"
        );
    }
}
