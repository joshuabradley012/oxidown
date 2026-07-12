//! Undo/redo as inverted-op stacks.
//!
//! Stack discipline makes coordinate mapping unnecessary for the normal
//! path: a unit's inverse splices are expressed against the document state
//! that existed right after that unit's edit — and because units above it
//! must be popped first, that state *is* the current document whenever the
//! unit reaches the top.
//!
//! Coalescing: a consecutive `user`/`ime` edit merges into the top undo unit
//! when (a) it is a single splice falling entirely within (or touching the
//! ends of) the region the unit's undo would remove — which covers typing
//! runs, insert-at-front, and backspace runs over just-typed text; this
//! region rule is deliberately broader than the contract's "touches the
//! previous edit's end position" wording, and the code's behavior is the
//! pinned one — and (b) it arrives within 500 ms of the unit's last absorbed
//! edit. `paste` (and undo/redo, `command`, `ai`) never coalesce;
//! multi-splice batches never coalesce. A coalesce that shrinks the unit's
//! inverse to a pure no-op (type a char, backspace it) drops the unit from
//! history entirely — undoing a nothing would burn a revision and eat the
//! keypress.
//!
//! Composition (boundary v0.1 clarification 1): `composition_begin` closes
//! any open group (history break, set by the editor); edits made while a
//! session is active coalesce regardless of the 500 ms window; and
//! `composition_end` closes the group — a composition session is exactly
//! one undo unit.
//!
//! ## Stream units (M1, plan.md §5.9 + boundary v0.2 clarification 2)
//!
//! An open AI stream owns exactly ONE undo unit no matter how many appends
//! arrive, while interleaved user edits still get their own correctly
//! ordered units. This is the one place stack discipline alone isn't
//! enough: an append may need to merge into a unit that is **not** on top
//! (user edits recorded since sit above it). [`History::record_stream_append`]
//! keeps every unit's frame invariant intact by cascading the append
//! insertion down through the stack — see its docs for the exact algebra.
//! The result: undoing after close reverts the entire stream in one step,
//! deleting exactly the streamed spans (mapped through whatever else
//! happened), without touching user edits made during the stream.

use crate::mapping::{self, Bias};
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
    /// Set when this unit is an AI stream's single unit — appends of that
    /// stream merge here instead of pushing new units. `pub(crate)` so
    /// `Editor::undo`/`redo` can carry it across the undo<->redo round trip
    /// (see `push_redo`/`push_undo_unit`) — a stream session must stay
    /// exactly ONE undo unit even after being undone and redone (boundary
    /// v0.2: "An ENTIRE stream session (open→close) is exactly ONE undo
    /// unit," no undo/redo carve-out).
    pub(crate) stream_id: Option<u64>,
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
    /// * `composing` — whether a composition session was active; a session's
    ///   edits coalesce regardless of the 500 ms window (clarification 1).
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
        let eligible = matches!(origin, EditOrigin::User | EditOrigin::Ime);

        if eligible && !self.break_next {
            if let Some((at, delete, insert_len)) = forward_single {
                if self.try_coalesce(at, delete, insert_len, now_ms, composing) {
                    return;
                }
            }
        }

        self.undo.push(UndoUnit {
            inverse,
            coalesce_last_ms: (eligible && forward_single.is_some()).then_some(now_ms),
            stream_id: None,
        });
        self.break_next = false;
    }

    /// Record a stream append: a pure insertion of `len` bytes at `at`
    /// (post-append current-doc byte coordinates — for a pure insertion the
    /// pre- and post-edit start position coincide).
    ///
    /// If the stream's unit is still in the undo stack, the append merges
    /// into it; otherwise (first append, or the unit was undone/redone away)
    /// a fresh unit tagged with `stream_id` is pushed.
    ///
    /// Merging into a non-top unit must preserve the stack's frame
    /// invariant (each unit's inverse valid in the doc obtained by applying
    /// the inverses above it). The insertion is therefore *cascaded down*:
    /// for each unit above the stream unit, from top to bottom, (a) the
    /// insertion position is translated into the next-deeper frame by
    /// mapping it through that unit's (old) inverse, and (b) that unit's
    /// inverse is rewritten as if the insertion had always been present in
    /// its frame ([`map_batch_through_insertion`] — positions at/after the
    /// insertion shift; a delete-span strictly containing it splits around
    /// it so no unit ever deletes streamed text). At the stream unit's own
    /// frame the same rewrite runs, and then the chunk's inverse (delete
    /// `[at', at'+len)`) merges into its splice list.
    pub fn record_stream_append(&mut self, stream_id: u64, at: usize, len: usize) {
        self.redo.clear();
        let Some(idx) = self
            .undo
            .iter()
            .rposition(|u| u.stream_id == Some(stream_id))
        else {
            self.undo.push(UndoUnit {
                inverse: vec![ByteSplice {
                    at,
                    delete: len,
                    insert: String::new(),
                }],
                coalesce_last_ms: None,
                stream_id: Some(stream_id),
            });
            return;
        };
        let mut pos = at;
        for k in (idx + 1..self.undo.len()).rev() {
            let old_inverse = std::mem::take(&mut self.undo[k].inverse);
            // Frame translation uses the OLD inverse (before rewriting).
            // Bias::Before: if this unit's undo restores text exactly at the
            // insertion point, the streamed chunk stays before it.
            let deeper_pos = mapping::map_pos(pos, &old_inverse, Bias::Before);
            self.undo[k].inverse = map_batch_through_insertion(old_inverse, pos, len);
            pos = deeper_pos;
        }
        let unit = &mut self.undo[idx];
        unit.inverse = map_batch_through_insertion(std::mem::take(&mut unit.inverse), pos, len);
        merge_delete(&mut unit.inverse, pos, len);
    }

    fn try_coalesce(
        &mut self,
        at: usize,
        delete: usize,
        insert_len: usize,
        now_ms: f64,
        composing: bool,
    ) -> bool {
        let Some(top) = self.undo.last_mut() else {
            return false;
        };
        let Some(last_ms) = top.coalesce_last_ms else {
            return false;
        };
        // While a composition session is active the 500 ms window does not
        // break the group (clarification 1) — the session's boundaries are
        // the breaks, set by composition_begin/composition_end.
        if (!composing && now_ms - last_ms > COALESCE_WINDOW_MS) || top.inverse.len() != 1 {
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
            if region.delete == 0 && region.insert.is_empty() {
                // The unit's inverse is now a pure no-op (e.g. a char typed
                // and backspaced away): a later undo would apply nothing yet
                // bump the revision and consume the keypress. Drop the unit.
                self.undo.pop();
            } else {
                top.coalesce_last_ms = Some(now_ms);
            }
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

    /// Push a unit popped from the undo stack onto the redo stack (`undo()`'s
    /// counterpart of [`Self::push_undo_unit`]). `stream_id` must be the
    /// popped unit's own (see `Editor::undo`) — preserving it across the
    /// round trip is what makes undo→redo of an open stream's unit still
    /// count as the SAME stream unit, so later appends keep merging into it
    /// instead of starting a second unit (boundary v0.2: one stream session
    /// is exactly one undo unit, with no undo/redo carve-out).
    pub fn push_redo(&mut self, inverse: Vec<ByteSplice>, stream_id: Option<u64>) {
        self.redo.push(UndoUnit {
            inverse,
            coalesce_last_ms: None,
            stream_id,
        });
    }

    /// Push a unit produced by `redo()` back onto the undo stack. Never
    /// coalescible: redo restores a completed unit. `stream_id` must be the
    /// redone unit's own (see `Editor::redo`) — same reasoning as
    /// `push_redo`. Note this only re-establishes the unit as the stream's
    /// MERGE TARGET if the stream is still open and appends again; undoing
    /// mid-stream and then making a NEW append WITHOUT first redoing still
    /// starts a fresh unit (the redo stack — including this unit, before it
    /// is ever pushed here — is cleared by that new edit per the normal
    /// "any edit clears redo" rule), which is correct: the guarantee is one
    /// unit per stream session, not immunity from the user unwinding the
    /// stream mid-flight and then diverging.
    pub fn push_undo_unit(&mut self, inverse: Vec<ByteSplice>, stream_id: Option<u64>) {
        self.undo.push(UndoUnit {
            inverse,
            coalesce_last_ms: None,
            stream_id,
        });
    }
}

/// Rewrite an inverse batch as if a pure insertion of `len` bytes at `at`
/// (same frame as the batch) had always been present:
///
/// * splices starting at or after `at` shift right by `len` (an insertion
///   exactly at a splice's start is *not* owned by that splice — streamed
///   text must never be deleted by a non-stream unit);
/// * a delete-span strictly containing `at` splits around the inserted
///   region: `[s, at) ++ [at+len, e+len)`, with the splice's restore text
///   staying with the first piece (sound: the streamed chunk survives this
///   unit's undo and the restored text lands immediately before it — the
///   stream's own unit deletes the chunk later).
fn map_batch_through_insertion(batch: Vec<ByteSplice>, at: usize, len: usize) -> Vec<ByteSplice> {
    let mut out = Vec::with_capacity(batch.len());
    for s in batch {
        let end = s.at + s.delete;
        if at <= s.at {
            out.push(ByteSplice {
                at: s.at + len,
                delete: s.delete,
                insert: s.insert,
            });
        } else if at >= end {
            out.push(s);
        } else {
            out.push(ByteSplice {
                at: s.at,
                delete: at - s.at,
                insert: s.insert,
            });
            out.push(ByteSplice {
                at: at + len,
                delete: end - at,
                insert: String::new(),
            });
        }
    }
    out
}

/// Insert a pure delete `[at, at+len)` into an ascending, non-overlapping
/// inverse batch, coalescing with pure-delete neighbors it touches. The
/// caller guarantees (via [`map_batch_through_insertion`]) that no existing
/// splice overlaps the new range.
fn merge_delete(batch: &mut Vec<ByteSplice>, at: usize, len: usize) {
    let idx = batch.partition_point(|s| s.at + s.delete < at);
    // Try extending the previous/current splice whose delete-range ends
    // exactly at `at` (pure deletes only — a splice with restore text keeps
    // its own identity).
    if idx < batch.len() && batch[idx].at + batch[idx].delete == at && batch[idx].insert.is_empty()
    {
        batch[idx].delete += len;
        try_absorb_next(batch, idx);
        return;
    }
    let insert_at = batch.partition_point(|s| s.at < at);
    batch.insert(
        insert_at,
        ByteSplice {
            at,
            delete: len,
            insert: String::new(),
        },
    );
    try_absorb_next(batch, insert_at);
}

/// After growing `batch[idx]`, absorb the next splice if it is a pure delete
/// starting exactly where `batch[idx]`'s delete-range now ends.
fn try_absorb_next(batch: &mut Vec<ByteSplice>, idx: usize) {
    if idx + 1 < batch.len() {
        let end = batch[idx].at + batch[idx].delete;
        if batch[idx + 1].at == end && batch[idx + 1].insert.is_empty() {
            batch[idx].delete += batch[idx + 1].delete;
            batch.remove(idx + 1);
        }
    }
}
