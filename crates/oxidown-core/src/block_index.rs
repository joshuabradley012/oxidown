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
//!      scores by point-containment;
//!    - a **split** (one old block overlapping two new blocks) gives the
//!      larger-overlap piece the old ID and allocates exactly one fresh ID
//!      for the other piece;
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
        let split = self
            .blocks
            .iter()
            .position(|b| b.span.end > region_start)
            .unwrap_or(self.blocks.len());
        let mapped_old: Vec<(Range<usize>, BlockId)> = self.blocks[split..]
            .iter()
            .map(|b| (mapping::map_range_shrink(&b.span, batch), b.id))
            .collect();
        let matched = match_spans(&mapped_old, tail_spans, self.replica, &mut self.next_counter);
        self.blocks.truncate(split);
        self.blocks.extend(matched);
    }
}

/// Linear-time overlap matcher — see the module docs for the semantics.
/// Both inputs are document-ordered with non-overlapping spans internally,
/// so each new span's overlap candidates are a contiguous old-span run;
/// `i` only ever advances, giving O(old + new + overlaps) total.
fn match_spans(
    mapped_old: &[(Range<usize>, BlockId)],
    new_spans: Vec<(BlockKind, Range<usize>)>,
    replica: u16,
    next_counter: &mut u64,
) -> Vec<Block> {
    let mut claimed = vec![false; mapped_old.len()];
    let mut result = Vec::with_capacity(new_spans.len());
    let mut i = 0usize;
    for (kind, span) in new_spans {
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
        let mut best: Option<(usize, usize)> = None; // (overlap, old index)
        let mut j = i;
        while j < mapped_old.len() && mapped_old[j].0.start < span.end {
            if !claimed[j] {
                let ov = overlap_score(&mapped_old[j].0, &span);
                if ov > 0 && best.is_none_or(|(b, _)| ov > b) {
                    best = Some((ov, j));
                }
            }
            j += 1;
        }
        let id = match best {
            Some((_, j)) => {
                claimed[j] = true;
                mapped_old[j].1
            }
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

/// Overlap in bytes; a collapsed old span (a block whose text was entirely
/// deleted) scores 1 when the new span contains its point, so a
/// full-text-replacement in one splice still keeps the block's ID.
fn overlap_score(old: &Range<usize>, new: &Range<usize>) -> usize {
    if old.start == old.end {
        return usize::from(new.start <= old.start && old.start < new.end);
    }
    let start = old.start.max(new.start);
    let end = old.end.min(new.end);
    end.saturating_sub(start)
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
        let ids_after: Vec<BlockId> = after.iter().map(|(_, id)| *id).collect();
        assert!(
            ids_after.contains(&original_id),
            "one half of the split keeps the original id"
        );
        assert_eq!(
            ids_after.iter().filter(|id| **id == original_id).count(),
            1,
            "only one half keeps it"
        );
        assert_ne!(after[0].1, after[1].1, "the two halves have distinct ids");
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
}
