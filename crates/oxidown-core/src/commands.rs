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
