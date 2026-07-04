//! Undo/redo as inverted-op stacks.
//!
//! Stack discipline makes coordinate mapping unnecessary: a unit's inverse
//! splices are expressed against the document state that existed right after
//! that unit's edit — and because units above it must be popped first, that
//! state *is* the current document whenever the unit reaches the top.
//!
//! Coalescing (contract): consecutive `user`/`ime` edits within 500 ms that
//! are positionally adjacent merge into one undo unit; `paste` (and undo/redo)
//! never coalesce; coalescing pauses while a composition session is active.
//! "Positionally adjacent" is implemented as: the new single splice falls
//! entirely within (or touches the ends of) the region the unit's undo would
//! remove — which covers typing runs, insert-at-front, and backspace runs
//! over just-typed text.

use crate::oplog::EditOrigin;
use crate::text::ByteSplice;

pub const COALESCE_WINDOW_MS: f64 = 500.0;

#[derive(Debug)]
pub struct UndoUnit {
    /// Inverse splices in the coordinates of the doc state where this unit is
    /// on top of its stack: ascending, non-overlapping, byte positions.
    pub inverse: Vec<ByteSplice>,
    /// Timestamp of the unit's most recent absorbed edit, present only while
    /// the unit is still eligible to absorb more (single-splice user/ime edit
    /// made outside a composition session).
    coalesce_last_ms: Option<f64>,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<UndoUnit>,
    redo: Vec<UndoUnit>,
    /// Set after undo/redo so the next edit starts a fresh unit even if it
    /// would otherwise coalesce with the unit now on top.
    break_next: bool,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.break_next = false;
    }

    pub fn set_break(&mut self) {
        self.break_next = true;
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// Record a freshly applied (non-undo/redo path) edit batch.
    ///
    /// * `inverse` — the batch's inverse splices in post-edit coordinates.
    /// * `forward_single` — `(at, delete, insert_len)` of the batch's only
    ///   splice in pre-edit byte coordinates, when the batch has exactly one.
    /// * `composing` — whether a composition session was active.
    ///
    /// Any new edit clears the redo stack.
    pub fn record_edit(
        &mut self,
        inverse: Vec<ByteSplice>,
        origin: EditOrigin,
        now_ms: f64,
        forward_single: Option<(usize, usize, usize)>,
        composing: bool,
    ) {
        self.redo.clear();
        let eligible = matches!(origin, EditOrigin::User | EditOrigin::Ime) && !composing;

        if eligible && !self.break_next {
            if let Some((at, delete, insert_len)) = forward_single {
                if self.try_coalesce(at, delete, insert_len, now_ms) {
                    return;
                }
            }
        }

        self.undo.push(UndoUnit {
            inverse,
            coalesce_last_ms: (eligible && forward_single.is_some()).then_some(now_ms),
        });
        self.break_next = false;
    }

    fn try_coalesce(&mut self, at: usize, delete: usize, insert_len: usize, now_ms: f64) -> bool {
        let Some(top) = self.undo.last_mut() else {
            return false;
        };
        let Some(last_ms) = top.coalesce_last_ms else {
            return false;
        };
        if now_ms - last_ms > COALESCE_WINDOW_MS || top.inverse.len() != 1 {
            return false;
        }
        // The unit's undo would replace current-doc region [c_start, c_end)
        // with the original text. An edit contained in that closed region
        // only touches unit-owned text, so the stored original text and
        // anchor stay valid; only the region length changes.
        let region = &mut top.inverse[0];
        let c_start = region.at;
        let c_end = region.at + region.delete;
        if c_start <= at && at + delete <= c_end {
            region.delete = region.delete - delete + insert_len;
            top.coalesce_last_ms = Some(now_ms);
            true
        } else {
            false
        }
    }

    pub fn pop_undo(&mut self) -> Option<UndoUnit> {
        self.undo.pop()
    }

    pub fn pop_redo(&mut self) -> Option<UndoUnit> {
        self.redo.pop()
    }

    pub fn push_redo(&mut self, inverse: Vec<ByteSplice>) {
        self.redo.push(UndoUnit {
            inverse,
            coalesce_last_ms: None,
        });
    }

    /// Push a unit produced by `redo()` back onto the undo stack. Never
    /// coalescible: redo restores a completed unit.
    pub fn push_undo_unit(&mut self, inverse: Vec<ByteSplice>) {
        self.undo.push(UndoUnit {
            inverse,
            coalesce_last_ms: None,
        });
    }
}
