//! Phase A parser: full-document reparse per edit via pulldown-cmark 0.13
//! with `into_offset_iter()` for byte-exact spans.
//!
//! M0 scope: ATX headings h1–h6, strong, emphasis, inline code.
//! M1 (v0.2) adds: strikethrough, links (inline + autolink/email), fenced
//! code blocks (fence + body lines), blockquotes (per-line, with depth),
//! lists (marker spans + task-item checkboxes), thematic breaks. GFM options
//! (tables, footnotes) are enabled so the parser recognizes those
//! constructs, but M1 emits no decorations for them (parser understands more
//! than it decorates, per plan.md §5.2).
//!
//! Delimiter/content spans are computed from event spans plus the source
//! bytes, never from event *text* payloads (which may be normalized/escaped
//! differently from the source) — this keeps every span byte-exact,
//! including nested constructs (`***bold-italic***`, links inside emphasis,
//! code spans containing delimiter-looking characters, lists inside
//! blockquotes). A key simplifying fact (verified empirically against this
//! pulldown-cmark version and pinned by the `span_spike` example): for every
//! construct this module decorates, the `Start` event's span already equals
//! the full node extent (delimiters included) — the matching `End` event
//! reports the identical range. So every node in this module (except list
//! markers/task widgets, and the blockquote per-line pass) is computed
//! directly at the `Start` event with no stack or deferred bookkeeping.

use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// ATX heading; payload is the level 1..=6.
    Heading(u8),
    Strong,
    Emphasis,
    Code,
    Strike,
    /// `autolink == true` for `<url>`/`<email>` (whole-span `mark:link`, no
    /// delimiters to conceal); `false` for inline `[text](url)` links.
    Link { autolink: bool },
    /// One quoted source line; payload is 1-based nesting depth.
    BlockQuoteLine(u8),
    /// A fenced code block's opening or closing fence line.
    CodeFenceLine,
    /// A fenced code block's body line.
    CodeBlockLine,
    ThematicBreak,
    /// A list-item marker run (`"- "`, `"1. "`). `task` marks the marker of
    /// a task item, which conceals/reveals in lockstep with its checkbox.
    /// Reveal is LINE-level (contract v0.3, matching headings): the reveal
    /// extent is the item's whole first line, so a caret anywhere on the
    /// line reveals every marker construct on it. `depth` is the 1-based
    /// list nesting depth (drives the view's hanging-indent line decoration).
    ///
    /// `number`/`delim` are present iff this is an ORDERED marker (`"1. "`,
    /// `"2) "`, …): `number` is the VIEW-COMPUTED CommonMark sequence
    /// position (the enclosing list's `start` plus this item's position in
    /// the run — never the item's raw source digits) and `delim` is the
    /// marker's delimiter byte (`b'.'` or `b')'`). Contract v0.3 amendment
    /// (research/07 §0/§1.2): CommonMark only gives a list's `start` number
    /// meaning, so the display number is computed here in the decoration
    /// pipeline — the core never rewrites source digits (unlike Obsidian's
    /// renumber-by-rewriting-the-file, and unlike this crate's own
    /// `indentList`/`outdentList`, which only ever rewrites a marker's
    /// digits when CommonMark parsing itself would otherwise fail). Both
    /// `None` for unordered (bullet/plus/asterisk) markers.
    ListMarker { task: bool, depth: u8, number: Option<u64>, delim: Option<u8> },
    /// The leading indentation whitespace of a NESTED list item (depth >= 2).
    /// Emits a `line:list-item` decoration carrying the depth (the view
    /// provides exact per-depth padding) and conceals the raw spaces.
    ListItemIndent { depth: u8 },
    /// A task item's checkbox span (`[ ]`/`[x]`), rendered as a widget
    /// unless revealed.
    TaskWidget { checked: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    /// Full node extent in bytes, **including delimiters**. This is the span
    /// the reveal predicate intersects with selections by default (see
    /// `reveal_extent` for nodes that use a different extent). For
    /// line-oriented kinds (headings, blockquote lines, fence/code-block
    /// lines, thematic breaks) the trailing newline is excluded.
    pub extent: Range<usize>,
    /// Content span (between the delimiters, or the line body for
    /// code-block lines). Empty when the kind has no content mark.
    pub content: Range<usize>,
    /// Delimiter spans, reveal-gated (conceal when not revealed, `mark:delim`
    /// when revealed or composing). Empty for kinds that are never concealed
    /// (list markers, code fences/blocks, thematic breaks, autolinks).
    pub delims: Vec<Range<usize>>,
    /// Link destination span — only present for non-autolink `Link` nodes;
    /// emitted as `mark:url` when the node is revealed.
    pub url: Option<Range<usize>>,
    /// Alternate extent used for the reveal *predicate* only (not for
    /// viewport filtering, which still uses `extent`). List markers, nested
    /// indents, and task widgets carry their item's WHOLE LINE (line-level
    /// reveal, contract v0.3); fence lines carry the whole fenced block
    /// (block-level reveal).
    pub reveal_extent: Option<Range<usize>>,
    /// The enclosing list item's full extent — only present for task
    /// widgets, where the `toggleTask` command needs "pos anywhere in the
    /// list item" resolution (the item's span, trailing newline included,
    /// exactly as pulldown reports it).
    pub item_extent: Option<Range<usize>>,
}

impl Node {
    /// Shift every span in this node by `by` bytes — used when a node was
    /// parsed from a document *slice* (the streaming tail fast path) and its
    /// spans must be rebased onto whole-document coordinates.
    pub fn offset(&mut self, by: usize) {
        let shift = |r: &mut Range<usize>| {
            r.start += by;
            r.end += by;
        };
        shift(&mut self.extent);
        shift(&mut self.content);
        for d in &mut self.delims {
            shift(d);
        }
        if let Some(u) = &mut self.url {
            shift(u);
        }
        if let Some(r) = &mut self.reveal_extent {
            shift(r);
        }
        if let Some(r) = &mut self.item_extent {
            shift(r);
        }
    }

    /// Signed variant of [`offset`](Self::offset): shift every span by
    /// `by` (negative for deletions) — used by the incremental reparse to
    /// rebase the kept-after portion of the overlay through an edit's net
    /// length delta. Every shifted position is by construction at/after the
    /// edit, so the result never underflows.
    pub fn offset_signed(&mut self, by: isize) {
        let shift = |r: &mut Range<usize>| {
            r.start = (r.start as isize + by) as usize;
            r.end = (r.end as isize + by) as usize;
        };
        shift(&mut self.extent);
        shift(&mut self.content);
        for d in &mut self.delims {
            shift(d);
        }
        if let Some(u) = &mut self.url {
            shift(u);
        }
        if let Some(r) = &mut self.reveal_extent {
            shift(r);
        }
        if let Some(r) = &mut self.item_extent {
            shift(r);
        }
    }
}

fn leaf(kind: NodeKind, extent: Range<usize>, content: Range<usize>, delims: Vec<Range<usize>>) -> Node {
    Node {
        kind,
        extent,
        content,
        delims,
        url: None,
        reveal_extent: None,
        item_extent: None,
    }
}

/// Top-level block kinds for the block index (plan.md §5.3). Defined here —
/// not in `block_index` — because the kinds+spans are produced by the same
/// single parse pass that builds the overlay nodes (one pulldown walk per
/// edit, not two).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading,
    BlockQuote,
    List,
    CodeBlock,
    ThematicBreak,
    Table,
    FootnoteDefinition,
    HtmlBlock,
}

fn tag_block_kind(tag: &Tag) -> Option<BlockKind> {
    match tag {
        Tag::Paragraph => Some(BlockKind::Paragraph),
        Tag::Heading { .. } => Some(BlockKind::Heading),
        Tag::BlockQuote(_) => Some(BlockKind::BlockQuote),
        Tag::List(_) => Some(BlockKind::List),
        Tag::CodeBlock(_) => Some(BlockKind::CodeBlock),
        Tag::Table(_) => Some(BlockKind::Table),
        Tag::FootnoteDefinition(_) => Some(BlockKind::FootnoteDefinition),
        Tag::HtmlBlock => Some(BlockKind::HtmlBlock),
        _ => None,
    }
}

/// Everything one full parse pass produces: the decoration overlay plus the
/// top-level block structure (kind + full span per block, document order)
/// that feeds the block index.
#[derive(Debug)]
pub struct ParseResult {
    pub nodes: Vec<Node>,
    pub blocks: Vec<(BlockKind, Range<usize>)>,
}

fn options() -> Options {
    let mut o = Options::empty();
    o.insert(Options::ENABLE_STRIKETHROUGH);
    o.insert(Options::ENABLE_TASKLISTS);
    o.insert(Options::ENABLE_TABLES);
    o.insert(Options::ENABLE_FOOTNOTES);
    o
}

/// Parse the full document and return only the M0+M1 overlay nodes in
/// document (extent-start) order. Thin wrapper over [`parse_document`], kept
/// for callers/tests that don't need the block structure.
pub fn parse(src: &str) -> Vec<Node> {
    parse_document(src).nodes
}

/// Parse the full document in **one** pulldown pass, producing both the
/// decoration overlay and the top-level block structure (see
/// [`ParseResult`]). One walk, not two — the block index and the overlay
/// share this per-edit cost (plan.md §5.2's Phase-A "full reparse per edit"
/// is a single reparse).
pub fn parse_document(src: &str) -> ParseResult {
    let bytes = src.as_bytes();
    let mut nodes = Vec::new();
    let mut blocks: Vec<(BlockKind, Range<usize>)> = Vec::new();
    // Container nesting depth over *all* tags — only `depth == 0` Starts are
    // top-level blocks.
    let mut depth: u32 = 0;
    // Blockquote nesting depth intervals, recorded at `Start` (the span is
    // already the full node extent); resolved into per-line nodes after the
    // full pass so a line's depth reflects the deepest blockquote covering
    // it (nested blockquotes close only after the outer one's span is fully
    // known in event order, but the *outer* Start fires first — depth is
    // correct immediately at each Start, the two-pass split is only needed
    // because per-line generation must see every interval, inner ones
    // included, before it can pick the max depth for a given line).
    let mut bq_depth: u8 = 0;
    let mut bq_intervals: Vec<(Range<usize>, u8)> = Vec::new();
    // A list item's marker is resolved by lookahead to the *next* event
    // (whatever it is): the marker's width follows CommonMark's
    // content-indentation rules, which pulldown has already computed for us
    // by locating where the item's real content begins. The full item range
    // is kept alongside so task widgets can record their item's extent. The
    // third element is this item's VIEW-COMPUTED ordered sequence number
    // (`None` for bullet-list items), captured at `Start(Tag::Item)` — see
    // `list_seq` below.
    let mut pending_item: Option<(Range<usize>, u8, Option<u64>)> = None;
    let mut list_depth: u8 = 0;
    // Per-open-list running sequence counter, parallel to `list_depth`'s
    // push/pop (one frame per currently-open `List`, nested lists push their
    // own on top): `Some(n)` for an ordered list (`Tag::List(Some(start))`),
    // holding the NEXT number to assign; `None` for a bullet list, where
    // there is nothing to compute. Popped at `End(List)`. This is what turns
    // "1./1./3." into a displayed "1,2,3" (research/07 §0/§1.2): CommonMark
    // only gives a list's `start` meaning, so the sequence is derived purely
    // from position-in-run, never from the item's own literal digits.
    let mut list_seq: Vec<Option<u64>> = Vec::new();

    for (event, range) in Parser::new_ext(src, options()).into_offset_iter() {
        // Top-level block collection (kind + full span at `Start`).
        match &event {
            Event::Start(tag) => {
                if depth == 0 {
                    if let Some(kind) = tag_block_kind(tag) {
                        blocks.push((kind, range.clone()));
                    }
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
            }
            Event::Rule if depth == 0 => {
                blocks.push((BlockKind::ThematicBreak, range.clone()));
            }
            _ => {}
        }

        if let Some((item, item_depth, item_number)) = pending_item.take() {
            let item_start = item.start;
            // Nested items (depth >= 2): the run of spaces/tabs immediately
            // before the marker is its own node — a `line:list-item` line
            // decoration (per-depth padding in the view) + concealed spaces.
            // Scanning back only over blanks keeps blockquote `>` markers
            // (their own nodes) out of the span.
            let mut ws_start = item_start;
            while ws_start > 0 && matches!(bytes[ws_start - 1], b' ' | b'\t') {
                ws_start -= 1;
            }
            if item_depth >= 2 && ws_start < item_start {
                nodes.push(leaf(
                    NodeKind::ListItemIndent { depth: item_depth },
                    ws_start..item_start,
                    item_start..item_start,
                    vec![],
                ));
            }
            // Reveal is LINE-level (matching headings, contract v0.3): a
            // cursor/selection touching ANY part of the item's first line
            // reveals its marker constructs — the marker glyphs, the task
            // brackets, and the nested leading indent all flip to source in
            // lockstep. (Replaces the earlier glyph-adjacency model: reveal
            // no longer depends on which character the caret touches.)
            let line = line_bounds(bytes, item_start);
            if item_depth >= 2 && ws_start < item_start {
                if let Some(last) = nodes.last_mut() {
                    if matches!(last.kind, NodeKind::ListItemIndent { .. }) {
                        last.reveal_extent = Some(line.clone());
                    }
                }
            }
            if let Event::TaskListMarker(checked) = &event {
                if range.start > item_start {
                    let delim = ordered_marker_delim(bytes, item_start, range.start);
                    let number = delim.and(item_number);
                    let mut marker = leaf(
                        NodeKind::ListMarker { task: true, depth: item_depth, number, delim },
                        item_start..range.start,
                        range.end..range.end,
                        vec![],
                    );
                    marker.reveal_extent = Some(line.clone());
                    nodes.push(marker);
                }
                let mut task = leaf(NodeKind::TaskWidget { checked: *checked }, range.clone(), range.end..range.end, vec![]);
                task.reveal_extent = Some(line.clone());
                task.item_extent = Some(item);
                nodes.push(task);
            } else {
                // Marker end: normally the next event's start (pulldown's
                // lookahead — the item's real content begins there). An
                // EMPTY item (`"- \n"`, `"1. \n"`, a bare `"-"`) emits
                // nothing between `Start(Item)` and `End(Item)`, so the
                // materializing event is the item's own End, whose range
                // starts back at `item_start` — the marker token is then
                // SYNTHESIZED by scanning the source bytes directly (glyphs
                // + delimiter + the single trailing space if present),
                // exactly as if content followed. Without this an empty
                // item had NO marker node at all: no bullet/ordered widget,
                // no `line:list-item` decoration, and the `enter` command's
                // empty-item exit rules couldn't see the item. Empty items
                // still consumed their `list_seq` slot at `Start(Item)`, so
                // ordered numbering counts them like any sibling.
                let marker_end = if range.start > item_start {
                    range.start
                } else {
                    empty_item_marker_end(bytes, item_start)
                };
                if marker_end > item_start {
                    let delim = ordered_marker_delim(bytes, item_start, marker_end);
                    let number = delim.and(item_number);
                    let mut marker = leaf(
                        NodeKind::ListMarker { task: false, depth: item_depth, number, delim },
                        item_start..marker_end,
                        marker_end..marker_end,
                        vec![],
                    );
                    marker.reveal_extent = Some(line);
                    nodes.push(marker);
                }
            }
        }

        match &event {
            Event::Start(Tag::Heading { level, .. }) => {
                nodes.extend(heading_node(bytes, range.clone(), *level));
            }
            Event::Start(Tag::Strong) => {
                nodes.extend(inline_delim_node(bytes, range.clone(), NodeKind::Strong, b'*', 2));
            }
            Event::Start(Tag::Emphasis) => {
                nodes.extend(inline_delim_node(bytes, range.clone(), NodeKind::Emphasis, b'*', 1));
            }
            Event::Start(Tag::Strikethrough) => {
                nodes.extend(inline_delim_node(bytes, range.clone(), NodeKind::Strike, b'~', 2));
            }
            Event::Code(_) => {
                nodes.extend(code_node(bytes, range.clone()));
            }
            Event::Start(Tag::Link { link_type, .. }) => {
                nodes.extend(link_node(bytes, range.clone(), *link_type));
            }
            Event::Start(Tag::BlockQuote(_)) => {
                bq_depth += 1;
                bq_intervals.push((range.clone(), bq_depth));
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                bq_depth = bq_depth.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                if kind.is_fenced() {
                    nodes.extend(fenced_code_lines(bytes, range.clone()));
                }
            }
            Event::Rule => {
                nodes.extend(thematic_break_node(bytes, range.clone()));
            }
            Event::Start(Tag::List(start)) => {
                list_depth = list_depth.saturating_add(1);
                // CommonMark: the list's displayed sequence begins at its
                // `start` (pulldown reports the first item's literal digits
                // here for an ordered list; `None` for a bullet list, carried
                // through unused). A delimiter change (`.` vs `)`) or a
                // marker-kind change ends the enclosing list and opens a NEW
                // one per CommonMark (verified against this pulldown-cmark
                // version), so this push always starts a fresh, correctly-
                // seeded counter.
                list_seq.push(*start);
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                list_seq.pop();
            }
            Event::Start(Tag::Item) => {
                // Consume this list's next number (ordered lists only) and
                // advance the counter for the following sibling — the
                // per-open-list running sequence `list_seq` tracks.
                let number = match list_seq.last_mut() {
                    Some(slot @ Some(_)) => {
                        let n = slot.unwrap();
                        *slot = Some(n + 1);
                        Some(n)
                    }
                    _ => None,
                };
                pending_item = Some((range.clone(), list_depth, number));
            }
            _ => {}
        }
    }

    // Per-line blockquote nodes: only walk lines within *top-level*
    // (depth == 1) intervals — nested intervals are subsets of those in
    // byte range, and `depth_at` picks the deepest interval covering each
    // line, so every line is visited exactly once regardless of nesting.
    for (range, depth) in bq_intervals.iter().filter(|(_, d)| *d == 1) {
        for line in split_lines(bytes, range.clone()) {
            if line.start >= line.end {
                continue;
            }
            let line_depth = depth_at_line(&bq_intervals, &line);
            if line_depth == 0 {
                continue;
            }
            let delims = blockquote_markers(bytes, line.clone());
            let _ = depth; // depth of the enclosing top-level interval; line_depth is authoritative
            // Reveal is LINE-level (matching headings, contract v0.3): the
            // node's extent IS the line (terminator excluded by split_lines),
            // and the default reveal predicate uses the extent — a cursor
            // anywhere on the line reveals the `>` run.
            let node = leaf(NodeKind::BlockQuoteLine(line_depth), line.clone(), line.end..line.end, delims);
            nodes.push(node);
        }
    }

    // pulldown emits Start events in document order (outer before inner for
    // equal starts); a stable sort by start keeps that property. Blockquote
    // line nodes are appended after the main pass, so re-sorting restores
    // document order for them too.
    nodes.sort_by_key(|n| n.extent.start);
    ParseResult { nodes, blocks }
}

/// Deepest interval *overlapping* `line` at all (not merely containing its
/// start byte). A nested blockquote's own recorded interval starts only at
/// its own marker, not at the outer marker sharing the same physical line
/// (e.g. in `"> > inner"`, the inner interval starts at the second `>`), so
/// point-containment against the line's first byte would under-count depth
/// for every line whose nested marker isn't at column 0. Overlap is exactly
/// what "this line is (at least partly) inside this blockquote" means.
fn depth_at_line(intervals: &[(Range<usize>, u8)], line: &Range<usize>) -> u8 {
    intervals
        .iter()
        .filter(|(r, _)| r.start < line.end && r.end > line.start)
        .map(|(_, d)| *d)
        .max()
        .unwrap_or(0)
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
    Some(leaf(
        NodeKind::Heading(heading_level_u8(level)),
        start..extent_end,
        delim_end..extent_end,
        delims,
    ))
}

/// Strong/emphasis (`dlen` 2/1, char `*` or `_`) or strikethrough (`dlen` 2,
/// char `~`). The event span covers the whole node including delimiters; the
/// delimiter bytes are verified against the source (defensive: a mismatch
/// drops the node rather than emitting wrong spans).
fn inline_delim_node(
    bytes: &[u8],
    range: Range<usize>,
    kind: NodeKind,
    expect_ch: u8,
    dlen: usize,
) -> Option<Node> {
    let (s, e) = (range.start, range.end);
    if e < s + 2 * dlen || e > bytes.len() {
        return None;
    }
    let open = &bytes[s..s + dlen];
    let close = &bytes[e - dlen..e];
    let ch = open[0];
    if expect_ch == b'*' {
        if !(ch == b'*' || ch == b'_') {
            return None;
        }
    } else if ch != expect_ch {
        return None;
    }
    if !open.iter().all(|&b| b == ch) || !close.iter().all(|&b| b == ch) {
        return None;
    }
    Some(leaf(
        kind,
        s..e,
        (s + dlen)..(e - dlen),
        vec![s..(s + dlen), (e - dlen)..e],
    ))
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
    Some(leaf(
        NodeKind::Code,
        s..e,
        (s + n)..(e - n),
        vec![s..(s + n), (e - n)..e],
    ))
}

/// Link (inline `[text](url)`, autolink `<url>`, email autolink
/// `<email>`). Reference/collapsed/shortcut links and wikilinks are outside
/// the M1 decoration scope (the parser still recognizes them; they simply
/// emit no node) — the contract only names `[text](url)` and autolinks.
fn link_node(bytes: &[u8], range: Range<usize>, link_type: LinkType) -> Option<Node> {
    let (start, end) = (range.start, range.end);
    if end > bytes.len() || start >= end {
        return None;
    }
    match link_type {
        LinkType::Autolink | LinkType::Email => {
            if bytes.get(start) != Some(&b'<') || bytes.get(end - 1) != Some(&b'>') {
                return None;
            }
            Some(leaf(NodeKind::Link { autolink: true }, start..end, start..end, vec![]))
        }
        LinkType::Inline => {
            if bytes.get(start) != Some(&b'[') || bytes.get(end - 1) != Some(&b')') {
                return None;
            }
            // Scan backward from the closing ')' tracking paren depth to
            // find the matching '(' that opens the destination part —
            // handles balanced parens inside the URL itself.
            let mut i = end - 1;
            let mut depth = 0i32;
            let paren_open = loop {
                match bytes[i] {
                    b')' => depth += 1,
                    b'(' => {
                        depth -= 1;
                        if depth == 0 {
                            break i;
                        }
                    }
                    _ => {}
                }
                if i == start {
                    return None;
                }
                i -= 1;
            };
            if paren_open == start || bytes.get(paren_open - 1) != Some(&b']') {
                return None;
            }
            let close_bracket = paren_open - 1;
            let text = (start + 1)..close_bracket;
            let dest_start = paren_open + 1;
            let mut j = dest_start;
            while j < end - 1 && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                j += 1;
            }
            let url = if bytes.get(j) == Some(&b'<') {
                let mut k = j + 1;
                while k < end - 1 && bytes[k] != b'>' {
                    k += 1;
                }
                let close = (k + 1).min(end - 1);
                j..close
            } else {
                let mut k = j;
                while k < end - 1 && !matches!(bytes[k], b' ' | b'\t' | b'\n' | b'\r') {
                    k += 1;
                }
                j..k
            };
            let mut node = leaf(
                NodeKind::Link { autolink: false },
                start..end,
                text,
                vec![start..(start + 1), close_bracket..end],
            );
            node.url = Some(url);
            Some(node)
        }
        _ => None,
    }
}

fn thematic_break_node(bytes: &[u8], range: Range<usize>) -> Option<Node> {
    let start = range.start;
    let mut end = range.end;
    if end > start && bytes.get(end - 1) == Some(&b'\n') {
        end -= 1;
    }
    if end > start && bytes.get(end - 1) == Some(&b'\r') {
        end -= 1;
    }
    if end <= start {
        return None;
    }
    Some(leaf(NodeKind::ThematicBreak, start..end, end..end, vec![]))
}

/// The delimiter byte (`.` or `)`) of an ORDERED marker occupying
/// `start..end` (the marker glyphs plus its required trailing space, e.g.
/// `"1. "`/`"10) "`), or `None` if `start` doesn't begin with an ASCII-digit
/// run followed by one of those two bytes (i.e. this is a bullet marker, or
/// malformed). Read directly from source bytes — the parser's own lookahead
/// already located the marker span; this just classifies it.
/// Marker-token end for an EMPTY list item, synthesized directly from the
/// source bytes because pulldown emits no content event whose start would
/// otherwise locate it: leading fold-in whitespace (pulldown can fold a few
/// bytes of incidental indentation into an item's span), the marker glyphs
/// (`-`/`+`/`*` or a digit run plus `.`/`)`), and the single trailing
/// space/tab IF PRESENT (a bare `"-"` with no trailing space is still an
/// empty item per CommonMark/pulldown — its marker is just the glyph).
/// Returns `item_start` (an empty span — no marker emitted) if no marker
/// shape is found; unreachable for spans pulldown reported as items, but
/// never worth corrupting spans over.
fn empty_item_marker_end(bytes: &[u8], item_start: usize) -> usize {
    let mut i = item_start;
    while matches!(bytes.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    match bytes.get(i) {
        Some(b'-' | b'+' | b'*') => i += 1,
        Some(b) if b.is_ascii_digit() => {
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            match bytes.get(i) {
                Some(b'.' | b')') => i += 1,
                _ => return item_start,
            }
        }
        _ => return item_start,
    }
    if matches!(bytes.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    i
}

fn ordered_marker_delim(bytes: &[u8], start: usize, end: usize) -> Option<u8> {
    if !bytes.get(start).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut i = start;
    while i < end && bytes[i].is_ascii_digit() {
        i += 1;
    }
    match bytes.get(i) {
        Some(&b) if b == b'.' || b == b')' => Some(b),
        _ => None,
    }
}

/// Split a byte range into per-source-line sub-ranges, each excluding its
/// own trailing `\n` (and a preceding `\r`, for CRLF). A final line with no
/// trailing terminator (partial line at the range's end) is still included.
/// The full line containing `pos`: from just after the previous `\n` through
/// the line's end, terminator excluded (same bounds discipline as
/// `split_lines`). Used for LINE-level reveal extents (contract v0.3).
fn line_bounds(bytes: &[u8], pos: usize) -> Range<usize> {
    let mut start = pos;
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    let mut end = pos;
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    if end > start && end < bytes.len() && bytes.get(end - 1) == Some(&b'\r') {
        end -= 1;
    }
    start..end
}

fn split_lines(bytes: &[u8], range: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut pos = range.start;
    while pos < range.end {
        let mut nl = pos;
        while nl < range.end && bytes[nl] != b'\n' {
            nl += 1;
        }
        let mut line_end = nl;
        if line_end > pos && bytes.get(line_end.wrapping_sub(1)) == Some(&b'\r') && nl < range.end {
            line_end -= 1;
        }
        out.push(pos..line_end);
        pos = if nl < range.end { nl + 1 } else { range.end };
    }
    out
}

/// Blockquote marker run(s) at the start of a line: zero to three leading
/// spaces, a `>`, and one optional following space/tab — repeated for
/// nested markers on the same line (`> > text`). A lazily-continued line
/// (no literal `>`) yields no markers even though its logical depth (from
/// `depth_at`) may be greater than zero — the conceal set only ever covers
/// bytes that are actually present.
fn blockquote_markers(bytes: &[u8], line: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut pos = line.start;
    loop {
        let marker_start = pos;
        let mut p = pos;
        let mut spaces = 0;
        while p < line.end && spaces < 3 && bytes[p] == b' ' {
            p += 1;
            spaces += 1;
        }
        if p >= line.end || bytes[p] != b'>' {
            break;
        }
        p += 1;
        if p < line.end && matches!(bytes[p], b' ' | b'\t') {
            p += 1;
        }
        out.push(marker_start..p);
        pos = p;
    }
    out
}

/// Detect a fence line's marker char (`` ` `` or `~`) and run length, after
/// up to 3 leading spaces (CommonMark's indented-fence allowance).
fn detect_fence(bytes: &[u8], line: Range<usize>) -> Option<(u8, usize)> {
    let mut p = line.start;
    let mut spaces = 0;
    while p < line.end && spaces < 3 && bytes[p] == b' ' {
        p += 1;
        spaces += 1;
    }
    if p >= line.end || !matches!(bytes[p], b'`' | b'~') {
        return None;
    }
    let ch = bytes[p];
    let start = p;
    while p < line.end && bytes[p] == ch {
        p += 1;
    }
    let len = p - start;
    if len < 3 {
        return None;
    }
    Some((ch, len))
}

/// Whether `line` is a valid closing fence for a fence opened with
/// `(fence_ch, fence_len)`: same char, at least as many repeats, and nothing
/// but trailing whitespace after.
fn is_closing_fence(bytes: &[u8], line: Range<usize>, fence_ch: u8, fence_len: usize) -> bool {
    let mut p = line.start;
    let mut spaces = 0;
    while p < line.end && spaces < 3 && bytes[p] == b' ' {
        p += 1;
        spaces += 1;
    }
    let run_start = p;
    while p < line.end && bytes[p] == fence_ch {
        p += 1;
    }
    if p - run_start < fence_len {
        return false;
    }
    bytes[p..line.end].iter().all(|&b| b == b' ' || b == b'\t')
}

/// Fenced code block: opening fence line (`line:code-fence`), body lines
/// (`line:code-block` + `mark:code`), and — if present — the closing fence
/// line (`line:code-fence`). Fence lines carry a BLOCK-level reveal extent
/// (the whole fence-to-fence range): a cursor anywhere inside the block
/// reveals both raw fences, so they are visible while the code is being
/// edited. Derived by scanning the raw source within the block's extent
/// (already the full fence-to-fence span at `Start` time), not from `Text`
/// event payloads — robust to however pulldown chunks the body into `Text`
/// events.
fn fenced_code_lines(bytes: &[u8], range: Range<usize>) -> Vec<Node> {
    let mut out = Vec::new();
    let block = range.clone();
    let lines = split_lines(bytes, range);
    if lines.is_empty() {
        return out;
    }
    let open = lines[0].clone();
    let mut open_node = leaf(NodeKind::CodeFenceLine, open.clone(), open.end..open.end, vec![]);
    open_node.reveal_extent = Some(block.clone());
    out.push(open_node);
    let Some((fence_ch, fence_len)) = detect_fence(bytes, open) else {
        // Malformed/unexpected: still emit the rest as body lines rather
        // than dropping them.
        for line in &lines[1..] {
            out.push(leaf(NodeKind::CodeBlockLine, line.clone(), line.clone(), vec![]));
        }
        return out;
    };
    let mut body_end_idx = lines.len();
    if lines.len() > 1 {
        let last = lines[lines.len() - 1].clone();
        if is_closing_fence(bytes, last.clone(), fence_ch, fence_len) {
            body_end_idx = lines.len() - 1;
        }
    }
    for line in &lines[1..body_end_idx] {
        out.push(leaf(NodeKind::CodeBlockLine, line.clone(), line.clone(), vec![]));
    }
    if body_end_idx < lines.len() {
        let close = lines[body_end_idx].clone();
        let mut close_node = leaf(NodeKind::CodeFenceLine, close.clone(), close.end..close.end, vec![]);
        close_node.reveal_extent = Some(block);
        out.push(close_node);
    }
    out
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

    // ---------------------------------------------------------- M1: GFM --

    #[test]
    fn strikethrough_span() {
        let n = one("~~del~~");
        assert_eq!(n.kind, NodeKind::Strike);
        assert_eq!(n.delims, vec![0..2, 5..7]);
        assert_eq!(n.content, 2..5);
    }

    #[test]
    fn inline_link_span() {
        let n = one("[text](http://example.com)");
        assert_eq!(n.kind, NodeKind::Link { autolink: false });
        assert_eq!(n.extent, 0..26);
        assert_eq!(n.content, 1..5);
        assert_eq!(n.delims, vec![0..1, 5..26]);
        assert_eq!(n.url, Some(7..25));
    }

    #[test]
    fn inline_link_with_title() {
        let n = one("[text](http://example.com \"title\")");
        assert_eq!(n.url, Some(7..25));
    }

    #[test]
    fn autolink_span_has_no_delims() {
        let n = one("<http://example.com>");
        assert_eq!(n.kind, NodeKind::Link { autolink: true });
        assert_eq!(n.extent, 0..20);
        assert_eq!(n.content, 0..20);
        assert!(n.delims.is_empty());
    }

    #[test]
    fn email_autolink() {
        let n = one("<foo@example.com>");
        assert_eq!(n.kind, NodeKind::Link { autolink: true });
    }

    #[test]
    fn link_inside_emphasis_nests() {
        let nodes = parse("*[text](url)*");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].kind, NodeKind::Emphasis);
        assert_eq!(nodes[1].kind, NodeKind::Link { autolink: false });
    }

    #[test]
    fn thematic_break_span() {
        let nodes = parse("a\n\n---\n\nb\n");
        let hr: Vec<_> = nodes.iter().filter(|n| n.kind == NodeKind::ThematicBreak).collect();
        assert_eq!(hr.len(), 1);
        assert_eq!(hr[0].extent, 3..6);
    }

    #[test]
    fn unordered_list_marker() {
        let nodes = parse("- one\n- two\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].extent, 0..2);
        assert_eq!(markers[1].extent, 6..8);
    }

    #[test]
    fn ordered_list_marker() {
        let nodes = parse("1. one\n2. two\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers[0].extent, 0..3);
        assert_eq!(markers[1].extent, 7..10);
    }

    fn ordered_number_delim(n: &Node) -> (Option<u64>, Option<u8>) {
        match n.kind {
            NodeKind::ListMarker { number, delim, .. } => (number, delim),
            _ => panic!("not a ListMarker: {n:?}"),
        }
    }

    #[test]
    fn bullet_markers_carry_no_ordered_info() {
        let nodes = parse("- one\n- two\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        for m in markers {
            assert_eq!(ordered_number_delim(m), (None, None));
        }
    }

    #[test]
    fn ordered_markers_get_sequential_view_computed_numbers() {
        let nodes = parse("1. a\n2. b\n3. c\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers.len(), 3);
        assert_eq!(ordered_number_delim(markers[0]), (Some(1), Some(b'.')));
        assert_eq!(ordered_number_delim(markers[1]), (Some(2), Some(b'.')));
        assert_eq!(ordered_number_delim(markers[2]), (Some(3), Some(b'.')));
    }

    #[test]
    fn ordered_markers_ignore_raw_digits_and_display_sequential_numbers() {
        // "1./1./3." must DISPLAY 1,2,3 — CommonMark only fixes the list's
        // start number; sibling digits are cosmetic (research/07 §0).
        let nodes = parse("1. a\n1. b\n3. c\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(
            markers.iter().map(|m| ordered_number_delim(m).0).collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3)]
        );
    }

    #[test]
    fn ordered_list_start_number_is_honored() {
        // "4./5./9." displays 4,5,6 (start=4, then strictly sequential).
        let nodes = parse("4. a\n5. b\n9. c\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(
            markers.iter().map(|m| ordered_number_delim(m).0).collect::<Vec<_>>(),
            vec![Some(4), Some(5), Some(6)]
        );
    }

    #[test]
    fn ordered_delimiter_change_starts_a_new_list_and_resets_the_sequence() {
        // Per CommonMark, a delimiter change (`.` vs `)`) ends the enclosing
        // list and starts a new one — verified directly against this
        // pulldown-cmark version. The new list's own `start` (its own first
        // item's literal digits) seeds a fresh counter.
        let nodes = parse("1. a\n2) b\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(ordered_number_delim(markers[0]), (Some(1), Some(b'.')));
        assert_eq!(ordered_number_delim(markers[1]), (Some(2), Some(b')')));
    }

    #[test]
    fn nested_ordered_list_restarts_its_own_sequence() {
        let nodes = parse("1. a\n   1. nested\n   2. nested2\n2. b\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        // Document order: "a" (depth1,#1), "nested" (depth2,#1),
        // "nested2" (depth2,#2), "b" (depth1,#2) — the nested list's own
        // counter is independent of (and restarts relative to) its parent's.
        assert_eq!(
            markers.iter().map(|m| ordered_number_delim(m).0).collect::<Vec<_>>(),
            vec![Some(1), Some(1), Some(2), Some(2)]
        );
    }

    #[test]
    fn nested_ordered_list_under_a_bullet_gets_its_own_sequence() {
        let doc = "- a\n  1. one\n  2. two\n- b\n";
        let nodes = parse(doc);
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        // "a" and "b" are bullets (no ordered info); the nested ordered pair
        // gets 1, 2.
        assert_eq!(
            markers.iter().map(|m| ordered_number_delim(m).0).collect::<Vec<_>>(),
            vec![None, Some(1), Some(2), None]
        );
    }

    #[test]
    fn ordered_list_inside_a_blockquote_computes_numbers_too() {
        let nodes = parse("> 1. a\n> 2. b\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(
            markers.iter().map(|m| ordered_number_delim(m).0).collect::<Vec<_>>(),
            vec![Some(1), Some(2)]
        );
    }

    #[test]
    fn ordered_task_item_still_gets_a_computed_number() {
        // GFM allows ordered task items ("1. [ ] x"); the marker node is
        // still emitted (task: true) and still carries ordered info.
        let nodes = parse("1. [ ] a\n2. [x] b\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers.len(), 2);
        for m in &markers {
            assert!(matches!(m.kind, NodeKind::ListMarker { task: true, .. }));
        }
        assert_eq!(
            markers.iter().map(|m| ordered_number_delim(m).0).collect::<Vec<_>>(),
            vec![Some(1), Some(2)]
        );
    }

    #[test]
    fn empty_bullet_item_gets_a_synthesized_marker() {
        // pulldown emits NOTHING between Start(Item) and End(Item) for an
        // empty item, so the marker is synthesized from source bytes: same
        // node shape as if content followed (extent = glyphs + the single
        // trailing space, LINE-level reveal extent).
        let nodes = parse("- a\n- \n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[1].extent, 4..6, "\"- \" incl. the trailing space");
        assert_eq!(markers[1].reveal_extent, Some(4..6), "the item's whole (empty) line");
        assert_eq!(markers[1].kind, NodeKind::ListMarker { task: false, depth: 1, number: None, delim: None });
    }

    #[test]
    fn bare_dash_empty_item_marker_is_just_the_glyph() {
        // A bare "-" with no trailing space is still an empty item per
        // CommonMark; its synthesized marker is glyph-only.
        let nodes = parse("- a\n-\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[1].extent, 4..5);
    }

    #[test]
    fn empty_ordered_item_keeps_its_sequence_slot() {
        // The empty middle item consumed its list_seq slot at Start(Item):
        // displayed numbering counts it like any sibling (1, 2, 3).
        let nodes = parse("1. a\n2. \n3. c\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers.len(), 3);
        assert_eq!(markers[1].extent, 5..8, "\"2. \" incl. the trailing space");
        assert_eq!(
            markers.iter().map(|m| ordered_number_delim(m)).collect::<Vec<_>>(),
            vec![(Some(1), Some(b'.')), (Some(2), Some(b'.')), (Some(3), Some(b'.'))]
        );
    }

    #[test]
    fn empty_nested_item_gets_marker_and_indent_nodes() {
        // The continue-created shape ("- a" > "  - b" > Enter): the empty
        // nested item still emits BOTH its ListItemIndent (hanging-indent
        // line decoration + concealed spaces) and its marker.
        let doc = "- a\n  - b\n  - \n";
        let nodes = parse(doc);
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers.len(), 3);
        let empty_marker = markers[2];
        assert_eq!(empty_marker.extent, 12..14);
        assert!(matches!(empty_marker.kind, NodeKind::ListMarker { task: false, depth: 2, .. }));
        assert_eq!(empty_marker.reveal_extent, Some(10..14), "whole line incl. the indent");
        let indents: Vec<_> = nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::ListItemIndent { .. }))
            .collect();
        assert!(
            indents.iter().any(|n| n.extent == (10..12)),
            "the empty nested item's leading indent is its own node"
        );
    }

    #[test]
    fn empty_item_inside_a_blockquote_gets_a_marker() {
        let nodes = parse("> - a\n> - \n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[1].extent, 8..10);
    }

    #[test]
    fn task_list_marker_and_widget() {
        let nodes = parse("- [ ] todo\n- [x] done\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].extent, 0..2);
        let widgets: Vec<_> = nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::TaskWidget { .. }))
            .collect();
        assert_eq!(widgets.len(), 2);
        assert_eq!(widgets[0].extent, 2..5);
        assert_eq!(widgets[0].kind, NodeKind::TaskWidget { checked: false });
        // LINE-level reveal (contract v0.3): the whole item line.
        assert_eq!(widgets[0].reveal_extent, Some(0..10));
        assert_eq!(widgets[1].kind, NodeKind::TaskWidget { checked: true });
    }

    #[test]
    fn blockquote_single_line_depth() {
        let nodes = parse("> quote\n");
        let lines: Vec<_> = nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::BlockQuoteLine(_)))
            .collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, NodeKind::BlockQuoteLine(1));
        assert_eq!(lines[0].extent, 0..7);
        assert_eq!(lines[0].delims, vec![0..2]);
    }

    #[test]
    fn blockquote_nested_depth_per_line() {
        let nodes = parse("> outer\n> > inner\n");
        let lines: Vec<_> = nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::BlockQuoteLine(_)))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].kind, NodeKind::BlockQuoteLine(1));
        assert_eq!(lines[1].kind, NodeKind::BlockQuoteLine(2));
        assert_eq!(lines[1].delims, vec![8..10, 10..12]);
    }

    #[test]
    fn blockquote_lazy_continuation_line_has_no_markers() {
        // Line 3 has one literal '>' but continues the depth-2 paragraph
        // (CommonMark lazy continuation): depth reflects nesting, but the
        // conceal set only covers the marker bytes actually present.
        let nodes = parse("> outer\n> > inner\n> outer again\n");
        let lines: Vec<_> = nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::BlockQuoteLine(_)))
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2].kind, NodeKind::BlockQuoteLine(2));
        assert_eq!(lines[2].delims, vec![18..20]);
    }

    #[test]
    fn fenced_code_block_lines() {
        let nodes = parse("```rust\nfn main() {}\n```\n");
        let fences: Vec<_> = nodes.iter().filter(|n| n.kind == NodeKind::CodeFenceLine).collect();
        let body: Vec<_> = nodes.iter().filter(|n| n.kind == NodeKind::CodeBlockLine).collect();
        assert_eq!(fences.len(), 2);
        assert_eq!(fences[0].extent, 0..7); // "```rust"
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].extent, 8..20); // "fn main() {}"
        assert_eq!(fences[1].extent, 21..24); // "```"
    }

    #[test]
    fn fenced_code_block_multi_line_body() {
        let nodes = parse("```\nplain\ntext\n```\n");
        let body: Vec<_> = nodes.iter().filter(|n| n.kind == NodeKind::CodeBlockLine).collect();
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].extent, 4..9);
        assert_eq!(body[1].extent, 10..14);
    }

    #[test]
    fn code_span_containing_tilde_is_not_strikethrough() {
        let nodes = parse("`~~not strike~~`");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, NodeKind::Code);
    }

    #[test]
    fn list_inside_blockquote_gets_both_marker_and_line() {
        let nodes = parse("> - item one\n> - item two\n");
        let markers: Vec<_> = nodes.iter().filter(|n| matches!(n.kind, NodeKind::ListMarker { .. })).collect();
        let lines: Vec<_> = nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::BlockQuoteLine(_)))
            .collect();
        assert_eq!(markers.len(), 2);
        assert_eq!(lines.len(), 2);
        assert_eq!(markers[0].extent, 2..4);
    }

    #[test]
    fn tables_and_footnotes_parse_but_emit_no_m1_nodes() {
        // GFM options are enabled so these constructs don't corrupt
        // surrounding parsing, but M1 emits no decorations for them.
        let nodes = parse("| a | b |\n| - | - |\n| 1 | 2 |\n");
        assert!(nodes.is_empty());
        let nodes = parse("text[^1]\n\n[^1]: note\n");
        assert!(nodes.is_empty());
    }
}
