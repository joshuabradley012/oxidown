//! Public anchors (plan.md §5.3, boundary v0.2): opaque ids for positions
//! that survive arbitrary edits. Internally an anchor is a byte position
//! plus a [`Bias`], mapped through every applied splice batch by
//! [`AnchorSet::map_through`] — the same shared machinery
//! ([`crate::mapping`]) the composition range and block index use.
//!
//! Contract semantics:
//! * `bias: "before"` stays put when an insertion lands exactly on it;
//!   `"after"` moves with the insertion.
//! * Deleting the anchored text collapses the anchor to the deletion site;
//!   it does **not** become unresolvable in M1.
//! * `resolve` returns `None` only for ids that were never created, were
//!   dropped, or belong to a document that was since replaced by `load`
//!   (the editor clears all anchors on `load` — a fresh document invalidates
//!   every position expressed against the old one).

use std::collections::HashMap;

use crate::mapping::{self, Bias};
use crate::text::ByteSplice;

#[derive(Debug, Default)]
pub struct AnchorSet {
    next_id: u64,
    /// id → (byte position, bias). A HashMap (not a position-sorted
    /// structure) because M1's anchor counts are small (cursors, stream
    /// insertion points, a handful of app bookmarks) and `map_through` is
    /// O(anchors × batch) either way.
    anchors: HashMap<u64, (usize, Bias)>,
}

impl AnchorSet {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            anchors: HashMap::new(),
        }
    }

    pub fn create(&mut self, byte_pos: usize, bias: Bias) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.anchors.insert(id, (byte_pos, bias));
        id
    }

    pub fn resolve(&self, id: u64) -> Option<usize> {
        self.anchors.get(&id).map(|&(pos, _)| pos)
    }

    pub fn remove(&mut self, id: u64) {
        self.anchors.remove(&id);
    }

    /// Drop every anchor but keep ids monotonic (never reused within an
    /// editor's lifetime — same discipline as the op log's counters).
    pub fn clear(&mut self) {
        self.anchors.clear();
    }

    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    /// Map every anchor through an applied splice batch (ascending,
    /// non-overlapping, pre-edit byte coordinates).
    pub fn map_through(&mut self, batch: &[ByteSplice]) {
        for (pos, bias) in self.anchors.values_mut() {
            *pos = mapping::map_pos(*pos, batch, *bias);
        }
    }
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
    fn create_resolve_drop() {
        let mut set = AnchorSet::new();
        let a = set.create(5, Bias::Before);
        let b = set.create(9, Bias::After);
        assert_ne!(a, b);
        assert_eq!(set.resolve(a), Some(5));
        assert_eq!(set.resolve(b), Some(9));
        set.remove(a);
        assert_eq!(set.resolve(a), None);
        assert_eq!(set.resolve(b), Some(9));
        assert_eq!(set.resolve(999), None);
    }

    #[test]
    fn bias_at_exact_insertion_point() {
        let mut set = AnchorSet::new();
        let before = set.create(5, Bias::Before);
        let after = set.create(5, Bias::After);
        set.map_through(&[sp(5, 0, "xyz")]);
        assert_eq!(set.resolve(before), Some(5), "before stays put");
        assert_eq!(set.resolve(after), Some(8), "after moves with the insertion");
    }

    #[test]
    fn deletion_collapses_to_site() {
        let mut set = AnchorSet::new();
        let a = set.create(5, Bias::Before);
        let b = set.create(6, Bias::After);
        set.map_through(&[sp(3, 6, "")]);
        assert_eq!(set.resolve(a), Some(3));
        assert_eq!(set.resolve(b), Some(3));
    }

    #[test]
    fn clear_keeps_ids_monotonic() {
        let mut set = AnchorSet::new();
        let a = set.create(0, Bias::Before);
        set.clear();
        let b = set.create(0, Bias::Before);
        assert!(b > a, "ids never reused after clear");
        assert_eq!(set.resolve(a), None);
    }
}
