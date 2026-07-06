//! Decoration emission: filters the cached overlay for a viewport, applies
//! the reveal predicate and the composition stability rule, and converts to
//! UTF-16 code units at the boundary. Never reparses.
//!
//! Reveal predicate (contract): a node is revealed when any selection range —
//! a cursor being an empty range — intersects the node's full extent
//! *including delimiters*, with boundary positions counting as intersecting
//! (a cursor sitting immediately before or after a delimiter reveals).
//! Revealed nodes emit `mark:delim` for their delimiter spans instead of
//! `conceal`. Nodes reveal independently, so nesting works per-node. A task
//! widget uses an alternate, larger extent for this predicate (see
//! `parser::Node::reveal_extent`) — the *list item's* marker extent, per the
//! contract, so that clicking the rendered checkbox still reveals.
//!
//! Composition stability (contract, model rule 5): while a session is active,
//! any conceal span intersecting the composition range is emitted as
//! `mark:delim` instead — hence no *new* conceal span can appear inside the
//! range either, since every one of them is diverted to `mark:delim`. The
//! same rule is applied to the task widget (composing over its checkbox
//! span suppresses the widget in favor of `mark:delim`), so an IME session
//! never has to fight a replace-range widget mid-composition.
//!
//! M0 decoration shapes (`Line`, and the existing `Mark`/`Conceal` styles)
//! are unchanged from v0 — the M1 additions are new enum variants (`Block`,
//! `Widget`) and new `MarkStyle` values, kept additive per
//! docs/boundary-v0.md's v0.2 rule ("Views MUST ignore decoration styles and
//! widget kinds they don't recognize").

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
    /// M1: strikethrough content (`~~x~~`).
    Strike,
    /// M1: a link's visible text (concealed state) or whole autolink span.
    Link,
    /// M1: a link's destination, emitted only when the link is revealed.
    Url,
    /// M1: a list item's bullet/number marker — always visible, never
    /// concealed.
    ListMarker,
}

impl MarkStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            MarkStyle::Strong => "strong",
            MarkStyle::Em => "em",
            MarkStyle::Code => "code",
            MarkStyle::Delim => "delim",
            MarkStyle::Strike => "strike",
            MarkStyle::Link => "link",
            MarkStyle::Url => "url",
            MarkStyle::ListMarker => "list-marker",
        }
    }
}

/// M1 line-level block styles beyond the M0 heading `Line` variant. Kept as
/// a separate `Decoration::Block` variant (rather than folding into `Line`)
/// so the M0 `Decoration::Line { at, level }` shape — and every M0 test that
/// matches it — is untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockStyle {
    /// 1-based nesting depth.
    BlockQuote(u8),
    CodeBlock,
    CodeFence,
    ThematicBreak,
}

impl BlockStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockStyle::BlockQuote(_) => "blockquote",
            BlockStyle::CodeBlock => "code-block",
            BlockStyle::CodeFence => "code-fence",
            BlockStyle::ThematicBreak => "hr",
        }
    }

    pub fn depth(&self) -> Option<u8> {
        match self {
            BlockStyle::BlockQuote(d) => Some(*d),
            _ => None,
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
    /// M0: ATX heading line. Unchanged shape — see module docs.
    Line {
        at: usize,
        /// Heading level 1..=6 (boundary style string "h1".."h6").
        level: u8,
    },
    /// M1: blockquote/code-fence/code-block/thematic-break line chrome.
    Block {
        at: usize,
        style: BlockStyle,
    },
    /// M1: a task item's checkbox, replacing the `[ ]`/`[x]` source span.
    /// Withheld (in favor of `mark:delim` over the same span) when the
    /// node is revealed or under active composition.
    Widget {
        from: usize,
        to: usize,
        checked: bool,
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

        let reveal_extent = node.reveal_extent.as_ref().unwrap_or(&node.extent);
        let revealed = selections
            .iter()
            .any(|&(a, b)| touches(a, b, reveal_extent.start, reveal_extent.end));

        match node.kind {
            NodeKind::ListMarker => {
                if node.extent.end > node.extent.start {
                    out.push(Decoration::Mark {
                        from: text.byte_to_utf16(node.extent.start),
                        to: text.byte_to_utf16(node.extent.end),
                        style: MarkStyle::ListMarker,
                    });
                }
                continue;
            }
            NodeKind::TaskWidget { checked } => {
                let in_composition = composition
                    .is_some_and(|c| touches(c.start, c.end, node.extent.start, node.extent.end));
                let from = text.byte_to_utf16(node.extent.start);
                let to = text.byte_to_utf16(node.extent.end);
                if revealed || in_composition {
                    out.push(Decoration::Mark {
                        from,
                        to,
                        style: MarkStyle::Delim,
                    });
                } else {
                    out.push(Decoration::Widget { from, to, checked });
                }
                continue;
            }
            _ => {}
        }

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
            NodeKind::Strike => Some(MarkStyle::Strike),
            NodeKind::Link { .. } => Some(MarkStyle::Link),
            NodeKind::BlockQuoteLine(depth) => {
                out.push(Decoration::Block {
                    at: text.byte_to_utf16(node.extent.start),
                    style: BlockStyle::BlockQuote(depth),
                });
                None
            }
            NodeKind::CodeFenceLine => {
                out.push(Decoration::Block {
                    at: text.byte_to_utf16(node.extent.start),
                    style: BlockStyle::CodeFence,
                });
                None
            }
            NodeKind::CodeBlockLine => {
                out.push(Decoration::Block {
                    at: text.byte_to_utf16(node.extent.start),
                    style: BlockStyle::CodeBlock,
                });
                Some(MarkStyle::Code)
            }
            NodeKind::ThematicBreak => {
                out.push(Decoration::Block {
                    at: text.byte_to_utf16(node.extent.start),
                    style: BlockStyle::ThematicBreak,
                });
                None
            }
            NodeKind::ListMarker | NodeKind::TaskWidget { .. } => {
                unreachable!("handled above with `continue`")
            }
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
            if !(revealed || in_composition) {
                out.push(Decoration::Conceal {
                    from: text.byte_to_utf16(d.start),
                    to: text.byte_to_utf16(d.end),
                });
                continue;
            }
            // Revealed. A link's second conceal span (`](url)`) opens up as
            // delim/url/delim PIECES (v0.2 clarification 4) — non-overlapping,
            // with the destination as `mark:url` and any title tail staying
            // delim. Everything else reveals as one whole delim mark.
            let url_inside = match (&node.kind, &node.url) {
                (NodeKind::Link { autolink: false }, Some(url))
                    if d.start <= url.start && url.end <= d.end && url.end > url.start =>
                {
                    Some(url.clone())
                }
                _ => None,
            };
            match url_inside {
                Some(url) => {
                    for (piece_from, piece_to, style) in [
                        (d.start, url.start, MarkStyle::Delim),
                        (url.start, url.end, MarkStyle::Url),
                        (url.end, d.end, MarkStyle::Delim),
                    ] {
                        if piece_to > piece_from {
                            out.push(Decoration::Mark {
                                from: text.byte_to_utf16(piece_from),
                                to: text.byte_to_utf16(piece_to),
                                style,
                            });
                        }
                    }
                }
                None => {
                    out.push(Decoration::Mark {
                        from: text.byte_to_utf16(d.start),
                        to: text.byte_to_utf16(d.end),
                        style: MarkStyle::Delim,
                    });
                }
            }
        }
    }
    out.sort_by_key(sort_key);
    out
}

fn sort_key(d: &Decoration) -> (usize, u8, usize) {
    match d {
        Decoration::Line { at, level } => (*at, 0, *level as usize),
        Decoration::Block { at, .. } => (*at, 0, 0),
        Decoration::Mark { from, to, .. } => (*from, 1, *to),
        Decoration::Conceal { from, to } => (*from, 1, *to),
        Decoration::Widget { from, to, .. } => (*from, 1, *to),
    }
}
