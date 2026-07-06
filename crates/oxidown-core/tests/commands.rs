//! Command contract tests (boundary v0.2 "Commands"): toggles on/off/
//! partial, double-toggle byte-identity on canonical sources, setHeading,
//! toggleTask, undo-unit granularity, never-coalescing, and mirror-verified
//! splice coordinates (every returned CoreChange's splices are applied to a
//! plain String mirror exactly as the view would apply them).

use oxidown_core::{Command, CoreChange, EditOrigin, Editor, Splice};

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

/// Run a command and verify the returned splices transform the mirror into
/// the core's text (the "splices are what the VIEW needs" requirement).
fn run(ed: &mut Editor, cmd: Command) -> Option<CoreChange> {
    let mut mirror = ed.get_text();
    let change = ed.command(cmd).unwrap()?;
    apply_to_mirror(&mut mirror, &change.splices);
    assert_eq!(
        mirror,
        ed.get_text(),
        "returned splices must reproduce the core's edit on the view buffer"
    );
    assert_eq!(change.revision, ed.revision());
    Some(change)
}

// ------------------------------------------------------------ toggles --

#[test]
fn toggle_strong_on_plain_range() {
    let mut ed = Editor::new(1);
    ed.load("hello world");
    let change = run(&mut ed, Command::ToggleStrong { from: 0, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "**hello** world");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (2, 7), "selection covers the content");
}

#[test]
fn toggle_strong_off_from_inside() {
    let mut ed = Editor::new(1);
    ed.load("a **bold** b");
    // Cursor inside the bold content.
    let change = run(&mut ed, Command::ToggleStrong { from: 5, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "a bold b");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (2, 6), "selection covers what was content");
}

#[test]
fn double_toggle_is_byte_identical_on_canonical_source() {
    for (doc, cmd_on, cmd_off) in [
        (
            "plain text",
            Command::ToggleStrong { from: 0, to: 5 },
            Command::ToggleStrong { from: 3, to: 3 },
        ),
        (
            "plain text",
            Command::ToggleEm { from: 0, to: 5 },
            Command::ToggleEm { from: 3, to: 3 },
        ),
        (
            "plain text",
            Command::ToggleStrike { from: 0, to: 5 },
            Command::ToggleStrike { from: 3, to: 3 },
        ),
        (
            "plain text",
            Command::ToggleCode { from: 0, to: 5 },
            Command::ToggleCode { from: 3, to: 3 },
        ),
    ] {
        let mut ed = Editor::new(1);
        ed.load(doc);
        run(&mut ed, cmd_on).unwrap();
        run(&mut ed, cmd_off).unwrap();
        assert_eq!(ed.get_text(), doc, "on→off byte-identical for {cmd_on:?}");
    }
    // And off→on→(off) from a canonical formatted source.
    let mut ed = Editor::new(1);
    ed.load("**bold**");
    run(&mut ed, Command::ToggleStrong { from: 3, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "bold");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 4 }).unwrap();
    assert_eq!(ed.get_text(), "**bold**", "off→on restores canonical source");
}

#[test]
fn toggle_strips_noncanonical_flavor() {
    let mut ed = Editor::new(1);
    ed.load("__bold__ and _em_");
    run(&mut ed, Command::ToggleStrong { from: 3, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "bold and _em_", "underscore strong strips as-is");
    run(&mut ed, Command::ToggleEm { from: 11, to: 11 }).unwrap();
    assert_eq!(ed.get_text(), "bold and em");
}

#[test]
fn partial_overlap_extends_to_the_union() {
    let mut ed = Editor::new(1);
    ed.load("**bold** more text");
    // Selection from inside the bold through unformatted text: extends.
    run(&mut ed, Command::ToggleStrong { from: 4, to: 13 }).unwrap();
    assert_eq!(ed.get_text(), "**bold more** text");
}

#[test]
fn selection_containing_formatted_node_absorbs_it() {
    let mut ed = Editor::new(1);
    ed.load("pre **bold** post");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 17 }).unwrap();
    assert_eq!(ed.get_text(), "**pre bold post**");
}

#[test]
fn adjacent_same_kind_node_merges() {
    let mut ed = Editor::new(1);
    ed.load("**ab** cd");
    // Selection starting exactly at the strong node's end.
    run(&mut ed, Command::ToggleStrong { from: 6, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "**ab cd**", "touching counts as overlap");
}

#[test]
fn toggle_innermost_when_same_kind_nests() {
    let mut ed = Editor::new(1);
    ed.load("_a *b* c_");
    // Cursor inside "b": both emphasis nodes contain it; innermost unwraps.
    run(&mut ed, Command::ToggleEm { from: 4, to: 4 }).unwrap();
    assert_eq!(ed.get_text(), "_a b c_");
}

#[test]
fn toggle_em_strips_only_em_from_bold_italic() {
    let mut ed = Editor::new(1);
    ed.load("***x***"); // em(strong(x)) per CommonMark
    run(&mut ed, Command::ToggleEm { from: 3, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "**x**");
}

#[test]
fn empty_range_inserts_empty_pair_with_cursor_between() {
    let mut ed = Editor::new(1);
    ed.load("ab");
    let change = run(&mut ed, Command::ToggleStrong { from: 1, to: 1 }).unwrap();
    assert_eq!(ed.get_text(), "a****b");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (3, 3), "cursor between the delimiters");
}

#[test]
fn toggle_code_uses_longer_backtick_run_when_content_has_backticks() {
    let mut ed = Editor::new(1);
    ed.load("has `tick` inside");
    // Select everything: the existing code node is absorbed; its delimiters
    // strip, and since the remaining content has no backticks a single-tick
    // run suffices.
    run(&mut ed, Command::ToggleCode { from: 0, to: 17 }).unwrap();
    assert_eq!(ed.get_text(), "`has tick inside`");

    let mut ed = Editor::new(1);
    ed.load("plain with ` a stray backtick");
    run(&mut ed, Command::ToggleCode { from: 0, to: 29 }).unwrap();
    assert_eq!(ed.get_text(), "``plain with ` a stray backtick``");
}

#[test]
fn toggle_code_pads_edge_backticks() {
    let mut ed = Editor::new(1);
    ed.load("`edge");
    run(&mut ed, Command::ToggleCode { from: 0, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "`` `edge ``");
}

#[test]
fn toggle_with_cjk_and_emoji_content() {
    let mut ed = Editor::new(1);
    ed.load("你好 😀 world");
    // "你好 😀" = CU 0..5 (2 + 1 + 2).
    run(&mut ed, Command::ToggleStrong { from: 0, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "**你好 😀** world");
    run(&mut ed, Command::ToggleStrong { from: 4, to: 4 }).unwrap();
    assert_eq!(ed.get_text(), "你好 😀 world");
}

#[test]
fn toggle_range_normalized_and_validated() {
    let mut ed = Editor::new(1);
    ed.load("hello");
    // Reversed range normalizes.
    run(&mut ed, Command::ToggleStrong { from: 5, to: 0 }).unwrap();
    assert_eq!(ed.get_text(), "**hello**");
    // Out-of-bounds errors.
    let err = ed.command(Command::ToggleStrong { from: 0, to: 999 }).unwrap_err();
    assert_eq!(err.name(), "OutOfBounds");
    // Surrogate split errors (strict: commands mutate).
    let mut ed = Editor::new(1);
    ed.load("😀ab");
    let err = ed.command(Command::ToggleStrong { from: 1, to: 3 }).unwrap_err();
    assert_eq!(err.name(), "SurrogateSplit");
}

// ---------------------------------------------------------- setHeading --

#[test]
fn set_heading_on_paragraph() {
    let mut ed = Editor::new(1);
    ed.load("title line\nbody\n");
    let change = run(&mut ed, Command::SetHeading { pos: 3, level: 2 }).unwrap();
    assert_eq!(ed.get_text(), "## title line\nbody\n");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (6, 6), "cursor follows its character");
}

#[test]
fn set_heading_rewrites_level() {
    let mut ed = Editor::new(1);
    ed.load("### deep\n");
    run(&mut ed, Command::SetHeading { pos: 5, level: 1 }).unwrap();
    assert_eq!(ed.get_text(), "# deep\n");
}

#[test]
fn set_heading_zero_removes_prefix() {
    let mut ed = Editor::new(1);
    ed.load("## title\n");
    run(&mut ed, Command::SetHeading { pos: 4, level: 0 }).unwrap();
    assert_eq!(ed.get_text(), "title\n");
}

#[test]
fn set_heading_same_level_is_noop_null() {
    let mut ed = Editor::new(1);
    ed.load("## title\n");
    let rev = ed.revision();
    assert!(ed.command(Command::SetHeading { pos: 4, level: 2 }).unwrap().is_none());
    assert_eq!(ed.revision(), rev, "no-op must not burn a revision");
}

#[test]
fn set_heading_zero_on_paragraph_is_null() {
    let mut ed = Editor::new(1);
    ed.load("plain\n");
    assert!(ed.command(Command::SetHeading { pos: 2, level: 0 }).unwrap().is_none());
}

#[test]
fn set_heading_does_not_apply_inside_code_or_lists() {
    let mut ed = Editor::new(1);
    ed.load("```\ncode line\n```\n");
    assert!(ed.command(Command::SetHeading { pos: 6, level: 1 }).unwrap().is_none());

    let mut ed = Editor::new(1);
    ed.load("- item\n");
    assert!(ed.command(Command::SetHeading { pos: 3, level: 1 }).unwrap().is_none());

    let mut ed = Editor::new(1);
    ed.load("    indented code\n");
    assert!(ed.command(Command::SetHeading { pos: 8, level: 1 }).unwrap().is_none());
}

#[test]
fn set_heading_inside_blockquote_goes_after_markers() {
    let mut ed = Editor::new(1);
    ed.load("> quoted line\n");
    run(&mut ed, Command::SetHeading { pos: 5, level: 2 }).unwrap();
    assert_eq!(ed.get_text(), "> ## quoted line\n");
}

#[test]
fn set_heading_on_setext_is_null() {
    let mut ed = Editor::new(1);
    ed.load("Title\n=====\n");
    assert!(ed.command(Command::SetHeading { pos: 2, level: 3 }).unwrap().is_none());
}

#[test]
fn set_heading_invalid_level_errors() {
    let mut ed = Editor::new(1);
    ed.load("x\n");
    assert!(ed.command(Command::SetHeading { pos: 0, level: 7 }).is_err());
}

// ---------------------------------------------------------- toggleTask --

#[test]
fn toggle_task_both_directions() {
    let mut ed = Editor::new(1);
    ed.load("- [ ] todo\n- [x] done\n");
    run(&mut ed, Command::ToggleTask { pos: 7 }).unwrap();
    assert_eq!(ed.get_text(), "- [x] todo\n- [x] done\n");
    run(&mut ed, Command::ToggleTask { pos: 18 }).unwrap();
    assert_eq!(ed.get_text(), "- [x] todo\n- [ ] done\n");
}

#[test]
fn toggle_task_pos_anywhere_in_item_including_marker_and_end() {
    for pos in [0usize, 2, 4, 10] {
        let mut ed = Editor::new(1);
        ed.load("- [ ] todo\n");
        run(&mut ed, Command::ToggleTask { pos }).unwrap();
        assert_eq!(ed.get_text(), "- [x] todo\n", "pos {pos}");
    }
}

#[test]
fn toggle_task_multiline_item_second_line_still_resolves() {
    let mut ed = Editor::new(1);
    ed.load("- [ ] first line\n  continuation\n");
    // Position on the continuation line (inside the item's extent).
    run(&mut ed, Command::ToggleTask { pos: 20 }).unwrap();
    assert_eq!(ed.get_text(), "- [x] first line\n  continuation\n");
}

#[test]
fn toggle_task_capital_x_unchecks() {
    let mut ed = Editor::new(1);
    ed.load("- [X] done\n");
    run(&mut ed, Command::ToggleTask { pos: 7 }).unwrap();
    assert_eq!(ed.get_text(), "- [ ] done\n");
}

#[test]
fn toggle_task_outside_any_task_is_null() {
    let mut ed = Editor::new(1);
    ed.load("- plain item\n\nparagraph\n");
    assert!(ed.command(Command::ToggleTask { pos: 3 }).unwrap().is_none());
    assert!(ed.command(Command::ToggleTask { pos: 16 }).unwrap().is_none());
}

#[test]
fn double_toggle_task_is_byte_identical() {
    let doc = "- [ ] a\n- [x] b\n";
    let mut ed = Editor::new(1);
    ed.load(doc);
    run(&mut ed, Command::ToggleTask { pos: 3 }).unwrap();
    run(&mut ed, Command::ToggleTask { pos: 3 }).unwrap();
    assert_eq!(ed.get_text(), doc);
}

// ------------------------------------------------------ history/oplog --

#[test]
fn command_is_single_undo_unit_and_never_coalesces() {
    let mut ed = Editor::new(1);
    let rev = ed.load("hello");
    // Quick user typing, then a command, then more quick typing: three units
    // (user + command + user), even inside the 500ms window.
    let rev = ed
        .apply_edit(rev, &[Splice { at: 5, delete: 0, insert: "!".into() }], EditOrigin::User, 0.0)
        .unwrap();
    ed.command(Command::ToggleStrong { from: 0, to: 5 }).unwrap().unwrap();
    // Doc is now "**hello**!" (10 CU); type "?" at the end.
    ed.apply_edit(
        ed.revision(),
        &[Splice { at: 10, delete: 0, insert: "?".into() }],
        EditOrigin::User,
        50.0,
    )
    .unwrap();
    let _ = rev;
    assert_eq!(ed.history_depths().0, 3, "command breaks coalescing both ways");

    // Undoing the command reverts BOTH its splices in one step.
    ed.undo().unwrap(); // "?"
    ed.undo().unwrap(); // the whole toggle
    assert_eq!(ed.get_text(), "hello!");
}

#[test]
fn command_undo_redo_roundtrip_with_mirror() {
    let mut ed = Editor::new(1);
    ed.load("a **b** c");
    run(&mut ed, Command::ToggleStrong { from: 5, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "a b c");

    let mut mirror = ed.get_text();
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "a **b** c");
    assert_eq!(mirror, ed.get_text(), "undo splices valid on view buffer");

    let r = ed.redo().unwrap();
    apply_to_mirror(&mut mirror, &r.splices);
    assert_eq!(ed.get_text(), "a b c");
    assert_eq!(mirror, ed.get_text(), "redo splices valid on view buffer");
}

#[test]
fn command_ops_carry_command_origin() {
    let mut ed = Editor::new(1);
    ed.load("x");
    ed.command(Command::ToggleStrong { from: 0, to: 1 }).unwrap().unwrap();
    let ops = ed.oplog().ops();
    assert!(!ops.is_empty());
    assert!(ops.iter().all(|op| op.origin == EditOrigin::Command));
}

#[test]
fn undo_redo_supply_selection() {
    let mut ed = Editor::new(1);
    let rev = ed.load("abc");
    ed.apply_edit(rev, &[Splice { at: 3, delete: 0, insert: "XYZ".into() }], EditOrigin::User, 0.0)
        .unwrap();
    let u = ed.undo().unwrap();
    // Undo removed "XYZ": cursor at the removal site.
    assert_eq!(u.selection.map(|s| (s.anchor, s.head)), Some((3, 3)));
    let r = ed.redo().unwrap();
    // Redo restored "XYZ": cursor at the end of the restored text.
    assert_eq!(r.selection.map(|s| (s.anchor, s.head)), Some((6, 6)));
}
