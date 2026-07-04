//! Decoration emission: filters the cached overlay for a viewport, applies
//! the reveal predicate and the composition stability rule, and converts to
//! UTF-16 code units at the boundary. Never reparses.
//!
//! Reveal predicate (contract): a node is revealed when any selection range —
//! a cursor being an empty range — intersects the node's full extent
//! *including delimiters*, with boundary positions counting as intersecting
//! (a cursor sitting immediately before or after a delimiter reveals).
//! Revealed nodes emit `mark:delim` for their delimiter spans instead of
//! `conceal`. Nodes reveal independently, so nesting works per-node.
//!
//! Composition stability (contract, model rule 5): while a session is active,
//! any conceal span intersecting the composition range is emitted as
//! `mark:delim` instead — hence no *new* conceal span can appear inside the
//! range either, since every one of them is diverted to `mark:delim`.

use std::ops::Range;

use crate::composition::Composition;
use crate::parser::{Node, NodeKind};
use crate::text::TextBuffer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkStyle {
    Strong,
    Em,
    Code,
    Delim,
}

impl MarkStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            MarkStyle::Strong => "strong",
            MarkStyle::Em => "em",
            MarkStyle::Code => "code",
            MarkStyle::Delim => "delim",
        }
    }
}

/// Boundary decoration. All positions are UTF-16 code units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoration {
    Mark {
        from: usize,
        to: usize,
        style: MarkStyle,
    },
    Conceal {
        from: usize,
        to: usize,
    },
    Line {
        at: usize,
        /// Heading level 1..=6 (boundary style string "h1".."h6").
        level: u8,
    },
}

/// Closed-interval intersection: empty ranges (cursors) at a boundary count.
fn touches(a: usize, b: usize, from: usize, to: usize) -> bool {
    a <= to && b >= from
}

pub fn compute(
    nodes: &[Node],
    text: &TextBuffer,
    viewport: Range<usize>,
    selections: &[(usize, usize)],
    composition: Option<&Composition>,
) -> Vec<Decoration> {
    let mut out = Vec::new();
    for node in nodes {
        // Half-open overlap with the viewport; nodes have non-empty extents.
        if node.extent.start >= viewport.end || node.extent.end <= viewport.start {
            continue;
        }
        let revealed = selections
            .iter()
            .any(|&(a, b)| touches(a, b, node.extent.start, node.extent.end));

        let content_style = match node.kind {
            NodeKind::Heading(level) => {
                out.push(Decoration::Line {
                    at: text.byte_to_utf16(node.extent.start),
                    level,
                });
                None
            }
            NodeKind::Strong => Some(MarkStyle::Strong),
            NodeKind::Emphasis => Some(MarkStyle::Em),
            NodeKind::Code => Some(MarkStyle::Code),
        };
        if let Some(style) = content_style {
            if node.content.end > node.content.start {
                out.push(Decoration::Mark {
                    from: text.byte_to_utf16(node.content.start),
                    to: text.byte_to_utf16(node.content.end),
                    style,
                });
            }
        }
        for d in &node.delims {
            if d.end <= d.start {
                continue;
            }
            let in_composition = composition
                .is_some_and(|c| touches(c.start, c.end, d.start, d.end));
            let from = text.byte_to_utf16(d.start);
            let to = text.byte_to_utf16(d.end);
            if revealed || in_composition {
                out.push(Decoration::Mark {
                    from,
                    to,
                    style: MarkStyle::Delim,
                });
            } else {
                out.push(Decoration::Conceal { from, to });
            }
        }
    }
    out.sort_by_key(sort_key);
    out
}

fn sort_key(d: &Decoration) -> (usize, u8, usize) {
    match d {
        Decoration::Line { at, level } => (*at, 0, *level as usize),
        Decoration::Mark { from, to, .. } => (*from, 1, *to),
        Decoration::Conceal { from, to } => (*from, 1, *to),
    }
}
