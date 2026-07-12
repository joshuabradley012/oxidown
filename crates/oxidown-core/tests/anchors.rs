//! Anchor contract tests (boundary v0.2 "Anchors"): bias behavior at exact
//! insertion points, collapse-on-delete, drop/unknown resolution, and a
//! seeded property test that anchors track their character through random
//! edit scripts — plus a second, no-exclusion-zone property run (edits may
//! land ON or ACROSS the anchors, in multi-splice batches) asserting only
//! the unconditional safety invariants, and directed multi-splice
//! (command-style) batch tests.

use oxidown_core::{Bias, EditOrigin, Editor, Splice};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn ins(at: usize, text: &str) -> Vec<Splice> {
    vec![Splice { at, delete: 0, insert: text.into() }]
}

fn del(at: usize, delete: usize) -> Vec<Splice> {
    vec![Splice { at, delete, insert: String::new() }]
}

#[test]
fn create_resolve_drop_roundtrip() {
    let mut ed = Editor::new(1);
    ed.load("hello world");
    let a = ed.create_anchor(6, Bias::Before).unwrap();
    assert_eq!(ed.resolve_anchor(a), Some(6));
    ed.drop_anchor(a);
    assert_eq!(ed.resolve_anchor(a), None);
    assert_eq!(ed.resolve_anchor(12345), None, "unknown id resolves to None");
}

#[test]
fn before_bias_stays_put_at_exact_insertion_point() {
    let mut ed = Editor::new(1);
    let rev = ed.load("ab");
    let a = ed.create_anchor(1, Bias::Before).unwrap();
    ed.apply_edit(rev, &ins(1, "XY"), EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.get_text(), "aXYb");
    assert_eq!(ed.resolve_anchor(a), Some(1), "before-bias does not move");
}

#[test]
fn after_bias_moves_with_insertion_at_exact_point() {
    let mut ed = Editor::new(1);
    let rev = ed.load("ab");
    let a = ed.create_anchor(1, Bias::After).unwrap();
    ed.apply_edit(rev, &ins(1, "XY"), EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.resolve_anchor(a), Some(3), "after-bias absorbs the insertion");
}

#[test]
fn both_biases_shift_for_edits_strictly_before() {
    let mut ed = Editor::new(1);
    let rev = ed.load("abcdef");
    let before = ed.create_anchor(4, Bias::Before).unwrap();
    let after = ed.create_anchor(4, Bias::After).unwrap();
    ed.apply_edit(rev, &ins(0, "123"), EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.resolve_anchor(before), Some(7));
    assert_eq!(ed.resolve_anchor(after), Some(7));
}

#[test]
fn deleting_anchored_text_collapses_to_deletion_site_not_null() {
    let mut ed = Editor::new(1);
    let rev = ed.load("abcdefgh");
    let a = ed.create_anchor(4, Bias::Before).unwrap();
    let b = ed.create_anchor(5, Bias::After).unwrap();
    ed.apply_edit(rev, &del(2, 5), EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.get_text(), "abh");
    assert_eq!(ed.resolve_anchor(a), Some(2), "collapsed, not null (M1)");
    assert_eq!(ed.resolve_anchor(b), Some(2));
}

#[test]
fn anchors_survive_undo_and_redo() {
    let mut ed = Editor::new(1);
    let rev = ed.load("hello");
    let a = ed.create_anchor(5, Bias::Before).unwrap();
    ed.apply_edit(rev, &ins(0, ">> "), EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.resolve_anchor(a), Some(8));
    ed.undo().unwrap();
    assert_eq!(ed.resolve_anchor(a), Some(5), "undo maps anchors back");
    ed.redo().unwrap();
    assert_eq!(ed.resolve_anchor(a), Some(8), "redo maps them forward again");
}

#[test]
fn load_invalidates_all_anchors() {
    let mut ed = Editor::new(1);
    ed.load("first document");
    let a = ed.create_anchor(3, Bias::Before).unwrap();
    ed.load("second document");
    assert_eq!(ed.resolve_anchor(a), None, "anchors die with their document");
}

#[test]
fn bias_does_not_disambiguate_replacements_at_the_anchor() {
    // Pinned behavior: bias only disambiguates PURE insertions (zero-delete
    // splices) landing exactly on the anchor. A replacement splice
    // (delete > 0) starting at the anchor deletes forward from it, and the
    // anchor stays at the replacement start for BOTH biases — After does
    // not absorb the replacement text.
    let mut ed = Editor::new(1);
    let rev = ed.load("abcdefgh");
    let before = ed.create_anchor(3, Bias::Before).unwrap();
    let after = ed.create_anchor(3, Bias::After).unwrap();
    ed.apply_edit(
        rev,
        &[Splice { at: 3, delete: 2, insert: "XYZ".into() }],
        EditOrigin::User,
        0.0,
    )
    .unwrap();
    assert_eq!(ed.get_text(), "abcXYZfgh");
    assert_eq!(ed.resolve_anchor(before), Some(3));
    assert_eq!(ed.resolve_anchor(after), Some(3), "replacement is not absorbed");
}

#[test]
fn replacement_starting_exactly_at_an_after_anchor_stays_before_the_insert() {
    // Pinned contract behavior (documented in docs/boundary-v0.md): a
    // REPLACEMENT splice starting exactly at the anchor (`at == anchor`,
    // `delete > 0`, non-empty insert) leaves the anchor at the replacement
    // START — BEFORE the inserted text — for BOTH biases. Bias only ever
    // disambiguates a PURE insertion (zero-delete splice) landing exactly on
    // the anchor. This deliberately DIFFERS from CodeMirror 6's `assoc: 1`
    // mapping, which treats the replacement's insert like an insertion at
    // the position and would move the anchor to 8 (after "xyz").
    let mut ed = Editor::new(1);
    let rev = ed.load("0123456789");
    let after = ed.create_anchor(5, Bias::After).unwrap();
    let before = ed.create_anchor(5, Bias::Before).unwrap();
    ed.apply_edit(
        rev,
        &[Splice { at: 5, delete: 3, insert: "xyz".into() }],
        EditOrigin::User,
        0.0,
    )
    .unwrap();
    assert_eq!(ed.get_text(), "01234xyz89");
    assert_eq!(
        ed.resolve_anchor(after),
        Some(5),
        "after-bias stays at the replacement start, NOT past the insert (CM6 assoc:1 would say 8)"
    );
    assert_eq!(ed.resolve_anchor(before), Some(5), "before-bias likewise");
}

#[test]
fn anchor_position_inside_surrogate_pair_snaps_by_bias() {
    let mut ed = Editor::new(1);
    ed.load("😀x");
    // Position 1 is inside the emoji's surrogate pair.
    let before = ed.create_anchor(1, Bias::Before).unwrap();
    let after = ed.create_anchor(1, Bias::After).unwrap();
    assert_eq!(ed.resolve_anchor(before), Some(0), "before floors");
    assert_eq!(ed.resolve_anchor(after), Some(2), "after ceils");
}

// ------------------------------------------------------------ property --

/// UTF-16 boundary positions of `s`.
fn utf16_boundaries(s: &str) -> Vec<usize> {
    let mut v = vec![0];
    let mut cu = 0;
    for ch in s.chars() {
        cu += ch.len_utf16();
        v.push(cu);
    }
    v
}

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

/// Property: an anchor placed immediately before a sentinel character keeps
/// pointing at that sentinel through arbitrary random edits that never
/// delete it. (Checked for both biases: as long as no edit lands *exactly*
/// on the anchor — guaranteed here by never editing inside the sentinel's
/// immediate neighborhood — bias is irrelevant, which the test asserts too.)
#[test]
fn anchors_track_sentinel_through_random_edit_scripts() {
    const SENTINEL: char = '¤'; // 2 UTF-8 bytes, 1 UTF-16 code unit
    for seed in 0..8u64 {
        let mut rng = StdRng::seed_from_u64(0xA2C0 + seed);
        let base = format!("# doc\n\nsome **text** here {SENTINEL} and 你好 more\n");
        let mut ed = Editor::new(1);
        let mut rev = ed.load(&base);
        let mut mirror = base.clone();

        let sentinel_cu = |s: &str| -> usize {
            let byte = s.find(SENTINEL).expect("sentinel present");
            s[..byte].chars().map(char::len_utf16).sum()
        };
        let a_before = ed.create_anchor(sentinel_cu(&mirror), Bias::Before).unwrap();
        let a_after = ed.create_anchor(sentinel_cu(&mirror), Bias::After).unwrap();

        for step in 0..120 {
            let bounds = utf16_boundaries(&mirror);
            let s_cu = sentinel_cu(&mirror);
            // Random single splice that avoids [s_cu - 1, s_cu + 2): never
            // deletes the sentinel and never lands exactly on the anchor.
            let candidates: Vec<usize> = bounds
                .iter()
                .copied()
                .filter(|&p| p + 1 < s_cu || p > s_cu + 2)
                .collect();
            if candidates.is_empty() {
                break;
            }
            let at = candidates[rng.gen_range(0..candidates.len())];
            let max_del = if at < s_cu { s_cu - 1 - at } else { mirror.chars().map(char::len_utf16).sum::<usize>() - at };
            // Snap the delete end to a boundary not crossing the sentinel.
            let del_end_candidates: Vec<usize> = bounds
                .iter()
                .copied()
                .filter(|&e| e >= at && e - at <= max_del.min(6))
                .filter(|&e| at + 1 < s_cu && e < s_cu || at > s_cu + 2 && e >= at)
                .collect();
            let delete = if del_end_candidates.is_empty() {
                0
            } else {
                del_end_candidates[rng.gen_range(0..del_end_candidates.len())] - at
            };
            let insert = ["", "x", "**b**", "😀", "\n\n# h\n", "你好"][rng.gen_range(0..6)];
            if delete == 0 && insert.is_empty() {
                continue;
            }
            let batch = vec![Splice { at, delete, insert: insert.into() }];
            rev = ed
                .apply_edit(rev, &batch, EditOrigin::User, step as f64 * 1000.0)
                .unwrap();
            apply_to_mirror(&mut mirror, &batch);
            assert_eq!(ed.get_text(), mirror, "seed {seed} step {step}");

            let expect = sentinel_cu(&mirror);
            assert_eq!(
                ed.resolve_anchor(a_before),
                Some(expect),
                "seed {seed} step {step}: before-anchor lost the sentinel"
            );
            assert_eq!(
                ed.resolve_anchor(a_after),
                Some(expect),
                "seed {seed} step {step}: after-anchor lost the sentinel"
            );
        }
    }
}

/// Property run WITHOUT the sentinel test's exclusion zone: random 1-3
/// splice batches may delete across the anchors, land exactly on them, or
/// replace the text they sit in. Exact tracking is not assertable here (an
/// anchor whose character is deleted legitimately collapses), so this
/// asserts only the unconditional safety invariants:
///
/// 1. every live anchor keeps resolving to `Some` (collapse, never null);
/// 2. the resolved position is in bounds;
/// 3. it lands on a UTF-16 character boundary (never inside a surrogate
///    pair / astral char);
/// 4. anchors stay mutually monotone: sorted by (creation position, bias
///    with Before ≤ After), their resolved positions never cross.
#[test]
fn anchors_stay_safe_under_unrestricted_random_multi_splice_batches() {
    for seed in 0..8u64 {
        let mut rng = StdRng::seed_from_u64(0x5AFE_0000 + seed);
        let base = "# doc\n\nsome **text** here ¤ and 你好 more 😀 tail words\n";
        let mut ed = Editor::new(1);
        let mut rev = ed.load(base);
        let mut mirror = base.to_string();

        // Anchors spread across the doc (including both ends), both biases
        // at each spot, in (position, Before-then-After) order.
        let bounds0 = utf16_boundaries(&mirror);
        let len0 = *bounds0.last().unwrap();
        let mut anchors: Vec<u64> = Vec::new();
        for target in [0, len0 / 4, len0 / 2, 3 * len0 / 4, len0] {
            // Snap to the nearest boundary at/after `target`.
            let pos = *bounds0.iter().find(|&&b| b >= target).unwrap();
            anchors.push(ed.create_anchor(pos, Bias::Before).unwrap());
            anchors.push(ed.create_anchor(pos, Bias::After).unwrap());
        }

        for step in 0..120 {
            let bounds = utf16_boundaries(&mirror);
            // 1-3 ascending, non-overlapping splices — the shape command
            // planners emit — anywhere in the doc, anchors included.
            let mut batch: Vec<Splice> = Vec::new();
            let mut min_idx = 0usize;
            for _ in 0..rng.gen_range(1..=3usize) {
                if min_idx >= bounds.len() {
                    break;
                }
                let ai = rng.gen_range(min_idx..bounds.len());
                let ei = rng.gen_range(ai..=(ai + 4).min(bounds.len() - 1));
                let delete = bounds[ei] - bounds[ai];
                let insert = ["", "x", "**b**", "😀", "\n\n# h\n", "你好"][rng.gen_range(0..6)];
                if delete == 0 && insert.is_empty() {
                    continue;
                }
                batch.push(Splice { at: bounds[ai], delete, insert: insert.into() });
                min_idx = ei + 1;
            }
            if batch.is_empty() {
                continue;
            }
            rev = ed
                .apply_edit(rev, &batch, EditOrigin::User, step as f64 * 1000.0)
                .unwrap();
            apply_to_mirror(&mut mirror, &batch);
            assert_eq!(ed.get_text(), mirror, "seed {seed} step {step}");

            let bounds_after = utf16_boundaries(&mirror);
            let len_after = *bounds_after.last().unwrap();
            let mut prev = 0usize;
            for (k, &a) in anchors.iter().enumerate() {
                let r = ed
                    .resolve_anchor(a)
                    .unwrap_or_else(|| panic!("seed {seed} step {step} anchor {k}: went null"));
                assert!(
                    r <= len_after,
                    "seed {seed} step {step} anchor {k}: {r} out of bounds (len {len_after})"
                );
                assert!(
                    bounds_after.binary_search(&r).is_ok(),
                    "seed {seed} step {step} anchor {k}: {r} not on a char boundary"
                );
                assert!(
                    r >= prev,
                    "seed {seed} step {step} anchor {k}: {r} crossed previous anchor at {prev}"
                );
                prev = r;
            }
        }
    }
}

// ------------------------------------------- directed multi-splice batches --

#[test]
fn anchor_tracks_through_a_command_style_wrap_batch() {
    // The exact batch shape the inline-toggle command planners emit: two
    // pure insertions bracketing the word, applied as ONE multi-splice
    // batch. The anchor sits inside the wrapped word and must keep pointing
    // at its character.
    let mut ed = Editor::new(1);
    let rev = ed.load("one two three");
    let a = ed.create_anchor(5, Bias::Before).unwrap(); // the 'w' of "two"
    let batch = vec![
        Splice { at: 4, delete: 0, insert: "**".into() },
        Splice { at: 7, delete: 0, insert: "**".into() },
    ];
    ed.apply_edit(rev, &batch, EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.get_text(), "one **two** three");
    assert_eq!(ed.resolve_anchor(a), Some(7), "still on the 'w'");
}

#[test]
fn multi_splice_batch_deleting_across_an_anchor_collapses_it_safely() {
    // One batch with a splice before the anchor, a splice deleting the
    // whole word the anchor sits in, and a splice after it — the anchor
    // must collapse to the deletion site (both biases), never go null or
    // land out of order with its neighbors.
    let mut ed = Editor::new(1);
    let rev = ed.load("alpha beta gamma delta");
    let in_beta = ed.create_anchor(7, Bias::Before).unwrap(); // 'e' of "beta"
    let before = ed.create_anchor(13, Bias::Before).unwrap(); // 'm' of "gamma"
    let after = ed.create_anchor(13, Bias::After).unwrap();
    let in_delta = ed.create_anchor(19, Bias::Before).unwrap(); // 'l' of "delta"
    let batch = vec![
        Splice { at: 0, delete: 5, insert: "A".into() }, // "alpha" -> "A"
        Splice { at: 11, delete: 6, insert: String::new() }, // delete "gamma "
        Splice { at: 17, delete: 5, insert: "D".into() }, // "delta" -> "D"
    ];
    ed.apply_edit(rev, &batch, EditOrigin::User, 0.0).unwrap();
    assert_eq!(ed.get_text(), "A beta D");
    assert_eq!(ed.resolve_anchor(in_beta), Some(3), "'e' of beta shifted left by 4");
    assert_eq!(ed.resolve_anchor(before), Some(7), "collapsed to the deletion site");
    assert_eq!(ed.resolve_anchor(after), Some(7), "both biases collapse the same way");
    assert_eq!(ed.resolve_anchor(in_delta), Some(7), "replacement collapses to its start");
}
