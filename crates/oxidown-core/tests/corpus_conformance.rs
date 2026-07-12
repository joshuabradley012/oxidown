//! Conformance corpus test (plan.md §9): the vendored corpus in
//! `corpus/cases.rs` is asserted, per entry, to:
//!
//! 1. never crash the core (load + full-viewport decorations, no panics);
//! 2. produce a **well-formed** decoration set — see `assert_well_formed`
//!    for the precise invariant, since "non-overlapping" doesn't mean what
//!    it might first suggest (nested marks, e.g. bold-around-italic, overlap
//!    by design);
//! 3. round-trip byte-identically (`load` → `get_text()`).
//!
//! It also differential-tests **block structure** (kinds + document order)
//! against `comrak` as an oracle, for the subset of constructs both parsers
//! agree are comparable — see `known divergences` below.

#[path = "corpus/cases.rs"]
mod cases;

use std::ops::Range;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{parse_document, Arena, Options as ComrakOptions};
use oxidown_core::{Decoration, Editor};
use pulldown_cmark::{Event, Options as PulldownOptions, Parser, Tag};

// ------------------------------------------------------- 1 & 3: no-crash, roundtrip --

#[test]
fn corpus_never_crashes_and_roundtrips_byte_identically() {
    for (i, doc) in cases::CASES.iter().enumerate() {
        let mut ed = Editor::new(1);
        let rev = ed.load(doc);
        assert_eq!(
            ed.get_text().as_bytes(),
            doc.as_bytes(),
            "case {i} not byte-identical: {doc:?}"
        );
        let decos = ed
            .decorations(rev, 0, ed.doc_len_utf16(), &[])
            .unwrap_or_else(|e| panic!("case {i} decorations errored: {e} ({doc:?})"));
        assert_well_formed(&decos, ed.doc_len_utf16(), i, doc);
    }
}

/// Well-formedness invariant for a decoration set:
///
/// - every span is in bounds (`from <= to <= doc_len` / `at <= doc_len`);
/// - `Conceal` spans never overlap each other, and never overlap a
///   `Widget` span — both are "exclusive" claims on their source bytes (hide
///   this / replace this), so two overlapping claims would be a
///   contradiction for a view to render. `Widget` spans likewise never
///   overlap each other.
/// - `Mark` spans MAY overlap (nesting — `**bold *em* bold**` legitimately
///   emits overlapping `strong`/`em` content marks) and are not checked for
///   disjointness; the boundary contract never promised mark disjointness,
///   only that conceal/widget don't double-claim bytes.
fn assert_well_formed(decos: &[Decoration], doc_len: usize, case: usize, doc: &str) {
    let mut exclusive: Vec<Range<usize>> = Vec::new();
    for d in decos {
        match *d {
            Decoration::Mark { from, to, .. } => {
                assert!(from <= to && to <= doc_len, "case {case}: mark oob {d:?} ({doc:?})");
            }
            Decoration::Conceal { from, to } => {
                assert!(from <= to && to <= doc_len, "case {case}: conceal oob {d:?} ({doc:?})");
                exclusive.push(from..to);
            }
            Decoration::Widget { from, to, .. } => {
                assert!(from <= to && to <= doc_len, "case {case}: widget oob {d:?} ({doc:?})");
                exclusive.push(from..to);
            }
            Decoration::Line { at, .. } => assert!(at <= doc_len, "case {case}: line oob {d:?}"),
            Decoration::Block { at, .. } => assert!(at <= doc_len, "case {case}: block oob {d:?}"),
        }
    }
    exclusive.sort_by_key(|r| r.start);
    for w in exclusive.windows(2) {
        assert!(
            w[0].end <= w[1].start,
            "case {case}: overlapping conceal/widget spans {:?} and {:?} ({doc:?})",
            w[0],
            w[1]
        );
    }
}

/// Corpus-wide viewport-seam losslessness: for every case, every single-line
/// viewport window's output is a subset of the full-viewport set, and the
/// union over the whole partition covers it — no decoration is lost at any
/// line-aligned seam (regression: zero-width nodes, e.g. blank fence-body
/// lines, used to vanish from a window starting exactly at them). Multi-line
/// inline nodes legitimately emit in several windows, so the union is
/// compared by containment, not multiset equality.
#[test]
fn corpus_line_aligned_viewport_windows_lose_nothing() {
    for (i, doc) in cases::CASES.iter().enumerate() {
        let mut ed = Editor::new(1);
        let rev = ed.load(doc);
        let full = ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
        // Window bounds in UTF-16 CU: every line start plus the doc end.
        let mut bounds = vec![0usize];
        let mut cu = 0usize;
        for ch in doc.chars() {
            cu += ch.len_utf16();
            if ch == '\n' {
                bounds.push(cu);
            }
        }
        if *bounds.last().unwrap() != cu {
            bounds.push(cu);
        }
        let mut union: Vec<Decoration> = Vec::new();
        for w in bounds.windows(2) {
            let window = ed.decorations(rev, w[0], w[1], &[]).unwrap();
            for d in &window {
                assert!(
                    full.contains(d),
                    "case {i}: window {w:?} invented {d:?} ({doc:?})"
                );
            }
            union.extend(window);
        }
        for d in &full {
            assert!(
                union.contains(d),
                "case {i}: {d:?} lost at a line-aligned viewport seam ({doc:?})"
            );
        }
    }
}

// -------------------------------------------------- 2: differential block structure --

/// Coarse, comparable block-kind vocabulary. Anything not in this list
/// (inline-level nodes, description lists, HEEx, etc.) is intentionally
/// excluded from the differential — this test compares block *structure*,
/// not the full tree.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum BlockKind {
    Paragraph,
    Heading,
    BlockQuote,
    List,
    Item,
    CodeBlock,
    ThematicBreak,
    Table,
    FootnoteDefinition,
    HtmlBlock,
}

fn comrak_blocks(doc: &str) -> Vec<BlockKind> {
    let arena = Arena::new();
    let mut opts = ComrakOptions::default();
    opts.extension.strikethrough = true;
    opts.extension.table = true;
    opts.extension.tasklist = true;
    opts.extension.footnotes = true;
    let root = parse_document(&arena, doc, &opts);
    let mut out = Vec::new();
    collect_comrak(root, &mut out, false);
    out
}

fn block_kind(v: &NodeValue) -> Option<BlockKind> {
    match v {
        NodeValue::Paragraph => Some(BlockKind::Paragraph),
        NodeValue::Heading(_) => Some(BlockKind::Heading),
        NodeValue::BlockQuote => Some(BlockKind::BlockQuote),
        NodeValue::List(_) => Some(BlockKind::List),
        NodeValue::Item(_) | NodeValue::TaskItem(_) => Some(BlockKind::Item),
        NodeValue::CodeBlock(_) => Some(BlockKind::CodeBlock),
        NodeValue::ThematicBreak => Some(BlockKind::ThematicBreak),
        NodeValue::Table(_) => Some(BlockKind::Table),
        NodeValue::FootnoteDefinition(_) => Some(BlockKind::FootnoteDefinition),
        NodeValue::HtmlBlock(_) => Some(BlockKind::HtmlBlock),
        _ => None,
    }
}

/// Known, systematic divergence (not a per-case exclusion), normalized away
/// on *both* sides rather than chased case by case: whether a list item's
/// own direct-content paragraph is represented as an explicit `Paragraph`
/// node/event at all is a tight/loose rendering wrinkle that the two
/// parsers compute differently at the edges (comrak keeps the AST-level
/// `Paragraph` regardless of tightness and defers `<p>`-tag suppression to
/// its HTML renderer; pulldown-cmark's own tightness algorithm sometimes
/// disagrees with comrak's on deeply-nested or task-list shapes about
/// whether a given list is "tight", which flips whether the `Paragraph`
/// event is emitted at all). M1 never decorates paragraphs or the
/// tight/loose distinction — list markers and task widgets are emitted
/// identically either way — so this is irrelevant noise for a *structural*
/// comparison. `collect_comrak`'s `suppress_paragraph` flag is true only for
/// an item's own *direct* children (reset to `false` for anything nested
/// deeper, e.g. a blockquote inside an item still keeps its paragraph);
/// `pulldown_blocks` mirrors this with an equivalent parent-stack check.
fn collect_comrak<'a>(node: &'a AstNode<'a>, out: &mut Vec<BlockKind>, suppress_paragraph: bool) {
    for child in node.children() {
        let kind = block_kind(&child.data.borrow().value);
        if suppress_paragraph && matches!(kind, Some(BlockKind::Paragraph)) {
            continue; // paragraphs only contain inlines: nothing to lose
        }
        match kind {
            Some(BlockKind::List) => {
                out.push(BlockKind::List);
                for item in child.children() {
                    out.push(BlockKind::Item);
                    collect_comrak(item, out, true);
                }
            }
            Some(k) => {
                out.push(k);
                collect_comrak(child, out, false);
            }
            None => collect_comrak(child, out, false),
        }
    }
}

fn pulldown_options() -> PulldownOptions {
    let mut o = PulldownOptions::empty();
    o.insert(PulldownOptions::ENABLE_STRIKETHROUGH);
    o.insert(PulldownOptions::ENABLE_TASKLISTS);
    o.insert(PulldownOptions::ENABLE_TABLES);
    o.insert(PulldownOptions::ENABLE_FOOTNOTES);
    o
}

fn tag_block_kind(tag: &Tag) -> Option<BlockKind> {
    match tag {
        Tag::Paragraph => Some(BlockKind::Paragraph),
        Tag::Heading { .. } => Some(BlockKind::Heading),
        Tag::BlockQuote(_) => Some(BlockKind::BlockQuote),
        Tag::List(_) => Some(BlockKind::List),
        Tag::Item => Some(BlockKind::Item),
        Tag::CodeBlock(_) => Some(BlockKind::CodeBlock),
        Tag::Table(_) => Some(BlockKind::Table),
        Tag::FootnoteDefinition(_) => Some(BlockKind::FootnoteDefinition),
        Tag::HtmlBlock => Some(BlockKind::HtmlBlock),
        _ => None,
    }
}

/// Mirrors `collect_comrak_list_items`'s normalization: a `Paragraph`
/// directly enclosed by an `Item` is dropped on both sides rather than
/// asserted, since whether it's emitted at all is a tight/loose wrinkle the
/// two parsers don't always agree on (see the divergence note above). A
/// small container stack tracks each Start/End tag's immediate parent kind
/// (`None` for non-block-container tags, e.g. `Strong`/`Emphasis`/`Link` —
/// a `Paragraph` can never be their direct child in CommonMark's grammar
/// anyway, so collapsing them to `None` here is safe).
fn pulldown_blocks(doc: &str) -> Vec<BlockKind> {
    let mut out = Vec::new();
    let mut stack: Vec<Option<BlockKind>> = Vec::new();
    for event in Parser::new_ext(doc, pulldown_options()) {
        match event {
            Event::Start(tag) => {
                let kind = tag_block_kind(&tag);
                let suppress =
                    matches!(kind, Some(BlockKind::Paragraph)) && stack.last() == Some(&Some(BlockKind::Item));
                if !suppress {
                    if let Some(k) = kind {
                        out.push(k);
                    }
                }
                stack.push(kind);
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Rule => out.push(BlockKind::ThematicBreak),
            _ => {}
        }
    }
    out
}

/// Hook for **known, documented** pulldown-cmark/comrak block-structure
/// divergences that can't be normalized away generically (unlike the
/// tight/loose paragraph-wrapping difference handled in
/// `collect_comrak`/`pulldown_blocks` above). Investigated candidates that
/// turned out to *not* be real divergences (so are deliberately absent
/// here): `"- - -\n"` looked like it could be the classic thematic-break-vs.
/// three-empty-list-items ambiguity, but both parsers actually agree it's a
/// `Rule` — verified by temporarily forcing this predicate to `false` and
/// confirming the differential still passes. Across this corpus's ~115
/// cases, once the paragraph-wrapping normalization is applied, pulldown-cmark
/// and comrak agree on block structure everywhere; this function exists so a
/// *real* future divergence has a documented place to land instead of a
/// silent test change.
fn known_divergence(_doc: &str) -> bool {
    false
}

#[test]
fn corpus_block_structure_matches_comrak_where_comparable() {
    for (i, doc) in cases::CASES.iter().enumerate() {
        if known_divergence(doc) {
            continue;
        }
        let ours = pulldown_blocks(doc);
        let theirs = comrak_blocks(doc);
        assert_eq!(
            ours, theirs,
            "case {i} block structure diverges from comrak: {doc:?}"
        );
    }
}
