//! Fuzzing harness for the slurp compiler: watchdog, corpus, mutators, minimiser.
//!
//! # Design
//!
//! Two properties shape the whole harness, and both come from the bug class it hunts: a compiler
//! that fails to terminate, or fails to bound its allocation, on some input.
//!
//! Every target runs under a WATCHDOG, so a hang is REPORTED with the offending input rather than
//! hanging the suite. A harness that measures elapsed time AFTER the call returns cannot observe
//! a hang at all, because the call never returns and the measurement never happens.
//!
//! Inputs come from mutation over a real seed corpus plus a grammar generator whose alphabet is
//! read from the compiler's OWN keyword tables (`slurp_compiler::lexer::block_keywords`), so a
//! keyword added to the lexer is fuzzed the same day. A fixed hand-maintained fragment list can
//! only ever produce inputs somebody already thought of, and the shapes that matter are rarely
//! exotic: one non-terminating lexer state was reached by ORDINARY JavaScript
//! (`<script>const cfg = {elseBranch: 1};</script>`).
//!
//! # The watchdog, and why there are two of them
//!
//! Rust cannot kill a thread, so the in-process watchdog (`run_in_thread`) detects a hang by
//! waiting on a channel with a deadline and then LEAKING the spinning worker. That is fast enough
//! to run tens of thousands of iterations per target, and it catches panics via `catch_unwind`.
//! What it cannot catch is a stack overflow or an abort, because those take the whole process
//! down, and it cannot leak an unbounded number of spinning threads.
//!
//! So confirmation and minimisation run in a SUBPROCESS (`run_in_subprocess`), which re-executes
//! this same test binary against a single input file and is killed on timeout. A subprocess costs
//! milliseconds, which is far too slow for the main loop but exactly right for the handful of runs
//! that follow a suspected hang. It also converts a stack overflow from an uncatchable abort of
//! the test suite into an attributable per-input failure.
//!
//! # Determinism
//!
//! Every run is seeded from a constant. A fuzzer that finds a different bug on each run cannot
//! gate a merge. Override with `SLURP_FUZZ_SEED` to explore beyond the pinned seed.
//!
//! # Environment
//!
//! * `SLURP_FUZZ_ITERS`  - iterations per target (default: a few thousand, CI-sized)
//! * `SLURP_FUZZ_SECS`   - wall-clock budget per target; whichever of the two runs out first wins
//! * `SLURP_FUZZ_SEED`   - PRNG seed
//! * `SLURP_FUZZ_TIMEOUT_MS` - per-input watchdog budget (default 3000)

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Write as _;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Once, mpsc};
use std::time::{Duration, Instant};

use arbitrary::Unstructured;

// ---------------------------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------------------------

/// Every fuzzable entry point. Named in ONE place: the in-process loop, the subprocess replay
/// worker and the corpus directory layout all derive from this, so a target cannot be added to
/// one and missed by the others - the drift shape that leaves an entry point unfuzzed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The lexer alone, across all of its modes.
    Lex,
    /// `slurp_compiler::parse`.
    Parse,
    /// `slurp_compiler::compile_template`, which is lex + parse + security check + render.
    CompileTemplate,
    /// `slurp_compiler::compile_with_registry`, which additionally resolves components/layouts
    /// out of a multi-file registry (so import cycles are reachable).
    CompileWithRegistry,
    /// `extract_schema` and `extract_block_schema`: the frontmatter path the publish endpoint runs.
    ExtractSchema,
    /// The section-state merge, which takes UNTRUSTED editor-supplied JSON.
    SectionsMerge,
}

impl Target {
    pub const ALL: &'static [Target] = &[
        Target::Lex,
        Target::Parse,
        Target::CompileTemplate,
        Target::CompileWithRegistry,
        Target::ExtractSchema,
        Target::SectionsMerge,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Target::Lex => "lex",
            Target::Parse => "parse",
            Target::CompileTemplate => "compile_template",
            Target::CompileWithRegistry => "compile_with_registry",
            Target::ExtractSchema => "extract_schema",
            Target::SectionsMerge => "sections_merge",
        }
    }

    pub fn from_name(s: &str) -> Option<Target> {
        Target::ALL.iter().copied().find(|t| t.name() == s)
    }
}

// ---------------------------------------------------------------------------------------------
// The targets themselves
// ---------------------------------------------------------------------------------------------

/// Split raw bytes into up to `n` parts on a marker, so one flat corpus file can drive a
/// multi-file target. The marker is deliberately a byte sequence a mutator can create by accident.
fn split_parts(data: &[u8], n: usize) -> Vec<&[u8]> {
    let mut parts: Vec<&[u8]> = Vec::new();
    let mut rest = data;
    // BOUNDED: at most `n` splits, and each split consumes at least the marker, so this cannot
    // spin even if `find` keeps matching at index 0.
    while parts.len() + 1 < n {
        match find_sub(rest, b"@@@") {
            Some(i) => {
                parts.push(&rest[..i]);
                rest = &rest[i + 3..];
            }
            None => break,
        }
    }
    parts.push(rest);
    parts
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// A schema-bearing template used when the fuzzed source declares no `section {}` of its own,
/// so the merge target always actually merges something.
const FALLBACK_SECTION_TEMPLATE: &str = r##"---
section {
  name: "Fuzz"
  settings {
    heading: text = "hi"
    count: number = 1
    on: toggle = true
    tone: select(a, b, c) = a
    body: richtext = "<p>x</p>"
    link: link = "/x"
    tint: color = "#fff"
    pic: image = ""
    sym: icon = "check"
  }
  blocks {
    item {
      settings { label: text = "L" }
      blocks { leaf { settings { v: text = "v" } } }
    }
  }
}
---
<div>${ section.settings.heading }</div>
"##;

/// Run one fuzz target against one input. THE dispatch point: the in-process loop and the
/// subprocess replay worker both call this, so they can never diverge on what a target means.
pub fn run_target(target: Target, data: &[u8]) {
    // Byte-level input converted lossily, exactly as libFuzzer targets do. This keeps a corpus
    // file byte-identical to the input that produced a finding, and still feeds the lexer
    // interesting U+FFFD sequences.
    let src = String::from_utf8_lossy(data).into_owned();

    match target {
        Target::Lex => {
            let _ = slurp_compiler::lexer::Lexer::new(&src).tokenize();
        }
        Target::Parse => {
            let _ = slurp_compiler::parse(&src, "fuzz.slurp");
        }
        Target::CompileTemplate => {
            let parts = split_parts(data, 2);
            let source = String::from_utf8_lossy(parts[0]).into_owned();
            // A context that is real JSON when the tail happens to parse, and a rich default
            // otherwise, so renderer paths that need data are reachable.
            let ctx = parts
                .get(1)
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .filter(|s| serde_json::from_str::<serde_json::Value>(s).is_ok())
                .unwrap_or_else(|| DEFAULT_CONTEXT.to_string());
            let opts = slurp_compiler::CompileOptions {
                context_json: Some(ctx),
                is_middleware: data.first().is_some_and(|b| b % 7 == 0),
                ..Default::default()
            };
            let _ = slurp_compiler::compile_template(&source, opts);
        }
        Target::CompileWithRegistry => {
            let parts = split_parts(data, 4);
            let entry = String::from_utf8_lossy(parts[0]).into_owned();
            let mut reg: HashMap<String, String> = HashMap::new();
            // Register every extra part under BOTH key forms. The dual-form registry key is a
            // documented gotcha (`@path` for `using`/`<layout src>`, `path.slurp` for routes);
            // registering only one form makes half the resolution paths unreachable by the fuzzer.
            let names = [
                ("components/A", "@components/A"),
                ("layouts/base", "@layouts/base"),
                ("components/B", "@components/B"),
            ];
            for (i, part) in parts.iter().skip(1).enumerate() {
                if let Some((path, at)) = names.get(i) {
                    let body = String::from_utf8_lossy(part).into_owned();
                    reg.insert(format!("{path}.slurp"), body.clone());
                    reg.insert((*at).to_string(), body);
                }
            }
            // `expect`, not `unwrap`: this is HARNESS data, not fuzzer input, and when it is
            // malformed the failure surfaces as a panic on every single input. The minimiser
            // then shrinks the "reproducer" to one arbitrary byte, because the panic does not
            // depend on the input at all - a finding that looks like a compiler bug and is not.
            // `default_context_is_valid_json` catches it first; this makes it unmistakable if
            // that test is ever removed.
            let ctx: serde_json::Value = serde_json::from_str(DEFAULT_CONTEXT)
                .expect("DEFAULT_CONTEXT is harness data and must be valid JSON");
            let _ = slurp_compiler::compile_with_registry(
                &entry,
                "fuzz.slurp",
                &ctx,
                slurp_compiler::BuildMode::Production,
                Arc::new(reg),
            );
        }
        Target::ExtractSchema => {
            let _ = slurp_compiler::extract_schema(&src, "fuzz.slurp");
            let _ = slurp_compiler::extract_block_schema(&src, "blocks/fuzz.slurp");
        }
        Target::SectionsMerge => {
            let parts = split_parts(data, 2);
            let source = String::from_utf8_lossy(parts[0]).into_owned();

            // The schema comes from the fuzzed frontmatter when it declares one, so schema shapes
            // are fuzzed too; otherwise from a fixed schema, so the merge is always exercised.
            let (doc, _) = match slurp_compiler::parse(&source, "fuzz.slurp") {
                Ok(d) => d,
                Err(_) => slurp_compiler::parse(FALLBACK_SECTION_TEMPLATE, "fb.slurp")
                    .expect("fallback template must parse"),
            };
            let fm = doc.frontmatter.as_ref();
            let owned_fallback;
            let schema = match fm.and_then(|f| f.section.as_ref()) {
                Some(s) => s,
                None => {
                    owned_fallback = slurp_compiler::parse(FALLBACK_SECTION_TEMPLATE, "fb.slurp")
                        .expect("fallback template must parse")
                        .0;
                    match owned_fallback
                        .frontmatter
                        .as_ref()
                        .and_then(|f| f.section.as_ref())
                    {
                        Some(s) => s,
                        None => return,
                    }
                }
            };

            // Editor-supplied state: the raw tail when it is valid JSON, and a generated
            // JSON tree otherwise, so both real-shaped and adversarial-shaped state are covered.
            let tail = parts.get(1).copied().unwrap_or(b"");
            let state: serde_json::Value = serde_json::from_slice(tail)
                .unwrap_or_else(|_| gen_json(&mut Unstructured::new(tail)));

            // A catalog built from the document's own block schema, so `blocks { @theme }`
            // resolution is reachable rather than always hitting the empty-catalog path.
            let mut catalog = slurp_compiler::BlockCatalog::new();
            if let Some(b) = fm.and_then(|f| f.block.as_ref()) {
                catalog.insert("item".to_string(), slurp_compiler::catalog_entry("item", b));
                catalog.insert("leaf".to_string(), slurp_compiler::catalog_entry("leaf", b));
            }
            let _ = slurp_compiler::merged_section_value(schema, &state);
            let _ = slurp_compiler::merged_section_value_with(schema, &state, &catalog);
        }
    }
}

/// The context every generated template renders against.
///
/// `attrs` / `attrs2` / `more` / `flag` exist so the ATTRIBUTE-SPREAD alphabet entries actually
/// reach `sanitize_attr_spread`. A `{html expr}` in tag position returns early when the
/// expression is empty, so without a bound value the construct compiles and the sanitizer is
/// never called - the generated markup would look like coverage while testing nothing. That is
/// close to how the path stayed unfuzzed in the first place, so the values are hostile on
/// purpose: a multi-byte leading character (the reported panic), a native handler, an Alpine
/// bind alias for one, and quote/`=`/NUL punctuation that would end a name.
const DEFAULT_CONTEXT: &str = r##"{
  "name":"n","items":[1,2,3],"nested":{"a":{"b":{"c":"d"}}},
  "html":"<b>x</b>","url":"/p","n":3,"t":true,"f":false,"nul":null,
  "products":[{"title":"t","price":"1.50","tags":["a","b"]}],
  "site":{"name":"s","currency":"USD"},
  "theme":{"settings":{"accent":"#fff"}},
  "params":{"slug":"s"},"section":{"settings":{},"blocks":[]},
  "flag":true,
  "attrs":"€=1 data-id=\"5\" onclick=\"alert(1)\" :onclick=\"x\" x-html=\"y\"",
  "attrs2":"﻿onclick=\"alert(1)\" href=\"javascript:alert(1)\" a\"b=c a>d a\u0000e=f",
  "more":"@click=\"go()\" x-model=\"v\" :disabled=\"l\" disabled x-bind:onclick=\"z\""
}"##;

/// The harness context, exposed so  can check it.
pub fn default_context() -> &'static str {
    DEFAULT_CONTEXT
}

/// A bounded JSON tree driven by fuzzer bytes. DEPTH IS CAPPED HERE ON PURPOSE: the point is to
/// probe the merge's own recursion bound, and an unbounded generator would blow the GENERATOR's
/// stack first and report the wrong culprit.
fn gen_json(u: &mut Unstructured<'_>) -> serde_json::Value {
    fn go(u: &mut Unstructured<'_>, depth: u32) -> serde_json::Value {
        use serde_json::Value;
        // Note `depth >= 24` and the `.take(6)` caps below: both are the generator's own
        // termination proof, needed because an exhausted `Unstructured` keeps returning the
        // same choice rather than erroring.
        if depth >= 24 || u.is_empty() {
            return Value::String("x".into());
        }
        match u.int_in_range(0u8..=8).unwrap_or(0) {
            0 => Value::Null,
            1 => Value::Bool(u.arbitrary().unwrap_or(false)),
            2 => Value::Number(u.arbitrary::<i32>().unwrap_or(0).into()),
            3 => {
                let n = u.int_in_range(0usize..=12).unwrap_or(0);
                Value::String(String::from_utf8_lossy(u.bytes(n).unwrap_or(b"")).into_owned())
            }
            4 | 5 => {
                let n = u.int_in_range(0usize..=6).unwrap_or(0);
                Value::Array((0..n).map(|_| go(u, depth + 1)).collect())
            }
            _ => {
                let n = u.int_in_range(0usize..=6).unwrap_or(0);
                let keys = ["id", "type", "settings", "blocks", "heading", "item", "x"];
                let mut m = serde_json::Map::new();
                for i in 0..n {
                    let k = keys[i % keys.len()].to_string();
                    m.insert(k, go(u, depth + 1));
                }
                Value::Object(m)
            }
        }
    }
    go(u, 0)
}

// ---------------------------------------------------------------------------------------------
// The watchdog
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Completed,
    Panicked(String),
    /// The watchdog fired: the worker did not return inside its budget.
    Hung,
    /// The process-wide memory ceiling was crossed while this input was running.
    OutOfMemory(usize),
}

// ---------------------------------------------------------------------------------------------
// The memory ceiling
// ---------------------------------------------------------------------------------------------

/// A global allocator that counts live bytes.
///
/// Memory is bounded for the same reason time is: a harness that dies of the thing it is meant to
/// find cannot report it. An unbounded allocation is squarely in this bug class - the
/// `{elseBranch}` lexer hang was itself "spin while allocating a token per pass" - and in a shared
/// render service an OOM kill is exactly as fatal as a hang.
///
/// The ceiling has a second job. A hang-detected worker is LEAKED, because Rust cannot kill a
/// thread, and a leaked worker keeps running and keeps allocating. A few leaked workers across
/// two campaigns running in parallel can reach many gigabytes on their own. The ceiling is
/// process-wide precisely so that leaked threads are counted too.
pub struct CountingAlloc;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

/// The ceiling in bytes, cached so the allocation path never touches the environment. 0 means
/// enforcement is off, which is the state before `init_memory_limit` runs and the only safe
/// default: reading an env var allocates, and allocating inside the allocator recurses.
static LIMIT_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Set when a worker has been parked at the ceiling. The watchdog reads this to classify.
static BREACHED: AtomicBool = AtomicBool::new(false);

/// Arm the ceiling for a campaign. Enforcement is off until this runs.
///
/// The ceiling is REBASED on whatever is live right now, so `SLURP_FUZZ_RSS_LIMIT_MB` is a BUDGET
/// for the work about to start rather than an absolute figure. That matters because a worker
/// parked at the ceiling never unwinds and never frees, so its memory is permanently live: an
/// absolute ceiling would shrink with every finding until healthy inputs started tripping it.
/// Serializes every test in the fuzz binary.
///
/// The memory budget, the breach flag and the leaked-worker count are all PROCESS state, so two
/// tests running concurrently see each other's allocations and each other's parked workers. That
/// is not a hypothetical: run in parallel, the hang self-test was reported as an OOM because a
/// concurrent test had already spent the budget. CI passes `--test-threads=1` for the same
/// reason; this makes a plain `cargo test` behave the same way rather than flaking.
pub fn serial_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A failing test poisons the mutex; recovering is right here, because the state the lock
    // protects is re-armed by `init_memory_limit` at the top of every test anyway.
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn init_memory_limit() {
    let budget = env_usize("SLURP_FUZZ_RSS_LIMIT_MB", 256) * 1024 * 1024;
    LIMIT_BYTES.store(live_bytes().saturating_add(budget), Ordering::Relaxed);
}

fn in_fuzz_worker() -> bool {
    // `try_with` because a thread in TLS teardown must not panic here, and `Cell<bool>` has no
    // destructor so this cannot itself allocate.
    IN_FUZZ_WORKER.try_with(|f| f.get()).unwrap_or(false)
}

/// ENFORCEMENT ON THE ALLOCATION PATH, not on a timer.
///
/// A 50 ms poll can observe a slow leak but cannot stop a fast one: a tight allocation loop, or
/// one huge allocation, goes from under the ceiling to many gigabytes between two samples. That
/// is not a hypothetical here - under a polled ceiling, a mutated input can reach tens of
/// gigabytes.
///
/// So the check happens inside `alloc`/`realloc`, before the memory is handed out. On breach the
/// FUZZ WORKER is parked forever: it stops allocating immediately, which is the containment
/// property, and the watchdog then reports it at its leisure. Parking rather than returning null
/// is deliberate - a null return routes to `handle_alloc_error`, which aborts the process and
/// takes the report with it.
///
/// Only fuzz workers park. The main thread holds the corpus and must stay alive to write the
/// reproducer out.
#[cold]
#[inline(never)]
fn park_forever() -> ! {
    BREACHED.store(true, Ordering::SeqCst);
    loop {
        std::thread::park();
    }
}

unsafe impl std::alloc::GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        let n = LIVE_BYTES.fetch_add(l.size(), Ordering::Relaxed) + l.size();
        PEAK_BYTES.fetch_max(n, Ordering::Relaxed);
        let lim = LIMIT_BYTES.load(Ordering::Relaxed);
        if lim != 0 && n > lim && in_fuzz_worker() {
            park_forever();
        }
        let p = unsafe { std::alloc::System.alloc(l) };
        if p.is_null() {
            LIVE_BYTES.fetch_sub(l.size(), Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        LIVE_BYTES.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, new: usize) -> *mut u8 {
        if new > l.size() {
            let n = LIVE_BYTES.fetch_add(new - l.size(), Ordering::Relaxed) + (new - l.size());
            PEAK_BYTES.fetch_max(n, Ordering::Relaxed);
            let lim = LIMIT_BYTES.load(Ordering::Relaxed);
            if lim != 0 && n > lim && in_fuzz_worker() {
                park_forever();
            }
        } else {
            LIVE_BYTES.fetch_sub(l.size() - new, Ordering::Relaxed);
        }
        let q = unsafe { std::alloc::System.realloc(p, l, new) };
        if q.is_null() && new > l.size() {
            LIVE_BYTES.fetch_sub(new - l.size(), Ordering::Relaxed);
        }
        q
    }
}

/// Has a worker been parked at the ceiling since the last reset?
pub fn memory_breached() -> bool {
    BREACHED.load(Ordering::SeqCst)
}

pub fn reset_memory_breach() {
    BREACHED.store(false, Ordering::SeqCst);
}

/// Live heap bytes across the whole process, leaked workers included.
pub fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

pub fn peak_bytes() -> usize {
    PEAK_BYTES.load(Ordering::Relaxed)
}

/// The process-wide ceiling, in bytes. Deliberately in libFuzzer's usual range (its
/// `-rss_limit_mb` defaults to 2048). It is PROCESS-wide rather than per-target, so the
/// worst-case footprint of a campaign is this number regardless of how many test threads
/// `cargo test` decides to run, plus whatever a worker allocates between two polls.
pub fn memory_limit_bytes() -> usize {
    let v = LIMIT_BYTES.load(Ordering::Relaxed);
    if v == 0 {
        env_usize("SLURP_FUZZ_RSS_LIMIT_MB", 256) * 1024 * 1024
    } else {
        v
    }
}

/// How often the watchdog samples memory while waiting. Small enough that a runaway is caught
/// within a few hundred MB of the ceiling rather than a few GB past it.
const MEMORY_POLL: Duration = Duration::from_millis(50);

static QUIET_HOOK: Once = Once::new();
static LEAKED_THREADS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static IN_FUZZ_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Silence panic output for panics raised INSIDE a fuzz worker only. A fuzz run provokes many
/// panics on purpose and the default hook would bury the actual report in backtraces; panics in
/// the harness's own assertions must still print, which is why this is thread-scoped rather than
/// a blanket `set_hook(|_| {})`.
fn install_quiet_hook() {
    QUIET_HOOK.call_once(|| {
        let prev = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if IN_FUZZ_WORKER.with(|f| f.get()) {
                return;
            }
            prev(info);
        }));
    });
}

fn describe_panic(e: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// How many hung workers we tolerate before giving up on the in-process loop. Each one is a
/// thread spinning on a core that Rust gives us no way to reclaim, so this is a real resource
/// bound and not a stylistic one.
const MAX_LEAKED_THREADS: usize = 3;

/// Run one input under a watchdog IN THIS PROCESS.
///
/// THE central property of this harness: on a hang we return `Outcome::Hung` to a caller that is
/// still alive and can print the offending input. The worker thread is leaked, because Rust
/// offers no way to stop it; `LEAKED_THREADS` bounds how often that is allowed to happen.
pub fn run_in_thread(target: Target, data: Vec<u8>, budget: Duration) -> Outcome {
    run_guarded(target.name(), budget, move || run_target(target, &data))
}

/// The watchdog primitive, independent of any target so it can be SELF-TESTED against a closure
/// that is known to spin. `watchdog_reports_a_hang_instead_of_hanging` does exactly that: without
/// a test that provokes a real hang, "the harness catches hangs" is an untested claim.
pub fn run_guarded<F>(name: &str, budget: Duration, body: F) -> Outcome
where
    F: FnOnce() + Send + 'static,
{
    install_quiet_hook();
    let (tx, rx) = mpsc::channel::<Option<String>>();
    let spawned = std::thread::Builder::new()
        .name(format!("fuzz-{name}"))
        // A deliberately generous stack: the crate grows its own stack with `stacker`, and a
        // stack overflow here would abort the whole test process rather than report an input.
        // The subprocess path is what actually catches overflow; this only makes it rarer.
        .stack_size(4 * 1024 * 1024)
        .spawn(move || {
            IN_FUZZ_WORKER.with(|f| f.set(true));
            let res = panic::catch_unwind(AssertUnwindSafe(body));
            let _ = tx.send(res.err().map(describe_panic));
        });

    let handle = match spawned {
        Ok(h) => h,
        // Thread exhaustion (usually caused by earlier leaks) is not a finding about the input.
        Err(_) => return Outcome::Completed,
    };

    // Sampled wait rather than one blocking `recv_timeout(budget)`. Blocking for the whole budget
    // lets an allocation runaway reach several GB unobserved: the deadline has not passed, so
    // nothing checks. Memory is checked every `MEMORY_POLL` instead.
    let deadline = Instant::now() + budget;
    let limit = memory_limit_bytes();
    loop {
        match rx.recv_timeout(MEMORY_POLL) {
            Ok(None) => {
                let _ = handle.join();
                return Outcome::Completed;
            }
            Ok(Some(msg)) => {
                let _ = handle.join();
                return Outcome::Panicked(msg);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // The worker vanished without sending. Treat as completed rather than inventing
                // a finding; a real abort would have taken this process with it.
                return Outcome::Completed;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // The ALLOCATOR has already stopped the runaway by parking the worker, so this
                // poll is only about reporting, not containment. Containment on the allocation
                // path is what makes the poll interval a reporting latency rather than a race.
                if memory_breached() {
                    LEAKED_THREADS.fetch_add(1, Ordering::Relaxed);
                    return Outcome::OutOfMemory(peak_bytes());
                }
                let live = live_bytes();
                if live > limit {
                    LEAKED_THREADS.fetch_add(1, Ordering::Relaxed);
                    return Outcome::OutOfMemory(live);
                }
                if Instant::now() >= deadline {
                    LEAKED_THREADS.fetch_add(1, Ordering::Relaxed);
                    return Outcome::Hung;
                }
            }
        }
    }
}

pub fn leaked_threads() -> usize {
    LEAKED_THREADS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------------------------
// The subprocess watchdog
// ---------------------------------------------------------------------------------------------

pub const REPLAY_TARGET_ENV: &str = "SLURP_FUZZ_REPLAY_TARGET";
pub const REPLAY_FILE_ENV: &str = "SLURP_FUZZ_REPLAY_FILE";
/// The name of the `#[test]` that acts as the single-input replay worker. Kept next to the
/// spawn code so the filter string and the test name cannot drift.
pub const REPLAY_TEST_NAME: &str = "zzz_fuzz_replay_worker";

#[derive(Debug, Clone, PartialEq)]
pub enum SubOutcome {
    /// Exited 0.
    Completed,
    /// Exited non-zero: a panic, an abort, or a stack overflow.
    Crashed(String),
    /// Still running at the deadline; we killed it.
    Hung,
}

/// Run one input in a FRESH PROCESS, killed on timeout.
///
/// This is the only path that can survive the two failure modes the in-process watchdog cannot:
/// a stack overflow (an uncatchable abort) and a hang we need to reproduce more than a handful of
/// times, as minimisation does. It costs a process spawn per call, which is why it is not the
/// main loop.
pub fn run_in_subprocess(target: Target, data: &[u8], budget: Duration) -> SubOutcome {
    run_in_subprocess_with_limit(target, data, budget, memory_limit_bytes() / (1024 * 1024))
}

/// As `run_in_subprocess`, with an explicit child memory ceiling in MiB. The ceiling is passed
/// EXPLICITLY rather than left to environment inheritance, so a CI job that sets a limit is
/// guaranteed to have it reach the children too; and so the ceiling itself can be tested.
pub fn run_in_subprocess_with_limit(
    target: Target,
    data: &[u8],
    budget: Duration,
    limit_mb: usize,
) -> SubOutcome {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return SubOutcome::Completed,
    };
    let path = match write_temp(data) {
        Some(p) => p,
        None => return SubOutcome::Completed,
    };

    let child = std::process::Command::new(exe)
        .args([
            "--exact",
            REPLAY_TEST_NAME,
            "--test-threads",
            "1",
            "--quiet",
        ])
        .env(REPLAY_TARGET_ENV, target.name())
        .env(REPLAY_FILE_ENV, &path)
        .env("SLURP_FUZZ_RSS_LIMIT_MB", limit_mb.to_string())
        // Keep the child from re-entering the fuzz loop or printing noise.
        .env("SLURP_FUZZ_ITERS", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            return SubOutcome::Completed;
        }
    };

    let deadline = Instant::now() + budget;
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    SubOutcome::Completed
                } else {
                    SubOutcome::Crashed(format!("exit status {status}"))
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break SubOutcome::Hung;
                }
                // BOUNDED: the loop is bounded by `deadline`, which is a wall-clock instant, so
                // it terminates regardless of what the child does.
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break SubOutcome::Completed,
        }
    };

    let _ = std::fs::remove_file(&path);
    outcome
}

fn write_temp(data: &[u8]) -> Option<PathBuf> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!(
        "slurp-fuzz-{}-{}-{}.bin",
        std::process::id(),
        n,
        Instant::now().elapsed().as_nanos()
    ));
    let mut f = std::fs::File::create(&p).ok()?;
    f.write_all(data).ok()?;
    Some(p)
}

/// The body of the replay worker `#[test]`. Returns `false` when no replay was requested, which is
/// what happens on an ordinary `cargo test` run.
pub fn replay_worker_if_requested() -> bool {
    let (Ok(t), Ok(f)) = (
        std::env::var(REPLAY_TARGET_ENV),
        std::env::var(REPLAY_FILE_ENV),
    ) else {
        return false;
    };
    let Some(target) = Target::from_name(&t) else {
        return false;
    };
    let data = std::fs::read(f).unwrap_or_default();
    init_memory_limit();

    // The child enforces the ceiling on ITSELF, by aborting. That is exactly why confirmation
    // runs out of process: the parent cannot stop a runaway thread, but it can observe a dead
    // child and classify the input. `abort` rather than `exit` so the death is unambiguous and
    // no destructor can hang on the way out.
    let limit = memory_limit_bytes();
    let _monitor = std::thread::Builder::new()
        .name("rss-monitor".into())
        .spawn(move || {
            loop {
                let live = live_bytes();
                if live > limit {
                    eprintln!("slurp fuzz replay: memory ceiling exceeded ({live} bytes)");
                    std::process::abort();
                }
                std::thread::sleep(MEMORY_POLL);
            }
        });

    run_target(target, &data);
    true
}

// ---------------------------------------------------------------------------------------------
// Minimiser
// ---------------------------------------------------------------------------------------------

/// Does this input still reproduce the failure? Confirmed in a subprocess so that a hang can be
/// asked repeatedly without leaking a thread per question.
fn still_fails(target: Target, data: &[u8], budget: Duration) -> bool {
    !matches!(
        run_in_subprocess(target, data, budget),
        SubOutcome::Completed
    )
}

/// Delta-debugging minimiser: shrink by deleting spans, keeping any deletion that still fails.
///
/// Every loop here is bounded by an explicit trial budget as well as by its own shrinking span
/// size, because the thing being minimised is by definition an input that does not terminate.
pub fn minimise(target: Target, input: &[u8], budget: Duration, max_trials: usize) -> Vec<u8> {
    let mut best = input.to_vec();
    let mut trials = 0usize;
    let mut span = best.len().max(1);

    while span > 0 && trials < max_trials {
        let mut start = 0usize;
        let mut progressed = false;
        while start < best.len() && trials < max_trials {
            let end = (start + span).min(best.len());
            let mut cand = Vec::with_capacity(best.len() - (end - start));
            cand.extend_from_slice(&best[..start]);
            cand.extend_from_slice(&best[end..]);
            trials += 1;
            if !cand.is_empty() && still_fails(target, &cand, budget) {
                best = cand;
                progressed = true;
                // Do not advance `start`: the next span now sits at the same offset. `best` got
                // strictly shorter, so this cannot loop forever.
            } else {
                start = end.max(start + 1);
            }
        }
        if !progressed && span == 1 {
            break;
        }
        span /= 2;
    }
    best
}

// ---------------------------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------------------------

pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz")
        .join("corpus")
}

/// Seed inputs for a target: its own directory plus the shared `common` directory. Sharing keeps
/// one copy of the real theme files instead of six.
pub fn load_corpus(target: Target) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for dir in [
        corpus_root().join("common"),
        corpus_root().join(target.name()),
    ] {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        // Sorted so the corpus order - and therefore the whole run - is reproducible across
        // filesystems that do not enumerate in a stable order.
        files.sort();
        for f in files {
            if let Ok(b) = std::fs::read(&f) {
                out.push(b);
            }
        }
    }
    if out.is_empty() {
        out.push(b"<p>${x}</p>".to_vec());
    }
    out
}

/// Where a reproducer is written when the fuzzer finds something.
pub fn crash_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz")
        .join("crashes")
}

pub fn save_repro(target: Target, kind: &str, data: &[u8]) -> Option<PathBuf> {
    let dir = crash_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    let p = dir.join(format!("{}-{}-{:016x}.bin", target.name(), kind, h));
    std::fs::write(&p, data).ok()?;
    Some(p)
}

// ---------------------------------------------------------------------------------------------
// Generation and mutation
// ---------------------------------------------------------------------------------------------

/// The token alphabet. Every entry that the compiler ALSO knows about is read from the compiler
/// rather than copied: `block_keywords()` and `raw_brace_openers()` come straight out of
/// `lexer`. The remaining entries are shapes the lexer has no table for (HTML, quotes, JS).
pub fn alphabet() -> Vec<String> {
    let mut a: Vec<String> = Vec::new();

    // Read from the compiler's OWN tables: a keyword added to the lexer becomes fuzzed input on
    // the same commit, with nothing here to update. This is the direct countermeasure to a
    // hand-maintained list drifting out of sync with the lexer.
    for k in slurp_compiler::lexer::block_keywords() {
        a.push(format!("{{{k} x}}"));
        a.push(format!("{{{k}}}"));
        a.push(format!("{{/{k}}}"));
        a.push(format!("{{{k}"));
    }
    for o in slurp_compiler::lexer::raw_brace_openers() {
        a.push((*o).to_string());
        a.push(format!("{o} x}}"));
    }

    // Shapes the lexer decides structurally rather than from a table.
    for s in [
        // Expressions and interpolation.
        "${",
        "}",
        "${}",
        "${x}",
        "${x|",
        "${x | upper}",
        "${ a ?? b }",
        "${a.b.c}",
        "${a?.b?.c}",
        "${'",
        "${\"",
        "${`",
        "${x(",
        "${[",
        "${x[0]}",
        "${x ? y : z}",
        // Braces and JS-shaped text, the family that produced the `{else` lexer hang.
        "{",
        "}",
        "{{",
        "}}",
        "{else}",
        "{else if x}",
        "{elseBranch: 1}",
        "{elsewhere}",
        "{ifx}",
        "{iffy: 2}",
        "{eachOther}",
        "{empty}",
        "{next}",
        "{$let a = 1}",
        "const cfg = {elseBranch: 1};",
        "if(a){b}else{c}",
        "x => { return {a:1}; }",
        "{*",
        "*}",
        "{* c *}",
        // Raw-text mode transitions, where non-termination has historically lived.
        "<script>",
        "</script>",
        "<script type=\"module\">",
        "<style>",
        "</style>",
        "<script>`${",
        "</scr",
        "<SCRIPT>",
        "</SCRIPT>",
        // Frontmatter, the other historically fragile mode.
        "---",
        "---\n",
        "\n---\n",
        "section {",
        "block {",
        "settings {",
        "blocks {",
        "name: \"x\"",
        "heading: text = \"h\"",
        "n: number = 1",
        "t: toggle = true",
        "s: select(a, b) = a",
        "c: color = \"#fff\"",
        "i: icon = \"x\"",
        "l: link = \"/\"",
        "img: image = \"\"",
        "r: richtext = \"<p>x</p>\"",
        "@theme",
        "@theme(a, b)",
        "accepts",
        "max: 3",
        "using \"@components/X\"",
        "import type { A } from \"./a\"",
        "props {",
        "let x = 1",
        "const y = 2",
        // HTML.
        "<div>",
        "</div>",
        "<div ",
        "<a href=\"${x}\">",
        "<img src=${x}>",
        "<slot/>",
        "<slot>",
        "<layout src=\"@layouts/base\">",
        "</layout>",
        "<!--",
        "-->",
        "<!",
        "<",
        ">",
        "</",
        "/>",
        "x-on:click=\"f()\"",
        ":href=\"u\"",
        "{...spread}",
        "<Comp prop={x}>",
        "</Comp>",
        // ATTRIBUTE SPREAD. `{html expr}` in TAG position is the widest data-to-markup seam in
        // the renderer - it turns a rendered value straight into attribute markup - and it was
        // entirely unreachable by this fuzzer: no seed contained the construct and no entry
        // above could build one. A CRITICAL panic in `is_safe_attr_name` survived 420,000
        // iterations for exactly that reason. The construct needs its own tokens because it is
        // the only place a `{html}` opener is legal INSIDE a tag, so mutating text-position
        // `{html ...}` never produces it.
        "<div {html attrs}>",
        "<a {html attrs}>",
        "<button {html attrs} class=\"c\">",
        "<div {html attrs} {html more}>",
        "{html attrs}",
        "{html attrs2}",
        "{html attrs sanitize}",
        // The conditional form, which is a separate parser branch (`{if}` + `{html}` + `{/if}`
        // scanned inside the tag) and so a separate way to reach the same sanitizer.
        "<div {if flag}{html attrs}{/if}>",
        "{if flag}{html attrs}{/if}",
        // Hostile attribute NAMES as bare fragments, so mutation can splice them into a spread
        // value or a context string. These are the shapes the name gate actually has to refuse.
        "\u{20ac}=1",
        "onclick=\"alert(1)\"",
        "ONCLICK=x",
        ":onclick=x",
        "x-bind:onclick=x",
        "X-BIND:ONCLICK=x",
        "x-html=x",
        "x-data=x",
        "a\"onclick=\"b",
        "a'b=c",
        "a>b",
        "a=b=c",
        "a\0b=c",
        // Raw hostile bytes and confusables.
        "\"",
        "'",
        "`",
        "\\",
        "&",
        "\0",
        "\r\n",
        "\t",
        "\u{feff}",
        "\u{202e}",
        "\u{10ffff}",
        "@@@",
    ] {
        a.push(s.to_string());
    }
    a
}

/// Mutation operators. Structure-aware where it matters (wrapping input in a raw-text or
/// frontmatter context is what reaches the mode transitions where hangs have historically lived).
fn mutate(u: &mut Unstructured<'_>, seed: &[u8], corpus: &[Vec<u8>], alpha: &[String]) -> Vec<u8> {
    /// Hard ceiling on a generated input. Without it, repeat-and-splice mutations grow without
    /// bound and the fuzzer OOMs itself rather than the compiler.
    const MAX_LEN: usize = 64 * 1024;

    let mut v = seed.to_vec();
    // BOUNDED: at most 6 operators per input, so an exhausted `Unstructured` (which keeps
    // returning the same choice) cannot spin here.
    let n_ops = u.int_in_range(1usize..=6).unwrap_or(1);
    for _ in 0..n_ops {
        if v.len() > MAX_LEN {
            v.truncate(MAX_LEN);
        }
        let len = v.len();
        let pick = |u: &mut Unstructured<'_>, n: usize| -> usize {
            if n == 0 {
                0
            } else {
                u.int_in_range(0usize..=n - 1).unwrap_or(0)
            }
        };
        match u.int_in_range(0u8..=10).unwrap_or(0) {
            // Byte flip.
            0 if len > 0 => {
                let i = pick(u, len);
                v[i] ^= u.arbitrary::<u8>().unwrap_or(1).max(1);
            }
            // Delete a span. Terminators vanish, which is how unterminated constructs appear.
            1 if len > 1 => {
                let i = pick(u, len);
                let j = (i + u.int_in_range(1usize..=64).unwrap_or(1)).min(len);
                v.drain(i..j);
            }
            // Duplicate a span.
            2 if len > 1 => {
                let i = pick(u, len);
                let j = (i + u.int_in_range(1usize..=128).unwrap_or(1)).min(len);
                let chunk = v[i..j].to_vec();
                let at = pick(u, len);
                v.splice(at..at, chunk);
            }
            // Insert an alphabet token.
            3 => {
                let t = alpha.get(pick(u, alpha.len())).cloned().unwrap_or_default();
                let at = pick(u, len.max(1)).min(len);
                v.splice(at..at, t.into_bytes());
            }
            // Replace a span with an alphabet token.
            4 if len > 0 => {
                let i = pick(u, len);
                let j = (i + u.int_in_range(1usize..=32).unwrap_or(1)).min(len);
                let t = alpha.get(pick(u, alpha.len())).cloned().unwrap_or_default();
                v.splice(i..j, t.into_bytes());
            }
            // Splice with another corpus entry.
            5 if !corpus.is_empty() => {
                let other = &corpus[pick(u, corpus.len())];
                let cut_a = pick(u, len.max(1)).min(len);
                let cut_b = pick(u, other.len().max(1)).min(other.len());
                v.truncate(cut_a);
                v.extend_from_slice(&other[cut_b..]);
            }
            // Truncate: the cheapest way to produce an unterminated construct.
            6 if len > 1 => {
                let i = pick(u, len);
                v.truncate(i);
            }
            // Repeat a span to force deep nesting. Bounded so this probes the compiler's depth
            // caps rather than the machine's memory.
            7 if len > 0 => {
                let i = pick(u, len);
                let j = (i + u.int_in_range(1usize..=24).unwrap_or(1)).min(len);
                let chunk = v[i..j].to_vec();
                let reps = u.int_in_range(2usize..=400).unwrap_or(2);
                let room = MAX_LEN.saturating_sub(v.len()) / chunk.len().max(1);
                let mut ins = Vec::new();
                for _ in 0..reps.min(room) {
                    ins.extend_from_slice(&chunk);
                }
                let at = pick(u, len);
                v.splice(at..at, ins);
            }
            // MODE WRAPPING. This is the operator that reaches the mode transitions: it takes any
            // input and re-reads it as a <script> body, a <style> body, frontmatter, a match arm
            // or a template literal.
            8 => {
                let (pre, post): (&str, &str) = *[
                    ("<script>", "</script>"),
                    ("<style>", "</style>"),
                    ("---\n", "\n---\n"),
                    ("{match x}", "{/match}"),
                    ("`", "`"),
                    ("<script>`", "`</script>"),
                    ("${", "}"),
                    ("<div attr=\"", "\">"),
                    ("{if x}", "{/if}"),
                    ("---\nsection {\n", "\n}\n---\n"),
                ]
                .get(pick(u, 10))
                .unwrap_or(&("<script>", "</script>"));
                let mut n = Vec::with_capacity(v.len() + pre.len() + post.len());
                n.extend_from_slice(pre.as_bytes());
                n.extend_from_slice(&v);
                n.extend_from_slice(post.as_bytes());
                v = n;
            }
            // Build a fresh input from the alphabet alone: reaches shapes no seed contains.
            9 => {
                let n = u.int_in_range(1usize..=60).unwrap_or(1);
                let mut n_v = Vec::new();
                for _ in 0..n {
                    if let Some(t) = alpha.get(pick(u, alpha.len())) {
                        n_v.extend_from_slice(t.as_bytes());
                    }
                }
                v = n_v;
            }
            // Insert raw bytes, including invalid UTF-8, which becomes U+FFFD after the lossy
            // conversion every target does.
            _ => {
                let n = u.int_in_range(1usize..=16).unwrap_or(1);
                let bytes = u.bytes(n).unwrap_or(b"\xff\xfe").to_vec();
                let at = pick(u, len.max(1)).min(len);
                v.splice(at..at, bytes);
            }
        }
    }
    if v.len() > MAX_LEN {
        v.truncate(MAX_LEN);
    }
    v
}

// ---------------------------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------------------------

fn env_usize(k: &str, default: usize) -> usize {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub struct Stats {
    pub target: Target,
    pub iterations: usize,
    pub elapsed: Duration,
    pub panics: usize,
    pub hangs: usize,
    /// Inputs that crossed the process memory ceiling.
    pub ooms: usize,
    /// Inputs that missed the fast budget but completed under the confirmation budget.
    pub slow: usize,
    pub corpus_grown_to: usize,
}

/// A cheap novelty signal so the in-memory corpus grows toward inputs that behave differently.
///
/// This is NOT coverage. Stable Rust gives no coverage feedback without a separate instrumented
/// build, so this uses the observable result shape instead - whether the call succeeded, the
/// first line of any diagnostic, and bucketed output size. It is weaker than libFuzzer's edge
/// coverage and is described that way deliberately; what it does buy is that an input which
/// provokes a NEW diagnostic gets kept and mutated further.
fn signature(target: Target, data: &[u8]) -> u64 {
    let src = String::from_utf8_lossy(data);
    let mut feat = String::new();
    match target {
        Target::Lex | Target::CompileWithRegistry | Target::SectionsMerge => {
            match slurp_compiler::lexer::Lexer::new(&src).tokenize() {
                Ok(t) => feat.push_str(&format!("ok:{}", t.len().next_power_of_two())),
                Err(e) => feat.push_str(&format!(
                    "err:{}",
                    &e.to_string().chars().take(40).collect::<String>()
                )),
            }
        }
        _ => match slurp_compiler::parse(&src, "f.slurp") {
            Ok((d, acc)) => feat.push_str(&format!(
                "ok:{}:{}:{}",
                d.children.len().next_power_of_two(),
                acc.errors.len().min(8),
                d.frontmatter.is_some()
            )),
            Err(e) => feat.push_str(&format!(
                "err:{}",
                &e.to_string().chars().take(40).collect::<String>()
            )),
        },
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in feat.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Fuzz one target. Fails the test on the first hang or panic, after minimising and printing the
/// offending input.
pub fn fuzz(target: Target) -> Stats {
    init_memory_limit();
    let iters = env_usize("SLURP_FUZZ_ITERS", 4_000);
    let secs = env_usize("SLURP_FUZZ_SECS", 25);
    let seed = env_usize("SLURP_FUZZ_SEED", 0x5eed) as u64;
    let per_input = Duration::from_millis(env_usize("SLURP_FUZZ_TIMEOUT_MS", 3_000) as u64);

    let mut corpus = load_corpus(target);
    let alpha = alphabet();
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();

    let started = Instant::now();
    let deadline = started + Duration::from_secs(secs as u64);
    let mut panics = 0usize;
    let mut hangs = 0usize;
    let mut ooms = 0usize;
    let mut slow = 0usize;
    let mut i = 0usize;

    // The budget a suspected hang must ALSO miss before it counts as one. Generous on purpose:
    // the cost of a false negative here is one missed finding on one iteration, while the cost
    // of a false positive is a flaky gate.
    let confirm_budget = Duration::from_millis(env_usize("SLURP_FUZZ_CONFIRM_MS", 60_000) as u64);

    // The entropy stream. Regenerated per iteration from a counter-based hash so the run is
    // fully reproducible from `seed` and the iteration index alone: a failure at iteration N is
    // re-reachable without replaying N-1 mutations.
    while i < iters && Instant::now() < deadline {
        let mut ent = [0u8; 512];
        let mut x = seed ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for chunk in ent.chunks_mut(8) {
            x ^= x >> 30;
            x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x ^= x >> 27;
            x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^= x >> 31;
            chunk.copy_from_slice(&x.to_le_bytes()[..chunk.len()]);
        }
        let mut u = Unstructured::new(&ent);
        let base_idx = if corpus.is_empty() {
            0
        } else {
            u.int_in_range(0..=corpus.len() - 1).unwrap_or(0)
        };
        let base = corpus.get(base_idx).cloned().unwrap_or_default();
        let input = mutate(&mut u, &base, &corpus, &alpha);

        match run_in_thread(target, input.clone(), per_input) {
            Outcome::Completed => {}
            Outcome::Panicked(msg) => {
                panics += 1;
                report_finding(target, "panic", &input, &msg, per_input);
            }
            Outcome::Hung => {
                // CONFIRM before reporting. The in-process budget is sized for throughput, and a
                // debug build under a parallel `cargo test` is slow enough that a legitimately
                // BOUNDED computation can blow it: `{repeat 1000}{repeat 1000}` is capped by the
                // renderer's 1M-iteration budget and still takes 2.4 s in debug. Reporting that
                // as a hang would make the fuzzer a flaky gate, and a flaky gate stops being read.
                //
                // So a suspected hang is re-run in a subprocess with a far larger budget. Only
                // an input that misses THAT deadline is a real non-termination.
                match run_in_subprocess(target, &input, confirm_budget) {
                    SubOutcome::Completed => {
                        slow += 1;
                        if slow <= 3 {
                            eprintln!(
                                "[fuzz] {} : slow input ({} bytes) exceeded {:?} but completed within {:?}; not a hang",
                                target.name(),
                                input.len(),
                                per_input,
                                confirm_budget
                            );
                        }
                    }
                    _ => {
                        hangs += 1;
                        report_finding(
                            target,
                            "hang",
                            &input,
                            "confirmed in a subprocess: no return inside the confirmation budget",
                            confirm_budget,
                        );
                    }
                }
            }
            Outcome::OutOfMemory(live) => {
                // A first-class finding, reported exactly like a hang or a panic. Unbounded
                // allocation is the same bug class: the process dies either way, and in a shared
                // render service it takes every other tenant with it.
                //
                // No subprocess confirmation here, and that is deliberate. The leaked worker is
                // STILL allocating in this process, so every extra second races the runaway
                // against the ceiling. Report immediately; the saved input replays under the
                // child's own ceiling, which aborts rather than growing.
                ooms += 1;
                report_finding(
                    target,
                    "oom",
                    &input,
                    &format!(
                        "process-wide live heap reached {live} bytes, over the {} byte ceiling",
                        memory_limit_bytes()
                    ),
                    confirm_budget,
                );
            }
        }

        // Grow the corpus toward inputs that behave differently. Guarded by the same watchdog
        // budget, because computing the signature runs the compiler again.
        //
        // The bounds here are the harness's OWN footprint, which counts against the same ceiling
        // the compiler does: 4000 entries at the 64 KiB mutation ceiling is 256 MB per target
        // before the compiler allocates anything, and two targets in parallel double it. 1200
        // entries capped at 16 KiB each is ~19 MB and loses nothing, because the interesting
        // inputs this fuzzer finds are all tiny.
        //
        // Corpus feedback runs INLINE, in the same watchdogged worker as the target itself.
        // Spawning a SECOND thread per iteration and abandoning it on timeout without counting it
        // would be an unbounded invisible thread leak, one per slow iteration; running it inline
        // also halves the thread churn of the whole campaign.
        if corpus.len() < 1_200 && input.len() <= 16 * 1024 {
            let sig_input = input.clone();
            let (tx, rx) = mpsc::channel();
            let outcome = run_guarded(target.name(), per_input, move || {
                let s =
                    panic::catch_unwind(AssertUnwindSafe(|| signature(target, &sig_input))).ok();
                let _ = tx.send(s);
            });
            if outcome == Outcome::Completed {
                if let Ok(Some(sig)) = rx.try_recv() {
                    if seen.insert(sig) {
                        corpus.push(input);
                    }
                }
            }
        }

        // Leaked workers keep running and keep allocating, so both of these bound the
        // campaign's footprint rather than merely tidying up after it.
        if leaked_threads() > MAX_LEAKED_THREADS {
            break;
        }
        if live_bytes() > memory_limit_bytes() / 2 {
            eprintln!(
                "[fuzz] {} : stopping early, live heap {} is over half the ceiling",
                target.name(),
                live_bytes()
            );
            break;
        }
        i += 1;
    }

    Stats {
        target,
        iterations: i,
        elapsed: started.elapsed(),
        panics,
        hangs,
        ooms,
        slow,
        corpus_grown_to: corpus.len(),
    }
}

static REPORTED: AtomicBool = AtomicBool::new(false);

/// Minimise, save and then FAIL. The whole point of the harness is that this function is
/// REACHABLE at all: a harness that hangs on a hang never gets here.
fn report_finding(target: Target, kind: &str, input: &[u8], msg: &str, budget: Duration) {
    // Only the first finding is minimised; minimisation is the expensive part and one
    // reproducible finding is enough to fail the run and start a triage.
    if REPORTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let small = minimise(target, input, budget, 400);
    let saved = save_repro(target, kind, &small);

    let mut out = String::new();
    out.push_str(&format!(
        "\n=== SLURP FUZZ {}: {} ===\n",
        kind.to_uppercase(),
        target.name()
    ));
    out.push_str(&format!("detail   : {msg}\n"));
    out.push_str(&format!("orig len : {}\n", input.len()));
    out.push_str(&format!("min len  : {}\n", small.len()));
    out.push_str(&format!(
        "min input (lossy): {:?}\n",
        String::from_utf8_lossy(&small)
    ));
    out.push_str(&format!(
        "min input (bytes): {:02x?}\n",
        &small[..small.len().min(400)]
    ));
    if let Some(p) = saved {
        out.push_str(&format!("saved    : {}\n", p.display()));
    }
    out.push_str("=========================================\n");
    eprintln!("{out}");
    panic!(
        "slurp fuzz found a {kind} in target `{}`{out}",
        target.name()
    );
}
