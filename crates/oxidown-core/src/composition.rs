//! IME composition session state.
//!
//! While a session is active (boundary contract, model rule 5):
//! * decoration output must be stable over the composition range — no new
//!   `conceal` spans inside it; conceal spans intersecting it are emitted as
//!   `mark:delim` (revealed) instead — see [`crate::decorations`];
//! * history coalescing is paused — see [`crate::history`].
//!
//! The tracked range lives in byte coordinates and is mapped through every
//! applied edit batch; IME-origin insertions touching the range grow it (the
//! contract's "may grow as composition updates arrive").

use crate::mapping::{self, Bias};
use crate::text::ByteSplice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Composition {
    /// Byte range of the composed region in the current document.
    pub start: usize,
    pub end: usize,
}

impl Composition {
    /// Map the range through an edit batch (`batch` is ascending,
    /// non-overlapping, in pre-edit byte coordinates). When `grow` is set
    /// (IME-origin edits), inserted regions touching the mapped range are
    /// unioned into it.
    pub fn map_through(&mut self, batch: &[ByteSplice], grow: bool) {
        let mut start = mapping::map_pos(self.start, batch, Bias::Before);
        let mut end = mapping::map_pos(self.end, batch, Bias::Before);
        if grow {
            let mut delta: isize = 0;
            for s in batch {
                let ins_start = usize::try_from(s.at as isize + delta).unwrap_or(0);
                let ins_end = ins_start + s.insert.len();
                if ins_start <= end && ins_end >= start {
                    start = start.min(ins_start);
                    end = end.max(ins_end);
                }
                delta += s.insert.len() as isize - s.delete as isize;
            }
        }
        self.start = start;
        self.end = end.max(start);
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
    fn insert_before_shifts() {
        let mut c = Composition { start: 5, end: 8 };
        c.map_through(&[sp(0, 0, "ab")], false);
        assert_eq!((c.start, c.end), (7, 10));
    }

    #[test]
    fn insert_at_end_grows_when_ime() {
        let mut c = Composition { start: 5, end: 8 };
        c.map_through(&[sp(8, 0, "xy")], true);
        assert_eq!((c.start, c.end), (5, 10));
    }

    #[test]
    fn insert_at_end_does_not_grow_otherwise() {
        let mut c = Composition { start: 5, end: 8 };
        c.map_through(&[sp(8, 0, "xy")], false);
        assert_eq!((c.start, c.end), (5, 8));
    }

    #[test]
    fn deletion_covering_range_collapses() {
        let mut c = Composition { start: 5, end: 8 };
        c.map_through(&[sp(4, 6, "")], false);
        assert_eq!((c.start, c.end), (4, 4));
    }

    #[test]
    fn replacement_inside_range() {
        let mut c = Composition { start: 5, end: 10 };
        c.map_through(&[sp(6, 2, "XYZ")], true);
        assert_eq!((c.start, c.end), (5, 11));
    }
}
