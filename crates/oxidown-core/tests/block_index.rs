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
