//! Composition stability rule (boundary contract, model rule 5): between
//! compositionBegin and compositionEnd, decoration output over any span
//! intersecting the composition range is stable — no new conceal spans appear
//! inside it, and conceal spans inside it are emitted as `mark:delim`.

use oxidown_core::{Decoration, EditOrigin, Editor, MarkStyle, Splice};

fn ins(at: usize, text: &str) -> Vec<Splice> {
    vec![Splice { at, delete: 0, insert: text.into() }]
}

fn conceals(d: &[Decoration]) -> Vec<(usize, usize)> {
    d.iter()
        .filter_map(|d| match d {
            Decoration::Conceal { from, to } => Some((*from, *to)),
            _ => None,
        })
        .collect()
}

fn delims(d: &[Decoration]) -> Vec<(usize, usize)> {
    d.iter()
        .filter_map(|d| match d {
            Decoration::Mark { from, to, style: MarkStyle::Delim } => Some((*from, *to)),
            _ => None,
        })
        .collect()
}

#[test]
fn conceal_spans_inside_composition_become_delim_marks() {
    let mut ed = Editor::new(1);
    let rev = ed.load("x **bold** y");
    // Concealed by default (no selections).
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    assert_eq!(conceals(&d), vec![(2, 4), (8, 10)]);

    // Composition over the whole node: both conceal spans flip to delim.
    ed.composition_begin(2, 10).unwrap();
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    assert!(conceals(&d).is_empty());
    assert_eq!(delims(&d), vec![(2, 4), (8, 10)]);

    // Narrow composition touching only the opening delimiter.
    ed.composition_begin(3, 3).unwrap();
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    assert_eq!(conceals(&d), vec![(8, 10)], "closing delim still concealed");
    assert_eq!(delims(&d), vec![(2, 4)]);

    // End of session restores concealment.
    ed.composition_end();
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    assert_eq!(conceals(&d), vec![(2, 4), (8, 10)]);
    assert!(delims(&d).is_empty());
}

#[test]
fn no_new_conceal_appears_inside_composition_when_ime_completes_a_node() {
    // Typing `*` at the end completes "*em*" — without the stability rule the
    // freshly parsed node would emit conceal spans inside the composed range.
    let mut ed = Editor::new(1);
    let mut rev = ed.load("b *em");
    ed.composition_begin(5, 5).unwrap();
    rev = ed.apply_edit(rev, &ins(5, "*"), EditOrigin::Ime, 10.0).unwrap();
    assert_eq!(ed.get_text(), "b *em*");

    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    // The em node exists (content mark emitted)...
    assert!(d.contains(&Decoration::Mark { from: 3, to: 5, style: MarkStyle::Em }));
    // ...the closing delimiter (inside the composition range) is a delim
    // mark, NOT a conceal...
    assert_eq!(delims(&d), vec![(5, 6)]);
    // ...and the only conceal is the opening delimiter, outside the range.
    assert_eq!(conceals(&d), vec![(2, 3)]);

    // After the session ends, normal concealment applies everywhere.
    ed.composition_end();
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    assert_eq!(conceals(&d), vec![(2, 3), (5, 6)]);
}

#[test]
fn composition_range_grows_with_ime_edits() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("ab");
    ed.composition_begin(2, 2).unwrap();
    // Three IME updates growing the composed text: each maps + grows the range.
    rev = ed.apply_edit(rev, &ins(2, "か"), EditOrigin::Ime, 0.0).unwrap();
    rev = ed.apply_edit(rev, &ins(3, "ん"), EditOrigin::Ime, 50.0).unwrap();
    rev = ed.apply_edit(rev, &ins(4, "じ"), EditOrigin::Ime, 100.0).unwrap();
    assert_eq!(ed.get_text(), "abかんじ");
    // Now an IME update replaces the composed run with markdown that would
    // conceal; the range must still cover it (it grew to 2..5).
    rev = ed
        .apply_edit(
            rev,
            &[Splice { at: 2, delete: 3, insert: "`c`".into() }],
            EditOrigin::Ime,
            150.0,
        )
        .unwrap();
    assert_eq!(ed.get_text(), "ab`c`");
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    assert!(conceals(&d).is_empty(), "no conceal inside composition: {d:?}");
    assert_eq!(delims(&d), vec![(2, 3), (4, 5)]);
}

#[test]
fn composition_range_survives_edits_before_it() {
    let mut ed = Editor::new(1);
    let mut rev = ed.load("xy *a");
    // Composing at the tail (the "*a" part could become emphasis).
    ed.composition_begin(3, 5).unwrap();
    // A non-IME edit lands BEFORE the composition (e.g. a collaborative-ish
    // splice or programmatic fix): the range must shift, not detach.
    rev = ed.apply_edit(rev, &ins(0, "## "), EditOrigin::Paste, 0.0).unwrap();
    assert_eq!(ed.get_text(), "## xy *a");
    // Completing the emphasis inside the (shifted) composition keeps the
    // would-be conceal spans revealed.
    rev = ed.apply_edit(rev, &ins(8, "a*"), EditOrigin::Ime, 50.0).unwrap();
    assert_eq!(ed.get_text(), "## xy *aa*");
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    // Heading delim (0..3) is far outside the composition: still concealed.
    assert_eq!(conceals(&d), vec![(0, 3)]);
    // Emphasis delims (6..7, 9..10) intersect the grown range: delim marks.
    assert_eq!(delims(&d), vec![(6, 7), (9, 10)]);
}

#[test]
fn composition_begin_validates() {
    let mut ed = Editor::new(1);
    ed.load("a😀b");
    assert_eq!(ed.composition_begin(3, 1).unwrap_err().name(), "InvalidRange");
    assert_eq!(ed.composition_begin(0, 99).unwrap_err().name(), "OutOfBounds");
    assert!(!ed.composition_active());
    // Contract v0.1: a composition range is a query range — positions inside
    // a surrogate pair snap outward (here: to cover the emoji) rather than
    // erroring.
    ed.composition_begin(2, 2).unwrap();
    assert!(ed.composition_active());
    ed.composition_end();
    ed.composition_begin(1, 3).unwrap();
    assert!(ed.composition_active());
}
