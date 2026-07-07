//! Contract semantics for the v0.2 (M1) decoration vocabulary:
//! docs/boundary-v0.md "v0.2 additions (M1)". All expected positions are
//! UTF-16 code units. Mirrors the style of `decorations_contract.rs`.

use oxidown_core::{BlockStyle, Decoration, Editor, MarkStyle, SelectionRange};

fn editor(doc: &str) -> (Editor, u64) {
    let mut ed = Editor::new(1);
    let rev = ed.load(doc);
    (ed, rev)
}

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

fn block(at: usize, style: BlockStyle) -> Decoration {
    Decoration::Block { at, style, revealed: false }
}

fn widget(from: usize, to: usize, checked: bool) -> Decoration {
    Decoration::Widget {
        from,
        to,
        kind: oxidown_core::WidgetKind::Task { checked },
    }
}

fn li(at: usize, depth: u8) -> Decoration {
    Decoration::Block {
        at,
        style: BlockStyle::ListItem(depth),
        revealed: false,
    }
}

fn li_rev(at: usize, depth: u8) -> Decoration {
    Decoration::Block {
        at,
        style: BlockStyle::ListItem(depth),
        revealed: true,
    }
}

fn bullet(from: usize, to: usize) -> Decoration {
    Decoration::Widget {
        from,
        to,
        kind: oxidown_core::WidgetKind::Bullet,
    }
}

// --------------------------------------------------------- strikethrough --

#[test]
fn strikethrough_concealed_and_revealed() {
    let d = decos("~~del~~", &[]);
    assert_eq!(
        d,
        vec![conceal(0, 2), mark(2, 5, MarkStyle::Strike), conceal(5, 7)]
    );
    let d = decos("~~del~~", &[(3, 3)]);
    assert_eq!(
        d,
        vec![
            mark(0, 2, MarkStyle::Delim),
            mark(2, 5, MarkStyle::Strike),
            mark(5, 7, MarkStyle::Delim),
        ]
    );
}

#[test]
fn strikethrough_cjk_content() {
    // "~~你好~~": delims 2 bytes/2 CU each; content 你好 = 6 bytes / 2 CU.
    let d = decos("~~你好~~", &[]);
    assert_eq!(
        d,
        vec![conceal(0, 2), mark(2, 4, MarkStyle::Strike), conceal(4, 6)]
    );
}

// ------------------------------------------------------------------ links --

#[test]
fn inline_link_concealed() {
    let d = decos("[text](http://x.com)", &[]);
    assert_eq!(
        d,
        vec![
            conceal(0, 1),
            mark(1, 5, MarkStyle::Link),
            conceal(5, 20),
        ]
    );
}

#[test]
fn inline_link_revealed_shows_url() {
    // v0.2 clarification 4: the `](url)` conceal span opens up as
    // NON-OVERLAPPING delim/url/delim pieces on reveal.
    let d = decos("[text](http://x.com)", &[(2, 2)]);
    assert_eq!(
        d,
        vec![
            mark(0, 1, MarkStyle::Delim),
            mark(1, 5, MarkStyle::Link),
            mark(5, 7, MarkStyle::Delim),  // "]("
            mark(7, 19, MarkStyle::Url),   // destination
            mark(19, 20, MarkStyle::Delim), // ")"
        ]
    );
}

#[test]
fn inline_link_with_title_revealed_keeps_title_in_delim() {
    // The ` "title"` tail is not part of the destination: it stays delim.
    let d = decos("[t](u \"ti\")", &[(1, 1)]);
    assert_eq!(
        d,
        vec![
            mark(0, 1, MarkStyle::Delim),
            mark(1, 2, MarkStyle::Link),
            mark(2, 4, MarkStyle::Delim), // "]("
            mark(4, 5, MarkStyle::Url),   // "u"
            mark(5, 11, MarkStyle::Delim), // " \"ti\")"
        ]
    );
}

#[test]
fn link_text_utf16_offsets() {
    // "[你好](url)": text is 2 CJK chars = 2 CU (6 bytes), so byte/CU spans
    // diverge but the decorator must report CU throughout.
    let d = decos("[你好](url)", &[]);
    assert_eq!(
        d,
        vec![conceal(0, 1), mark(1, 3, MarkStyle::Link), conceal(3, 9)]
    );
}

#[test]
fn autolink_always_shows_link_mark_never_conceals() {
    let d = decos("<http://x.com>", &[]);
    assert_eq!(d, vec![mark(0, 14, MarkStyle::Link)]);
    // Even with a selection touching it, there's nothing to reveal/conceal:
    // still just the one whole-span link mark.
    let d = decos("<http://x.com>", &[(3, 3)]);
    assert_eq!(d, vec![mark(0, 14, MarkStyle::Link)]);
}

#[test]
fn email_autolink_whole_span_link_mark() {
    let d = decos("<foo@example.com>", &[]);
    assert_eq!(d, vec![mark(0, 17, MarkStyle::Link)]);
}

#[test]
fn link_inside_emphasis_both_reveal_independently() {
    let doc = "*[text](url)*";
    // Cursor inside link text: only the link reveals; the outer emphasis
    // extent (0..13) also contains this cursor, so it reveals too (nodes
    // reveal independently, both happen to cover this position).
    let d = decos(doc, &[(3, 3)]);
    assert!(d.contains(&mark(0, 1, MarkStyle::Delim)), "em delim: {d:?}");
    assert!(d.contains(&mark(1, 2, MarkStyle::Delim)), "link '[': {d:?}");
}

// ------------------------------------------------------------- blockquote --

#[test]
fn blockquote_single_line() {
    let d = decos("> quote\n", &[]);
    assert_eq!(
        d,
        vec![block(0, BlockStyle::BlockQuote(1)), conceal(0, 2)]
    );
}

#[test]
fn blockquote_reveal_is_per_line() {
    let doc = "> one\n> two\n";
    // Cursor on line 1 only reveals line 1's marker.
    let d = decos(doc, &[(1, 1)]);
    assert!(d.contains(&mark(0, 2, MarkStyle::Delim)));
    assert!(d.contains(&conceal(6, 8)));
}

#[test]
fn blockquote_nested_depth_and_markers() {
    let doc = "> outer\n> > inner\n";
    let d = decos(doc, &[]);
    assert_eq!(
        d,
        vec![
            block(0, BlockStyle::BlockQuote(1)),
            conceal(0, 2),
            block(8, BlockStyle::BlockQuote(2)),
            conceal(8, 10),
            conceal(10, 12),
        ]
    );
}

#[test]
fn blockquote_cjk_marker_offsets() {
    // "> 你好\n": marker "> " is 2 bytes/2 CU either way; content 你好 after.
    let d = decos("> 你好\n", &[]);
    assert_eq!(d, vec![block(0, BlockStyle::BlockQuote(1)), conceal(0, 2)]);
}

// --------------------------------------------------------------- fences --

#[test]
fn fenced_code_block_fences_conceal_and_reveal_per_line() {
    let doc = "```rust\nfn main() {}\n```\n";
    // Concealed: fence lines keep their line style; the raw ``` + info
    // string conceal (the styled fence line reads as the block's edge).
    let d = decos(doc, &[]);
    assert_eq!(
        d,
        vec![
            block(0, BlockStyle::CodeFence),
            conceal(0, 7),
            block(8, BlockStyle::CodeBlock),
            mark(8, 20, MarkStyle::Code),
            block(21, BlockStyle::CodeFence),
            conceal(21, 24),
        ]
    );
    // BLOCK-level reveal: a cursor anywhere inside the fenced block (here
    // in the body) reveals BOTH raw fences for editing.
    let d = decos(doc, &[(10, 10)]);
    assert!(d.contains(&mark(0, 7, MarkStyle::Delim)));
    assert!(d.contains(&mark(21, 24, MarkStyle::Delim)));
    // Outside the block: both concealed.
    let d = decos(doc, &[]);
    assert!(d.contains(&conceal(0, 7)));
    assert!(d.contains(&conceal(21, 24)));
}

#[test]
fn fenced_code_multi_line_body() {
    let doc = "```\nplain\ntext\n```\n";
    let d = decos(doc, &[]);
    assert_eq!(
        d,
        vec![
            block(0, BlockStyle::CodeFence),
            conceal(0, 3),
            block(4, BlockStyle::CodeBlock),
            mark(4, 9, MarkStyle::Code),
            block(10, BlockStyle::CodeBlock),
            mark(10, 14, MarkStyle::Code),
            block(15, BlockStyle::CodeFence),
            conceal(15, 18),
        ]
    );
}

// ------------------------------------------------------------------ lists --

#[test]
fn unordered_list_marker_bullet_widget_and_adjacency_reveal() {
    // Concealed: bullets render as a widget over the whole marker span; every
    // item line carries a list-item line decoration (hanging indent).
    let d = decos("- one\n- two\n", &[]);
    assert_eq!(d, vec![li(0, 1), bullet(0, 2), li(6, 1), bullet(6, 8)]);
    // LINE-level reveal (contract v0.3, matching headings): a cursor ANYWHERE
    // on the item's line — marker, text, or line end — reveals its marker as
    // raw source and flags the line (the view drops decorative padding).
    // Other lines are untouched.
    for pos in [0, 1, 2, 4, 5] {
        let d = decos("- one\n- two\n", &[(pos, pos)]);
        assert_eq!(
            d,
            vec![
                li_rev(0, 1),
                mark(0, 2, MarkStyle::ListMarker),
                li(6, 1),
                bullet(6, 8),
            ],
            "pos {pos}"
        );
    }
    // Composition touching the marker reveals it (stability rule).
    let mut ed = Editor::new(1);
    let rev = ed.load("- one\n");
    ed.composition_begin(1, 1).unwrap();
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    assert_eq!(d[1], mark(0, 2, MarkStyle::ListMarker));
}

#[test]
fn ordered_list_marker() {
    let d = decos("1. one\n2. two\n", &[]);
    assert_eq!(
        d,
        vec![
            li(0, 1),
            mark(0, 3, MarkStyle::ListMarker),
            li(7, 1),
            mark(7, 10, MarkStyle::ListMarker),
        ]
    );
}

#[test]
fn task_item_widget_when_not_revealed() {
    let d = decos("- [ ] todo\n- [x] done\n", &[]);
    assert_eq!(
        d,
        vec![
            // Task-item markers conceal entirely (no bullet): the checkbox
            // widget alone represents the item.
            li(0, 1),
            conceal(0, 2),
            widget(2, 5, false),
            li(11, 1),
            conceal(11, 13),
            widget(13, 16, true),
        ]
    );
}

#[test]
fn task_item_widget_withheld_when_line_selected() {
    // LINE-level reveal: a cursor anywhere on the task line withholds the
    // widget in favor of raw delim text — dash and brackets in lockstep —
    // and flags the line revealed (view drops decorative padding).
    let d = decos("- [ ] todo\n", &[(0, 0)]);
    assert_eq!(
        d,
        vec![
            li_rev(0, 1),
            mark(0, 2, MarkStyle::Delim),
            mark(2, 5, MarkStyle::Delim),
        ]
    );
    // Cursor inside the checkbox glyphs themselves also withholds it.
    let d = decos("- [ ] todo\n", &[(3, 3)]);
    assert!(d.contains(&mark(2, 5, MarkStyle::Delim)));
    assert!(!d
        .iter()
        .any(|d| matches!(d, Decoration::Widget { kind: oxidown_core::WidgetKind::Task { .. }, .. })));
    // A cursor in the item's BODY TEXT reveals too (line-level, v0.3).
    let d = decos("- [ ] todo\n", &[(8, 8)]);
    assert!(d.contains(&mark(0, 2, MarkStyle::Delim)));
    assert!(d.contains(&mark(2, 5, MarkStyle::Delim)));
    // A cursor on a DIFFERENT line leaves the widget in place.
    let d = decos("- [ ] todo\nplain\n", &[(13, 13)]);
    assert!(d.contains(&widget(2, 5, false)));
    assert!(d.contains(&conceal(0, 2)));
}

#[test]
fn task_item_composition_over_checkbox_withholds_widget() {
    let mut ed = Editor::new(1);
    let rev = ed.load("- [ ] todo\n");
    ed.composition_begin(3, 3).unwrap();
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    assert!(d.contains(&mark(2, 5, MarkStyle::Delim)));
    assert!(!d
        .iter()
        .any(|d| matches!(d, Decoration::Widget { kind: oxidown_core::WidgetKind::Task { .. }, .. })));
}

#[test]
fn list_inside_blockquote_gets_marker_and_blockquote_line() {
    let doc = "> - item\n";
    let d = decos(doc, &[]);
    assert!(d.contains(&block(0, BlockStyle::BlockQuote(1))));
    assert!(d.contains(&conceal(0, 2)));
    assert!(d.contains(&bullet(2, 4)));
}

// ----------------------------------------------------------- thematic break --

#[test]
fn thematic_break_line_style_and_reveal() {
    // Concealed: the hr line style plus a conceal over the raw dashes (the
    // view draws the rule; the source collapses).
    let d = decos("a\n\n---\n\nb\n", &[]);
    assert!(d.contains(&block(3, BlockStyle::ThematicBreak)));
    assert!(d.contains(&conceal(3, 6)));
    // Cursor on the hr line reveals the dashes as delim text.
    let d = decos("a\n\n---\n\nb\n", &[(4, 4)]);
    assert!(d.contains(&block(3, BlockStyle::ThematicBreak)));
    assert!(d.contains(&mark(3, 6, MarkStyle::Delim)));
    assert!(!d.contains(&conceal(3, 6)));
}

// --------------------------------------------------------- viewport/misc --

#[test]
fn m1_constructs_never_error_on_stale_or_oob_queries() {
    let (ed, rev) = editor("~~x~~ [a](b) > q\n```\nc\n```\n- [ ] t\n---\n");
    assert_eq!(
        ed.decorations(rev + 1, 0, 5, &[]).unwrap_err().name(),
        "StaleRevision"
    );
    assert_eq!(
        ed.decorations(rev, 0, ed.doc_len_utf16() + 1, &[])
            .unwrap_err()
            .name(),
        "OutOfBounds"
    );
}

#[test]
fn viewport_filters_new_constructs() {
    let doc = "~~a~~\n\n- [ ] task\n";
    let (ed, rev) = editor(doc);
    // Viewport covering only the strike line.
    let first = ed.decorations(rev, 0, 5, &[]).unwrap();
    assert!(first.iter().any(|d| matches!(d, Decoration::Mark { style: MarkStyle::Strike, .. })));
    assert!(!first.iter().any(|d| matches!(d, Decoration::Widget { .. })));
}


#[test]
fn nested_list_items_emit_list_item_lines_with_concealed_indent() {
    // depth-2 bullet under a bullet (2-space indent), depth-3 (4 spaces),
    // and a bullet nested under an ordered item (3-space indent).
    let doc = "- a\n  - b\n    - c\n1. x\n   - y\n";
    let d = decos(doc, &[]);
    // Line decorations at the MARKER positions, for every depth (they drive
    // the view's hanging indent); indent whitespace conceals for depth >= 2.
    assert!(d.contains(&li(0, 1)), "depth 1: {d:?}");
    assert!(d.contains(&li(6, 2)), "depth 2");
    assert!(d.contains(&conceal(4, 6)), "depth-2 indent conceals");
    assert!(d.contains(&li(14, 3)), "depth 3");
    assert!(d.contains(&conceal(10, 14)), "depth-3 indent conceals");
    assert!(d.contains(&li(18, 1)), "ordered depth 1");
    assert!(d.contains(&li(26, 2)), "under ordered");
    assert!(d.contains(&conceal(23, 26)), "3-space indent conceals");
    // Cursor inside the indent reveals it as delim.
    let d = decos(doc, &[(5, 5)]);
    assert!(d.contains(&mark(4, 6, MarkStyle::Delim)));
}

#[test]
fn task_marker_reveals_in_lockstep_with_checkbox() {
    let doc = "- [ ] todo\nnext line\n";
    // Caret anywhere on the task LINE [0, 10] reveals BOTH the dash and the
    // brackets together (line-level, v0.3)...
    for pos in 0..=10 {
        let d = decos(doc, &[(pos, pos)]);
        assert!(
            d.contains(&mark(0, 2, MarkStyle::Delim)) && d.contains(&mark(2, 5, MarkStyle::Delim)),
            "pos {pos}: {d:?}"
        );
    }
    // ...and on another line, both conceal together.
    let d = decos(doc, &[(14, 14)]);
    assert!(d.contains(&conceal(0, 2)));
    assert!(d.contains(&widget(2, 5, false)));
}


#[test]
fn blockquote_reveals_when_line_selected_and_flags_line() {
    let doc = "> quoted text here\nplain\n";
    // Cursor on a DIFFERENT line: markers stay concealed, line not revealed.
    let d = decos(doc, &[(21, 21)]);
    assert!(d.contains(&block(0, BlockStyle::BlockQuote(1))));
    assert!(d.contains(&conceal(0, 2)));
    // Caret anywhere on the quote line — marker, text, or line end — shows
    // raw markers + revealed line (view drops bars/padding -> source
    // geometry). Line-level reveal, matching headings (contract v0.3).
    for pos in [0, 1, 2, 9, 18] {
        let d = decos(doc, &[(pos, pos)]);
        assert!(
            d.contains(&Decoration::Block {
                at: 0,
                style: BlockStyle::BlockQuote(1),
                revealed: true
            }),
            "pos {pos}: {d:?}"
        );
        assert!(d.contains(&mark(0, 2, MarkStyle::Delim)), "pos {pos}");
    }
}

#[test]
fn nested_quote_bullet_line_reveal() {
    // "> > - item": a caret anywhere on the line reveals EVERY marker
    // construct on it — quote run and bullet in lockstep (line-level, v0.3).
    let doc = "> > - item\nplain\n";
    for pos in [0, 3, 4, 6, 8, 10] {
        let d = decos(doc, &[(pos, pos)]);
        assert!(d.contains(&mark(0, 2, MarkStyle::Delim)), "pos {pos}: {d:?}");
        assert!(d.contains(&mark(2, 4, MarkStyle::Delim)), "pos {pos}");
        assert!(d.contains(&mark(4, 6, MarkStyle::ListMarker)), "pos {pos}");
    }
    // Caret on a different line: everything conceals.
    let d = decos(doc, &[(13, 13)]);
    assert!(d.contains(&conceal(0, 2)));
    assert!(d.contains(&conceal(2, 4)));
    assert!(d.iter().any(|x| matches!(x, Decoration::Widget { .. })));
}

#[test]
fn nested_indent_reveals_in_lockstep_with_its_marker() {
    // "  - b" (depth 2): a caret anywhere on the nested line reveals the
    // marker AND the leading indent spaces — true source geometry, no
    // invisible indent (line-level, v0.3).
    let doc = "- a\n  - b\n";
    for pos in [4, 7, 9] {
        let d = decos(doc, &[(pos, pos)]);
        assert!(d.contains(&mark(4, 6, MarkStyle::Delim)), "pos {pos}: indent revealed: {d:?}");
        assert!(d.contains(&mark(6, 8, MarkStyle::ListMarker)), "pos {pos}: marker revealed");
        assert!(d.contains(&li_rev(6, 2)), "pos {pos}: line flagged");
    }
    // Caret on the parent line: nested indent + marker concealed.
    let d = decos(doc, &[(1, 1)]);
    assert!(d.contains(&conceal(4, 6)));
}
