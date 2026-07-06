//! Shared position/range mapping through an edit batch.
//!
//! Every internal consumer of "where did this byte position end up after
//! this edit" — the IME composition range (`composition.rs`), the block
//! index's stable spans (`block_index.rs`), and public anchors
//! (`anchor.rs`) — needs the same core algorithm: walk an ascending,
//! non-overlapping `ByteSplice` batch (pre-edit byte coordinates) and
//! compute the position's image in post-edit coordinates. The only thing
//! that differs between callers is **bias**: what happens when an insertion
//! (a zero-delete splice) lands exactly on the position. `Bias` makes that
//! the one parameter instead of three near-duplicate implementations.

use std::ops::Range;

use crate::text::ByteSplice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bias {
    /// An insertion landing exactly at this position leaves it BEFORE the
    /// insertion (the position does not move forward past the new text).
    /// This is the M0 composition-range behavior, preserved exactly.
    Before,
    /// An insertion landing exactly at this position moves it to AFTER the
    /// inserted text (the position "absorbs" the insertion).
    After,
}

/// Map a single byte position through `batch` (ascending, non-overlapping,
/// pre-edit byte coordinates). A position strictly inside a deleted range
/// always collapses to the start of that splice's replacement, regardless
/// of bias — bias only disambiguates a *pure insertion* (zero-delete
/// splice) landing exactly on the position. A batch's splices are assumed
/// ascending, so once a splice strictly past `p` is seen, nothing later can
/// affect the result.
pub fn map_pos(p: usize, batch: &[ByteSplice], bias: Bias) -> usize {
    let mut delta: isize = 0;
    for s in batch {
        if s.delete == 0 && s.at == p {
            match bias {
                // Absorb this (and any further same-position) insertion(s)
                // and keep scanning — a later splice in the same batch could
                // also sit exactly at `p`.
                Bias::After => {
                    delta += s.insert.len() as isize;
                    continue;
                }
                // Nothing at or after `p` in an ascending batch can affect a
                // Before-biased position once we've reached this splice.
                Bias::Before => break,
            }
        }
        let del_end = s.at + s.delete;
        if del_end < p || (del_end == p && s.delete > 0) {
            // Splice entirely before p (a deletion ending exactly at p
            // still shifts it; a pure insertion exactly at p is handled
            // above, before this check is reached).
            delta += s.insert.len() as isize - s.delete as isize;
        } else if s.at < p {
            // p strictly inside the deleted range: collapse to the start of
            // the replacement.
            return usize::try_from(s.at as isize + delta).unwrap_or(0);
        } else {
            break; // batch is ascending: everything else is after p
        }
    }
    usize::try_from(p as isize + delta).unwrap_or(0)
}

/// Map a byte range through `batch`: the start with [`Bias::Before`] (an
/// insertion exactly at the start is absorbed *into* the range, since the
/// start position itself doesn't move while everything from it onward
/// shifts forward) and the end with [`Bias::After`] (an insertion exactly at
/// the end is likewise absorbed, extending the range) — the natural
/// "typing at either edge of this span extends it" behavior for a block or
/// node extent. Collapses to an empty range at `start` if the batch would
/// otherwise invert it (e.g. the whole range was deleted).
pub fn map_range(range: &Range<usize>, batch: &[ByteSplice]) -> Range<usize> {
    let start = map_pos(range.start, batch, Bias::Before);
    let end = map_pos(range.end, batch, Bias::After).max(start);
    start..end
}

/// Map a byte range through `batch`, *excluding* edge insertions: start with
/// [`Bias::After`], end with [`Bias::Before`], so the mapped range covers
/// only the images of the range's ORIGINAL bytes — text inserted exactly at
/// either edge stays outside. This is the right shape for **identity**
/// matching (the block index): an insertion at a block's start that forms
/// its own new block must not let the old block's mapped span claim it by
/// overlap. A fully deleted range collapses to an empty range at the
/// deletion site (still meaningful as a point for identity matching).
pub fn map_range_shrink(range: &Range<usize>, batch: &[ByteSplice]) -> Range<usize> {
    let start = map_pos(range.start, batch, Bias::After);
    let end = map_pos(range.end, batch, Bias::Before).max(start);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(at: usize, delete: usize, insert: &str) -> ByteSplice {
        ByteSplice {
            at,
            delete,
            insert: insert.into(),
        }
    }

    #[test]
    fn before_bias_insertion_at_point_does_not_move() {
        assert_eq!(map_pos(5, &[sp(5, 0, "xyz")], Bias::Before), 5);
    }

    #[test]
    fn after_bias_insertion_at_point_moves_past_it() {
        assert_eq!(map_pos(5, &[sp(5, 0, "xyz")], Bias::After), 8);
    }

    #[test]
    fn both_biases_shift_equally_for_insertions_strictly_before() {
        assert_eq!(map_pos(5, &[sp(0, 0, "ab")], Bias::Before), 7);
        assert_eq!(map_pos(5, &[sp(0, 0, "ab")], Bias::After), 7);
    }

    #[test]
    fn position_inside_deleted_range_collapses_regardless_of_bias() {
        assert_eq!(map_pos(5, &[sp(2, 6, "")], Bias::Before), 2);
        assert_eq!(map_pos(5, &[sp(2, 6, "")], Bias::After), 2);
    }

    #[test]
    fn multiple_same_position_insertions_all_absorbed_after_bias() {
        let batch = [sp(5, 0, "a"), sp(5, 0, "bb")];
        assert_eq!(map_pos(5, &batch, Bias::After), 8); // 5 + 1 + 2
        assert_eq!(map_pos(5, &batch, Bias::Before), 5);
    }

    #[test]
    fn map_range_absorbs_insertions_at_both_edges() {
        let r = map_range(&(5..8), &[sp(5, 0, "AB"), sp(8, 0, "CD")]);
        assert_eq!(r, 5..12); // "AB" absorbed at start, "CD" absorbed at end
    }

    #[test]
    fn map_range_shrinks_when_fully_deleted() {
        let r = map_range(&(5..8), &[sp(4, 6, "")]);
        assert_eq!(r, 4..4);
    }

    #[test]
    fn map_range_shrink_excludes_insertions_at_both_edges() {
        let r = map_range_shrink(&(5..8), &[sp(5, 0, "AB"), sp(8, 0, "CD")]);
        assert_eq!(r, 7..10); // shifted past "AB"; "CD" stays outside
    }

    #[test]
    fn map_range_shrink_collapses_to_deletion_site() {
        let r = map_range_shrink(&(5..8), &[sp(4, 6, "")]);
        assert_eq!(r, 4..4);
    }
}
