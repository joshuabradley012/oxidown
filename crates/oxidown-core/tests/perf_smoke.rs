//! Perf smoke test (ignored by default; bench-style, uses std::time under
//! native cargo test only — the core library itself never touches clocks).
//!
//! Run: cargo test -p oxidown-core --test perf_smoke -- --ignored --nocapture
//!
//! Measures apply_edit (1-char insert) + decorations (3k-CU viewport with a
//! cursor) combined on a ~100KB markdown document. Loose ceiling: p95 < 5ms.
//! The real 1ms budget (boundary contract) is measured from JS later.

use std::time::Instant;

use oxidown_core::{EditOrigin, Editor, SelectionRange, Splice};

fn generate_doc(target_bytes: usize) -> String {
    let mut doc = String::with_capacity(target_bytes + 256);
    let mut i = 0;
    while doc.len() < target_bytes {
        doc.push_str(&format!("## Section {i}\n\n"));
        doc.push_str(
            "Lorem **ipsum** dolor *sit* amet, `consectetur` adipiscing elit. \
             Some 你好 CJK and an emoji 😀 mixed in with __strong__ text and \
             _emphasis_ plus ***bold italic*** runs to exercise the parser.\n\n",
        );
        doc.push_str("Plain paragraph with no formatting at all, just words words words.\n\n");
        i += 1;
    }
    doc
}

#[test]
#[ignore = "perf smoke; run with --ignored --nocapture"]
fn apply_edit_plus_decorations_under_5ms_on_100kb() {
    let doc = generate_doc(100 * 1024);
    println!(
        "doc: {} bytes, {} utf16 CU",
        doc.len(),
        doc.chars().map(char::len_utf16).sum::<usize>()
    );
    let mut ed = Editor::new(1);
    let mut rev = ed.load(&doc);
    let len16 = ed.doc_len_utf16();

    const ITERS: usize = 300;
    let mut samples_us: Vec<f64> = Vec::with_capacity(ITERS);
    let viewport_cu = 3000usize;

    for i in 0..ITERS {
        // Insert one ASCII char at a wandering (valid: doc is regenerated
        // each run so positions near multiples are fine — we insert at an
        // ASCII-only region by choosing a position right after a newline).
        let raw = (i * 331) % len16;
        let pos = nearest_boundary(&mut ed, raw);

        let t = Instant::now();
        rev = ed
            .apply_edit(
                rev,
                &[Splice { at: pos, delete: 0, insert: "x".into() }],
                EditOrigin::User,
                i as f64 * 1000.0, // spaced out: no coalescing pathologies
            )
            .unwrap();
        let apply_us = t.elapsed().as_secs_f64() * 1e6;

        // Snapping the viewport to valid boundaries is a test artifact
        // (a real view passes positions from its own buffer), so it is
        // excluded from the measurement.
        let vp_from = nearest_boundary(&mut ed, pos.saturating_sub(viewport_cu / 2));
        let vp_raw = (vp_from + viewport_cu).min(ed.doc_len_utf16());
        let vp_to = nearest_boundary(&mut ed, vp_raw);

        let t = Instant::now();
        let _decos = ed
            .decorations(
                rev,
                vp_from,
                vp_to,
                &[SelectionRange { anchor: pos + 1, head: pos + 1 }],
            )
            .unwrap();
        let deco_us = t.elapsed().as_secs_f64() * 1e6;
        samples_us.push(apply_us + deco_us);
    }

    samples_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples_us[ITERS / 2];
    let p95 = samples_us[ITERS * 95 / 100];
    let max = samples_us[ITERS - 1];
    let mean = samples_us.iter().sum::<f64>() / ITERS as f64;
    println!(
        "apply_edit(1 char) + decorations(3k CU viewport) on ~100KB doc over {ITERS} iters:"
    );
    println!("  mean {mean:.0}µs  p50 {p50:.0}µs  p95 {p95:.0}µs  max {max:.0}µs");

    assert!(
        p95 < 5000.0,
        "p95 {p95:.0}µs exceeds the 5ms loose ceiling"
    );
}

/// Snap a raw CU offset to a valid boundary (never inside a surrogate pair).
///
/// Probes with an all-no-op splice batch: `apply_edit` strictly validates
/// splice positions (SurrogateSplit) BEFORE discovering the batch is a
/// no-op, and a no-op batch never mutates or bumps the revision — so this
/// is a pure validity check. (The previous probe used `decorations(pos,
/// pos)`, which stopped rejecting mid-surrogate positions when contract
/// v0.1 made query positions SNAP instead of error — a latent test-helper
/// bug that let strict `apply_edit` positions through unvalidated.)
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
