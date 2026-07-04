//! Undo/redo and coalescing semantics.
//!
//! Every undo/redo result is also applied to a "view mirror" String using the
//! returned splices, simulating the CM6 buffer applying them verbatim — this
//! proves the splices really are in current-doc coordinates, including redo
//! entries after multiple undos.

use oxidown_core::{EditOrigin, Editor, Splice};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn utf16_to_byte_str(s: &str, cu: usize) -> usize {
    let mut count = 0;
    for (bi, ch) in s.char_indices() {
        if count == cu {
            return bi;
        }
        count += ch.len_utf16();
    }
    assert_eq!(count, cu, "utf16 offset {cu} not a boundary in mirror");
    s.len()
}

fn apply_to_mirror(mirror: &mut String, splices: &[Splice]) {
    for s in splices.iter().rev() {
        let from = utf16_to_byte_str(mirror, s.at);
        let to = utf16_to_byte_str(mirror, s.at + s.delete);
        mirror.replace_range(from..to, &s.insert);
    }
}

fn ins(at: usize, text: &str) -> Vec<Splice> {
    vec![Splice { at, delete: 0, insert: text.into() }]
}

fn del(at: usize, delete: usize) -> Vec<Splice> {
    vec![Splice { at, delete, insert: String::new() }]
}

// ---------------------------------------------------------------- basics --

#[test]
fn undo_redo_roundtrip_single_edit() {
    let mut ed = Editor::new(1);
    let rev = ed.load("hello");
    ed.apply_edit(rev, &ins(5, " world"), EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.get_text(), "hello world");

    let mut mirror = String::from("hello world");
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "hello");
    assert_eq!(mirror, "hello");
    assert_eq!(u.revision, ed.revision());

    let r = ed.redo().unwrap();
    apply_to_mirror(&mut mirror, &r.splices);
    assert_eq!(ed.get_text(), "hello world");
    assert_eq!(mirror, "hello world");
}

#[test]
fn undo_empty_stack_returns_none() {
    let mut ed = Editor::new(1);
    ed.load("x");
    assert!(ed.undo().is_none());
    assert!(ed.redo().is_none());
    assert_eq!(ed.revision(), 1, "empty undo/redo must not bump revision");
}

#[test]
fn new_edit_clears_redo() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("");
    rev = ed.apply_edit(rev, &ins(0, "a"), EditOrigin::Paste, 0.0).unwrap();
    ed.apply_edit(rev, &ins(1, "b"), EditOrigin::Paste, 1000.0).unwrap();
    let u = ed.undo().unwrap();
    assert_eq!(ed.get_text(), "a");
    assert_eq!(ed.history_depths().1, 1);
    ed.apply_edit(u.revision, &ins(1, "c"), EditOrigin::Paste, 2000.0).unwrap();
    assert_eq!(ed.history_depths().1, 0, "redo cleared by new edit");
    assert!(ed.redo().is_none());
}

#[test]
fn redo_entries_correct_after_multiple_undos() {
    // Three separate units, undo all three, then redo all three; the redo
    // splices must be valid in each intermediate doc (mirror-verified).
    let mut ed = Editor::new(1);
    let mut rev = ed.load("base ");
    let mut mirror = String::from("base ");

    let edits: [Vec<Splice>; 3] = [
        ins(5, "one "),
        ins(9, "two 😀 "),
        vec![Splice { at: 0, delete: 4, insert: "BASE".into() }],
    ];
    let mut snapshots = vec![mirror.clone()];
    for (i, batch) in edits.iter().enumerate() {
        rev = ed
            .apply_edit(rev, batch, EditOrigin::Paste, i as f64 * 10_000.0)
            .unwrap();
        apply_to_mirror(&mut mirror, batch);
        assert_eq!(ed.get_text(), mirror);
        snapshots.push(mirror.clone());
    }
    // Undo everything.
    for expect in [&snapshots[2], &snapshots[1], &snapshots[0]] {
        let u = ed.undo().unwrap();
        apply_to_mirror(&mut mirror, &u.splices);
        assert_eq!(ed.get_text(), **expect);
        assert_eq!(mirror, **expect);
    }
    assert!(ed.undo().is_none());
    // Redo everything.
    for expect in [&snapshots[1], &snapshots[2], &snapshots[3]] {
        let r = ed.redo().unwrap();
        apply_to_mirror(&mut mirror, &r.splices);
        assert_eq!(ed.get_text(), **expect);
        assert_eq!(mirror, **expect);
    }
    assert!(ed.redo().is_none());
    assert_eq!(ed.get_text(), snapshots[3]);
}

#[test]
fn multi_splice_batch_is_one_undo_unit() {
    let mut ed = Editor::new(1);
    let rev = ed.load("aa bb cc");
    let batch = vec![
        Splice { at: 0, delete: 2, insert: "XX".into() },
        Splice { at: 3, delete: 2, insert: "YY".into() },
        Splice { at: 6, delete: 2, insert: "ZZ".into() },
    ];
    ed.apply_edit(rev, &batch, EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.get_text(), "XX YY ZZ");
    assert_eq!(ed.history_depths().0, 1);
    let mut mirror = String::from("XX YY ZZ");
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "aa bb cc");
    assert_eq!(mirror, "aa bb cc");
}

// ----------------------------------------------------------- coalescing --

#[test]
fn two_quick_adjacent_inserts_coalesce() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("");
    rev = ed.apply_edit(rev, &ins(0, "a"), EditOrigin::User, 0.0).unwrap();
    ed.apply_edit(rev, &ins(1, "b"), EditOrigin::User, 100.0).unwrap();
    assert_eq!(ed.get_text(), "ab");
    assert_eq!(ed.history_depths().0, 1, "coalesced into one unit");
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "");
}

#[test]
fn ime_and_user_edits_coalesce_together() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("");
    rev = ed.apply_edit(rev, &ins(0, "a"), EditOrigin::User, 0.0).unwrap();
    ed.apply_edit(rev, &ins(1, "い"), EditOrigin::Ime, 100.0).unwrap();
    assert_eq!(ed.history_depths().0, 1);
}

#[test]
fn slow_inserts_do_not_coalesce() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("");
    rev = ed.apply_edit(rev, &ins(0, "a"), EditOrigin::User, 0.0).unwrap();
    ed.apply_edit(rev, &ins(1, "b"), EditOrigin::User, 501.0).unwrap();
    assert_eq!(ed.history_depths().0, 2, "past the 500ms window");
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "a");
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "");
}

#[test]
fn non_adjacent_inserts_do_not_coalesce() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("0123456789");
    rev = ed.apply_edit(rev, &ins(0, "a"), EditOrigin::User, 0.0).unwrap();
    ed.apply_edit(rev, &ins(9, "b"), EditOrigin::User, 100.0).unwrap();
    assert_eq!(ed.history_depths().0, 2);
}

#[test]
fn paste_between_quick_inserts_breaks_units() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("");
    rev = ed.apply_edit(rev, &ins(0, "a"), EditOrigin::User, 0.0).unwrap();
    rev = ed.apply_edit(rev, &ins(1, "PASTE"), EditOrigin::Paste, 50.0).unwrap();
    ed.apply_edit(rev, &ins(6, "b"), EditOrigin::User, 100.0).unwrap();
    assert_eq!(
        ed.history_depths().0,
        3,
        "paste never coalesces, in either direction"
    );
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "aPASTE");
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "a");
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "");
}

#[test]
fn typing_then_backspace_coalesces() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("");
    rev = ed.apply_edit(rev, &ins(0, "a"), EditOrigin::User, 0.0).unwrap();
    rev = ed.apply_edit(rev, &ins(1, "b"), EditOrigin::User, 50.0).unwrap();
    rev = ed.apply_edit(rev, &del(1, 1), EditOrigin::User, 100.0).unwrap();
    assert_eq!(ed.get_text(), "a");
    assert_eq!(ed.history_depths().0, 1, "backspace over own text coalesces");
    ed.apply_edit(rev, &ins(1, "c"), EditOrigin::User, 150.0).unwrap();
    assert_eq!(ed.get_text(), "ac");
    assert_eq!(ed.history_depths().0, 1);
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "");
}

#[test]
fn composition_pauses_coalescing() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("");
    rev = ed.apply_edit(rev, &ins(0, "a"), EditOrigin::User, 0.0).unwrap();
    ed.composition_begin(1, 1).unwrap();
    rev = ed.apply_edit(rev, &ins(1, "b"), EditOrigin::Ime, 50.0).unwrap();
    rev = ed.apply_edit(rev, &ins(2, "c"), EditOrigin::Ime, 100.0).unwrap();
    assert_eq!(
        ed.history_depths().0,
        3,
        "edits during composition each form their own unit"
    );
    ed.composition_end();
    // After the session, quick adjacent typing coalesces again (new unit).
    rev = ed.apply_edit(rev, &ins(3, "d"), EditOrigin::User, 150.0).unwrap();
    ed.apply_edit(rev, &ins(4, "e"), EditOrigin::User, 200.0).unwrap();
    assert_eq!(ed.history_depths().0, 4);
}

#[test]
fn undo_breaks_coalescing_with_surviving_unit() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("");
    rev = ed.apply_edit(rev, &ins(0, "a"), EditOrigin::User, 0.0).unwrap();
    ed.apply_edit(rev, &ins(1, "b"), EditOrigin::Paste, 10_000.0).unwrap();
    assert_eq!(ed.history_depths().0, 2);
    let u = ed.undo().unwrap(); // removes the paste, "a" unit is on top again
    rev = u.revision;
    assert_eq!(ed.get_text(), "a");
    // Quick adjacent user edit right after undo must NOT merge into the
    // surviving "a" unit.
    ed.apply_edit(rev, &ins(1, "c"), EditOrigin::User, 10_050.0).unwrap();
    assert_eq!(ed.history_depths().0, 2);
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "a", "undo removes only the post-undo typing");
}

// ------------------------------------------------------- random scripts --

#[test]
fn random_edit_scripts_undo_redo_interleaved() {
    for seed in 0..6u64 {
        let mut rng = StdRng::seed_from_u64(0xD0C0 + seed);
        let mut ed = Editor::new(1);
        let base = "# doc\n\n**seed** text 你好\n";
        let mut rev = ed.load(base);
        let mut mirror = String::from(base);
        let mut now = 0.0f64;

        // Random forward edits (widely spaced -> no coalescing surprises,
        // though coalescing correctness is itself mirror-checked).
        let n_edits = rng.gen_range(5..15);
        let mut snapshots = vec![mirror.clone()];
        for _ in 0..n_edits {
            let boundaries: Vec<usize> = {
                let mut v = vec![0];
                let mut cu = 0;
                for ch in mirror.chars() {
                    cu += ch.len_utf16();
                    v.push(cu);
                }
                v
            };
            let ai = rng.gen_range(0..boundaries.len());
            let bi = rng.gen_range(ai..boundaries.len().min(ai + 5));
            let batch = vec![Splice {
                at: boundaries[ai],
                delete: boundaries[bi] - boundaries[ai],
                insert: ["x", "**b**", "😀", "\n# h\n", ""][rng.gen_range(0..5)].into(),
            }];
            now += 1000.0;
            rev = ed.apply_edit(rev, &batch, EditOrigin::User, now).unwrap();
            apply_to_mirror(&mut mirror, &batch);
            assert_eq!(ed.get_text(), mirror);
            snapshots.push(mirror.clone());
        }

        // Undo everything -> back to base.
        let mut depth = snapshots.len() - 1;
        while let Some(u) = ed.undo() {
            apply_to_mirror(&mut mirror, &u.splices);
            depth -= 1;
            assert_eq!(ed.get_text(), mirror);
            assert_eq!(ed.get_text(), snapshots[depth]);
        }
        assert_eq!(ed.get_text(), base);

        // Redo everything -> back to final.
        while let Some(r) = ed.redo() {
            apply_to_mirror(&mut mirror, &r.splices);
            depth += 1;
            assert_eq!(ed.get_text(), mirror);
            assert_eq!(ed.get_text(), snapshots[depth]);
        }
        assert_eq!(&ed.get_text(), snapshots.last().unwrap());

        // Random undo/redo walk.
        for _ in 0..40 {
            if rng.gen_bool(0.5) {
                if let Some(u) = ed.undo() {
                    apply_to_mirror(&mut mirror, &u.splices);
                    depth -= 1;
                }
            } else if let Some(r) = ed.redo() {
                apply_to_mirror(&mut mirror, &r.splices);
                depth += 1;
            }
            assert_eq!(ed.get_text(), mirror, "seed {seed}");
            assert_eq!(ed.get_text(), snapshots[depth], "seed {seed}");
        }
    }
}

#[test]
fn oplog_records_every_edit_including_undo_redo() {
    let mut ed = Editor::new(3);
    let rev = ed.load("");
    ed.apply_edit(rev, &ins(0, "hi"), EditOrigin::User, 0.0).unwrap();
    ed.undo().unwrap();
    ed.redo().unwrap();
    let ops = ed.oplog().ops();
    assert_eq!(ops.len(), 3);
    assert_eq!(ops[0].origin, EditOrigin::User);
    assert_eq!(ops[1].origin, EditOrigin::Undo);
    assert_eq!(ops[2].origin, EditOrigin::Redo);
    assert_eq!(ops[0].id.replica, 3);
    assert!(ops[0].id.counter < ops[1].id.counter);
    assert!(ops[0].lamport < ops[1].lamport);
    assert_eq!(ops[1].parent_counter, ops[0].id.counter);
}
