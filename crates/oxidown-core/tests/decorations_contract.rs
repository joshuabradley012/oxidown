//! Contract semantics for decoration emission (docs/boundary-v0.md, M0 scope)
//! on hand-written cases. All expected positions are UTF-16 code units.

use oxidown_core::{Decoration, Editor, MarkStyle, SelectionRange};

fn editor(doc: &str) -> (Editor, u64) {
    let mut ed = Editor::new(1);
    let rev = ed.load(doc);
    (ed, rev)
}

/// Decorations over the whole doc with the given selections.
fn decos(doc: &str, selections: &[(usize, usize)]) -> Vec<Decoration> {
    let (ed, rev) = editor(doc);
    let sels: Vec<SelectionRange> = selections
        .iter()
        .map(|&(anchor, head)| SelectionRange { anchor, head })
        .collect();
    ed.decorations(rev, 0, ed.doc_len_utf16(), &sels).unwrap()
}

fn mark(from: usize, to: usize, style: MarkStyle) -> Decoration {
    Decoration::Mark { from, to, style }
}

fn conceal(from: usize, to: usize) -> Decoration {
    Decoration::Conceal { from, to }
}

fn line(at: usize, level: u8) -> Decoration {
    Decoration::Line { at, level }
}

#[test]
fn headings_h1_to_h6() {
    for level in 1u8..=6 {
        let hashes = "#".repeat(level as usize);
        let doc = format!("{hashes} Title\n");
        let d = decos(&doc, &[]);
        assert_eq!(
            d,
            vec![line(0, level), conceal(0, level as usize + 1)],
            "level {level}"
        );
    }
}

#[test]
fn heading_without_space_is_not_a_heading() {
    assert!(decos("#Title\n", &[]).is_empty());
}

#[test]
fn seven_hashes_is_not_a_heading() {
    assert!(decos("####### x\n", &[]).is_empty());
}

#[test]
fn setext_heading_emits_nothing_in_m0() {
    assert!(decos("Title\n===\n", &[]).is_empty());
    assert!(decos("Title\n---\n", &[]).is_empty());
}

#[test]
fn strong_asterisk() {
    assert_eq!(
        decos("**bold**", &[]),
        vec![
            conceal(0, 2),
            mark(2, 6, MarkStyle::Strong),
            conceal(6, 8),
        ]
    );
}

#[test]
fn strong_underscore() {
    assert_eq!(
        decos("__bold__", &[]),
        vec![
            conceal(0, 2),
            mark(2, 6, MarkStyle::Strong),
            conceal(6, 8),
        ]
    );
}

#[test]
fn emphasis_both_delimiters() {
    assert_eq!(
        decos("*em*", &[]),
        vec![conceal(0, 1), mark(1, 3, MarkStyle::Em), conceal(3, 4)]
    );
    assert_eq!(
        decos("_em_", &[]),
        vec![conceal(0, 1), mark(1, 3, MarkStyle::Em), conceal(3, 4)]
    );
}

#[test]
fn inline_code() {
    assert_eq!(
        decos("`code`", &[]),
        vec![conceal(0, 1), mark(1, 5, MarkStyle::Code), conceal(5, 6)]
    );
}

#[test]
fn inline_code_multi_backtick_with_inner_backtick() {
    // ``a ` b`` — two-backtick delimiters, content "a ` b".
    assert_eq!(
        decos("``a ` b``", &[]),
        vec![conceal(0, 2), mark(2, 7, MarkStyle::Code), conceal(7, 9)]
    );
}

#[test]
fn bold_italic_triple_delimiters() {
    // ***x*** = Emphasis(0..7) wrapping Strong(1..6).
    assert_eq!(
        decos("***x***", &[]),
        vec![
            conceal(0, 1),                    // em open
            conceal(1, 3),                    // strong open
            mark(1, 6, MarkStyle::Em),        // em content = **x**
            mark(3, 4, MarkStyle::Strong),    // strong content = x
            conceal(4, 6),                    // strong close
            conceal(6, 7),                    // em close
        ]
    );
}

#[test]
fn nested_strong_with_inner_emphasis() {
    let doc = "**bold *italic* bold**";
    assert_eq!(
        decos(doc, &[]),
        vec![
            conceal(0, 2),
            mark(2, 20, MarkStyle::Strong),
            conceal(7, 8),
            mark(8, 14, MarkStyle::Em),
            conceal(14, 15),
            conceal(20, 22),
        ]
    );
}

#[test]
fn delimiters_at_line_start_and_end() {
    // Node flush at doc start and doc end, no trailing newline.
    let d = decos("*a*\ntext *b*", &[]);
    assert_eq!(
        d,
        vec![
            conceal(0, 1),
            mark(1, 2, MarkStyle::Em),
            conceal(2, 3),
            conceal(9, 10),
            mark(10, 11, MarkStyle::Em),
            conceal(11, 12),
        ]
    );
}

#[test]
fn cjk_content_utf16_offsets() {
    // "## 你好" — bytes: ## (2) + space (1) + 你好 (6) = 9;
    // UTF-16: ## (2) + space (1) + 你好 (2) = 5. Delim = 0..3 either way.
    let d = decos("## 你好\n", &[]);
    assert_eq!(d, vec![line(0, 2), conceal(0, 3)]);
    // Verify content byte/UTF-16 divergence via a strong node after CJK:
    // "你好**bold**" — bytes: 你好=6, so strong at bytes 6..14;
    // UTF-16: 你好=2, strong at 2..10, content 4..8.
    let d = decos("你好**bold**", &[]);
    assert_eq!(
        d,
        vec![
            conceal(2, 4),
            mark(4, 8, MarkStyle::Strong),
            conceal(8, 10),
        ]
    );
}

#[test]
fn emoji_content_utf16_offsets() {
    // "**😀x**": bytes 0..2 delim, content 2..7 (emoji 4 bytes + x), delim 7..9.
    // UTF-16: delim 0..2, content 2..5 (emoji = 2 CU), delim 5..7.
    assert_eq!(
        decos("**😀x**", &[]),
        vec![
            conceal(0, 2),
            mark(2, 5, MarkStyle::Strong),
            conceal(5, 7),
        ]
    );
}

#[test]
fn combining_mark_content() {
    // "*e\u{301}*": combining acute = 2 bytes / 1 CU.
    // UTF-16: conceal 0..1, em content 1..3, conceal 3..4.
    assert_eq!(
        decos("*e\u{301}*", &[]),
        vec![conceal(0, 1), mark(1, 3, MarkStyle::Em), conceal(3, 4)]
    );
}

#[test]
fn heading_with_inline_content() {
    // "# a **b**" — heading line + nested strong, both emitted.
    assert_eq!(
        decos("# a **b**\n", &[]),
        vec![
            line(0, 1),
            conceal(0, 2),
            conceal(4, 6),
            mark(6, 7, MarkStyle::Strong),
            conceal(7, 9),
        ]
    );
}

#[test]
fn viewport_filters_nodes() {
    let doc = "# one\n\n**two**\n\n*three*\n";
    // Whole doc: heading + strong + em.
    let all = decos(doc, &[]);
    assert_eq!(all.len(), 2 + 3 + 3);
    // Viewport covering only the first line.
    let (ed, rev) = editor(doc);
    let first = ed.decorations(rev, 0, 5, &[]).unwrap();
    assert_eq!(first, vec![line(0, 1), conceal(0, 2)]);
    // Viewport covering only the strong paragraph (CU 7..14).
    let mid = ed.decorations(rev, 7, 15, &[]).unwrap();
    assert_eq!(
        mid,
        vec![
            conceal(7, 9),
            mark(9, 12, MarkStyle::Strong),
            conceal(12, 14),
        ]
    );
}

#[test]
fn decorations_does_not_mutate_revision() {
    let (ed, rev) = editor("**x**");
    ed.decorations(rev, 0, 5, &[]).unwrap();
    assert_eq!(ed.revision(), rev);
}

#[test]
fn stale_revision_errors() {
    let (ed, rev) = editor("**x**");
    let err = ed.decorations(rev + 1, 0, 5, &[]).unwrap_err();
    assert_eq!(err.name(), "StaleRevision");
    let err = ed.decorations(rev - 1, 0, 5, &[]).unwrap_err();
    assert_eq!(err.name(), "StaleRevision");
}

#[test]
fn out_of_bounds_viewport_errors() {
    let (ed, rev) = editor("abc");
    assert_eq!(ed.decorations(rev, 0, 4, &[]).unwrap_err().name(), "OutOfBounds");
    assert_eq!(ed.decorations(rev, 2, 1, &[]).unwrap_err().name(), "InvalidRange");
}

#[test]
fn surrogate_split_selection_snaps_instead_of_erroring() {
    // Contract v0.1: query positions (selections/viewport) snap outward to
    // code-point boundaries; only splices reject surrogate splits.
    let (ed, rev) = editor("😀**x**");
    // anchor/head 1 is inside the emoji's surrogate pair: lo floors to 0,
    // hi ceils to 2 — a selection covering the emoji, adjacent to the strong
    // node's delimiter, which reveals it (touching counts as intersecting).
    let decos = ed
        .decorations(rev, 0, 7, &[SelectionRange { anchor: 1, head: 1 }])
        .unwrap();
    assert!(decos
        .iter()
        .any(|d| matches!(d, Decoration::Mark { style: MarkStyle::Delim, .. })));
}

/// Query positions falling inside a surrogate pair must snap outward, not
/// error — regression test for the integration-smoke SurrogateSplit failure.
#[test]
fn viewport_and_selection_positions_snap_inside_surrogate_pairs() {
    let mut ed = oxidown_core::Editor::new(1);
    // "🎉" occupies 2 UTF-16 code units at positions 0-1.
    ed.load("🎉 **bold** 🎉");
    let rev = ed.revision();
    let len = ed.doc_len_utf16();
    // Viewport edges inside the leading and trailing emoji.
    let d = ed.decorations(rev, 1, len - 1, &[]).unwrap();
    assert!(!d.is_empty(), "snapped viewport still yields decorations");
    // Selection endpoint inside a surrogate pair.
    let sel = oxidown_core::SelectionRange { anchor: 1, head: 1 };
    ed.decorations(rev, 0, len, &[sel]).unwrap();
    // Composition range inside surrogate pairs.
    ed.composition_begin(1, len - 1).unwrap();
    ed.decorations(rev, 0, len, &[]).unwrap();
    ed.composition_end();
    // Splices must STILL reject surrogate splits strictly.
    let err = ed.apply_edit(
        rev,
        &[oxidown_core::Splice { at: 1, delete: 0, insert: "x".into() }],
        oxidown_core::EditOrigin::User,
        0.0,
    );
    assert!(err.is_err(), "splice inside surrogate pair must error");
}
