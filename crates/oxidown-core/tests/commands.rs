//! Command contract tests (boundary v0.2 "Commands"): toggles on/off/
//! partial, double-toggle byte-identity on canonical sources, setHeading,
//! toggleTask, undo-unit granularity, never-coalescing, and mirror-verified
//! splice coordinates (every returned CoreChange's splices are applied to a
//! plain String mirror exactly as the view would apply them).

use oxidown_core::{Command, CoreChange, Decoration, EditOrigin, Editor, Splice, WidgetKind};

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

/// Line indices (0-based) of every line carrying its own list marker, per a
/// fresh parse — the input to the indent/outdent whole-document invariant.
fn item_line_indices(text: &str) -> Vec<usize> {
    oxidown_core::parser::parse(text)
        .iter()
        .filter(|n| matches!(n.kind, oxidown_core::parser::NodeKind::ListMarker { .. }))
        .map(|n| text[..n.extent.start].matches('\n').count())
        .collect()
}

/// Start offsets of every line carrying its own list marker. The `enter`
/// invariant is keyed by POSITION mapped through the change's splices (not
/// line index like indent/outdent's) because a continue press inserts a
/// newline, shifting every following line index by one. Offsets are byte ==
/// UTF-16 code unit for the all-ASCII enter fixtures.
fn item_line_starts(text: &str) -> Vec<usize> {
    oxidown_core::parser::parse(text)
        .iter()
        .filter(|n| matches!(n.kind, oxidown_core::parser::NodeKind::ListMarker { .. }))
        .map(|n| text[..n.extent.start].rfind('\n').map_or(0, |i| i + 1))
        .collect()
}

/// Map a position through an ascending splice batch (original -> new doc
/// coordinates, before-bias: an insertion exactly at the position leaves it
/// in place; a position inside a deleted range collapses to its start).
fn map_pos16(pos: usize, splices: &[Splice]) -> usize {
    let mut shift: isize = 0;
    for s in splices {
        let end = s.at + s.delete;
        if end < pos || (end == pos && s.delete > 0) {
            shift += s.insert.len() as isize - s.delete as isize;
        } else if s.at < pos {
            return (s.at as isize + shift) as usize; // inside the deletion
        } else {
            break;
        }
    }
    (pos as isize + shift) as usize
}

/// Whole-document itemness invariant for `enter` (mirrors indent/outdent's,
/// but position-mapped — see `item_line_starts` — and exempting the line
/// containing `from`: an EXIT press de-lists that line by design; every
/// OTHER pre-edit item line must still parse as an item afterwards).
fn assert_enter_itemness(before: &str, after: &str, splices: &[Splice], from: usize) {
    let from_line_start = before[..from.min(before.len())].rfind('\n').map_or(0, |i| i + 1);
    let after_items: std::collections::BTreeSet<usize> = item_line_starts(after).into_iter().collect();
    for start in item_line_starts(before) {
        if start == from_line_start {
            continue; // the pressed line: exit may de-list it by design
        }
        let mapped = map_pos16(start, splices);
        let mapped_line_start = after[..mapped.min(after.len())].rfind('\n').map_or(0, |i| i + 1);
        assert!(
            after_items.contains(&mapped_line_start),
            "enter itemness invariant violated: pre-edit item line at {start} \
             (mapped to line start {mapped_line_start}) is no longer a list item in {after:?}"
        );
    }
}

/// Run a command and verify the returned splices transform the mirror into
/// the core's text (the "splices are what the VIEW needs" requirement).
///
/// For indentList/outdentList/toggleTask this additionally asserts the
/// WHOLE-DOCUMENT itemness invariant — for indent/outdent it's the
/// acceptance bar for the paragraph-interruption guards (every line that
/// parsed as a list item BEFORE the command still parses as one AFTER it;
/// marker digits may differ, itemness may not); for toggleTask (v0.5
/// promotion) it's the corresponding "promotions only ADD itemness" bar —
/// no line that was already an item loses it. None of the three commands
/// add or remove PHYSICAL LINES, so line indices correspond before/after.
fn run(ed: &mut Editor, cmd: Command) -> Option<CoreChange> {
    let before_text = ed.get_text();
    let mut mirror = before_text.clone();
    let checks_itemness = matches!(
        cmd,
        Command::IndentList { .. }
            | Command::OutdentList { .. }
            | Command::ToggleTask { .. }
            | Command::ToggleQuote { .. }
            | Command::ToggleBulletList { .. }
            | Command::ToggleOrderedList { .. }
    );
    // The list toggles' STRIP mode de-lists the selected lines by explicit
    // gesture (boundary v0.6, same rationale as enter's marker-clear), so
    // intersecting lines are exempt from the invariant; every line the
    // command does NOT touch must keep its itemness (the below-line guard's
    // acceptance bar). toggleQuote has no exemption: quoting/de-quoting
    // must never cost any line its itemness.
    let exempt: Vec<usize> = match cmd {
        Command::ToggleBulletList { from, to } | Command::ToggleOrderedList { from, to } => {
            intersecting_line_index_set(&before_text, from.min(to), from.max(to))
        }
        _ => Vec::new(),
    };
    let items_before = checks_itemness.then(|| item_line_indices(&mirror));
    let change = ed.command(cmd).unwrap()?;
    apply_to_mirror(&mut mirror, &change.splices);
    assert_eq!(
        mirror,
        ed.get_text(),
        "returned splices must reproduce the core's edit on the view buffer"
    );
    assert_eq!(change.revision, ed.revision());
    if let Some(before) = items_before {
        let after = item_line_indices(&ed.get_text());
        for line in before {
            if exempt.contains(&line) {
                continue;
            }
            assert!(
                after.contains(&line),
                "whole-doc invariant violated by {cmd:?}: line {line} was a list item \
                 before the command and is not one after; result: {:?}",
                ed.get_text()
            );
        }
    }
    if let Command::Enter { from, to } = cmd {
        assert_enter_itemness(&before_text, &ed.get_text(), &change.splices, from.min(to));
    }
    Some(change)
}

/// 0-based indices of the physical lines intersecting `[from, to]`
/// (byte == UTF-16 offsets for the ASCII fixtures), CM6 semantics like the
/// core's own `intersecting_lines`: a cursor yields its containing line; a
/// non-empty range excludes a trailing line touched only at its very start.
fn intersecting_line_index_set(text: &str, from: usize, to: usize) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut line_no = 0usize;
    loop {
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'\n' && bytes[end] != b'\r' {
            end += 1;
        }
        let hit = if from == to {
            from >= start && from <= end
        } else {
            from <= end && to > start
        };
        if hit {
            out.push(line_no);
        }
        let mut next = end;
        if next < bytes.len() && bytes[next] == b'\r' {
            next += 1;
        }
        if next < bytes.len() && bytes[next] == b'\n' {
            next += 1;
        }
        if next == end {
            break; // no terminator: the document's last line
        }
        start = next;
        line_no += 1;
    }
    out
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
    // Touching counts as overlap: the selection starts exactly AT the
    // strong node's end (no whitespace between — a whitespace edge would
    // trim off first, see the flanking-trim tests below).
    let mut ed = Editor::new(1);
    ed.load("**ab**cd");
    run(&mut ed, Command::ToggleStrong { from: 6, to: 8 }).unwrap();
    assert_eq!(ed.get_text(), "**abcd**", "touching counts as overlap");

    // With a space at the selection's start, trimming moves `from` past it
    // BEFORE touch detection (delimiters may never sit against whitespace):
    // the existing node is no longer touched, so only "cd" wraps.
    let mut ed = Editor::new(1);
    ed.load("**ab** cd");
    run(&mut ed, Command::ToggleStrong { from: 6, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "**ab** **cd**", "no merge across the trimmed space");
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
fn toggle_extend_over_mixed_flavor_nested_same_kind_nodes() {
    // `_…_` and `*…*` are both Emphasis, so same-KIND nodes nest here even
    // though same-DELIMITER emphasis cannot: the ON/EXTEND batch's delimiter
    // deletes must be emitted in POSITION order, not node order (the outer
    // node's closing delimiter sits after both inner spans). Pre-fix the
    // batch was non-ascending and silently corrupted the text (`*a b c_ *`)
    // AND its undo inverse (`_a* *b c__ `).
    let mut ed = Editor::new(1);
    ed.load("_a *b* c_ x");
    run(&mut ed, Command::ToggleEm { from: 0, to: 11 }).unwrap();
    assert_eq!(ed.get_text(), "*a b c x*");

    // Undo restores the original exactly, mirror-verified.
    let mut mirror = ed.get_text();
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "_a *b* c_ x", "undo restores the original exactly");
    assert_eq!(mirror, ed.get_text(), "undo splices valid on view buffer");

    // Double-toggle on the (canonical) result is byte-identical.
    ed.redo().unwrap();
    assert_eq!(ed.get_text(), "*a b c x*");
    run(&mut ed, Command::ToggleEm { from: 3, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "a b c x");
    run(&mut ed, Command::ToggleEm { from: 0, to: 7 }).unwrap();
    assert_eq!(ed.get_text(), "*a b c x*", "canonical double-toggle is byte-identical");

    // Same shape for Strong (`__…__` outer, `**…**` inner).
    let mut ed = Editor::new(1);
    ed.load("__a **b** c__ x");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 15 }).unwrap();
    assert_eq!(ed.get_text(), "**a b c x**");
    let mut mirror = ed.get_text();
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "__a **b** c__ x", "undo restores the original exactly");
    assert_eq!(mirror, ed.get_text());
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
fn toggle_code_off_strips_the_padding_pair_on_padded_spans() {
    // ON pads edge-backtick content per CommonMark; OFF must shed the pad
    // pair too (iff the content has BOTH a leading and trailing space and
    // is not all spaces — the exact unpadding condition), or ON→OFF leaves
    // two stray spaces instead of round-tripping byte-identically.
    let mut ed = Editor::new(1);
    ed.load("`edge");
    run(&mut ed, Command::ToggleCode { from: 0, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "`` `edge ``");
    run(&mut ed, Command::ToggleCode { from: 5, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "`edge", "ON→OFF byte-identical for edge-backtick content");

    // A padded span wrapping nested backticks strips clean the same way.
    let mut ed = Editor::new(1);
    ed.load("`` `x` ``");
    let change = run(&mut ed, Command::ToggleCode { from: 4, to: 4 }).unwrap();
    assert_eq!(ed.get_text(), "`x`", "no stray pad spaces around the inner span");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (0, 3), "selection covers the unpadded content");

    // Plain unpadded spans keep the delimiter-only strip: byte-identical.
    let mut ed = Editor::new(1);
    ed.load("`code`");
    run(&mut ed, Command::ToggleCode { from: 3, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "code");
    run(&mut ed, Command::ToggleCode { from: 0, to: 4 }).unwrap();
    assert_eq!(ed.get_text(), "`code`", "unpadded round-trip stays byte-identical");

    // All-space content is never unpadded (CommonMark keeps it verbatim).
    let mut ed = Editor::new(1);
    ed.load("` `");
    run(&mut ed, Command::ToggleCode { from: 1, to: 1 }).unwrap();
    assert_eq!(ed.get_text(), " ", "only the ticks go; the space is content");
}

#[test]
fn toggle_code_pads_space_edged_content_and_round_trips() {
    // Space-edged content needs the same CommonMark pad pair as
    // backtick-edged content: `` ` x ` `` renders as `x` (the renderer sheds
    // one pad pair), so edge spaces survive rendering only under a pad. The
    // pad also keeps ON→OFF byte-identical — before it, OFF's pad-shedding
    // deleted the USER'S edge spaces ("a x b" → toggle " x " ON→OFF →
    // "axb", data loss).

    // Both edges.
    let mut ed = Editor::new(1);
    ed.load("a x b");
    let change = run(&mut ed, Command::ToggleCode { from: 1, to: 4 }).unwrap();
    assert_eq!(ed.get_text(), "a`  x  `b");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (3, 6), "selection covers the user content, not the pads");
    run(&mut ed, Command::ToggleCode { from: sel.anchor, to: sel.head }).unwrap();
    assert_eq!(ed.get_text(), "a x b", "ON→OFF byte-identical for space-edged content");

    // Leading edge only.
    let mut ed = Editor::new(1);
    ed.load("a xb");
    let change = run(&mut ed, Command::ToggleCode { from: 1, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "a`  x `b");
    let sel = change.selection.unwrap();
    run(&mut ed, Command::ToggleCode { from: sel.anchor, to: sel.head }).unwrap();
    assert_eq!(ed.get_text(), "a xb", "ON→OFF byte-identical for leading-space content");

    // Trailing edge only.
    let mut ed = Editor::new(1);
    ed.load("ax b");
    let change = run(&mut ed, Command::ToggleCode { from: 1, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "a` x  `b");
    let sel = change.selection.unwrap();
    run(&mut ed, Command::ToggleCode { from: sel.anchor, to: sel.head }).unwrap();
    assert_eq!(ed.get_text(), "ax b", "ON→OFF byte-identical for trailing-space content");

    // Space-only content is NOT padded (CommonMark never unpads an all-space
    // span, so a pad pair would become extra content bytes) and still
    // round-trips: OFF's shed rule has the matching all-space exemption.
    let mut ed = Editor::new(1);
    ed.load("a b");
    let change = run(&mut ed, Command::ToggleCode { from: 1, to: 2 }).unwrap();
    assert_eq!(ed.get_text(), "a` `b");
    let sel = change.selection.unwrap();
    run(&mut ed, Command::ToggleCode { from: sel.anchor, to: sel.head }).unwrap();
    assert_eq!(ed.get_text(), "a b", "ON→OFF byte-identical for space-only content");
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
fn toggle_strike_strips_single_tilde_delimiters() {
    // `~del~` is valid GFM strikethrough (single-tilde flavor): toggle OFF
    // must strip the source tildes — never wrap to `~~~del~~~`. The
    // canonical `~~` cycle is then byte-identical, matching the `__x__`
    // normalization rule for non-canonical flavors.
    let mut ed = Editor::new(1);
    ed.load("~del~");
    run(&mut ed, Command::ToggleStrike { from: 2, to: 2 }).unwrap();
    assert_eq!(ed.get_text(), "del");
    run(&mut ed, Command::ToggleStrike { from: 0, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "~~del~~");
    run(&mut ed, Command::ToggleStrike { from: 3, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "del", "canonical double-toggle is byte-identical");
}

#[test]
fn toggle_inline_across_leaf_blocks_errors_without_mutating() {
    // `**a\n\nb**` does not parse as strong (and a re-toggle would stack
    // delimiters), so a selection spanning more than one leaf block is
    // refused with InvalidArgument. A thrown command is a consumed no-op
    // for the view — the core must not have mutated anything.
    for doc in [
        "a\n\nb\n",                  // two paragraphs
        "# h\nbody\n",               // heading + paragraph
        "- a\n- b\n",                // two list items
        "```\ncode\nmore\n```\n",    // fenced code lines
        "> q\nplain para\n\nnext\n", // lazy quote paragraph, blank, paragraph
    ] {
        let mut ed = Editor::new(1);
        ed.load(doc);
        let rev = ed.revision();
        let end = ed.doc_len_utf16();
        let err = ed.command(Command::ToggleStrong { from: 0, to: end }).unwrap_err();
        assert_eq!(err.name(), "InvalidArgument", "{doc:?}");
        assert_eq!(ed.get_text(), doc, "thrown command must not mutate");
        assert_eq!(ed.revision(), rev, "thrown command must not burn a revision");
    }
    // Multi-LINE within ONE paragraph still applies — a softbreak inside
    // strong parses fine.
    let mut ed = Editor::new(1);
    ed.load("soft\nwrap\n");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "**soft\nwrap**\n");
    // ...including inside a blockquote paragraph (same depth per line).
    let mut ed = Editor::new(1);
    ed.load("> a\n> b\n");
    run(&mut ed, Command::ToggleEm { from: 2, to: 7 }).unwrap();
    assert_eq!(ed.get_text(), "> *a\n> b*\n");
}

#[test]
fn toggle_inline_does_not_apply_in_code_contexts() {
    // Single-line range on a fenced-code line: `single_leaf_block`
    // short-circuits true for single-line ranges, so the fence refusal must
    // fire on its own. Ok(None) like `set_heading`'s code check (the
    // nearest analog) — no mutation, no burned revision, unlike the
    // multi-block InvalidArgument throw.
    let doc = "```\ncode\n```\n";
    let mut ed = Editor::new(1);
    ed.load(doc);
    let rev = ed.revision();
    assert!(ed.command(Command::ToggleStrong { from: 4, to: 8 }).unwrap().is_none());
    assert_eq!(ed.get_text(), doc, "no delimiters written into the fence");
    assert_eq!(ed.revision(), rev, "refusal must not burn a revision");
    // The fence line itself (backticks) is equally off limits, for every kind.
    assert!(ed.command(Command::ToggleCode { from: 0, to: 3 }).unwrap().is_none());
    assert_eq!(ed.get_text(), doc);

    // Range strictly inside an inline code span: the delimiters would be
    // literal content (`` `c**od**e` ``), and a re-toggle would stack.
    let mut ed = Editor::new(1);
    ed.load("`code`");
    let rev = ed.revision();
    assert!(ed.command(Command::ToggleStrong { from: 2, to: 4 }).unwrap().is_none());
    assert_eq!(ed.get_text(), "`code`", "no mutation inside the code span");
    assert_eq!(ed.revision(), rev, "refusal must not burn a revision");
    // A cursor inside the span (the stacking re-toggle shape) is refused too.
    assert!(ed.command(Command::ToggleStrong { from: 3, to: 3 }).unwrap().is_none());
    assert_eq!(ed.get_text(), "`code`");

    // ...but endpoints AT the span's extent boundaries wrap the WHOLE span,
    // which parses fine (strong containing code).
    let mut ed = Editor::new(1);
    ed.load("`code`");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "**`code`**");
}

#[test]
fn empty_pair_cursor_retoggle_nests_another_pair() {
    // Pins the module-doc-documented ACCEPTED behavior: an empty-range
    // toggle inserts a non-parsing empty pair, and a second cursor toggle
    // between the delimiters nests another pair rather than removing the
    // first — the empty pair parses as no node, so there is nothing to
    // strip. If this drifts, the module doc must change with it.
    let mut ed = Editor::new(1);
    ed.load("ab");
    let change = run(&mut ed, Command::ToggleStrong { from: 1, to: 1 }).unwrap();
    assert_eq!(ed.get_text(), "a****b");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (3, 3), "cursor between the delimiters");
    let change = run(&mut ed, Command::ToggleStrong { from: 3, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "a********b", "second press nests, not unwraps");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (5, 5), "cursor between the inner pair");
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

#[test]
fn toggle_trims_whitespace_edges_and_double_toggle_stays_byte_identical() {
    // "a b" toggled over "a " (0..2): the space trims off, delimiters land
    // tight against content — never the flanking-violating `**a **b` that
    // stacked `****a ****b` on retoggle.
    let mut ed = Editor::new(1);
    ed.load("a b");
    let change = run(&mut ed, Command::ToggleStrong { from: 0, to: 2 }).unwrap();
    assert_eq!(ed.get_text(), "**a** b");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (2, 3), "selection covers the trimmed content");
    // Retoggle over the returned selection: byte-identical round trip.
    run(&mut ed, Command::ToggleStrong { from: sel.anchor, to: sel.head }).unwrap();
    assert_eq!(ed.get_text(), "a b");

    // Leading whitespace trims too, for every non-code kind.
    let mut ed = Editor::new(1);
    ed.load("a b");
    run(&mut ed, Command::ToggleEm { from: 1, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "a *b*");
    let mut ed = Editor::new(1);
    ed.load("a b");
    run(&mut ed, Command::ToggleStrike { from: 1, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "a ~~b~~");

    // A selection swallowing a trailing softbreak trims it off.
    let mut ed = Editor::new(1);
    ed.load("ab\ncd");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "**ab**\ncd");

    // OFF detection sees the trimmed range too: selecting "**a** " (the
    // trailing space included) unwraps rather than extending.
    let mut ed = Editor::new(1);
    ed.load("**a** b");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "a b");

    // Trimming runs BEFORE the multi-block guard: "a" plus the two
    // terminators trims down to just "a" (single block), so this applies
    // instead of throwing.
    let mut ed = Editor::new(1);
    ed.load("a\n\nb");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "**a**\n\nb");

    // toggleCode keeps its exact range (no flanking rules for code spans):
    // "a " wraps as-is — the trailing space stays content, space-edged
    // content padded per the existing rule.
    let mut ed = Editor::new(1);
    ed.load("a b");
    run(&mut ed, Command::ToggleCode { from: 0, to: 2 }).unwrap();
    assert_eq!(ed.get_text(), "` a  `b", "code toggle does not trim");
}

#[test]
fn toggle_whitespace_only_selection_does_not_apply() {
    // Trimmed-to-empty selections: `Ok(None)`, no mutation, no burned
    // revision — like any other doesn't-apply target. Cursor behavior
    // (empty-pair insertion) is unaffected, pinned elsewhere.
    for (doc, from, to) in [
        ("a  b", 1, 3),   // plain spaces
        ("a\tb", 1, 2),   // tab
        ("a\n \nb", 1, 4), // whitespace spanning a blank line (trim precedes the guard)
    ] {
        let mut ed = Editor::new(1);
        ed.load(doc);
        let rev = ed.revision();
        assert!(
            ed.command(Command::ToggleStrong { from, to }).unwrap().is_none(),
            "{doc:?} {from}..{to}"
        );
        assert_eq!(ed.get_text(), doc, "no mutation for {doc:?}");
        assert_eq!(ed.revision(), rev, "no revision bump for {doc:?}");
    }
}

#[test]
fn toggle_trims_unicode_whitespace_and_keeps_astral_content() {
    // NBSP (U+00A0) and ideographic space (U+3000) are in the pinned WS
    // set; the astral 😀 is content, not whitespace. UTF-16 offsets.
    let mut ed = Editor::new(1);
    ed.load("a\u{00A0}\u{3000}b");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "**a**\u{00A0}\u{3000}b");

    // Trailing space after a surrogate-pair scalar trims; 😀 stays wrapped.
    let mut ed = Editor::new(1);
    ed.load("😀 x");
    run(&mut ed, Command::ToggleStrong { from: 0, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "**😀** x");

    // Em space + thin space (U+2003, U+2009): whitespace-only, no apply.
    let mut ed = Editor::new(1);
    ed.load("a\u{2003}\u{2009}b");
    let rev = ed.revision();
    assert!(ed.command(Command::ToggleEm { from: 1, to: 3 }).unwrap().is_none());
    assert_eq!(ed.get_text(), "a\u{2003}\u{2009}b");
    assert_eq!(ed.revision(), rev);
}

#[test]
fn command_positions_splitting_a_crlf_pair_are_refused() {
    // Contract: any command position argument falling between the '\r' and
    // '\n' of a CRLF pair refuses with InvalidArgument and this EXACT
    // message (byte-pinned across core and mock by the conformance suite).
    let doc = "ab\r\ncd\r\n";
    let mut ed = Editor::new(1);
    ed.load(doc);
    let rev = ed.revision();
    for cmd in [
        Command::ToggleStrong { from: 3, to: 6 },
        Command::ToggleEm { from: 0, to: 3 },
        Command::ToggleStrike { from: 3, to: 3 },
        Command::ToggleCode { from: 3, to: 3 },
        Command::SetHeading { pos: 3, level: 1 },
        Command::ToggleTask { pos: 3 },
        Command::IndentList { from: 3, to: 3 },
        Command::OutdentList { from: 3, to: 3 },
        Command::Enter { from: 3, to: 3 },
        Command::ToggleQuote { from: 3, to: 6 },
        Command::ToggleLink { from: 0, to: 3 },
        Command::ToggleBulletList { from: 3, to: 3 },
        Command::ToggleOrderedList { from: 3, to: 3 },
        Command::InsertHr { pos: 3 },
        Command::ToggleCodeBlock { from: 3, to: 3 },
    ] {
        let err = ed.command(cmd).unwrap_err();
        assert_eq!(err.name(), "InvalidArgument", "{cmd:?}");
        assert_eq!(
            err.to_string(),
            "InvalidArgument: position 3 splits a CRLF sequence",
            "{cmd:?}"
        );
        assert_eq!(ed.get_text(), doc, "no mutation for {cmd:?}");
        assert_eq!(ed.revision(), rev, "no revision bump for {cmd:?}");
    }
    // Positions AT either side of the pair stay valid (2 = before '\r',
    // 4 = after '\n').
    run(&mut ed, Command::ToggleStrong { from: 0, to: 2 }).unwrap();
    assert_eq!(ed.get_text(), "**ab**\r\ncd\r\n");
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
fn set_heading_same_level_toggles_back_to_paragraph() {
    // Bug 2 (v0.5 amendment): pressing the SAME heading level again removes
    // the heading instead of no-op'ing, exactly like level 0 — an idempotent
    // toolbar press, matching Obsidian's toggle behavior.
    for level in 1u8..=6 {
        let prefix = "#".repeat(level as usize);
        let doc = format!("{prefix} title\n");
        let mut ed = Editor::new(1);
        ed.load(&doc);
        let pos = prefix.len() + 1; // inside "title"
        run(&mut ed, Command::SetHeading { pos, level }).unwrap();
        assert_eq!(ed.get_text(), "title\n", "level {level}");
    }
}

#[test]
fn set_heading_same_level_toggle_is_irregular_whitespace_tolerant() {
    // The same-level comparison reads the parsed `Heading` node's own level,
    // not a byte-identical prefix match — an irregular "##  x" (extra inner
    // space, only the FIRST of which is the delimiter's required space)
    // still counts as "already level 2" and toggles back (the delimiter
    // span itself — "## " — is removed; the extra space is content, same as
    // the pre-existing level-0 removal path, and stays).
    let mut ed = Editor::new(1);
    ed.load("##  x\n");
    run(&mut ed, Command::SetHeading { pos: 4, level: 2 }).unwrap();
    assert_eq!(ed.get_text(), " x\n");
}

#[test]
fn set_heading_different_level_still_replaces_not_toggles() {
    // Control: a DIFFERENT level always replaces the prefix (never treated
    // as a toggle-back), same as before this change.
    let mut ed = Editor::new(1);
    ed.load("## title\n");
    let rev = ed.revision();
    run(&mut ed, Command::SetHeading { pos: 4, level: 3 }).unwrap();
    assert_eq!(ed.get_text(), "### title\n");
    assert_ne!(ed.revision(), rev);
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
fn set_heading_handles_atx_with_legal_leading_indent() {
    // 1-3 spaces of leading indent are legal ATX indentation (CommonMark):
    // relevel and remove must find the hash run past them. Pre-fix both
    // returned Ok(None) because the heading's delimiter doesn't start at
    // the line start.
    let mut ed = Editor::new(1);
    ed.load("  # foo\n");
    run(&mut ed, Command::SetHeading { pos: 4, level: 2 }).unwrap();
    assert_eq!(ed.get_text(), "  ## foo\n", "indented ATX relevels in place");

    let mut ed = Editor::new(1);
    ed.load("  # foo\n");
    run(&mut ed, Command::SetHeading { pos: 4, level: 0 }).unwrap();
    assert_eq!(ed.get_text(), "  foo\n", "level 0 removes the indented hash run");

    // 3 spaces is the maximum; the hash run still resolves.
    let mut ed = Editor::new(1);
    ed.load("   ## bar\n");
    run(&mut ed, Command::SetHeading { pos: 6, level: 1 }).unwrap();
    assert_eq!(ed.get_text(), "   # bar\n");

    // 4 spaces is indented code in the v0 grammar (pulldown parses it as an
    // indented code block, so the BlockKind gate refuses): unchanged.
    let mut ed = Editor::new(1);
    ed.load("    # foo\n");
    let rev = ed.revision();
    assert!(ed.command(Command::SetHeading { pos: 6, level: 2 }).unwrap().is_none());
    assert_eq!(ed.get_text(), "    # foo\n");
    assert_eq!(ed.revision(), rev);
}

#[test]
fn set_heading_on_an_empty_quote_line_is_null_like_a_blank_line() {
    // A quote line with EMPTY content must behave like a blank paragraph
    // line: nothing to promote. Pre-fix `">"` + level 1 produced `"># "` —
    // an empty heading inside the empty quote line.
    for doc in [">", "> ", "> x\n>\n"] {
        let mut ed = Editor::new(1);
        ed.load(doc);
        let rev = ed.revision();
        let pos = doc.len() - if doc.ends_with('\n') { 1 } else { 0 };
        assert!(
            ed.command(Command::SetHeading { pos, level: 1 }).unwrap().is_none(),
            "{doc:?}"
        );
        assert_eq!(ed.get_text(), doc, "no mutation for {doc:?}");
        assert_eq!(ed.revision(), rev);
    }
    // The blank-paragraph-line control keeps returning None.
    let mut ed = Editor::new(1);
    ed.load("x\n\ny\n");
    assert!(ed.command(Command::SetHeading { pos: 2, level: 1 }).unwrap().is_none());
    assert_eq!(ed.get_text(), "x\n\ny\n");
    // ...while a NON-empty quote line still promotes (the guard is about
    // content emptiness, not quotes).
    let mut ed = Editor::new(1);
    ed.load("> quoted\n");
    run(&mut ed, Command::SetHeading { pos: 4, level: 1 }).unwrap();
    assert_eq!(ed.get_text(), "> # quoted\n");
}

#[test]
fn set_heading_inside_blockquote_goes_after_markers() {
    let mut ed = Editor::new(1);
    ed.load("> quoted line\n");
    run(&mut ed, Command::SetHeading { pos: 5, level: 2 }).unwrap();
    assert_eq!(ed.get_text(), "> ## quoted line\n");
}

#[test]
fn set_heading_refuses_blockquote_nested_list_items_and_breaks_like_top_level() {
    // The same block gate as at top level applies AFTER the quote markers
    // are stripped: a quote-nested list item / thematic break refuses
    // (None, no burned revision). Pre-fix `"> - item"` + level 1 produced
    // `"> # - item"` — a heading swallowing the marker.
    for doc in ["> - item\n", "> 1. item\n", "> > - deep\n", "> ---\n"] {
        let mut ed = Editor::new(1);
        ed.load(doc);
        let rev = ed.revision();
        let pos = doc.len() - 2; // on the construct's own line content
        assert!(
            ed.command(Command::SetHeading { pos, level: 1 }).unwrap().is_none(),
            "{doc:?}"
        );
        assert_eq!(ed.get_text(), doc, "no mutation for {doc:?}");
        assert_eq!(ed.revision(), rev, "no revision bump for {doc:?}");
    }
    // Control: a plain quoted line still promotes (the refusal is about
    // the nested construct, not quotes).
    let mut ed = Editor::new(1);
    ed.load("> quoted\n");
    run(&mut ed, Command::SetHeading { pos: 4, level: 1 }).unwrap();
    assert_eq!(ed.get_text(), "> # quoted\n");
}

#[test]
fn set_heading_zero_removes_the_closing_hash_run_too() {
    // Level 0 deletes ALL heading delimiter spans — the ATX closing hash
    // run included. Pre-fix `"# foo #"` became `"foo #"`, leaving a stray
    // delimiter behind.
    let mut ed = Editor::new(1);
    ed.load("# foo #\n");
    run(&mut ed, Command::SetHeading { pos: 3, level: 0 }).unwrap();
    assert_eq!(ed.get_text(), "foo\n");

    let mut ed = Editor::new(1);
    ed.load("## x ##\n");
    run(&mut ed, Command::SetHeading { pos: 3, level: 0 }).unwrap();
    assert_eq!(ed.get_text(), "x\n");

    // Releveling leaves the closing run alone (unchanged behavior).
    let mut ed = Editor::new(1);
    ed.load("# foo #\n");
    run(&mut ed, Command::SetHeading { pos: 3, level: 3 }).unwrap();
    assert_eq!(ed.get_text(), "### foo #\n");
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

// ------------------------------------------------- toggleTask promotion --
// Bug 1 (v0.5 amendment): toggleTask on a non-task target now PROMOTES
// instead of refusing (Obsidian parity, research/07 §1.6). `run()` also
// asserts the whole-doc itemness invariant for every case here — a
// promotion may ADD itemness to the target line, but must never cost any
// OTHER line its own.

#[test]
fn toggle_task_promotes_a_plain_bullet_and_ordered_item() {
    let mut ed = Editor::new(1);
    ed.load("- plain item\n");
    run(&mut ed, Command::ToggleTask { pos: 3 }).unwrap();
    assert_eq!(ed.get_text(), "- [ ] plain item\n");

    let mut ed = Editor::new(1);
    ed.load("1. plain item\n");
    run(&mut ed, Command::ToggleTask { pos: 4 }).unwrap();
    assert_eq!(ed.get_text(), "1. [ ] plain item\n");
}

#[test]
fn toggle_task_promotes_a_nested_bullet_without_disturbing_the_parent() {
    let mut ed = Editor::new(1);
    ed.load("- parent\n  - child\n");
    run(&mut ed, Command::ToggleTask { pos: 14 }).unwrap();
    assert_eq!(ed.get_text(), "- parent\n  - [ ] child\n");
}

#[test]
fn toggle_task_promotes_a_quoted_bullet() {
    let mut ed = Editor::new(1);
    ed.load("> - item\n");
    run(&mut ed, Command::ToggleTask { pos: 5 }).unwrap();
    assert_eq!(ed.get_text(), "> - [ ] item\n");
}

#[test]
fn toggle_task_promotes_a_plain_paragraph() {
    let mut ed = Editor::new(1);
    ed.load("paragraph\n");
    run(&mut ed, Command::ToggleTask { pos: 3 }).unwrap();
    assert_eq!(ed.get_text(), "- [ ] paragraph\n");
}

#[test]
fn toggle_task_promotes_a_quoted_paragraph_preserving_the_prefix() {
    let mut ed = Editor::new(1);
    ed.load("> quoted text\n");
    run(&mut ed, Command::ToggleTask { pos: 5 }).unwrap();
    assert_eq!(ed.get_text(), "> - [ ] quoted text\n");
}

#[test]
fn toggle_task_promotes_a_blank_line() {
    // Obsidian promotes a blank line rather than no-op'ing — more permissive
    // than setHeading's own blank-line refusal (deliberate, v0.5).
    let mut ed = Editor::new(1);
    ed.load("a\n\nb\n");
    run(&mut ed, Command::ToggleTask { pos: 2 }).unwrap();
    assert_eq!(ed.get_text(), "a\n- [ ] \nb\n");
}

#[test]
fn toggle_task_promotes_an_empty_document() {
    let mut ed = Editor::new(1);
    ed.load("");
    run(&mut ed, Command::ToggleTask { pos: 0 }).unwrap();
    assert_eq!(ed.get_text(), "- [ ] ");
}

#[test]
fn toggle_task_still_null_on_heading_fence_and_hr() {
    for (doc, pos) in [("# heading\n", 3usize), ("```\ncode\n```\n", 5), ("---\n", 1)] {
        let mut ed = Editor::new(1);
        ed.load(doc);
        let rev = ed.revision();
        assert!(
            ed.command(Command::ToggleTask { pos }).unwrap().is_none(),
            "{doc:?}"
        );
        assert_eq!(ed.get_text(), doc, "no mutation for {doc:?}");
        assert_eq!(ed.revision(), rev, "no revision bump for {doc:?}");
    }
}

#[test]
fn toggle_task_promotion_is_a_single_undo_unit() {
    let mut ed = Editor::new(1);
    ed.load("- plain item\n");
    run(&mut ed, Command::ToggleTask { pos: 3 }).unwrap();
    assert_eq!(ed.get_text(), "- [ ] plain item\n");
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "- plain item\n", "undo restores the plain bullet in one step");
}

#[test]
fn toggle_task_promotion_maps_selection_to_stay_with_its_character() {
    // Bias::After: the insertion always lands at/before `pos`, so the
    // character immediately after the (collapsed) cursor is unchanged —
    // typing right where the user clicked still lands in "item", not
    // stranded between the new marker and the new checkbox.
    let mut ed = Editor::new(1);
    ed.load("- item\n");
    let change = ed.command(Command::ToggleTask { pos: 2 }).unwrap().unwrap();
    assert_eq!(ed.get_text(), "- [ ] item\n");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (6, 6), "cursor lands right before \"item\"");
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

// ------------------------------------------------- indentList/outdentList --
//
// Marker-width-aware Tab nesting (boundary v0.2): indenting nests a child
// under the nearest previous item at or above its own marker column, to
// that item's CONTENT column (marker + required trailing space), not a
// fixed 2-space shift.

/// Post-condition for EVERY indent/outdent test (the invariant the
/// paragraph-interruption bug violated): reparse the result and assert the
/// line containing `needle` still carries its own ListMarker — i.e. the
/// command never produced markdown where the moved line silently degrades
/// to lazy-continuation paragraph text.
fn assert_list_item_line(ed: &Editor, needle: &str) {
    let text = ed.get_text();
    let pos = text
        .find(needle)
        .unwrap_or_else(|| panic!("line {needle:?} not found in {text:?}"));
    let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    let nodes = oxidown_core::parser::parse(&text);
    assert!(
        nodes.iter().any(|n| {
            matches!(n.kind, oxidown_core::parser::NodeKind::ListMarker { .. })
                && n.extent.start >= line_start
                && n.extent.start < line_end
        }),
        "the line containing {needle:?} must still parse as a list item in {text:?}"
    );
}

#[test]
fn indent_bullet_under_bullet_is_plus_two() {
    let mut ed = Editor::new(1);
    ed.load("- a\n- b\n");
    // Cursor inside "b"'s text — indenting the ITEM, not the cursor position.
    let change = run(&mut ed, Command::IndentList { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n  - b\n");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (8, 8), "cursor follows its character");
    assert_list_item_line(&ed, "- b");
}

#[test]
fn indent_works_on_lone_cr_terminated_lines() {
    // pulldown-cmark treats a lone '\r' as a line ending ("- a\r- b\r- c"
    // is THREE list items), so the command planners' own line resolution
    // must agree. Pre-fix bug: `line_containing`'s hand-rolled backward
    // scan only stopped at '\n', so from inside "b" it resolved a "line"
    // spanning back through "- a", found no marker STARTING there in the
    // mis-derived range's node lookup, and the command no-op'd/misfired.
    let mut ed = Editor::new(1);
    ed.load("- a\r- b\r- c");
    let pos = "- a\r- b".len() - 1; // inside "b"'s text
    let change = run(&mut ed, Command::IndentList { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "- a\r  - b\r- c", "\"- b\" nests under \"- a\"; \"- c\" untouched");
    assert!(!change.splices.is_empty());
    // And outdent reverses it, resolving the same lone-\r line shape.
    let sel = change.selection.unwrap();
    run(&mut ed, Command::OutdentList { from: sel.anchor, to: sel.head }).unwrap();
    assert_eq!(ed.get_text(), "- a\r- b\r- c");
}

#[test]
fn indent_ordered_under_ordered_is_plus_three() {
    let mut ed = Editor::new(1);
    ed.load("1. a\n2. b\n");
    run(&mut ed, Command::IndentList { from: 8, to: 8 }).unwrap();
    // "2." is rewritten to "1.": a non-1 ordered marker cannot interrupt the
    // parent item's paragraph, so "   2. b" would de-list into continuation
    // text (the paragraph-interruption guard; see the structural-rewrite
    // tests below).
    assert_eq!(ed.get_text(), "1. a\n   1. b\n");
    assert_list_item_line(&ed, "1. b");
}

#[test]
fn indent_under_double_digit_marker_is_plus_four() {
    let mut ed = Editor::new(1);
    ed.load("10. a\n11. b\n");
    run(&mut ed, Command::IndentList { from: 10, to: 10 }).unwrap();
    // +4 (the "10. " content column), digits guard-rewritten to "1".
    assert_eq!(ed.get_text(), "10. a\n    1. b\n");
    assert_list_item_line(&ed, "1. b");
}

#[test]
fn indent_bullet_under_ordered_is_plus_three() {
    let mut ed = Editor::new(1);
    ed.load("1. a\n- b\n");
    run(&mut ed, Command::IndentList { from: 7, to: 7 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n   - b\n");
    assert_list_item_line(&ed, "- b");
}

#[test]
fn indent_task_under_task_is_plus_two() {
    let mut ed = Editor::new(1);
    ed.load("- [ ] a\n- [ ] b\n");
    // Cursor on the "b" character itself.
    run(&mut ed, Command::IndentList { from: 14, to: 14 }).unwrap();
    assert_eq!(ed.get_text(), "- [ ] a\n  - [ ] b\n");
    assert_list_item_line(&ed, "- [ ] b");
}

#[test]
fn indent_inside_a_quote_stays_relative_to_the_quote_prefix() {
    let mut ed = Editor::new(1);
    ed.load("> 1. a\n> 2. b\n");
    // Cursor inside "b", after the quote prefix.
    run(&mut ed, Command::IndentList { from: 12, to: 12 }).unwrap();
    // 3 spaces after "> " (the parent's content column), digits
    // guard-rewritten to "1" (landing on the shallower "1. a").
    assert_eq!(ed.get_text(), "> 1. a\n>    1. b\n");
    assert_list_item_line(&ed, "1. b");
}

#[test]
fn indent_does_not_nest_across_a_quote_boundary() {
    // A list outside a quote never nests relative to items inside it: the
    // line above is quoted (depth 1), "- b" is depth 0 — the scan stops
    // immediately and there is no candidate.
    let mut ed = Editor::new(1);
    ed.load("> - a\n- b\n");
    let rev = ed.revision();
    let change = run(&mut ed, Command::IndentList { from: 8, to: 8 }).unwrap();
    assert_eq!(ed.get_text(), "> - a\n- b\n", "no-op: quote depth differs");
    assert!(change.splices.is_empty());
    assert_eq!(ed.revision(), rev, "no-op must not burn a revision");
}

#[test]
fn indent_multiline_selection_moves_together_by_first_lines_delta() {
    let mut ed = Editor::new(1);
    ed.load("- a\n- b\n- c\n");
    // Select across "b" and "c": both siblings shift by "b"'s delta (2),
    // they do NOT nest under one another.
    let change = run(&mut ed, Command::IndentList { from: 4, to: 11 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n  - b\n  - c\n");
    assert_eq!(change.splices.len(), 2, "one splice per intersecting item line");
    assert_list_item_line(&ed, "- b");
    assert_list_item_line(&ed, "- c");
}

#[test]
fn indent_first_item_of_a_list_is_a_noop() {
    let mut ed = Editor::new(1);
    ed.load("- a\n- b\n");
    let rev = ed.revision();
    let (undo_depth, _) = ed.history_depths();
    let change = run(&mut ed, Command::IndentList { from: 2, to: 2 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n- b\n", "no movement possible");
    assert!(change.splices.is_empty());
    assert!(change.selection.is_none());
    assert_eq!(ed.revision(), rev, "no-op must not burn a revision");
    assert_eq!(ed.history_depths().0, undo_depth, "no-op must not create an undo unit");
}

#[test]
fn indent_non_list_range_does_not_apply() {
    let mut ed = Editor::new(1);
    ed.load("plain paragraph\n");
    assert!(ed.command(Command::IndentList { from: 3, to: 3 }).unwrap().is_none());
}

#[test]
fn indent_already_at_parents_content_column_is_a_noop() {
    // "  - b" is already exactly at "- a"'s content column (2): indenting
    // further has no target that would move it forward.
    let mut ed = Editor::new(1);
    ed.load("- a\n  - b\n");
    let change = run(&mut ed, Command::IndentList { from: 8, to: 8 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n  - b\n");
    assert!(change.splices.is_empty());
}

#[test]
fn outdent_reverses_each_indent_case() {
    // Nested ordered children are written as "1." here — exactly what the
    // paragraph-interruption guard produces when indenting (a non-1 ordered
    // marker nested directly under an open paragraph would not parse as a
    // list item, so indent rewrites it; these fixtures ARE the guard's
    // output shape, and the chained Tab→Shift-Tab test below covers the
    // full round trip through one editor).
    for (indented, from, to, flat) in [
        ("- a\n  - b\n", 8, 8, "- a\n- b\n"),
        ("1. a\n   1. b\n", 11, 11, "1. a\n1. b\n"),
        ("10. a\n    1. b\n", 13, 13, "10. a\n1. b\n"),
        ("1. a\n   - b\n", 10, 10, "1. a\n- b\n"),
        ("- [ ] a\n  - [ ] b\n", 16, 16, "- [ ] a\n- [ ] b\n"),
        ("> 1. a\n>    1. b\n", 15, 15, "> 1. a\n> 1. b\n"),
    ] {
        let mut ed = Editor::new(1);
        ed.load(indented);
        run(&mut ed, Command::OutdentList { from, to }).unwrap();
        assert_eq!(ed.get_text(), flat, "outdent reverses {indented:?}");
        assert_list_item_line(&ed, flat.lines().nth(1).unwrap().trim_start_matches("> "));
    }
}

#[test]
fn outdent_at_top_level_is_a_noop() {
    let mut ed = Editor::new(1);
    ed.load("- a\n- b\n");
    let rev = ed.revision();
    let change = run(&mut ed, Command::OutdentList { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n- b\n");
    assert!(change.splices.is_empty());
    assert_eq!(ed.revision(), rev, "no-op must not burn a revision");
}

#[test]
fn outdent_clamps_to_a_lines_own_leading_space_count() {
    // "- a" under leading space each: the "a" child has 2 leading spaces,
    // the "c" child has only 1 (a malformed/partial indent). Outdenting
    // both together wants delta=2, but "c" only has 1 to give up.
    let mut ed = Editor::new(1);
    ed.load("- p\n  - a\n - c\n");
    // Select across "a" and "c"; "a" (col 2) is the first intersecting item.
    let change = run(&mut ed, Command::OutdentList { from: 8, to: 13 }).unwrap();
    assert_eq!(ed.get_text(), "- p\n- a\n- c\n");
    assert_eq!(change.splices.len(), 2);
    let deletes: Vec<usize> = change.splices.iter().map(|s| s.delete).collect();
    assert_eq!(deletes, vec![2, 1], "the second line clamps to its own 1 leading space");
    assert_list_item_line(&ed, "- a");
    assert_list_item_line(&ed, "- c");
}

#[test]
fn undo_restores_a_multiline_indent_in_one_step() {
    let mut ed = Editor::new(1);
    ed.load("- a\n- b\n- c\n");
    let (undo_depth, _) = ed.history_depths();
    run(&mut ed, Command::IndentList { from: 4, to: 11 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n  - b\n  - c\n");
    assert_eq!(ed.history_depths().0, undo_depth + 1, "one undo unit for the whole batch");

    let mut mirror = ed.get_text();
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "- a\n- b\n- c\n", "undo restores both lines in one step");
    assert_eq!(mirror, ed.get_text());

    let r = ed.redo().unwrap();
    apply_to_mirror(&mut mirror, &r.splices);
    assert_eq!(ed.get_text(), "- a\n  - b\n  - c\n");
    assert_eq!(mirror, ed.get_text());
}

#[test]
fn indent_with_multibyte_text_before_the_list_still_works() {
    let mut ed = Editor::new(1);
    let doc = "prefix 你好😀\n\n- a\n- b\n";
    ed.load(doc);
    let b_byte = doc.rfind("- b").unwrap() + 2; // byte index of 'b' in "- b"
    let cu = doc[..b_byte].encode_utf16().count();
    let change = run(&mut ed, Command::IndentList { from: cu, to: cu }).unwrap();
    assert_eq!(ed.get_text(), "prefix 你好😀\n\n- a\n  - b\n");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (cu + 2, cu + 2));
    assert_list_item_line(&ed, "- b");
}

// ---------------------------------------------- subtree-aware affected set --
//
// Not just the intersecting lines: for every intersecting item line, its
// whole subtree (consecutive following lines, same quote depth, marker
// column strictly greater than THAT line's own) moves with it by the same
// single delta.

#[test]
fn indent_a_parent_moves_its_whole_subtree_with_it() {
    let mut ed = Editor::new(1);
    // Cursor on "p" only — no selection spans "c1"/"c2" — but both are "p"'s
    // children and must move with it.
    ed.load("- x\n- p\n  - c1\n  - c2\n");
    let change = run(&mut ed, Command::IndentList { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "- x\n  - p\n    - c1\n    - c2\n");
    assert_eq!(change.splices.len(), 3, "p + both children move");
    assert_list_item_line(&ed, "- p");
}

#[test]
fn outdent_reverses_a_subtree_move() {
    let mut ed = Editor::new(1);
    ed.load("- x\n  - p\n    - c1\n    - c2\n");
    let change = run(&mut ed, Command::OutdentList { from: 8, to: 8 }).unwrap();
    assert_eq!(ed.get_text(), "- x\n- p\n  - c1\n  - c2\n");
    assert_eq!(change.splices.len(), 3);
    assert_list_item_line(&ed, "- p");
}

#[test]
fn indent_subtree_does_not_include_a_following_sibling() {
    // "sibling" has the SAME marker column as "p" (0): it's a sibling, not a
    // descendant, and must not move.
    let mut ed = Editor::new(1);
    ed.load("- x\n- p\n  - c1\n- sibling\n");
    let change = run(&mut ed, Command::IndentList { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "- x\n  - p\n    - c1\n- sibling\n");
    assert_eq!(change.splices.len(), 2, "p + c1 only");
    assert_list_item_line(&ed, "- p");
}

#[test]
fn indent_subtree_walk_stops_at_a_blank_line() {
    // "c2" is indented like a child of "p", but a blank line sits between it
    // and "c1" — v1 does not look past a non-list-context line to see
    // whether list content resumes, so "c2" is left behind.
    let mut ed = Editor::new(1);
    ed.load("- x\n- p\n  - c1\n\n  - c2\n");
    let change = run(&mut ed, Command::IndentList { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "- x\n  - p\n    - c1\n\n  - c2\n", "c2 untouched");
    assert_eq!(change.splices.len(), 2, "p + c1 only");
    assert_list_item_line(&ed, "- p");
}

#[test]
fn indent_subtree_inside_a_quote_respects_quote_depth() {
    // "c1" is inside the same quote as "p" (depth 1) and moves with it;
    // "outside" is depth 0 and must not.
    let mut ed = Editor::new(1);
    ed.load("> - x\n> - p\n>   - c1\n- outside\n");
    let change = run(&mut ed, Command::IndentList { from: 10, to: 10 }).unwrap();
    assert_eq!(ed.get_text(), "> - x\n>   - p\n>     - c1\n- outside\n");
    assert_eq!(change.splices.len(), 2, "p + c1 only, not outside");
    assert_list_item_line(&ed, "- p");
}

// --------------------------------------- paragraph-interruption guard --
//
// A non-1 ordered marker cannot START a list in paragraph-interruption
// position (CommonMark): without the guard, indenting "2. b" under "1. a"
// produces "   2. b", which reparses as LAZY CONTINUATION of item 1's
// paragraph — the item (and its whole subtree) silently de-lists, and the
// next Shift-Tab finds no item line and returns None. The guard rewrites
// the moved line's digits to "1" (same batch, same undo unit) unless it
// lands where a same-delimiter ordered list is already open.

#[test]
fn chained_indent_then_outdent_on_the_flagship_repro() {
    // The demo-doc repro, chained through ONE editor instance (no reload
    // between commands) — the regression the original tests missed.
    let doc = "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n3. ordered three\n";
    let mut ed = Editor::new(1);
    ed.load(doc);

    // Tab on "2. ordered two" (cursor mid-text).
    let change = run(&mut ed, Command::IndentList { from: 21, to: 21 }).unwrap();
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n   1. ordered two\n      1. nested ordered item\n      - a bullet nested under an ordered item\n3. ordered three\n",
        "digits rewritten to 1, subtree carried"
    );
    assert_eq!(change.splices.len(), 4, "3 indents + 1 digit rewrite");
    // The moved line IS a list item after reparse (the invariant the bug broke).
    assert_list_item_line(&ed, "1. ordered two");
    assert_list_item_line(&ed, "1. nested ordered item");
    assert_list_item_line(&ed, "- a bullet nested under an ordered item");

    // Shift-Tab at the returned selection restores the nesting structure
    // (numbers may legitimately differ from the original bytes — assert
    // structure, not byte-identity).
    let sel = change.selection.expect("indent returns a selection");
    let out = run(&mut ed, Command::OutdentList { from: sel.anchor, to: sel.head })
        .expect("chained outdent must apply — the indented line must still be an item");
    assert!(!out.splices.is_empty(), "outdent actually moves the lines back");
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n1. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n3. ordered three\n",
        "structure restored: top-level item with its two children re-attached"
    );
    assert_list_item_line(&ed, "1. ordered two");
    assert_list_item_line(&ed, "1. nested ordered item");
    assert_list_item_line(&ed, "- a bullet nested under an ordered item");
}

#[test]
fn chained_indent_repro_end_state_has_correctly_sequenced_widgets() {
    // Same doc/first step as `chained_indent_then_outdent_on_the_flagship_repro`
    // (research/07 view-computed numbering, boundary-v0.md v0.3 amendment):
    // the command's digit rewrite (1./1./1./.../3.) is STRUCTURAL only — the
    // DISPLAYED sequence must still read 1,2,3 per list, regardless of the
    // raw digits the command left behind.
    let doc = "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n3. ordered three\n";
    let mut ed = Editor::new(1);
    ed.load(doc);
    run(&mut ed, Command::IndentList { from: 21, to: 21 }).unwrap();
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n   1. ordered two\n      1. nested ordered item\n      - a bullet nested under an ordered item\n3. ordered three\n",
    );

    let rev = ed.revision();
    let decos = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    let numbers: Vec<u64> = decos
        .iter()
        .filter_map(|d| match d {
            Decoration::Widget {
                kind: WidgetKind::Ordered { number, .. },
                ..
            } => Some(*number),
            _ => None,
        })
        .collect();
    // Document order: "ordered one" (top-level list, #1), "ordered two"
    // (its own single-item nested list, #1), "nested ordered item" (a
    // further-nested single-item list, #1), "ordered three" (back at
    // top-level, second item of the SAME list as "ordered one" -> #2, even
    // though its raw digit is still "3").
    assert_eq!(numbers, vec![1, 1, 1, 2]);
}

#[test]
fn indent_joining_an_open_ordered_sublist_keeps_its_number() {
    // "2. b" lands at the same column as the open "   1. a1" sublist, same
    // "." family: it JOINS that list — any number is valid there, no rewrite.
    let mut ed = Editor::new(1);
    ed.load("1. a\n   1. a1\n2. b\n");
    run(&mut ed, Command::IndentList { from: 17, to: 17 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n   1. a1\n   2. b\n");
    assert_list_item_line(&ed, "2. b");
}

#[test]
fn indent_onto_a_bullet_family_at_the_same_column_rewrites_to_one() {
    // Same landing column but a BULLET list is open there: an ordered marker
    // starts a NEW list (lists are homogeneous per marker family), which a
    // non-1 number cannot do in paragraph-interruption position → rewrite.
    let mut ed = Editor::new(1);
    ed.load("1. a\n   - a1\n2. b\n");
    run(&mut ed, Command::IndentList { from: 16, to: 16 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n   - a1\n   1. b\n");
    assert_list_item_line(&ed, "1. b");
}

#[test]
fn outdent_rejoining_an_open_ordered_list_keeps_its_number() {
    // Shift-Tab on "   3. c": after skipping the deeper "1. b"/"1. b1"
    // lines, the landing line at the target column is "1. a" — ordered,
    // same family → "3. c" REJOINS the outer open list, no rewrite.
    let mut ed = Editor::new(1);
    ed.load("1. a\n   1. b\n      1. b1\n   3. c\n");
    run(&mut ed, Command::OutdentList { from: 31, to: 31 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n   1. b\n      1. b1\n3. c\n");
    assert_list_item_line(&ed, "3. c");
}

#[test]
fn outdent_onto_a_bullet_parent_rewrites_to_one() {
    // "2. c" outdents to the bullet parent's column: different family →
    // starting a new ordered list there needs number 1 → rewrite.
    let mut ed = Editor::new(1);
    ed.load("- a\n  1. b\n  2. c\n");
    run(&mut ed, Command::OutdentList { from: 16, to: 16 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n  1. b\n1. c\n");
    assert_list_item_line(&ed, "1. c");
}

// ------------------------------- below-context interruption guard --
//
// The edit can change the parse context of a line BELOW the affected set
// that the command never touched: outdenting a nested bullet to top level
// puts a following non-1 ordered sibling against the new bullet list
// instead of the outer ordered list it used to continue — without the
// guard it de-lists (loses all list decorations). The guard runs the same
// landing-scan check on the first unaffected item line below the affected
// set (skipping adopted descendants).

/// The demo doc's list torture block (apps/web-demo/src/sample-doc.ts).
const ORDERED_TORTURE: &str = "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n   - [x] a task nested under an ordered item\n3. ordered three\n";

#[test]
fn outdent_bullet_rewrites_the_below_ordered_sibling_it_recontexted() {
    let mut ed = Editor::new(1);
    ed.load(ORDERED_TORTURE);
    let pos = ORDERED_TORTURE.find("a bullet").unwrap(); // all-ASCII: byte == CU
    run(&mut ed, Command::OutdentList { from: pos, to: pos }).unwrap();
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n2. ordered two\n   1. nested ordered item\n- a bullet nested under an ordered item\n   - [x] a task nested under an ordered item\n1. ordered three\n",
        "the untouched '3.' line is guard-rewritten to '1.'; the task sibling is adopted"
    );
    // The below line the command never touched is still a list item...
    assert_list_item_line(&ed, "1. ordered three");
    // ...and the equal-column task sibling became the outdented bullet's
    // CHILD (adoption — intended standard outliner behavior).
    assert_list_item_line(&ed, "- [x] a task nested under an ordered item");
}

#[test]
fn outdent_task_rewrites_the_below_ordered_sibling_the_same_way() {
    let mut ed = Editor::new(1);
    ed.load(ORDERED_TORTURE);
    let pos = ORDERED_TORTURE.find("a task").unwrap();
    run(&mut ed, Command::OutdentList { from: pos, to: pos }).unwrap();
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n- [x] a task nested under an ordered item\n1. ordered three\n",
    );
    assert_list_item_line(&ed, "1. ordered three");
}

#[test]
fn below_line_untouched_when_no_interruption_hazard() {
    // A bullet below: bullets always interrupt — never rewritten.
    let mut ed = Editor::new(1);
    ed.load("1. a\n   - b\n- c\n");
    let pos = "1. a\n   - b\n- c\n".find('b').unwrap();
    run(&mut ed, Command::OutdentList { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n- b\n- c\n", "bullet below stays byte-identical");

    // An ordered "1." below: already safe in any position — untouched.
    let mut ed = Editor::new(1);
    ed.load("1. a\n   - b\n1. c\n");
    run(&mut ed, Command::OutdentList { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n- b\n1. c\n", "'1.' below stays byte-identical");

    // The below line lands on the moved line itself at an equal column with
    // the same ordered flavor: it continues that open list — untouched.
    let mut ed = Editor::new(1);
    ed.load("1. a\n   1. b\n2. c\n");
    run(&mut ed, Command::OutdentList { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n1. b\n2. c\n", "same-flavor continuation keeps its number");
}

#[test]
fn user_sequence_outdent_twice_then_indent_twice_on_the_nested_bullet() {
    // The user's literal repro sequence, chained through ONE editor; run()
    // asserts the whole-doc itemness invariant at every step.
    let mut ed = Editor::new(1);
    ed.load(ORDERED_TORTURE);
    let mut pos = ORDERED_TORTURE.find("a bullet").unwrap();
    let track = |c: &CoreChange, pos: usize| c.selection.map_or(pos, |s| s.head);

    // Shift-Tab #1: bullet to top level; task adopted; "3." guard-rewritten.
    let c = run(&mut ed, Command::OutdentList { from: pos, to: pos }).unwrap();
    pos = track(&c, pos);
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n2. ordered two\n   1. nested ordered item\n- a bullet nested under an ordered item\n   - [x] a task nested under an ordered item\n1. ordered three\n"
    );

    // Shift-Tab #2: already top level — applies but is a no-op.
    let c = run(&mut ed, Command::OutdentList { from: pos, to: pos }).unwrap();
    assert!(c.splices.is_empty(), "top level: no-op, not a fallback");
    pos = track(&c, pos);

    // Tab #1: re-nests under "2. ordered two" (+3); the adopted task moves
    // along as the bullet's subtree.
    let c = run(&mut ed, Command::IndentList { from: pos, to: pos }).unwrap();
    pos = track(&c, pos);
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n      - [x] a task nested under an ordered item\n1. ordered three\n"
    );

    // Tab #2: nests under "   1. nested ordered item" (+3), subtree carried.
    let c = run(&mut ed, Command::IndentList { from: pos, to: pos }).unwrap();
    pos = track(&c, pos);
    let _ = pos;
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n2. ordered two\n   1. nested ordered item\n      - a bullet nested under an ordered item\n         - [x] a task nested under an ordered item\n1. ordered three\n"
    );
}

// ------------------------------------------------------------------ enter --
//
// Construct-aware Enter (boundary v0.3, research/07 §1.3/§1.4/§2.1):
// continues a list marker/quote prefix on non-empty content, exits an EMPTY
// one in a SINGLE press. Every applicable case produces real splices —
// `enter` returns `None` or `Some` with a change, never a no-op `Some` like
// indentList/outdentList's applies-but-no-movement case.

/// Byte range of the Nth (0-based) physical line in `text` (terminator
/// excluded) — for asserting itemness of a line whose CONTENT can't
/// disambiguate it from another line (an emptied marker reads just "- "
/// regardless of which line it's on).
fn nth_line_range(text: &str, n: usize) -> std::ops::Range<usize> {
    let mut start = 0;
    for _ in 0..n {
        start = text[start..]
            .find('\n')
            .map(|i| start + i + 1)
            .unwrap_or_else(|| panic!("line {n} out of range in {text:?}"));
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    start..end
}

/// Whole-document itemness invariant, keyed by LINE INDEX rather than needle
/// text (see `nth_line_range`): the line still carries its own `ListMarker`
/// after reparse.
fn assert_list_item_at_line(text: &str, line_idx: usize) {
    let range = nth_line_range(text, line_idx);
    let nodes = oxidown_core::parser::parse(text);
    assert!(
        nodes.iter().any(|n| {
            matches!(n.kind, oxidown_core::parser::NodeKind::ListMarker { .. })
                && n.extent.start >= range.start
                && n.extent.start < range.end
        }),
        "line {line_idx} ({:?}) must still parse as a list item in {text:?}",
        &text[range]
    );
}

// -- continue -----------------------------------------------------------

#[test]
fn enter_continue_bullet_flavors_match_source_glyph() {
    for glyph in ["-", "*", "+"] {
        let doc = format!("{glyph} a\n");
        let mut ed = Editor::new(1);
        ed.load(&doc);
        let pos = doc.find('a').unwrap() + 1; // right after "a"
        run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
        assert_eq!(ed.get_text(), format!("{glyph} a\n{glyph} \n"));
        assert_list_item_at_line(&ed.get_text(), 0);
    }
}

#[test]
fn enter_continue_ordered_increments_raw_source_digits() {
    let mut ed = Editor::new(1);
    ed.load("6) a\n");
    let pos = "6) a".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "6) a\n7) \n");
}

#[test]
fn enter_continue_ordered_grows_digit_width() {
    // "9." -> "10. " — digit width grows naturally, no zero-padding.
    let mut ed = Editor::new(1);
    ed.load("9. a\n");
    let pos = "9. a".len();
    let change = run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "9. a\n10. \n");
    assert_list_item_at_line(&ed.get_text(), 1);
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (9, 9), "cursor lands right after the inserted '10. '");
}

#[test]
fn enter_continue_ordered_clamps_at_commonmarks_nine_digit_cap() {
    // CommonMark caps ordered-list numbers at 9 digits: "1000000000. "
    // would not parse as a list item at all, so CONTINUE clamps at the cap
    // and repeats "999999999." rather than emit an unparseable marker.
    let mut ed = Editor::new(1);
    ed.load("999999999. a\n");
    let pos = "999999999. a".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "999999999. a\n999999999. \n");
    assert_list_item_at_line(&ed.get_text(), 1);

    // One below the cap still increments normally.
    let mut ed = Editor::new(1);
    ed.load("999999998. a\n");
    let pos = "999999998. a".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "999999998. a\n999999999. \n");
    assert_list_item_at_line(&ed.get_text(), 1);
}

#[test]
fn enter_continue_task_adds_unchecked_brackets_regardless_of_source_state() {
    let mut ed = Editor::new(1);
    ed.load("- [ ] a\n");
    let pos = "- [ ] a".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "- [ ] a\n- [ ] \n");

    let mut ed = Editor::new(1);
    ed.load("- [x] a\n");
    let pos = "- [x] a".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "- [x] a\n- [ ] \n", "new items always start unchecked");
}

#[test]
fn enter_continue_ordered_task_increments_digits_and_adds_brackets() {
    let mut ed = Editor::new(1);
    ed.load("1. [x] a\n");
    let pos = "1. [x] a".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "1. [x] a\n2. [ ] \n");
}

#[test]
fn enter_continue_preserves_quote_prefix_for_a_list_inside_a_quote() {
    let mut ed = Editor::new(1);
    ed.load("> - a\n");
    let pos = "> - a".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "> - a\n> - \n");
}

#[test]
fn enter_continue_plain_quote_line() {
    let mut ed = Editor::new(1);
    ed.load("> text\n");
    let pos = "> text".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "> text\n> \n");
}

#[test]
fn enter_continue_nested_item_continues_at_its_own_indent() {
    let mut ed = Editor::new(1);
    ed.load("- a\n  - b\n");
    let pos = "- a\n  - b".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "- a\n  - b\n  - \n");
    assert_list_item_at_line(&ed.get_text(), 1);
}

#[test]
fn enter_continue_mid_line_splits_the_item() {
    let mut ed = Editor::new(1);
    ed.load("- hello world\n");
    let pos = "- hello ".len(); // right before "world"
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "- hello \n- world\n", "trailing text becomes the new item's content");
    assert_list_item_at_line(&ed.get_text(), 1);
}

#[test]
fn enter_continue_selection_deletes_and_continues_in_one_batch() {
    let mut ed = Editor::new(1);
    ed.load("- hello world\n");
    let from = "- ".len();
    let to = "- hello".len();
    let change = run(&mut ed, Command::Enter { from, to }).unwrap();
    assert_eq!(ed.get_text(), "- \n-  world\n");
    assert_eq!(change.splices.len(), 1, "delete + continue is one splice");
    let sel = change.selection.unwrap();
    assert_eq!(sel.anchor, sel.head);
}

// -- exit/outdent ---------------------------------------------------------

#[test]
fn enter_exit_basic_nested_item_outdents_one_level() {
    // The realistic continue-created shape: Enter on "  - b" made "  - ",
    // Enter again outdents it one level. (A lone "  - " directly under
    // paragraph content would parse as a setext-H2 underline per CommonMark
    // — not an item — so this is the honest empty-nested-item fixture.)
    let mut ed = Editor::new(1);
    ed.load("- a\n  - b\n  - \n");
    let pos = "- a\n  - b\n  - ".len();
    let change = run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "- a\n  - b\n- \n");
    assert!(
        change.splices.iter().all(|s| !s.insert.contains('\n')),
        "exit never inserts a newline — one press is one level of escape"
    );
    assert_list_item_at_line(&ed.get_text(), 2);
}

#[test]
fn enter_exit_nested_item_outdents_and_rewrites_a_below_sibling_the_edit_recontexted() {
    // "   - [ ] " is an EMPTY nested task under "2. b" (a task item's
    // checkbox is GFM content, so unlike a bare "   - " — which CommonMark
    // reads as a setext underline for "b", not an item — it really is an
    // empty item both before and after the outdent). Outdenting it to top
    // level re-anchors the below "3. c" against the new top-level BULLET
    // list — where the guard's landing-scan rule says a non-1 ordered marker
    // would START a new list — so the below-context guard fires and rewrites
    // "3." -> "1." in the SAME batch/undo unit, exactly like `outdentList`'s
    // own guard on this shape.
    let mut ed = Editor::new(1);
    ed.load("1. a\n2. b\n   - [ ] x\n   - [ ] \n3. c\n");
    let pos = "1. a\n2. b\n   - [ ] x\n   - [ ] ".len();
    let change = run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n2. b\n   - [ ] x\n- [ ] \n1. c\n");
    assert_eq!(
        change.splices.len(),
        2,
        "the outdent's whitespace removal + the below-line digit rewrite"
    );
    assert_list_item_at_line(&ed.get_text(), 3);
    assert_list_item_at_line(&ed.get_text(), 4);
}

#[test]
fn enter_exit_selection_past_line_end_still_rewrites_the_below_sibling() {
    // Same repro shape as the collapsed-cursor control above, plus one more
    // item line ("   - [ ] y") between the empty item and "3. c". Enter is
    // pressed with a SELECTION from the empty item's content start through
    // the END of the "   - [ ] y" line (consuming it outright, `to` past
    // `L`'s own end). The below-line guard must still run — scanning from
    // past the line CONTAINING `to`, not just past `L`'s own end — and
    // land on the same below-context line ("3. c") the collapsed-cursor
    // control does, producing the BYTE-IDENTICAL rewrite ("3." -> "1.").
    // `run()` isn't used here: the consumed "   - [ ] y" line losing its own
    // itemness is an explicit, intended consequence of the user's own
    // selection (not the silent side effect the whole-doc invariant guards
    // against), so `assert_enter_itemness`'s single-line exemption doesn't
    // apply and would false-positive on this test.
    let mut ed = Editor::new(1);
    let doc = "1. a\n2. b\n   - [ ] x\n   - [ ] \n   - [ ] y\n3. c\n";
    ed.load(doc);
    let from = "1. a\n2. b\n   - [ ] x\n   - [ ] ".len();
    let to = "1. a\n2. b\n   - [ ] x\n   - [ ] \n   - [ ] y".len();
    let change = ed.command(Command::Enter { from, to }).unwrap().unwrap();
    assert_eq!(
        ed.get_text(),
        "1. a\n2. b\n   - [ ] x\n- [ ] \n1. c\n",
        "same rewrite as the collapsed-cursor control"
    );
    assert_eq!(
        change.splices.len(),
        3,
        "outdent whitespace removal + the selection delete + the below-line digit rewrite"
    );
    let rewrite = change.splices.last().unwrap();
    assert_eq!(
        (rewrite.at, rewrite.delete, rewrite.insert.as_str()),
        (doc.find("3. c").unwrap(), 1, "1"),
        "the digit rewrite lands exactly on the below sibling's marker digits"
    );
    assert_list_item_at_line(&ed.get_text(), 3);
    assert_list_item_at_line(&ed.get_text(), 4);
}

#[test]
fn enter_exit_selection_past_line_end_below_line_untouched_when_no_hazard() {
    // Same selection shape as the test above, but the below line is either a
    // bullet (always safe to interrupt) or an already-"1."-numbered ordered
    // item (also always safe): the below-line guard still runs (scanning
    // from past the consumed "   - [ ] y" line) but finds nothing to
    // rewrite, same as the collapsed-cursor no-hazard control.
    let from = "1. a\n2. b\n   - [ ] x\n   - [ ] ".len();
    let to = "1. a\n2. b\n   - [ ] x\n   - [ ] \n   - [ ] y".len();

    let mut ed = Editor::new(1);
    ed.load("1. a\n2. b\n   - [ ] x\n   - [ ] \n   - [ ] y\n- c\n");
    let change = ed.command(Command::Enter { from, to }).unwrap().unwrap();
    assert_eq!(
        ed.get_text(),
        "1. a\n2. b\n   - [ ] x\n- [ ] \n- c\n",
        "bullet below stays byte-identical"
    );
    assert_eq!(change.splices.len(), 2, "no below-line rewrite needed");

    let mut ed = Editor::new(1);
    ed.load("1. a\n2. b\n   - [ ] x\n   - [ ] \n   - [ ] y\n1. c\n");
    let change = ed.command(Command::Enter { from, to }).unwrap().unwrap();
    assert_eq!(
        ed.get_text(),
        "1. a\n2. b\n   - [ ] x\n- [ ] \n1. c\n",
        "'1.' below stays byte-identical"
    );
    assert_eq!(change.splices.len(), 2, "no below-line rewrite needed");
}

#[test]
fn enter_exit_empty_top_level_item_clears_the_marker() {
    let mut ed = Editor::new(1);
    ed.load("- a\n- \n");
    let pos = "- a\n- ".len();
    let change = run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "- a\n\n");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (4, 4));
}

#[test]
fn enter_exit_empty_top_level_task_clears_marker_and_brackets() {
    let mut ed = Editor::new(1);
    ed.load("- [ ] a\n- [ ] \n");
    let pos = "- [ ] a\n- [ ] ".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "- [ ] a\n\n");
}

#[test]
fn enter_exit_quote_drops_one_level_per_press() {
    let mut ed = Editor::new(1);
    ed.load("> > x\n> > \n");
    let pos = "> > x\n> > ".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "> > x\n> \n", "first press: '> > ' -> '> '");

    let pos2 = "> > x\n> ".len();
    run(&mut ed, Command::Enter { from: pos2, to: pos2 }).unwrap();
    assert_eq!(ed.get_text(), "> > x\n\n", "second press: '> ' -> plain");
}

#[test]
fn enter_exit_empty_top_level_item_inside_a_quote_keeps_the_quote_prefix() {
    let mut ed = Editor::new(1);
    ed.load("> - a\n> - \n");
    let pos = "> - a\n> - ".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "> - a\n> \n", "marker clears but '> ' stays for the next press");
}

#[test]
fn enter_exit_empty_nested_item_inside_a_quote_outdents_within_the_quote() {
    // Rule 6 (mixed): the outdent keeps the quote prefix intact — the item
    // steps out one LIST level, not out of the quote.
    let mut ed = Editor::new(1);
    ed.load("> - a\n>   - b\n>   - \n");
    let pos = "> - a\n>   - b\n>   - ".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "> - a\n>   - b\n> - \n");
}

// -- null cases -----------------------------------------------------------

#[test]
fn enter_null_on_plain_paragraph() {
    let mut ed = Editor::new(1);
    ed.load("plain text\n");
    assert!(ed.command(Command::Enter { from: 5, to: 5 }).unwrap().is_none());
}

#[test]
fn enter_null_when_cursor_sits_inside_the_marker_prefix() {
    let mut ed = Editor::new(1);
    ed.load("- item\n");
    // Between "-" and the content start.
    assert!(ed.command(Command::Enter { from: 1, to: 1 }).unwrap().is_none());

    let mut ed = Editor::new(1);
    ed.load("> text\n");
    // Inside the quote's own marker run.
    assert!(ed.command(Command::Enter { from: 1, to: 1 }).unwrap().is_none());
}

#[test]
fn enter_null_on_heading_line() {
    let mut ed = Editor::new(1);
    ed.load("# Heading\n");
    assert!(ed.command(Command::Enter { from: 5, to: 5 }).unwrap().is_none());
}

// -- undo -------------------------------------------------------------------

#[test]
fn enter_is_a_single_undo_unit() {
    let mut ed = Editor::new(1);
    ed.load("- a\n");
    let (undo_depth, _) = ed.history_depths();
    let pos = "- a".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.history_depths().0, undo_depth + 1, "one undo unit for the whole press");

    let mut mirror = ed.get_text();
    let u = ed.undo().unwrap();
    apply_to_mirror(&mut mirror, &u.splices);
    assert_eq!(ed.get_text(), "- a\n");
    assert_eq!(mirror, ed.get_text());
}

#[test]
fn enter_presses_never_coalesce() {
    let mut ed = Editor::new(1);
    ed.load("- a\n");
    let (undo0, _) = ed.history_depths();
    let pos = "- a".len();
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert_eq!(ed.get_text(), "- a\n- \n");
    let (undo1, _) = ed.history_depths();
    assert_eq!(undo1, undo0 + 1);

    // Second press, immediately after (no coalescing window applies to
    // commands at all — "command" origin never coalesces).
    let pos2 = ed.get_text().len() - 1; // content start of the new empty item
    run(&mut ed, Command::Enter { from: pos2, to: pos2 }).unwrap();
    let (undo2, _) = ed.history_depths();
    assert_eq!(undo2, undo1 + 1, "second press is its own undo unit");
}

#[test]
fn enter_lookups_resolve_deep_in_a_large_overlay() {
    // Regression guard for the binary-search conversion of `enter`'s
    // marker-node / TaskWidget / BlockQuoteLine lookups: identical behavior
    // to the linear scans, exercised far from the overlay's start so a
    // wrong partition window would miss the nodes.
    let mut doc = String::new();
    for i in 0..200 {
        doc.push_str(&format!("- item {i}\n"));
    }
    doc.push_str("- [x] task\n");
    doc.push_str("> \n");
    let mut ed = Editor::new(1);
    ed.load(&doc);

    // Continue the task item (ListMarker + TaskWidget lookups).
    let pos = doc.find("task").unwrap() + 4;
    run(&mut ed, Command::Enter { from: pos, to: pos }).unwrap();
    assert!(ed.get_text().contains("- [x] task\n- [ ] \n"), "{:?}", ed.get_text());

    // Exit the empty quote line (BlockQuoteLine lookup, rule 5).
    let text = ed.get_text();
    let qpos = text.rfind("> ").unwrap() + 2;
    run(&mut ed, Command::Enter { from: qpos, to: qpos }).unwrap();
    assert!(ed.get_text().ends_with("- [ ] \n\n"), "{:?}", ed.get_text());
}

#[test]
fn user_sequence_outdent_twice_then_indent_twice_on_the_nested_task() {
    let mut ed = Editor::new(1);
    ed.load(ORDERED_TORTURE);
    let mut pos = ORDERED_TORTURE.find("a task").unwrap();
    let track = |c: &CoreChange, pos: usize| c.selection.map_or(pos, |s| s.head);

    // Shift-Tab #1: task to top level; "3." guard-rewritten.
    let c = run(&mut ed, Command::OutdentList { from: pos, to: pos }).unwrap();
    pos = track(&c, pos);
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n- [x] a task nested under an ordered item\n1. ordered three\n"
    );

    // Shift-Tab #2: no-op at top level.
    let c = run(&mut ed, Command::OutdentList { from: pos, to: pos }).unwrap();
    assert!(c.splices.is_empty());
    pos = track(&c, pos);

    // Tab #1: back under "2. ordered two" (+3) — the original shape modulo
    // the "3." → "1." rewrite.
    let c = run(&mut ed, Command::IndentList { from: pos, to: pos }).unwrap();
    pos = track(&c, pos);
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n   - [x] a task nested under an ordered item\n1. ordered three\n"
    );

    // Tab #2: nests under the bullet sibling (+2 — bullet token width).
    let c = run(&mut ed, Command::IndentList { from: pos, to: pos }).unwrap();
    pos = track(&c, pos);
    let _ = pos;
    assert_eq!(
        ed.get_text(),
        "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n     - [x] a task nested under an ordered item\n1. ordered three\n"
    );
}

// -------------------------------------------------- toggleQuote (v0.6) --

#[test]
fn toggle_quote_adds_one_level_to_a_plain_paragraph() {
    let mut ed = Editor::new(1);
    ed.load("alpha");
    let change = run(&mut ed, Command::ToggleQuote { from: 2, to: 2 }).unwrap();
    assert_eq!(ed.get_text(), "> alpha");
    // Cursor glued to its character: shifted right by the inserted marker.
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (4, 4));
}

#[test]
fn toggle_quote_add_covers_blank_lines_to_keep_the_quote_contiguous() {
    let mut ed = Editor::new(1);
    ed.load("alpha\n\nbeta");
    run(&mut ed, Command::ToggleQuote { from: 0, to: 11 }).unwrap();
    assert_eq!(ed.get_text(), "> alpha\n> \n> beta");
    // One contiguous quote: every line is a depth-1 BlockQuoteLine.
    let quoted = ed
        .overlay_nodes()
        .iter()
        .filter(|n| matches!(n.kind, oxidown_core::parser::NodeKind::BlockQuoteLine(1)))
        .count();
    assert_eq!(quoted, 3);
}

#[test]
fn toggle_quote_removes_one_level_when_every_line_is_quoted() {
    let mut ed = Editor::new(1);
    ed.load("> alpha\n> beta");
    run(&mut ed, Command::ToggleQuote { from: 0, to: 14 }).unwrap();
    assert_eq!(ed.get_text(), "alpha\nbeta");
}

#[test]
fn toggle_quote_remove_is_stepped_one_level_per_press() {
    let mut ed = Editor::new(1);
    ed.load("> > alpha");
    run(&mut ed, Command::ToggleQuote { from: 5, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "> alpha", "first press: innermost level only");
    run(&mut ed, Command::ToggleQuote { from: 3, to: 3 }).unwrap();
    assert_eq!(ed.get_text(), "alpha", "second press: back to plain");
}

#[test]
fn toggle_quote_mixed_selection_adds_a_level_to_every_line() {
    // "beta" is its own paragraph (the blank line ends the quote — a
    // directly-adjacent unmarked line would be a LAZY CONTINUATION, i.e.
    // already quoted, and the press would step the quote down instead).
    let mut ed = Editor::new(1);
    ed.load("> alpha\n\nbeta");
    run(&mut ed, Command::ToggleQuote { from: 0, to: 13 }).unwrap();
    assert_eq!(ed.get_text(), "> > alpha\n> \n> beta");
}

#[test]
fn toggle_quote_add_and_remove_round_trip_on_lists_and_headings() {
    let mut ed = Editor::new(1);
    let original = "- item\n# head";
    ed.load(original);
    run(&mut ed, Command::ToggleQuote { from: 0, to: 13 }).unwrap();
    assert_eq!(ed.get_text(), "> - item\n> # head");
    // The quoted constructs survive the round trip byte-identically.
    let len = ed.doc_len_utf16();
    run(&mut ed, Command::ToggleQuote { from: 0, to: len }).unwrap();
    assert_eq!(ed.get_text(), original);
}

#[test]
fn toggle_quote_remove_skips_lazy_continuation_lines() {
    // "lazy" carries quote depth 1 but no marker run of its own; the press
    // unwinds only the marked line and leaves the lazy line's bytes alone.
    let mut ed = Editor::new(1);
    ed.load("> alpha\nlazy");
    run(&mut ed, Command::ToggleQuote { from: 0, to: 12 }).unwrap();
    assert_eq!(ed.get_text(), "alpha\nlazy");
}

#[test]
fn toggle_quote_all_lazy_selection_is_an_applies_but_no_op() {
    let mut ed = Editor::new(1);
    ed.load("> alpha\nlazy");
    let rev = ed.revision();
    let (undo_before, _) = ed.history_depths();
    // The lazy line alone: quoted (depth 1) but marker-less.
    let change = run(&mut ed, Command::ToggleQuote { from: 9, to: 9 }).unwrap();
    assert!(change.splices.is_empty());
    assert_eq!(ed.revision(), rev, "no revision bump for the no-op");
    assert_eq!(ed.history_depths().0, undo_before, "no undo unit for the no-op");
}

#[test]
fn toggle_quote_add_rewrites_the_below_ordered_sibling_it_recontexted() {
    // Quoting "2. b" interrupts the list; "3. c" directly below would be
    // lazy-continuation text of the quote's open paragraph (a non-1 ordered
    // marker cannot start a list there) — the guard rewrites it to "1." so
    // its itemness survives (run() asserts the whole-document invariant).
    let mut ed = Editor::new(1);
    ed.load("1. a\n2. b\n3. c");
    run(&mut ed, Command::ToggleQuote { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n> 2. b\n1. c");
}

#[test]
fn toggle_quote_add_below_guard_skips_when_a_blank_line_separates() {
    let mut ed = Editor::new(1);
    ed.load("alpha\n\n3. c");
    run(&mut ed, Command::ToggleQuote { from: 2, to: 2 }).unwrap();
    // The blank line stops the scan: "3. c" starts its own list regardless.
    assert_eq!(ed.get_text(), "> alpha\n\n3. c");
}

#[test]
fn toggle_quote_remove_rewrites_a_dequoted_non_one_ordered_item_that_no_longer_joins() {
    // De-quoting "2. b" alone drops it after the remaining quote's open
    // paragraph, where a non-1 ordered marker cannot start a list — the
    // digits rewrite to 1 (the moved line is the user's own gesture target,
    // same contract as outdentList's interruption rewrite).
    let mut ed = Editor::new(1);
    ed.load("> 1. a\n> 2. b");
    run(&mut ed, Command::ToggleQuote { from: 9, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "> 1. a\n1. b");
}

#[test]
fn toggle_quote_remove_keeps_digits_when_the_dequoted_run_moves_together() {
    let mut ed = Editor::new(1);
    ed.load("> 1. a\n> 2. b");
    run(&mut ed, Command::ToggleQuote { from: 0, to: 13 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n2. b", "the whole run de-quotes: b still joins a");
}

#[test]
fn toggle_quote_is_a_single_undo_unit() {
    let mut ed = Editor::new(1);
    ed.load("alpha\nbeta");
    run(&mut ed, Command::ToggleQuote { from: 0, to: 10 }).unwrap();
    assert_eq!(ed.get_text(), "> alpha\n> beta");
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "alpha\nbeta", "one undo restores every line");
}

// --------------------------------------------------- toggleLink (v0.6) --

#[test]
fn toggle_link_wraps_a_selection_with_the_cursor_in_the_url_slot() {
    let mut ed = Editor::new(1);
    ed.load("hello world");
    let change = run(&mut ed, Command::ToggleLink { from: 6, to: 11 }).unwrap();
    assert_eq!(ed.get_text(), "hello [world]()");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (14, 14), "cursor between the parens");
}

#[test]
fn toggle_link_empty_range_inserts_the_skeleton_with_the_cursor_in_the_text_slot() {
    let mut ed = Editor::new(1);
    ed.load("ab");
    let change = run(&mut ed, Command::ToggleLink { from: 1, to: 1 }).unwrap();
    assert_eq!(ed.get_text(), "a[]()b");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (2, 2), "cursor between the brackets");
}

#[test]
fn toggle_link_unwraps_an_intersected_link_keeping_the_text() {
    let mut ed = Editor::new(1);
    ed.load("see [docs](https://x.y) now");
    let change = run(&mut ed, Command::ToggleLink { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "see docs now");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (4, 8), "selection covers the surviving text");
}

#[test]
fn toggle_link_unwraps_an_autolink_keeping_the_destination_text() {
    let mut ed = Editor::new(1);
    ed.load("go <https://x.y> now");
    let change = run(&mut ed, Command::ToggleLink { from: 5, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "go https://x.y now");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (3, 14));
}

#[test]
fn toggle_link_multi_line_range_is_null() {
    let mut ed = Editor::new(1);
    ed.load("ab\ncd");
    assert!(ed.command(Command::ToggleLink { from: 0, to: 4 }).unwrap().is_none());
    assert_eq!(ed.get_text(), "ab\ncd", "no mutation");
}

#[test]
fn toggle_link_refuses_code_contexts() {
    let mut ed = Editor::new(1);
    ed.load("`code`");
    // Endpoint strictly inside the code span's extent.
    assert!(ed.command(Command::ToggleLink { from: 2, to: 4 }).unwrap().is_none());
    ed.load("```\nbody\n```");
    // Range touching a fenced-code line.
    assert!(ed.command(Command::ToggleLink { from: 5, to: 7 }).unwrap().is_none());
}

#[test]
fn toggle_link_wrap_then_unwrap_round_trips_the_text() {
    let mut ed = Editor::new(1);
    ed.load("hello world");
    run(&mut ed, Command::ToggleLink { from: 6, to: 11 }).unwrap();
    assert_eq!(ed.get_text(), "hello [world]()");
    // Second toggle from inside the link unwraps (URL slot is empty — the
    // documented asymmetry: the wrap can't restore a URL the unwrap threw
    // away, but the TEXT round-trips byte-identically).
    run(&mut ed, Command::ToggleLink { from: 8, to: 8 }).unwrap();
    assert_eq!(ed.get_text(), "hello world");
}

#[test]
fn toggle_link_is_a_single_undo_unit() {
    let mut ed = Editor::new(1);
    ed.load("hello world");
    run(&mut ed, Command::ToggleLink { from: 6, to: 11 }).unwrap();
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "hello world");
}

// ------------------------------- toggleBulletList / toggleOrderedList --

#[test]
fn toggle_bullet_converts_plain_lines() {
    let mut ed = Editor::new(1);
    ed.load("alpha\nbeta");
    let change = run(&mut ed, Command::ToggleBulletList { from: 0, to: 10 }).unwrap();
    assert_eq!(ed.get_text(), "- alpha\n- beta");
    // Selection glued: both endpoints shifted right by their line's marker.
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (2, 14));
}

#[test]
fn toggle_bullet_strips_an_all_bullet_selection_back_to_plain_lines() {
    let mut ed = Editor::new(1);
    ed.load("- alpha\n- beta");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 14 }).unwrap();
    assert_eq!(ed.get_text(), "alpha\nbeta", "explicit de-listing gesture");
}

#[test]
fn toggle_bullet_strip_removes_task_brackets_too() {
    // Task items are bullet-flavor, so an all-task selection STRIPS —
    // marker, brackets, and the whitespace between them all go.
    let mut ed = Editor::new(1);
    ed.load("- [ ] a\n- [x] b");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 15 }).unwrap();
    assert_eq!(ed.get_text(), "a\nb");
}

#[test]
fn toggle_bullet_strip_removes_nested_leading_indent() {
    // Leaving 4+ leading spaces behind would re-type the line as indented
    // code — strip produces genuinely plain lines at any depth.
    let mut ed = Editor::new(1);
    ed.load("- a\n    - deep");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 14 }).unwrap();
    assert_eq!(ed.get_text(), "a\ndeep");
}

#[test]
fn toggle_bullet_converts_ordered_items_in_place() {
    let mut ed = Editor::new(1);
    ed.load("1. a\n2. b");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n- b");
}

#[test]
fn toggle_bullet_keeps_task_brackets_when_converting_an_ordered_task() {
    // Pinned decision: ordered task -> bullet stays a task (now
    // bullet-flavored); only conversions TO ordered strip brackets.
    let mut ed = Editor::new(1);
    ed.load("1. [ ] a");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 8 }).unwrap();
    assert_eq!(ed.get_text(), "- [ ] a");
}

#[test]
fn toggle_bullet_mixed_selection_converts_only_the_non_bullet_lines() {
    let mut ed = Editor::new(1);
    ed.load("- a\nplain\n1. c");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 14 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n- plain\n- c");
}

#[test]
fn toggle_bullet_adopts_the_runs_bullet_glyph() {
    // "*" siblings get "*" (a different bullet char would split the list
    // per CommonMark).
    let mut ed = Editor::new(1);
    ed.load("* a\nplain");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "* a\n* plain");
}

#[test]
fn toggle_ordered_converts_plain_lines_with_sequential_digits() {
    let mut ed = Editor::new(1);
    ed.load("alpha\nbeta\ngamma");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 16 }).unwrap();
    assert_eq!(ed.get_text(), "1. alpha\n2. beta\n3. gamma");
}

#[test]
fn toggle_ordered_strips_an_all_ordered_selection() {
    let mut ed = Editor::new(1);
    ed.load("1. a\n2. b");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "a\nb");
}

#[test]
fn toggle_ordered_replaces_bullet_markers_in_place_keeping_indent() {
    let mut ed = Editor::new(1);
    ed.load("- a\n- b");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 7 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n2. b");
}

#[test]
fn toggle_ordered_strips_task_brackets_when_converting() {
    // Pinned decision: converting to ordered strips the checkbox.
    let mut ed = Editor::new(1);
    ed.load("- [ ] a\n- [x] b");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 15 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n2. b");
}

#[test]
fn toggle_ordered_all_ordered_task_selection_strips() {
    // An ordered task is ordered-flavor, so the toggle semantics see an
    // all-target selection and STRIP (marker + brackets).
    let mut ed = Editor::new(1);
    ed.load("1. [ ] a");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 8 }).unwrap();
    assert_eq!(ed.get_text(), "a");
}

#[test]
fn toggle_ordered_numbering_restarts_per_contiguous_run() {
    let mut ed = Editor::new(1);
    ed.load("a\nb\n\nc");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 6 }).unwrap();
    // The blank line passes through untouched and breaks the run.
    assert_eq!(ed.get_text(), "1. a\n2. b\n\n1. c");
}

#[test]
fn toggle_ordered_untouched_lines_feed_the_counter_and_delimiter() {
    // Already-ordered lines keep their raw digits (never-rewrite) but the
    // run adopts value+1 and the ')' flavor so written markers JOIN.
    let mut ed = Editor::new(1);
    ed.load("3) a\nplain");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 10 }).unwrap();
    assert_eq!(ed.get_text(), "3) a\n4) plain");
}

#[test]
fn toggle_ordered_seeds_from_the_item_directly_above_the_selection() {
    let mut ed = Editor::new(1);
    ed.load("1. a\nplain");
    run(&mut ed, Command::ToggleOrderedList { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n2. plain", "mid-list conversion reads sequentially");
}

#[test]
fn toggle_ordered_inside_a_quote_goes_after_the_prefix() {
    let mut ed = Editor::new(1);
    ed.load("> alpha\n> beta");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 14 }).unwrap();
    assert_eq!(ed.get_text(), "> 1. alpha\n> 2. beta");
}

#[test]
fn toggle_bullet_on_a_quoted_list_replaces_markers_in_place() {
    let mut ed = Editor::new(1);
    ed.load("> 1. a\n> 2. b");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 13 }).unwrap();
    assert_eq!(ed.get_text(), "> - a\n> - b");
}

#[test]
fn toggle_ordered_strip_rewrites_the_below_sibling_it_recontexted() {
    // Stripping "1. a\n2. b" leaves plain text; "3. c" below cannot start a
    // list in paragraph-interruption position — the guard rewrites it to
    // "1." (run() asserts its itemness survives).
    let mut ed = Editor::new(1);
    ed.load("1. a\n2. b\n3. c");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "a\nb\n1. c");
}

#[test]
fn toggle_bullet_convert_rewrites_the_below_ordered_sibling() {
    // Re-flavoring "1. a\n2. b" to bullets closes the ordered list; the
    // unaffected "3. c" no longer joins and rewrites to "1.".
    let mut ed = Editor::new(1);
    ed.load("1. a\n2. b\n3. c");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n- b\n1. c");
}

#[test]
fn toggle_list_below_guard_skips_when_a_blank_line_separates() {
    let mut ed = Editor::new(1);
    ed.load("1. a\n\n3. c");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 4 }).unwrap();
    assert_eq!(ed.get_text(), "- a\n\n3. c", "blank line already anchors the list below");
}

#[test]
fn toggle_ordered_below_guard_no_rewrite_when_the_sibling_still_joins() {
    // Converting "plain" between "1. a" and "3. c" leaves one contiguous
    // ordered run — "3. c" joins it (same column, same delimiter), so its
    // digits stay untouched.
    let mut ed = Editor::new(1);
    ed.load("1. a\nplain\n3. c");
    run(&mut ed, Command::ToggleOrderedList { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n2. plain\n3. c");
}

#[test]
fn toggle_list_passes_fenced_code_lines_through_untouched() {
    let mut ed = Editor::new(1);
    ed.load("alpha\n```\ncode\n```\nbeta");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 23 }).unwrap();
    assert_eq!(ed.get_text(), "- alpha\n```\ncode\n```\n- beta");
}

#[test]
fn toggle_list_is_null_when_nothing_is_convertible() {
    let mut ed = Editor::new(1);
    ed.load("```\ncode\n```");
    assert!(ed
        .command(Command::ToggleBulletList { from: 5, to: 5 })
        .unwrap()
        .is_none());
    ed.load("\n\n");
    assert!(ed
        .command(Command::ToggleOrderedList { from: 0, to: 1 })
        .unwrap()
        .is_none());
}

#[test]
fn toggle_bullet_then_toggle_again_round_trips_plain_text() {
    let mut ed = Editor::new(1);
    ed.load("alpha\nbeta");
    run(&mut ed, Command::ToggleBulletList { from: 0, to: 10 }).unwrap();
    assert_eq!(ed.get_text(), "- alpha\n- beta");
    let len = ed.doc_len_utf16();
    run(&mut ed, Command::ToggleBulletList { from: 0, to: len }).unwrap();
    assert_eq!(ed.get_text(), "alpha\nbeta");
}

#[test]
fn toggle_list_commands_are_single_undo_units() {
    let mut ed = Editor::new(1);
    ed.load("a\nb\nc");
    run(&mut ed, Command::ToggleOrderedList { from: 0, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "1. a\n2. b\n3. c");
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "a\nb\nc", "one undo restores every line");
}

#[test]
fn toggle_ordered_selection_maps_with_the_cursor_glued() {
    let mut ed = Editor::new(1);
    ed.load("alpha");
    let change = run(&mut ed, Command::ToggleOrderedList { from: 2, to: 4 }).unwrap();
    assert_eq!(ed.get_text(), "1. alpha");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (5, 7), "still covers 'ph'");
}

// ----------------------------------------------------- insertHr (v0.6) --

#[test]
fn insert_hr_after_a_paragraph_line_guarantees_blank_lines_and_parses_as_hr() {
    // THE setext trap: "---" directly under paragraph text is an H2
    // underline. The splice must produce a shape that reparses as a
    // ThematicBreak with both paragraphs intact.
    let mut ed = Editor::new(1);
    ed.load("para1\npara2");
    let change = run(&mut ed, Command::InsertHr { pos: 2 }).unwrap();
    assert_eq!(ed.get_text(), "para1\n\n---\n\npara2");
    // Reparse assertion: exactly one ThematicBreak node, and the block
    // structure is Paragraph / ThematicBreak / Paragraph.
    let hrs = ed
        .overlay_nodes()
        .iter()
        .filter(|n| matches!(n.kind, oxidown_core::parser::NodeKind::ThematicBreak))
        .count();
    assert_eq!(hrs, 1, "the inserted dashes parse as a ThematicBreak, not setext");
    let kinds: Vec<_> = ed.block_index().blocks().iter().map(|b| b.kind).collect();
    assert_eq!(
        kinds,
        vec![
            oxidown_core::parser::BlockKind::Paragraph,
            oxidown_core::parser::BlockKind::ThematicBreak,
            oxidown_core::parser::BlockKind::Paragraph,
        ]
    );
    // Cursor glued: the insertion is at/after pos, so it does not move.
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (2, 2));
}

#[test]
fn insert_hr_on_a_blank_line_needs_no_extra_separator_above() {
    let mut ed = Editor::new(1);
    ed.load("a\n\nb");
    run(&mut ed, Command::InsertHr { pos: 2 }).unwrap();
    assert_eq!(ed.get_text(), "a\n\n---\n\nb");
    assert!(ed
        .overlay_nodes()
        .iter()
        .any(|n| matches!(n.kind, oxidown_core::parser::NodeKind::ThematicBreak)));
}

#[test]
fn insert_hr_at_the_document_end() {
    let mut ed = Editor::new(1);
    ed.load("para");
    run(&mut ed, Command::InsertHr { pos: 4 }).unwrap();
    assert_eq!(ed.get_text(), "para\n\n---");
    assert!(ed
        .overlay_nodes()
        .iter()
        .any(|n| matches!(n.kind, oxidown_core::parser::NodeKind::ThematicBreak)));
}

#[test]
fn insert_hr_before_an_already_blank_line_below_adds_no_extra_blank() {
    let mut ed = Editor::new(1);
    ed.load("para\n\nnext");
    run(&mut ed, Command::InsertHr { pos: 0 }).unwrap();
    assert_eq!(ed.get_text(), "para\n\n---\n\nnext");
}

#[test]
fn insert_hr_is_null_on_fenced_code_lines() {
    let mut ed = Editor::new(1);
    ed.load("```\ncode\n```");
    assert!(ed.command(Command::InsertHr { pos: 5 }).unwrap().is_none());
    assert_eq!(ed.get_text(), "```\ncode\n```");
}

#[test]
fn insert_hr_is_a_single_undo_unit() {
    let mut ed = Editor::new(1);
    ed.load("para1\npara2");
    run(&mut ed, Command::InsertHr { pos: 2 }).unwrap();
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "para1\npara2");
}

// ----------------------------------------------- toggleCodeBlock (v0.6) --

#[test]
fn toggle_code_block_wraps_the_intersecting_lines_with_the_selection_inside() {
    let mut ed = Editor::new(1);
    ed.load("alpha\nbeta");
    let change = run(&mut ed, Command::ToggleCodeBlock { from: 0, to: 10 }).unwrap();
    assert_eq!(ed.get_text(), "```\nalpha\nbeta\n```");
    let sel = change.selection.unwrap();
    assert_eq!((sel.anchor, sel.head), (4, 14), "selection lands inside the block");
    // Reparse: the body decorates as code-block lines.
    let body = ed
        .overlay_nodes()
        .iter()
        .filter(|n| matches!(n.kind, oxidown_core::parser::NodeKind::CodeBlockLine))
        .count();
    assert_eq!(body, 2);
}

#[test]
fn toggle_code_block_unwraps_an_intersected_block_keeping_the_body() {
    let mut ed = Editor::new(1);
    ed.load("```\nbody\n```");
    run(&mut ed, Command::ToggleCodeBlock { from: 5, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "body");
}

#[test]
fn toggle_code_block_unwrap_works_from_a_fence_line_touch() {
    // Block-level reveal semantics: a touch on either fence counts as
    // inside the block.
    let mut ed = Editor::new(1);
    ed.load("before\n\n```\nbody\n```\n\nafter");
    run(&mut ed, Command::ToggleCodeBlock { from: 9, to: 9 }).unwrap();
    assert_eq!(ed.get_text(), "before\n\nbody\n\nafter");
}

#[test]
fn toggle_code_block_unwraps_an_unterminated_block() {
    let mut ed = Editor::new(1);
    ed.load("```\nbody");
    run(&mut ed, Command::ToggleCodeBlock { from: 5, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "body");
}

#[test]
fn toggle_code_block_unwraps_an_empty_block() {
    let mut ed = Editor::new(1);
    ed.load("```\n```");
    run(&mut ed, Command::ToggleCodeBlock { from: 2, to: 2 }).unwrap();
    assert_eq!(ed.get_text(), "");
}

#[test]
fn toggle_code_block_is_null_in_quote_context() {
    // v1 punt, documented: fences-in-quotes need per-line prefix surgery.
    let mut ed = Editor::new(1);
    ed.load("> quoted");
    assert!(ed
        .command(Command::ToggleCodeBlock { from: 3, to: 3 })
        .unwrap()
        .is_none());
    ed.load("> ```\n> x\n> ```");
    assert!(ed
        .command(Command::ToggleCodeBlock { from: 8, to: 8 })
        .unwrap()
        .is_none());
}

#[test]
fn toggle_code_block_uses_a_longer_fence_when_the_body_leads_with_backticks() {
    let mut ed = Editor::new(1);
    ed.load("``x``");
    run(&mut ed, Command::ToggleCodeBlock { from: 0, to: 5 }).unwrap();
    assert_eq!(ed.get_text(), "```\n``x``\n```");
}

#[test]
fn toggle_code_block_wrap_then_unwrap_round_trips() {
    let mut ed = Editor::new(1);
    ed.load("alpha\nbeta");
    run(&mut ed, Command::ToggleCodeBlock { from: 0, to: 10 }).unwrap();
    assert_eq!(ed.get_text(), "```\nalpha\nbeta\n```");
    run(&mut ed, Command::ToggleCodeBlock { from: 6, to: 6 }).unwrap();
    assert_eq!(ed.get_text(), "alpha\nbeta");
}

#[test]
fn toggle_code_block_is_a_single_undo_unit() {
    let mut ed = Editor::new(1);
    ed.load("alpha\nbeta");
    run(&mut ed, Command::ToggleCodeBlock { from: 0, to: 10 }).unwrap();
    ed.undo().unwrap();
    assert_eq!(ed.get_text(), "alpha\nbeta");
}
