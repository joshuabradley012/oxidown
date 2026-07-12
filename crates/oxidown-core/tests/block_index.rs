//! Block index stickiness through the full `Editor` API (not just the
//! `BlockIndex` unit alone) — `apply_edit`, `undo`, and `redo` all mutate
//! text through `Editor::apply_bytes`, which is the index's only update
//! path, so this exercises that wiring end to end.

use oxidown_core::block_index::BlockKind;
use oxidown_core::{EditOrigin, Editor, Splice};

fn ins(at: usize, text: &str) -> Vec<Splice> {
    vec![Splice { at, delete: 0, insert: text.into() }]
}

#[test]
fn ids_survive_apply_edit_undo_and_redo() {
    let mut ed = Editor::new(1);
    let rev = ed.load("first\n\nsecond\n\nthird\n");
    let before: Vec<_> = ed.block_index().blocks().iter().map(|b| b.id).collect();
    assert_eq!(before.len(), 3);

    // Position 3 is inside "first" ("fir|st"), well clear of the blank-line
    // separator at 5..7 — an edit there must not disturb block boundaries.
    let rev = ed
        .apply_edit(rev, &ins(3, "!"), EditOrigin::User, 0.0)
        .unwrap();
    let after_edit: Vec<_> = ed.block_index().blocks().iter().map(|b| b.id).collect();
    assert_eq!(before, after_edit, "editing inside 'first' keeps every id");

    ed.undo().unwrap();
    let after_undo: Vec<_> = ed.block_index().blocks().iter().map(|b| b.id).collect();
    assert_eq!(before, after_undo, "undo restores the same ids");

    ed.redo().unwrap();
    let _ = rev;
    let after_redo: Vec<_> = ed.block_index().blocks().iter().map(|b| b.id).collect();
    assert_eq!(before, after_redo, "redo restores the same ids again");
}

#[test]
fn block_kinds_reflect_top_level_structure_only() {
    let mut ed = Editor::new(1);
    ed.load("# heading\n\n> quote\n\n- a\n- b\n\n```\ncode\n```\n\n---\n");
    let kinds: Vec<BlockKind> = ed.block_index().blocks().iter().map(|b| b.kind).collect();
    assert_eq!(
        kinds,
        vec![
            BlockKind::Heading,
            BlockKind::BlockQuote,
            BlockKind::List,
            BlockKind::CodeBlock,
            BlockKind::ThematicBreak,
        ]
    );
}

#[test]
fn deleting_a_block_entirely_does_not_steal_the_next_blocks_id() {
    // Regression: "bbb" fully deleted collapses to a point that used to
    // score a containment overlap of 1 — TYING the surviving "c" block's
    // real 1-byte overlap, with the tie going to the earlier (deleted)
    // block. The deleted block's ID must retire; "c" must keep its own.
    let mut ed = Editor::new(1);
    let rev = ed.load("aaa\n\nbbb\n\nc");
    let before: Vec<_> = ed.block_index().blocks().iter().map(|b| b.id).collect();
    assert_eq!(before.len(), 3);
    let bbb_id = before[1];
    let c_id = before[2];

    // Delete "bbb\n\n" (UTF-16 code units 5..10) in one splice.
    let splice = vec![Splice { at: 5, delete: 5, insert: String::new() }];
    ed.apply_edit(rev, &splice, EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.get_text(), "aaa\n\nc");

    let after: Vec<_> = ed.block_index().blocks().iter().map(|b| b.id).collect();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0], before[0], "'aaa' keeps its id");
    assert_eq!(after[1], c_id, "'c' keeps its OWN id, not the deleted block's");
    assert!(!after.contains(&bbb_id), "the deleted block's id retires");
}

#[test]
fn deleted_block_does_not_outrank_a_neighbor_rewritten_to_one_surviving_byte() {
    // Variant: the same batch deletes "bbb" entirely AND rewrites the
    // neighbor down to exactly 1 byte of surviving mapped overlap. The
    // neighbor's real 1-byte overlap must still beat the deleted block's
    // collapsed-point containment.
    let mut ed = Editor::new(1);
    let rev = ed.load("aaa\n\nbbb\n\ncc");
    let before: Vec<_> = ed.block_index().blocks().iter().map(|b| b.id).collect();
    assert_eq!(before.len(), 3);
    let bbb_id = before[1];
    let cc_id = before[2];

    // One batch: delete "bbb\n\n" (5..10) and the second "c" (11..12),
    // leaving "cc" with a single surviving byte.
    let batch = vec![
        Splice { at: 5, delete: 5, insert: String::new() },
        Splice { at: 11, delete: 1, insert: String::new() },
    ];
    ed.apply_edit(rev, &batch, EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.get_text(), "aaa\n\nc");

    let after: Vec<_> = ed.block_index().blocks().iter().map(|b| b.id).collect();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0], before[0], "'aaa' keeps its id");
    assert_eq!(after[1], cc_id, "the 1-surviving-byte neighbor keeps its own id");
    assert!(!after.contains(&bbb_id), "the deleted block's id retires");
}

#[test]
fn multiple_edits_in_a_row_keep_unrelated_blocks_stable() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("alpha\n\nbeta\n\ngamma\n");
    let alpha_id = ed.block_index().blocks()[0].id;
    let gamma_id = ed.block_index().blocks()[2].id;

    for (i, text) in ["beta", "beta2", "beta23", "beta234"].iter().enumerate() {
        let at = ed.get_text().find("beta").unwrap() as u32 as usize;
        let old_len = if i == 0 { 4 } else { ["beta", "beta2", "beta23"][i - 1].len() };
        let splice = vec![Splice {
            at,
            delete: old_len,
            insert: (*text).to_string(),
        }];
        rev = ed.apply_edit(rev, &splice, EditOrigin::User, i as f64 * 1000.0).unwrap();
        assert_eq!(ed.block_index().blocks()[0].id, alpha_id);
        assert_eq!(ed.block_index().blocks()[2].id, gamma_id);
    }
    let _ = rev;
}
