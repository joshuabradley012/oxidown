//! Mirror-consistency invariant (plan.md §4): apply N random valid splice
//! batches through the editor and mirror the identical splices onto a plain
//! String; `get_text()` must equal the mirror after every step, and
//! `doc_len_utf16()` must match the mirror's UTF-16 length.

use oxidown_core::{EditOrigin, Editor, Splice};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Valid UTF-16 cursor positions in `s` (char boundaries only, so never
/// inside a surrogate pair).
fn utf16_boundaries(s: &str) -> Vec<usize> {
    let mut positions = vec![0];
    let mut cu = 0;
    for ch in s.chars() {
        cu += ch.len_utf16();
        positions.push(cu);
    }
    positions
}

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

/// Apply an ascending original-coordinate splice batch to a String.
fn apply_to_mirror(mirror: &mut String, splices: &[Splice]) {
    for s in splices.iter().rev() {
        let from = utf16_to_byte_str(mirror, s.at);
        let to = utf16_to_byte_str(mirror, s.at + s.delete);
        mirror.replace_range(from..to, &s.insert);
    }
}

const INSERT_POOL: &[&str] = &[
    "a", "xyz", " ", "\n", "**", "*", "`", "#", "# ", "你好", "😀", "e\u{301}",
    "**bold**", "*em*\n", "## h\n", "``", "_", "\t", "\r\n", "日本語テキスト",
];

const SEED_DOCS: &[&str] = &[
    "",
    "# Title\n\nSome **bold** and *em* and `code`.\n",
    "你好 **世界** 😀 *emoji* text\n",
    "***nested*** __views__ `spans`\n",
];

#[test]
fn random_splices_stay_mirror_consistent() {
    for seed in 0..8u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let base = SEED_DOCS[(seed as usize) % SEED_DOCS.len()];
        let mut ed = Editor::new(1);
        let mut rev = ed.load(base);
        let mut mirror = String::from(base);
        let mut now_ms = 0.0f64;

        for step in 0..200 {
            let bounds = utf16_boundaries(&mirror);
            // Build a batch of 1..=3 ascending, non-overlapping splices.
            let batch_len = rng.gen_range(1..=3usize);
            let mut batch: Vec<Splice> = Vec::new();
            let mut cursor_idx = 0usize; // index into bounds
            for _ in 0..batch_len {
                if cursor_idx >= bounds.len() {
                    break;
                }
                let at_idx = rng.gen_range(cursor_idx..bounds.len());
                let end_idx = rng.gen_range(at_idx..bounds.len().min(at_idx + 6));
                let at = bounds[at_idx];
                let delete = bounds[end_idx] - at;
                let insert = if rng.gen_bool(0.7) {
                    INSERT_POOL[rng.gen_range(0..INSERT_POOL.len())].to_string()
                } else {
                    String::new()
                };
                batch.push(Splice { at, delete, insert });
                cursor_idx = end_idx + 1; // strictly past previous end
            }
            let origin = match rng.gen_range(0..3) {
                0 => EditOrigin::User,
                1 => EditOrigin::Ime,
                _ => EditOrigin::Paste,
            };
            now_ms += rng.gen_range(0.0..800.0);
            rev = ed
                .apply_edit(rev, &batch, origin, now_ms)
                .unwrap_or_else(|e| panic!("seed {seed} step {step}: {e} batch {batch:?}"));
            apply_to_mirror(&mut mirror, &batch);

            assert_eq!(
                ed.get_text(),
                mirror,
                "seed {seed} step {step}: text diverged after {batch:?}"
            );
            let expected_cu: usize = mirror.chars().map(char::len_utf16).sum();
            assert_eq!(ed.doc_len_utf16(), expected_cu, "seed {seed} step {step}");
            // Decorations over the whole doc must never error mid-fuzz.
            ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
        }
    }
}

#[test]
fn rejects_bad_batches_without_mutating() {
    let mut ed = Editor::new(1);
    let rev = ed.load("hello 😀 world");
    let before = ed.get_text();

    // Overlapping splices.
    let err = ed
        .apply_edit(
            rev,
            &[
                Splice { at: 0, delete: 3, insert: "x".into() },
                Splice { at: 2, delete: 1, insert: "y".into() },
            ],
            EditOrigin::User,
            0.0,
        )
        .unwrap_err();
    assert_eq!(err.name(), "InvalidSplice");

    // Out of bounds.
    let err = ed
        .apply_edit(
            rev,
            &[Splice { at: 999, delete: 0, insert: "x".into() }],
            EditOrigin::User,
            0.0,
        )
        .unwrap_err();
    assert_eq!(err.name(), "OutOfBounds");

    // Surrogate split ("😀" starts at CU 6).
    let err = ed
        .apply_edit(
            rev,
            &[Splice { at: 7, delete: 0, insert: "x".into() }],
            EditOrigin::User,
            0.0,
        )
        .unwrap_err();
    assert_eq!(err.name(), "SurrogateSplit");

    // Stale revision.
    let err = ed
        .apply_edit(
            rev + 1,
            &[Splice { at: 0, delete: 0, insert: "x".into() }],
            EditOrigin::User,
            0.0,
        )
        .unwrap_err();
    assert_eq!(err.name(), "StaleRevision");

    assert_eq!(ed.get_text(), before, "failed edits must not mutate");
    assert_eq!(ed.revision(), rev, "failed edits must not bump revision");
}
