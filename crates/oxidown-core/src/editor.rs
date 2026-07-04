//! The top-level `Editor`: the whole boundary-v0 contract behind one struct.
//! All public positions are UTF-16 code units; conversion to internal UTF-8
//! byte offsets happens exactly here (and nowhere leaks back out).

use crate::composition::Composition;
use crate::decorations::{self, Decoration};
use crate::error::CoreError;
use crate::history::History;
use crate::oplog::{EditOrigin, OpLog};
use crate::parser::{self, Node};
use crate::text::{ByteSplice, TextBuffer};

/// Boundary splice: positions in UTF-16 code units, in original-doc
/// coordinates for the batch it belongs to (CM6 ChangeSet semantics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub at: usize,
    pub delete: usize,
    pub insert: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    pub anchor: usize,
    pub head: usize,
}

/// Result of `undo`/`redo`: splices in current-doc (pre-application) UTF-16
/// coordinates for the view to apply verbatim, plus the resulting revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryResult {
    pub revision: u64,
    pub splices: Vec<Splice>,
}

#[derive(Debug)]
pub struct Editor {
    text: TextBuffer,
    /// Cached parse overlay; rebuilt on every text change, only *filtered*
    /// by `decorations()`.
    overlay: Vec<Node>,
    oplog: OpLog,
    history: History,
    composition: Option<Composition>,
    revision: u64,
}

impl Editor {
    pub fn new(replica_id: u16) -> Self {
        Self {
            text: TextBuffer::new(),
            overlay: Vec::new(),
            oplog: OpLog::new(replica_id),
            history: History::new(),
            composition: None,
            revision: 0,
        }
    }

    /// Create/replace the document. Clears history, oplog contents and any
    /// composition session. Returns the new revision (1 on a fresh editor:
    /// "revision 0's successor").
    pub fn load(&mut self, text: &str) -> u64 {
        self.text = TextBuffer::from_text(text);
        self.history.clear();
        self.oplog.clear();
        self.composition = None;
        self.reparse();
        self.revision += 1;
        self.revision
    }

    /// Apply an edit batch. `splices` must be ascending, non-overlapping, in
    /// original-doc UTF-16 coordinates against `base_revision` (which must be
    /// current). `now_ms` is an injected wall-clock timestamp (the core never
    /// reads clocks — `std::time` panics on wasm32); it only drives undo
    /// coalescing.
    pub fn apply_edit(
        &mut self,
        base_revision: u64,
        splices: &[Splice],
        origin: EditOrigin,
        now_ms: f64,
    ) -> Result<u64, CoreError> {
        if base_revision != self.revision {
            return Err(CoreError::StaleRevision {
                current: self.revision,
                requested: base_revision,
            });
        }
        let len16 = self.text.len_utf16();
        let mut batch: Vec<ByteSplice> = Vec::with_capacity(splices.len());
        let mut prev_end = 0usize;
        for (i, s) in splices.iter().enumerate() {
            let end = s.at.checked_add(s.delete).ok_or_else(|| CoreError::InvalidSplice {
                index: i,
                detail: "position overflow".into(),
            })?;
            if i > 0 && s.at < prev_end {
                return Err(CoreError::InvalidSplice {
                    index: i,
                    detail: format!(
                        "splices must be ascending and non-overlapping (at {} < previous end {})",
                        s.at, prev_end
                    ),
                });
            }
            if end > len16 {
                return Err(CoreError::OutOfBounds { pos: end, len: len16 });
            }
            let from_b = self.text.utf16_to_byte(s.at)?;
            let to_b = self.text.utf16_to_byte(end)?;
            prev_end = end;
            if to_b == from_b && s.insert.is_empty() {
                continue; // no-op splice
            }
            batch.push(ByteSplice {
                at: from_b,
                delete: to_b - from_b,
                insert: s.insert.clone(),
            });
        }
        if batch.is_empty() {
            return Ok(self.revision); // nothing changed; revision unchanged
        }
        let forward_single = (batch.len() == 1)
            .then(|| (batch[0].at, batch[0].delete, batch[0].insert.len()));
        let composing = self.composition.is_some();
        let inverse = self.apply_bytes(&batch, origin);
        self.history
            .record_edit(inverse, origin, now_ms, forward_single, composing);
        self.reparse();
        self.revision += 1;
        Ok(self.revision)
    }

    /// Core-driven undo. Returns splices in current-doc (pre-application)
    /// UTF-16 coordinates, or `None` if the stack is empty.
    pub fn undo(&mut self) -> Option<HistoryResult> {
        let unit = self.history.pop_undo()?;
        let splices = self.to_utf16_splices(&unit.inverse);
        let redo_inverse = self.apply_bytes(&unit.inverse, EditOrigin::Undo);
        self.history.push_redo(redo_inverse);
        self.history.set_break();
        self.reparse();
        self.revision += 1;
        Some(HistoryResult {
            revision: self.revision,
            splices,
        })
    }

    /// Core-driven redo; counterpart of [`Editor::undo`].
    pub fn redo(&mut self) -> Option<HistoryResult> {
        let unit = self.history.pop_redo()?;
        let splices = self.to_utf16_splices(&unit.inverse);
        let undo_inverse = self.apply_bytes(&unit.inverse, EditOrigin::Redo);
        self.history.push_undo_unit(undo_inverse);
        self.history.set_break();
        self.reparse();
        self.revision += 1;
        Some(HistoryResult {
            revision: self.revision,
            splices,
        })
    }

    /// Decorations for the viewport `[from, to)` (UTF-16), computed against
    /// `revision` (must be current) and the given selections. Filters the
    /// cached overlay; never reparses.
    pub fn decorations(
        &self,
        revision: u64,
        from: usize,
        to: usize,
        selections: &[SelectionRange],
    ) -> Result<Vec<Decoration>, CoreError> {
        if revision != self.revision {
            return Err(CoreError::StaleRevision {
                current: self.revision,
                requested: revision,
            });
        }
        if from > to {
            return Err(CoreError::InvalidRange { from, to });
        }
        // Query positions snap outward to code-point boundaries rather than
        // erroring: a viewport edge or selection endpoint landing inside a
        // surrogate pair is a range-filter input, not a text mutation.
        let from_b = self.text.utf16_to_byte_floor(from)?;
        let to_b = self.text.utf16_to_byte_ceil(to)?;
        let mut sels_b = Vec::with_capacity(selections.len());
        for sel in selections {
            let lo = sel.anchor.min(sel.head);
            let hi = sel.anchor.max(sel.head);
            sels_b.push((
                self.text.utf16_to_byte_floor(lo)?,
                self.text.utf16_to_byte_ceil(hi)?,
            ));
        }
        Ok(decorations::compute(
            &self.overlay,
            &self.text,
            from_b..to_b,
            &sels_b,
            self.composition.as_ref(),
        ))
    }

    /// Begin an IME composition session over `[from, to]` (UTF-16,
    /// current-doc). Re-invoking while active replaces the tracked range.
    pub fn composition_begin(&mut self, from: usize, to: usize) -> Result<(), CoreError> {
        if from > to {
            return Err(CoreError::InvalidRange { from, to });
        }
        let start = self.text.utf16_to_byte_floor(from)?;
        let end = self.text.utf16_to_byte_ceil(to)?;
        self.composition = Some(Composition { start, end });
        Ok(())
    }

    pub fn composition_end(&mut self) {
        self.composition = None;
    }

    pub fn composition_active(&self) -> bool {
        self.composition.is_some()
    }

    pub fn get_text(&self) -> String {
        self.text.text()
    }

    /// Document length in UTF-16 code units.
    pub fn doc_len_utf16(&self) -> usize {
        self.text.len_utf16()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Read access to the append-only op log (debug/verification/tests).
    pub fn oplog(&self) -> &OpLog {
        &self.oplog
    }

    /// `(undo_depth, redo_depth)` — for tests and debugging.
    pub fn history_depths(&self) -> (usize, usize) {
        (self.history.undo_depth(), self.history.redo_depth())
    }

    // ---- internals ----------------------------------------------------

    /// Apply a validated byte-splice batch (ascending, non-overlapping,
    /// pre-application coordinates): mutate the rope, append ops, map the
    /// composition range, and return the batch's inverse in post-application
    /// coordinates.
    fn apply_bytes(&mut self, batch: &[ByteSplice], origin: EditOrigin) -> Vec<ByteSplice> {
        // Capture deleted text first: all splices reference the pre-edit doc.
        let deleted: Vec<String> = batch
            .iter()
            .map(|s| self.text.byte_slice_to_string(s.at..s.end()))
            .collect();
        // Apply back-to-front so earlier positions stay valid.
        for s in batch.iter().rev() {
            self.text.replace_bytes(s.at..s.end(), &s.insert);
        }
        // Inverse, in post-edit coordinates.
        let mut inverse = Vec::with_capacity(batch.len());
        let mut delta: isize = 0;
        for (s, deleted_text) in batch.iter().zip(deleted) {
            let at_post = usize::try_from(s.at as isize + delta).unwrap_or(0);
            inverse.push(ByteSplice {
                at: at_post,
                delete: s.insert.len(),
                insert: deleted_text,
            });
            delta += s.insert.len() as isize - s.delete as isize;
        }
        for s in batch {
            self.oplog.append(origin, s.clone());
        }
        if let Some(comp) = self.composition.as_mut() {
            comp.map_through(batch, origin == EditOrigin::Ime);
        }
        inverse
    }

    /// Convert byte splices (valid against the *current* doc) to UTF-16.
    fn to_utf16_splices(&self, batch: &[ByteSplice]) -> Vec<Splice> {
        batch
            .iter()
            .map(|s| {
                let at = self.text.byte_to_utf16(s.at);
                let end = self.text.byte_to_utf16(s.end());
                Splice {
                    at,
                    delete: end - at,
                    insert: s.insert.clone(),
                }
            })
            .collect()
    }

    fn reparse(&mut self) {
        self.overlay = parser::parse(&self.text.text());
    }
}
