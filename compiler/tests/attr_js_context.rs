// Two compile-time properties that a rendering test cannot reach.
//
// 1. A JavaScript TEMPLATE LITERAL written in an HTML attribute is diagnosed at COMPILE time.
//    The bug class it names is not a security hole, it is a silent WRONG PAGE: the author writes
//    `` :href="`/cart/${item.id}`" `` meaning the client framework's `item`, slurp reads `${ }` as
//    its own interpolation, evaluates `item.id` against the SERVER context where it does not
//    exist, and renders `` `/cart/` ``. Nothing fails: the page builds clean, the link is dead,
//    and the defect survives review because there is no diagnostic to read. A render-time signal
//    is not enough, because `slurp validate` never renders and an unrendered branch never warns -
//    which are precisely the two places the bug lives. These tests pin that it comes out of
//    `parse`.
//
// 2. The per-render MEMORY budget is host-configurable. The caps exist because a 64 KB adversarial
//    input can reach tens of gigabytes through a chain of expanding filters; the default has to
//    stay exactly where it is, and a host has to be able to move it without a fork.

use std::collections::HashMap;
use std::sync::Arc;

use slurp_compiler::{
    BuildMode, CompileError, CompileOptions, ErrorCode, MemoryBudget, RenderOptions,
    compile_template, compile_with_render_options, parser::parse,
};

/// Every diagnostic `parse` produced for `src`, warnings included.
fn parse_diagnostics(src: &str) -> (Vec<CompileError>, Vec<CompileError>) {
    let (_, diag) = parse(src, "t.slurp").expect("lex");
    (diag.errors.clone(), diag.warnings.clone())
}

// ── 1. The JS template literal in an attribute ──────────────────────────────────────────────────

#[test]
fn a_js_template_literal_in_an_attribute_is_diagnosed_at_parse_time() {
    let (errors, warnings) = parse_diagnostics(r#"<a :href="`/cart/${item.id}`">buy</a>"#);
    assert!(
        errors.is_empty(),
        "it is a warning, not an error: {errors:?}"
    );
    let w = warnings
        .iter()
        .find(|w| w.code == ErrorCode::JsTemplateLiteralInAttribute)
        .unwrap_or_else(|| panic!("no template-literal diagnostic: {warnings:?}"));
    assert!(
        w.message.contains(":href"),
        "the diagnostic must name the attribute: {}",
        w.message
    );
    assert!(
        w.message.contains("SERVER"),
        "the diagnostic must explain WHY the value renders empty: {}",
        w.message
    );
    assert_eq!(w.line, 1, "the diagnostic must carry a usable line");
}

/// It reports the attribute wherever it sits in the file, so the author can find it. The failure
/// mode of a render-time signal is that it only ever speaks about code that ran.
#[test]
fn the_diagnostic_carries_the_line_the_author_has_to_open() {
    let src = "<div>\n  <p>one</p>\n  <span :title=\"`v ${x}`\">y</span>\n</div>";
    let (_, warnings) = parse_diagnostics(src);
    let w = warnings
        .iter()
        .find(|w| w.code == ErrorCode::JsTemplateLiteralInAttribute)
        .expect("diagnostic present");
    assert_eq!(w.line, 3, "wrong line: {w:?}");
}

/// THE POINT OF DIAGNOSING AT PARSE TIME. A branch that never renders would otherwise never warn,
/// and a theme's `{if}` branches are exactly where a dead link hides for months.
#[test]
fn a_branch_that_never_renders_is_still_diagnosed() {
    let (_, warnings) = parse_diagnostics(r#"{if false}<a :href="`/c/${id}`">x</a>{/if}"#);
    assert!(
        warnings
            .iter()
            .any(|w| w.code == ErrorCode::JsTemplateLiteralInAttribute),
        "an unrendered branch must still be diagnosed: {warnings:?}"
    );
}

/// It is a WARNING on purpose, so it must not fail a build. An ERROR would refuse third-party
/// themes that use the shape correctly - slurp CAN legitimately interpolate a SERVER value inside
/// a backtick literal, and the compiler cannot tell the author's `item` from the server's.
#[test]
fn the_diagnostic_does_not_fail_the_compile() {
    let html = compile_template(
        r#"<a :href="`/cart/${id}`">buy</a>"#,
        CompileOptions {
            context_json: Some(r#"{"id":"7"}"#.to_string()),
            ..Default::default()
        },
    )
    .expect("a warning must never fail the build");
    assert!(html.contains("/cart/7"), "{html}");
}

/// No false positives on the shapes a theme really writes. An attribute with no backtick, a
/// backtick with no slot, and a slot in a plain quoted attribute are all silent - a warning that
/// fires on ordinary markup is a warning everyone learns to ignore.
#[test]
fn ordinary_attributes_are_not_diagnosed() {
    for src in [
        r#"<a href="/cart/${id}">x</a>"#,
        r#"<div @click="f('${id}')">x</div>"#,
        r#"<div x-init="n = ${count}">x</div>"#,
        r#"<div title="a ` b ${x}">x</div>"#,
        r#"<div x-data="{ q: '`' }">x</div>"#,
    ] {
        let (_, warnings) = parse_diagnostics(src);
        assert!(
            !warnings
                .iter()
                .any(|w| w.code == ErrorCode::JsTemplateLiteralInAttribute),
            "false positive on {src}: {warnings:?}"
        );
    }
}

// ── 2. The host-configurable memory budget ──────────────────────────────────────────────────────

/// A filter chain that expands without bound is what the budget exists for: `json` roughly doubles
/// a string, because it re-quotes and re-escapes something already quoted and escaped, so N of them
/// allocate 2^N bytes. The render must stop with a TYPED diagnostic, never a panic and never an
/// allocation the host did not agree to.
fn compile_expanding_chain(budget: MemoryBudget) -> (String, slurp_compiler::ErrorAccumulator) {
    let chain = std::iter::repeat_n("json", 64)
        .collect::<Vec<_>>()
        .join(" | ");
    compile_with_render_options(
        &format!("${{ seed | {chain} }}"),
        "t.slurp",
        &serde_json::json!({ "seed": "<a>&'\"" }),
        &RenderOptions::new(BuildMode::Production).with_budget(budget),
    )
}

#[test]
fn the_budget_is_host_configurable_downward() {
    // 1 MiB per value: far under the 8 MiB default, so the chain is cut where the host said.
    let tight = MemoryBudget {
        filter_value_bytes: 1024 * 1024,
        ..MemoryBudget::DEFAULT
    };
    let (_, diag) = compile_expanding_chain(tight);
    let e = diag
        .errors
        .iter()
        .find(|e| e.message.contains("per-value limit"))
        .unwrap_or_else(|| panic!("the tightened budget did not fire: {:?}", diag.errors));
    assert_eq!(e.code, ErrorCode::IterationLimitExceeded);
    assert!(
        e.message.contains(&(1024 * 1024).to_string()),
        "the diagnostic must report the budget in force, not the default: {}",
        e.message
    );
}

#[test]
fn the_default_budget_is_unchanged_by_the_option_existing() {
    // The same input under the default budget must report the DEFAULT number. This is the test
    // that catches a refactor silently re-pointing the caps at something else.
    let (_, diag) = compile_expanding_chain(MemoryBudget::DEFAULT);
    let e = diag
        .errors
        .iter()
        .find(|e| e.message.contains("per-value limit"))
        .expect("the default budget still bounds this chain");
    assert!(
        e.message.contains(&(8 * 1024 * 1024).to_string()),
        "default per-value cap moved: {}",
        e.message
    );
    assert_eq!(MemoryBudget::default(), MemoryBudget::DEFAULT);
    assert_eq!(MemoryBudget::DEFAULT.filter_value_bytes, 8 * 1024 * 1024);
    assert_eq!(MemoryBudget::DEFAULT.filter_total_bytes, 64 * 1024 * 1024);
    assert_eq!(MemoryBudget::DEFAULT.output_bytes, 16 * 1024 * 1024);
}

/// The pre-existing entry points must be untouched by the option existing - the whole reason it is
/// additive and defaulted. `compile_with_registry` is the entry point a host render service calls.
#[test]
fn the_older_entry_points_keep_the_default_budget() {
    let chain = std::iter::repeat_n("json", 64)
        .collect::<Vec<_>>()
        .join(" | ");
    let via_old = slurp_compiler::compile_with_registry(
        &format!("${{ seed | {chain} }}"),
        "t.slurp",
        &serde_json::json!({ "seed": "<a>&'\"" }),
        BuildMode::Production,
        Arc::new(HashMap::new()),
    );
    let via_new = compile_expanding_chain(MemoryBudget::DEFAULT);
    assert_eq!(via_old.0, via_new.0, "output diverged");
    assert_eq!(
        via_old.1.errors.len(),
        via_new.1.errors.len(),
        "diagnostics diverged"
    );
}

/// Legitimate work must fit. Measured peaks are 10 to 12 MB for a full build of a real theme, and
/// 20 MB for the worst adversarial corpus seed, so an ordinary page must not come near the caps.
#[test]
fn an_ordinary_render_never_approaches_the_budget() {
    let out = compile_template(
        "{each i in items}<p>${ i | upper }</p>{/each}",
        CompileOptions {
            context_json: Some(r#"{"items":["a","b","c"]}"#.to_string()),
            ..Default::default()
        },
    )
    .expect("an ordinary render must not be refused");
    assert!(out.contains("<p>A</p>"), "{out}");
}

/// `scaled` is the shape a host actually wants ("give this box a quarter of the default"), and a
/// zero factor must not produce a zero cap that refuses the first byte of every render.
#[test]
fn a_scaled_budget_is_never_zero() {
    let quarter = MemoryBudget::scaled(0.25);
    assert_eq!(quarter.filter_value_bytes, 2 * 1024 * 1024);
    assert_eq!(quarter.output_bytes, 4 * 1024 * 1024);
    let none = MemoryBudget::scaled(0.0);
    assert!(none.filter_value_bytes >= 1);
    assert!(none.filter_total_bytes >= 1);
    assert!(none.output_bytes >= 1);
}
