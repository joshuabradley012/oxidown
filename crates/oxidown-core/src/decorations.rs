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
//! Line-prefix marker constructs (list markers, task widgets, nested indent,
//! blockquote runs) use an alternate extent for this predicate — their WHOLE
//! LINE (see `parser::Node::reveal_extent`; contract v0.3): a cursor anywhere
//! on the line reveals all of its markers, matching heading semantics.
//! Fence lines likewise use the whole fenced block (block-level reveal).
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
use crate::text::{SrcBytes, TextBuffer};

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
    /// A NESTED list item's line (depth >= 2): the view supplies exact
    /// per-depth padding while the raw indent whitespace conceals.
    ListItem(u8),
}

impl BlockStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockStyle::BlockQuote(_) => "blockquote",
            BlockStyle::CodeBlock => "code-block",
            BlockStyle::CodeFence => "code-fence",
            BlockStyle::ThematicBreak => "hr",
            BlockStyle::ListItem(_) => "list-item",
        }
    }

    pub fn depth(&self) -> Option<u8> {
        match self {
            BlockStyle::BlockQuote(d) | BlockStyle::ListItem(d) => Some(*d),
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
    /// M1: blockquote/code-fence/code-block/hr/list-item line chrome.
    /// `revealed` is meaningful for `BlockQuote` and `ListItem`: the line's
    /// marker region is being edited (caret adjacent), so the view drops the
    /// line's decorative padding/bars and shows source geometry.
    Block {
        at: usize,
        style: BlockStyle,
        revealed: bool,
    },
    /// M1: a widget replacing a source span visually. Withheld (in favor of
    /// a mark over the same span) when the node is revealed or under active
    /// composition.
    Widget {
        from: usize,
        to: usize,
        kind: WidgetKind,
    },
}

/// Widget vocabulary (wire: the `widget` field of `{kind:"widget", ...}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetKind {
    /// A task item's checkbox, replacing the `[ ]`/`[x]` source span.
    /// Withheld as `mark:delim` on reveal.
    Task { checked: bool },
    /// An unordered list item's bullet, replacing the whole marker span
    /// (glyph + trailing whitespace, e.g. `"- "`). Withheld as
    /// `mark:list-marker` on reveal. Reveal is LINE-level (contract v0.3,
    /// matching every other marker construct): a cursor/selection touching
    /// any part of the item's line shows the raw marker instead.
    Bullet,
    /// An ordered list item's marker, replacing the whole marker span
    /// (digits + delimiter + trailing whitespace, e.g. `"1. "`) with the
    /// VIEW-COMPUTED CommonMark sequence number (contract v0.3 amendment;
    /// research/07 §0/§1.2: CommonMark only gives a list's `start` number
    /// meaning, so the core computes the displayed number from position-in-
    /// run here rather than rewriting source digits, unlike Obsidian).
    /// Withheld as `mark:list-marker` (raw source digits) on reveal —
    /// LINE-level, matching every other marker construct.
    Ordered { number: u64, delim: u8 },
}

/// Closed-interval intersection: empty ranges (cursors) at a boundary count.
fn touches(a: usize, b: usize, from: usize, to: usize) -> bool {
    a <= to && b >= from
}

/// Start of the last blank (whitespace-only) line at/before `from`, or 0 —
/// the windowing FLOOR for `compute`'s viewport filter. Found by a backward
/// byte scan (chunk-cached via [`SrcBytes`]: O(bytes back to the previous
/// blank line), which is exactly the region whose nodes can still overlap
/// the viewport). A `\r\n` pair is ONE terminator — the empty "segment"
/// between `\r` and `\n` must not read as a blank line, or the floor could
/// land where the spans-no-blank-line invariant doesn't hold.
fn blank_line_floor(text: &TextBuffer, from: usize) -> usize {
    let src = SrcBytes::new(text);
    // Start of the line containing `from` (which may sit mid-line).
    let mut ls = from.min(src.len());
    while ls > 0 && !matches!(src.byte(ls - 1), b'\n' | b'\r') {
        ls -= 1;
    }
    loop {
        if ls == 0 {
            return 0;
        }
        // Step onto the previous line: skip its terminator (`\n`, `\r`, or
        // the two-byte `\r\n`), then scan that line's content back to its
        // own start, checking for anything non-blank.
        let mut p = ls - 1;
        if src.byte(p) == b'\n' && p > 0 && src.byte(p - 1) == b'\r' {
            p -= 1;
        }
        let mut blank = true;
        while p > 0 && !matches!(src.byte(p - 1), b'\n' | b'\r') {
            if !matches!(src.byte(p - 1), b' ' | b'\t') {
                blank = false;
            }
            p -= 1;
        }
        if blank {
            return p;
        }
        ls = p;
    }
}

pub fn compute(
    nodes: &[Node],
    text: &TextBuffer,
    viewport: Range<usize>,
    selections: &[(usize, usize)],
    composition: Option<&Composition>,
) -> Vec<Decoration> {
    let mut out = Vec::new();
    // Viewport window over the overlay — O(window + log n), not a linear
    // scan of every node per call. `nodes` is sorted by `extent.start`
    // (`parse_document` stable-sorts; the editor's tail/incremental splice
    // paths preserve the order), so the window END is a plain
    // `partition_point`. The START cannot be: a node may begin BEFORE the
    // viewport and still overlap it, and such nodes are NOT a contiguous
    // suffix of the prefix — counterexample: `"> **a\n> b\n> c** d"` with a
    // viewport inside the third line has the strong node (spanning all
    // three lines) overlapping, but the per-line quote node of line two
    // (later start, no overlap) sits between it and the viewport, so
    // "back-scan until the first non-overlapping node" would drop the
    // strong. Instead the start is lower-bounded by a parser-wide
    // invariant: NO node's extent spans a blank (whitespace-only) line —
    // line-oriented kinds (headings, quote/fence/code lines, markers,
    // breaks) are single-line by construction, and inline kinds live inside
    // one leaf block (paragraph/heading/table row), which a blank line
    // always terminates. Every node overlapping the viewport therefore
    // starts at/after the last blank line at/before `viewport.start`
    // (anything starting earlier and ending inside the viewport would
    // contain that whole blank line). Both bounds are asserted in debug
    // builds, so every debug test run enforces the invariants this
    // windowing relies on.
    debug_assert!(
        nodes.windows(2).all(|w| w[0].extent.start <= w[1].extent.start),
        "compute requires the overlay sorted by extent.start"
    );
    let floor = blank_line_floor(text, viewport.start);
    let lo = nodes.partition_point(|n| n.extent.start < floor);
    let hi = nodes.partition_point(|n| n.extent.start < viewport.end);
    debug_assert!(
        nodes[..lo].iter().all(|n| n.extent.end <= viewport.start),
        "windowing floor violated: a node below the floor overlaps the \
         viewport (some extent spans a blank line?)"
    );
    for node in &nodes[lo..hi] {
        // Half-open overlap with the viewport; nodes have non-empty extents.
        if node.extent.start >= viewport.end || node.extent.end <= viewport.start {
            continue;
        }

        let reveal_extent = node.reveal_extent.as_ref().unwrap_or(&node.extent);
        let revealed = selections
            .iter()
            .any(|&(a, b)| touches(a, b, reveal_extent.start, reveal_extent.end));

        match node.kind {
            NodeKind::ListMarker { task, depth, number, delim } => {
                if node.extent.end <= node.extent.start {
                    continue;
                }
                // Every list item line carries a list-item line decoration
                // (all depths): the view uses it for hanging indent, so
                // wrapped item text aligns with the first line's text.
                out.push(Decoration::Block {
                    at: text.byte_to_utf16(node.extent.start),
                    style: BlockStyle::ListItem(depth),
                    revealed,
                });
                let from = text.byte_to_utf16(node.extent.start);
                let to = text.byte_to_utf16(node.extent.end);
                // pulldown folds up to 3 bytes of incidental leading
                // whitespace into a sibling item's span (`"- a\n - b"`), so
                // the marker glyph may sit past extent.start — probe past
                // blanks (mirrors commands.rs's `line_marker`).
                let mut glyph = node.extent.start;
                while glyph < node.extent.end
                    && matches!(text.byte_at(glyph), Some(b' ' | b'\t'))
                {
                    glyph += 1;
                }
                let is_bullet = matches!(text.byte_at(glyph), Some(b'-' | b'*' | b'+'));
                let in_composition = composition
                    .is_some_and(|c| touches(c.start, c.end, node.extent.start, node.extent.end));
                // Reveal is LINE-level (`revealed` uses the marker node's
                // reveal_extent = the item's whole first line): the raw
                // markers are editable whenever the cursor is on the line,
                // never one keystroke away from surprise.
                if is_bullet && task {
                    // Task items: the checkbox alone represents the item;
                    // the `- ` conceals/reveals in lockstep with it.
                    if revealed || in_composition {
                        out.push(Decoration::Mark {
                            from,
                            to,
                            style: MarkStyle::Delim,
                        });
                    } else {
                        out.push(Decoration::Conceal { from, to });
                    }
                } else if is_bullet {
                    if revealed || in_composition {
                        out.push(Decoration::Mark {
                            from,
                            to,
                            style: MarkStyle::ListMarker,
                        });
                    } else {
                        out.push(Decoration::Widget {
                            from,
                            to,
                            kind: WidgetKind::Bullet,
                        });
                    }
                } else if revealed || in_composition {
                    // Revealed (line-level, matching every other marker
                    // construct): raw source digits, unchanged from today.
                    out.push(Decoration::Mark {
                        from,
                        to,
                        style: MarkStyle::ListMarker,
                    });
                } else if let (Some(number), Some(delim)) = (number, delim) {
                    // Concealed: a computed-number WIDGET replacing the whole
                    // marker span — the view-computed display number
                    // (contract v0.3 amendment, research/07 §0/§1.2), never
                    // the item's raw source digits. Alignment (fixed-width,
                    // right-aligned, tabular numerals) is the view's job, the
                    // same box `mark:list-marker` used before.
                    out.push(Decoration::Widget {
                        from,
                        to,
                        kind: WidgetKind::Ordered { number, delim },
                    });
                } else {
                    // Defensive: an ordered marker (is_bullet == false) always
                    // carries number+delim from the parser; never silently
                    // drop the marker if that invariant is somehow violated.
                    out.push(Decoration::Mark {
                        from,
                        to,
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
                    out.push(Decoration::Widget {
                        from,
                        to,
                        kind: WidgetKind::Task { checked },
                    });
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
                    revealed,
                });
                None
            }
            NodeKind::CodeFenceLine => {
                out.push(Decoration::Block {
                    at: text.byte_to_utf16(node.extent.start),
                    style: BlockStyle::CodeFence,
                    revealed: false,
                });
                // Like the thematic break: the raw fence (``` + info string)
                // conceals unless the cursor is on the fence line — the
                // styled fence line itself reads as the code block's edge.
                let in_composition = composition
                    .is_some_and(|c| touches(c.start, c.end, node.extent.start, node.extent.end));
                let from = text.byte_to_utf16(node.extent.start);
                let to = text.byte_to_utf16(node.extent.end);
                if to > from {
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
                None
            }
            NodeKind::CodeBlockLine => {
                out.push(Decoration::Block {
                    at: text.byte_to_utf16(node.extent.start),
                    style: BlockStyle::CodeBlock,
                    revealed: false,
                });
                Some(MarkStyle::Code)
            }
            NodeKind::ListItemIndent { .. } => {
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
                    out.push(Decoration::Conceal { from, to });
                }
                None
            }
            NodeKind::ThematicBreak => {
                out.push(Decoration::Block {
                    at: text.byte_to_utf16(node.extent.start),
                    style: BlockStyle::ThematicBreak,
                    revealed: false,
                });
                // The `---` source participates in reveal like any delimiter:
                // concealed (the view draws the rule via the hr line style)
                // unless the cursor is on the line or composition touches it.
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
                    out.push(Decoration::Conceal { from, to });
                }
                None
            }
            NodeKind::ListMarker { .. } | NodeKind::TaskWidget { .. } => {
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
