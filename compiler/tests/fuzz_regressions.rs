//! Regression tests for every bug the fuzz harness (`tests/fuzz_targets.rs`) found.
//!
//! Each test here corresponds to a minimised reproducer committed under `fuzz/corpus/`, so the
//! bug is pinned twice: once as a named test that says what went wrong, and once as a corpus
//! input that `every_corpus_input_is_clean_on_every_target` replays on every target.

mod fuzz_support;

use std::sync::Arc;

use serde_json::json;
use slurp_compiler::{BuildMode, CompileOptions, compile_template};

fn opts(ctx: serde_json::Value) -> CompileOptions {
    CompileOptions {
        context_json: Some(ctx.to_string()),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------------------------
// `| sort` panicked on a mixed-type array.
// ---------------------------------------------------------------------------------------------

/// `value_cmp` returned `Ordering::Equal` for every cross-type pair, which is not a total order:
/// given `1`, `"a"`, `2`, it reports `1 == "a"` and `"a" == 2` but `1 < 2`. Rust 1.88's `sort_by`
/// (driftsort) DETECTS that and panics with "user-provided comparison function does not correctly
/// implement a total order". A panic in the render path is a 500 on a live page, and the array is
/// user-supplied data, so this is input-reachable.
#[test]
fn sorting_a_mixed_type_array_does_not_panic() {
    let items: Vec<serde_json::Value> = (0..64)
        .map(|i| match i % 3 {
            0 => json!(i),
            1 => json!(format!("s{i}")),
            _ => json!(true),
        })
        .collect();
    let out = compile_template(
        "{each x in items | sort}${x},{/each}",
        opts(json!({ "items": items })),
    );
    assert!(out.is_ok(), "sorting a mixed array must not fail: {out:?}");
}

/// The same shape reached through a sort KEY, where the key is missing on some elements so the
/// comparator falls back to comparing the elements themselves.
#[test]
fn sorting_by_a_key_that_is_absent_on_some_elements_does_not_panic() {
    let items: Vec<serde_json::Value> = (0..64)
        .map(|i| match i % 3 {
            0 => json!({ "k": i }),
            1 => json!({ "k": format!("s{i}") }),
            _ => json!({ "other": i }),
        })
        .collect();
    let out = compile_template(
        r#"{each x in items | sort("k")}${x.k},{/each}"#,
        opts(json!({ "items": items })),
    );
    assert!(
        out.is_ok(),
        "sorting by a sparse key must not fail: {out:?}"
    );
}

/// Sorting must still WORK. Without this, "return Equal for everything" would pass the two tests
/// above while silently disabling the filter.
#[test]
fn sorting_still_orders_homogeneous_values() {
    let out = compile_template(
        "{each x in items | sort}${x},{/each}",
        opts(json!({ "items": [3, 1, 2] })),
    )
    .expect("must compile");
    let pos = |n: &str| out.find(n).unwrap_or(usize::MAX);
    assert!(
        pos("1") < pos("2") && pos("2") < pos("3"),
        "sort did not order: {out}"
    );
}

/// The test that pins the total order itself, rather than the absence of a panic.
///
/// `sort_by` requires a TOTAL ORDER. A `value_cmp` that returns `Equal` for every cross-type pair
/// leaves a mixed array in essentially its input order; the observable consequence is that values
/// are NOT grouped by type. Asserting the grouping is how a behavioural test detects a broken
/// comparator without depending on whether the standard library happens to notice.
#[test]
fn sorting_a_mixed_array_produces_a_total_order_grouped_by_type() {
    // Deliberately interleaved on input, so passing cannot happen by accident.
    let items = json!(["b", 2, null, true, "a", 1, false, null, 3, "c"]);
    let out = compile_template(
        "{each x in items | sort}[${x}]{/each}",
        opts(json!({ "items": items })),
    )
    .expect("compile");

    // Rendered forms, in the order the type ranking requires:
    // null (renders empty), bools, numbers, strings.
    let order = [
        "[]", "[]", "[false]", "[true]", "[1]", "[2]", "[3]", "[a]", "[b]", "[c]",
    ];
    let joined = order.concat();
    assert_eq!(
        out, joined,
        "mixed array must sort into one total order; got {out}"
    );
}

/// Mixed types must sort DETERMINISTICALLY, so a page does not reshuffle between renders.
#[test]
fn sorting_mixed_types_is_deterministic() {
    let items = json!([3, "b", true, null, 1, "a", false, 2, {"o":1}, [1]]);
    let src = "{each x in items | sort}${x}|{/each}";
    let a = compile_template(src, opts(json!({ "items": items }))).expect("compile");
    let b = compile_template(src, opts(json!({ "items": items }))).expect("compile");
    assert_eq!(a, b, "mixed-type sort must be stable across runs");
}

/// Deeply nested arrays reach `value_cmp`'s own recursion. Bounded by `VALUE_CMP_MAX_DEPTH`;
/// without a bound this is a stack overflow driven by user-supplied JSON, which is an uncatchable
/// abort rather than a failed render.
#[test]
fn sorting_deeply_nested_arrays_does_not_overflow_the_stack() {
    fn nest(d: usize) -> serde_json::Value {
        let mut v = json!([]);
        for _ in 0..d {
            v = json!([v]);
        }
        v
    }
    let items: Vec<serde_json::Value> = (0..40).map(|i| nest(200 + i)).collect();
    let out = compile_template(
        "{each x in items | sort}.{/each}",
        opts(json!({ "items": items })),
    );
    assert!(
        out.is_ok(),
        "deep nested arrays must sort without overflowing: {out:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// `<Image>` bypassed the output-byte budget and the iteration budget.
// ---------------------------------------------------------------------------------------------

/// `render_component` returned early for `name == "Image"` BEFORE `charge_iteration`, and
/// `render_image_component` built its markup with a bare `format!` that never went through
/// `charge_output`. So the 16 MiB output ceiling and the 1M iteration ceiling both counted zero
/// for the one component that expands ~330 bytes of markup per instance, three of them copies of
/// a URL-encoded (up to 3x) user-supplied string.
///
/// The test asserts a BOUND, not an exact size: the point is that the budget applies at all.
#[test]
fn image_components_are_charged_against_the_output_budget() {
    let src = "{repeat 20000}<Image src={u} width={1} height={1} />{/repeat}";
    let big = "x&?".repeat(20_000); // ~60 KB that url-encodes to ~180 KB per copy
    let out = compile_template(src, opts(json!({ "u": big }))).expect("must not fail");
    assert!(
        out.len() < 64 * 1024 * 1024,
        "output was {} bytes; the 16 MiB budget must apply to <Image> too",
        out.len()
    );
}

/// An `<Image>` still renders. A "fix" that dropped the component would pass the bound above.
#[test]
fn a_single_image_component_still_renders() {
    let out = compile_template(
        r#"<Image src="/a.png" width={10} height={10} />"#,
        opts(json!({})),
    )
    .expect("must compile");
    assert!(
        out.contains("a.png"),
        "the Image component must still render: {out}"
    );
}

// ---------------------------------------------------------------------------------------------
// The `{blocks}` wrapper was emitted without being charged.
// ---------------------------------------------------------------------------------------------

/// `render_block_children` wrapped each child in `<div data-bs-block="{id}" ...>` with a bare
/// `push_str`, so neither the ~45-byte wrapper nor the escaped id counted against the output
/// budget. The id comes from untrusted saved state and `escape_html` expands it up to 6x.
#[test]
fn block_wrappers_are_charged_against_the_output_budget() {
    let section = r#"---
section { name: "S" blocks { item { max: 1000000 settings { v: text = "v" } } } }
---
{blocks}"#;
    // The worst-case id THAT SURVIVES THE MERGE, which is now much smaller than it was.
    //
    // This used to be 250 chars of `&`, chosen because `escape_html` expands each to `&amp;` and
    // gave a ~1312-byte wrapper per block. That id no longer reaches the renderer: block ids go
    // through `safe_ident` at the merge boundary, and every character `escape_html` expands
    // (`& < > " '`) is on its reject list, so a surviving id cannot expand AT ALL. 256 plain
    // characters - the length cap - is the true worst case, giving ~318 bytes per block.
    //
    // The block COUNT is raised to compensate. The bound below has to sit between "wrapper
    // uncharged" and "wrapper charged", so the uncharged total must still clear it: 120k blocks
    // x ~318 bytes is ~38 MB uncharged, and latches at the 16 MiB ceiling if charged. Leaving
    // the count at 40k would have made both outcomes ~12 MB and the assertion vacuous.
    let id = "a".repeat(256);
    let state = json!({
        "type": "s",
        "blocks": (0..120_000)
            .map(|_| json!({ "type": "item", "id": id }))
            .collect::<Vec<_>>()
    });
    // BOTH registry key forms. The renderer resolves a block through `@blocks/{type}`, so
    // registering only the `blocks/item.slurp` path form renders nothing at all, which would
    // leave the bound below asserting on a single byte of output.
    let mut reg = std::collections::HashMap::new();
    let block_src =
        "---\nblock { name: \"I\" settings { v: text = \"v\" } }\n---\n<i>${block.settings.v}</i>"
            .to_string();
    reg.insert("blocks/item.slurp".to_string(), block_src.clone());
    reg.insert("@blocks/item".to_string(), block_src);
    let (doc, _) = slurp_compiler::parse(section, "sections/s.slurp").expect("section must parse");
    let (html, _) = slurp_compiler::render_section_with_registry(
        &doc,
        &state,
        &json!({}),
        BuildMode::Production,
        Arc::new(reg),
    );
    // See the note on the fixture above: ~38 MB if the wrapper is uncharged, latching at the
    // 16 MiB ceiling if it is charged.
    assert!(
        html.len() < 32 * 1024 * 1024,
        "section html was {} bytes; the wrapper must be charged too",
        html.len()
    );
    // ... and the blocks must actually have rendered, or the bound above proves nothing.
    assert!(
        html.len() > 1024 * 1024,
        "expected real output, got {} bytes",
        html.len()
    );
}

/// Rendering N blocks was O(N^2), so a section with many blocks was a CPU-exhaustion vector.
///
/// Two separate O(N)-per-child copies stacked up: `scoped_context` cloned the whole globals map,
/// and `Scope::root` then deep-cloned every top-level value out of it - including `section`,
/// which contains the very blocks array being iterated. Measured in a debug build before the fix:
/// 500 blocks 1.2 s, 1000 blocks 5.0 s, 2000 blocks 22 s, 4000 blocks 96 s. After hoisting both
/// and using a child scope, 4000 blocks is 271 ms and the curve is linear.
///
/// `BLOCK_HARD_CEILING` caps a stock theme at 200 blocks per type per level, which is what hid
/// this; a section schema may declare `max: 100000`, and a theme may be user-supplied.
///
/// Asserted as wall time, and as a RATIO rather than an absolute, so the test measures complexity
/// rather than how fast the CI box is. Quadratic growth doubles the per-block cost each time N
/// doubles; linear growth keeps it flat.
#[test]
fn rendering_many_blocks_is_linear_not_quadratic() {
    fn render_n(n: usize) -> std::time::Duration {
        let section = r#"---
section { name: "S" blocks { item { max: 100000 settings { v: text = "v" } } } }
---
{blocks}"#;
        // A LEGITIMATE long id (see the fixture note on the previous test): block ids now pass
        // through `safe_ident`, which rejects every character `escape_html` would expand, so
        // the old 250-char `&` id is dropped entirely and each block renders a bare 62-byte
        // wrapper. Plain characters keep the per-block output large enough for the timing here
        // to measure rendering rather than measuring the clock.
        let id = "a".repeat(250);
        let state = json!({
            "type": "s",
            "blocks": (0..n).map(|_| json!({ "type": "item", "id": id })).collect::<Vec<_>>()
        });
        let body =
            "---\nblock { name: \"I\" settings { v: text = \"v\" } }\n---\n<i>${block.settings.v}</i>"
                .to_string();
        let mut reg = std::collections::HashMap::new();
        reg.insert("blocks/item.slurp".to_string(), body.clone());
        reg.insert("@blocks/item".to_string(), body);
        let (doc, _) = slurp_compiler::parse(section, "sections/s.slurp").expect("parse");
        let started = std::time::Instant::now();
        let (html, _) = slurp_compiler::render_section_with_registry(
            &doc,
            &state,
            &json!({}),
            BuildMode::Production,
            Arc::new(reg),
        );
        let elapsed = started.elapsed();
        assert_eq!(
            html.len(),
            n * 312 + 1,
            "output shape changed; retune this test"
        );
        elapsed
    }

    let small = render_n(1_000).as_secs_f64().max(0.001);
    let large = render_n(4_000).as_secs_f64();
    // 4x the work. Linear predicts ~4x the time; quadratic predicts ~16x. The threshold sits
    // well between the two so ordinary CI noise cannot flip it.
    let ratio = large / small;
    assert!(
        ratio < 9.0,
        "4x the blocks took {ratio:.1}x the time ({small:.3}s -> {large:.3}s); \
         block rendering has gone quadratic again"
    );
}

// ---------------------------------------------------------------------------------------------
// The `{html` lookahead amplified CPU ~800x per five bytes consumed.
// ---------------------------------------------------------------------------------------------

/// `html_open_has_sanitize` scanned up to 4096 chars looking for `sanitize` before the next `}`,
/// but `lex_template` consumes only the five bytes `{html`. A file of repeated `{html` therefore
/// cost ~800 char-steps per input byte: `"{html}".repeat(200_000)` is 1.2 MB and took 723 ms in
/// a RELEASE build. The scan is now a single pass that stops at the first `}`, which brings the
/// same input to 20.6 ms.
///
/// The CLOSED `{html}` form is the shape that matters. The unclosed `{html` form pushes a lexer
/// mode that never pops, so it is now stopped by `MAX_MODE_DEPTH` long before the scan cost can
/// accumulate; testing that shape instead would pass for the wrong reason.
///
/// Asserted as wall time because the defect IS wall time. The bound is loose enough for a debug
/// build on a loaded CI box and still far under the pre-fix cost.
#[test]
fn repeated_html_openers_do_not_amplify_lexer_work() {
    let src = "{html}".repeat(200_000);
    let started = std::time::Instant::now();
    let r = slurp_compiler::lexer::Lexer::new(&src).tokenize();
    let elapsed = started.elapsed();
    assert!(
        r.is_ok(),
        "the input must actually lex, or this measures the wrong thing"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(8),
        "lexing 200k `{{html}}` openers took {elapsed:?}"
    );
}

/// The lookahead must still WORK: `{html x sanitize}` has to lex as the sanitizing form, and
/// `{html x}` as the raw form. A fix that simply stopped scanning would pass the timing test
/// while silently turning every sanitizing interpolation into a raw one, which is an XSS.
#[test]
fn the_sanitize_lookahead_still_distinguishes_the_two_html_forms() {
    let sanitized = compile_template(
        "{html v sanitize}",
        opts(json!({ "v": "<img src=x onerror=alert(1)>" })),
    )
    .expect("compile");
    assert!(
        !sanitized.contains("onerror"),
        "sanitize form must strip: {sanitized}"
    );

    let raw = compile_template("{html v}", opts(json!({ "v": "<b>ok</b>" }))).expect("compile");
    assert!(
        raw.contains("<b>ok</b>"),
        "raw form must pass markup through: {raw}"
    );
}

/// The keyword may legitimately sit far from `{html` on the same opener, and a multi-line opener
/// must not be mistaken for one. Pins the exact boundary the fix moved.
#[test]
fn the_sanitize_lookahead_spans_a_long_single_line_opener() {
    let padding = " ".repeat(300);
    let src = format!("{{html v{padding}sanitize}}");
    let out = compile_template(&src, opts(json!({ "v": "<img src=x onerror=alert(1)>" })))
        .expect("compile");
    assert!(
        !out.contains("onerror"),
        "a long opener must still sanitize: {out}"
    );
}

// ---------------------------------------------------------------------------------------------
// The lexer emitted one heap token per byte, with no ceiling.
// ---------------------------------------------------------------------------------------------

/// A bare `<` produced one `Token::Text("<")` per byte, each an independent heap allocation of
/// roughly 70 bytes with the span. Nothing capped `tokens.len()`, so a large template expanded
/// ~70x into resident memory and OOM-killed the whole shared service rather than failing one
/// compile. The lexer now refuses past a token ceiling.
#[test]
fn a_pathological_template_is_refused_rather_than_exhausting_memory() {
    let src = "<".repeat(4_000_000);
    let r = slurp_compiler::lexer::Lexer::new(&src).tokenize();
    assert!(
        r.is_err(),
        "a 4M-token template must be refused, not tokenized"
    );
}

/// Ordinary templates must stay far under the ceiling. Without this, a ceiling set too low would
/// pass the test above and break every real theme.
#[test]
fn a_realistic_template_is_nowhere_near_the_token_ceiling() {
    let src = "<div class=\"a\"><p>${ x | upper }</p></div>\n".repeat(20_000);
    let r = slurp_compiler::lexer::Lexer::new(&src).tokenize();
    assert!(
        r.is_ok(),
        "a 20k-line realistic template must still lex: {:?}",
        r.err()
    );
}

// ---------------------------------------------------------------------------------------------
// Unbounded recursion on public entry points that take a caller-supplied Document.
// ---------------------------------------------------------------------------------------------

/// `eval_fetch_urls` and `security_check`'s `check_expr` walked an `ast::Document` with no depth
/// bound of their own, relying entirely on the PARSER's `MAX_DEPTH` having produced the document.
/// `ast::Document` is public and `eval_fetch_urls` is a public API the dev server calls, so a
/// hand-built document reached them unbounded, and a stack overflow is an uncatchable abort.
///
/// Deep but parser-legal input, which is what a template can actually reach.
#[test]
fn deeply_nested_documents_do_not_overflow_the_stack() {
    let depth = 60;
    let src = format!(
        "{}{}{}",
        "<div>".repeat(depth),
        "{fetch \"/a\" as v}${v}{/fetch}",
        "</div>".repeat(depth)
    );
    let (doc, _) = slurp_compiler::parse(&src, "f.slurp").expect("parse");
    let urls = slurp_compiler::eval_fetch_urls(&doc, &json!({}));
    assert!(urls.len() <= 1);
}

// ---------------------------------------------------------------------------------------------
// The lexer mode stack had no ceiling.
// ---------------------------------------------------------------------------------------------

/// Nesting like ``${` `` grows the lexer's mode stack two entries per three bytes. The lexer had
/// no cap, so it happily produced a token stream nested tens of thousands deep and handed it to a
/// recursive-descent parser. The parser's own `MAX_DEPTH` catches that, but only after building
/// the whole stream; refusing at the lexer is both cheaper and the layer that knows.
#[test]
fn absurd_lexer_nesting_is_refused_rather_than_handed_to_the_parser() {
    let src = "${`".repeat(200_000);
    let r = slurp_compiler::lexer::Lexer::new(&src).tokenize();
    assert!(r.is_err(), "200k levels of `${{` nesting must be refused");
}

/// Nesting a theme could plausibly write must still lex.
#[test]
fn realistic_nesting_still_lexes() {
    let src = format!("{}x{}", "${`".repeat(20), "`}".repeat(20));
    let r = slurp_compiler::lexer::Lexer::new(&src).tokenize();
    assert!(r.is_ok(), "20 levels of nesting must lex: {:?}", r.err());
}

// ---------------------------------------------------------------------------------------------
// An unbounded filter chain allocated without limit.
// ---------------------------------------------------------------------------------------------

/// The most severe of the amplification bugs, and the one the fuzz harness reached first.
///
/// Several filters EXPAND their input: `json` roughly doubles a string, because it re-quotes and
/// re-escapes something that is already quoted and escaped. Nothing bounded the INTERMEDIATE
/// values in a filter chain, so N chained expanding filters allocated 2^N bytes. Measured before
/// the fix, from a 6-byte value: 229 KB at 16 filters, 3.6 MB at 20, and a generated input with
/// about 50 of them allocated 22 GB before an external kill.
///
/// Neither existing budget could see it: `MAX_OUTPUT_BYTES` charges only what is EMITTED, and a
/// filter chain emits nothing until it finishes, while `MAX_TOTAL_ITERATIONS` counts loop passes
/// and there is no loop here. `MAX_FILTER_VALUE_BYTES` is the budget that was missing.
///
/// Why it matters beyond one failed compile: templates are rendered in a shared multi-tenant
/// process, so one template exhausting memory takes down every other tenant on that host.
#[test]
fn an_unbounded_filter_chain_is_refused_rather_than_allocating_without_limit() {
    let chain = std::iter::repeat_n("json", 64)
        .collect::<Vec<_>>()
        .join(" | ");
    let src = format!("${{ x | {chain} }}");
    let started = std::time::Instant::now();
    let out = compile_template(&src, opts(json!({ "x": "<a>&'\"" })));
    let elapsed = started.elapsed();

    assert!(
        out.is_err(),
        "a 64-deep expanding filter chain must be refused, got Ok"
    );
    // A clean typed error, fast. Before the fix this input grew without bound.
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "refusing the chain took {elapsed:?}; the budget must stop it early, not after it grows"
    );
}

/// The exact minimised reproducer the fuzzer saved, replayed from the committed corpus so the
/// test and the corpus entry can never drift apart.
#[test]
fn the_minimised_filter_chain_reproducer_is_refused() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz/corpus/common/repro-filter-chain-oom.slurp");
    let data = std::fs::read(&path).expect("the reproducer must stay committed");
    let src = String::from_utf8_lossy(&data).into_owned();
    let started = std::time::Instant::now();
    let out = compile_template(&src, opts(json!({ "x": "<a>&'\"" })));
    assert!(out.is_err(), "the saved reproducer must be refused");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the reproducer must be refused quickly"
    );
}

/// Chains a theme would actually write must still work, or the budget is a bug of its own.
#[test]
fn ordinary_filter_chains_still_work() {
    let out = compile_template(
        r#"${ name | upper | truncate(9) }"#,
        opts(json!({ "name": "a product name" })),
    )
    .expect("an ordinary chain must compile");
    assert!(out.contains("A PRODUCT"), "filters must still apply: {out}");

    // `json` on a realistic catalogue is a normal thing to do and is nowhere near the cap.
    let items: Vec<serde_json::Value> = (0..500)
        .map(|i| json!({ "id": i, "title": format!("Product {i}") }))
        .collect();
    let out = compile_template(
        r#"<script type="application/json">${ items | json }</script>"#,
        opts(json!({ "items": items })),
    )
    .expect("a large but legitimate json must compile");
    assert!(out.contains("Product 499"), "the whole payload must render");
}

// ---------------------------------------------------------------------------------------------
// The attribute-spread path was UNREACHABLE by the fuzzer, and a panic lived there.
// ---------------------------------------------------------------------------------------------

/// `is_safe_attr_name` tested for the `on` prefix with `&bind_target[..2]`, a BYTE slice, so a
/// name beginning with a multi-byte character split a UTF-8 sequence and panicked.
///
/// The interesting part is not the slice, it is why 420,000 fuzz iterations never found it:
/// `{html expr}` in TAG position is the only way to reach `sanitize_attr_spread`, no corpus seed
/// contained that construct, and no alphabet entry could build one - mutating a text-position
/// `{html ...}` never yields a spread. The function was never executed once. Coverage is now
/// carried by `fuzz/corpus/common/repro-attr-spread-multibyte-name.slurp` plus the
/// `sec-attr-spread-*` seeds and the spread shapes in `fuzz_support::alphabet`.
#[test]
fn an_attribute_spread_name_starting_with_a_multibyte_character_does_not_panic() {
    let out = compile_template(
        "<div {html attrs}>hi</div>",
        opts(json!({ "attrs": "\u{20ac}=1" })),
    )
    .expect("must compile");
    assert_eq!(out, "<div>hi</div>", "got: {out}");
}

/// A COVERAGE GUARD, not a behaviour test.
///
/// The gap that hid the panic was structural: the fuzzer's inputs could not contain the
/// construct. Pinning the FIX alone would leave that intact, so this asserts the construct is
/// present in both places the fuzzer draws from - the alphabet and the seed corpus - and that a
/// seed carrying it really does drive the sanitizer rather than merely parsing.
#[test]
fn the_fuzz_inputs_can_reach_the_attribute_spread_path() {
    let alpha = fuzz_support::alphabet();
    let spread_shapes: Vec<&String> = alpha
        .iter()
        .filter(|t| t.contains("{html") && t.contains('<'))
        .collect();
    assert!(
        !spread_shapes.is_empty(),
        "the fuzz alphabet contains no `{{html ...}}` in TAG position, so no generated input can \
         reach sanitize_attr_spread - the exact gap that hid a CRITICAL panic"
    );

    let corpus = fuzz_support::load_corpus(fuzz_support::Target::CompileTemplate);
    let seeds_with_spread = corpus
        .iter()
        .filter(|b| {
            let s = String::from_utf8_lossy(b);
            s.contains("{html attrs}") || s.contains("{html more}")
        })
        .count();
    assert!(
        seeds_with_spread > 0,
        "no corpus seed contains an attribute spread; mutation cannot reach the construct from \
         nothing"
    );

    // ...and the construct really does reach the sanitizer, proven by the sanitizer's OBSERVABLE
    // work: the handler is dropped while the data attribute beside it survives. A seed that
    // merely parses would leave both, or neither.
    let out = compile_template(
        "<div {html attrs}>x</div>",
        opts(json!({ "attrs": r#"data-ok="1" onclick="alert(1)""# })),
    )
    .expect("must compile");
    assert!(out.contains(r#"data-ok="1""#), "got: {out}");
    assert!(!out.contains("onclick"), "got: {out}");
}
