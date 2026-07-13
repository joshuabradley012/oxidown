//! M2 de-risk suite (MEASUREMENT ONLY — no optimization changes): ignored-by-
//! default, bench-style tests using `std::time::Instant` under native
//! `cargo test --release` only, same conventions as `perf_baseline.rs`
//! (loose ceilings, not tight regression gates; iteration counts overridable
//! via `OXIDOWN_PERF_ITERS`; doc generation inline, no shared test-util
//! module).
//!
//! Written for `research/09-1mb-derisk.md` — quantifying where M1's
//! remaining O(doc) residues (documented in research/08-perf-baseline.md's
//! "After" section: suffix span-rebase + block-ID rematch, the overlay
//! `Vec` splice memmove, `decorations()`'s overlay scan, `reparse_tail`'s
//! full-overlay `retain`) actually land at M2 scale (1MB/3MB documents),
//! ahead of building the virtual-viewport UI (plan.md §8).
//!
//! Run everything (release mode required for representative numbers):
//!   cargo test -p oxidown-core --release --test perf_1mb_derisk -- --ignored --nocapture
//!
//! Deeper local run:
//!   OXIDOWN_PERF_ITERS=500 cargo test -p oxidown-core --release --test perf_1mb_derisk -- --ignored --nocapture
//!
//! NOTE on enforcement: like `perf_baseline.rs`, every ceiling below is a
//! LOOSE, generous trip-wire (order-of-magnitude headroom over measured
//! cost) — these are not tight regression gates, and (being `#[ignore]`d)
//! they do not run in the normal `cargo test --workspace` pass at all.
//!
//! This file measures (research/09-1mb-derisk.md is the write-up):
//!   (a) parser::parse_document full-parse time + overlay node count/memory
//!       estimate, at 300KB/1MB/3MB.
//!   (b) Editor::apply_edit (1-char insert) at START/MIDDLE/END, same sizes
//!       — the END-vs-MIDDLE delta isolates the doc-size-dependent
//!       bookkeeping term (suffix rebase/rematch, or the tail path's
//!       full-overlay retain scan) from window-parse cost.
//!   (c) Editor::decorations for a ~3k-CU viewport at the middle.
//!   (d) Editor::command(IndentList) on a nested list item mid-document.
//!   (e) undo/redo of a mid-document edit.
//!   (f) BlockIndex::update in isolation (no parsing), scaled by block
//!       count, to find its own per-block constant.
//!   (g) apply_edit's anchor-mapping cost with 0 vs. 1,000 live anchors.
//!   (h) wasm-boundary-equivalent JSON serialization of a decorations()
//!       result at 1MB (native Rust replica of oxidown-wasm's direct
//!       writer — no wasm/browser involved; the JS-side boundary-crossing
//!       tax is measured separately, see packages/oxidown-view-cm6/test/
//!       perf-1mb.bench.ts).
//!
//! No `src/` file was modified for this suite's PERMANENT contents. A
//! temporary, since-reverted instrumentation pass (env-gated prints inside
//! `reparse_incremental`) was used once, locally, to read out real
//! convergence-window sizes for item (3b) of the research writeup; it left
//! no trace in `src/` (see research/09-1mb-derisk.md's method section) and
//! is not part of this file or this test run.

use std::time::Instant;

use oxidown_core::block_index::BlockIndex;
use oxidown_core::text::ByteSplice;
use oxidown_core::{
    parser, Bias, BlockStyle, Command, Decoration, EditOrigin, Editor, MarkStyle, SelectionRange,
    Splice, WidgetKind,
};

// ---- shared test scaffolding (same shape as perf_baseline.rs, extended) --

const SIZES: &[(&str, usize)] = &[
    ("~300KB", 300 * 1024),
    ("~1MB", 1024 * 1024),
    ("~3MB", 3 * 1024 * 1024),
];

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
            "mean {:>9.1}us  p50 {:>9.1}us  p95 {:>9.1}us  max {:>9.1}us",
            self.mean, self.p50, self.p95, self.max
        )
    }
}

const INDENT_TARGET_MARKER: &str = "PERF_INDENT_TARGET";

/// Identical generator to `perf_baseline.rs`'s `generate_mixed_doc` (kept as
/// its own copy — "doc generation inline per file", no shared test-util
/// module exists yet), so the 300KB row here is a same-shape cross-check
/// against research/08's numbers, and the 1MB/3MB rows extend the same
/// mixed-markdown corpus shape up two more size classes.
fn generate_mixed_doc(target_bytes: usize) -> String {
    const INDENT_TARGET_BLOCK: &str = "\n- outer container item\n  \
        - nested sibling alpha\n  - PERF_INDENT_TARGET nested sibling beta\n\
        - outer container item two\n\n";

    let mut doc = String::with_capacity(target_bytes + 4096);
    doc.push_str("# Oxidown perf corpus\n\nGenerated mixed-markdown corpus for M2 de-risk profiling.\n\n");
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

fn byte_to_utf16_offset(doc: &str, byte_idx: usize) -> usize {
    doc[..byte_idx].encode_utf16().count()
}

// ---- (a) parse_document scaling + overlay node count/memory estimate -----

#[test]
#[ignore = "M2 de-risk; run with --release --ignored --nocapture"]
fn parse_document_scaling_1mb_3mb() {
    let n = iters(30);
    println!("\n=== parse_document full-parse scaling, 300KB/1MB/3MB (release-mode) ===");
    println!(
        "node size_of: {} bytes; (BlockKind, Range<usize>) size_of: {} bytes",
        std::mem::size_of::<parser::Node>(),
        std::mem::size_of::<(parser::BlockKind, std::ops::Range<usize>)>()
    );
    println!(
        "{:<8} {:>10} {:>10} {:>10} {:>14} {:>34}",
        "size", "bytes", "nodes", "blocks", "node-mem-est", "timing"
    );
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);
        let warm = parser::parse_document(&doc);
        let node_count = warm.nodes.len();
        let block_count = warm.blocks.len();
        let mem_est = node_count * std::mem::size_of::<parser::Node>();
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
        println!(
            "{label:<8} {:>10} {:>10} {:>10} {:>11}KB {}",
            doc.len(),
            node_count,
            block_count,
            mem_est / 1024,
            s
        );

        assert!(
            s.p95 < 500_000.0,
            "{label} parse p95 {:.0}us exceeds the 500ms loose ceiling",
            s.p95
        );
    }
}

// ---- (b) apply_edit at START / MIDDLE / END, 300KB/1MB/3MB ---------------

#[test]
#[ignore = "M2 de-risk; run with --release --ignored --nocapture"]
fn apply_edit_position_scaling_1mb_3mb() {
    let n = iters(100);
    println!("\n=== apply_edit(1-char insert) by position, 300KB/1MB/3MB (release-mode) ===");
    println!("{:<8} {:<8} {:>34}", "size", "pos", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);

        let mut by_pos: Vec<(&str, Stats)> = Vec::new();
        for pos_label in ["start", "middle", "end"] {
            let mut ed = Editor::new(1);
            let mut rev = ed.load(&doc);
            let mut samples = Vec::with_capacity(n);
            for i in 0..n {
                let len16 = ed.doc_len_utf16();
                let pos = match pos_label {
                    "start" => nearest_boundary(&mut ed, 4),
                    "middle" => nearest_boundary(&mut ed, len16 / 2),
                    _ => len16,
                };
                let t = Instant::now();
                rev = ed
                    .apply_edit(
                        rev,
                        &[Splice { at: pos, delete: 0, insert: "x".into() }],
                        EditOrigin::User,
                        i as f64 * 1000.0,
                    )
                    .unwrap();
                samples.push(t.elapsed().as_secs_f64() * 1e6);
            }
            let s = stats(samples);
            println!("{label:<8} {pos_label:<8} {s}");
            by_pos.push((pos_label, s));

            assert!(
                s.p95 < 500_000.0,
                "{label}/{pos_label} apply_edit p95 {:.0}us exceeds the 500ms loose ceiling",
                s.p95
            );
        }
        let middle = by_pos[1].1;
        let end = by_pos[2].1;
        println!(
            "{label:<8} [middle-mean minus end-mean = doc-size-dependent bookkeeping term]: {:.1}us",
            middle.mean - end.mean
        );
    }
}

// ---- (c) decorations() alone, ~3k-CU middle viewport ----------------------

#[test]
#[ignore = "M2 de-risk; run with --release --ignored --nocapture"]
fn decorations_middle_viewport_scaling_1mb_3mb() {
    let n = iters(200);
    let viewport_cu = 3000usize;
    println!("\n=== decorations() only, ~3k-CU middle viewport, 300KB/1MB/3MB (release-mode) ===");
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
            s.p95 < 100_000.0,
            "{label} decorations p95 {:.0}us exceeds the 100ms loose ceiling",
            s.p95
        );
    }
}

// ---- (d) command(IndentList) on a nested list item, mid-document ---------

#[test]
#[ignore = "M2 de-risk; run with --release --ignored --nocapture"]
fn command_indent_list_nested_mid_document_1mb_3mb() {
    let n = iters(80);
    println!("\n=== command(IndentList) mid-document, 300KB/1MB/3MB (release-mode) ===");
    println!("{:<8} {:>34}", "size", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);
        let byte_idx = doc
            .find(INDENT_TARGET_MARKER)
            .expect("generate_mixed_doc always injects the indent-target block");
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

            let sel = change.selection.expect("indent always places the cursor");
            ed.command(Command::OutdentList { from: sel.anchor, to: sel.head })
                .unwrap()
                .expect("outdent must reverse the indent we just applied");
        }
        let s = stats(samples);
        println!("{label:<8} {s}");

        assert!(
            s.p95 < 500_000.0,
            "{label} command(IndentList) p95 {:.0}us exceeds the 500ms loose ceiling",
            s.p95
        );
    }
}

// ---- (e) undo/redo of a mid-document edit ---------------------------------

#[test]
#[ignore = "M2 de-risk; run with --release --ignored --nocapture"]
fn undo_redo_mid_doc_edit_1mb_3mb() {
    let n = iters(80);
    println!("\n=== undo/redo of a mid-document 1-char edit, 300KB/1MB/3MB (release-mode) ===");
    println!("{:<8} {:<8} {:>34}", "size", "op", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);
        let mut ed = Editor::new(1);
        let mut rev = ed.load(&doc);
        let len16 = ed.doc_len_utf16();
        let pos = nearest_boundary(&mut ed, len16 / 2);

        let mut undo_samples = Vec::with_capacity(n);
        let mut redo_samples = Vec::with_capacity(n);
        for i in 0..n {
            ed.apply_edit(
                rev,
                &[Splice { at: pos, delete: 0, insert: "x".into() }],
                EditOrigin::User,
                i as f64 * 10_000.0, // far enough apart: never coalesces
            )
            .unwrap();

            let t = Instant::now();
            let _change = ed.undo().expect("an edit was just applied");
            undo_samples.push(t.elapsed().as_secs_f64() * 1e6);

            let t = Instant::now();
            let _change = ed.redo().expect("the undo above pushed a redo unit");
            redo_samples.push(t.elapsed().as_secs_f64() * 1e6);

            // Restore baseline (untimed) so next iteration's `pos` is valid
            // against the ORIGINAL text again. Only THIS revision is ever
            // read again (by next iteration's apply_edit), which is why the
            // undo/redo results above are intentionally discarded.
            let restore = ed.undo().expect("redo above re-applied the edit");
            rev = restore.revision;
        }
        let su = stats(undo_samples);
        let sr = stats(redo_samples);
        println!("{label:<8} {:<8} {su}", "undo");
        println!("{label:<8} {:<8} {sr}", "redo");

        assert!(
            su.p95 < 500_000.0 && sr.p95 < 500_000.0,
            "{label} undo/redo p95 exceeds the 500ms loose ceiling (undo {:.0}us, redo {:.0}us)",
            su.p95,
            sr.p95
        );
    }
}

// ---- (f) BlockIndex::update in isolation, scaled by block count ----------

/// Times `BlockIndex::update` ALONE (no parsing) for a realistic "typed one
/// character into an existing paragraph, no block boundary changed" batch:
/// every block after the edit point shifts by +1 byte, matching what
/// `map_range_shrink` would produce for that scenario. Isolates the
/// ID-rematching pass's own per-block constant, unclouded by parse cost.
#[test]
#[ignore = "M2 de-risk; run with --release --ignored --nocapture"]
fn block_index_update_scaling_1mb_3mb() {
    let n = iters(200);
    println!("\n=== BlockIndex::update alone (no parsing), 300KB/1MB/3MB block counts (release-mode) ===");
    println!("{:<8} {:>10} {:>34}", "size", "blocks", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);
        let parsed = parser::parse_document(&doc);
        let block_count = parsed.blocks.len();
        let mid_byte = doc.len() / 2;
        let batch = [ByteSplice { at: mid_byte, delete: 0, insert: "x".into() }];
        let shifted: Vec<(parser::BlockKind, std::ops::Range<usize>)> = parsed
            .blocks
            .iter()
            .map(|(k, r)| {
                let shift = |p: usize| if p >= mid_byte { p + 1 } else { p };
                (*k, shift(r.start)..shift(r.end))
            })
            .collect();

        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let mut idx = BlockIndex::new(1);
            idx.update(parsed.blocks.clone(), &[]); // seed (untimed baseline)
            let new_spans = shifted.clone();
            let t = Instant::now();
            idx.update(new_spans, &batch);
            samples.push(t.elapsed().as_secs_f64() * 1e6);
        }
        let s = stats(samples);
        println!("{label:<8} {:>10} {s}", block_count);

        assert!(
            s.p95 < 100_000.0,
            "{label} BlockIndex::update p95 {:.0}us exceeds the 100ms loose ceiling",
            s.p95
        );
    }
}

// ---- (f2) BlockIndex::update_range in isolation — the FIX for (f) --------

/// `research/09-1mb-derisk.md`'s ranked-fix-list item #2, implemented: the
/// windowed counterpart to (f) above, isolating `BlockIndex::update_range`
/// alone (no parsing, no `Editor`) for the SAME "typed one character into an
/// existing paragraph, no block boundary changed" scenario — except the
/// window handed to `update_range` is the small, realistic one
/// `reparse_incremental` actually computes (one block of slack before the
/// edit, converging at the very next old block boundary after it), not the
/// whole document. §8 of the report measured real convergence windows
/// averaging ~244 bytes (max 409) on a 1MB document for ordinary keystrokes
/// — a handful of blocks, not thousands — so this bench's window (2-3
/// blocks) is the representative case, not a cherry-picked best case.
#[test]
#[ignore = "M2 de-risk; run with --release --ignored --nocapture"]
fn block_index_update_range_scaling_1mb_3mb() {
    let n = iters(200);
    println!("\n=== BlockIndex::update_range alone (no parsing), windowed, 300KB/1MB/3MB (release-mode) ===");
    println!("{:<8} {:>10} {:>8} {:>34}", "size", "blocks", "window", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);
        let parsed = parser::parse_document(&doc);
        let block_count = parsed.blocks.len();
        let mid_byte = doc.len() / 2;
        let batch = [ByteSplice { at: mid_byte, delete: 0, insert: "x".into() }];
        let shifted: Vec<(parser::BlockKind, std::ops::Range<usize>)> = parsed
            .blocks
            .iter()
            .map(|(k, r)| {
                let shift = |p: usize| if p >= mid_byte { p + 1 } else { p };
                (*k, shift(r.start)..shift(r.end))
            })
            .collect();

        // Mirror `reparse_incremental`'s real window: one block of slack
        // before the block containing `mid_byte`, converging right at the
        // end of that same block (inserting one char never moves a block
        // boundary, so the very next old block end is already a valid
        // convergence point) — exactly `[containing - 1, containing + 1)`.
        let containing = parsed.blocks.partition_point(|(_, r)| r.start <= mid_byte) - 1;
        let prefix_end = containing.saturating_sub(1);
        let window_end = containing + 1;
        let fresh_spans = shifted[prefix_end..window_end].to_vec();
        let window_len = window_end - prefix_end;

        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let mut idx = BlockIndex::new(1);
            idx.update(parsed.blocks.clone(), &[]); // seed (untimed baseline)
            let fresh = fresh_spans.clone();
            let t = Instant::now();
            idx.update_range(prefix_end..window_end, fresh, 1, &batch);
            samples.push(t.elapsed().as_secs_f64() * 1e6);
        }
        let s = stats(samples);
        println!("{label:<8} {:>10} {:>8} {s}", block_count, window_len);

        // TIGHT ceiling (relative to (f)'s 100ms loose convention): a
        // windowed update over 2-3 blocks should cost low-single-digit
        // microseconds regardless of document size. 50us is still >10x
        // headroom over the expected cost, but it sits FAR below (f)'s own
        // measured full-rematch cost at this size (order of 10s-100s of
        // us, growing with doc size) — so a regression back to full-doc
        // rematch (e.g. someone widening the window unconditionally, or
        // routing this call through `update` again) trips this locally,
        // unlike (f)'s ceiling which is deliberately loose.
        assert!(
            s.p95 < 50.0,
            "{label} BlockIndex::update_range p95 {:.1}us exceeds the 50us tight ceiling — \
             this should be ~O(window), not O(doc); a regression to full-document rematch cost \
             would blow past this by 10-100x (see (f) above for that cost at this size)",
            s.p95
        );
    }
}

// ---- (g) anchor-mapping cost: apply_edit with 0 vs 1000 live anchors ------

#[test]
#[ignore = "M2 de-risk; run with --release --ignored --nocapture"]
fn anchor_mapping_cost_1mb_3mb() {
    let n = iters(100);
    const NUM_ANCHORS: usize = 1000;
    println!(
        "\n=== apply_edit mid-doc, 0 vs {NUM_ANCHORS} live anchors, 300KB/1MB/3MB (release-mode) ==="
    );
    println!("{:<8} {:>12} {:>34}", "size", "anchors", "timing");
    for &(label, target) in SIZES {
        let doc = generate_mixed_doc(target);

        for anchor_count in [0usize, NUM_ANCHORS] {
            let mut ed = Editor::new(1);
            let mut rev = ed.load(&doc);
            let len16 = ed.doc_len_utf16();
            for k in 0..anchor_count {
                let pos = (k * 997) % len16.max(1); // scattered, deterministic
                ed.create_anchor(pos, Bias::Before).unwrap();
            }

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
                samples.push(t.elapsed().as_secs_f64() * 1e6);
            }
            let s = stats(samples);
            println!("{label:<8} {:>12} {s}", anchor_count);

            assert!(
                s.p95 < 500_000.0,
                "{label}/{anchor_count} anchors apply_edit p95 {:.0}us exceeds the 500ms loose ceiling",
                s.p95
            );
        }
    }
}

// ---- (h) wasm-boundary-equivalent JSON serialization at 1MB --------------
// Direct-writer replica of oxidown-wasm's `decorations_json_string` (the
// CURRENT wire path — see perf_baseline.rs §8/(f) for the retired
// value-tree path this superseded). Native-only: excludes the actual
// wasm-bindgen/JS boundary crossing (measured separately, JS-side, in
// packages/oxidown-view-cm6/test/perf-1mb.bench.ts).

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
#[ignore = "M2 de-risk; run with --release --ignored --nocapture"]
fn wasm_decoration_json_serialization_1mb() {
    let n = iters(200);
    let viewport_cu = 3000usize;
    let doc = generate_mixed_doc(1024 * 1024);
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
    println!("\n=== wasm-boundary decoration JSON serialization, ~3k-CU viewport on 1MB doc ===");
    println!("decorations in viewport: {}", decos.len());

    let mut samples = Vec::with_capacity(n);
    let mut payload_bytes = 0usize;
    for _ in 0..n {
        let t = Instant::now();
        let json_str = decorations_json_string(&decos);
        let us = t.elapsed().as_secs_f64() * 1e6;
        payload_bytes = json_str.len();
        std::hint::black_box(&json_str);
        samples.push(us);
    }
    let s = stats(samples);
    println!("payload size: {payload_bytes} bytes");
    println!("direct writer: {s}");

    assert!(
        s.p95 < 100_000.0,
        "JSON serialization p95 {:.0}us exceeds the 100ms loose ceiling",
        s.p95
    );
}
