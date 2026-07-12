//! Streaming append fast-path perf test (ignored by default; bench-style,
//! uses std::time under native cargo test only — the core library itself
//! never touches clocks).
//!
//! Run: cargo test -p oxidown-core --test stream_perf -- --ignored --nocapture
//!
//! Streams ~2000 chunks of ~50 chars each into a ~100KB document (doc grows
//! to ~200KB) and asserts the MEAN append cost stays under the 1ms budget
//! (boundary v0.2 "Append fast-path"). The fast path re-parses only the
//! open tail block, so per-append cost must not scale with DOCUMENT size —
//! it does scale with the open TAIL BLOCK's size (the whole block is
//! re-parsed per append; see `reparse_tail`'s COST NOTE in editor.rs). The
//! budget test below passes partly because its chunk pool injects a block
//! boundary every ~40 chunks, resetting the tail block; the second test
//! characterizes the never-closing-block worst case explicitly.

use std::time::Instant;

use oxidown_core::Editor;

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
fn stream_append_mean_under_1ms_on_100kb() {
    let doc = generate_doc(100 * 1024);
    let mut ed = Editor::new(1);
    ed.load(&doc);
    println!(
        "doc: {} bytes, {} utf16 CU",
        doc.len(),
        ed.doc_len_utf16()
    );

    let id = ed.stream_open(ed.doc_len_utf16()).unwrap();
    const CHUNKS: usize = 2000;
    // ~50 chars, mixed content: plain words, inline markdown, occasional
    // newlines and (every ~40 chunks) a block boundary so the tail block
    // both grows and splits along the way.
    let chunk_pool = [
        "lorem ipsum dolor sit amet consectetur adipiscing ",
        "words **bold run** and *emphasis* plus `code x` ..",
        "streaming line content with 你好 mixed in as well.\n",
        "more plain streaming words to extend the tail here ",
        "\n\n## New streamed section\n\nfresh paragraph starts ",
    ];

    let mut samples_us: Vec<f64> = Vec::with_capacity(CHUNKS);
    let mut appended = 0usize;
    for i in 0..CHUNKS {
        let chunk = if i % 40 == 39 { chunk_pool[4] } else { chunk_pool[i % 4] };
        appended += chunk.len();
        let t = Instant::now();
        ed.stream_append(id, chunk).unwrap();
        samples_us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    ed.stream_close(id);

    // Correctness spot checks alongside the timing.
    assert_eq!(ed.get_text().len(), doc.len() + appended);
    assert_eq!(
        ed.history_depths().0,
        1,
        "the whole 2000-chunk stream is one undo unit"
    );

    samples_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = samples_us.iter().sum::<f64>() / samples_us.len() as f64;
    let p50 = samples_us[samples_us.len() / 2];
    let p95 = samples_us[samples_us.len() * 95 / 100];
    let max = samples_us[samples_us.len() - 1];
    println!("stream_append over {CHUNKS} chunks (~50 chars) into ~100KB doc (grew by {appended} bytes):");
    println!("  mean {mean:.0}µs  p50 {p50:.0}µs  p95 {p95:.0}µs  max {max:.0}µs");

    assert!(
        mean < 1000.0,
        "mean append cost {mean:.0}µs exceeds the 1ms budget"
    );

    // The one-step undo after 2000 merged appends must also stay sane.
    let t = Instant::now();
    ed.undo().unwrap();
    println!("  undo of the whole stream: {:.0}µs", t.elapsed().as_secs_f64() * 1e6);
    assert_eq!(ed.get_text(), doc, "undo reverts the entire stream");
}

/// Characterization (deliberately NOT a growth gate): per-append cost when
/// the streamed content never closes the tail block — one ever-growing
/// paragraph with no blank lines, a realistic AI-output shape (a single
/// top-level list with no blank lines behaves identically: one List block).
/// The tail fast path re-parses the whole open block on every append, so
/// per-append cost grows linearly with streamed-so-far (quadratic total) —
/// see `reparse_tail`'s COST NOTE in editor.rs for why a bounded per-append
/// update isn't provably safe with a non-incremental inline parser. This
/// test PRINTS the growth curve (decile means + last/first ratio) and
/// asserts only a generous absolute ceiling, so it stays CI-safe while
/// still catching an order-of-magnitude regression — and keeps passing if
/// the growth is ever actually fixed.
#[test]
#[ignore = "perf characterization; run with --ignored --nocapture"]
fn stream_append_into_never_closing_tail_block_grows_per_append() {
    // ~100KB of closed blocks above: untouched by the appends, present so
    // the measurement reflects tail-block growth, not document size.
    let doc = generate_doc(100 * 1024);
    let mut ed = Editor::new(1);
    ed.load(&doc);
    let id = ed.stream_open(ed.doc_len_utf16()).unwrap();

    // Open a fresh paragraph, then stream ~4600 chunks that NEVER produce a
    // blank line: soft line breaks only, so the tail block never closes and
    // grows to ~230KB. Inline-marked content (the realistic AI-output
    // shape), so the per-append re-parse pays real inline work.
    ed.stream_append(id, "\n\n").unwrap();
    const CHUNKS: usize = 4600;
    let chunk_pool = [
        "words streaming into one endless paragraph block h", // 50 bytes
        "lorem **ipsum** dolor *sit* amet `consectetur` and ",
        "prose with 你好 CJK and an emoji 😀 plus _emphasis_\n", // soft break
        "more [links](https://example.com) and ~~strikes~~ ",
    ];
    let mut samples_us: Vec<f64> = Vec::with_capacity(CHUNKS);
    let mut appended = 2usize;
    for i in 0..CHUNKS {
        let chunk = chunk_pool[i % chunk_pool.len()];
        appended += chunk.len();
        let t = Instant::now();
        ed.stream_append(id, chunk).unwrap();
        samples_us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    ed.stream_close(id);
    assert_eq!(ed.get_text().len(), doc.len() + appended);
    assert_eq!(ed.history_depths().0, 1, "still one undo unit");
    let blocks = ed.block_index().blocks();
    let tail_span = blocks.last().unwrap().span.clone();
    assert!(
        // Slack for the opening "\n\n" and a possibly-excluded trailing
        // newline; the point is the whole streamed body is ONE block.
        tail_span.end - tail_span.start >= appended - 8,
        "the streamed content stayed one never-closing tail block \
         (tail span {tail_span:?}, appended {appended} bytes)"
    );

    let bucket = CHUNKS / 10;
    let decile_means: Vec<f64> = samples_us
        .chunks(bucket)
        .map(|c| c.iter().sum::<f64>() / c.len() as f64)
        .collect();
    println!(
        "never-closing tail block: per-append decile means over {CHUNKS} \
         ~50-byte chunks (tail block grows to ~{}KB):",
        appended / 1024
    );
    for (i, m) in decile_means.iter().enumerate() {
        println!(
            "  decile {i}: {m:>6.0}µs  (tail block ~{}KB)",
            (i + 1) * bucket * 50 / 1024
        );
    }
    let first = decile_means[0];
    let last = *decile_means.last().unwrap();
    println!("  growth ratio last/first decile: {:.1}x", last / first);

    // Ceiling only (measured last-decile mean ~500-800µs at ~230KB on a
    // 2023 laptop): an order of magnitude of headroom, so CI variance never
    // trips it — and a fix for the growth only makes it greener.
    assert!(
        last < 10_000.0,
        "last-decile mean {last:.0}µs blew the 10ms characterization ceiling"
    );

    // The one-step undo must still revert the entire stream exactly.
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), doc, "undo reverts the entire stream");
}
