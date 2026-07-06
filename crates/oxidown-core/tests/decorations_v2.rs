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
    Decoration::Block { at, style }
}

fn widget(from: usize, to: usize, checked: bool) -> Decoration {
    Decoration::Widget { from, to, checked }
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
fn fenced_code_block_lines_styled_never_concealed() {
    let doc = "```rust\nfn main() {}\n```\n";
    let d = decos(doc, &[]);
    assert_eq!(
        d,
        vec![
            block(0, BlockStyle::CodeFence),
            block(8, BlockStyle::CodeBlock),
            mark(8, 20, MarkStyle::Code),
            block(21, BlockStyle::CodeFence),
        ]
    );
}

#[test]
fn fenced_code_multi_line_body() {
    let doc = "```\nplain\ntext\n```\n";
    let d = decos(doc, &[]);
    assert_eq!(
        d,
        vec![
            block(0, BlockStyle::CodeFence),
            block(4, BlockStyle::CodeBlock),
            mark(4, 9, MarkStyle::Code),
            block(10, BlockStyle::CodeBlock),
            mark(10, 14, MarkStyle::Code),
            block(15, BlockStyle::CodeFence),
        ]
    );
}

// ------------------------------------------------------------------ lists --

#[test]
fn unordered_list_marker_always_visible() {
    let d = decos("- one\n- two\n", &[]);
    assert_eq!(
        d,
        vec![
            mark(0, 2, MarkStyle::ListMarker),
            mark(6, 8, MarkStyle::ListMarker),
        ]
    );
    // Even with no selection touching it at all, the marker never conceals.
}

#[test]
fn ordered_list_marker() {
    let d = decos("1. one\n2. two\n", &[]);
    assert_eq!(
        d,
        vec![
            mark(0, 3, MarkStyle::ListMarker),
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
            mark(0, 2, MarkStyle::ListMarker),
            widget(2, 5, false),
            mark(11, 13, MarkStyle::ListMarker),
            widget(13, 16, true),
        ]
    );
}

#[test]
fn task_item_widget_withheld_when_item_marker_extent_revealed() {
    // Reveal extent is the *list item's* marker extent (bullet through the
    // closing ']'), so a cursor right on the bullet (not the checkbox
    // itself) still withholds the widget in favor of raw delim text.
    let d = decos("- [ ] todo\n", &[(0, 0)]);
    assert_eq!(
        d,
        vec![
            mark(0, 2, MarkStyle::ListMarker),
            mark(2, 5, MarkStyle::Delim),
        ]
    );
    // Cursor inside the checkbox glyphs themselves also withholds it.
    let d = decos("- [ ] todo\n", &[(3, 3)]);
    assert!(d.contains(&mark(2, 5, MarkStyle::Delim)));
    assert!(!d.iter().any(|d| matches!(d, Decoration::Widget { .. })));
    // Cursor well past the item's marker extent (in the body text) leaves
    // the widget in place.
    let d = decos("- [ ] todo\n", &[(8, 8)]);
    assert!(d.contains(&widget(2, 5, false)));
}

#[test]
fn task_item_composition_over_checkbox_withholds_widget() {
    let mut ed = Editor::new(1);
    let rev = ed.load("- [ ] todo\n");
    ed.composition_begin(3, 3).unwrap();
    let d = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    assert!(d.contains(&mark(2, 5, MarkStyle::Delim)));
    assert!(!d.iter().any(|d| matches!(d, Decoration::Widget { .. })));
}

#[test]
fn list_inside_blockquote_gets_marker_and_blockquote_line() {
    let doc = "> - item\n";
    let d = decos(doc, &[]);
    assert!(d.contains(&block(0, BlockStyle::BlockQuote(1))));
    assert!(d.contains(&conceal(0, 2)));
    assert!(d.contains(&mark(2, 4, MarkStyle::ListMarker)));
}

// ----------------------------------------------------------- thematic break --

#[test]
fn thematic_break_line_style() {
    let d = decos("a\n\n---\n\nb\n", &[]);
    assert!(d.contains(&block(3, BlockStyle::ThematicBreak)));
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
