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
//! * **Whitespace trimming (all kinds but code)**: a NON-EMPTY range first
//!   trims `from` forward and `to` backward over the pinned whitespace set
//!   ([`is_toggle_ws`]) — a delimiter placed against a whitespace edge
//!   violates CommonMark's flanking rules (`"a b"` + toggleStrong 0..2
//!   emitted `**a **b`, which parses as no strong at all, and a re-toggle
//!   over the returned selection stacked `****a ****b`). Every rule below
//!   (the block-boundary guard included) then operates on the trimmed
//!   range. A range that trims to nothing (whitespace-only selection) means
//!   the toggle doesn't apply: `Ok(None)`, no mutation, no burned revision.
//!   Cursor ranges (`from == to`) are untouched — the empty-pair insertion
//!   below is flanking-safe, since the pair only parses once content is
//!   typed between the delimiters. Code spans have no flanking rules, so
//!   toggleCode keeps its exact range.
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
//!   accepted). A second empty-range toggle between the delimiters nests
//!   another pair (`a****b` → `a********b`): the empty pair parses as no
//!   node, so there is nothing to strip — also accepted (pinned by test).
//! * **Code contexts don't toggle**: a range touching a fenced-code line,
//!   or (for the non-code toggles) a range endpoint strictly inside an
//!   inline code span, returns `Ok(None)` — delimiters there would be
//!   literal bytes, and a re-toggle would stack them. `Ok(None)` (no
//!   mutation, no burned revision) rather than the multi-block
//!   `InvalidArgument`, matching `set_heading`'s code-context check — the
//!   nearest analog: the target is a well-formed range that the command
//!   simply doesn't apply to, not a malformed argument.
//! * The code ON path space-pads its delimiters when the final content is
//!   backtick-edged OR space-edged (but not all spaces) — the exact shapes
//!   CommonMark unpads at render time. Stripping a code span sheds one such
//!   padding pair iff the content has both a leading and a trailing space
//!   and is not all spaces (the same render-time unpadding condition, and a
//!   byte-exact inverse of the ON path's rule), so spans round-trip
//!   byte-identically (`` `edge `` ⇄ ``` `` `edge `` ```, `" x "` ⇄
//!   `` `· x ·` ``).
//!
//! ## setHeading
//!
//! Operates on the LINE containing `pos`. Applies only when the line
//! belongs to a Paragraph, ATX Heading, or BlockQuote top-level block —
//! `None` on code blocks/fences, lists, tables, HTML blocks, thematic
//! breaks, blank lines, and setext headings (whose "delimiter" is the
//! following underline, not a leading-hash run this command rewrites).
//! Inside a blockquote the hashes go after the line's `> ` markers — and the
//! SAME block gate applies to what sits after those markers: a quote-nested
//! list item or thematic break (constructs the overlay records per line) is
//! refused exactly like its top-level counterpart (`"> - item"` → `None`;
//! the BlockKind gate alone only sees the top-level BlockQuote and would
//! have written hashes into the item's line). Level 0 removes an existing
//! heading's delimiter spans — ALL of them, an ATX closing hash run
//! included (`"# foo #"` → `"foo"`); `None` if there is no heading.
//! **Same-level press toggles back to a paragraph (v0.5 amendment):**
//! `setHeading(pos, N)` where the line is ALREADY exactly level `N`
//! (compared via the parsed `Heading` node's own level, not a byte-identical
//! prefix match — an irregular `"##  x"` at level 2 still counts as "already
//! level 2") removes the heading exactly like `level 0` does. This makes the
//! toolbar's H1–H6 buttons idempotent presses (Obsidian parity: pressing the
//! same heading level again clears it) instead of silently no-op'ing. A
//! DIFFERENT level still replaces the opening delimiter as before; level 0
//! is unchanged (always clears, regardless of current level).
//!
//! ## toggleTask
//!
//! `pos` anywhere in an EXISTING task item (the parser records each task
//! item's full extent) flips exactly one byte: the `[ ]`/`[x]` checkbox
//! interior. `[X]` (capital) also toggles off to `[ ]`.
//!
//! **Promotion (v0.5 amendment — Obsidian parity, research/07 §1.6):**
//! when `pos` does not resolve inside an existing task item, the command
//! PROMOTES the line containing `pos` into a task instead of refusing,
//! matching Obsidian's "Toggle checkbox status" behavior of converting a
//! plain bullet into a checkbox rather than no-op'ing:
//!
//! * **Non-task list item** (bullet or ordered, any nesting depth, any
//!   quote depth — found via the shared `parser::scan_marker` lexer, the
//!   same one `marker_token_width`/`line_marker` use): the `"[ ] "` run is
//!   inserted right after the marker token — after the required single
//!   space when one is present (the ordinary case, since CommonMark
//!   requires content to be separated from the marker by whitespace), or
//!   with its own leading space when the marker has none (an empty item,
//!   `"-"` with nothing after it) so the result is still valid GFM task
//!   syntax (`"- [ ] "`, ready for content) rather than the unrecognized
//!   `"-[ ] "`. Resolved per LINE (the line carrying its OWN marker), not
//!   the whole possibly-multi-line item — a cursor on a plain list item's
//!   CONTINUATION line does not promote (same v1 scope limit as
//!   `indentList`/`outdentList`/`enter`'s own line-oriented model; the
//!   flip path above already handles "anywhere in the item" for the
//!   EXISTING-task case, which is the contract's stated guarantee).
//! * **Plain paragraph/blockquote-content line** (the LINE's block, seen
//!   through any quote prefix exactly like `setHeading`'s gate, is a
//!   Paragraph or BlockQuote, and the line isn't blank): `"- [ ] "` is
//!   inserted at the content start — right after the quote prefix, which is
//!   empty at top level and preserved verbatim when quoted (`"> text"` →
//!   `"> - [ ] text"`).
//! * **Blank line** (including a quote line with empty content, e.g. `">"`):
//!   also gets `"- [ ] "` inserted after any quote prefix. This is MORE
//!   permissive than `setHeading`'s own blank-line refusal — a deliberate
//!   difference: Obsidian's checkbox toggle promotes a blank line too, and
//!   a toolbar button that sometimes silently does nothing on an empty line
//!   is a worse experience than a predictable empty task item.
//! * **Still `None`** on headings, fenced/indented code lines, thematic
//!   breaks, and any other block kind a plain checkbox makes no sense on
//!   (tables, HTML blocks, footnote definitions) — the same conservative
//!   set `setHeading` refuses, since a checkbox on a heading or inside a
//!   fence is exactly as nonsensical as a hash run would be there.
//!
//! Selection after a promotion maps the ORIGINAL `pos` forward through the
//! inserted text ([`mapping::Bias::After`]): the insertion always lands
//! at/before `pos`, so the character immediately after the cursor is
//! unchanged — the cursor stays glued to its content, now shifted right by
//! the inserted marker/checkbox bytes, rather than ending up stranded
//! mid-syntax between the new marker and the new checkbox.
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
//!
//! ## enter
//!
//! Construct-aware Enter (contract v0.3 addition, research/07 §1.3/§1.4/§2.1):
//! continues a list marker or quote prefix, or exits an EMPTY one in a
//! SINGLE press — Obsidian needs an awkward double-Enter for the latter; we
//! don't. Every rule below reads constructs from the parsed overlay (never a
//! line regex) — the same discipline `indentList`/`outdentList` uses, and
//! the reason research/07 §2.3 gives for why Obsidian's own Tab/Enter have
//! quote+list interaction bugs it doesn't.
//!
//! Let `L` = the line containing `from` (after `from`/`to` are normalized to
//! `from <= to`, matching every other range command). Vocabulary reused
//! from `indentList`/`outdentList`: quote prefix, list marker, marker
//! column, marker token width. **Content start** = marker column's content
//! column (marker token width past the marker glyph), EXCEPT for a task
//! item, where it is past the `- [ ] ` run — found via the `TaskWidget`
//! node's own extent (`widget.extent.end + 1`, the required space after
//! `]`) rather than a fixed-width guess, so any tolerated extra pre-checkbox
//! whitespace is still handled correctly.
//!
//! 1. **Not applicable → `None`**: `L` has neither a list marker nor a quote
//!    prefix, OR `from` sits inside `L`'s prefix region (before content
//!    start for a list item; before the quote prefix's end for a quote-only
//!    line). **v1 punt**: both cases fall back to the view's default Enter
//!    (a plain newline) rather than doing anything construct-aware.
//! 2. **Continue** (list item, content after the marker is non-empty):
//!    replace `[from, to]` with `"\n"` + `L`'s quote prefix + `L`'s leading
//!    indent (the raw bytes between the quote prefix and the marker glyph,
//!    copied verbatim) + the next marker: same bullet glyph; ordered raw
//!    source digits + 1 with the same delimiter (`"9. "` → `"10. "`, digit
//!    width grows naturally — no zero-padding); task items append `"[ ] "`
//!    (new items always start unchecked). Text after `to` on `L` becomes the
//!    new item's content — a mid-line Enter splits the item, no special
//!    casing needed. Selection collapses to the end of the inserted prefix.
//! 3. **Exit/outdent** (list item, content EMPTY — nothing or only
//!    whitespace from content start to `L`'s end, and `from` is at/after
//!    content start): NO `"\n"` is ever inserted in this branch — one Enter
//!    press is one level of escape, matching §1.4's "ship the better
//!    mechanic" recommendation.
//!    - Marker column `> 0` (nested, incl. nested-in-quote): outdent this
//!      ONE line by the same target-scan/delta arithmetic as
//!      `plan_list_nesting`'s outdent path, INCLUDING both structural
//!      rewrite guards (`interruption_rewrite`/`below_line_rewrite`) — the
//!      whole-document itemness invariant must hold exactly as it does for
//!      `outdentList`. No subtree walk: an empty item is accepted to carry
//!      none (v1 simplification — a degenerate empty item with block
//!      children below it is out of scope). If the target scan finds no
//!      qualifying parent above (the same v1 "doesn't look past a blank
//!      line" limitation `outdentList` has — vanishingly rare for a
//!      genuinely nested marker), falls through to the top-level branch
//!      instead of leaving the press inert.
//!    - Marker column `0` (top-level): delete the marker token
//!      (dash/digits+delimiter+space, PLUS the task brackets+space for a
//!      task item) from `L`, leaving any quote prefix intact — `L` becomes
//!      an (empty) paragraph/quote line. No guard needed here: deleting a
//!      line's entire marker run always leaves that line blank, which
//!      naturally inserts the blank-line separator CommonMark needs before
//!      any non-1 ordered sibling below can safely start/continue a list —
//!      the interruption hazard `outdentList`'s guards exist for cannot
//!      arise from this branch.
//! 4. **Quote continue** (quote prefix, no marker, non-empty content after
//!    the prefix): replace `[from, to]` with `"\n"` + `L`'s exact quote
//!    prefix bytes.
//! 5. **Quote exit** (quote-only line, content after the prefix EMPTY):
//!    drop the LAST `"> "` run element only — one level per press
//!    (`"> > "` → `"> "` on the first press, `"> "` → plain on the second),
//!    never all levels at once (the single-press philosophy applies per
//!    level, not per line).
//! 6. **Mixed** (list inside a quote): the innermost construct governs,
//!    piecewise, matching the contract's construct-aware discipline
//!    elsewhere — rules 2/3 keep `L`'s quote prefix intact in the
//!    continuation/outdent; an empty TOP-LEVEL item inside a quote clears
//!    just the marker (rule 3's top-level branch), leaving `"> "` for rule 5
//!    to strip on a later press.
//! 7. **Selection** (`from != to`): context is resolved from the pre-edit
//!    parse at `from` only. Rules 2/4 fold the delete into the same splice
//!    as the continuation insert. Rules 3/5 (no insert) instead append a
//!    separate delete-`[from, to]` splice after the marker/prefix edit
//!    (ascending: `from` is always at/after content start, strictly past
//!    where the marker/prefix edit lands) — one batch, one undo unit either
//!    way. When `to` extends past `L`'s own end (a selection spanning into
//!    further lines), rule 3's below-line guard still runs: it resumes its
//!    downward scan from past the line CONTAINING `to` (accounting for the
//!    whole region the selection consumes, not just `L`), using post-edit
//!    columns exactly as the collapsed-cursor case does, and lands on the
//!    same below-context line a collapsed cursor reaching the same resulting
//!    shape would — see `outdent_single_line`. Lines the selection deletes
//!    outright lose itemness as a direct, explicit consequence of the user's
//!    own selection (not a silent side effect the guard exists to prevent),
//!    same rationale as rule 3's own line.
//! 8. Unlike `indentList`/`outdentList`, the applies-but-no-op distinction
//!    never arises here: every applicable case (2 through 6) produces a real
//!    splice batch. `enter` returns either `None` (rule 1) or `Some` with a
//!    non-empty batch — never `Some` with an empty one.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::block_index::BlockKind;
use crate::error::CoreError;
use crate::mapping::{self, Bias};
use crate::parser::{self, Node, NodeKind};
use crate::text::{ByteSplice, SrcBytes};

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
    /// Construct-aware Enter (boundary v0.3): continue a list marker/quote
    /// prefix, or exit an empty one in one press. UTF-16 range; the editor
    /// resolves to bytes and normalizes `from <= to`. `None` when neither
    /// construct applies at the target (the view falls back to a plain
    /// newline) — see the module doc comment's "## enter" section.
    Enter { from: usize, to: usize },
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

/// Whether `[from_b, to_b]` stays within ONE leaf block's inline content.
/// Inline delimiters cannot cross a leaf-block boundary — `**a\n\nb**` does
/// not parse as strong, and a re-toggle would then stack delimiters — so
/// `toggle_inline` refuses multi-block selections. Softbreaks WITHIN one
/// paragraph are fine (`**a\nb**` parses), so the walk is per physical
/// line: a blank line, a line carrying a line-terminated construct
/// (heading, thematic break, fence/code line), a fresh list-item marker on
/// a later line, or a blockquote-depth change all mark a boundary.
/// Detection covers the constructs the overlay knows; the overlay lookups
/// binary-search per line like `quote_context`/`line_marker`.
fn single_leaf_block(nodes: &[Node], src: &SrcBytes, from_b: usize, to_b: usize) -> bool {
    let line_terminated = |k: &NodeKind| {
        matches!(
            k,
            NodeKind::Heading(_)
                | NodeKind::ThematicBreak
                | NodeKind::CodeFenceLine
                | NodeKind::CodeBlockLine
        )
    };
    let line_has = |line: &Range<usize>, pred: &dyn Fn(&NodeKind) -> bool| {
        let lo = nodes.partition_point(|n| n.extent.start < line.start);
        let hi = nodes.partition_point(|n| n.extent.start < line.end);
        nodes[lo..hi].iter().any(|n| pred(&n.kind))
    };
    let first = line_containing(src, from_b);
    if to_b <= first.end {
        return true; // a single line is at most one leaf block's inlines
    }
    if is_blank(src, first.clone()) || line_has(&first, &line_terminated) {
        return false;
    }
    let (depth, _) = quote_context(nodes, first.start);
    let mut line = first;
    while to_b > line.end {
        let Some(next) = next_line(src, line.end) else {
            return false; // defensive: to_b past the last terminator
        };
        if is_blank(src, next.clone())
            || line_has(&next, &|k| {
                line_terminated(k) || matches!(k, NodeKind::ListMarker { .. })
            })
            || quote_context(nodes, next.start).0 != depth
        {
            return false;
        }
        line = next;
    }
    true
}

/// The inline-toggle whitespace set (contract v0.3 consolidation, pinned
/// byte-for-byte against the JS mock's implementation of the same rule):
/// U+0009, U+000A, U+000C, U+000D, U+0020, U+00A0, U+1680, U+2000–U+200A,
/// U+2028, U+2029, U+202F, U+205F, U+3000. Deliberately NOT Rust's
/// `char::is_whitespace()` (which also matches U+0085) and not JS `\s`
/// (which also matches U+FEFF) — the two disagree exactly there, and the
/// cross-core conformance suite pins this explicit set on both sides.
fn is_toggle_ws(cp: u32) -> bool {
    matches!(
        cp,
        0x09 | 0x0A
            | 0x0C
            | 0x0D
            | 0x20
            | 0xA0
            | 0x1680
            | 0x2000..=0x200A
            | 0x2028
            | 0x2029
            | 0x202F
            | 0x205F
            | 0x3000
    )
}

/// Decode the UTF-8 scalar starting at `i`: `(code point, byte length)`, or
/// `None` past the document's end. `i` must sit on a char boundary — every
/// toggle endpoint is a validated UTF-16 conversion, and trimming only ever
/// steps by whole decoded scalars.
fn scalar_at(src: &SrcBytes, i: usize) -> Option<(u32, usize)> {
    let b0 = src.get(i)?;
    match b0 {
        0x00..=0x7F => Some((u32::from(b0), 1)),
        0xC0..=0xDF => Some((
            (u32::from(b0 & 0x1F) << 6) | u32::from(src.get(i + 1)? & 0x3F),
            2,
        )),
        0xE0..=0xEF => Some((
            (u32::from(b0 & 0x0F) << 12)
                | (u32::from(src.get(i + 1)? & 0x3F) << 6)
                | u32::from(src.get(i + 2)? & 0x3F),
            3,
        )),
        // 4-byte lead: an astral scalar (>= U+10000). Never in the WS set,
        // and the exact value is irrelevant here — only that trimming stops
        // at it — so a sentinel above the BMP suffices. (Continuation bytes
        // can't reach this arm: `i` is a char boundary.)
        _ => Some((0x10000, 4)),
    }
}

/// Trim `[from_b, to_b]` inward over [`is_toggle_ws`] — the flanking-safety
/// rule of the module doc's "Toggle semantics". Both endpoints sit on char
/// boundaries; the backward step scans to the previous lead byte (at most 3
/// continuation bytes) and decodes forward from there.
fn trim_flanking_ws(src: &SrcBytes, mut from_b: usize, mut to_b: usize) -> (usize, usize) {
    while from_b < to_b {
        let Some((cp, len)) = scalar_at(src, from_b) else {
            break;
        };
        if !is_toggle_ws(cp) {
            break;
        }
        from_b += len;
    }
    while to_b > from_b {
        let mut lead = to_b - 1;
        while lead > from_b && src.byte(lead) & 0xC0 == 0x80 {
            lead -= 1;
        }
        match scalar_at(src, lead) {
            // `lead + len == to_b` re-checks the decode's consistency with
            // the boundary invariant; a mismatch stops rather than corrupts.
            Some((cp, len)) if is_toggle_ws(cp) && lead + len == to_b => to_b = lead,
            _ => break,
        }
    }
    (from_b, to_b)
}

pub fn toggle_inline(
    nodes: &[Node],
    src: &SrcBytes,
    kind: InlineKind,
    from_b: usize,
    to_b: usize,
) -> Result<Option<CommandPlan>, CoreError> {
    // Flanking-safety trim (module doc, "Toggle semantics"): trim BEFORE
    // any planning or guard, so OFF/EXTEND detection and the block guard
    // all see the trimmed range; a whitespace-only selection trims to
    // nothing and the toggle simply doesn't apply. Cursor ranges pass
    // through untouched, and code spans keep their exact range (no
    // flanking rules for code).
    let (from_b, to_b) = if kind == InlineKind::Code || from_b == to_b {
        (from_b, to_b)
    } else {
        let (f, t) = trim_flanking_ws(src, from_b, to_b);
        if f == t {
            return Ok(None); // whitespace-only selection: doesn't apply
        }
        (f, t)
    };
    // Block-boundary guard (see `single_leaf_block`): a selection spanning
    // more than one leaf block errors instead of planning — the wrapped
    // text could never parse as one inline node, and a re-toggle would
    // stack delimiters. A thrown command is a consumed no-op for the view
    // (contract: `command()` throws WITHOUT mutating; views must not
    // resync), so nothing is lost relative to silently doing nothing, and
    // the caller can tell "refused" from "didn't apply" (`Ok(None)`).
    if !single_leaf_block(nodes, src, from_b, to_b) {
        return Err(CoreError::InvalidArgument {
            detail: "inline toggle range spans more than one leaf block".into(),
        });
    }
    // Code-context guard (module doc, "Toggle semantics"): delimiters
    // written onto a fenced-code line are literal bytes, never formatting
    // (`**code**` inside a fence stays raw, and a re-toggle over the
    // returned selection stacks `****code****`). `single_leaf_block`'s own
    // fence detection never runs for a single-line range (it short-circuits
    // true), so this must be checked independently. Like `set_heading`'s
    // code check — the nearest analog — this is "doesn't apply" (`Ok(None)`,
    // no mutation, no burned revision), not the multi-block error above.
    if nodes.iter().any(|n| {
        matches!(n.kind, NodeKind::CodeFenceLine | NodeKind::CodeBlockLine)
            && n.extent.start <= to_b
            && from_b <= n.extent.end
    }) {
        return Ok(None);
    }
    // OFF: innermost same-kind node whose closed extent contains the range
    // (rfind on the document-ordered overlay = last-starting = innermost).
    let containing = nodes.iter().rfind(|n| {
        node_is(kind, n)
            && n.delims.len() == 2
            && n.extent.start <= from_b
            && to_b <= n.extent.end
    });
    if let Some(node) = containing {
        let (open_span, close_span) = strip_spans(kind, src, node);
        let open_len = open_span.end - open_span.start;
        return Ok(Some(CommandPlan {
            batch: vec![del(&open_span), del(&close_span)],
            // Post-apply, the surviving content starts where the open span
            // did and ends one open-span-length left of the close span.
            selection: Some((open_span.start, close_span.start - open_len)),
        }));
    }

    // Inline-code half of the code-context guard, for the non-code toggles
    // only (a code toggle's own interaction with Code nodes is the
    // strip/extend logic around this): a range endpoint STRICTLY inside a
    // code span's extent would drop a delimiter into the span's literal
    // content (`` `code` `` + toggleStrong 2..4 → `` `c**od**e` ``, and a
    // re-toggle stacks). Endpoints AT the extent's boundaries are fine —
    // ``**`code`**`` parses as strong containing code. Checked AFTER the
    // OFF branch above: unwrapping a containing same-kind node (e.g.
    // toggleStrong inside the code span of ``**a `b` c**``) only deletes
    // delimiters OUTSIDE the code span and stays safe.
    if kind != InlineKind::Code {
        let inside_code = |p: usize| {
            nodes.iter().any(|n| {
                matches!(n.kind, NodeKind::Code) && n.extent.start < p && p < n.extent.end
            })
        };
        if inside_code(from_b) || inside_code(to_b) {
            return Ok(None);
        }
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

    // Same-KIND nodes CAN nest via different delimiter flavors (`_a *b* c_`
    // is Emphasis inside Emphasis; `__a **b** c__` Strong inside Strong), so
    // document order over `touched` is NOT position order over their
    // delimiter spans — the outer node's closing delimiter sits after both
    // of the inner node's. The apply path requires an ascending batch
    // (counterexample pre-sort: `_a *b* c_ x` + toggleEm 0..11 emitted
    // deletes at 0, 8, 3, 5 and silently corrupted both the text and its
    // undo inverse), so collect every delimiter span and sort by position.
    let mut delim_spans: Vec<&Range<usize>> =
        touched.iter().flat_map(|n| n.delims.iter()).collect();
    delim_spans.sort_unstable_by_key(|d| d.start);
    let mut batch = Vec::with_capacity(2 + delim_spans.len());
    batch.push(ins(t_start, open));
    let mut deleted = 0usize;
    for d in &delim_spans {
        deleted += d.end - d.start;
        batch.push(del(d));
    }
    batch.push(ins(t_end, close));
    // Delimiter spans are disjoint byte ranges; sorted by position, with the
    // open insert at t_start ≤ every delim start and the close insert at
    // t_end ≥ every delim end, the batch is ascending and non-overlapping.
    Ok(Some(CommandPlan {
        batch,
        selection: Some((t_start + open_len, t_end - deleted + open_len)),
    }))
}

/// Canonical delimiter pair for an ON/EXTEND toggle. For code, the run is
/// one backtick longer than the longest backtick run remaining in the final
/// content, space-padded when that content starts or ends with a backtick
/// OR a space (but is not all spaces). The backtick case keeps the edge
/// backtick out of the delimiter run; the space case is CommonMark's own
/// requirement — `` ` x ` `` renders as `x` (the renderer sheds one pad
/// pair), so edge spaces survive rendering only under a pad pair. All-space
/// content is exempt because CommonMark never unpads it (`` ` ` `` renders
/// as a literal space), so a pad there would become content. Padding
/// whenever the content is space-edged also keeps [`strip_spans`]' shed
/// rule a byte-exact inverse — counterexample without it: `" x "` → ON →
/// `` `· x ·` `` had no pad to shed, yet OFF shed the user's own edge
/// spaces, returning `"x"` (data loss).
fn delimiters(
    kind: InlineKind,
    src: &SrcBytes,
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
                src.push_slice_to(&mut content, pos..d.start);
                pos = d.end;
            }
            src.push_slice_to(&mut content, pos..t_end);
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
            let space_edged = (content.starts_with(' ') || content.ends_with(' '))
                && !content.bytes().all(|b| b == b' ');
            if content.starts_with('`') || content.ends_with('`') || space_edged {
                (format!("{ticks} "), format!(" {ticks}"))
            } else {
                (ticks.clone(), ticks)
            }
        }
    }
}

/// The two delete spans for an OFF (strip) toggle — [`delimiters`]' inverse.
/// For every kind but code these are exactly the node's delimiter spans. A
/// code span additionally sheds its CommonMark padding pair — one space on
/// each side — iff the raw content has BOTH a leading and a trailing space
/// and is not all spaces (the exact condition under which CommonMark strips
/// a pad pair at render time, and the shape [`delimiters`]' ON path emits
/// for edge-backtick and space-edged content). Counterexample without this: `` `edge `` →
/// ON → ``` `` `edge `` ``` → OFF left `· `edge ·` (two stray pad spaces)
/// instead of round-tripping byte-identically. `` ` ` `` (all-space content)
/// keeps its space: CommonMark never unpads it, so neither do we.
fn strip_spans(kind: InlineKind, src: &SrcBytes, node: &Node) -> (Range<usize>, Range<usize>) {
    let d0 = node.delims[0].clone();
    let d1 = node.delims[1].clone();
    let content = &node.content;
    let padded = kind == InlineKind::Code
        && content.end - content.start >= 2
        && src.get(content.start) == Some(b' ')
        && src.get(content.end - 1) == Some(b' ')
        && !(content.start..content.end).all(|i| src.get(i) == Some(b' '));
    if padded {
        (d0.start..d0.end + 1, d1.start - 1..d1.end)
    } else {
        (d0, d1)
    }
}

/// The ATX level (1..=6) of an overlay `Heading` node, or `None` for any
/// other node kind — a small typed accessor so `set_heading`'s same-level
/// toggle-back check (v0.5) reads the parsed level rather than re-deriving
/// it from delimiter bytes.
fn existing_level(node: &Node) -> Option<u8> {
    match node.kind {
        NodeKind::Heading(level) => Some(level),
        _ => None,
    }
}

pub fn set_heading(
    nodes: &[Node],
    src: &SrcBytes,
    block_kind: Option<BlockKind>,
    line: Range<usize>,
    pos_b: usize,
    level: u8,
) -> Option<CommandPlan> {
    // Defensive: never rewrite lines the overlay knows are code.
    if nodes.iter().any(|n| {
        matches!(n.kind, NodeKind::CodeFenceLine | NodeKind::CodeBlockLine)
            && n.extent.start <= line.start
            && line.start <= n.extent.end
    }) {
        return None;
    }
    // Blockquote-nested block gate (module doc, "setHeading"): the BlockKind
    // gate below only sees the TOP-LEVEL block, so a list item or thematic
    // break nested inside a quote would slip through it and get hashes
    // written into its line (`"> - item"` + level 1 produced `"> # - item"`,
    // a heading swallowing the marker). Refused the same way as their
    // top-level counterparts: `None`, no burned revision. The overlay lookup
    // binary-searches this line's node window, like `quote_context`.
    {
        let lo = nodes.partition_point(|n| n.extent.start < line.start);
        let hi = nodes.partition_point(|n| n.extent.start < line.end);
        if nodes[lo..hi]
            .iter()
            .any(|n| matches!(n.kind, NodeKind::ListMarker { .. } | NodeKind::ThematicBreak))
        {
            return None;
        }
    }
    // Inside a blockquote the hashes go after this line's `> ` markers.
    let after_quote = nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::BlockQuoteLine(_)))
        .find(|n| n.extent.start == line.start)
        .and_then(|n| n.delims.last().map(|d| d.end))
        .unwrap_or(line.start);
    // ATX headings tolerate 1-3 spaces of leading indent (CommonMark), so
    // the hash run sits/goes after such a run — counterexample without
    // this: `"  # foo"` + any level returned `None` because the heading's
    // delimiter starts at 2, not at the line start. A 4+-space run is
    // indented code territory (its own line never passes the BlockKind gate
    // above; a run this deep reaching here — e.g. a lazily-continued
    // paragraph line — keeps the old at-the-run's-start behavior).
    let space_run = (after_quote..line.end)
        .take_while(|&i| src.get(i) == Some(b' '))
        .count();
    let insertion = if space_run <= 3 { after_quote + space_run } else { after_quote };

    let existing = nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Heading(_)))
        .find(|n| n.delims.first().is_some_and(|d| d.start == insertion));

    // Applies only on Paragraph/ATX-Heading/BlockQuote lines. The BlockKind
    // gate alone is blind to an ATX heading with legal 1-3 spaces of leading
    // indent: the heading BLOCK's span starts at its first `#`, not the line
    // start, so the caller's line-start block lookup misses (`block_kind`
    // `None`) — counterexample: `"  # foo"` could neither relevel nor be
    // removed. An overlay heading whose delimiter sits exactly at the
    // insertion point IS that case, so `existing` passes the gate too.
    if !matches!(
        block_kind,
        Some(BlockKind::Paragraph) | Some(BlockKind::Heading) | Some(BlockKind::BlockQuote)
    ) && existing.is_none()
    {
        return None;
    }

    let batch = match (existing, level) {
        (None, 0) => return None, // nothing to remove
        (Some(node), n) if n == 0 || Some(n) == existing_level(node) => {
            // Level 0 deletes ALL delimiter spans unconditionally — an ATX
            // closing hash run included (`"# foo #"` → `"foo"`, not
            // `"foo #"`). A press at the line's CURRENT level (v0.5
            // amendment) toggles back to a paragraph the SAME way: an
            // idempotent H2 press clears the heading instead of no-op'ing,
            // matching the toolbar's Obsidian-parity expectation. Compared
            // via the parsed `Heading` node's own level, not a
            // byte-identical prefix match — an irregular `"##  x"` (extra
            // inner space) still counts as "already level 2". Delimiter
            // spans are position-ordered (opening run first), so the batch
            // is ascending as the apply path requires.
            node.delims.iter().map(del).collect()
        }
        (Some(node), n) => {
            let d = node.delims[0].clone();
            let prefix = format!("{} ", "#".repeat(n as usize));
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
            // Blank line: nothing to promote. Measured from `insertion`,
            // not the line start, so a quote line with EMPTY content (`">"`,
            // `"> "`) counts as blank exactly like a blank paragraph line —
            // counterexample without this: `">"` + level 1 produced `"># "`,
            // an empty heading inside the empty quote line. (Space-only
            // remainders also land here via the 1-3-space skip above.)
            if insertion >= line.end {
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

pub fn toggle_task(
    nodes: &[Node],
    src: &SrcBytes,
    block_kind: Option<BlockKind>,
    doc_len: usize,
    pos_b: usize,
) -> Option<CommandPlan> {
    // Flip: `pos` anywhere inside an EXISTING task item's full extent
    // (multi-line items included — the contract's "pos anywhere in the list
    // item" guarantee is pinned to this path).
    if let Some(widget) = nodes.iter().rfind(|n| {
        matches!(n.kind, NodeKind::TaskWidget { .. })
            && n.item_extent.as_ref().is_some_and(|item| {
                item.start <= pos_b
                    && (pos_b < item.end || (pos_b == item.end && item.end == doc_len))
            })
    }) {
        let checked = matches!(widget.kind, NodeKind::TaskWidget { checked: true });
        return Some(CommandPlan {
            batch: vec![ByteSplice {
                at: widget.extent.start + 1,
                delete: 1,
                insert: if checked { " " } else { "x" }.into(),
            }],
            selection: None, // 1-for-1 byte swap: the view's cursor is unaffected
        });
    }

    // Promote (v0.5 amendment, see the module doc comment's "## toggleTask"
    // section for the full rationale): `pos` didn't resolve inside any
    // existing task item above, so try to turn the LINE containing it into
    // one instead of refusing.
    let line = line_containing(src, pos_b);

    // A non-task list item (bullet or ordered, any depth, any quote depth):
    // insert `"[ ] "` right after the marker token. `line_marker` locates
    // the marker's own glyph start exactly like `indentList`/`outdentList`
    // do; `scan_marker` (the crate's one marker lexer) then classifies its
    // trailing whitespace so the checkbox lands after the required space
    // when one exists, or brings its own so the result is still valid GFM
    // task syntax when the item is empty (`"-"` → `"- [ ] "`, not the
    // unrecognized `"-[ ] "`).
    if let Some((item_start, _)) = line_marker(nodes, src, &line) {
        let shape = parser::scan_marker(|i| src.get(i), item_start)?;
        let (at, insert) = if shape.has_trailing_space {
            (shape.glyph_end + 1, "[ ] ".to_string())
        } else {
            (shape.glyph_end, " [ ] ".to_string())
        };
        let batch = vec![ins(at, insert)];
        let cursor = mapping::map_pos(pos_b, &batch, Bias::After);
        return Some(CommandPlan {
            batch,
            selection: Some((cursor, cursor)),
        });
    }

    // Otherwise: a plain paragraph/blockquote-content line, or a blank one
    // (including an empty quote line, `">"`), promotes with a fresh
    // `"- [ ] "` marker — same block-kind gate as `setHeading` (Paragraph or
    // BlockQuote; `None` covers a blockless blank line between blocks), PLUS
    // an explicit blank-line allowance `setHeading` does NOT have: a blank
    // line always promotes (Obsidian does; a toolbar button that sometimes
    // silently no-ops on an empty line is worse than a predictable empty
    // task item). Headings, fenced/indented code, thematic breaks, and any
    // other block kind (tables, HTML blocks, footnote definitions, or a
    // continuation line inside some OTHER list item) fall through to `None`
    // — a checkbox makes no sense there, same as `setHeading`'s hash run.
    let (_, quote_end) = quote_context(nodes, line.start);
    let blank = is_blank(src, quote_end..line.end);
    let applies = matches!(block_kind, Some(BlockKind::Paragraph) | Some(BlockKind::BlockQuote))
        || (blank && block_kind.is_none());
    if !applies {
        return None;
    }
    let batch = vec![ins(quote_end, "- [ ] ".to_string())];
    let cursor = mapping::map_pos(pos_b, &batch, Bias::After);
    Some(CommandPlan {
        batch,
        selection: Some((cursor, cursor)),
    })
}

// ---------------------------------------------------------------------
// indentList / outdentList (boundary v0.2: marker-width-aware Tab nesting).
// See the module doc comment's "## indentList / outdentList" section for
// the full spec — this is a direct transcription of it.
// ---------------------------------------------------------------------

/// One physical source line's list/quote context. Built fresh per line from
/// the parser overlay + raw source bytes — cheap per line (the overlay
/// lookups binary-search the extent-sorted node list, see `quote_context`/
/// `line_marker`), which matters because a multi-line selection or a deep
/// subtree walk can visit thousands of lines, not just a handful.
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
/// terminator — `\n`, `\r\n`, or a lone `\r` — excluded). Delegates to
/// [`SrcBytes::line_range_at`] (backed by `TextBuffer::line_range_at`/ropey's
/// own line metric) rather than hand-scanning: the old hand-rolled backward
/// scan here only stopped at `\n`, so a lone-`\r`-terminated line (which
/// pulldown-cmark itself treats as its own line — verified against this
/// pulldown-cmark version) would merge into whatever preceded it.
fn line_containing(src: &SrcBytes, pos: usize) -> Range<usize> {
    src.line_range_at(pos)
}

/// The physical line immediately preceding `line_start`, or `None` at the
/// start of the document.
fn prev_line(src: &SrcBytes, line_start: usize) -> Option<Range<usize>> {
    if line_start == 0 {
        return None;
    }
    let mut end = line_start;
    if end > 0 && src.byte(end - 1) == b'\n' {
        end -= 1;
    }
    if end > 0 && src.byte(end - 1) == b'\r' {
        end -= 1;
    }
    Some(line_containing(src, end))
}

/// The physical line immediately following the line ending at `line_end`
/// (that line's own extent, terminator excluded), or `None` when `line_end`
/// has no terminator (the document's last line).
fn next_line(src: &SrcBytes, line_end: usize) -> Option<Range<usize>> {
    let mut next = line_end;
    if src.get(next) == Some(b'\r') {
        next += 1;
    }
    if src.get(next) == Some(b'\n') {
        next += 1;
    }
    if next == line_end {
        return None; // no terminator: `line_end` is the document's end
    }
    Some(line_containing(src, next))
}

/// Physical lines intersecting `[from_b, to_b]` (`from_b <= to_b`), mirroring
/// CodeMirror's own multi-line command iteration: an empty range (cursor)
/// always yields its containing line; a non-empty range excludes a trailing
/// line touched only at its very start (`to_b` landing exactly on a line
/// boundary selects none of that line). LAZY — `plan_list_nesting`'s early
/// "doesn't apply"/no-op returns must not pay to materialize every line of a
/// huge selection (a select-all Tab that turns out to be a no-op only ever
/// needs the lines up to the first list-item line).
fn intersecting_lines<'s, 'a>(
    src: &'s SrcBytes<'a>,
    from_b: usize,
    to_b: usize,
) -> impl Iterator<Item = Range<usize>> + use<'s, 'a> {
    let empty = from_b == to_b;
    let mut pos: Option<usize> = Some(from_b);
    std::iter::from_fn(move || {
        let p = pos?;
        let line = line_containing(src, p);
        pos = if p >= to_b {
            None
        } else {
            let mut next = line.end;
            if src.get(next) == Some(b'\r') {
                next += 1;
            }
            if src.get(next) == Some(b'\n') {
                next += 1;
            }
            // `next <= p` is the defensive doc-end stop (no terminator left).
            (next > p).then_some(next)
        };
        if empty || to_b > line.start {
            Some(line)
        } else {
            // A trailing line touched only at its very start is excluded —
            // and it is necessarily the LAST candidate (to_b <= line.start
            // <= p can only coexist with the `p >= to_b` stop above), so
            // ending the iteration here matches the eager original exactly.
            None
        }
    })
}

/// This line's blockquote depth (0 outside any blockquote) and the byte
/// offset just past its `> `/`> > `/… marker run, from the parser's per-line
/// `BlockQuoteLine` node.
///
/// `nodes` (the cached overlay) is sorted by `extent.start` (`parser::
/// parse_document`'s final sort, preserved by every incremental-reparse
/// splice — see `editor.rs`'s `reparse_incremental` step 3a) and no node's
/// extent spans a line boundary here (a `BlockQuoteLine`'s own extent starts
/// exactly at `line_start` when present), so binary search jumps straight to
/// this line's small node cluster instead of a linear scan over the WHOLE
/// overlay — the fix for `indentList`/`outdentList`/`enter` being
/// accidentally O(lines × nodes) (verified: 40ms/103ms for a 5k/10k-item
/// select-all Tab).
fn quote_context(nodes: &[Node], line_start: usize) -> (u8, usize) {
    let lo = nodes.partition_point(|n| n.extent.start < line_start);
    let hi = nodes.partition_point(|n| n.extent.start <= line_start);
    nodes[lo..hi]
        .iter()
        .find_map(|n| match n.kind {
            NodeKind::BlockQuoteLine(depth) => Some((depth, n.delims.last().map_or(line_start, |d| d.end))),
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
fn line_marker(nodes: &[Node], src: &SrcBytes, line: &Range<usize>) -> Option<(usize, usize)> {
    // Same binary-search jump as `quote_context` (see its doc comment): the
    // overlay is sorted by `extent.start`, so this line's node cluster is a
    // contiguous `[lo, hi)` window instead of a full linear scan.
    let lo = nodes.partition_point(|n| n.extent.start < line.start);
    let hi = nodes.partition_point(|n| n.extent.start < line.end);
    let raw_start = nodes[lo..hi].iter().find_map(|n| match n.kind {
        NodeKind::ListMarker { .. } => Some(n.extent.start),
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
    while matches!(src.get(item_start), Some(b' ' | b'\t')) {
        item_start += 1;
    }
    Some((item_start, marker_token_width(src, item_start)))
}

/// Marker glyph run length + 1 (the required following space) — see
/// [`line_marker`]. Built on the shared [`parser::scan_marker`] lexer;
/// preserves this call site's own pre-existing quirks exactly:
/// * the `+ 1` is UNCONDITIONAL (a fixed-width formula per the contract —
///   `indentList`/`outdentList`'s column math and the clamp in `enter`
///   depend on this NOT varying with however much whitespace actually
///   follows the marker in the source), unlike `parser::empty_item_marker_end`,
///   which only adds a byte when a trailing space is actually present;
/// * a digit run with no delimiter byte after it still counts (glyph_end
///   stops at the digit run's end rather than bailing entirely) — this
///   never actually happens in practice since `item_start` only ever comes
///   from a real `ListMarker` node, but preserves the original fallback;
/// * a byte that's neither a bullet nor a digit yields width 1 (`glyph_end
///   == item_start`), matching the original's silent `_ => {}` arm.
fn marker_token_width(src: &SrcBytes, item_start: usize) -> usize {
    let glyph_end = parser::scan_marker(|i| src.get(i), item_start)
        .map_or(item_start, |m| m.glyph_end);
    (glyph_end - item_start) + 1
}

fn list_line_ctx(nodes: &[Node], src: &SrcBytes, line: Range<usize>) -> ListLineCtx {
    let (quote_depth, quote_end) = quote_context(nodes, line.start);
    let marker = line_marker(nodes, src, &line);
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
    src: &SrcBytes,
    from_b: usize,
    to_b: usize,
    indent: bool,
) -> Option<CommandPlan> {
    // Applies iff at least one intersecting line carries a marker. Found via
    // the cheap (O(log nodes), see `line_marker`'s doc comment) marker
    // lookup over the LAZY line iterator — neither the remaining lines nor
    // any line's full `ListLineCtx` (which also computes blockquote depth)
    // is touched until past every early "doesn't apply"/"no-op" return
    // below, so a select-all Tab that turns out to be a no-op never pays
    // for the other 9,999 lines at all.
    let mut line_ranges = intersecting_lines(src, from_b, to_b);
    let first_range = line_ranges
        .by_ref()
        .find(|l| line_marker(nodes, src, l).is_some())?;
    let first = list_line_ctx(nodes, src, first_range.clone());
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
    while let Some(range) = prev_line(src, cursor) {
        let ctx = list_line_ctx(nodes, src, range.clone());
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

    // Only now (past every early "doesn't apply"/no-op return above) walk
    // the REMAINING intersecting lines and build their contexts — the first
    // item line plus everything after it (lines before it carry no marker
    // and the affected-set loop would skip them anyway).
    let lines: Vec<ListLineCtx> = std::iter::once(first_range)
        .chain(line_ranges)
        .map(|l| list_line_ctx(nodes, src, l))
        .collect();

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
        // Already collected by an EARLIER intersecting line's subtree walk?
        // Then this line's own subtree is a subset of that walk's coverage
        // and re-walking it changes nothing: this line sits at a column
        // strictly greater than that earlier root's, so (a) every line this
        // walk would collect (consecutive, same depth, column > this line's
        // > the root's) satisfies the root walk's own collection condition,
        // and (b) any line that would stop THIS walk also stops (or already
        // stopped) the root's. Skipping keeps the union identical while
        // turning a multi-line selection over a strictly-deepening chain
        // from O(lines²) walk visits into O(lines).
        if affected.contains_key(&line.start) {
            continue;
        }
        affected.insert(line.start, *line);
        let mut cursor_end = line.end;
        while let Some(range) = next_line(src, cursor_end) {
            let ctx = list_line_ctx(nodes, src, range.clone());
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
    if let Some(rewrite) = interruption_rewrite(nodes, src, &first, new_col, &affected, delta, indent) {
        batch.insert(1, rewrite);
    }
    // Below-line guard scan starts right after the affected set's own last
    // line (no selection-consumed region to account for here — unlike
    // `enter`'s single-line outdent, indentList/outdentList never delete
    // extra text past the affected lines themselves).
    let below_scan_from = affected.values().next_back().expect("affected is non-empty (batch was)").end;
    if let Some(rewrite) =
        below_line_rewrite(nodes, src, &affected, first_depth, new_col, delta, indent, below_scan_from)
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

/// Parse a raw ordered marker's literal digit run at `item_start`:
/// `(digit_run_len, numeric_value, delimiter_byte)`. `None` for a bullet
/// marker, OR a digit run with no delimiter after it (unlike
/// `marker_token_width`, this call site REQUIRES the delimiter — preserved
/// quirk, matching its pre-refactor behavior exactly). Shared by
/// `ordered_marker` (which only needs the "is this `1`" family check for the
/// paragraph-interruption guard) and `enter`'s CONTINUE rule (which needs the
/// actual literal value, to increment it). Built on the shared
/// [`parser::scan_marker`] lexer.
fn ordered_marker_value(src: &SrcBytes, item_start: usize) -> Option<(usize, u64, u8)> {
    let m = parser::scan_marker(|i| src.get(i), item_start)?;
    let delim = m.delim?;
    let value = m.number?;
    Some((m.glyph_end - item_start - 1, value, delim))
}

/// The ordered-marker shape at `item_start`: `(digit_run_len, value_is_one,
/// delimiter_byte)`. `None` for bullet markers. `value_is_one` is numeric
/// (`01.` counts as 1, per CommonMark's "start number" semantics) —
/// `ordered_marker_value`'s parsed value already has any leading zeros
/// stripped arithmetically.
fn ordered_marker(src: &SrcBytes, item_start: usize) -> Option<(usize, bool, u8)> {
    let (digit_len, value, delim) = ordered_marker_value(src, item_start)?;
    Some((digit_len, value == 1, delim))
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
    src: &SrcBytes,
    line: &ListLineCtx,
    line_col_post: usize,
    affected: &BTreeMap<usize, ListLineCtx>,
    delta: usize,
    indent: bool,
) -> Option<ByteSplice> {
    let (item_start, _) = line.marker?;
    let (digit_len, is_one, delim) = ordered_marker(src, item_start)?;
    if is_one {
        return None; // "1." can interrupt anything; never rewritten
    }
    // Landing scan: skip consecutive same-quote-depth item lines strictly
    // deeper than the checked line's post-edit column; the first line that
    // isn't is the landing.
    let mut joins = false;
    let mut cursor = line.start;
    while let Some(range) = prev_line(src, cursor) {
        let ctx = list_line_ctx(nodes, src, range.clone());
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
            && ordered_marker(src, land_start).is_some_and(|(_, _, d)| d == delim);
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
/// cannot start a list). Walk down from `scan_from` (the byte position past
/// the ENTIRE region the edit affected/consumed — the caller computes this;
/// for `indentList`/`outdentList` it's simply the affected set's last line's
/// own end, but `enter`'s selection path may need to skip further, past
/// lines a multi-line selection deleted outright — see `outdent_single_line`)
/// over consecutive same-quote-depth item lines, SKIPPING adopted
/// descendants (post-edit column strictly greater than the moved line's new
/// column — they nest under the moved block, whose itemness the first guard
/// already preserves); the first item line at column <= the new column is
/// the one whose landing the edit re-anchored — run the same landing-scan
/// check on it at its own (unchanged) column. Stops at a non-item/blank line
/// or quote-depth change, like every other scan here.
#[allow(clippy::too_many_arguments)] // mirrors `interruption_rewrite`'s parameter set + the scan start
fn below_line_rewrite(
    nodes: &[Node],
    src: &SrcBytes,
    affected: &BTreeMap<usize, ListLineCtx>,
    root_depth: u8,
    root_post_col: usize,
    delta: usize,
    indent: bool,
    scan_from: usize,
) -> Option<ByteSplice> {
    let mut cursor_end = scan_from;
    loop {
        let range = next_line(src, cursor_end)?;
        let ctx = list_line_ctx(nodes, src, range.clone());
        if ctx.quote_depth != root_depth {
            return None;
        }
        let (item_start, _) = ctx.marker?;
        let col = item_start - ctx.quote_end; // unaffected: pre == post
        if col > root_post_col {
            cursor_end = range.end; // adopted descendant of the moved block
            continue;
        }
        return interruption_rewrite(nodes, src, &ctx, col, affected, delta, indent);
    }
}

/// Indent every list-item line intersecting `[from_b, to_b]` — plus, for
/// each one, its whole subtree (see the module doc comment) — to its
/// nesting parent's content column. `None` when no intersecting line is a
/// list item; `Some` with an empty batch when it applies but no movement is
/// possible (first item of its list, or already as deep as its parent
/// allows).
pub fn indent_list(nodes: &[Node], src: &SrcBytes, from_b: usize, to_b: usize) -> Option<CommandPlan> {
    plan_list_nesting(nodes, src, from_b, to_b, true)
}

/// Outdent every list-item line intersecting `[from_b, to_b]` — plus each
/// one's subtree — by the first line's distance to its nesting parent.
/// `None`/no-op cases mirror [`indent_list`] (already top-level → no-op
/// instead of `None`, since the command still applies).
pub fn outdent_list(nodes: &[Node], src: &SrcBytes, from_b: usize, to_b: usize) -> Option<CommandPlan> {
    plan_list_nesting(nodes, src, from_b, to_b, false)
}

// ---------------------------------------------------------------------
// enter (boundary v0.3: construct-aware Enter — continue/exit list markers
// and quote prefixes). See the module doc comment's "## enter" section for
// the full spec — this is a direct transcription of it.
// ---------------------------------------------------------------------

/// Whether every byte in `range` is a plain space or tab — "no real content"
/// for `enter`'s empty-item/empty-quote-line checks. An empty range counts
/// as blank (nothing at all after the marker/prefix).
fn is_blank(src: &SrcBytes, range: Range<usize>) -> bool {
    (range.start..range.end).all(|i| matches!(src.get(i), Some(b' ' | b'\t')))
}

/// The `BlockQuoteLine` node for the physical line starting at `line_start`,
/// if any — gives direct access to that line's per-level `"> "` delimiter
/// spans (`enter`'s QUOTE EXIT rule drops only the LAST one).
fn blockquote_line_node(nodes: &[Node], line_start: usize) -> Option<&Node> {
    // Binary-search jump like `quote_context` (extent-start-sorted overlay,
    // runs per keypress): a line's BlockQuoteLine node starts exactly at
    // `line_start`.
    let lo = nodes.partition_point(|n| n.extent.start < line_start);
    let hi = nodes.partition_point(|n| n.extent.start <= line_start);
    nodes[lo..hi]
        .iter()
        .find(|n| matches!(n.kind, NodeKind::BlockQuoteLine(_)))
}

/// `enter`'s EXIT/OUTDENT rule for a NESTED empty item (marker column > 0):
/// outdent this ONE line (no subtree — an empty item is accepted to carry
/// none, per the module doc comment) by the same target-scan/delta
/// arithmetic as `plan_list_nesting`'s outdent path, INCLUDING both
/// structural rewrite guards. `content_start` is the line's own (pre-edit)
/// content start, used to place the post-edit cursor. Returns `None` when no
/// qualifying parent is found above (the same v1 blank-line-scan limitation
/// `outdentList` has) so the caller can fall back to a full marker clear
/// instead of leaving the press inert.
fn outdent_single_line(
    nodes: &[Node],
    src: &SrcBytes,
    line: &ListLineCtx,
    content_start: usize,
    from_b: usize,
    to_b: usize,
) -> Option<CommandPlan> {
    let first_col = line.marker_column().expect("caller verified this line has a marker");

    // Target scan: nearest line above, same quote depth, strictly smaller
    // marker column (identical to `plan_list_nesting`'s outdent branch, just
    // restricted to a single starting line rather than a whole batch).
    let mut target_col: Option<usize> = None;
    let mut cursor = line.start;
    while let Some(range) = prev_line(src, cursor) {
        let ctx = list_line_ctx(nodes, src, range.clone());
        if ctx.quote_depth != line.quote_depth {
            break;
        }
        let Some(col) = ctx.marker_column() else {
            break;
        };
        if col < first_col {
            target_col = Some(col);
            break;
        }
        cursor = range.start;
    }
    let target_col = target_col?;
    let delta = first_col - target_col; // > 0 by construction

    let mut affected: BTreeMap<usize, ListLineCtx> = BTreeMap::new();
    affected.insert(line.start, *line);

    let mut batch = vec![ByteSplice {
        at: line.quote_end,
        delete: delta,
        insert: String::new(),
    }];
    let new_col = first_col - delta; // == target_col
    if let Some(rewrite) = interruption_rewrite(nodes, src, line, new_col, &affected, delta, false) {
        batch.insert(1, rewrite);
    }
    if from_b != to_b {
        batch.push(ByteSplice {
            at: from_b,
            delete: to_b - from_b,
            insert: String::new(),
        });
    }
    // The below-line guard scans from past the ENTIRE region the press
    // affects — not just this line's own end. A selection (rule 7) can
    // extend past `line`'s end, consuming further lines outright (they
    // disappear from the post-edit document, `assert_enter_itemness`'s
    // per-`from`-line exemption does not cover them — losing their itemness
    // is an explicit consequence of the user's own selection, not a silent
    // side effect); the guard must resume scanning from wherever the
    // selection's own deletion actually ends, i.e. the line containing
    // `to_b`, so it lands on the same below-context line the collapsed-
    // cursor (single-line) case would.
    let scan_from = if to_b > line.end { line_containing(src, to_b).end } else { line.end };
    if let Some(rewrite) =
        below_line_rewrite(nodes, src, &affected, line.quote_depth, new_col, delta, false, scan_from)
    {
        batch.push(rewrite);
    }
    let cursor = mapping::map_pos(content_start, &batch, Bias::After);
    Some(CommandPlan {
        batch,
        selection: Some((cursor, cursor)),
    })
}

/// Construct-aware Enter (boundary v0.3): continue a list marker/quote
/// prefix, or exit an empty one in one press. `None` when neither construct
/// applies at `from` (the view falls back to a plain newline). See the
/// module doc comment's "## enter" section for the full spec.
pub fn enter(nodes: &[Node], src: &SrcBytes, from_b: usize, to_b: usize) -> Option<CommandPlan> {
    let (from_b, to_b) = (from_b.min(to_b), from_b.max(to_b));
    let line_range = line_containing(src, from_b);
    let ctx = list_line_ctx(nodes, src, line_range.clone());

    if let Some((item_start, token_width)) = ctx.marker {
        // Same binary-search jump as `quote_context`/`line_marker` (the
        // overlay is extent-start-sorted; this runs per keypress): the
        // marker node starts on this line, at or before the glyph.
        let marker_node = {
            let lo = nodes.partition_point(|n| n.extent.start < line_range.start);
            let hi = nodes.partition_point(|n| n.extent.start < line_range.end);
            nodes[lo..hi].iter().find(|n| {
                matches!(n.kind, NodeKind::ListMarker { .. })
                    && n.extent.start <= item_start
                    && item_start < n.extent.end
            })?
        };
        let task = matches!(marker_node.kind, NodeKind::ListMarker { task: true, .. });
        let after_glyphs = item_start + token_width;
        let content_start = if task {
            // Read the checkbox's own extent rather than assuming a fixed
            // width, so any (CommonMark-tolerated) extra pre-checkbox
            // whitespace is still handled correctly. Binary search like the
            // marker lookup above.
            let widget = {
                let lo = nodes.partition_point(|n| n.extent.start < after_glyphs);
                let hi = nodes.partition_point(|n| n.extent.start < line_range.end);
                nodes[lo..hi]
                    .iter()
                    .find(|n| matches!(n.kind, NodeKind::TaskWidget { .. }))?
            };
            widget.extent.end + 1 // the checkbox's required trailing space
        } else {
            after_glyphs
        }
        // A bare "-"/"1." with NO trailing space is still an empty item per
        // CommonMark/pulldown; the fixed token width assumes the space
        // exists, so clamp — content can never start past the line's end.
        .min(line_range.end);
        if from_b < content_start {
            return None; // cursor sits inside the marker's prefix region
        }
        if !is_blank(src, content_start..line_range.end) {
            // CONTINUE (rule 2).
            let mut prefix = String::new();
            src.push_slice_to(&mut prefix, line_range.start..ctx.quote_end); // quote prefix
            src.push_slice_to(&mut prefix, ctx.quote_end..item_start); // leading indent
            if let Some((_, value, delim)) = ordered_marker_value(src, item_start) {
                // CommonMark caps ordered-list numbers at 9 digits: a
                // 10-digit marker does not parse as a list item at all, so
                // `999999999 + 1` would break this command's own itemness
                // contract. Clamp at the cap — the new item repeats
                // `999999999.` (simplest valid marker; renderers renumber
                // sequentially anyway, so the duplicate is cosmetic-only).
                prefix.push_str(&(value + 1).min(999_999_999).to_string());
                prefix.push(delim as char);
                prefix.push(' ');
            } else {
                prefix.push(src.byte(item_start) as char);
                prefix.push(' ');
            }
            if task {
                prefix.push_str("[ ] ");
            }
            let insert = format!("\n{prefix}");
            let end = from_b + insert.len();
            return Some(CommandPlan {
                batch: vec![ByteSplice {
                    at: from_b,
                    delete: to_b - from_b,
                    insert,
                }],
                selection: Some((end, end)),
            });
        }
        // EXIT/OUTDENT (rule 3).
        let marker_column = item_start - ctx.quote_end;
        if marker_column > 0 {
            if let Some(plan) = outdent_single_line(nodes, src, &ctx, content_start, from_b, to_b) {
                return Some(plan);
            }
            // No qualifying parent above: fall through to the top-level
            // marker-clear branch below rather than leaving the press inert.
        }
        let mut batch = vec![ByteSplice {
            at: item_start,
            delete: content_start - item_start,
            insert: String::new(),
        }];
        if from_b != to_b {
            batch.push(ByteSplice {
                at: from_b,
                delete: to_b - from_b,
                insert: String::new(),
            });
        }
        let cursor = mapping::map_pos(content_start, &batch, Bias::After);
        return Some(CommandPlan {
            batch,
            selection: Some((cursor, cursor)),
        });
    }

    if ctx.quote_depth > 0 {
        if from_b < ctx.quote_end {
            return None; // cursor sits inside the quote markers
        }
        if !is_blank(src, ctx.quote_end..line_range.end) {
            // QUOTE CONTINUE (rule 4).
            let mut prefix = String::new();
            src.push_slice_to(&mut prefix, line_range.start..ctx.quote_end);
            let insert = format!("\n{prefix}");
            let end = from_b + insert.len();
            return Some(CommandPlan {
                batch: vec![ByteSplice {
                    at: from_b,
                    delete: to_b - from_b,
                    insert,
                }],
                selection: Some((end, end)),
            });
        }
        // QUOTE EXIT (rule 5): drop the LAST "> " run element only.
        let bq = blockquote_line_node(nodes, line_range.start)?;
        let last = bq.delims.last()?.clone();
        let mut batch = vec![del(&last)];
        if from_b != to_b {
            batch.push(ByteSplice {
                at: from_b,
                delete: to_b - from_b,
                insert: String::new(),
            });
        }
        let cursor = mapping::map_pos(ctx.quote_end, &batch, Bias::After);
        return Some(CommandPlan {
            batch,
            selection: Some((cursor, cursor)),
        });
    }

    None // rule 1: neither a list marker nor a quote prefix applies here
}
