//! Reparse-path equivalence gates: every fast path (`reparse_tail` for
//! qualifying end-of-document edits; the incremental window path for
//! mid-document edits) must produce an overlay + block index BYTE-IDENTICAL
//! to a from-scratch `parser::parse_document` of the same text — node lists
//! compared `Vec`-for-`Vec` (order included; `parse_document` stable-sorts by
//! extent start, and the splicing paths preserve that order), block spans and
//! kinds compared exactly. Block IDs are *sticky* by design and therefore not
//! comparable against a fresh parse; their stability has its own tests in
//! `block_index.rs` / `streaming.rs`.
//!
//! These tests run un-ignored (they're fast) so CI always gates equivalence.
//! Seeds are fixed for reproducibility; `OXIDOWN_FUZZ_EDITS` scales the edit
//! count up for deeper local runs.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use oxidown_core::{parser, Command, EditOrigin, Editor, Splice};

// ---- shared helpers --------------------------------------------------------

fn fuzz_edits(default: usize) -> usize {
    std::env::var("OXIDOWN_FUZZ_EDITS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Mixed-markdown corpus (same shape as perf_baseline.rs's generator):
/// headings, inline-marked paragraphs, nested blockquotes, fenced code,
/// nested/task/ordered lists, thematic breaks, CJK + emoji.
fn generate_mixed_doc(target_bytes: usize) -> String {
    let mut doc = String::with_capacity(target_bytes + 2048);
    doc.push_str("# Equivalence corpus\n\nMixed constructs for reparse fuzzing.\n\n");
    let mut i = 0usize;
    while doc.len() < target_bytes {
        match i % 6 {
            0 => {
                doc.push_str(&format!("## Section {i}\n\n"));
                doc.push_str(&format!(
                    "Lorem **ipsum {i}** dolor *sit* amet, `consectetur` adipiscing. \
                     Some 你好 CJK and an emoji 😀 with __strong__ text and _emphasis_ \
                     plus ***bold italic***, a [link](https://example.com/{i}) and an \
                     autolink <https://oxidown.dev/{i}>.\n\n"
                ));
            }
            1 => {
                doc.push_str(&format!(
                    "Plain paragraph {i} with no formatting, just words to pad out \
                     prose between the richer constructs.\n\n"
                ));
            }
            2 => {
                doc.push_str(&format!(
                    "> Quoted line one at {i}.\n> > Nested with **bold** and `code`.\n\
                     > Back to depth one.\n\n"
                ));
            }
            3 => {
                doc.push_str(&format!(
                    "```rust\nfn section_{i}() -> u32 {{\n    let x = {i};\n    x * 2\n}}\n```\n\n"
                ));
            }
            4 => {
                doc.push_str(&format!(
                    "- item one at {i}\n- item two with **bold**\n  - nested alpha\n  \
                     - nested beta\n- [ ] a task\n- [x] done task\n1. ordered one\n2. ordered two\n\n"
                ));
            }
            _ => {
                doc.push_str(&format!(
                    "### Subsection {i}\n\nA paragraph with ~~strike~~ and *style*.\n\n---\n\n"
                ));
            }
        }
        i += 1;
    }
    doc
}

/// Adversarial insert pool: block-boundary-changing constructs (fence
/// markers, list markers, quote markers, headings, setext underlines, blank
/// lines, de-interrupting plain chars) plus multibyte text and alternate
/// line terminators (`\r`, `\r\n` — pulldown treats a lone `\r` as a line
/// ending, and so must every fast path).
const INSERTS: &[&str] = &[
    "x",
    "words and more ",
    "\n",
    "\n\n",
    "\r",
    "\r\n",
    "```",
    "```rust\n",
    "# ",
    "## heading\n",
    "- ",
    "- [ ] ",
    "1. ",
    "> ",
    "===\n",
    "---\n",
    "**",
    "*",
    "`",
    "~~",
    "    ",
    "\t",
    "[l](u)",
    "😀",
    "你好",
    "-",
    "x# ",
];

/// Snap `pos` down to a char boundary of `s`.
fn floor_char(s: &str, mut pos: usize) -> usize {
    pos = pos.min(s.len());
    while !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

fn utf16_at(s: &str, byte: usize) -> usize {
    s[..byte].encode_utf16().count()
}

/// Assert the editor's cached overlay + block index equal a from-scratch
/// parse of its current text. `ctx` names the failing step.
fn assert_equivalent(ed: &Editor, ctx: &str) {
    let text = ed.get_text();
    let expect = parser::parse_document(&text);
    let got = ed.overlay_nodes();
    if got != expect.nodes.as_slice() {
        // Locate the first divergence for a readable failure.
        let n = got.len().min(expect.nodes.len());
        let first = (0..n).find(|&i| got[i] != expect.nodes[i]).unwrap_or(n);
        panic!(
            "{ctx}: overlay diverges from from-scratch parse at node {first}\n\
             cached:  {:?}\nexpect:  {:?}\n(cached {} nodes, expected {})\ndoc len {} bytes",
            got.get(first),
            expect.nodes.get(first),
            got.len(),
            expect.nodes.len(),
            text.len(),
        );
    }
    let got_blocks: Vec<_> = ed
        .block_index()
        .blocks()
        .iter()
        .map(|b| (b.kind, b.span.clone()))
        .collect();
    assert_eq!(
        got_blocks, expect.blocks,
        "{ctx}: block index (kind, span) diverges from from-scratch parse"
    );
}

/// One random splice against `ed` + the mirror string. Returns false if the
/// edit was a no-op (skipped).
fn random_edit(ed: &mut Editor, mirror: &mut String, rng: &mut StdRng, lo_byte: usize) -> bool {
    let lo = floor_char(mirror, lo_byte);
    let at_b = floor_char(mirror, rng.gen_range(lo..=mirror.len()));
    let (delete_b, insert) = if rng.gen_bool(0.35) && at_b < mirror.len() {
        // Delete 1..=40 bytes (char-snapped), sometimes replacing.
        let end = floor_char(mirror, (at_b + rng.gen_range(1..=40)).min(mirror.len()));
        let ins = if rng.gen_bool(0.3) { INSERTS[rng.gen_range(0..INSERTS.len())] } else { "" };
        (end - at_b, ins)
    } else {
        (0, INSERTS[rng.gen_range(0..INSERTS.len())])
    };
    if delete_b == 0 && insert.is_empty() {
        return false;
    }
    let at16 = utf16_at(mirror, at_b);
    let del16 = mirror[at_b..at_b + delete_b].encode_utf16().count();
    let rev = ed.revision();
    ed.apply_edit(
        rev,
        &[Splice { at: at16, delete: del16, insert: insert.into() }],
        EditOrigin::User,
        0.0,
    )
    .expect("char-snapped positions are always valid");
    mirror.replace_range(at_b..at_b + delete_b, insert);
    true
}

// ---- Stage A: tail fast path in apply_edit ---------------------------------

/// Random edits concentrated in the document's tail region (including
/// positions at/before the last block's start and inside its first line, so
/// both the fast path and its fallback guards are exercised), each checked
/// node-for-node against a from-scratch parse.
#[test]
fn tail_edits_match_full_reparse_node_for_node() {
    let doc = generate_mixed_doc(8 * 1024);
    let mut ed = Editor::new(1);
    ed.load(&doc);
    let mut mirror = doc.clone();
    let mut rng = StdRng::seed_from_u64(0xda11_fa57);

    let edits = fuzz_edits(400);
    let mut applied = 0usize;
    for step in 0..edits {
        // Everything from ~85% of the doc onward, so edits land before,
        // at, and after the last top-level block's start.
        let lo = mirror.len().saturating_sub(mirror.len() / 7 + 64);
        if random_edit(&mut ed, &mut mirror, &mut rng, lo) {
            applied += 1;
            assert_eq!(ed.get_text(), mirror, "step {step}: text mirror agreement");
            assert_equivalent(&ed, &format!("tail fuzz step {step}"));
        }
    }
    let counts = ed.reparse_counts();
    assert!(applied > edits / 2, "fuzz actually applied edits ({applied})");
    assert!(
        counts.tail > 0,
        "the tail fast path never fired across {applied} tail-region edits: {counts:?}"
    );
    println!("tail fuzz: {applied} edits, reparse counts {counts:?}");
}

/// The flagship use case: typing text character by character at the end of
/// the document. Every keystroke must take the tail fast path (not a full
/// reparse) and stay node-identical to a full parse.
#[test]
fn typing_at_eof_takes_the_tail_path_every_keystroke() {
    let doc = generate_mixed_doc(8 * 1024);
    let mut ed = Editor::new(1);
    let mut rev = ed.load(&doc);
    let full_before = ed.reparse_counts().full;

    let mut mirror = doc.clone();
    for (i, ch) in "\nSome **new** text with `code` and a [link](https://x.dev)."
        .chars()
        .enumerate()
    {
        let at16 = utf16_at(&mirror, mirror.len());
        let s = ch.to_string();
        rev = ed
            .apply_edit(
                rev,
                &[Splice { at: at16, delete: 0, insert: s.clone() }],
                EditOrigin::User,
                i as f64 * 1000.0,
            )
            .unwrap();
        mirror.push_str(&s);
        assert_equivalent(&ed, &format!("eof keystroke {i}"));
    }
    let counts = ed.reparse_counts();
    assert_eq!(
        counts.full, full_before,
        "no keystroke fell back to a full reparse: {counts:?}"
    );
    assert!(counts.tail >= 10, "tail path fired per keystroke: {counts:?}");
}

/// Directed hazard cases on the tail block's FIRST line — the edits the fast
/// path must refuse (they can merge the tail block into the block above,
/// which a standalone tail parse cannot see). Each must fall back to the
/// full path and stay node-identical.
#[test]
fn first_line_hazard_edits_fall_back_and_stay_correct() {
    // (doc, utf16 edit position, insert, delete)
    let last_start = |s: &str, needle: &str| s.rfind(needle).unwrap();
    let cases: Vec<(String, usize, &str, usize)> = vec![
        // De-interruption: "para\n# head" -> insert 'x' before '#'.
        ("alpha *em\npara text\n# head*tail".into(), last_start("alpha *em\npara text\n# head*tail", "# head"), "x", 0),
        // Delete the '#' marker itself.
        ("alpha *em\npara text\n# head*tail".into(), last_start("alpha *em\npara text\n# head*tail", "# head"), "", 1),
        // De-listing: "- a" -> "-xa" (insert right after the marker glyph).
        ("para above\n- last item".into(), last_start("para above\n- last item", "- last") + 1, "x", 0),
        // Setext-ification: last block's first line becomes "===".
        ("some paragraph\nlast".into(), last_start("some paragraph\nlast", "last"), "===\n", 4),
        // Indent capture: "- item\n\npara" -> indent para's first line.
        ("- item one\n- item two\n\npara tail".into(), last_start("- item one\n- item two\n\npara tail", "para tail"), "    ", 0),
    ];
    for (i, (doc, at, insert, delete)) in cases.into_iter().enumerate() {
        let mut ed = Editor::new(1);
        let rev = ed.load(&doc);
        ed.apply_edit(
            rev,
            &[Splice { at, delete, insert: insert.into() }],
            EditOrigin::User,
            0.0,
        )
        .unwrap();
        assert_equivalent(&ed, &format!("first-line hazard case {i}"));
    }
}

// ---- Stage B: incremental (windowed) mid-document reparse ------------------

/// The big gate: random edits ANYWHERE in a mixed ~30KB document —
/// block-boundary-changing inserts (fences, markers, quotes, headings,
/// setext underlines, blank lines), deletes crossing block boundaries,
/// undo/redo, and indent/outdent commands — with the overlay + block index
/// compared node-for-node against a from-scratch parse after EVERY step.
#[test]
fn whole_doc_fuzz_matches_full_reparse_node_for_node() {
    let doc = generate_mixed_doc(30 * 1024);
    let mut ed = Editor::new(1);
    ed.load(&doc);
    let mut mirror = doc.clone();
    let mut rng = StdRng::seed_from_u64(0x71c4_e5d1_b10c_ca15);

    let edits = fuzz_edits(400);
    for step in 0..edits {
        match rng.gen_range(0u8..20) {
            // Occasionally undo (and sometimes redo right after): history
            // units route through the same incremental path.
            0 => {
                if ed.undo().is_some() {
                    mirror = ed.get_text();
                    assert_equivalent(&ed, &format!("step {step}: undo"));
                    if rng.gen_bool(0.5) && ed.redo().is_some() {
                        mirror = ed.get_text();
                        assert_equivalent(&ed, &format!("step {step}: redo"));
                    }
                }
            }
            // Occasionally run a (multi-splice) command at a random spot.
            1 => {
                let pos = utf16_at(&mirror, floor_char(&mirror, rng.gen_range(0..=mirror.len())));
                let cmd = if rng.gen_bool(0.5) {
                    Command::IndentList { from: pos, to: pos }
                } else {
                    Command::OutdentList { from: pos, to: pos }
                };
                if ed.command(cmd).unwrap().is_some() {
                    mirror = ed.get_text();
                    assert_equivalent(&ed, &format!("step {step}: {cmd:?}"));
                }
            }
            _ => {
                if random_edit(&mut ed, &mut mirror, &mut rng, 0) {
                    assert_eq!(ed.get_text(), mirror, "step {step}: text mirror agreement");
                    assert_equivalent(&ed, &format!("whole-doc fuzz step {step}"));
                }
            }
        }
    }
    let counts = ed.reparse_counts();
    println!("whole-doc fuzz: {edits} steps, reparse counts {counts:?}");
    assert!(
        counts.incremental > counts.full,
        "the incremental path must carry most mid-document edits, not fall \
         back: {counts:?}"
    );
}

/// The canonical non-converging edit: opening a fence mid-document swallows
/// everything below it, so no window can realign with old block boundaries
/// — the incremental path MUST degrade (to a tail/full reparse) and stay
/// node-identical. Closing the fence again must also stay correct (and the
/// fresh boundary realignment lets later edits converge again).
#[test]
fn fence_open_mid_doc_degrades_gracefully() {
    let doc = generate_mixed_doc(12 * 1024);
    let mut ed = Editor::new(1);
    ed.load(&doc);
    let mut mirror = doc.clone();

    // Type "```" (char by char, like a user) at the start of a mid-document
    // line: after the third backtick a real fence opens and swallows the
    // rest of the document.
    let mid_line_start = {
        let mid = floor_char(&mirror, mirror.len() / 2);
        mirror[..mid].rfind('\n').map_or(0, |i| i + 1)
    };
    for (i, ch) in ["`", "`", "`"].iter().enumerate() {
        let at_b = mid_line_start + i;
        let at16 = utf16_at(&mirror, at_b);
        let rev = ed.revision();
        ed.apply_edit(
            rev,
            &[Splice { at: at16, delete: 0, insert: (*ch).into() }],
            EditOrigin::User,
            0.0,
        )
        .unwrap();
        mirror.insert_str(at_b, ch);
        assert_equivalent(&ed, &format!("fence-open keystroke {i}"));
    }
    // The overlay must now see one giant fenced block to EOF (spot check:
    // a CodeBlockLine node exists beyond the fence line).
    assert!(
        ed.overlay_nodes().iter().any(|n| {
            matches!(n.kind, parser::NodeKind::CodeBlockLine) && n.extent.start > mid_line_start
        }),
        "the opened fence swallows following text"
    );

    // Now close it a few lines further down: a newline-terminated "```".
    let close_at = {
        let mut pos = mid_line_start + 3;
        for _ in 0..4 {
            if let Some(nl) = mirror[pos..].find('\n') {
                pos += nl + 1;
            }
        }
        pos
    };
    let close_at = floor_char(&mirror, close_at);
    let at16 = utf16_at(&mirror, close_at);
    let rev = ed.revision();
    ed.apply_edit(
        rev,
        &[Splice { at: at16, delete: 0, insert: "```\n".into() }],
        EditOrigin::User,
        0.0,
    )
    .unwrap();
    mirror.insert_str(close_at, "```\n");
    assert_equivalent(&ed, "fence close");

    // With boundaries realigned, an ordinary mid-doc edit converges again.
    let before = ed.reparse_counts().incremental;
    let deep = floor_char(&mirror, mirror.len() * 3 / 4);
    let at16 = utf16_at(&mirror, deep);
    let rev = ed.revision();
    ed.apply_edit(
        rev,
        &[Splice { at: at16, delete: 0, insert: "x".into() }],
        EditOrigin::User,
        0.0,
    )
    .unwrap();
    mirror.insert(deep, 'x');
    assert_equivalent(&ed, "post-fence ordinary edit");
    assert_eq!(
        ed.reparse_counts().incremental,
        before + 1,
        "ordinary mid-doc edits converge incrementally again after the fence closes"
    );
}

// ---- Stage C: streaming append fast path ------------------------------------

/// Streaming analog of `tail_edits_match_full_reparse_node_for_node`:
/// streams opened at directed + random positions (document start/end, block
/// starts, just after block boundaries, mid-block), fed random small appends
/// including newline-bearing and hazard-shaped chunks — after EVERY append
/// the overlay AND block index (spans + kinds) must match a from-scratch
/// parse of the current text.
#[test]
fn stream_appends_match_full_reparse_node_for_node() {
    const CHUNKS: &[&str] = &[
        "x",
        "words and more ",
        "\n",
        "\n\n",
        "\r\n",
        "# ",
        "## head\n",
        "- ",
        "- item\n",
        "1. ",
        "> quoted\n",
        "===\n",
        "---\n",
        "**",
        "*em",
        "em*",
        "`",
        "```",
        "    ",
        "😀",
        "你好",
        "x# ",
        "-",
    ];
    // Hazard-shaped document tails: the last block sits directly against a
    // paragraph (no insulating blank line) or under an absorbing list, so a
    // stream opened on its first line exercises exactly the merge hazards
    // the fast path must refuse.
    const HAZARD_TAILS: &[&str] = &[
        "tail para\n# head",
        "para *em\n# head*",
        "prose words\n- item",
        "- item one\n\ntail para",
        "closing words\nlast",
    ];
    let mut rng = StdRng::seed_from_u64(0x57e4_a11f);
    let appends = fuzz_edits(24);
    let mut total_tail = 0u64;
    for round in 0..24usize {
        let mut doc = generate_mixed_doc(512 + (round % 6) * 1024);
        if round % 2 == 1 {
            doc.push_str(HAZARD_TAILS[rng.gen_range(0..HAZARD_TAILS.len())]);
        }
        // Candidate open positions: every top-level block's start /
        // just-after-start / end / middle (so streams open at block
        // boundaries, right past them, before headings/lists, and
        // mid-block), plus doc start/end and a few purely random interior
        // points — biased toward the LAST two blocks, where the tail fast
        // path (and its hazard analysis) lives.
        let blocks = parser::parse_document(&doc).blocks;
        let mut candidates: Vec<usize> = vec![0, doc.len()];
        let push_block = |candidates: &mut Vec<usize>, span: &std::ops::Range<usize>| {
            candidates.push(span.start);
            candidates.push(floor_char(&doc, (span.start + 1).min(doc.len())));
            candidates.push(span.end);
            candidates.push(floor_char(&doc, (span.start + span.end) / 2));
        };
        for (_, span) in &blocks {
            push_block(&mut candidates, span);
        }
        for (_, span) in blocks.iter().rev().take(2) {
            for _ in 0..4 {
                push_block(&mut candidates, span); // tail-region bias
            }
        }
        for _ in 0..4 {
            candidates.push(floor_char(&doc, rng.gen_range(0..=doc.len())));
        }
        let open_b = floor_char(&doc, candidates[rng.gen_range(0..candidates.len())]);

        let mut ed = Editor::new(1);
        ed.load(&doc);
        let id = ed.stream_open(utf16_at(&doc, open_b)).unwrap();
        for step in 0..appends {
            let chunk = CHUNKS[rng.gen_range(0..CHUNKS.len())];
            ed.stream_append(id, chunk).unwrap();
            assert_equivalent(
                &ed,
                &format!("stream round {round} (open at byte {open_b}) append {step} {chunk:?}"),
            );
        }
        ed.stream_close(id);
        total_tail += ed.reparse_counts().tail;
    }
    assert!(total_tail > 0, "the streaming tail fast path never fired");
}

/// Deterministic regressions for the streaming fast path's first-line
/// hazards: an append into the tail block's FIRST line can merge it with the
/// block above (de-interruption / lazy continuation), which a standalone
/// tail-slice parse cannot see — `stream_append` must run the same hazard
/// analysis as `apply_edit` and fall back. Pre-fix, the weak precondition
/// took the fast path and both the overlay (missing emphasis marks) and the
/// block index diverged from a full parse.
#[test]
fn stream_append_first_line_hazards_fall_back_and_stay_correct() {
    // (doc, stream-open position (UTF-16 == bytes, all-ASCII), chunk)
    let cases: &[(&str, usize, &str)] = &[
        // De-interruption: "x# head" is a lazy continuation — one paragraph.
        ("para\n# head", 5, "x"),
        // Same shape with an emphasis pair spanning the merge: the merged
        // paragraph gains em marks the fast path used to drop entirely.
        ("para *em\n# head*", 9, "x"),
        // De-listing: "x- item" merges into the paragraph above.
        ("para\n- item", 5, "x"),
        // Indent capture: "    para" is absorbed by the list ABOVE the
        // insulating blank line.
        ("- item\n\npara", 8, "    "),
        // Newline-bearing chunk at the tail block's start (safe shape — the
        // block above is a paragraph — so the fast path may fire; either
        // way the result must match a full parse).
        ("alpha\n\npara", 7, "# h\nx"),
    ];
    for (i, &(doc, at, chunk)) in cases.iter().enumerate() {
        let mut ed = Editor::new(1);
        ed.load(doc);
        let id = ed.stream_open(at).unwrap();
        ed.stream_append(id, chunk).unwrap();
        assert_equivalent(&ed, &format!("stream hazard case {i}: {doc:?} + {chunk:?}"));
        ed.stream_close(id);
    }
}

/// Perf-shape guard: safe appends at EOF must keep taking the tail fast
/// path (no full-document reparse), with the stricter hazard analysis in
/// place.
#[test]
fn safe_eof_stream_appends_take_the_tail_path() {
    let doc = generate_mixed_doc(4 * 1024);
    let mut ed = Editor::new(1);
    ed.load(&doc);
    let full_before = ed.reparse_counts().full;
    let id = ed.stream_open(ed.doc_len_utf16()).unwrap();
    let chunks = [
        "streamed words ",
        "more **bold** here",
        "\nsecond line",
        "\n\n## streamed head\n\nfresh para",
    ];
    for chunk in chunks {
        ed.stream_append(id, chunk).unwrap();
        assert_equivalent(&ed, "safe eof append");
    }
    ed.stream_close(id);
    let counts = ed.reparse_counts();
    assert_eq!(counts.full, full_before, "no full reparse: {counts:?}");
    assert!(
        counts.tail >= chunks.len() as u64,
        "every safe EOF append re-parsed only the tail: {counts:?}"
    );
}

/// Multi-splice batches take the same incremental path with a UNION dirty
/// region — several splices across different blocks, checked node-for-node.
#[test]
fn multi_splice_batches_match_full_reparse() {
    let doc = generate_mixed_doc(12 * 1024);
    let mut ed = Editor::new(1);
    ed.load(&doc);
    let mut mirror = doc.clone();
    let mut rng = StdRng::seed_from_u64(0x5b11_ce5a);

    for step in 0..fuzz_edits(60) {
        // 2-4 ascending, non-overlapping splices, clustered mid-document
        // (like a command batch) roughly half the time, spread out the rest.
        let n = rng.gen_range(2..=4);
        let clustered = rng.gen_bool(0.5);
        let mut at = if clustered {
            floor_char(&mirror, rng.gen_range(0..mirror.len() / 2))
        } else {
            floor_char(&mirror, rng.gen_range(0..mirror.len() / 4))
        };
        let mut splices16 = Vec::new();
        let mut byte_edits = Vec::new();
        for _ in 0..n {
            let gap = if clustered { rng.gen_range(1..80) } else { rng.gen_range(200..2000) };
            at = floor_char(&mirror, (at + gap).min(mirror.len()));
            let end = floor_char(&mirror, (at + rng.gen_range(0..8)).min(mirror.len()));
            let insert = INSERTS[rng.gen_range(0..INSERTS.len())];
            splices16.push(Splice {
                at: utf16_at(&mirror, at),
                delete: mirror[at..end].encode_utf16().count(),
                insert: insert.into(),
            });
            byte_edits.push((at, end, insert));
            at = end;
        }
        let rev = ed.revision();
        ed.apply_edit(rev, &splices16, EditOrigin::User, 0.0).unwrap();
        for &(at, end, insert) in byte_edits.iter().rev() {
            mirror.replace_range(at..end, insert);
        }
        assert_eq!(ed.get_text(), mirror, "step {step}: text mirror agreement");
        assert_equivalent(&ed, &format!("multi-splice step {step}"));
    }
    println!("multi-splice fuzz reparse counts: {:?}", ed.reparse_counts());
}
