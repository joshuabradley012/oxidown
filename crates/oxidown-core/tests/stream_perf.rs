//! Streaming append fast-path perf test (ignored by default; bench-style,
//! uses std::time under native cargo test only — the core library itself
//! never touches clocks).
//!
//! Run: cargo test -p oxidown-core --test stream_perf -- --ignored --nocapture
//!
//! Streams ~2000 chunks of ~50 chars each into a ~100KB document (doc grows
//! to ~200KB) and asserts the MEAN append cost stays under the 1ms budget
//! (boundary v0.2 "Append fast-path"). The fast path re-parses only the
//! open tail block, so per-append cost must not scale with document size.

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
