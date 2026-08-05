// Deterministic fuzzer for the slurp compiler.
//
// WHY NOT cargo-fuzz. libFuzzer needs a nightly toolchain and a separate `fuzz/` crate, and it is
// run out-of-band - which in practice means rarely. This repo pins a stable toolchain and gates on
// `cargo test`, so a fuzzer that is a plain `#[test]` is one that actually runs on every change. It
// trades raw coverage for being present in CI permanently. A cargo-fuzz target can be added later
// and reuse the same generators and the same invariants.
//
// Determinism is a feature, not a limitation: a fuzzer that finds a different bug on every run
// cannot gate a merge. The PRNG is a fixed-seed xorshift, so a failure reproduces exactly, and the
// failure message prints the offending input.
//
// THREE INVARIANTS, in increasing order of what they buy us:
//
//   1. The compiler never PANICS. A panic in the publish path is a 500 on a user action, and a
//      panic reachable from theme text is reachable by an AI-authored theme.
//   2. The compiler always TERMINATES. Loop and depth caps exist; this checks nothing routes around
//      them.
//   3. DATA can never produce an executable construct. This is the one that matters: templates are
//      first-party, but the DATA rendered through them is untrusted (titles, review bodies,
//      display names). Invariant 3 is the fuzzed form of the whole xss_regression.rs suite.

use std::time::{Duration, Instant};

use slurp_compiler::{CompileOptions, compile_template};

/// Fixed-seed xorshift64*. Reproducible by construction - no `rand`, no clock, no entropy.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// Fragments chosen to sit on the boundaries the parser cares about: block openers without closers,
/// unbalanced braces and quotes, filters, nesting, and raw control characters.
const FRAGMENTS: &[&str] = &[
    "${",
    "}",
    "${}",
    "${x}",
    "${x|",
    "${x | upper}",
    "${ x ?? y }",
    "${a.b.c}",
    "${a?.b?.c}",
    "{if x}",
    "{/if}",
    "{else}",
    "{each xs as x}",
    "{/each}",
    "{match x}",
    "{/match}",
    "{fetch url}",
    "{/fetch}",
    "{$let a = 1}",
    "{html x}",
    "{html x sanitize}",
    "{sections}",
    "{sections \"header\"}",
    "<div>",
    "</div>",
    "<a href=\"${x}\">",
    "<img src=${x}>",
    "<script>",
    "</script>",
    "\"",
    "'",
    "`",
    "\\",
    "<",
    ">",
    "&",
    "\0",
    "\r\n",
    "\t",
    "{{",
    "}}",
    "{",
    "}",
    "[",
    "]",
    "(",
    ")",
    "using \"@components/X\"",
    "<slot/>",
    "<layout src=\"@layouts/base\">",
    "x-on:click=\"f()\"",
    ":href=\"u\"",
    "${'",
    "\u{feff}",
    "\u{202e}",
    "\u{0}\u{1}\u{2}",
    "\u{10ffff}",
];

/// Hostile DATA values. These go through a FIXED, well-formed template - the template is trusted,
/// the data is not, which is exactly the real threat model.
const HOSTILE_VALUES: &[&str] = &[
    "<script>alert(1)</script>",
    "<ScRiPt>alert(1)</ScRiPt>",
    "<img src=x onerror=alert(1)>",
    "<svg/onload=alert(1)>",
    "javascript:alert(1)",
    "JaVaScRiPt:alert(1)",
    "java\tscript:alert(1)",
    "\u{feff}javascript:alert(1)",
    "\u{a0}javascript:alert(1)",
    "data:text/html,<script>alert(1)</script>",
    "vbscript:msgbox(1)",
    r#"" onload="alert(1)"#,
    r#"');alert(1);('"#,
    "`+alert(1)+`",
    "${alert(1)}",
    "</p><script>alert(1)</script><p>",
    "\"><script>alert(1)</script>",
    "&lt;script&gt;alert(1)&lt;/script&gt;",
    "&#60;script&#62;",
    "\0<script>",
    "\u{202e}<script>",
    "'; DROP TABLE x; --",
    "{if 1}{/if}",
    "${x}",
    "{{7*7}}",
];

/// The templates data is rendered through. Every position a value can land in.
const DATA_TEMPLATES: &[&str] = &[
    "<p>${v}</p>",
    r#"<a href="${v}">x</a>"#,
    r#"<img src="${v}" alt="${v}">"#,
    r#"<div title="${v}">${v}</div>"#,
    r#"<div x-on:click="f('${v}')">x</div>"#,
    r#"<div class="${v}">x</div>"#,
    "{if v}${v}{/if}",
    "{each items as it}<li>${it}</li>{/each}",
    r#"<form action="${v}"><input value="${v}"></form>"#,
];

fn opts(ctx: Option<String>) -> CompileOptions {
    CompileOptions {
        context_json: ctx,
        ..Default::default()
    }
}

/// Invariant 3, applied to whatever the renderer produced. Deliberately checks STRUCTURE (can the
/// browser's parser be made to start a tag or a script URL) rather than substrings of payload text -
/// `&lt;script&gt;` legitimately contains "script" and is inert.
fn assert_no_executable_construct(out: &str, input: &str, template: &str) {
    let low = out.to_ascii_lowercase();

    // The template itself may legitimately contain <script>; the DATA may not introduce one. We
    // detect that by counting: rendering must not add tags the template did not already have.
    let tpl_low = template.to_ascii_lowercase();
    for tag in ["<script", "<iframe", "<object", "<embed", "<svg"] {
        let before = tpl_low.matches(tag).count();
        let after = low.matches(tag).count();
        assert!(
            after <= before,
            "data introduced {tag}: input={input:?} template={template:?} out={out:?}"
        );
    }

    // No inline event handler may appear as real markup. Escaped text ("&quot; onload=&quot;") is
    // fine; an unquoted or double-quote-delimited handler in tag position is not.
    for h in [
        "onerror=\"",
        "onload=\"",
        "onmouseover=\"",
        "onclick=\"",
        "onfocus=\"",
    ] {
        let before = tpl_low.matches(h).count();
        assert!(
            low.matches(h).count() <= before,
            "data introduced an event handler {h}: input={input:?} out={out:?}"
        );
    }

    // No live script URL in a URL-BEARING attribute. Scoped to those attribute names on purpose:
    // `alt="javascript:alert(1)"` is inert text and matching it was a false positive the first run
    // of this fuzzer produced. Only a navigable attribute can turn a scheme into execution.
    for attr in [
        "href",
        "src",
        "xlink:href",
        "poster",
        "formaction",
        "action",
        "cite",
        "background",
    ] {
        for scheme in ["javascript:", "vbscript:", "data:text/html"] {
            for q in ['"', '\''] {
                let live = format!("{attr}={q}{scheme}");
                assert!(
                    !low.contains(&live),
                    "data produced a live script URL {live:?}: input={input:?} out={out:?}"
                );
            }
            // Unquoted form, e.g. <img src=javascript:...>.
            let bare = format!("{attr}={scheme}");
            assert!(
                !low.contains(&bare),
                "data produced an unquoted script URL {bare:?}: input={input:?} out={out:?}"
            );
        }
    }
}

/// Invariant 1 + 2 over random template TEXT. The compiler is allowed to reject anything it likes;
/// it is not allowed to panic, hang, or take unbounded time.
#[test]
fn random_templates_never_panic_or_hang() {
    let mut rng = Rng::new(0x5109_2026_0726);
    let budget = Duration::from_secs(30);
    let started = Instant::now();
    let mut compiled = 0usize;

    for i in 0..4000 {
        if started.elapsed() > budget {
            eprintln!("time budget reached after {i} cases");
            break;
        }
        let n = 1 + rng.below(14);
        let mut t = String::new();
        for _ in 0..n {
            t.push_str(rng.pick(FRAGMENTS).as_ref());
        }

        let case_start = Instant::now();
        // A panic here fails the test with the input printed, which is the whole point.
        let _ = compile_template(&t, opts(None));
        let took = case_start.elapsed();
        assert!(
            took < Duration::from_secs(5),
            "compile took {took:?}, cap is 5s - a loop/depth guard is missing: {t:?}"
        );
        compiled += 1;
    }
    assert!(
        compiled > 500,
        "fuzzer barely ran ({compiled} cases) - the budget is too tight"
    );
}

/// Invariant 3: hostile DATA through trusted templates can never yield an executable construct.
/// This is the fuzzed generalisation of xss_regression.rs.
#[test]
fn hostile_data_never_produces_an_executable_construct() {
    let mut rng = Rng::new(0xB0DE_2026_0726);

    // Exhaustive over the cross product first - it is small and every cell is a real case.
    for template in DATA_TEMPLATES {
        for value in HOSTILE_VALUES {
            let ctx = serde_json::json!({ "v": value, "items": [value] }).to_string();
            if let Ok(out) = compile_template(template, opts(Some(ctx))) {
                assert_no_executable_construct(&out, value, template);
            }
        }
    }

    // Then randomised COMBINATIONS, which is where concatenation bugs live: two payloads that are
    // each inert can form a live construct across a boundary.
    for _ in 0..3000 {
        let template = rng.pick(DATA_TEMPLATES);
        let a = rng.pick(HOSTILE_VALUES);
        let b = rng.pick(HOSTILE_VALUES);
        let joined = format!("{a}{b}");
        let value = match rng.below(3) {
            0 => a.to_string(),
            1 => joined.clone(),
            _ => format!("{b}{a}"),
        };
        let ctx = serde_json::json!({ "v": value, "items": [&value] }).to_string();
        if let Ok(out) = compile_template(template, opts(Some(ctx))) {
            assert_no_executable_construct(&out, &value, template);
        }
    }
}

/// Malformed CONTEXT must be rejected or tolerated, never panic. The context arrives as JSON from
/// the edge, so a malformed or hostile shape is reachable from a bad sample record.
#[test]
fn hostile_context_json_never_panics() {
    let mut rng = Rng::new(0xC7A1_2026_0726);
    let shapes = [
        "null",
        "[]",
        "{}",
        "0",
        "\"\"",
        "true",
        r#"{"v": null}"#,
        r#"{"v": {"deep": {"deeper": {"deepest": 1}}}}"#,
        r#"{"v": [[[[[[[[[[1]]]]]]]]]]}"#,
        r#"{"items": null}"#,
        r#"{"items": "not-an-array"}"#,
        r#"{"v": 1e309}"#,
        // Invisible + bidi-control codepoints as ESCAPES: rustc denies raw text-direction
        // codepoints in literals, which is itself a small lesson in why they belong in a corpus.
        "{\"v\": \"\u{0}\u{feff}\u{202e}\"}",
        "not json at all",
        "{\"unterminated\": ",
    ];
    for _ in 0..1500 {
        let template = rng.pick(DATA_TEMPLATES);
        let ctx = rng.pick(&shapes);
        let _ = compile_template(template, opts(Some((*ctx).to_string())));
    }
}

// ── Lexer forward progress ────────────────────────────────────────────────────
//
// `lex_raw_script` consumed only the exact strings `{else if` and `{else}`, while
// `lex_raw_script_text` stopped on the bare PREFIX `{else`. Anything else starting
// `{else` therefore hit neither: the text scanner returned Text("") without moving
// `self.pos`, and `tokenize` re-entered forever, pushing an empty token per pass
// until the allocator aborted. `{elseBranch: 1}` is ORDINARY JavaScript, so a plain
// <script> block hung the compiler.
//
// WHY A THREAD AND NOT A TIME BUDGET: `random_templates_never_panic_or_hang` above
// measures elapsed time AFTER compile_template returns, so it cannot observe a call
// that never returns - it hangs with it. A hang has to be detected from OUTSIDE the
// call. On timeout the process exits rather than merely failing the test, because
// the runaway thread allocates a token per iteration and would keep doing so for the
// rest of the run.
fn lex_bounded(src: impl Into<String>) -> Result<usize, String> {
    let src = src.into();
    let for_thread = src.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = slurp_compiler::lexer::Lexer::new(&for_thread)
            .tokenize()
            .map(|t| t.len())
            .map_err(|e| e.message);
        let _ = tx.send(r);
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(r) => r,
        Err(_) => {
            eprintln!(
                "FAILED: lexer_makes_forward_progress: tokenize({src:?}) did not return within 5s.\n\
                 The lexer is in an infinite loop and is allocating a token per iteration, so the \
                 test process is being terminated instead of left to consume memory."
            );
            std::process::exit(101);
        }
    }
}

#[test]
fn lexer_makes_forward_progress_on_braces_in_script() {
    // Each of these hung before the fix. Terminating is the invariant; Ok or Err is
    // the lexer's business.
    let cases = [
        "<script>const cfg = {elseBranch: 1, a: 2};</script>",
        "<script>{elsex</script>",
        "<script>{else foo}</script>",
        "<script>{else",
        "<script>{elsewhere = 1}</script>",
        "<script>{empty_thing}</script>",
        "<script>{nextish}</script>",
        "<script>{iffy}</script>",
    ];
    for src in cases {
        let _ = lex_bounded(src);
    }
}

#[test]
fn ordinary_javascript_object_literals_still_lex() {
    // The fix must not turn a plain object literal into an error.
    let n = lex_bounded("<script>const cfg = {elseBranch: 1, a: 2};</script>")
        .expect("plain JavaScript must still lex");
    assert!(n > 5, "expected a real token stream, got {n} tokens");
}

#[test]
fn real_else_block_in_script_still_lexes() {
    // The two forms `lex_raw_script` really does consume must keep working.
    for src in [
        "<script>{if x}a{else}b{/if}</script>",
        "<script>{if x}a{else if y}b{/if}</script>",
    ] {
        lex_bounded(src).unwrap_or_else(|e| panic!("{src:?} must lex: {e}"));
    }
}

#[test]
fn short_unicode_escape_reports_a_precise_error() {
    // `\u` used to consume the next 4 characters unconditionally, eating the closing
    // quote and the closing brace, then reporting "Unterminated string literal"
    // somewhere well past the real mistake.
    let err = lex_bounded(r#"${"\u00"}"#).expect_err("a 2-digit escape must be an error");
    assert!(
        err.contains("Invalid unicode escape"),
        "expected a precise escape error, got: {err}"
    );
    // A well-formed 4-digit escape must still work. The escape is assembled rather
    // than written as a literal so that this test source carries no backslash the
    // reader has to count.
    let bs = char::from(92u8);
    lex_bounded(format!("${{\"{bs}u0041\"}}")).expect("a valid 4-digit escape must lex");
    // A non-hex escape is refused rather than silently mangled into U+FFFD.
    lex_bounded(format!("${{\"{bs}uZZZZ\"}}")).expect_err("a non-hex escape must be an error");
}
