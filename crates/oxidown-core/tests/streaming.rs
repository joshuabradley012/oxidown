//! Streaming contract tests (boundary v0.2 "Streaming", plan §5.9):
//! open/append/close life cycle, mirror-verified splices, single-undo-unit
//! semantics including interleaved user edits (v0.2 clarification 2), the
//! stream anchor mapping through concurrent edits, and error behavior.

use oxidown_core::{EditOrigin, Editor, Splice};

fn utf16_to_byte_str(s: &str, cu: usize) -> usize {
    let mut count = 0;
    for (bi, ch) in s.char_indices() {
        if count == cu {
            return bi;
        }
        count += ch.len_utf16();
    }
    s.len()
}

fn apply_to_mirror(mirror: &mut String, splices: &[Splice]) {
    for s in splices.iter().rev() {
        let from = utf16_to_byte_str(mirror, s.at);
        let to = utf16_to_byte_str(mirror, s.at + s.delete);
        mirror.replace_range(from..to, &s.insert);
    }
}

/// Append and verify the returned CoreChange splices reproduce the core's
/// state on a view mirror.
fn append(ed: &mut Editor, mirror: &mut String, id: u64, chunk: &str) {
    let change = ed.stream_append(id, chunk).unwrap();
    apply_to_mirror(mirror, &change.splices);
    assert_eq!(*mirror, ed.get_text(), "stream splices valid on view buffer");
    assert_eq!(change.revision, ed.revision());
    assert!(change.selection.is_none(), "streams never move the user cursor");
}

#[test]
fn basic_open_append_close() {
    let mut ed = Editor::new(1);
    ed.load("before  after");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(7).unwrap();
    append(&mut ed, &mut mirror, id, "one");
    append(&mut ed, &mut mirror, id, " two");
    append(&mut ed, &mut mirror, id, " three");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "before one two three after");
}

#[test]
fn stream_ops_carry_ai_origin() {
    let mut ed = Editor::new(1);
    ed.load("x");
    let id = ed.stream_open(1).unwrap();
    ed.stream_append(id, "abc").unwrap();
    ed.stream_close(id);
    let ops = ed.oplog().ops();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].origin, EditOrigin::Ai);
}

#[test]
fn whole_stream_is_one_undo_unit() {
    let mut ed = Editor::new(1);
    ed.load("doc\n");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(4).unwrap();
    for chunk in ["# streamed\n", "\nline one\n", "line two\n"] {
        append(&mut ed, &mut mirror, id, chunk);
    }
    ed.stream_close(id);
    assert_eq!(ed.history_depths().0, 1, "3 appends, 1 unit");

    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "doc\n", "one undo reverts the whole stream");
    assert_eq!(mirror, ed.get_text());

    let r = ed.redo().unwrap();
    apply_to_mirror(&mut mirror, &r.splices);
    assert_eq!(ed.get_text(), "doc\n# streamed\n\nline one\nline two\n");
    assert_eq!(mirror, ed.get_text());
}

#[test]
fn empty_chunk_is_noop() {
    let mut ed = Editor::new(1);
    ed.load("x");
    let rev = ed.revision();
    let id = ed.stream_open(0).unwrap();
    let change = ed.stream_append(id, "").unwrap();
    assert!(change.splices.is_empty());
    assert_eq!(change.revision, rev, "no-op append must not burn a revision");
    assert_eq!(ed.history_depths().0, 0);
}

#[test]
fn append_on_unknown_or_closed_stream_throws() {
    let mut ed = Editor::new(1);
    ed.load("x");
    let err = ed.stream_append(42, "chunk").unwrap_err();
    assert_eq!(err.name(), "UnknownStream");

    let id = ed.stream_open(0).unwrap();
    ed.stream_append(id, "ok").unwrap();
    ed.stream_close(id);
    let err = ed.stream_append(id, "late").unwrap_err();
    assert_eq!(err.name(), "UnknownStream");
}

#[test]
fn close_is_noop_on_unknown_and_idempotent() {
    let mut ed = Editor::new(1);
    ed.load("x");
    ed.stream_close(999); // never opened: no-op
    let id = ed.stream_open(0).unwrap();
    ed.stream_close(id);
    ed.stream_close(id); // double close: no-op
}

#[test]
fn stream_open_validates_position() {
    let mut ed = Editor::new(1);
    ed.load("😀ab");
    assert_eq!(ed.stream_open(99).unwrap_err().name(), "OutOfBounds");
    assert_eq!(
        ed.stream_open(1).unwrap_err().name(),
        "SurrogateSplit",
        "an insertion point inside a surrogate pair would corrupt text"
    );
}

// --------------------------------------------------- concurrent editing --

#[test]
fn user_edit_above_stream_point_shifts_subsequent_appends() {
    let mut ed = Editor::new(1);
    ed.load("HEAD tail");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(5).unwrap();
    append(&mut ed, &mut mirror, id, "one ");
    // User types at the very top while the stream is open.
    let batch = vec![Splice { at: 0, delete: 0, insert: ">> ".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 0.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    // The stream continues at its mapped insertion point.
    append(&mut ed, &mut mirror, id, "two ");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), ">> HEAD one two tail");
}

#[test]
fn user_edit_below_stream_point_does_not_disturb_appends() {
    let mut ed = Editor::new(1);
    ed.load("head TAIL");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(5).unwrap();
    append(&mut ed, &mut mirror, id, "one ");
    // User types after the streamed region.
    let end = ed.doc_len_utf16();
    let batch = vec![Splice { at: end, delete: 0, insert: "!".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 0.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    append(&mut ed, &mut mirror, id, "two ");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "head one two TAIL!");
}

#[test]
fn interleaved_user_edit_keeps_its_own_undo_unit() {
    // Stream chunks before AND after a user edit still form ONE stream unit;
    // the user edit is its own unit; undo order is stack order: user edit
    // first, then the whole stream.
    let mut ed = Editor::new(1);
    ed.load("base ");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(5).unwrap();
    append(&mut ed, &mut mirror, id, "one ");
    // User edit at the very top (above the streamed region).
    let batch = vec![Splice { at: 0, delete: 0, insert: "# ".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 0.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    append(&mut ed, &mut mirror, id, "two");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "# base one two");
    assert_eq!(ed.history_depths().0, 2, "stream unit + user unit");

    // Undo 1: the user edit (most recent unit), stream text untouched.
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "base one two");
    assert_eq!(mirror, ed.get_text());

    // Undo 2: the entire stream in one step.
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "base ");
    assert_eq!(mirror, ed.get_text());
    assert!(ed.undo().is_none());
}

#[test]
fn user_edit_inside_streamed_region_undo_stays_sound() {
    // Editing inside the already-streamed region is not blocked in M1. The
    // stream's unit must still revert exactly the streamed spans (mapped),
    // and the user's edit must revert independently.
    let mut ed = Editor::new(1);
    ed.load("base ");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(5).unwrap();
    append(&mut ed, &mut mirror, id, "one ");
    assert_eq!(ed.get_text(), "base one ");
    // User inserts inside the streamed text ("on|e ").
    let batch = vec![Splice { at: 7, delete: 0, insert: "<U>".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 0.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    assert_eq!(ed.get_text(), "base on<U>e ");
    // Stream continues (its anchor sat at the end: unaffected position-wise
    // beyond mapping).
    append(&mut ed, &mut mirror, id, "two");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "base on<U>e two");
    assert_eq!(ed.history_depths().0, 2);

    // Undo 1: user insert goes; streamed text intact.
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "base one two");
    assert_eq!(mirror, ed.get_text());

    // Undo 2: exactly the streamed spans go.
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "base ");
    assert_eq!(mirror, ed.get_text());
}

#[test]
fn user_deletes_part_of_streamed_text_then_stream_continues() {
    // The nastiest interleaving: the user deletes a slice OF the streamed
    // text mid-stream. The stream unit must never try to delete bytes it no
    // longer owns; undo order stays sound.
    let mut ed = Editor::new(1);
    ed.load("");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(0).unwrap();
    append(&mut ed, &mut mirror, id, "abcdef");
    // User deletes "cd" (inside the streamed region).
    let batch = vec![Splice { at: 2, delete: 2, insert: String::new() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 0.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    assert_eq!(ed.get_text(), "abef");
    append(&mut ed, &mut mirror, id, "gh");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "abefgh");

    // Undo 1: user deletion restored.
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "abcdefgh");
    assert_eq!(mirror, ed.get_text());

    // Undo 2: all streamed text (both chunks) goes in one step.
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "");
    assert_eq!(mirror, ed.get_text());

    // Redo everything back: the stream unit first (ALL streamed bytes —
    // "abcdef" and "gh" — restored in one step), then the user deletion.
    let r = ed.redo().unwrap();
    apply_to_mirror(&mut mirror, &r.splices);
    assert_eq!(ed.get_text(), "abcdefgh");
    let r = ed.redo().unwrap();
    apply_to_mirror(&mut mirror, &r.splices);
    assert_eq!(ed.get_text(), "abefgh");
    assert_eq!(mirror, ed.get_text());
}

/// Regression (CRITICAL, found by review probe): the stream-append cascade
/// used to translate the chunk position through an above unit's inverse
/// with plain `map_pos`, which collapses a position strictly inside a
/// delete-span to the span START — but the cascade's batch rewrite splits
/// such a span placing the RESTORE TEXT before the surviving chunk, so the
/// chunk's true deeper-frame position is start + restore-text length.
/// Whenever a user REPLACEMENT (delete+insert in one splice) covered the
/// stream's insertion point, the stream unit's merged delete landed short
/// by the restore text's length: this exact script used to undo to
/// "a WXONbeta" instead of "alpha beta".
#[test]
fn stacked_user_replacements_above_stream_unit_round_trip() {
    let mut ed = Editor::new(1);
    ed.load("alpha beta");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(6).unwrap();
    append(&mut ed, &mut mirror, id, "WX");
    assert_eq!(ed.get_text(), "alpha WXbeta");

    // User replacement 1: [0,8) ("alpha WX" — original text AND streamed
    // text together) becomes "zz ".
    let batch = vec![Splice { at: 0, delete: 8, insert: "zz ".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 0.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    assert_eq!(ed.get_text(), "zz beta");

    // User replacement 2, stacked above the first: [1,6) becomes "e".
    let batch = vec![Splice { at: 1, delete: 5, insert: "e".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 10_000.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    assert_eq!(ed.get_text(), "zea");

    // The stream keeps flowing: this append cascades through BOTH
    // replacement units (and lands strictly inside their inverse spans).
    append(&mut ed, &mut mirror, id, "ONE ");
    ed.stream_close(id);
    let final_text = ed.get_text();
    assert_eq!(ed.history_depths().0, 3, "stream unit + two user units");

    // Full undo restores the EXACT original document...
    let mut undo_texts = Vec::new();
    for _ in 0..3 {
        let u = ed.undo().unwrap();
        apply_to_mirror(&mut mirror, &u.splices);
        assert_eq!(mirror, ed.get_text(), "undo splices valid on view buffer");
        undo_texts.push(ed.get_text());
    }
    assert_eq!(ed.get_text(), "alpha beta");
    assert!(ed.undo().is_none());

    // ...and full redo the exact final one, retracing every intermediate.
    for expect in undo_texts.iter().rev().skip(1).chain([&final_text]) {
        let r = ed.redo().unwrap();
        apply_to_mirror(&mut mirror, &r.splices);
        assert_eq!(mirror, ed.get_text(), "redo splices valid on view buffer");
        assert_eq!(&ed.get_text(), expect);
    }
    assert!(ed.redo().is_none());
}

/// Multibyte variant of the cascade regression above: with 2-byte and
/// 4-byte chars at the same offsets, the mis-translated cascade position
/// landed the stream unit's merged delete mid-character, and `undo`
/// PANICKED inside ropey (byte_slice char-boundary) across the boundary
/// instead of merely corrupting text.
#[test]
fn stacked_user_replacements_above_stream_unit_round_trip_multibyte() {
    let mut ed = Editor::new(1);
    ed.load("αλφα βήτα"); // 9 chars, 2 bytes each; UTF-16 offsets 0..9
    let original = ed.get_text();
    let mut mirror = ed.get_text();
    let id = ed.stream_open(5).unwrap(); // after "αλφα "
    append(&mut ed, &mut mirror, id, "ŵ😀");
    // User replacement 1 covers the streamed text: [0,8) -> "žž ".
    let batch = vec![Splice { at: 0, delete: 8, insert: "žž ".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 0.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    // User replacement 2, stacked above: [1,5) -> "é".
    let batch = vec![Splice { at: 1, delete: 4, insert: "é".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 10_000.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    // Stream continues through both replacement inverses.
    append(&mut ed, &mut mirror, id, "ÖNÉ 😀");
    ed.stream_close(id);
    let final_text = ed.get_text();
    assert_eq!(ed.history_depths().0, 3);

    for _ in 0..3 {
        let u = ed.undo().unwrap(); // pre-fix: ropey char-boundary panic here
        apply_to_mirror(&mut mirror, &u.splices);
        assert_eq!(mirror, ed.get_text());
    }
    assert_eq!(ed.get_text(), original);
    for _ in 0..3 {
        let r = ed.redo().unwrap();
        apply_to_mirror(&mut mirror, &r.splices);
        assert_eq!(mirror, ed.get_text());
    }
    assert_eq!(ed.get_text(), final_text);
}

/// A replacement whose span strictly CONTAINS the stream's insertion point
/// mid-stream, twice over (two stacked user units above the stream unit):
/// the next append must cascade to the right spot in every deeper frame and
/// the whole history must round-trip exactly.
#[test]
fn replacements_containing_the_insertion_point_mid_stream_round_trip() {
    let mut ed = Editor::new(1);
    ed.load("hello world");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(5).unwrap(); // after "hello"
    append(&mut ed, &mut mirror, id, " S1");
    assert_eq!(ed.get_text(), "hello S1 world");

    // Replacement spanning across the insertion anchor (which sits after
    // "S1"): [3,10) -> "-R1-".
    let batch = vec![Splice { at: 3, delete: 7, insert: "-R1-".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 0.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    // A second overlapping replacement stacked above: [2,7) -> "X".
    let batch = vec![Splice { at: 2, delete: 5, insert: "X".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 10_000.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);

    append(&mut ed, &mut mirror, id, " S2");
    ed.stream_close(id);
    let final_text = ed.get_text();
    assert_eq!(ed.history_depths().0, 3, "stream unit + two user units");

    for _ in 0..3 {
        let u = ed.undo().unwrap();
        apply_to_mirror(&mut mirror, &u.splices);
        assert_eq!(mirror, ed.get_text());
    }
    assert_eq!(ed.get_text(), "hello world", "full undo restores the original");
    for _ in 0..3 {
        let r = ed.redo().unwrap();
        apply_to_mirror(&mut mirror, &r.splices);
        assert_eq!(mirror, ed.get_text());
    }
    assert_eq!(ed.get_text(), final_text, "full redo restores the final text");
}

#[test]
fn two_concurrent_streams_have_independent_units() {
    let mut ed = Editor::new(1);
    ed.load("A  B ");
    let mut mirror = ed.get_text();
    let s1 = ed.stream_open(2).unwrap();
    let s2 = ed.stream_open(5).unwrap();
    append(&mut ed, &mut mirror, s1, "one");
    append(&mut ed, &mut mirror, s2, "TWO");
    append(&mut ed, &mut mirror, s1, "!");
    ed.stream_close(s1);
    ed.stream_close(s2);
    assert_eq!(ed.get_text(), "A one! B TWO");
    assert_eq!(ed.history_depths().0, 2, "one unit per stream");

    // Undo s2's stream (most recent unit is s2's? Unit order is creation
    // order: s1's unit was pushed first, s2's second; s1's later append
    // merged DOWN into s1's existing unit). So undo pops s2 first.
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "A one! B ");
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "A  B ");
    assert_eq!(mirror, ed.get_text());
}

#[test]
fn stream_decorations_stay_coherent_with_fast_path() {
    // Appends into the tail block use the fast path; decorations computed
    // afterwards must match a from-scratch parse of the same document.
    let mut ed = Editor::new(1);
    ed.load("# head\n\npara\n");
    let id = ed.stream_open(13).unwrap();
    for chunk in ["stream **bold*", "* and `code", "` here\n\n- [ ] task\n"] {
        ed.stream_append(id, chunk).unwrap();
    }
    ed.stream_close(id);

    let text = ed.get_text();
    let fast = ed
        .decorations(ed.revision(), 0, ed.doc_len_utf16(), &[])
        .unwrap();
    let mut fresh = Editor::new(2);
    let rev = fresh.load(&text);
    let full = fresh.decorations(rev, 0, fresh.doc_len_utf16(), &[]).unwrap();
    assert_eq!(fast, full, "fast-path overlay == full-reparse overlay");
}

#[test]
fn stream_block_ids_stay_sticky_across_appends() {
    let mut ed = Editor::new(1);
    ed.load("intro\n\ntail");
    let intro_id = ed.block_index().blocks()[0].id;
    let tail_id = ed.block_index().blocks()[1].id;

    let id = ed.stream_open(ed.doc_len_utf16()).unwrap();
    ed.stream_append(id, " grows").unwrap();
    assert_eq!(ed.block_index().blocks()[0].id, intro_id);
    assert_eq!(ed.block_index().blocks()[1].id, tail_id, "tail keeps id");

    // A chunk containing a block boundary splits the tail: one new id.
    ed.stream_append(id, "\n\nnew block").unwrap();
    ed.stream_close(id);
    let blocks = ed.block_index().blocks();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].id, intro_id);
    assert_eq!(blocks[1].id, tail_id);
    assert_ne!(blocks[2].id, tail_id, "split piece gets a fresh id");
}

/// Regression (found by differential fuzz): a blank-line append at the tail
/// block's start, under a List (or FootnoteDefinition) that already ends
/// with a blank line, must not leave the absorber's cached span short.
/// pulldown-cmark reports List/FootnoteDefinition spans INCLUDING trailing
/// blank lines, so the appended blank line extends the span of the block
/// ABOVE the tail slice — invisible to the standalone tail parse the fast
/// path runs. Minimized from the fuzz failure (`"\r\n"` streamed between a
/// loose-ended list and the next block left `List 0..5` cached vs `List
/// 0..7` from a full parse, a 2-byte gap before the following block).
/// Checked node-for-node (block kinds + spans, overlay) against a
/// from-scratch parse.
#[test]
fn blank_line_append_at_tail_start_under_a_list_matches_full_parse() {
    // (doc, stream-open position (UTF-16 == bytes, all-ASCII), chunk)
    let cases: &[(&str, usize, &str)] = &[
        // The minimized fuzz repro: CRLF appended right where the tail
        // block starts, list above ending in a blank line.
        ("- a\n\npara", 5, "\r\n"),
        // Same with a lone LF, and with a code fence as the tail block
        // (the original failure's shape: List then CodeBlock).
        ("- a\n\npara", 5, "\n"),
        ("- a\n\n```\ncode\n```", 5, "\r\n"),
        // Footnote definitions absorb trailing blank lines the same way.
        ("[^a]: note\n\npara", 12, "\n"),
        // Multi-line chunk whose FIRST line is blank: still hazardous.
        ("- a\n\npara", 5, "\n\nmore"),
        // Control: non-blank first line — the list cannot absorb it, so
        // the fast path may fire; either way it must match a full parse.
        ("- a\n\npara", 5, "x\n"),
    ];
    for (i, &(doc, at, chunk)) in cases.iter().enumerate() {
        let mut ed = Editor::new(1);
        ed.load(doc);
        let id = ed.stream_open(at).unwrap();
        ed.stream_append(id, chunk).unwrap();
        ed.stream_close(id);

        let text = ed.get_text();
        let expect = oxidown_core::parser::parse_document(&text);
        let got_blocks: Vec<_> = ed
            .block_index()
            .blocks()
            .iter()
            .map(|b| (b.kind, b.span.clone()))
            .collect();
        assert_eq!(
            got_blocks, expect.blocks,
            "case {i} ({doc:?} + {chunk:?}): block index diverges from full parse"
        );
        assert_eq!(
            ed.overlay_nodes(),
            expect.nodes.as_slice(),
            "case {i} ({doc:?} + {chunk:?}): overlay diverges from full parse"
        );
    }
}

#[test]
fn stream_internal_anchor_is_invisible_to_the_public_anchor_api() {
    // The stream's insertion anchor shares the id counter with public
    // anchors but is INTERNAL: resolve_anchor/drop_anchor must treat its id
    // exactly like an unknown id, so no id passed over the boundary can
    // disturb an open stream. (Pre-fix: drop_anchor removed the internal
    // anchor and the next stream_append PANICKED across the wasm boundary.)
    let mut ed = Editor::new(1);
    ed.load("hello world");
    let mut mirror = ed.get_text();
    let public = ed.create_anchor(0, oxidown_core::Bias::Before).unwrap();
    let id = ed.stream_open(5).unwrap();
    // Sweep an id range that necessarily covers the stream's internal
    // anchor id (ids are allocated from one sequential counter).
    for probe in 0..=(public + 10) {
        if probe == public {
            continue;
        }
        assert_eq!(ed.resolve_anchor(probe), None, "internal/unknown id {probe} resolves None");
        ed.drop_anchor(probe); // must be a no-op for the internal id
    }
    // The stream is unharmed: appends keep working at the right position.
    append(&mut ed, &mut mirror, id, " there");
    assert_eq!(ed.get_text(), "hello there world");
    assert_eq!(ed.resolve_anchor(public), Some(0), "public anchor untouched");
    ed.stream_close(id);
}

#[test]
fn anchors_map_through_stream_appends() {
    let mut ed = Editor::new(1);
    ed.load("before after");
    let a = ed.create_anchor(7, oxidown_core::Bias::Before).unwrap();
    let id = ed.stream_open(7).unwrap();
    ed.stream_append(id, "XXX ").unwrap();
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "before XXX after");
    // Before-bias anchor at the insertion point stays put.
    assert_eq!(ed.resolve_anchor(a), Some(7));
}

#[test]
fn undo_mid_stream_then_more_appends_starts_fresh_unit() {
    // Undoing the stream's unit while the stream is open moves it to redo;
    // a NEW append WITHOUT first redoing clears the redo stack (normal
    // "any edit clears redo" rule — the undone unit, stream tag included,
    // is gone for good) and starts a fresh unit. Correct per the contract:
    // the guarantee is one unit per stream session, not immunity from the
    // user unwinding the stream mid-flight and then diverging. Contrast
    // with `undo_then_redo_mid_stream_keeps_one_unit`, where redoing FIRST
    // resurrects the same unit and later appends keep merging into it.
    let mut ed = Editor::new(1);
    ed.load("");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(0).unwrap();
    append(&mut ed, &mut mirror, id, "one ");
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "");
    assert_eq!(ed.history_depths(), (0, 1), "stream unit moved to redo");
    append(&mut ed, &mut mirror, id, "two ");
    assert_eq!(ed.history_depths(), (1, 0), "fresh unit; redo cleared by the new append");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "two ");
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "");
}

#[test]
fn undo_then_redo_mid_stream_keeps_one_unit() {
    // The undo->redo round trip must preserve the unit's stream tag
    // (boundary v0.2: an ENTIRE stream session is exactly ONE undo unit —
    // no undo/redo carve-out): after open -> append -> undo -> redo,
    // further appends of the still-open stream merge into the SAME
    // resurrected unit, and one undo reverts everything the stream wrote.
    // (Pre-fix bug: the redo path re-pushed the unit with stream_id: None,
    // so the post-redo append started a SECOND unit and two undos were
    // needed.)
    let mut ed = Editor::new(1);
    ed.load("");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(0).unwrap();
    append(&mut ed, &mut mirror, id, "one ");

    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "");
    let r = ed.redo().unwrap();
    apply_to_mirror(&mut mirror, &r.splices);
    assert_eq!(ed.get_text(), "one ");

    append(&mut ed, &mut mirror, id, "two ");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "one two ");
    assert_eq!(
        ed.history_depths(),
        (1, 0),
        "post-redo appends merged into the resurrected stream unit"
    );

    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "", "ONE undo reverts the whole stream session");
    assert_eq!(mirror, ed.get_text());
    assert!(ed.undo().is_none());
}

#[test]
fn undo_redo_after_close_still_one_unit() {
    // Same round trip but AFTER the stream closed: the tag rides along
    // harmlessly (no more appends can arrive for a closed id — stream ids
    // are never reused), and the session stays one unit through arbitrary
    // undo/redo cycling.
    let mut ed = Editor::new(1);
    ed.load("base ");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(5).unwrap();
    append(&mut ed, &mut mirror, id, "one ");
    append(&mut ed, &mut mirror, id, "two");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "base one two");
    assert_eq!(ed.history_depths(), (1, 0));

    for _ in 0..3 {
        let u = ed.undo().unwrap();
        apply_to_mirror(&mut mirror, &u.splices);
        assert_eq!(ed.get_text(), "base ");
        assert_eq!(mirror, ed.get_text());
        assert_eq!(ed.history_depths(), (0, 1));

        let r = ed.redo().unwrap();
        apply_to_mirror(&mut mirror, &r.splices);
        assert_eq!(ed.get_text(), "base one two");
        assert_eq!(mirror, ed.get_text());
        assert_eq!(ed.history_depths(), (1, 0), "still exactly one unit");
    }
}

#[test]
fn undo_redo_round_trip_with_interleaved_user_edit_mid_stream() {
    // v0.2 clarification 2 under the round trip: a user edit made
    // mid-stream keeps its own unit in creation order (LIFO pops it before
    // the stream's unit), and undoing/redoing BOTH units mid-stream still
    // leaves the stream's unit tagged — post-redo appends merge into it,
    // and the final undo order is user edit first, then the whole stream.
    let mut ed = Editor::new(1);
    ed.load("base ");
    let mut mirror = ed.get_text();
    let id = ed.stream_open(5).unwrap();
    append(&mut ed, &mut mirror, id, "one ");
    // User edit at the top while the stream is open: its own unit, above
    // the stream's in the stack.
    let batch = vec![Splice { at: 0, delete: 0, insert: "# ".into() }];
    ed.apply_edit(ed.revision(), &batch, EditOrigin::User, 0.0).unwrap();
    apply_to_mirror(&mut mirror, &batch);
    assert_eq!(ed.get_text(), "# base one ");
    assert_eq!(ed.history_depths(), (2, 0));

    // Unwind both units, then redo both (stream unit last on undo, first
    // on redo — creation order).
    let u = ed.undo().unwrap(); // pops the user edit
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "base one ");
    let u = ed.undo().unwrap(); // pops the stream unit
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "base ");
    let r = ed.redo().unwrap(); // restores the stream unit (tag preserved)
    apply_to_mirror(&mut mirror, &r.splices);
    assert_eq!(ed.get_text(), "base one ");
    let r = ed.redo().unwrap(); // restores the user edit
    apply_to_mirror(&mut mirror, &r.splices);
    assert_eq!(ed.get_text(), "# base one ");
    assert_eq!(ed.history_depths(), (2, 0));

    // The stream keeps flowing: this append must merge into the restored
    // stream unit (below the user edit's unit), not start a third unit.
    append(&mut ed, &mut mirror, id, "two");
    ed.stream_close(id);
    assert_eq!(ed.get_text(), "# base one two");
    assert_eq!(ed.history_depths(), (2, 0), "stream unit + user unit, nothing extra");

    // LIFO: the user edit pops first, then ONE undo reverts both chunks.
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "base one two");
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "base ");
    assert_eq!(mirror, ed.get_text());
    assert!(ed.undo().is_none());
}

// ------------------------------------------------------- randomized fuzz --

/// UTF-16 offsets of every char boundary of `s` (0..=len inclusive), so
/// randomly chosen splice positions never split a surrogate pair.
fn utf16_boundaries(s: &str) -> Vec<usize> {
    let mut v = Vec::with_capacity(s.chars().count() + 1);
    let mut cu = 0;
    v.push(0);
    for ch in s.chars() {
        cu += ch.len_utf16();
        v.push(cu);
    }
    v
}

/// Seeded randomized interleaving of stream appends and single-splice user
/// edits — insertions, deletions, and REPLACEMENTS (delete+insert in one
/// splice), multibyte included, landing anywhere in the document including
/// inside the already-streamed region. After closing, a full undo must
/// restore the exact original document and a full redo the exact final one,
/// with every returned splice batch mirror-verified along the way.
///
/// Deterministic: fixed per-seed `StdRng` seeds, no clocks. Seed count
/// scales with `OXIDOWN_FUZZ_EDITS` (default 300, same knob as
/// `reparse_equivalence.rs`).
///
/// Regression coverage: the stream-append cascade's replacement-inverse
/// mis-translation (see `stacked_user_replacements_above_stream_unit_round_trip`)
/// failed 9/300 of exactly this interleaving shape before the fix — user
/// replacements over the streamed region with later appends cascading
/// through them — as text corruption or a ropey char-boundary panic.
#[test]
fn fuzz_interleaved_user_replacements_and_appends_round_trip() {
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    let seeds: u64 = std::env::var("OXIDOWN_FUZZ_EDITS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    const CHUNKS: &[&str] = &[
        "W",
        "XY",
        "ONE ",
        "chunk of text ",
        "你好",
        "é😀",
        "line\n",
        "**b**",
    ];
    const INSERTS: &[&str] = &["z", "zz ", "é", "你好", "# ", "\n", "<U>", "😀"];
    const DOCS: &[&str] = &["alpha beta", "one two three four", "héllo wörld 你好", ""];
    for seed in 0..seeds {
        let mut rng = StdRng::seed_from_u64(0x0dd5_eed0_0000 + seed);
        let base = DOCS[(seed as usize) % DOCS.len()];
        let mut ed = Editor::new(1);
        ed.load(base);
        let mut mirror = ed.get_text();
        let open_bounds = utf16_boundaries(&mirror);
        let open16 = open_bounds[rng.gen_range(0..open_bounds.len())];
        let id = ed.stream_open(open16).unwrap();
        let mut now = 0.0f64;
        for _ in 0..rng.gen_range(4..14) {
            if rng.gen_bool(0.5) {
                let chunk = CHUNKS[rng.gen_range(0..CHUNKS.len())];
                append(&mut ed, &mut mirror, id, chunk);
            } else {
                // Random single-splice user edit; most carry BOTH a delete
                // range and an insert (replacements — the shape the cascade
                // bug needed).
                let bounds = utf16_boundaries(&mirror);
                let ai = rng.gen_range(0..bounds.len());
                let bi = (ai + rng.gen_range(0..6)).min(bounds.len() - 1);
                let (at, end) = (bounds[ai], bounds[bi]);
                let insert = if rng.gen_bool(0.7) {
                    INSERTS[rng.gen_range(0..INSERTS.len())]
                } else {
                    ""
                };
                if end == at && insert.is_empty() {
                    continue;
                }
                let batch = vec![Splice { at, delete: end - at, insert: insert.into() }];
                now += 1000.0; // outside the coalesce window: one unit per edit
                ed.apply_edit(ed.revision(), &batch, EditOrigin::User, now).unwrap();
                apply_to_mirror(&mut mirror, &batch);
                assert_eq!(mirror, ed.get_text(), "seed {seed}: edit mirror agreement");
            }
        }
        ed.stream_close(id);
        let final_text = ed.get_text();

        while let Some(u) = ed.undo() {
            apply_to_mirror(&mut mirror, &u.splices);
            assert_eq!(mirror, ed.get_text(), "seed {seed}: undo splices mirror-valid");
        }
        assert_eq!(ed.get_text(), base, "seed {seed}: full undo restores the original");
        while let Some(r) = ed.redo() {
            apply_to_mirror(&mut mirror, &r.splices);
            assert_eq!(mirror, ed.get_text(), "seed {seed}: redo splices mirror-valid");
        }
        assert_eq!(
            ed.get_text(),
            final_text,
            "seed {seed}: full redo restores the final text"
        );
    }
}
