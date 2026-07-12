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
//!
//! The set also holds the core's own INTERNAL anchors (stream insertion
//! points). They share the id counter but are invisible to the public API:
//! `resolve`/`remove` treat an internal id exactly like an unknown one, so
//! no id a caller can pass over the boundary ever disturbs core-owned state.

use std::collections::HashMap;

use crate::mapping::{self, Bias};
use crate::text::ByteSplice;

#[derive(Debug, Default)]
pub struct AnchorSet {
    next_id: u64,
    /// id → (byte position, bias, internal). A HashMap (not a position-sorted
    /// structure) because M1's anchor counts are small (cursors, stream
    /// insertion points, a handful of app bookmarks) and `map_through` is
    /// O(anchors × batch) either way.
    anchors: HashMap<u64, (usize, Bias, bool)>,
}

impl AnchorSet {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            anchors: HashMap::new(),
        }
    }

    pub fn create(&mut self, byte_pos: usize, bias: Bias) -> u64 {
        self.insert(byte_pos, bias, false)
    }

    /// Create a core-owned anchor (stream insertion points). Same id
    /// counter, but the public `resolve`/`remove` treat the id as unknown.
    pub fn create_internal(&mut self, byte_pos: usize, bias: Bias) -> u64 {
        self.insert(byte_pos, bias, true)
    }

    fn insert(&mut self, byte_pos: usize, bias: Bias, internal: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.anchors.insert(id, (byte_pos, bias, internal));
        id
    }

    /// Public resolution: internal ids read as unknown (`None`).
    pub fn resolve(&self, id: u64) -> Option<usize> {
        self.anchors
            .get(&id)
            .and_then(|&(pos, _, internal)| (!internal).then_some(pos))
    }

    /// Core-internal resolution: any live id, internal or public.
    pub fn resolve_internal(&self, id: u64) -> Option<usize> {
        self.anchors.get(&id).map(|&(pos, _, _)| pos)
    }

    /// Public removal: internal ids are untouchable (no-op, like unknown).
    pub fn remove(&mut self, id: u64) {
        if let Some(&(_, _, internal)) = self.anchors.get(&id) {
            if !internal {
                self.anchors.remove(&id);
            }
        }
    }

    /// Core-internal removal: any id (used by `stream_close`).
    pub fn remove_internal(&mut self, id: u64) {
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
        for (pos, bias, _) in self.anchors.values_mut() {
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
    fn internal_anchors_invisible_to_public_api() {
        let mut set = AnchorSet::new();
        let internal = set.create_internal(5, Bias::After);
        assert_eq!(set.resolve(internal), None, "public resolve: unknown");
        set.remove(internal); // public remove: no-op on internal ids
        assert_eq!(set.resolve_internal(internal), Some(5), "still alive");
        set.map_through(&[sp(0, 0, "ab")]);
        assert_eq!(set.resolve_internal(internal), Some(7), "still mapped");
        set.remove_internal(internal);
        assert_eq!(set.resolve_internal(internal), None);
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
