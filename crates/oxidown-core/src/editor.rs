//! The top-level `Editor`: the whole boundary contract (v0/v0.1 + v0.2)
//! behind one struct. All public positions are UTF-16 code units; conversion
//! to internal UTF-8 byte offsets happens exactly here (and nowhere leaks
//! back out).

use std::collections::HashMap;

use crate::anchor::AnchorSet;
use crate::block_index::BlockIndex;
use crate::commands::{self, Command, CommandPlan};
use crate::composition::Composition;
use crate::decorations::{self, Decoration};
use crate::error::CoreError;
use crate::history::History;
use crate::mapping::Bias;
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

/// The one shape every core-driven change returns (boundary v0.2:
/// `undo`/`redo`, `command`, `streamAppend`): splices in current-doc
/// (pre-application) UTF-16 coordinates for the view to apply verbatim
/// under its skip annotation, the resulting revision, and optional cursor
/// placement (post-application coordinates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreChange {
    pub revision: u64,
    pub splices: Vec<Splice>,
    pub selection: Option<SelectionRange>,
}

#[derive(Debug)]
struct StreamState {
    /// Internal after-bias anchor at the stream's insertion point.
    anchor: u64,
}

#[derive(Debug)]
pub struct Editor {
    text: TextBuffer,
    /// Cached parse overlay; rebuilt on every text change, only *filtered*
    /// by `decorations()`.
    overlay: Vec<Node>,
    /// Top-level block index with sticky IDs (plan.md §5.3). Internal only
    /// in M1 — not yet exposed over the boundary; consumed by streaming's
    /// fast path and, later, sync.
    block_index: BlockIndex,
    /// Public anchors plus streaming's internal insertion anchors.
    anchors: AnchorSet,
    /// Open AI streams by id.
    streams: HashMap<u64, StreamState>,
    next_stream_id: u64,
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
            block_index: BlockIndex::new(replica_id),
            anchors: AnchorSet::new(),
            streams: HashMap::new(),
            next_stream_id: 1,
            oplog: OpLog::new(replica_id),
            history: History::new(),
            composition: None,
            revision: 0,
        }
    }

    /// Create/replace the document. Clears history, oplog contents, any
    /// composition session, all anchors (positions against the old document
    /// are meaningless in the new one — `resolveAnchor` returns null for
    /// them afterwards), and closes all open streams. Returns the new
    /// revision (1 on a fresh editor: "revision 0's successor").
    pub fn load(&mut self, text: &str) -> u64 {
        self.text = TextBuffer::from_text(text);
        self.history.clear();
        self.oplog.clear();
        self.composition = None;
        self.anchors.clear();
        self.streams.clear();
        self.block_index.clear();
        self.reparse_with(&[]);
        self.revision += 1;
        self.revision
    }

    /// Read access to the block index (debug/verification/tests; not yet a
    /// boundary API — see the module docs on `block_index`).
    pub fn block_index(&self) -> &BlockIndex {
        &self.block_index
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
        self.reparse_with(&batch);
        self.revision += 1;
        Ok(self.revision)
    }

    /// Core-driven undo. Returns splices in current-doc (pre-application)
    /// UTF-16 coordinates plus cursor placement (v0.2: the end of the last
    /// restored splice), or `None` if the stack is empty.
    pub fn undo(&mut self) -> Option<CoreChange> {
        let unit = self.history.pop_undo()?;
        let splices = self.to_utf16_splices(&unit.inverse);
        let redo_inverse = self.apply_bytes(&unit.inverse, EditOrigin::Undo);
        let selection = self.change_cursor(&redo_inverse);
        self.history.push_redo(redo_inverse);
        self.history.set_break();
        self.reparse_with(&unit.inverse);
        self.revision += 1;
        Some(CoreChange {
            revision: self.revision,
            splices,
            selection,
        })
    }

    /// Core-driven redo; counterpart of [`Editor::undo`].
    pub fn redo(&mut self) -> Option<CoreChange> {
        let unit = self.history.pop_redo()?;
        let splices = self.to_utf16_splices(&unit.inverse);
        let undo_inverse = self.apply_bytes(&unit.inverse, EditOrigin::Redo);
        let selection = self.change_cursor(&undo_inverse);
        self.history.push_undo_unit(undo_inverse);
        self.history.set_break();
        self.reparse_with(&unit.inverse);
        self.revision += 1;
        Some(CoreChange {
            revision: self.revision,
            splices,
            selection,
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

    // ---- anchors (boundary v0.2) ---------------------------------------

    /// Create an anchor at `pos` (UTF-16). A position inside a surrogate
    /// pair snaps toward the anchor's bias (floor for `before`, ceil for
    /// `after`) — an anchor is a tracked query position, not a mutation.
    pub fn create_anchor(&mut self, pos: usize, bias: Bias) -> Result<u64, CoreError> {
        let byte = match bias {
            Bias::Before => self.text.utf16_to_byte_floor(pos)?,
            Bias::After => self.text.utf16_to_byte_ceil(pos)?,
        };
        Ok(self.anchors.create(byte, bias))
    }

    /// Current position of the anchor (UTF-16), or `None` for unknown /
    /// dropped ids (or anchors invalidated by a subsequent `load`).
    pub fn resolve_anchor(&self, id: u64) -> Option<usize> {
        self.anchors.resolve(id).map(|b| self.text.byte_to_utf16(b))
    }

    pub fn drop_anchor(&mut self, id: u64) {
        self.anchors.remove(id);
    }

    // ---- commands (boundary v0.2) ---------------------------------------

    /// Run a command against the overlay. Returns `Ok(None)` when the
    /// command doesn't apply at the target (per the contract), `Ok(Some)`
    /// with the applied change otherwise. Command edits enter the op log
    /// with origin `command` and form single, never-coalescing undo units.
    pub fn command(&mut self, cmd: Command) -> Result<Option<CoreChange>, CoreError> {
        let src = self.text.text();
        let plan = match cmd {
            Command::ToggleStrong { from, to }
            | Command::ToggleEm { from, to }
            | Command::ToggleStrike { from, to }
            | Command::ToggleCode { from, to } => {
                let kind = match cmd {
                    Command::ToggleStrong { .. } => commands::InlineKind::Strong,
                    Command::ToggleEm { .. } => commands::InlineKind::Em,
                    Command::ToggleStrike { .. } => commands::InlineKind::Strike,
                    _ => commands::InlineKind::Code,
                };
                let (lo, hi) = (from.min(to), from.max(to));
                let from_b = self.text.utf16_to_byte(lo)?;
                let to_b = self.text.utf16_to_byte(hi)?;
                commands::toggle_inline(&self.overlay, &src, kind, from_b, to_b)
            }
            Command::SetHeading { pos, level } => {
                if level > 6 {
                    return Err(CoreError::InvalidRange {
                        from: level as usize,
                        to: 6,
                    });
                }
                let pos_b = self.text.utf16_to_byte_floor(pos)?;
                let line = self.text.line_range_at(pos_b);
                let block_kind = self
                    .block_index
                    .blocks()
                    .iter()
                    .find(|b| b.span.start <= line.start && line.start < b.span.end)
                    .map(|b| b.kind);
                commands::set_heading(&self.overlay, &src, block_kind, line, pos_b, level)
            }
            Command::ToggleTask { pos } => {
                let pos_b = self.text.utf16_to_byte_floor(pos)?;
                commands::toggle_task(&self.overlay, self.text.len_bytes(), pos_b)
            }
        };
        Ok(plan.map(|plan| self.apply_plan(plan)))
    }

    // ---- streaming (boundary v0.2, plan §5.9) ----------------------------

    /// Open a stream at `pos` (UTF-16, strict: an insertion point inside a
    /// surrogate pair would corrupt text and errors). The insertion point
    /// becomes an internal after-bias anchor that maps through all edits.
    pub fn stream_open(&mut self, pos: usize) -> Result<u64, CoreError> {
        let byte = self.text.utf16_to_byte(pos)?;
        let anchor = self.anchors.create(byte, Bias::After);
        let id = self.next_stream_id;
        self.next_stream_id += 1;
        self.streams.insert(id, StreamState { anchor });
        Ok(id)
    }

    /// Append a chunk at the stream's (mapped) insertion point. Origin `ai`;
    /// the entire stream session forms one undo unit (see
    /// [`crate::history::History::record_stream_append`]). Errors with
    /// `UnknownStream` on never-opened or closed ids.
    pub fn stream_append(&mut self, id: u64, chunk: &str) -> Result<CoreChange, CoreError> {
        let Some(stream) = self.streams.get(&id) else {
            return Err(CoreError::UnknownStream { id });
        };
        if chunk.is_empty() {
            return Ok(CoreChange {
                revision: self.revision,
                splices: Vec::new(),
                selection: None,
            });
        }
        let at_b = self
            .anchors
            .resolve(stream.anchor)
            .expect("internal stream anchor is never dropped while the stream is open");
        let at16 = self.text.byte_to_utf16(at_b);
        let splices16 = vec![Splice {
            at: at16,
            delete: 0,
            insert: chunk.to_string(),
        }];
        let batch = [ByteSplice {
            at: at_b,
            delete: 0,
            insert: chunk.to_string(),
        }];
        let fast_region = self.tail_fast_path_region(at_b);
        self.apply_bytes(&batch, EditOrigin::Ai);
        self.history.record_stream_append(id, at_b, chunk.len());
        match fast_region {
            Some(region_start) => self.reparse_tail(region_start, &batch),
            None => self.reparse_with(&batch),
        }
        self.revision += 1;
        Ok(CoreChange {
            revision: self.revision,
            splices: splices16,
            // No cursor placement: an AI stream must never yank the user's
            // cursor to its insertion point.
            selection: None,
        })
    }

    /// Close a stream. No-op on unknown/already-closed ids (per contract).
    pub fn stream_close(&mut self, id: u64) {
        if let Some(stream) = self.streams.remove(&id) {
            self.anchors.remove(stream.anchor);
        }
    }

    // ---- misc accessors --------------------------------------------------

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
    /// composition range and every anchor, and return the batch's inverse
    /// in post-application coordinates. Does NOT reparse — callers follow
    /// with `reparse_with`/`reparse_tail`.
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
        self.anchors.map_through(batch);
        inverse
    }

    /// Apply a planned command through the normal apply path (origin
    /// `command`, single undo unit) and package the [`CoreChange`].
    fn apply_plan(&mut self, plan: CommandPlan) -> CoreChange {
        let splices16 = self.to_utf16_splices(&plan.batch);
        let composing = self.composition.is_some();
        let inverse = self.apply_bytes(&plan.batch, EditOrigin::Command);
        self.history
            .record_edit(inverse, EditOrigin::Command, 0.0, None, composing);
        self.reparse_with(&plan.batch);
        self.revision += 1;
        let selection = plan.selection.map(|(anchor_b, head_b)| SelectionRange {
            anchor: self.text.byte_to_utf16(anchor_b),
            head: self.text.byte_to_utf16(head_b),
        });
        CoreChange {
            revision: self.revision,
            splices: splices16,
            selection,
        }
    }

    /// Cursor placement after applying a history unit: the end of the last
    /// splice's newly inserted text. `applied_inverse` is the inverse of the
    /// just-applied batch (post-application coordinates), whose last
    /// element's `at + delete` is exactly that end position.
    fn change_cursor(&self, applied_inverse: &[ByteSplice]) -> Option<SelectionRange> {
        applied_inverse.last().map(|s| {
            let cursor = self.text.byte_to_utf16(s.at + s.delete);
            SelectionRange {
                anchor: cursor,
                head: cursor,
            }
        })
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

    /// Full reparse: one pulldown pass producing overlay + block spans;
    /// block IDs re-matched through `batch` (empty on `load`).
    fn reparse_with(&mut self, batch: &[ByteSplice]) {
        let text = self.text.text();
        let parsed = parser::parse_document(&text);
        self.overlay = parsed.nodes;
        self.block_index.update(parsed.blocks, batch);
    }

    /// Streaming fast path precondition (boundary v0.2: "an append that only
    /// extends the open tail block must not force full-document work"): the
    /// insertion lands at/after the LAST top-level block's start, and that
    /// block starts at a line boundary (an indented code block's span starts
    /// mid-line and would not re-parse equivalently as a standalone slice).
    /// Returns the region start to re-parse from, or `None` → full reparse.
    ///
    /// Correctness note (documented Phase-A fast-path assumption): parsing
    /// the tail slice standalone is decoration-equivalent because top-level
    /// markdown blocks are prefix-independent at line granularity — the only
    /// whole-document couplings (link reference definitions, footnote
    /// definitions) affect constructs M1 does not decorate.
    fn tail_fast_path_region(&self, at_b: usize) -> Option<usize> {
        let last = self.block_index.blocks().last()?;
        if at_b < last.span.start {
            return None;
        }
        let region_start = last.span.start;
        if region_start == 0 || self.text.byte_at(region_start - 1) == Some(b'\n') {
            Some(region_start)
        } else {
            None
        }
    }

    /// Re-parse only `[region_start, end)` and splice the results into the
    /// overlay and block index. `batch` (the applied append) lands entirely
    /// at/after `region_start`, so everything before it is untouched.
    fn reparse_tail(&mut self, region_start: usize, batch: &[ByteSplice]) {
        self.overlay.retain(|n| n.extent.end <= region_start);
        let slice = self
            .text
            .byte_slice_to_string(region_start..self.text.len_bytes());
        let parsed = parser::parse_document(&slice);
        self.overlay.extend(parsed.nodes.into_iter().map(|mut n| {
            n.offset(region_start);
            n
        }));
        let tail_spans = parsed
            .blocks
            .into_iter()
            .map(|(k, r)| (k, r.start + region_start..r.end + region_start))
            .collect();
        self.block_index.update_tail(region_start, tail_spans, batch);
    }
}
