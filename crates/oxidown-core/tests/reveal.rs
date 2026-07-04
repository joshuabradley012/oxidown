//! Reveal predicate: a node is revealed iff any selection range (cursor =
//! empty range) intersects the node's full extent including delimiters,
//! boundary positions inclusive. Revealed nodes swap `conceal` for
//! `mark:delim`; nested nodes reveal independently.

use oxidown_core::{Decoration, Editor, MarkStyle, SelectionRange};

fn decos(doc: &str, selections: &[(usize, usize)]) -> Vec<Decoration> {
    let mut ed = Editor::new(1);
    let rev = ed.load(doc);
    let sels: Vec<SelectionRange> = selections
        .iter()
        .map(|&(anchor, head)| SelectionRange { anchor, head })
        .collect();
    ed.decorations(rev, 0, ed.doc_len_utf16(), &sels).unwrap()
}

fn has_conceal(d: &[Decoration], from: usize, to: usize) -> bool {
    d.contains(&Decoration::Conceal { from, to })
}

fn has_delim(d: &[Decoration], from: usize, to: usize) -> bool {
    d.contains(&Decoration::Mark {
        from,
        to,
        style: MarkStyle::Delim,
    })
}

/// "ab **bold** cd" — strong extent is CU 3..11 (delims 3..5 and 9..11).
const DOC: &str = "ab **bold** cd";

fn strong_revealed(cursor: usize) -> bool {
    let d = decos(DOC, &[(cursor, cursor)]);
    let revealed = has_delim(&d, 3, 5) && has_delim(&d, 9, 11);
    let concealed = has_conceal(&d, 3, 5) && has_conceal(&d, 9, 11);
    assert!(
        revealed ^ concealed,
        "cursor {cursor}: inconsistent delim emission: {d:?}"
    );
    revealed
}

#[test]
fn cursor_far_outside_conceals() {
    assert!(!strong_revealed(0));
    assert!(!strong_revealed(14));
}

#[test]
fn cursor_just_outside_boundaries_conceals() {
    assert!(!strong_revealed(2)); // one before opening delimiter
    assert!(!strong_revealed(12)); // one after closing delimiter
}

#[test]
fn cursor_touching_boundaries_reveals() {
    assert!(strong_revealed(3)); // immediately before opening delimiter
    assert!(strong_revealed(11)); // immediately after closing delimiter
}

#[test]
fn cursor_on_delimiter_reveals() {
    assert!(strong_revealed(4)); // inside opening **
    assert!(strong_revealed(10)); // inside closing **
}

#[test]
fn cursor_just_inside_reveals() {
    assert!(strong_revealed(5)); // right after opening delimiter
    assert!(strong_revealed(9)); // right before closing delimiter
}

#[test]
fn cursor_in_content_reveals() {
    assert!(strong_revealed(7));
}

#[test]
fn nonempty_selection_overlapping_delimiter_reveals() {
    let d = decos(DOC, &[(1, 4)]); // covers 'b', space, first '*'
    assert!(has_delim(&d, 3, 5) && has_delim(&d, 9, 11));
}

#[test]
fn nonempty_selection_outside_conceals() {
    let d = decos(DOC, &[(0, 2)]);
    assert!(has_conceal(&d, 3, 5) && has_conceal(&d, 9, 11));
}

#[test]
fn selection_covering_whole_node_reveals() {
    let d = decos(DOC, &[(0, 14)]);
    assert!(has_delim(&d, 3, 5) && has_delim(&d, 9, 11));
}

#[test]
fn any_of_multiple_selections_reveals() {
    let d = decos(DOC, &[(0, 1), (7, 7)]);
    assert!(has_delim(&d, 3, 5) && has_delim(&d, 9, 11));
}

#[test]
fn nested_nodes_reveal_independently() {
    // "**bold *italic* bold**": strong extent 0..22, em extent 7..15.
    let doc = "**bold *italic* bold**";
    // Cursor inside the italic content: BOTH nodes intersect -> both revealed.
    let d = decos(doc, &[(10, 10)]);
    assert!(has_delim(&d, 0, 2) && has_delim(&d, 20, 22), "strong revealed");
    assert!(has_delim(&d, 7, 8) && has_delim(&d, 14, 15), "em revealed");
    // Cursor in "bold " (inside strong, outside em extent 7..15 by > 1).
    let d = decos(doc, &[(4, 4)]);
    assert!(has_delim(&d, 0, 2) && has_delim(&d, 20, 22), "strong revealed");
    assert!(has_conceal(&d, 7, 8) && has_conceal(&d, 14, 15), "em concealed");
    // Cursor outside everything (no selections): both concealed.
    let d = decos(doc, &[]);
    assert!(has_conceal(&d, 0, 2) && has_conceal(&d, 20, 22));
    assert!(has_conceal(&d, 7, 8) && has_conceal(&d, 14, 15));
}

#[test]
fn heading_reveal_boundaries() {
    // "# T\nxx" — heading extent 0..3 (trailing newline excluded).
    let doc = "# T\nxx";
    // Cursor at end of the heading line: revealed.
    let d = decos(doc, &[(3, 3)]);
    assert!(has_delim(&d, 0, 2));
    // Cursor at start of the next line: concealed.
    let d = decos(doc, &[(4, 4)]);
    assert!(has_conceal(&d, 0, 2));
    // Cursor on the hashes: revealed.
    let d = decos(doc, &[(0, 0)]);
    assert!(has_delim(&d, 0, 2));
}

#[test]
fn reveal_keeps_content_marks() {
    let d = decos(DOC, &[(7, 7)]);
    assert!(d.contains(&Decoration::Mark {
        from: 5,
        to: 9,
        style: MarkStyle::Strong
    }));
}

#[test]
fn backwards_selection_is_normalized() {
    let d = decos(DOC, &[(4, 1)]); // head < anchor
    assert!(has_delim(&d, 3, 5));
}
