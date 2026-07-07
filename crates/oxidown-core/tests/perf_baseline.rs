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
    let s = stats(samples);
    println!("payload size: {payload_bytes} bytes");
    println!("{s}");

    assert!(
        s.p95 < 50_000.0,
        "JSON serialization p95 {:.0}us exceeds the 50ms loose ceiling",
        s.p95
    );
}
