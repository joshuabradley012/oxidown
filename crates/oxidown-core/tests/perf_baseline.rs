//! Perf BASELINE suite (measurement only, no optimization): ignored-by-
//! default, bench-style tests using `std::time::Instant` under native `cargo
//! test --release` only — the core library itself never touches clocks.
//! Conventions follow `perf_smoke.rs` / `stream_perf.rs`: loose ceilings (not
//! tight regression gates), medians/p95 over enough iterations to be stable,
//! doc generation inline per file (no shared test-util module exists yet).
//!
//! Run everything (release mode is required for representative numbers):
//!   cargo test -p oxidown-core --release --test perf_baseline -- --ignored --nocapture
//!
//! Iteration counts default to fast-CI-friendly values; override via the
//! `OXIDOWN_PERF_ITERS` env var (applies as a per-test base count — see each
//! test for how it's used) for a deeper local run:
//!   OXIDOWN_PERF_ITERS=1000 cargo test -p oxidown-core --release --test perf_baseline -- --ignored --nocapture
//!
//! This file measures (research/08-perf-baseline.md is the write-up):
//!   (a) parser::parse_document full-parse time, 4 doc sizes.
//!   (b) Editor::apply_edit (1-char insert) at doc START / MIDDLE / END.
//!   (c) Editor::decorations for a ~3k-CU viewport at the middle.
//!   (d) Editor::command(IndentList) on a nested list item mid-document.
//!   (e) Overlay node count per doc size (parser::parse_document(..).nodes.len()).
//!   (f) wasm-boundary-equivalent JSON serialization of a decorations() result
//!       (replicates crates/oxidown-wasm/src/lib.rs's `decoration_json` +
//!       `to_js` string-round-trip step, in native Rust — no wasm/browser
//!       involved).
//!
//! No src/ file was modified for this suite. `parser::ParseResult.nodes` and
//! every `Editor` method used here were already `pub`. The one non-test
//! change made anywhere in the repo is a `serde_json` dev-dependency added to
//! this crate's `Cargo.toml`, needed to replicate the wasm boundary's exact
//! JSON-string serialization step (see (f)) without depending on
//! wasm-bindgen/js-sys (which don't link on a native target).

use std::time::Instant;

use oxidown_core::{
    parser, BlockStyle, Command, Decoration, EditOrigin, Editor, MarkStyle, SelectionRange,
    Splice, WidgetKind,
};

// ---- shared test scaffolding -----------------------------------------------

const SIZES: &[(&str, usize)] = &[
    ("~3KB", 3 * 1024),
    ("~30KB", 30 * 1024),
    ("~100KB", 100 * 1024),
    ("~300KB", 300 * 1024),
];

/// Base iteration count for a test, overridable via `OXIDOWN_PERF_ITERS`
/// (e.g. for a deeper local run than the fast CI-friendly default).
fn iters(default: usize) -> usize {
    std::env::var("OXIDOWN_PERF_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

#[derive(Clone, Copy)]
struct Stats {
    mean: f64,
    p50: f64,
    p95: f64,
    max: f64,
}

fn stats(mut samples_us: Vec<f64>) -> Stats {
    samples_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = samples_us.len();
    let mean = samples_us.iter().sum::<f64>() / n as f64;
    Stats {
        mean,
        p50: samples_us[n / 2],
        p95: samples_us[n * 95 / 100],
        max: samples_us[n - 1],
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "mean {:>8.1}us  p50 {:>8.1}us  p95 {:>8.1}us  max {:>8.1}us",
            self.mean, self.p50, self.p95, self.max
        )
    }
}

/// A marker embedded in the generated doc's injected list block (see
/// `generate_mixed_doc`) so tests can locate a nested list item by text
/// search rather than hardcoding an offset.
const INDENT_TARGET_MARKER: &str = "PERF_INDENT_TARGET";

/// One block of every mixed-markdown construct we care about (headings,
/// paragraphs w/ inline marks, blockquotes incl. nesting, fenced code,
/// lists incl. nested/ordered/task), cycled to reach `target_bytes` — a
/// richer relative of `perf_smoke.rs`'s `generate_doc`, matching the shape of
/// `apps/web-demo/src/sample-doc.ts`'s `SAMPLE_DOC` (headings, bold/italic in
/// both flavors, inline code, strikethrough, links/autolinks, nested
/// blockquotes, a fenced+highlighted code block, deep/mixed list nesting)
/// without depending on that file (constraint: don't touch apps/).
///
/// A single nested-list snippet containing `INDENT_TARGET_MARKER` is spliced
/// in at roughly the document's midpoint (at a clean block boundary) for the
/// `indentList` benchmark: `outer container item` / `nested sibling alpha` /
/// `PERF_INDENT_TARGET nested sibling beta` — indenting "beta" nests it under
/// "alpha" (same marker column, matching `commands.rs`'s
/// `indent_bullet_under_bullet_is_plus_two` pattern), i.e. a list item that
/// is ALREADY nested (depth 2) goes one level deeper (depth 3): "a nested
/// list item", not a top-level one.
fn generate_mixed_doc(target_bytes: usize) -> String {
    const INDENT_TARGET_BLOCK: &str = "\n- outer container item\n  \
        - nested sibling alpha\n  - PERF_INDENT_TARGET nested sibling beta\n\
        - outer container item two\n\n";

    let mut doc = String::with_capacity(target_bytes + 4096);
    doc.push_str("# Oxidown perf corpus\n\nGenerated mixed-markdown corpus for baseline profiling.\n\n");
    let mut i = 0usize;
    let mut injected = false;
    while doc.len() < target_bytes {
        if !injected && doc.len() * 2 >= target_bytes {
            doc.push_str(INDENT_TARGET_BLOCK);
            injected = true;
        }
        match i % 6 {
            0 => {
                doc.push_str(&format!("## Section {i}\n\n"));
                doc.push_str(&format!(
                    "Lorem **ipsum {i}** dolor *sit* amet, `consectetur` adipiscing elit. \
                     Some 你好 CJK and an emoji 😀 mixed in with __strong__ text and \
                     _emphasis_ plus ***bold italic*** runs, a [link](https://example.com/{i}) \
                     and an autolink <https://oxidown.dev/{i}> to exercise the parser.\n\n"
                ));
            }
            1 => {
                doc.push_str(&format!(
                    "Plain paragraph {i} with no formatting at all, just words words words \
                     to pad out prose content between the richer constructs.\n\n"
                ));
            }
            2 => {
                doc.push_str(&format!(
                    "> Quoted line one at section {i}.\n\
                     > > A nested quote with **bold** and `code`.\n\
                     > Back to depth one.\n\n"
                ));
            }
            3 => {
                doc.push_str(&format!(
                    "```rust\nfn section_{i}() -> u32 {{\n    // a comment\n    let x = {i};\n    x * 2\n}}\n```\n\n"
                ));
            }
            4 => {
                doc.push_str(&format!(
                    "- item one at {i}\n- item two with **bold**\n  - nested item alpha\n  \
                     - nested item beta\n- [ ] a task item\n- [x] a completed task\n\
                     1. ordered one\n2. ordered two\n\n"
                ));
            }
            _ => {
                doc.push_str(&format!(
                    "### Subsection {i}\n\nAnother paragraph with ~~strikethrough~~ and a mix \
                     of *style* to round out the cycle.\n\n"
                ));
            }
        }
        i += 1;
    }
    if !injected {
        doc.push_str(INDENT_TARGET_BLOCK);
    }
    doc
}

/// Snap a raw CU offset to a valid boundary (never inside a surrogate pair).
/// Identical technique to `perf_smoke.rs::nearest_boundary`: a no-op splice
/// batch validates positions without mutating or bumping the revision.
fn nearest_boundary(ed: &mut Editor, raw: usize) -> usize {
    let mut pos = raw.min(ed.doc_len_utf16());
    loop {
        let probe = ed.apply_edit(
            ed.revision(),
            &[Splice { at: pos, delete: 0, insert: String::new() }],
            EditOrigin::User,
            0.0,
        );
        match probe {
            Ok(_) => return pos,
            Err(_) if pos > 0 => pos -= 1,
            Err(_) => return 0,
        }
    }
}

/// UTF-16 CU offset of a byte index into `doc`, computed the honest way
/// (encode_utf16 over the prefix) so it's correct even with CJK/emoji
/// earlier in the document shifting byte/CU offsets apart.
fn byte_to_utf16_offset(doc: &str, byte_idx: usize) -> usize {
    doc[..byte_idx].encode_utf16().count()
}

// ---- (a) + (e): parse_document scaling + overlay node count --------------

#[test]
#[ignore = "perf baseline; run with --release --ignored --nocapture"]
fn parse_document_scaling() {
    let n = iters(80);
    println!("\n=== parse_document full-parse scaling (release-mode timings) ===");
    println!("{:<8} {:>10} {:>10} {:>34}", "size", "bytes", "nodes", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);
        // Warm-up (page-in, allocator warm) — excluded from samples.
        let warm = parser::parse_document(&doc);
        let node_count = warm.nodes.len();
        std::hint::black_box(&warm);

        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let result = parser::parse_document(&doc);
            let us = t.elapsed().as_secs_f64() * 1e6;
            std::hint::black_box(&result);
            samples.push(us);
        }
        let s = stats(samples);
        println!("{label:<8} {:>10} {:>10} {}", doc.len(), node_count, s);

        // Loose ceiling only — this is a baseline measurement, not a
        // regression gate. 300KB parsing should not take >100ms even on a
        // slow/loaded CI box; if it does, something is very wrong.
        assert!(
            s.p95 < 100_000.0,
            "{label} parse p95 {:.0}us exceeds the 100ms loose ceiling",
            s.p95
        );
    }
}

// ---- (b) apply_edit at START / MIDDLE / END -------------------------------

#[test]
#[ignore = "perf baseline; run with --release --ignored --nocapture"]
fn apply_edit_position_scaling() {
    let n = iters(150);
    println!("\n=== apply_edit(1-char insert) by position (release-mode timings) ===");
    println!("{:<8} {:<8} {:>34}", "size", "pos", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);

        // Three independent editors so inserting at one position never
        // perturbs another position's measurements.
        for pos_label in ["start", "middle", "end"] {
            let mut ed = Editor::new(1);
            let mut rev = ed.load(&doc);
            let mut samples = Vec::with_capacity(n);
            for i in 0..n {
                let len16 = ed.doc_len_utf16();
                let pos = match pos_label {
                    // A few chars in (not literally 0) so the insert lands
                    // in ordinary text, not before the doc's leading "#".
                    "start" => nearest_boundary(&mut ed, 4),
                    "middle" => nearest_boundary(&mut ed, len16 / 2),
                    _ => len16, // end-of-doc append: always a valid boundary
                };
                let t = Instant::now();
                rev = ed
                    .apply_edit(
                        rev,
                        &[Splice { at: pos, delete: 0, insert: "x".into() }],
                        EditOrigin::User,
                        i as f64 * 1000.0, // spaced out: no undo-coalescing pathologies
                    )
                    .unwrap();
                samples.push(t.elapsed().as_secs_f64() * 1e6);
            }
            let s = stats(samples);
            println!("{label:<8} {pos_label:<8} {s}");

            assert!(
                s.p95 < 200_000.0,
                "{label}/{pos_label} apply_edit p95 {:.0}us exceeds the 200ms loose ceiling",
                s.p95
            );
            // Reparse-path regression gate: a mid-document keystroke on the
            // 300KB doc must stay an order of magnitude below the ~1.3ms an
            // accidental full reparse would cost (incremental path measured
            // ~31-45us on an M4 Pro; 500us leaves >10x headroom for slower
            // CI hardware while still catching any O(doc) dispatch bug).
            if label == "~300KB" && pos_label == "middle" {
                assert!(
                    s.p95 < 500.0,
                    "300KB mid-doc apply_edit p95 {:.0}us — the incremental \
                     reparse path has regressed toward a full reparse",
                    s.p95
                );
            }
        }
    }
}

// ---- (c) decorations() alone, ~3k-CU middle viewport ----------------------

#[test]
#[ignore = "perf baseline; run with --release --ignored --nocapture"]
fn decorations_middle_viewport_scaling() {
    let n = iters(300);
    let viewport_cu = 3000usize;
    println!("\n=== decorations() only, ~3k-CU middle viewport (release-mode timings) ===");
    println!("{:<8} {:>34}", "size", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);
        let mut ed = Editor::new(1);
        let rev = ed.load(&doc);
        let len16 = ed.doc_len_utf16();
        let center = len16 / 2;
        let vp_from = nearest_boundary(&mut ed, center.saturating_sub(viewport_cu / 2));
        let vp_to = nearest_boundary(&mut ed, (vp_from + viewport_cu).min(len16));
        let cursor = nearest_boundary(&mut ed, center);

        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let decos = ed
                .decorations(
                    rev,
                    vp_from,
                    vp_to,
                    &[SelectionRange { anchor: cursor, head: cursor }],
                )
                .unwrap();
            let us = t.elapsed().as_secs_f64() * 1e6;
            std::hint::black_box(&decos);
            samples.push(us);
        }
        let s = stats(samples);
        println!("{label:<8} {s}");

        assert!(
            s.p95 < 50_000.0,
            "{label} decorations p95 {:.0}us exceeds the 50ms loose ceiling",
            s.p95
        );
    }
}

// ---- combined apply_edit + decorations (the actual boundary-contract shape,
//      generalizing perf_smoke.rs's single-100KB-size check to all 4 sizes) --

#[test]
#[ignore = "perf baseline; run with --release --ignored --nocapture"]
fn combined_apply_edit_plus_decorations_scaling() {
    let n = iters(200);
    let viewport_cu = 3000usize;
    println!("\n=== applyEdit + decorations combined, mid-doc typing (release-mode timings) ===");
    println!("(boundary v0 contract: p95 < 1ms for a ~3k-CU viewport on a 100KB doc)");
    println!("{:<8} {:>34}", "size", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);
        let mut ed = Editor::new(1);
        let mut rev = ed.load(&doc);

        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let len16 = ed.doc_len_utf16();
            let pos = nearest_boundary(&mut ed, len16 / 2);

            let t = Instant::now();
            rev = ed
                .apply_edit(
                    rev,
                    &[Splice { at: pos, delete: 0, insert: "x".into() }],
                    EditOrigin::User,
                    i as f64 * 1000.0,
                )
                .unwrap();
            let apply_us = t.elapsed().as_secs_f64() * 1e6;

            let vp_from = nearest_boundary(&mut ed, pos.saturating_sub(viewport_cu / 2));
            let vp_raw = (vp_from + viewport_cu).min(ed.doc_len_utf16());
            let vp_to = nearest_boundary(&mut ed, vp_raw);

            let t = Instant::now();
            let decos = ed
                .decorations(
                    rev,
                    vp_from,
                    vp_to,
                    &[SelectionRange { anchor: pos + 1, head: pos + 1 }],
                )
                .unwrap();
            let deco_us = t.elapsed().as_secs_f64() * 1e6;
            std::hint::black_box(&decos);
            samples.push(apply_us + deco_us);
        }
        let s = stats(samples);
        println!("{label:<8} {s}");

        // Loose ceiling — the real 1ms budget verdict is computed in the
        // written-up report (research/08-perf-baseline.md), not asserted
        // here (this suite must stay green regardless of contract status).
        assert!(
            s.p95 < 200_000.0,
            "{label} combined p95 {:.0}us exceeds the 200ms loose ceiling",
            s.p95
        );
        // Contract-shaped regression gate at the reference size: the core
        // side of the 1ms boundary budget, with ~18x headroom over the
        // measured ~56us (M4 Pro) so slow CI hardware never flakes, while a
        // reparse-path regression (~440us+ at this size) still trips it.
        if label == "~100KB" {
            assert!(
                s.p95 < 1000.0,
                "100KB combined applyEdit+decorations p95 {:.0}us exceeds \
                 the boundary contract's 1ms core budget",
                s.p95
            );
        }
    }
}

// ---- (d) command(IndentList) on a nested list item mid-document ----------

#[test]
#[ignore = "perf baseline; run with --release --ignored --nocapture"]
fn command_indent_list_nested_mid_document() {
    let n = iters(150);
    println!("\n=== command(IndentList) on a nested list item, mid-document (release-mode timings) ===");
    println!("{:<8} {:>34}", "size", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);
        let byte_idx = doc
            .find(INDENT_TARGET_MARKER)
            .expect("generate_mixed_doc always injects the indent-target block");
        // Land the cursor a few characters into the marker word — anywhere
        // in the item's own text works, per commands.rs's indent tests.
        let pos = byte_to_utf16_offset(&doc, byte_idx) + 3;

        let mut ed = Editor::new(1);
        ed.load(&doc);

        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let change = ed
                .command(Command::IndentList { from: pos, to: pos })
                .unwrap()
                .expect("nested list item always has a same-column preceding sibling to nest under");
            let us = t.elapsed().as_secs_f64() * 1e6;
            samples.push(us);

            // Restore original text/position (untimed) so every iteration
            // measures the same starting state — outdent is indent's exact
            // inverse (commands.rs::outdent_reverses_each_indent_case).
            let sel = change.selection.expect("indent always places the cursor");
            ed.command(Command::OutdentList { from: sel.anchor, to: sel.head })
                .unwrap()
                .expect("outdent must reverse the indent we just applied");
        }
        let s = stats(samples);
        println!("{label:<8} {s}");

        assert!(
            s.p95 < 200_000.0,
            "{label} command(IndentList) p95 {:.0}us exceeds the 200ms loose ceiling",
            s.p95
        );
    }
}

// ---- (g) list-command planner scaling on huge lists ------------------------

/// Regression gate for the planner-scan fix (commands.rs `quote_context`/
/// `line_marker` binary-searching the extent-sorted overlay instead of
/// linearly scanning every node per visited line, plus `plan_list_nesting`'s
/// no-op early-out before building every intersecting line's context, plus
/// the redundant-subtree-walk skip):
///
/// * select-all Tab on a flat 10k-item ordered list — a NO-OP plan (the
///   first intersecting item line is the list's first item; nothing to nest
///   under). Pre-fix this cost ~100ms (O(lines x nodes): every intersecting
///   line's ctx built eagerly, each with two full linear node scans); fixed
///   it is ~1-2us (the lazy line iterator stops at the first item line).
///   Ceiling 20ms: enormous headroom over the fixed cost so slow CI never
///   flakes, while the pre-fix cost (5x the ceiling) trips it instantly.
/// * Tab on items 2..10k of the same list (the plan APPLIES: ~10k affected
///   lines, ~10k splices, apply + reparse included — inherently O(lines)).
///   Measured ~11-17ms fixed (M4 Pro); ceiling 150ms (~9x): pre-fix the
///   planner alone exceeded it.
/// * Shift-Tab on the top of a 10k-line always-deeper subtree (every line
///   below the target stays strictly deeper, so the subtree walk visits all
///   of them — the shape that measured ~180ms pre-fix through
///   `Editor::command`). Measured ~8ms fixed; ceiling 100ms (~12x).
#[test]
#[ignore = "perf baseline; run with --release --ignored --nocapture"]
fn command_list_planner_scaling_on_huge_lists() {
    let n = iters(30);

    // -- select-all Tab on a flat 10k-item list: no-op plan ------------------
    let mut flat = String::with_capacity(10_000 * 14);
    for i in 0..10_000 {
        flat.push_str(&format!("{}. item {i}\n", i + 1));
    }
    let mut ed = Editor::new(1);
    ed.load(&flat);
    let end = ed.doc_len_utf16();

    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let change = ed
            .command(Command::IndentList { from: 0, to: end })
            .unwrap()
            .expect("a list is selected: the command applies (as a no-op)");
        let us = t.elapsed().as_secs_f64() * 1e6;
        assert!(change.splices.is_empty(), "first item of its list: no-op");
        samples.push(us);
    }
    let noop = stats(samples);

    // -- Tab on items 2..end of the same list: the plan applies --------------
    let second_item = byte_to_utf16_offset(&flat, flat.find('\n').unwrap() + 1);
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let change = ed
            .command(Command::IndentList { from: second_item, to: end })
            .unwrap()
            .expect("items 2.. nest under item 1");
        let us = t.elapsed().as_secs_f64() * 1e6;
        assert!(!change.splices.is_empty(), "the batch moves ~10k lines");
        samples.push(us);
        // Restore (untimed) so every iteration measures the same doc.
        ed.undo().expect("the indent was a real edit");
    }
    let applies = stats(samples);

    // -- Shift-Tab atop a 10k-line always-deeper subtree ---------------------
    // "- root" at column 0, then "  - top" at column 2 whose walk collects
    // every following line: columns alternate 4/6, always strictly greater
    // than 2 (a strictly-deepening 10k-COLUMN chain would need a ~50MB doc;
    // alternating exercises the same all-lines walk in ~200KB).
    let mut deep = String::with_capacity(10_000 * 16);
    deep.push_str("- root\n  - top\n");
    for i in 0..10_000 {
        if i % 2 == 0 {
            deep.push_str(&format!("    - a{i}\n"));
        } else {
            deep.push_str(&format!("      - b{i}\n"));
        }
    }
    let mut ed = Editor::new(1);
    ed.load(&deep);
    let pos = byte_to_utf16_offset(&deep, deep.find("top").unwrap());

    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let change = ed
            .command(Command::OutdentList { from: pos, to: pos })
            .unwrap()
            .expect("\"  - top\" outdents to \"- root\"'s level");
        let us = t.elapsed().as_secs_f64() * 1e6;
        assert!(!change.splices.is_empty(), "the whole subtree moves");
        samples.push(us);
        ed.undo().expect("the outdent was a real edit");
    }
    let subtree = stats(samples);

    println!("\n=== list-command planner scaling, 10k-item lists (release-mode timings) ===");
    println!("select-all Tab (no-op plan):         {noop}");
    println!("Tab items 2.. (applies, ~10k lines): {applies}");
    println!("Shift-Tab atop 10k-line subtree:     {subtree}");

    assert!(
        noop.p95 < 20_000.0,
        "select-all no-op plan p95 {:.0}us — the planner's per-line node \
         scans have regressed toward O(lines x nodes)",
        noop.p95
    );
    assert!(
        applies.p95 < 150_000.0,
        "select-all applying plan p95 {:.0}us exceeds the 150ms ceiling",
        applies.p95
    );
    assert!(
        subtree.p95 < 100_000.0,
        "subtree outdent p95 {:.0}us exceeds the 100ms ceiling",
        subtree.p95
    );
}

// ---- (h) blockquote-heavy full-parse scaling -------------------------------

/// Quote-heavy corpus: short blockquote blocks of mixed nesting separated by
/// blank lines, ~`lines` physical lines total. This maximizes both the
/// number of blockquote INTERVALS and the number of quoted lines — the shape
/// that made the parser's per-line blockquote pass quadratic
/// (O(quoted lines × intervals): `depth_at_line` rescanned every interval
/// for every quoted line).
fn generate_quote_heavy_doc(lines: usize) -> String {
    let mut doc = String::with_capacity(lines * 24);
    let mut i = 0usize;
    let mut emitted = 0usize;
    while emitted < lines {
        match i % 4 {
            0 => {
                doc.push_str(&format!("> quoted {i} words here\n> second line {i}\n\n"));
                emitted += 3;
            }
            1 => {
                doc.push_str(&format!("> > nested {i} **bold**\n> > > third {i}\n> back\n\n"));
                emitted += 4;
            }
            2 => {
                doc.push_str(&format!("> single {i} `code`\n\n"));
                emitted += 2;
            }
            _ => {
                doc.push_str(&format!("> a{i}\n> b{i}\n> c{i}\n\n"));
                emitted += 4;
            }
        }
        i += 1;
    }
    doc
}

/// Regression gate for the two-pointer blockquote per-line pass
/// (parser.rs's `parse_document`): pre-fix, `depth_at_line` scanned EVERY
/// interval for EVERY quoted line — measured ~22ms / ~74ms / ~252ms for
/// 8k/16k/32k-line quote-heavy docs in release (clearly quadratic). Fixed,
/// the pass is O(quoted lines + intervals) and the whole parse is
/// single-digit ms at 32k lines. The ceiling is generous (100ms, well over
/// an order of magnitude above the fixed cost so slow CI hardware never
/// flakes) while the pre-fix cost (~2.5x the ceiling) trips it instantly.
#[test]
#[ignore = "perf baseline; run with --release --ignored --nocapture"]
fn parse_blockquote_heavy_scaling() {
    let n = iters(30);
    println!("\n=== parse_document on blockquote-heavy docs (release-mode timings) ===");
    println!("{:<12} {:>10} {:>34}", "lines", "bytes", "timing");
    for lines in [8 * 1024usize, 16 * 1024, 32 * 1024] {
        let doc = generate_quote_heavy_doc(lines);
        let warm = parser::parse_document(&doc);
        std::hint::black_box(&warm);

        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let result = parser::parse_document(&doc);
            let us = t.elapsed().as_secs_f64() * 1e6;
            std::hint::black_box(&result);
            samples.push(us);
        }
        let s = stats(samples);
        println!("{:<12} {:>10} {}", format!("~{}k", lines / 1024), doc.len(), s);

        if lines == 32 * 1024 {
            assert!(
                s.p95 < 100_000.0,
                "32k-line quote-heavy parse p95 {:.0}us — the per-line \
                 blockquote pass has regressed toward O(lines x intervals) \
                 (pre-fix: ~252ms)",
                s.p95
            );
        }
    }
}

// ---- (f) wasm-boundary-equivalent JSON serialization, native Rust ---------

/// Replicates `crates/oxidown-wasm/src/lib.rs`'s `decoration_json` mapping
/// exactly (field names, shapes, optional-field omission) — see that file's
/// module doc for the payload strategy rationale (one JSON-string blob per
/// call, not field-by-field JsValue reflection). This is the CPU-side half
/// of the boundary's serialization cost: the actual wasm path additionally
/// round-trips the string through `js_sys::JSON::parse`, which only exists
/// on wasm32 and cannot run natively — that half is out of scope here (the
/// coordinator's browser-side numbers cover it; see
/// research/08-perf-baseline.md's placeholder section).
fn decoration_to_json(d: &Decoration) -> serde_json::Value {
    use serde_json::json;
    match d {
        Decoration::Mark { from, to, style } => json!({
            "kind": "mark",
            "from": from,
            "to": to,
            "style": style_str(*style),
        }),
        Decoration::Conceal { from, to } => json!({
            "kind": "conceal",
            "from": from,
            "to": to,
        }),
        Decoration::Line { at, level } => json!({
            "kind": "line",
            "at": at,
            "style": format!("h{level}"),
        }),
        Decoration::Block { at, style, revealed } => {
            let mut obj = json!({
                "kind": "line",
                "at": at,
                "style": block_style_str(*style),
            });
            if let Some(depth) = block_style_depth(*style) {
                obj["depth"] = json!(depth);
            }
            if *revealed {
                obj["revealed"] = json!(true);
            }
            obj
        }
        Decoration::Widget { from, to, kind } => match kind {
            WidgetKind::Task { checked } => json!({
                "kind": "widget",
                "from": from,
                "to": to,
                "widget": "task",
                "checked": checked,
            }),
            WidgetKind::Bullet => json!({
                "kind": "widget",
                "from": from,
                "to": to,
                "widget": "bullet",
            }),
            WidgetKind::Ordered { number, delim } => json!({
                "kind": "widget",
                "from": from,
                "to": to,
                "widget": "ordered",
                "number": number,
                "delim": (*delim as char).to_string(),
            }),
        },
    }
}

fn style_str(style: MarkStyle) -> &'static str {
    match style {
        MarkStyle::Strong => "strong",
        MarkStyle::Em => "em",
        MarkStyle::Code => "code",
        MarkStyle::Delim => "delim",
        MarkStyle::Strike => "strike",
        MarkStyle::Link => "link",
        MarkStyle::Url => "url",
        MarkStyle::ListMarker => "list-marker",
    }
}

fn block_style_str(style: BlockStyle) -> &'static str {
    match style {
        BlockStyle::BlockQuote(_) => "blockquote",
        BlockStyle::CodeBlock => "code-block",
        BlockStyle::CodeFence => "code-fence",
        BlockStyle::ThematicBreak => "hr",
        BlockStyle::ListItem(_) => "list-item",
    }
}

fn block_style_depth(style: BlockStyle) -> Option<u8> {
    match style {
        BlockStyle::BlockQuote(d) | BlockStyle::ListItem(d) => Some(d),
        _ => None,
    }
}

/// Replica of `oxidown-wasm`'s `decorations_json_string` direct writer (the
/// CURRENT wire path since the Stage-A serialization fix) — byte-identical
/// output to the value-tree path above, pinned in that crate's `wire_format`
/// test module.
fn decorations_json_string(decos: &[Decoration]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(decos.len() * 56 + 2);
    s.push('[');
    for (i, d) in decos.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        match d {
            Decoration::Mark { from, to, style } => {
                let _ = write!(
                    s,
                    "{{\"from\":{from},\"kind\":\"mark\",\"style\":\"{}\",\"to\":{to}}}",
                    style_str(*style)
                );
            }
            Decoration::Conceal { from, to } => {
                let _ = write!(s, "{{\"from\":{from},\"kind\":\"conceal\",\"to\":{to}}}");
            }
            Decoration::Line { at, level } => {
                let _ = write!(s, "{{\"at\":{at},\"kind\":\"line\",\"style\":\"h{level}\"}}");
            }
            Decoration::Block { at, style, revealed } => {
                let _ = write!(s, "{{\"at\":{at}");
                if let Some(depth) = block_style_depth(*style) {
                    let _ = write!(s, ",\"depth\":{depth}");
                }
                s.push_str(",\"kind\":\"line\"");
                if *revealed {
                    s.push_str(",\"revealed\":true");
                }
                let _ = write!(s, ",\"style\":\"{}\"}}", block_style_str(*style));
            }
            Decoration::Widget { from, to, kind } => match kind {
                WidgetKind::Task { checked } => {
                    let _ = write!(
                        s,
                        "{{\"checked\":{checked},\"from\":{from},\"kind\":\"widget\",\"to\":{to},\"widget\":\"task\"}}"
                    );
                }
                WidgetKind::Bullet => {
                    let _ = write!(
                        s,
                        "{{\"from\":{from},\"kind\":\"widget\",\"to\":{to},\"widget\":\"bullet\"}}"
                    );
                }
                WidgetKind::Ordered { number, delim } => {
                    let _ = write!(
                        s,
                        "{{\"delim\":\"{}\",\"from\":{from},\"kind\":\"widget\",\"number\":{number},\"to\":{to},\"widget\":\"ordered\"}}",
                        *delim as char
                    );
                }
            },
        }
    }
    s.push(']');
    s
}

#[test]
#[ignore = "perf baseline; run with --release --ignored --nocapture"]
fn wasm_decoration_json_serialization_100kb() {
    let n = iters(300);
    let viewport_cu = 3000usize;
    let doc = generate_mixed_doc(100 * 1024);
    let mut ed = Editor::new(1);
    let rev = ed.load(&doc);
    let len16 = ed.doc_len_utf16();
    let center = len16 / 2;
    let vp_from = nearest_boundary(&mut ed, center.saturating_sub(viewport_cu / 2));
    let vp_to = nearest_boundary(&mut ed, (vp_from + viewport_cu).min(len16));
    let cursor = nearest_boundary(&mut ed, center);

    let decos = ed
        .decorations(rev, vp_from, vp_to, &[SelectionRange { anchor: cursor, head: cursor }])
        .unwrap();
    println!(
        "\n=== wasm-boundary decoration JSON serialization, ~3k-CU viewport on 100KB doc ==="
    );
    println!("decorations in viewport: {}", decos.len());

    // OLD wire path (pre-Stage-A): serde_json::Value tree, then to_string.
    let mut samples = Vec::with_capacity(n);
    let mut payload_bytes = 0usize;
    for _ in 0..n {
        let t = Instant::now();
        let payload = serde_json::Value::Array(decos.iter().map(decoration_to_json).collect());
        let json_str = payload.to_string();
        let us = t.elapsed().as_secs_f64() * 1e6;
        payload_bytes = json_str.len();
        std::hint::black_box(&json_str);
        samples.push(us);
    }
    let old = stats(samples);

    // NEW wire path: direct string writer (byte-identical output).
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let json_str = decorations_json_string(&decos);
        let us = t.elapsed().as_secs_f64() * 1e6;
        std::hint::black_box(&json_str);
        samples.push(us);
    }
    let new = stats(samples);

    assert_eq!(
        decorations_json_string(&decos),
        serde_json::Value::Array(decos.iter().map(decoration_to_json).collect()).to_string(),
        "the two wire paths must stay byte-identical"
    );
    println!("payload size: {payload_bytes} bytes");
    println!("value-tree (old):    {old}");
    println!("direct writer (new): {new}");

    assert!(
        new.p95 < 50_000.0,
        "JSON serialization p95 {:.0}us exceeds the 50ms loose ceiling",
        new.p95
    );
}
