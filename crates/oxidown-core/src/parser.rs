//! Phase A parser: full-document reparse per edit via pulldown-cmark 0.13
//! with `into_offset_iter()` for byte-exact spans.
//!
//! Only the M0 node set is extracted: ATX headings h1–h6, strong, emphasis,
//! inline code. Delimiter spans are computed from the event spans plus the
//! source bytes, so they are byte-exact including nested `***bold-italic***`
//! (pulldown nests Emphasis around Strong with spans offset by one byte).
//!
//! The parse result is a flat, document-ordered node list cached on the
//! editor; `decorations()` only filters this cache and never reparses.

use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// ATX heading; payload is the level 1..=6.
    Heading(u8),
    Strong,
    Emphasis,
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    /// Full node extent in bytes, **including delimiters**. This is the span
    /// the reveal predicate intersects with selections. For headings the
    /// trailing newline is excluded so a cursor at the start of the next line
    /// does not reveal the heading.
    pub extent: Range<usize>,
    /// Content span (between the delimiters). Empty for empty headings.
    pub content: Range<usize>,
    /// Delimiter spans: one for headings (`#`s + following space), two for
    /// strong/emphasis/inline code (opening and closing runs).
    pub delims: Vec<Range<usize>>,
}

/// Parse the full document and return the M0 overlay nodes in document order.
pub fn parse(src: &str) -> Vec<Node> {
    let bytes = src.as_bytes();
    let mut nodes = Vec::new();
    for (event, range) in Parser::new_ext(src, Options::empty()).into_offset_iter() {
        let node = match event {
            Event::Start(Tag::Heading { level, .. }) => heading_node(bytes, range, level),
            Event::Start(Tag::Strong) => inline_node(bytes, range, NodeKind::Strong, 2),
            Event::Start(Tag::Emphasis) => inline_node(bytes, range, NodeKind::Emphasis, 1),
            Event::Code(_) => code_node(bytes, range),
            _ => None,
        };
        nodes.extend(node);
    }
    // pulldown emits Start events in document order (outer before inner for
    // equal starts); a stable sort by start keeps that property.
    nodes.sort_by_key(|n| n.extent.start);
    nodes
}

fn heading_level_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// ATX heading. pulldown's heading span starts at the first `#` (even for
/// indented headings) and runs through the trailing newline. Setext headings
/// also arrive as Heading events but start with non-`#` — they are outside
/// the M0 decoration scope and are skipped.
fn heading_node(bytes: &[u8], range: Range<usize>, level: HeadingLevel) -> Option<Node> {
    let start = range.start;
    if bytes.get(start) != Some(&b'#') {
        return None; // setext heading: no M0 decorations
    }
    let mut hash_end = start;
    while hash_end < range.end && bytes.get(hash_end) == Some(&b'#') {
        hash_end += 1;
    }
    // Delimiter = the hashes plus the single following space/tab, per contract.
    let mut delim_end = hash_end;
    if delim_end < range.end && matches!(bytes.get(delim_end), Some(&b' ') | Some(&b'\t')) {
        delim_end += 1;
    }
    // Extent excludes the trailing line terminator.
    let mut extent_end = range.end;
    if extent_end > start && bytes.get(extent_end - 1) == Some(&b'\n') {
        extent_end -= 1;
    }
    if extent_end > start && bytes.get(extent_end - 1) == Some(&b'\r') {
        extent_end -= 1;
    }
    let delim_end = delim_end.min(extent_end).max(start);
    let delims = vec![Range {
        start,
        end: delim_end,
    }];
    Some(Node {
        kind: NodeKind::Heading(heading_level_u8(level)),
        extent: start..extent_end,
        content: delim_end..extent_end,
        delims,
    })
}

/// Strong (`**`/`__`, delimiter length 2) or emphasis (`*`/`_`, length 1).
/// The event span covers the whole node including delimiters; the delimiter
/// bytes are verified against the source (defensive: a mismatch drops the
/// node rather than emitting wrong spans).
fn inline_node(bytes: &[u8], range: Range<usize>, kind: NodeKind, dlen: usize) -> Option<Node> {
    let (s, e) = (range.start, range.end);
    if e < s + 2 * dlen || e > bytes.len() {
        return None;
    }
    let open = &bytes[s..s + dlen];
    let close = &bytes[e - dlen..e];
    let ch = open[0];
    if !(ch == b'*' || ch == b'_') {
        return None;
    }
    if !open.iter().all(|&b| b == ch) || !close.iter().all(|&b| b == ch) {
        return None;
    }
    Some(Node {
        kind,
        extent: s..e,
        content: (s + dlen)..(e - dlen),
        delims: vec![s..(s + dlen), (e - dlen)..e],
    })
}

/// Inline code. The event span covers the whole node including the backtick
/// runs; the run length is discovered from the source (`` `x` ``, ``` ``x`` ```,
/// …). Content keeps any CommonMark padding spaces — they are content bytes.
fn code_node(bytes: &[u8], range: Range<usize>) -> Option<Node> {
    let (s, e) = (range.start, range.end);
    if e > bytes.len() || s >= e {
        return None;
    }
    let mut n = 0;
    while s + n < e && bytes[s + n] == b'`' {
        n += 1;
    }
    if n == 0 || e < s + 2 * n {
        return None;
    }
    let mut m = 0;
    while m < n && bytes[e - 1 - m] == b'`' {
        m += 1;
    }
    if m != n {
        return None;
    }
    Some(Node {
        kind: NodeKind::Code,
        extent: s..e,
        content: (s + n)..(e - n),
        delims: vec![s..(s + n), (e - n)..e],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(src: &str) -> Node {
        let nodes = parse(src);
        assert_eq!(nodes.len(), 1, "expected exactly one node in {src:?}: {nodes:?}");
        nodes.into_iter().next().unwrap()
    }

    #[test]
    fn atx_heading_delimiter_includes_space() {
        let n = one("## Title\n");
        assert_eq!(n.kind, NodeKind::Heading(2));
        assert_eq!(n.extent, 0..8);
        assert_eq!(n.delims, vec![0..3]);
        assert_eq!(n.content, 3..8);
    }

    #[test]
    fn empty_heading() {
        let n = one("#\n");
        assert_eq!(n.delims, vec![0..1]);
        assert_eq!(n.content, 1..1);
        let n = one("# \n");
        assert_eq!(n.delims, vec![0..2]);
        assert_eq!(n.content, 2..2);
    }

    #[test]
    fn setext_heading_is_skipped() {
        assert!(parse("Title\n===\n").is_empty());
        assert!(parse("Title\n---\n").is_empty());
    }

    #[test]
    fn bold_italic_nesting() {
        let nodes = parse("***x***");
        assert_eq!(nodes.len(), 2);
        let em = &nodes[0];
        let strong = &nodes[1];
        assert_eq!(em.kind, NodeKind::Emphasis);
        assert_eq!(em.extent, 0..7);
        assert_eq!(em.delims, vec![0..1, 6..7]);
        assert_eq!(strong.kind, NodeKind::Strong);
        assert_eq!(strong.extent, 1..6);
        assert_eq!(strong.delims, vec![1..3, 4..6]);
        assert_eq!(strong.content, 3..4);
    }

    #[test]
    fn code_multi_backtick() {
        let n = one("``a ` b``");
        assert_eq!(n.kind, NodeKind::Code);
        assert_eq!(n.delims, vec![0..2, 7..9]);
        assert_eq!(n.content, 2..7);
    }

    #[test]
    fn not_headings() {
        assert!(parse("#no space\n").is_empty());
        assert!(parse("####### seven\n").is_empty());
    }
}
