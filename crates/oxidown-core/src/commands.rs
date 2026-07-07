//! Commands (plan.md §5.8, boundary v0.2): text transforms computed against
//! the overlay. Each planner here is a pure function from (overlay, source,
//! target) to a [`CommandPlan`] — minimal splices in pre-application byte
//! coordinates plus an optional post-application selection — or `None` when
//! the command doesn't apply at the target. `Editor::command` converts
//! boundary UTF-16 positions, runs the planner, and pushes the plan through
//! the normal apply path with origin `Command` (single undo unit, never
//! coalesces).
//!
//! ## Toggle semantics (contract-open decisions, documented here and in the
//! crate README)
//!
//! * **OFF (strip)** when a node of the toggled kind's *closed extent fully
//!   contains* the range — the innermost such node when several nest
//!   (`_a *b* c_` + toggleEm inside `b` unwraps `*b*`, not the outer `_…_`).
//!   Both delimiter spans are deleted, whatever their flavor (`__`, `_`,
//!   longer backtick runs — the actual source bytes go).
//! * **ON / EXTEND** otherwise: the target range unions with the extents of
//!   every same-kind node it *touches* (closed intersection — adjacency
//!   counts, so toggling right next to existing formatting merges with it
//!   rather than stacking `****`-style adjacent delimiters), the touched
//!   nodes' delimiters are stripped, and one canonical delimiter pair wraps
//!   the union. Canonical delimiters: `**`, `*`, `~~`; inline code uses a
//!   backtick run one longer than the longest run inside the final content
//!   (plus a padding space on each side when the content starts or ends
//!   with a backtick, per CommonMark).
//! * Double-toggle byte-identity therefore holds exactly when the source
//!   uses canonical flavors (`**x**` → `x` → `**x**`); an `__x__` unwraps
//!   to `x` and re-wraps as `**x**` (normalization, documented).
//! * An empty range with no touched node inserts an empty delimiter pair
//!   and places the cursor between them (standard toolbar behavior; the
//!   empty pair doesn't parse as formatting until content is typed —
//!   accepted).
//!
//! ## setHeading
//!
//! Operates on the LINE containing `pos`. Applies only when the line
//! belongs to a Paragraph, ATX Heading, or BlockQuote top-level block —
//! `None` on code blocks/fences, lists, tables, HTML blocks, thematic
//! breaks, blank lines, and setext headings (whose "delimiter" is the
//! following underline, not a leading-hash run this command rewrites).
//! Inside a blockquote the hashes go after the line's `> ` markers. Level 0
//! removes an existing hash prefix (`None` if there isn't one).
//!
//! ## toggleTask
//!
//! `pos` anywhere in the list item (the parser records each task item's
//! full extent). Flips exactly one byte: the `[ ]`/`[x]` checkbox interior.
//! `[X]` (capital) also toggles off to `[ ]`.
//!
//! ## indentList / outdentList
//!
//! Obsidian-style Tab nesting: indent a list item to its PARENT MARKER'S
//! CONTENT COLUMN (`- ` → 2, `1. ` → 3, `10. ` → 4; a task item's `- ` is
//! the same 2 — the checkbox is GFM content, not part of the marker),
//! rather than a fixed 2-space shift. All positions/quantities below are
//! per PHYSICAL SOURCE LINE.
//!
//! * The "quote prefix" of a line is its blockquote marker run (`> `,
//!   `> > `, …), from the parser's `BlockQuoteLine` nodes. The "marker
//!   column" is the column of a list marker's first glyph measured AFTER
//!   the quote prefix. "Content column" = marker column + marker token
//!   width.
//! * Applies iff at least one line intersecting `[from, to]` carries a list
//!   marker (a "list-item line") — otherwise `None` (the view falls back to
//!   its own default Tab handling).
//! * The edit moves by ONE delta, computed from the FIRST intersecting item
//!   line only: scanning upward over consecutive same-quote-depth list-item
//!   lines (stopping at the first line that isn't one, or whose quote depth
//!   differs) to find the nearest candidate — indent's target is the
//!   nearest with marker column <= the first line's; outdent's parent is
//!   the nearest with marker column STRICTLY less. No candidate (already
//!   first-in-list / already top-level) → applies, but is a NO-OP
//!   `CommandPlan` (empty batch); `Editor::command` special-cases an empty
//!   batch so it skips the undo unit/revision bump entirely (see its doc
//!   comment).
//! * indent: delta = target's content column − first line's marker column
//!   (`<= 0` → no-op). outdent: delta = first line's marker column − parent's
//!   (always `> 0`, since the parent's column is strictly smaller).
//! * **Subtree-aware affected-line set** (not just the intersecting lines):
//!   for EVERY intersecting item line, its whole subtree moves with it —
//!   walk forward from that line collecting consecutive following lines
//!   that are (a) list-item lines (b) at the SAME quote depth as it
//!   (c) with marker column STRICTLY GREATER than ITS OWN column (not the
//!   previous line's — a grandchild and a second child both still compare
//!   against the root, so a whole multi-level subtree, several children
//!   included, is captured in one walk). The walk stops at the first line
//!   that fails any of those three (a sibling/shallower item, a quote-depth
//!   change, or a non-item line — including a blank line: v1 does not look
//!   past one to see whether list content resumes). The union of every
//!   intersecting item line plus its subtree is the final affected set
//!   (deduplicated by line, applied in document order); every line in it
//!   gets the SAME single delta from above. indent inserts `delta` spaces
//!   right after each affected line's quote prefix; outdent removes
//!   `min(delta, that line's own marker column)` (clamped independently per
//!   line, so a shallower descendant than expected never goes negative).
//! * **Paragraph-interruption guard** (structural, not cosmetic): a non-1
//!   ordered marker cannot START a new list in paragraph-interruption
//!   position per CommonMark — such a line silently degrades to
//!   lazy-continuation text. After computing the batch, TWO lines are
//!   checked with the same landing-scan rule, and rewritten (digits → `1`,
//!   `2.` → `1.`, `10.` → `1.`, extra splice in the SAME batch/undo unit)
//!   when they fail it:
//!
//!   1. the FIRST affected line (the moved item itself), at its new column;
//!   2. the first UNAFFECTED item line BELOW the affected set, at its own
//!      unchanged column — found by walking down from the last affected
//!      line over consecutive same-quote-depth item lines, skipping ADOPTED
//!      descendants (post-edit column strictly greater than the moved
//!      line's new column; they nest under the moved block and stay items
//!      with it), stopping at a non-item/blank line or quote-depth change.
//!      The command never touched this line, but the edit restructured the
//!      parse context above it (e.g. outdenting a nested bullet to top
//!      level puts a following `3.` sibling against the new bullet list
//!      instead of the outer ordered list it used to continue); keeping it
//!      a list item is part of the command's contract, and its displayed
//!      number is cosmetic (view-computed numbering, research/07).
//!
//!   The landing-scan rule, for a line at post-edit column `c`: scan upward
//!   past consecutive same-quote-depth item lines whose POST-EDIT marker
//!   column (affected lines shift by the batch's per-line change) is
//!   STRICTLY GREATER than `c`, landing on the nearest line with column
//!   <= `c`. If the landing line is an item at column == `c` whose marker
//!   is also ordered with the SAME delimiter (`.` vs `)`), the checked item
//!   JOINS that open list — any number is valid — no rewrite. Otherwise
//!   (shallower landing, different marker family, or the scan broke on a
//!   non-item/other-depth line) it would start a new list → rewrite.
//!
//!   No COSMETIC renumbering of siblings ever happens (view territory, per
//!   research/07). Accepted v1 imprecision: `10.` → `1.` shrinks the marker
//!   token width by one, so the line's descendants (shifted by the
//!   pre-rewrite delta) may sit one column past the ideal content column —
//!   still valid nesting.
//! * **Adoption**: outdenting an item past a following equal-column sibling
//!   makes that sibling (and anything deeper) a CHILD of the outdented item
//!   on reparse — its column now exceeds the moved item's new content
//!   column. This is intended standard outliner behavior (the sibling keeps
//!   its itemness; a later re-indent of the parent carries it along as part
//!   of the subtree).

use std::collections::BTreeMap;
use std::ops::Range;

use crate::block_index::BlockKind;
use crate::mapping::{self, Bias};
use crate::parser::{Node, NodeKind};
use crate::text::ByteSplice;

/// Typed command surface (the wasm boundary flattens this to
/// `command(name, a, b?)`). All positions are UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    ToggleStrong { from: usize, to: usize },
    ToggleEm { from: usize, to: usize },
    ToggleStrike { from: usize, to: usize },
    ToggleCode { from: usize, to: usize },
    /// `level` 0 = back to paragraph.
    SetHeading { pos: usize, level: u8 },
    ToggleTask { pos: usize },
    /// Marker-width-aware Tab nesting (boundary v0.2). UTF-16 range like the
    /// toggles; the editor resolves to bytes and normalizes `from <= to`.
    IndentList { from: usize, to: usize },
    OutdentList { from: usize, to: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineKind {
    Strong,
    Em,
    Strike,
    Code,
}

/// A planned command: splices in **pre-application** byte coordinates
/// (ascending, non-overlapping) and an optional selection in
/// **post-application** byte coordinates.
#[derive(Debug)]
pub struct CommandPlan {
    pub batch: Vec<ByteSplice>,
    pub selection: Option<(usize, usize)>,
}

fn node_is(kind: InlineKind, n: &Node) -> bool {
    matches!(
        (kind, &n.kind),
        (InlineKind::Strong, NodeKind::Strong)
            | (InlineKind::Em, NodeKind::Emphasis)
            | (InlineKind::Strike, NodeKind::Strike)
            | (InlineKind::Code, NodeKind::Code)
    )
}

fn del(span: &Range<usize>) -> ByteSplice {
    ByteSplice {
        at: span.start,
        delete: span.end - span.start,
        insert: String::new(),
    }
}

fn ins(at: usize, text: String) -> ByteSplice {
    ByteSplice {
        at,
        delete: 0,
        insert: text,
    }
}

pub fn toggle_inline(
    nodes: &[Node],
    src: &str,
    kind: InlineKind,
    from_b: usize,
    to_b: usize,
) -> Option<CommandPlan> {
    // OFF: innermost same-kind node whose closed extent contains the range
    // (rfind on the document-ordered overlay = last-starting = innermost).
    let containing = nodes.iter().rfind(|n| {
        node_is(kind, n)
            && n.delims.len() == 2
            && n.extent.start <= from_b
            && to_b <= n.extent.end
    });
    if let Some(node) = containing {
        let d0 = &node.delims[0];
        let d1 = &node.delims[1];
        let open_len = d0.end - d0.start;
        return Some(CommandPlan {
            batch: vec![del(d0), del(d1)],
            selection: Some((node.content.start - open_len, node.content.end - open_len)),
        });
    }

    // ON / EXTEND: union with every touched same-kind node.
    let touched: Vec<&Node> = nodes
        .iter()
        .filter(|n| node_is(kind, n))
        .filter(|n| n.delims.len() == 2)
        .filter(|n| n.extent.start <= to_b && n.extent.end >= from_b)
        .collect();
    let t_start = touched
        .iter()
        .map(|n| n.extent.start)
        .min()
        .unwrap_or(from_b)
        .min(from_b);
    let t_end = touched
        .iter()
        .map(|n| n.extent.end)
        .max()
        .unwrap_or(to_b)
        .max(to_b);

    let (open, close) = delimiters(kind, src, &touched, t_start, t_end);
    let open_len = open.len();

    let mut batch = Vec::with_capacity(2 + touched.len() * 2);
    batch.push(ins(t_start, open));
    let mut deleted = 0usize;
    for n in &touched {
        deleted += (n.delims[0].end - n.delims[0].start) + (n.delims[1].end - n.delims[1].start);
        batch.push(del(&n.delims[0]));
        batch.push(del(&n.delims[1]));
    }
    batch.push(ins(t_end, close));
    // `touched` nodes are document-ordered and non-overlapping for the same
    // kind (same-delimiter emphasis cannot directly nest); with the open
    // insert at t_start ≤ first delim and the close insert at t_end ≥ last
    // delim end, the batch is ascending and non-overlapping as built.
    Some(CommandPlan {
        batch,
        selection: Some((t_start + open_len, t_end - deleted + open_len)),
    })
}

/// Canonical delimiter pair for an ON/EXTEND toggle. For code, the run is
/// one backtick longer than the longest backtick run remaining in the final
/// content, space-padded when that content starts or ends with a backtick.
fn delimiters(
    kind: InlineKind,
    src: &str,
    touched: &[&Node],
    t_start: usize,
    t_end: usize,
) -> (String, String) {
    match kind {
        InlineKind::Strong => ("**".into(), "**".into()),
        InlineKind::Em => ("*".into(), "*".into()),
        InlineKind::Strike => ("~~".into(), "~~".into()),
        InlineKind::Code => {
            let mut content = String::new();
            let mut pos = t_start;
            let mut spans: Vec<&Range<usize>> =
                touched.iter().flat_map(|n| n.delims.iter()).collect();
            spans.sort_by_key(|r| r.start);
            for d in spans {
                content.push_str(&src[pos..d.start]);
                pos = d.end;
            }
            content.push_str(&src[pos..t_end]);
            let mut longest = 0usize;
            let mut run = 0usize;
            for b in content.bytes() {
                if b == b'`' {
                    run += 1;
                    longest = longest.max(run);
                } else {
                    run = 0;
                }
            }
            let ticks = "`".repeat(longest + 1);
            if content.starts_with('`') || content.ends_with('`') {
                (format!("{ticks} "), format!(" {ticks}"))
            } else {
                (ticks.clone(), ticks)
            }
        }
    }
}

pub fn set_heading(
    nodes: &[Node],
    src: &str,
    block_kind: Option<BlockKind>,
    line: Range<usize>,
    pos_b: usize,
    level: u8,
) -> Option<CommandPlan> {
    if !matches!(
        block_kind,
        Some(BlockKind::Paragraph) | Some(BlockKind::Heading) | Some(BlockKind::BlockQuote)
    ) {
        return None;
    }
    // Defensive: never rewrite lines the overlay knows are code.
    if nodes.iter().any(|n| {
        matches!(n.kind, NodeKind::CodeFenceLine | NodeKind::CodeBlockLine)
            && n.extent.start <= line.start
            && line.start <= n.extent.end
    }) {
        return None;
    }
    // Inside a blockquote the hashes go after this line's `> ` markers.
    let insertion = nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::BlockQuoteLine(_)))
        .find(|n| n.extent.start == line.start)
        .and_then(|n| n.delims.last().map(|d| d.end))
        .unwrap_or(line.start);

    let existing = nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Heading(_)))
        .find(|n| n.delims.first().is_some_and(|d| d.start == insertion));

    let batch = match (existing, level) {
        (None, 0) => return None, // nothing to remove
        (Some(_), 0) => {
            let d = existing.unwrap().delims[0].clone();
            vec![del(&d)]
        }
        (Some(node), n) => {
            let d = node.delims[0].clone();
            let prefix = format!("{} ", "#".repeat(n as usize));
            if &src[d.start..d.end] == prefix.as_str() {
                return None; // already at this level, byte-identically
            }
            vec![ByteSplice {
                at: d.start,
                delete: d.end - d.start,
                insert: prefix,
            }]
        }
        (None, n) => {
            // A Heading block with no ATX node at the insertion point is a
            // setext heading — out of scope for a leading-hash rewrite.
            if block_kind == Some(BlockKind::Heading) {
                return None;
            }
            // Blank line: nothing to promote.
            if line.start >= line.end {
                return None;
            }
            vec![ins(insertion, format!("{} ", "#".repeat(n as usize)))]
        }
    };
    let cursor = mapping::map_pos(pos_b, &batch, Bias::Before);
    Some(CommandPlan {
        batch,
        selection: Some((cursor, cursor)),
    })
}

pub fn toggle_task(nodes: &[Node], doc_len: usize, pos_b: usize) -> Option<CommandPlan> {
    let widget = nodes.iter().rfind(|n| {
        matches!(n.kind, NodeKind::TaskWidget { .. })
            && n.item_extent.as_ref().is_some_and(|item| {
                item.start <= pos_b
                    && (pos_b < item.end || (pos_b == item.end && item.end == doc_len))
            })
    })?;
    let checked = matches!(widget.kind, NodeKind::TaskWidget { checked: true });
    Some(CommandPlan {
        batch: vec![ByteSplice {
            at: widget.extent.start + 1,
            delete: 1,
            insert: if checked { " " } else { "x" }.into(),
        }],
        selection: None, // 1-for-1 byte swap: the view's cursor is unaffected
    })
}

// ---------------------------------------------------------------------
// indentList / outdentList (boundary v0.2: marker-width-aware Tab nesting).
// See the module doc comment's "## indentList / outdentList" section for
// the full spec — this is a direct transcription of it.
// ---------------------------------------------------------------------

/// One physical source line's list/quote context. Built fresh per line from
/// the parser overlay + raw source bytes — cheap, since these commands only
/// ever look at a handful of lines around the selection.
#[derive(Clone, Copy)]
struct ListLineCtx {
    start: usize,
    /// This line's own extent end (terminator excluded) — where to resume
    /// scanning from `next_line` for the subtree walk.
    end: usize,
    /// Byte offset just past this line's blockquote marker run (`> `,
    /// `> > `, …) — equals `start` when the line isn't quoted.
    quote_end: usize,
    quote_depth: u8,
    /// `(marker glyph's first byte, marker token width)` when this line's
    /// OWN list item begins here — `None` for continuation/blank/non-list
    /// lines (those never carry their own `ListMarker` node).
    marker: Option<(usize, usize)>,
}

impl ListLineCtx {
    /// Marker column: the marker's first glyph, measured from just past the
    /// quote prefix. `None` when this line has no marker of its own.
    fn marker_column(&self) -> Option<usize> {
        self.marker.map(|(item_start, _)| item_start - self.quote_end)
    }
}

/// Byte range of the physical source line containing `pos` (the trailing
/// `\n`/`\r\n` terminator excluded). Commands work directly off `src`
/// (unlike `Editor`, which has `TextBuffer::line_range_at` over the rope).
fn line_containing(bytes: &[u8], pos: usize) -> Range<usize> {
    let mut start = pos.min(bytes.len());
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    let mut end = start;
    while end < bytes.len() && bytes[end] != b'\n' && bytes[end] != b'\r' {
        end += 1;
    }
    start..end
}

/// The physical line immediately preceding `line_start`, or `None` at the
/// start of the document.
fn prev_line(bytes: &[u8], line_start: usize) -> Option<Range<usize>> {
    if line_start == 0 {
        return None;
    }
    let mut end = line_start;
    if end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && bytes[end - 1] == b'\r' {
        end -= 1;
    }
    Some(line_containing(bytes, end))
}

/// The physical line immediately following the line ending at `line_end`
/// (that line's own extent, terminator excluded), or `None` when `line_end`
/// has no terminator (the document's last line).
fn next_line(bytes: &[u8], line_end: usize) -> Option<Range<usize>> {
    let mut next = line_end;
    if bytes.get(next) == Some(&b'\r') {
        next += 1;
    }
    if bytes.get(next) == Some(&b'\n') {
        next += 1;
    }
    if next == line_end {
        return None; // no terminator: `line_end` is the document's end
    }
    Some(line_containing(bytes, next))
}

/// Physical lines intersecting `[from_b, to_b]` (`from_b <= to_b`), mirroring
/// CodeMirror's own multi-line command iteration: an empty range (cursor)
/// always yields its containing line; a non-empty range excludes a trailing
/// line touched only at its very start (`to_b` landing exactly on a line
/// boundary selects none of that line).
fn intersecting_lines(bytes: &[u8], from_b: usize, to_b: usize) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let empty = from_b == to_b;
    let mut pos = from_b;
    loop {
        let line = line_containing(bytes, pos);
        if empty || to_b > line.start {
            lines.push(line.clone());
        }
        if pos >= to_b {
            break;
        }
        let mut next = line.end;
        if bytes.get(next) == Some(&b'\r') {
            next += 1;
        }
        if bytes.get(next) == Some(&b'\n') {
            next += 1;
        }
        if next <= pos {
            break; // defensive: no terminator left (doc end)
        }
        pos = next;
    }
    lines
}

/// This line's blockquote depth (0 outside any blockquote) and the byte
/// offset just past its `> `/`> > `/… marker run, from the parser's per-line
/// `BlockQuoteLine` node.
fn quote_context(nodes: &[Node], line_start: usize) -> (u8, usize) {
    nodes
        .iter()
        .find_map(|n| match n.kind {
            NodeKind::BlockQuoteLine(depth) if n.extent.start == line_start => {
                Some((depth, n.delims.last().map_or(line_start, |d| d.end)))
            }
            _ => None,
        })
        .unwrap_or((0, line_start))
}

/// This line's list marker, if its own item begins here: `(glyph_start,
/// token_width)`. `token_width` uses the spec's FIXED-width definition —
/// marker glyphs plus exactly one following space (`- ` = 2, `1. ` = 3,
/// `10. ` = 4; a task item's `- ` is the same 2, the checkbox is content) —
/// not however much whitespace the source actually has after the marker
/// (CommonMark lets a marker's real content start several spaces later;
/// that extra whitespace is deliberately not part of this arithmetic).
fn line_marker(nodes: &[Node], bytes: &[u8], line: &Range<usize>) -> Option<(usize, usize)> {
    let raw_start = nodes.iter().find_map(|n| match n.kind {
        NodeKind::ListMarker { .. } if n.extent.start >= line.start && n.extent.start < line.end => {
            Some(n.extent.start)
        }
        _ => None,
    })?;
    // The parser's `ListMarker` extent starts exactly at the glyph for a
    // properly-nested (depth >= 2) item — its leading indent is split into a
    // separate `ListItemIndent` node. But pulldown-cmark can fold a FEW
    // (0-3) bytes of incidental leading whitespace directly into a marker's
    // own span when that whitespace isn't establishing a new nested
    // container (e.g. a lightly/unevenly indented sibling within a
    // depth-1 list). Skip forward past any such whitespace so the marker
    // column always measures from the actual `-`/`+`/`*`/digit glyph, per
    // the spec's literal definition — never from stray leading spaces.
    let mut item_start = raw_start;
    while matches!(bytes.get(item_start), Some(b' ' | b'\t')) {
        item_start += 1;
    }
    Some((item_start, marker_token_width(bytes, item_start)))
}

/// Marker glyph run length + 1 (the required following space) — see
/// [`line_marker`].
fn marker_token_width(bytes: &[u8], item_start: usize) -> usize {
    let mut i = item_start;
    match bytes.get(i) {
        Some(b'-' | b'+' | b'*') => i += 1,
        Some(b) if b.is_ascii_digit() => {
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            if matches!(bytes.get(i), Some(b'.' | b')')) {
                i += 1;
            }
        }
        _ => {}
    }
    (i - item_start) + 1
}

fn list_line_ctx(nodes: &[Node], bytes: &[u8], line: Range<usize>) -> ListLineCtx {
    let (quote_depth, quote_end) = quote_context(nodes, line.start);
    let marker = line_marker(nodes, bytes, &line);
    ListLineCtx {
        start: line.start,
        end: line.end,
        quote_end,
        quote_depth,
        marker,
    }
}

/// Shared planner for `indentList`/`outdentList`. `from_b <= to_b` (the
/// editor normalizes before calling, same convention as `toggle_inline`).
fn plan_list_nesting(
    nodes: &[Node],
    src: &str,
    from_b: usize,
    to_b: usize,
    indent: bool,
) -> Option<CommandPlan> {
    let bytes = src.as_bytes();
    let lines: Vec<ListLineCtx> = intersecting_lines(bytes, from_b, to_b)
        .into_iter()
        .map(|l| list_line_ctx(nodes, bytes, l))
        .collect();

    // Applies iff at least one intersecting line carries a marker.
    let first_idx = lines.iter().position(|l| l.marker.is_some())?;
    let first = &lines[first_idx];
    let first_col = first.marker_column().expect("first_idx line has a marker");
    let first_depth = first.quote_depth;

    let no_op = || CommandPlan {
        batch: Vec::new(),
        selection: None,
    };

    // Scan upward over consecutive same-quote-depth list-item lines for the
    // nearest qualifying candidate: indent's target allows `<=`, outdent's
    // parent requires strictly `<`. Stops (no candidate) at the first line
    // that isn't itself a list-item line, or whose quote depth differs.
    let mut target: Option<(usize, usize)> = None; // (marker_column, token_width)
    let mut cursor = first.start;
    while let Some(range) = prev_line(bytes, cursor) {
        let ctx = list_line_ctx(nodes, bytes, range.clone());
        if ctx.quote_depth != first_depth {
            break;
        }
        let Some((item_start, width)) = ctx.marker else {
            break;
        };
        let col = item_start - ctx.quote_end;
        let qualifies = if indent { col <= first_col } else { col < first_col };
        if qualifies {
            target = Some((col, width));
            break;
        }
        cursor = range.start;
    }
    let Some((target_col, target_width)) = target else {
        return Some(no_op()); // no candidate above: nothing to nest under/from
    };

    let delta: usize = if indent {
        let content_col = target_col + target_width;
        if content_col <= first_col {
            return Some(no_op());
        }
        content_col - first_col
    } else {
        // target_col < first_col by construction (the strict `<` qualifier
        // above), so this never underflows.
        first_col - target_col
    };

    // Subtree-aware affected set: every intersecting item line, PLUS, for
    // each one, its whole subtree (consecutive following lines at the same
    // quote depth whose marker column is strictly greater than that line's
    // own — see the module doc comment). Keyed/ordered by line start so the
    // final batch is built in ascending document order with no duplicates.
    let mut affected: BTreeMap<usize, ListLineCtx> = BTreeMap::new();
    for line in &lines {
        let Some(root_col) = line.marker_column() else {
            continue;
        };
        affected.entry(line.start).or_insert(*line);
        let mut cursor_end = line.end;
        while let Some(range) = next_line(bytes, cursor_end) {
            let ctx = list_line_ctx(nodes, bytes, range.clone());
            if ctx.quote_depth != line.quote_depth {
                break;
            }
            let Some(col) = ctx.marker_column() else {
                break;
            };
            if col <= root_col {
                break;
            }
            cursor_end = range.end;
            affected.entry(ctx.start).or_insert(ctx);
        }
    }

    let mut batch = Vec::with_capacity(affected.len());
    for line in affected.values() {
        let Some(col) = line.marker_column() else {
            continue;
        };
        if indent {
            batch.push(ByteSplice {
                at: line.quote_end,
                delete: 0,
                insert: " ".repeat(delta),
            });
        } else {
            let remove = delta.min(col);
            if remove == 0 {
                continue;
            }
            batch.push(ByteSplice {
                at: line.quote_end,
                delete: remove,
                insert: String::new(),
            });
        }
    }
    if batch.is_empty() {
        return Some(no_op());
    }

    // Paragraph-interruption guard (see the module doc comment): a non-1
    // ordered item that would START a new list — rather than join an
    // already-open same-delimiter ordered list at its landing column — is
    // refused by CommonMark in paragraph-interruption position and would
    // silently degrade to lazy-continuation text. Two lines can end up in
    // that position:
    //
    // 1. The moved (first affected) line itself. Its own splice is batch[0]
    //    (it is the minimum start of the affected set and always
    //    contributes), and the rewrite lands after it but before the next
    //    line's splice, so index 1 keeps the batch ascending.
    // 2. The first UNAFFECTED item line below the affected set (skipping
    //    adopted descendants — see `below_line_rewrite`): the edit changed
    //    the parse context ABOVE that line even though the command never
    //    touched it. Its rewrite is on a later line than every whitespace
    //    splice, so appending keeps the batch ascending.
    let new_col = if indent { first_col + delta } else { first_col - delta };
    if let Some(rewrite) = interruption_rewrite(nodes, bytes, first, new_col, &affected, delta, indent) {
        batch.insert(1, rewrite);
    }
    if let Some(rewrite) =
        below_line_rewrite(nodes, bytes, &affected, first_depth, new_col, delta, indent)
    {
        batch.push(rewrite);
    }

    // Selection maps through the batch like any other command (Bias::After:
    // an insertion sitting exactly at a mapped endpoint moves past it, so a
    // cursor/selection anchored right at a line's content start shifts with
    // the line rather than staying pinned before the new indentation).
    let anchor = mapping::map_pos(from_b, &batch, Bias::After);
    let head = mapping::map_pos(to_b, &batch, Bias::After);
    Some(CommandPlan {
        batch,
        selection: Some((anchor, head)),
    })
}

/// The ordered-marker shape at `item_start`: `(digit_run_len, value_is_one,
/// delimiter_byte)`. `None` for bullet markers. `value_is_one` is numeric
/// (`01.` counts as 1, per CommonMark's "start number" semantics).
fn ordered_marker(bytes: &[u8], item_start: usize) -> Option<(usize, bool, u8)> {
    let mut i = item_start;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == item_start {
        return None;
    }
    let delim = *bytes.get(i)?;
    if delim != b'.' && delim != b')' {
        return None;
    }
    let digits = &bytes[item_start..i];
    let significant = digits
        .iter()
        .position(|&b| b != b'0')
        .map_or(&b""[..], |p| &digits[p..]);
    Some((i - item_start, significant == b"1", delim))
}

/// A line's marker column as it will read AFTER the edit: affected lines
/// shift by the batch's per-line whitespace change (`+delta` for indent,
/// `-min(delta, col)` for outdent — the same clamp the batch applies);
/// unaffected lines keep their pre-edit column.
fn post_edit_col(
    ctx: &ListLineCtx,
    col: usize,
    affected: &BTreeMap<usize, ListLineCtx>,
    delta: usize,
    indent: bool,
) -> usize {
    if !affected.contains_key(&ctx.start) {
        return col;
    }
    if indent {
        col + delta
    } else {
        col - delta.min(col)
    }
}

/// Paragraph-interruption guard for one line (module doc comment,
/// "Paragraph-interruption guard"): returns the digit-rewrite splice
/// (pre-edit coordinates — the whitespace edits never touch the digits, so
/// pre- and post-edit digit spans are the same bytes) when `line`'s non-1
/// ordered marker, sitting at `line_col_post` after the edit, would start a
/// new list rather than join an open one; `None` when no rewrite is needed.
/// The landing scan uses post-edit columns throughout (`post_edit_col`):
/// for the first-affected-line check the scan only crosses lines ABOVE the
/// affected set (identity mapping), but the below-line check's scan crosses
/// the affected set itself, whose columns the batch changes.
fn interruption_rewrite(
    nodes: &[Node],
    bytes: &[u8],
    line: &ListLineCtx,
    line_col_post: usize,
    affected: &BTreeMap<usize, ListLineCtx>,
    delta: usize,
    indent: bool,
) -> Option<ByteSplice> {
    let (item_start, _) = line.marker?;
    let (digit_len, is_one, delim) = ordered_marker(bytes, item_start)?;
    if is_one {
        return None; // "1." can interrupt anything; never rewritten
    }
    // Landing scan: skip consecutive same-quote-depth item lines strictly
    // deeper than the checked line's post-edit column; the first line that
    // isn't is the landing.
    let mut joins = false;
    let mut cursor = line.start;
    while let Some(range) = prev_line(bytes, cursor) {
        let ctx = list_line_ctx(nodes, bytes, range.clone());
        if ctx.quote_depth != line.quote_depth {
            break; // landing outside the quote context: not an open list
        }
        let Some((land_start, _)) = ctx.marker else {
            break; // landing on a non-item line: not an open list
        };
        let col = post_edit_col(&ctx, land_start - ctx.quote_end, affected, delta, indent);
        if col > line_col_post {
            cursor = range.start;
            continue;
        }
        // Landing line: the checked item joins an open list only at an
        // EQUAL column with the SAME ordered delimiter flavor ('.' vs ')')
        // — a shallower item or a different family means it starts a NEW
        // list where a non-1 ordered marker cannot interrupt. (A landing
        // line the FIRST guard rewrites keeps its delimiter — only digits
        // change — so this family check is stable across the two guards.)
        joins = col == line_col_post
            && ordered_marker(bytes, land_start).is_some_and(|(_, _, d)| d == delim);
        break;
        // Scan exhaustion (document start) would leave `joins` false and
        // rewrite; unreachable in practice, since the indent target /
        // outdent parent found earlier always sits above the first line.
    }
    if joins {
        return None;
    }
    Some(ByteSplice {
        at: item_start,
        delete: digit_len,
        insert: "1".into(),
    })
}

/// Below-context paragraph-interruption guard: the edit can change the
/// parse context of a line BELOW the affected set that the command never
/// touched (e.g. outdenting a nested bullet to top level makes a following
/// `3.` sibling — previously continuing the open outer ordered list — sit
/// against the new bullet list instead, where a non-1 ordered marker
/// cannot start a list). Walk down from the last affected line over
/// consecutive same-quote-depth item lines, SKIPPING adopted descendants
/// (post-edit column strictly greater than the moved line's new column —
/// they nest under the moved block, whose itemness the first guard already
/// preserves); the first item line at column <= the new column is the one
/// whose landing the edit re-anchored — run the same landing-scan check on
/// it at its own (unchanged) column. Stops at a non-item/blank line or
/// quote-depth change, like every other scan here.
fn below_line_rewrite(
    nodes: &[Node],
    bytes: &[u8],
    affected: &BTreeMap<usize, ListLineCtx>,
    root_depth: u8,
    root_post_col: usize,
    delta: usize,
    indent: bool,
) -> Option<ByteSplice> {
    let last = affected.values().next_back()?;
    let mut cursor_end = last.end;
    loop {
        let range = next_line(bytes, cursor_end)?;
        let ctx = list_line_ctx(nodes, bytes, range.clone());
        if ctx.quote_depth != root_depth {
            return None;
        }
        let (item_start, _) = ctx.marker?;
        let col = item_start - ctx.quote_end; // unaffected: pre == post
        if col > root_post_col {
            cursor_end = range.end; // adopted descendant of the moved block
            continue;
        }
        return interruption_rewrite(nodes, bytes, &ctx, col, affected, delta, indent);
    }
}

/// Indent every list-item line intersecting `[from_b, to_b]` — plus, for
/// each one, its whole subtree (see the module doc comment) — to its
/// nesting parent's content column. `None` when no intersecting line is a
/// list item; `Some` with an empty batch when it applies but no movement is
/// possible (first item of its list, or already as deep as its parent
/// allows).
pub fn indent_list(nodes: &[Node], src: &str, from_b: usize, to_b: usize) -> Option<CommandPlan> {
    plan_list_nesting(nodes, src, from_b, to_b, true)
}

/// Outdent every list-item line intersecting `[from_b, to_b]` — plus each
/// one's subtree — by the first line's distance to its nesting parent.
/// `None`/no-op cases mirror [`indent_list`] (already top-level → no-op
/// instead of `None`, since the command still applies).
pub fn outdent_list(nodes: &[Node], src: &str, from_b: usize, to_b: usize) -> Option<CommandPlan> {
    plan_list_nesting(nodes, src, from_b, to_b, false)
}
