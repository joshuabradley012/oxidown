//! Block index (plan.md §5.3): a top-level block structure with **stable
//! IDs** that survive edits, sticky the way the contract wants op/anchor
//! identity to be sticky elsewhere in this crate. Internal to M1 (not yet
//! exposed over the wasm boundary) — consumed by streaming's "append only
//! dirties the open tail block" fast path and, per plan.md §5.5, eventually
//! the sidecar/sync story.
//!
//! ## What counts as a "block"
//!
//! Only **top-level** constructs: paragraphs, ATX/setext headings,
//! blockquotes (as a whole — not one entry per nested line), lists (as a
//! whole — not one entry per item), fenced/indented code blocks, thematic
//! breaks, tables, footnote definitions, HTML blocks. A block's span is its
//! *entire* extent including everything nested inside it (a list's span
//! covers every item and their content) — nested structure is what the
//! parser overlay (`parser.rs`) already provides at finer grain for
//! decoration purposes; the block index is deliberately coarse. The spans
//! themselves come from the same single parse pass that builds the overlay
//! ([`crate::parser::parse_document`]) — this module only assigns and
//! maintains identity.
//!
//! ## Stability algorithm
//!
//! [`BlockIndex::update`] is given the freshly parsed top-level spans and
//! the edit batch that was just applied (pre-edit byte coordinates):
//!
//! 1. Map every old block's span through the batch with
//!    [`mapping::map_range_shrink`] — start biased `After`, end biased
//!    `Before`, so a mapped span covers only the images of the block's
//!    ORIGINAL bytes. The shrink bias is load-bearing: with extend bias, an
//!    insertion at a block's exact start that forms its own new block
//!    (e.g. pasting `"para\n\n"` right before an existing paragraph) would
//!    be covered by the old block's mapped span and *steal its identity by
//!    overlap*, leaving the actually-unchanged text with a fresh ID —
//!    backwards. A block whose text was entirely deleted collapses to a
//!    point, which still matches by containment (below), so replacing a
//!    block's whole text in one splice keeps its ID.
//! 2. Match new blocks to mapped-old blocks by span **overlap**, processing
//!    both document-ordered lists with a linear two-pointer merge (both are
//!    sorted and internally non-overlapping, so a new block's overlap
//!    candidates form a contiguous run of old blocks):
//!    - the unclaimed old block with the greatest byte overlap donates its
//!      ID (an edit *inside* a block, or a block merely shifting because
//!      something above it changed, keeps its ID); a collapsed old span
//!      scores by point-containment, but strictly BELOW any real byte
//!      overlap — a fully deleted block's collapsed point must never
//!      outrank a surviving neighbor's genuine mapped bytes (else deleting
//!      block B while rewriting its neighbor down to a 1-byte overlap would
//!      hand B's ID to the unchanged neighbor and retire the neighbor's
//!      own ID);
//!    - a **split** (one old block overlapping two new blocks) gives the
//!      larger-overlap piece the old ID (ties: the earlier piece) and
//!      allocates exactly one fresh ID for the other piece;
//!    - a new block overlapping nothing (fresh content, or the very first
//!      parse) gets a fresh ID.
//!
//!    Old blocks that no new block claims (deleted, or the losing side of a
//!    **merge** where two old blocks' mapped spans both overlap one new
//!    block and only the larger-overlap one wins) simply retire — IDs are
//!    never reused, matching the op log's counter discipline.
//!
//! [`BlockIndex::update_tail`] is the streaming fast path: everything
//! strictly before `region_start` is untouched (an append at/after the tail
//! block's start cannot move earlier blocks), and only the tail sublist is
//! re-matched.

use std::ops::Range;

use crate::mapping;
pub use crate::parser::BlockKind;
use crate::text::ByteSplice;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId {
    pub replica: u16,
    pub counter: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub id: BlockId,
    pub kind: BlockKind,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct BlockIndex {
    replica: u16,
    next_counter: u64,
    blocks: Vec<Block>,
}

impl BlockIndex {
    pub fn new(replica: u16) -> Self {
        Self {
            replica,
            next_counter: 1,
            blocks: Vec::new(),
        }
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Forget all blocks (used by `load`: a replaced document must not
    /// inherit IDs by coincidental span overlap). Counters stay monotonic —
    /// IDs are never reused within an editor's lifetime.
    pub fn clear(&mut self) {
        self.blocks.clear();
    }

    /// Rebuild the whole index from freshly parsed `new_spans` (the document
    /// *after* `batch` was applied), matching against the previous blocks
    /// mapped through `batch`. An empty `batch` (e.g. right after `load`,
    /// where `clear` ran first) simply assigns fresh IDs to everything.
    pub fn update(&mut self, new_spans: Vec<(BlockKind, Range<usize>)>, batch: &[ByteSplice]) {
        let mapped_old: Vec<(Range<usize>, BlockId)> = self
            .blocks
            .iter()
            .map(|b| (mapping::map_range_shrink(&b.span, batch), b.id))
            .collect();
        self.blocks = match_spans(&mapped_old, new_spans, self.replica, &mut self.next_counter);
    }

    /// Streaming fast path: re-match only the blocks whose span reaches into
    /// `[region_start, ..)`. `tail_spans` are the freshly parsed top-level
    /// spans of that region, already offset to whole-document coordinates;
    /// `batch` is the applied append batch (its splices all land at/after
    /// `region_start`, so earlier blocks cannot have moved).
    pub fn update_tail(
        &mut self,
        region_start: usize,
        tail_spans: Vec<(BlockKind, Range<usize>)>,
        batch: &[ByteSplice],
    ) {
        // Spans are document-ordered and non-overlapping, so `span.end` is
        // strictly increasing — the first block reaching past `region_start`
        // is a partition_point (O(log blocks); this runs per stream append).
        let split = self.blocks.partition_point(|b| b.span.end <= region_start);
        let mapped_old: Vec<(Range<usize>, BlockId)> = self.blocks[split..]
            .iter()
            .map(|b| (mapping::map_range_shrink(&b.span, batch), b.id))
            .collect();
        let matched = match_spans(&mapped_old, tail_spans, self.replica, &mut self.next_counter);
        self.blocks.truncate(split);
        self.blocks.extend(matched);
    }

    /// Windowed fast path for `reparse_incremental`'s block-index step
    /// (`editor.rs` step 3b): mirrors the overlay's prefix/fresh/suffix
    /// splice (step 3a) instead of routing through the whole-document
    /// [`BlockIndex::update`]. `old_window` indexes into the CURRENT
    /// `self.blocks` (pre-call): blocks strictly before `old_window.start`
    /// are left completely alone — same `Block` entries, same IDs, no
    /// allocation, no call into [`mapping::map_range_shrink`]; blocks in
    /// `old_window` are re-matched against `fresh_spans` (already in
    /// document/absolute byte coordinates) using the exact same
    /// [`match_spans`] heuristic `update` uses, just restricted to that
    /// sub-slice; blocks from `old_window.end` on have their spans shifted
    /// IN PLACE by `suffix_delta` (`batch`'s net byte delta,
    /// `Σ insert.len() - delete`), IDs retained, no rematch.
    ///
    /// `old_window` and `fresh_spans` are exactly what `reparse_incremental`
    /// already computes for the overlay splice (the window
    /// `[before_len, m + 1)` and the freshly parsed window blocks, offset to
    /// absolute coordinates) — see its doc comment for the "one block of
    /// slack" / convergence-point argument this relies on.
    ///
    /// ## Why this produces exactly what `update` would have
    ///
    /// `update` maps EVERY old block through `batch` and hands the whole
    /// mapped-old list plus the whole new-spans list to [`match_spans`]'s
    /// two-pointer overlap merge. Splitting that single call into three
    /// independent pieces is only valid if no edge (candidate ID donation)
    /// can ever cross a piece boundary — otherwise a window match could
    /// "steal" an ID that the full computation would have awarded to a
    /// prefix/suffix block, or vice versa. Two boundary facts, both
    /// load-bearing invariants of `reparse_incremental`'s own window
    /// selection (not reproved here — see its doc comment), rule that out:
    ///
    /// * **prefix boundary.** `old_window.start` is chosen so every prefix
    ///   block's span ends at or before `region_start`, which is itself at
    ///   or before every splice in `batch` (`reparse_incremental`'s "one
    ///   block of slack" window start). A span entirely before every splice
    ///   maps through [`mapping::map_range_shrink`] to ITSELF, unchanged
    ///   (`Bias::After`/`Bias::Before` only ever move a position at or past
    ///   the first splice they touch) — so the prefix's mapped-old spans are
    ///   bit-for-bit the prefix's stored spans, which is exactly what this
    ///   function passes through. Since `fresh_spans`' own first span starts
    ///   at/after `region_start` (it comes from parsing the slice
    ///   `[region_start, ..)`), no prefix span and no fresh span can overlap
    ///   — the prefix's candidate edges in a full `match_spans` call are
    ///   confined to (prefix-old, prefix-new) pairs, each a perfect,
    ///   unambiguous self-match (identical range, no competing candidate).
    /// * **suffix boundary.** `old_window.end` is chosen so `blocks[m]`
    ///   (the last window block) ends exactly at `p_pre`, an old block end
    ///   at/after `batch`'s last splice — so every suffix block's span
    ///   starts at/after `p_pre`, at/after every splice in `batch`. A
    ///   position at/after every splice maps through `map_range_shrink` to
    ///   itself shifted by the batch's full net delta (each splice
    ///   contributes its own `insert.len() - delete` once, whether via the
    ///   ordinary "splice entirely before" arm or, for a position landing
    ///   exactly on a trailing pure insertion, the bias-driven absorb arm —
    ///   both land on the same total): exactly `suffix_delta`, exactly what
    ///   this function applies in place. `fresh_spans`' own last span ends
    ///   at `p_pre`'s image, at or before every (shifted) suffix span's
    ///   start, so again no cross-boundary overlap is possible.
    ///
    /// With no edges crossing either boundary, the full computation's edge
    /// set is exactly the union of the three pieces' edge sets — so
    /// matching them independently (or, for prefix/suffix, not matching at
    /// all, since their only possible edge is the trivial self-match)
    /// yields identical `(span, id)` results to calling `update` on the
    /// assembled `before ++ fresh ++ shifted-after` list. Pinned by
    /// `tests/reparse_equivalence.rs`'s sentinel-block property fuzz and by
    /// this module's own `update_range_matches_full_update_on_the_same_edit`
    /// tests.
    pub fn update_range(
        &mut self,
        old_window: Range<usize>,
        fresh_spans: Vec<(BlockKind, Range<usize>)>,
        suffix_delta: isize,
        batch: &[ByteSplice],
    ) {
        debug_assert!(
            old_window.start <= old_window.end && old_window.end <= self.blocks.len(),
            "old_window {old_window:?} out of bounds for {} blocks",
            self.blocks.len()
        );
        let mapped_old: Vec<(Range<usize>, BlockId)> = self.blocks[old_window.clone()]
            .iter()
            .map(|b| (mapping::map_range_shrink(&b.span, batch), b.id))
            .collect();
        let matched = match_spans(&mapped_old, fresh_spans, self.replica, &mut self.next_counter);
        for b in &mut self.blocks[old_window.end..] {
            b.span.start = (b.span.start as isize + suffix_delta) as usize;
            b.span.end = (b.span.end as isize + suffix_delta) as usize;
        }
        self.blocks.splice(old_window, matched);
    }
}

/// Linear-time overlap matcher — see the module docs for the semantics.
/// Both inputs are document-ordered with non-overlapping spans internally,
/// so each new span's overlap candidates are a contiguous old-span run;
/// `i` only ever advances, giving O(old + new + overlaps) total.
///
/// The split/merge rule is symmetric in both directions of ambiguity: an
/// old block DONATES its ID to the new span it overlaps most (a split's
/// larger-overlap piece keeps the ID, ties to the earlier piece — the
/// module docs' promise; a plain document-order greedy would instead hand
/// it to whichever piece comes first); a new span TAKES the largest-overlap
/// donor among the old blocks that chose it (a merge's larger side wins,
/// ties to the earlier block). Old blocks that donate nowhere, or lose a
/// merge, retire — IDs are never reused.
fn match_spans(
    mapped_old: &[(Range<usize>, BlockId)],
    new_spans: Vec<(BlockKind, Range<usize>)>,
    replica: u16,
    next_counter: &mut u64,
) -> Vec<Block> {
    // (new index, old index, overlap) edges from the two-pointer walk.
    let mut edges: Vec<(usize, usize, OverlapScore)> = Vec::new();
    let mut i = 0usize;
    for (n, (_, span)) in new_spans.iter().enumerate() {
        // Skip old spans entirely before `span`. A collapsed old span
        // sitting exactly at `span.start` is NOT "before" — it matches this
        // span by point-containment, so only skip it when strictly before.
        while i < mapped_old.len() && {
            let old = &mapped_old[i].0;
            if old.start < old.end {
                old.end <= span.start
            } else {
                old.start < span.start
            }
        } {
            i += 1;
        }
        let mut j = i;
        while j < mapped_old.len() && mapped_old[j].0.start < span.end {
            let ov = overlap_score(&mapped_old[j].0, span);
            if ov.1 > 0 {
                edges.push((n, j, ov));
            }
            j += 1;
        }
    }
    // Strictly-greater comparisons make both passes keep the FIRST maximum
    // in document order (the tie rules above).
    let mut donates_to: Vec<Option<(OverlapScore, usize)>> = vec![None; mapped_old.len()]; // (overlap, new idx)
    for &(n, j, ov) in &edges {
        if donates_to[j].is_none_or(|(best, _)| ov > best) {
            donates_to[j] = Some((ov, n));
        }
    }
    let mut taken: Vec<Option<(OverlapScore, usize)>> = vec![None; new_spans.len()]; // (overlap, old idx)
    for &(n, j, ov) in &edges {
        if donates_to[j] == Some((ov, n)) && taken[n].is_none_or(|(best, _)| ov > best) {
            taken[n] = Some((ov, j));
        }
    }
    let mut result = Vec::with_capacity(new_spans.len());
    for (n, (kind, span)) in new_spans.into_iter().enumerate() {
        let id = match taken[n] {
            Some((_, j)) => mapped_old[j].1,
            None => {
                let id = BlockId {
                    replica,
                    counter: *next_counter,
                };
                *next_counter += 1;
                id
            }
        };
        result.push(Block { id, kind, span });
    }
    result
}

/// Overlap score, ordered as a tuple: `(is_real_byte_overlap, byte_count)`.
///
/// A collapsed old span (a block whose text was entirely deleted) scores
/// `(false, 1)` when the new span contains its point, so a
/// full-text-replacement in one splice still keeps the block's ID — but the
/// `false` first component ranks it strictly below ANY genuine byte overlap
/// (`(true, n)`, n ≥ 1). Without that ranking, a fully deleted block's
/// collapsed point would TIE a surviving neighbor whose real mapped overlap
/// is exactly 1 byte, and the "earlier old block wins" tie rule would hand
/// the deleted block's ID to the unchanged neighbor — violating the module
/// invariant that deleted blocks simply retire. Real-overlap ties (and
/// collapsed-vs-collapsed ties) keep the documented split/merge tie rules.
///
/// A score whose `byte_count` is 0 means "no match" regardless of the flag
/// (callers filter on `.1 > 0`).
type OverlapScore = (bool, usize);

fn overlap_score(old: &Range<usize>, new: &Range<usize>) -> OverlapScore {
    if old.start == old.end {
        return (false, usize::from(new.start <= old.start && old.start < new.end));
    }
    let start = old.start.max(new.start);
    let end = old.end.min(new.end);
    (true, end.saturating_sub(start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    /// Test convenience: parse + update in one call (production code goes
    /// through `Editor`, which shares one parse pass with the overlay).
    fn reparse(idx: &mut BlockIndex, text: &str, batch: &[ByteSplice]) {
        idx.update(parser::parse_document(text).blocks, batch);
    }

    fn ids(idx: &BlockIndex) -> Vec<(BlockKind, BlockId)> {
        idx.blocks().iter().map(|b| (b.kind, b.id)).collect()
    }

    fn sp(at: usize, delete: usize, insert: &str) -> ByteSplice {
        ByteSplice {
            at,
            delete,
            insert: insert.into(),
        }
    }

    #[test]
    fn fresh_parse_assigns_sequential_ids() {
        let mut idx = BlockIndex::new(1);
        reparse(&mut idx, "# h\n\npara one\n\npara two\n", &[]);
        let blocks = idx.blocks();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].kind, BlockKind::Heading);
        assert_eq!(blocks[1].kind, BlockKind::Paragraph);
        assert_eq!(blocks[2].kind, BlockKind::Paragraph);
        assert_eq!(blocks[0].id, BlockId { replica: 1, counter: 1 });
        assert_eq!(blocks[1].id, BlockId { replica: 1, counter: 2 });
        assert_eq!(blocks[2].id, BlockId { replica: 1, counter: 3 });
    }

    #[test]
    fn edit_inside_a_block_keeps_its_id() {
        let mut idx = BlockIndex::new(1);
        let base = "para one\n\npara two\n";
        reparse(&mut idx, base, &[]);
        let before = ids(&idx);

        // Insert text inside "para one".
        let batch = [sp(4, 0, "XYZ")];
        let after_text = "paraXYZ one\n\npara two\n";
        reparse(&mut idx, after_text, &batch);
        let after = ids(&idx);

        assert_eq!(before, after, "both blocks keep their IDs across an interior edit");
    }

    #[test]
    fn unrelated_edit_elsewhere_keeps_all_ids_stable() {
        let mut idx = BlockIndex::new(1);
        reparse(&mut idx, "first\n\nsecond\n\nthird\n", &[]);
        let before = ids(&idx);

        // Insert a new paragraph in the middle (shifts "third" down, adds
        // one new block); first/second/third must keep their identities and
        // the new block must be fresh.
        let batch = [sp(15, 0, "inserted\n\n")];
        let after_text = "first\n\nsecond\n\ninserted\n\nthird\n";
        reparse(&mut idx, after_text, &batch);
        let after = ids(&idx);

        assert_eq!(after.len(), 4);
        assert_eq!(after[0], before[0], "first");
        assert_eq!(after[1], before[1], "second");
        // after[2] is the freshly inserted paragraph — must not collide.
        assert!(!before.contains(&after[2]));
        assert_eq!(after[3], before[2], "third");
    }

    #[test]
    fn splitting_a_block_keeps_one_id_and_allocates_one_new_one() {
        let mut idx = BlockIndex::new(1);
        let base = "one paragraph of text\n";
        reparse(&mut idx, base, &[]);
        let before = ids(&idx);
        assert_eq!(before.len(), 1);
        let original_id = before[0].1;

        // Split it into two paragraphs by inserting a blank line in the
        // middle ("one paragraph" / "of text").
        let batch = [sp(13, 0, "\n\n")];
        let after_text = "one paragraph\n\nof text\n";
        reparse(&mut idx, after_text, &batch);
        let after = ids(&idx);

        assert_eq!(after.len(), 2);
        // The LARGER-overlap piece keeps the ID ("one paragraph" = 14 bytes
        // incl. its newline vs "of text" = 8) — not merely "some piece".
        assert_eq!(after[0].1, original_id, "the larger (first) piece keeps the id");
        assert_ne!(after[1].1, original_id, "the smaller piece gets a fresh id");
    }

    #[test]
    fn split_gives_the_larger_overlap_piece_the_old_id_regardless_of_order() {
        // Larger piece SECOND: a document-order greedy would hand the ID to
        // the first piece; the documented rule must not.
        let mut idx = BlockIndex::new(1);
        reparse(&mut idx, "ab paragraph long text here\n", &[]);
        let original = idx.blocks()[0].id;
        let batch = [sp(2, 1, "\n\n")]; // replace the space after "ab"
        reparse(&mut idx, "ab\n\nparagraph long text here\n", &batch);
        assert_ne!(idx.blocks()[0].id, original, "smaller (first) piece: fresh id");
        assert_eq!(idx.blocks()[1].id, original, "larger piece keeps the id despite coming second");

        // Tie: equal-overlap halves — the earlier piece wins.
        let mut idx = BlockIndex::new(1);
        reparse(&mut idx, "aa bb\n", &[]);
        let original = idx.blocks()[0].id;
        let batch = [sp(2, 1, "\n\n")];
        reparse(&mut idx, "aa\n\nbb\n", &batch);
        assert_eq!(idx.blocks()[0].id, original, "tie goes to the earlier piece");
        assert_ne!(idx.blocks()[1].id, original);
    }

    #[test]
    fn merging_two_blocks_keeps_exactly_one_id() {
        let mut idx = BlockIndex::new(1);
        let base = "one\n\ntwo\n";
        reparse(&mut idx, base, &[]);
        let before = ids(&idx);
        assert_eq!(before.len(), 2);

        // Delete the blank line between them, merging into one paragraph
        // (CommonMark: "one\ntwo\n" is a single paragraph with a soft break).
        let batch = [sp(3, 1, "")];
        let after_text = "one\ntwo\n";
        reparse(&mut idx, after_text, &batch);
        let after = ids(&idx);

        assert_eq!(after.len(), 1);
        assert!(
            after[0].1 == before[0].1 || after[0].1 == before[1].1,
            "the merged block keeps one of the two original ids"
        );
    }

    #[test]
    fn merge_gives_the_larger_overlap_donor_the_id_and_ties_go_to_the_earlier_block() {
        // MERGE direction (two old blocks -> one new block) of the
        // split/merge tie/size rule, pinned specifically: the donor with
        // the larger mapped overlap wins REGARDLESS of document order.
        // Here the SECOND paragraph is larger ("aa\n" maps to 3 bytes,
        // "bbbb bbbb\n" to 10), so a document-order greedy would be wrong.
        let mut idx = BlockIndex::new(1);
        reparse(&mut idx, "aa\n\nbbbb bbbb\n", &[]);
        let (first, second) = (idx.blocks()[0].id, idx.blocks()[1].id);
        let batch = [sp(3, 1, "")]; // delete the separating blank line
        reparse(&mut idx, "aa\nbbbb bbbb\n", &batch);
        assert_eq!(idx.blocks().len(), 1);
        assert_eq!(idx.blocks()[0].id, second, "larger donor wins the merge");
        assert_ne!(idx.blocks()[0].id, first, "the smaller donor retires");

        // Tie: equal mapped overlaps (3 bytes each) — the EARLIER old block
        // donates, per the documented tie rule.
        let mut idx = BlockIndex::new(1);
        reparse(&mut idx, "aa\n\nbb\n", &[]);
        let (first, second) = (idx.blocks()[0].id, idx.blocks()[1].id);
        let batch = [sp(3, 1, "")];
        reparse(&mut idx, "aa\nbb\n", &batch);
        assert_eq!(idx.blocks().len(), 1);
        assert_eq!(idx.blocks()[0].id, first, "tie goes to the earlier block");
        assert_ne!(idx.blocks()[0].id, second);
    }

    #[test]
    fn deleting_a_block_retires_its_id_permanently() {
        let mut idx = BlockIndex::new(1);
        reparse(&mut idx, "first\n\nsecond\n\nthird\n", &[]);
        let before = ids(&idx);
        let second_id = before[1].1;

        // Delete "second\n\n" entirely.
        let batch = [sp(7, 8, "")];
        let after_text = "first\n\nthird\n";
        reparse(&mut idx, after_text, &batch);
        let after = ids(&idx);

        assert_eq!(after.len(), 2);
        assert!(!after.iter().any(|(_, id)| *id == second_id));

        // Re-adding a similar paragraph later must NOT reuse the retired id.
        let batch2 = [sp(7, 0, "second again\n\n")];
        let after_text2 = "first\n\nsecond again\n\nthird\n";
        reparse(&mut idx, after_text2, &batch2);
        assert!(!idx.blocks().iter().any(|b| b.id == second_id));
    }

    #[test]
    fn unchanged_blocks_keep_ids_across_many_reparses() {
        let mut idx = BlockIndex::new(1);
        let mut text = String::from("alpha\n\nbeta\n\ngamma\n");
        reparse(&mut idx, &text, &[]);
        let alpha_id = idx.blocks()[0].id;
        let gamma_id = idx.blocks()[2].id;

        for i in 0..5 {
            let marker = format!("<{i}>");
            let at = text.find("beta").unwrap() + 4;
            let batch = [sp(at, 0, &marker)];
            text.insert_str(at, &marker);
            reparse(&mut idx, &text, &batch);
            assert_eq!(idx.blocks()[0].id, alpha_id, "alpha stable at step {i}");
            assert_eq!(idx.blocks()[2].id, gamma_id, "gamma stable at step {i}");
        }
    }

    #[test]
    fn list_and_blockquote_are_single_top_level_blocks() {
        let mut idx = BlockIndex::new(1);
        reparse(&mut idx, "- one\n- two\n- three\n", &[]);
        assert_eq!(idx.blocks().len(), 1);
        assert_eq!(idx.blocks()[0].kind, BlockKind::List);

        idx.clear();
        reparse(&mut idx, "> line one\n> line two\n", &[]);
        assert_eq!(idx.blocks().len(), 1);
        assert_eq!(idx.blocks()[0].kind, BlockKind::BlockQuote);
    }

    #[test]
    fn clear_prevents_coincidental_inheritance_and_keeps_counters() {
        let mut idx = BlockIndex::new(1);
        reparse(&mut idx, "same text\n", &[]);
        let old_id = idx.blocks()[0].id;
        idx.clear();
        reparse(&mut idx, "same text\n", &[]);
        assert_ne!(idx.blocks()[0].id, old_id, "load must not resurrect ids");
        assert!(idx.blocks()[0].id.counter > old_id.counter);
    }

    #[test]
    fn update_tail_rematches_only_the_tail() {
        let mut idx = BlockIndex::new(1);
        let text = "first\n\nsecond\n\ntail paragraph";
        reparse(&mut idx, text, &[]);
        let before = ids(&idx);
        let tail_start = text.find("tail").unwrap();

        // Append to the tail block; re-parse only the tail slice.
        let batch = [sp(text.len(), 0, " grows")];
        let new_text = "first\n\nsecond\n\ntail paragraph grows";
        let slice = &new_text[tail_start..];
        let tail_spans: Vec<(BlockKind, Range<usize>)> = parser::parse_document(slice)
            .blocks
            .into_iter()
            .map(|(k, r)| (k, r.start + tail_start..r.end + tail_start))
            .collect();
        idx.update_tail(tail_start, tail_spans, &batch);

        let after = ids(&idx);
        assert_eq!(before, after, "all three ids survive a tail-only update");
        assert_eq!(
            idx.blocks()[2].span,
            tail_start..new_text.len(),
            "tail span grew to cover the append"
        );
    }

    #[test]
    fn update_tail_split_allocates_one_new_id() {
        let mut idx = BlockIndex::new(1);
        let text = "head\n\ntail paragraph";
        reparse(&mut idx, text, &[]);
        let tail_id = idx.blocks()[1].id;
        let tail_start = text.find("tail").unwrap();

        // Append a chunk containing a block boundary: tail splits in two.
        let batch = [sp(text.len(), 0, "\n\nnew block")];
        let new_text = "head\n\ntail paragraph\n\nnew block";
        let slice = &new_text[tail_start..];
        let tail_spans: Vec<(BlockKind, Range<usize>)> = parser::parse_document(slice)
            .blocks
            .into_iter()
            .map(|(k, r)| (k, r.start + tail_start..r.end + tail_start))
            .collect();
        idx.update_tail(tail_start, tail_spans, &batch);

        assert_eq!(idx.blocks().len(), 3);
        assert_eq!(idx.blocks()[1].id, tail_id, "original tail keeps its id");
        assert_ne!(idx.blocks()[2].id, tail_id, "new block gets a fresh id");
    }

    // ---- update_range: windowed fast path (mirrors reparse_incremental's
    // step 3b; research/09-1mb-derisk.md's "FIXED" item) --------------------

    /// Five distinct top-level blocks; the edit below lands entirely inside
    /// block 2's text and replaces a marker with a blank line, splitting it
    /// in two — exercising a window that both shrinks a slack block's
    /// neighbor untouched AND changes the block COUNT inside the window,
    /// while blocks 0 (prefix) and 3/4 (suffix) sit outside it.
    fn five_block_doc() -> String {
        "block0 text\n\nblock1 text\n\nblock2 firstpart SPLIT_MARKER secondpart\n\n\
         block3 text\n\nblock4 text\n"
            .to_string()
    }

    #[test]
    fn update_range_leaves_prefix_blocks_byte_for_byte_untouched() {
        let mut idx = BlockIndex::new(1);
        let text = five_block_doc();
        reparse(&mut idx, &text, &[]);
        assert_eq!(idx.blocks().len(), 5);
        let block0_before = idx.blocks()[0].clone();

        let at = text.find("SPLIT_MARKER").unwrap();
        let batch = [sp(at, "SPLIT_MARKER".len(), "\n\n")];
        let after_text = format!("{}{}{}", &text[..at], "\n\n", &text[at + "SPLIT_MARKER".len()..]);
        let delta: isize = "\n\n".len() as isize - "SPLIT_MARKER".len() as isize;
        let new_blocks = parser::parse_document(&after_text).blocks;

        // Window = [1, 3): block1 (one block of slack) + block2 (the edited
        // block); block0 is entirely outside it.
        idx.update_range(1..3, new_blocks[1..4].to_vec(), delta, &batch);

        assert_eq!(
            idx.blocks()[0], block0_before,
            "a block strictly before the window keeps its exact (id, span) entry, \
             untouched by the edit or by any remapping"
        );
    }

    #[test]
    fn update_range_shifts_suffix_spans_in_place_and_keeps_ids() {
        let mut idx = BlockIndex::new(1);
        let text = five_block_doc();
        reparse(&mut idx, &text, &[]);
        let block3_id = idx.blocks()[3].id;
        let block4_id = idx.blocks()[4].id;
        let block3_span_before = idx.blocks()[3].span.clone();
        let block4_span_before = idx.blocks()[4].span.clone();

        let at = text.find("SPLIT_MARKER").unwrap();
        let batch = [sp(at, "SPLIT_MARKER".len(), "\n\n")];
        let after_text = format!("{}{}{}", &text[..at], "\n\n", &text[at + "SPLIT_MARKER".len()..]);
        let delta: isize = "\n\n".len() as isize - "SPLIT_MARKER".len() as isize;
        let new_blocks = parser::parse_document(&after_text).blocks;

        idx.update_range(1..3, new_blocks[1..4].to_vec(), delta, &batch);

        assert_eq!(idx.blocks().len(), 6, "block2 split into two, one net new block");
        assert_eq!(idx.blocks()[4].id, block3_id, "suffix block keeps its id");
        assert_eq!(idx.blocks()[5].id, block4_id, "suffix block keeps its id");
        assert_eq!(
            idx.blocks()[4].span,
            (block3_span_before.start as isize + delta) as usize
                ..(block3_span_before.end as isize + delta) as usize,
            "suffix span shifts by exactly the batch's net delta, in place"
        );
        assert_eq!(
            idx.blocks()[5].span,
            (block4_span_before.start as isize + delta) as usize
                ..(block4_span_before.end as isize + delta) as usize,
        );
    }

    #[test]
    fn update_range_rematches_a_split_inside_the_window_like_update_does() {
        let mut idx = BlockIndex::new(1);
        let text = five_block_doc();
        reparse(&mut idx, &text, &[]);
        let block1_id = idx.blocks()[1].id;
        let block2_id = idx.blocks()[2].id;

        let at = text.find("SPLIT_MARKER").unwrap();
        let batch = [sp(at, "SPLIT_MARKER".len(), "\n\n")];
        let after_text = format!("{}{}{}", &text[..at], "\n\n", &text[at + "SPLIT_MARKER".len()..]);
        let delta: isize = "\n\n".len() as isize - "SPLIT_MARKER".len() as isize;
        let new_blocks = parser::parse_document(&after_text).blocks;

        idx.update_range(1..3, new_blocks[1..4].to_vec(), delta, &batch);

        assert_eq!(idx.blocks()[1].id, block1_id, "the slack block inside the window keeps its id");
        // block2 split into blocks[2] ("...firstpart") and blocks[3]
        // ("secondpart..."): exactly one keeps block2's old id (the
        // larger-overlap piece — "firstpart" here), the other is fresh.
        let split_ids = [idx.blocks()[2].id, idx.blocks()[3].id];
        assert_eq!(
            split_ids.iter().filter(|&&id| id == block2_id).count(),
            1,
            "exactly one half of the split keeps block2's old id: {split_ids:?}"
        );
    }

    #[test]
    fn update_range_matches_full_update_on_the_same_edit() {
        // The equivalence claim `update_range`'s doc comment argues for,
        // pinned directly: driving the SAME edit through the OLD "assemble
        // before ++ fresh ++ shifted-after, call `update`" shape and through
        // `update_range` must produce byte-for-byte, id-for-id identical
        // results.
        let mut idx_full = BlockIndex::new(1);
        let mut idx_windowed = BlockIndex::new(1);
        let text = five_block_doc();
        reparse(&mut idx_full, &text, &[]);
        reparse(&mut idx_windowed, &text, &[]);
        assert_eq!(idx_full.blocks(), idx_windowed.blocks());

        let at = text.find("SPLIT_MARKER").unwrap();
        let batch = [sp(at, "SPLIT_MARKER".len(), "\n\n")];
        let after_text = format!("{}{}{}", &text[..at], "\n\n", &text[at + "SPLIT_MARKER".len()..]);
        let delta: isize = "\n\n".len() as isize - "SPLIT_MARKER".len() as isize;
        let new_blocks = parser::parse_document(&after_text).blocks;
        assert_eq!(new_blocks.len(), 6, "block2 splits into two paragraphs");

        idx_full.update(new_blocks.clone(), &batch);
        idx_windowed.update_range(1..3, new_blocks[1..4].to_vec(), delta, &batch);

        assert_eq!(
            idx_full.blocks(),
            idx_windowed.blocks(),
            "windowed update_range must match the full assemble-and-update path exactly"
        );
    }

    #[test]
    fn update_range_matches_full_update_when_the_window_touches_a_deleted_block() {
        // A second equivalence case with a DIFFERENT shape: the window's
        // last block is fully deleted (collapses to a point for matching
        // purposes, per the module docs) right at the window/suffix
        // boundary — the scenario the module doc's overlap-scoring section
        // calls out as needing the `(false, n)` vs `(true, n)` ranking, now
        // checked across the window/suffix split too.
        let mut idx_full = BlockIndex::new(1);
        let mut idx_windowed = BlockIndex::new(1);
        let text = "block0 text\n\nblock1 text\n\nblock2 doomed\n\nblock3 text\n\nblock4 text\n";
        reparse(&mut idx_full, text, &[]);
        reparse(&mut idx_windowed, text, &[]);

        // Delete "block2 doomed\n\n" entirely (block 2 fully collapses).
        let at = text.find("block2").unwrap();
        let del_len = "block2 doomed\n\n".len();
        let batch = [sp(at, del_len, "")];
        let after_text = format!("{}{}", &text[..at], &text[at + del_len..]);
        let delta: isize = -(del_len as isize);
        let new_blocks = parser::parse_document(&after_text).blocks;
        assert_eq!(new_blocks.len(), 4, "block2 disappears entirely");

        idx_full.update(new_blocks.clone(), &batch);
        // Window = [1, 3): block1 (slack) + block2 (deleted); fresh spans
        // for that window = just the new block1 (new_blocks[1], since
        // block2 contributes nothing).
        idx_windowed.update_range(1..3, new_blocks[1..2].to_vec(), delta, &batch);

        assert_eq!(idx_full.blocks(), idx_windowed.blocks());
    }
}
