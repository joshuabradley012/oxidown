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
use crate::text::{ByteSplice, SrcBytes, TextBuffer};

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

/// Which reparse strategy each text change took — exposed for tests and perf
/// diagnostics (see [`Editor::reparse_counts`]). The counters let the
/// equivalence tests assert the fast paths actually FIRE (a dispatch bug
/// that silently full-reparsed everything would still be correct, just
/// slow — invisible to any correctness assertion).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReparseCounts {
    /// Full-document reparses (`reparse_with`).
    pub full: u64,
    /// Tail-only reparses (`reparse_tail`: streaming appends, qualifying
    /// end-of-document edits, and incremental reparses whose window reached
    /// the document's end).
    pub tail: u64,
    /// Windowed mid-document reparses that CONVERGED (`reparse_incremental`
    /// splicing a bounded window into the overlay/blocks). Non-converging
    /// attempts degrade and count under `full` or `tail` instead.
    pub incremental: u64,
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
    reparse_counts: ReparseCounts,
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
            reparse_counts: ReparseCounts::default(),
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
        // Fast-path decision BEFORE the text mutates (same discipline as
        // `stream_append`): the precondition reads pre-edit text and the
        // pre-edit block index, in the batch's own (pre-edit) coordinates.
        let fast_region = (batch.len() == 1)
            .then(|| self.tail_edit_fast_path_region(&batch[0]))
            .flatten();
        let composing = self.composition.is_some();
        let inverse = self.apply_bytes(&batch, origin);
        self.history
            .record_edit(inverse, origin, now_ms, forward_single, composing);
        match fast_region {
            Some(region_start) => self.reparse_tail(region_start, &batch),
            None => self.reparse_incremental(&batch),
        }
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
        self.history.push_redo(redo_inverse, unit.stream_id);
        self.history.set_break();
        self.reparse_incremental(&unit.inverse);
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
        self.history.push_undo_unit(undo_inverse, unit.stream_id);
        self.history.set_break();
        self.reparse_incremental(&unit.inverse);
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
        // Clarification 1: compositionBegin closes any open undo group; the
        // session's own edits then coalesce into exactly one unit.
        self.history.set_break();
        self.composition = Some(Composition { start, end });
        Ok(())
    }

    pub fn composition_end(&mut self) {
        // Clarification 1: compositionEnd closes the session's undo group.
        self.history.set_break();
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
    /// Core-internal anchors (stream insertion points) read as unknown.
    pub fn resolve_anchor(&self, id: u64) -> Option<usize> {
        self.anchors.resolve(id).map(|b| self.text.byte_to_utf16(b))
    }

    /// No-op on unknown ids — and on core-internal anchor ids (stream
    /// insertion points), which no boundary caller may disturb.
    pub fn drop_anchor(&mut self, id: u64) {
        self.anchors.remove(id);
    }

    // ---- commands (boundary v0.2) ---------------------------------------

    /// Run a command against the overlay. Returns `Ok(None)` when the
    /// command doesn't apply at the target (per the contract), `Ok(Some)`
    /// with the applied change otherwise. Command edits enter the op log
    /// with origin `command` and form single, never-coalescing undo units.
    pub fn command(&mut self, cmd: Command) -> Result<Option<CoreChange>, CoreError> {
        // Chunk-cached byte reader — the planners only ever read a handful
        // of local lines, so the old whole-document `String` materialization
        // here was a pure O(doc) waste (research/08-perf-baseline.md §10.4).
        let src = SrcBytes::new(&self.text);
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
                commands::toggle_inline(&self.overlay, &src, kind, from_b, to_b)?
            }
            Command::SetHeading { pos, level } => {
                if level > 6 {
                    return Err(CoreError::InvalidArgument {
                        detail: format!("setHeading level {level} is out of range 0..=6"),
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
            Command::IndentList { from, to } | Command::OutdentList { from, to } => {
                let (lo, hi) = (from.min(to), from.max(to));
                let from_b = self.text.utf16_to_byte(lo)?;
                let to_b = self.text.utf16_to_byte(hi)?;
                if matches!(cmd, Command::IndentList { .. }) {
                    commands::indent_list(&self.overlay, &src, from_b, to_b)
                } else {
                    commands::outdent_list(&self.overlay, &src, from_b, to_b)
                }
            }
            Command::Enter { from, to } => {
                let (lo, hi) = (from.min(to), from.max(to));
                let from_b = self.text.utf16_to_byte(lo)?;
                let to_b = self.text.utf16_to_byte(hi)?;
                commands::enter(&self.overlay, &src, from_b, to_b)
            }
        };
        // A plan with an empty batch means the command APPLIES but no
        // movement is possible (boundary v0.2: indentList/outdentList at the
        // top/bottom of their nesting range) — it must not create an undo
        // unit or bump the revision, unlike every other (non-empty) plan.
        Ok(plan.map(|plan| {
            if plan.batch.is_empty() {
                CoreChange {
                    revision: self.revision,
                    splices: Vec::new(),
                    selection: None,
                }
            } else {
                self.apply_plan(plan)
            }
        }))
    }

    // ---- streaming (boundary v0.2, plan §5.9) ----------------------------

    /// Open a stream at `pos` (UTF-16, strict: an insertion point inside a
    /// surrogate pair would corrupt text and errors). The insertion point
    /// becomes an internal after-bias anchor that maps through all edits.
    pub fn stream_open(&mut self, pos: usize) -> Result<u64, CoreError> {
        let byte = self.text.utf16_to_byte(pos)?;
        let anchor = self.anchors.create_internal(byte, Bias::After);
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
        let Some(at_b) = self.anchors.resolve_internal(stream.anchor) else {
            // Invariant: the internal anchor lives as long as its stream
            // (public drop_anchor cannot touch internal ids). Fail soft if
            // it ever breaks — a panic would cross the wasm boundary.
            debug_assert!(false, "internal anchor missing for open stream {id}");
            return Err(CoreError::UnknownStream { id });
        };
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
        // Same first-line hazard analysis as apply_edit (the append is an
        // insert-only splice): appending into the tail block's first line
        // can merge it with the block above — de-interruption, setext,
        // indent capture — which a standalone tail-slice parse cannot see.
        let fast_region = self.tail_edit_fast_path_region(&batch[0]);
        self.apply_bytes(&batch, EditOrigin::Ai);
        self.history.record_stream_append(id, at_b, chunk.len());
        match fast_region {
            Some(region_start) => self.reparse_tail(region_start, &batch),
            None => self.reparse_incremental(&batch),
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
            self.anchors.remove_internal(stream.anchor);
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

    /// Read access to the cached parse overlay (debug/verification/tests;
    /// not a boundary API). The equivalence tests compare this node-for-node
    /// against a from-scratch `parser::parse_document` of the current text.
    pub fn overlay_nodes(&self) -> &[Node] {
        &self.overlay
    }

    /// How many times each reparse strategy has run on this editor
    /// (debug/verification/tests; not a boundary API).
    pub fn reparse_counts(&self) -> ReparseCounts {
        self.reparse_counts
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
        // Op::splice's invariant: each op is valid against the document
        // state produced by its parent op. A batch's splices share PRE-batch
        // coordinates, so each one is rebased through the cumulative delta
        // of its predecessors (ascending + non-overlapping makes a plain
        // delta shift exact) before it is logged.
        let mut op_delta: isize = 0;
        for s in batch {
            self.oplog.append(
                origin,
                ByteSplice {
                    at: usize::try_from(s.at as isize + op_delta).unwrap_or(0),
                    delete: s.delete,
                    insert: s.insert.clone(),
                },
            );
            op_delta += s.insert.len() as isize - s.delete as isize;
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
        self.reparse_incremental(&plan.batch);
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
        self.reparse_counts.full += 1;
        let text = self.text.text();
        let parsed = parser::parse_document(&text);
        self.overlay = parsed.nodes;
        self.block_index.update(parsed.blocks, batch);
    }

    /// Weak tail fast path precondition (the shared base of
    /// [`Editor::tail_edit_fast_path_region`], which BOTH `apply_edit` and
    /// `stream_append` go through): the edit lands at/after the LAST
    /// top-level block's start, and that block starts at a line boundary (an
    /// indented code block's span starts mid-line and would not re-parse
    /// equivalently as a standalone slice). Returns the region start to
    /// re-parse from, or `None` → the ordinary reparse path.
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
        // A block start is "at a line boundary" when the byte right before
        // it ends a line terminator: `\n` (lone, or the second byte of
        // `\r\n`) or a lone `\r` (pulldown-cmark treats a bare `\r` as a
        // line ending too — verified against this pulldown-cmark version;
        // see `parser.rs`'s `line_bounds`/`split_lines`). Without the `\r`
        // arm this probe would wrongly refuse the fast path for every block
        // that starts right after a lone-`\r`-terminated line — always
        // SAFE (it only forces the slower fallback), just needlessly so.
        if region_start == 0
            || matches!(self.text.byte_at(region_start - 1), Some(b'\n') | Some(b'\r'))
        {
            Some(region_start)
        } else {
            None
        }
    }

    /// Tail fast path for `apply_edit` (interactive typing) and
    /// `stream_append` (an insert-only splice). STRICTER than the weak
    /// precondition above, because editing the tail block's FIRST LINE can
    /// change how the block relates to the block ABOVE it — which a
    /// standalone tail-slice reparse cannot see:
    ///
    /// * de-interruption: `"para\n# head"` — deleting the `#` (or making the
    ///   line `"-x"`, `"x# head"`, …) turns the line into a lazy
    ///   continuation of the directly adjacent paragraph, MERGING the two;
    /// * setext: the first line becoming `"==="` heading-ifies a directly
    ///   preceding paragraph;
    /// * indent/marker capture: `"- item\n\npara"` — giving `para`'s first
    ///   line leading indent (or a list marker) merges it into the list
    ///   above, even ACROSS the blank line;
    /// * blank-line span absorption: `"- a\n\npara"` + `"\r\n"` at `para`'s
    ///   start → `"- a\n\n\r\npara"` — no content merges, but pulldown-cmark
    ///   reports List (and FootnoteDefinition) spans INCLUDING trailing
    ///   blank lines, so the new blank line extends the absorber's span
    ///   (`List 0..5` → `List 0..7`) — a change strictly ABOVE the tail
    ///   slice, invisible to a standalone tail parse. Deletions trigger it
    ///   too: `"- a\n\nx\npara"` minus the `x` blanks the first line the
    ///   same way.
    ///
    /// The first two require direct line adjacency, so a blank line above
    /// the tail block ("insulation") rules them out. The last two survive
    /// blank lines but only for block kinds that absorb indented/marker
    /// lines or trailing blanks (lists, indented code, footnote
    /// definitions) and only when the edit leaves the first line with an
    /// absorbable SHAPE (leading space/tab, a list-marker character, or a
    /// line-terminator byte — i.e. the first line became blank). Hence:
    ///
    /// * an edit strictly past the first line is always safe (the block's
    ///   start boundary is byte-determined by unchanged text);
    /// * a first-line edit is safe iff the tail block is insulated AND
    ///   (the block above cannot absorb, OR the post-edit first line does
    ///   not start with an absorbable byte).
    ///
    /// Under these conditions the standalone tail parse is exactly
    /// equivalent to a full reparse (not merely decoration-equivalent),
    /// modulo the documented whole-document couplings (link reference
    /// definitions, footnote definitions) that M1 does not decorate —
    /// gated by `tests/reparse_equivalence.rs`. Everything else takes the
    /// ordinary reparse path.
    fn tail_edit_fast_path_region(&self, splice: &ByteSplice) -> Option<usize> {
        let region_start = self.tail_fast_path_region(splice.at)?;
        let first_line = self.text.line_range_at(region_start);
        // An edit AT `first_line.end` still appends to the first line's
        // text (`"-"` + `"x"` = `"-x"`, no longer a list item), so only
        // strictly-past positions skip the first-line analysis.
        if splice.at > first_line.end {
            return Some(region_start);
        }
        // Insulation: a blank line (or document start) directly above —
        // i.e. the line CONTAINING `region_start - 1` (a terminator byte,
        // already verified by `tail_fast_path_region`) has empty content.
        // Resolved through the rope's own line metric rather than a raw
        // two-byte `\n\n` probe so every terminator flavor is handled
        // (`\n\n`, `\r\r`, `\r\n\r\n`, mixes — a naive `\n|\r` byte pair
        // would misread ONE `\r\n` terminator as two, claiming insulation
        // that isn't there).
        let insulated =
            region_start == 0 || self.text.line_range_at(region_start - 1).is_empty();
        if !insulated {
            return None;
        }
        let blocks = self.block_index.blocks();
        let above_absorbs = blocks
            .len()
            .checked_sub(2)
            .map(|i| blocks[i].kind)
            .is_some_and(|k| {
                matches!(
                    k,
                    parser::BlockKind::List
                        | parser::BlockKind::CodeBlock
                        | parser::BlockKind::FootnoteDefinition
                )
            });
        if !above_absorbs {
            return Some(region_start);
        }
        // Post-edit first byte of the first line: from the insert when the
        // edit sits exactly at the block start, else the (unchanged) byte
        // already there.
        let post_first_byte = if splice.at == region_start {
            splice
                .insert
                .as_bytes()
                .first()
                .copied()
                .or_else(|| self.text.byte_at(region_start + splice.delete))
        } else {
            self.text.byte_at(region_start)
        };
        // `\r` / `\n` arms: a first line that became BLANK is absorbable
        // too — List/FootnoteDefinition spans extend over trailing blank
        // lines (the doc comment's fourth hazard). Without them, streaming
        // `"\r\n"` at the tail block's start under a loose list took the
        // fast path and left the cached List span 2 bytes short of a full
        // parse. Only an edit touching `region_start` itself can blank the
        // first line (the `else` arm's byte is a block's first byte, never
        // a terminator), so unchanged-first-byte edits stay on the fast
        // path.
        let absorbable_shape = post_first_byte.is_some_and(|b| {
            matches!(b, b' ' | b'\t' | b'-' | b'+' | b'*' | b'\r' | b'\n') || b.is_ascii_digit()
        });
        (!absorbable_shape).then_some(region_start)
    }

    /// Re-parse only `[region_start, end)` and splice the results into the
    /// overlay and block index. `batch` (the applied append) lands entirely
    /// at/after `region_start`, so everything before it is untouched.
    ///
    /// COST NOTE (known limitation): per-append work is O(tail block), not
    /// O(chunk) — the whole open block is re-parsed on every append. A
    /// stream that never closes its tail block (one long paragraph with no
    /// blank lines, or a single top-level list — a realistic AI-output
    /// shape) therefore pays O(streamed-so-far) per append and O(n²) total;
    /// measured ~36µs → ~745µs per 50-byte append as the tail block grows
    /// to ~232KB. Characterized (not gated) by `tests/stream_perf.rs::
    /// stream_append_into_never_closing_tail_block_grows_per_append`. This
    /// is inherent to exact overlay equivalence with a non-incremental
    /// parser: inline structure is globally coupled WITHIN a block (an
    /// appended `*` can close an emphasis opened at the block's first
    /// character; a line gaining `===` re-types the whole paragraph as a
    /// heading; an appended backtick can re-pair every code span), so any
    /// bounded per-append update would have to re-derive the parser's
    /// delimiter state to stay provably equivalent — exactly the divergence
    /// risk the `reparse_equivalence` gate exists to forbid. A real fix
    /// needs an incremental inline parser (or per-block node caching keyed
    /// on block text), deferred beyond M1.
    fn reparse_tail(&mut self, region_start: usize, batch: &[ByteSplice]) {
        self.reparse_counts.tail += 1;
        // COST NOTE: this `retain` walks the ENTIRE overlay (the kept prefix
        // included), so even a 1-char EOF keystroke pays O(overlay nodes) —
        // small constants (a predicate over two usizes, compacting in
        // place), but not strictly size-independent. See the invariant note
        // in `reparse_incremental` step 3a for the shared flat-vec tradeoff
        // and the scale at which it would matter.
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

    /// Incremental, block-local reparse after `batch` (ascending,
    /// non-overlapping, PRE-application byte coordinates; the text has
    /// already been mutated). Makes `apply_edit` O(edit + dirty region)
    /// instead of O(doc) for typical edits — boundary-v0.md line 64's
    /// complexity requirement. Multi-splice batches are handled as one
    /// UNION dirty region `[first.at, last.end()]` (commands and undo/redo
    /// batches are line-local clusters, so the union stays small; a
    /// pathological far-apart batch simply gets a large window and, at
    /// worst, degrades below).
    ///
    /// ## Algorithm
    ///
    /// 1. **Window start** — the start of the top-level block ONE BEFORE the
    ///    block containing the dirty region's start (walked further back to
    ///    a line-boundary block start; document start if none). The one
    ///    block of slack is load-bearing: an edit at/near a block's first
    ///    line can change that block's relationship to the block directly
    ///    above it (lazy continuation / de-interruption, setext
    ///    underlining, list indent-capture), so the block above must be
    ///    inside the reparsed window. It cannot reach FURTHER back: those
    ///    effects need either direct line adjacency to a paragraph or an
    ///    absorbing container, and the slack block's own first line and its
    ///    relationship to everything above it are untouched bytes — if the
    ///    slack block was a top-level boundary before, it still is.
    /// 2. **Window end / convergence** — parse the post-edit slice
    ///    `[region_start, W)` where `W` is an old block end (shifted by the
    ///    batch's net `delta`) past the dirty region; accept a convergence
    ///    point `P` when:
    ///      * `P` is a top-level block boundary of the SLICE parse with at
    ///        least one more slice block after it (see safety below),
    ///      * `P - delta` is an OLD top-level block end, and
    ///      * `P` sits at/after every edit (the text from `P` on is
    ///        untouched bytes).
    ///
    ///    Then everything in `[region_start, P)` is replaced with the fresh
    ///    parse and everything from `P` on is the old overlay/blocks
    ///    shifted by `delta`. No convergence by document end → the window
    ///    becomes the whole tail → `reparse_tail` semantics (and a window
    ///    reaching EOF on the FIRST attempt short-circuits the same way).
    ///
    /// ## Why boundary alignment (plus a successor block) suffices
    ///
    /// The slice parser reads exactly the same bytes as a full parse would
    /// until `W`, so the only way the slice parse can disagree with the
    /// true parse of `[region_start, ..)` is TRUNCATION at `W` — and every
    /// truncation artifact lives in the slice's FINAL block:
    ///
    /// * a construct closed by slice-EOF rather than real syntax (an
    ///   unterminated fence, an HTML block) is one slice block running to
    ///   exactly `W` — it can never contain an interior boundary, so `P`
    ///   (which must have a successor block) can never land inside or at
    ///   the end of it; requiring a successor block is precisely what
    ///   rejects "the boundary that realigns by coincidence inside a
    ///   fence": a fence opened by the edit swallows the rest of the slice,
    ///   leaving no interior boundaries to falsely accept, and its
    ///   coincidental end-at-`W` is excluded for having no successor;
    /// * a block cut mid-way (paragraph continuing past `W`, a setext
    ///   underline just past `W`) is likewise the final block.
    ///
    /// Interior boundaries of the slice are therefore REAL: nothing spans
    /// `P` in the new parse, so the parser state at `P` is clean
    /// (top-of-document); `P - delta` being an old block end means the old
    /// parse state there was clean too; and the text from `P` on is
    /// unchanged — identical bytes parsed from identical clean state parse
    /// identically, so the old overlay/blocks from `P` on (shifted) ARE the
    /// new parse's tail. Comparing block KINDS at `P` would be wrong rather
    /// than extra-safe — the block ENDING at `P` is dirty and may have
    /// legitimately changed kind — but as belt-and-braces this check DOES
    /// require the successor slice block to match the corresponding old
    /// block's start and kind (by the argument above it always does; a
    /// mismatch means the reasoning broke and we degrade instead of
    /// corrupting). The known whole-document couplings that violate
    /// "identical bytes ⇒ identical parse" — link reference definitions and
    /// footnote definitions — affect constructs M1 does not decorate
    /// (documented Phase-A assumption, same as the streaming fast path).
    ///
    /// Equivalence is gated by `tests/reparse_equivalence.rs` (node-for-node
    /// against a from-scratch parse after every fuzzed edit).
    fn reparse_incremental(&mut self, batch: &[ByteSplice]) {
        let blocks = self.block_index.blocks();
        let Some(last_splice) = batch.last() else {
            return self.reparse_with(batch); // load-style: no dirty region
        };
        if blocks.is_empty() {
            return self.reparse_with(batch);
        }
        let dirty_lo = batch[0].at;
        let dirty_hi = last_splice.end();
        let delta: isize = batch
            .iter()
            .map(|s| s.insert.len() as isize - s.delete as isize)
            .sum();
        let len_post = self.text.len_bytes();

        // 1. Window start: one block of slack, at a line-boundary start.
        //    (Bytes before `dirty_lo` are unchanged, so probing the mutated
        //    text below is probing pre-edit bytes.)
        let containing = blocks.partition_point(|b| b.span.start <= dirty_lo);
        let mut region_start = 0usize;
        let mut idx = containing as isize - 2; // one block before the containing one
        while idx >= 0 {
            let s = blocks[idx as usize].span.start;
            // See `tail_fast_path_region`'s doc comment: a lone `\r` ends a
            // line here too, not just `\n`.
            if s == 0 || matches!(self.text.byte_at(s - 1), Some(b'\n') | Some(b'\r')) {
                region_start = s;
                break;
            }
            idx -= 1;
        }

        // 2. Grow the window over candidate old block ends until the slice
        //    parse converges. `after_hi` is the first block starting past
        //    the dirty region; the first window already includes one whole
        //    untouched block (the convergence certificate needs a successor
        //    block inside the slice).
        let after_hi = blocks.partition_point(|b| b.span.start <= dirty_hi);
        let mut cand = after_hi;
        let mut step = 1usize;
        loop {
            if cand >= blocks.len() {
                // Window reached EOF without (or before) convergence: the
                // whole tail is the dirty region. reparse_tail from a
                // mid-document region_start is sound by the same clean-
                // boundary argument (see the doc comment's step 1).
                return self.reparse_tail(region_start, batch);
            }
            let w_pre = blocks[cand].span.end;
            let w_post = (w_pre as isize + delta) as usize;
            if w_post >= len_post {
                return self.reparse_tail(region_start, batch);
            }
            let slice = self.text.byte_slice_to_string(region_start..w_post);
            let parsed = parser::parse_document(&slice);

            // Earliest valid convergence point: an interior slice boundary
            // at/after every edit whose pre-image is an old block end, with
            // a start+kind-matching successor.
            let mut found: Option<(usize, usize)> = None; // (p_abs, old idx m)
            for i in 0..parsed.blocks.len().saturating_sub(1) {
                let p_abs = region_start + parsed.blocks[i].1.end;
                let p_pre = p_abs as isize - delta;
                if p_pre < dirty_hi as isize {
                    continue; // text before P still touched by the batch
                }
                let p_pre = p_pre as usize;
                // Old block ends are strictly increasing (ordered,
                // non-overlapping spans): binary-search for m with
                // blocks[m].span.end == p_pre.
                let m = blocks.partition_point(|b| b.span.end < p_pre);
                if m >= blocks.len() || blocks[m].span.end != p_pre {
                    continue;
                }
                // Belt-and-braces successor check (see the doc comment).
                let succ = &parsed.blocks[i + 1];
                let succ_matches = blocks.get(m + 1).is_some_and(|old| {
                    old.kind == succ.0
                        && old.span.start as isize + delta
                            == (region_start + succ.1.start) as isize
                });
                if !succ_matches {
                    continue;
                }
                found = Some((p_abs, m));
                break;
            }
            let Some((p_abs, m)) = found else {
                cand += step;
                step *= 2; // geometric growth: total work <= 2x final window
                continue;
            };
            self.reparse_counts.incremental += 1;
            let p_rel = p_abs - region_start;
            let p_pre = (p_abs as isize - delta) as usize;

            // 3a. Overlay splice. The overlay is sorted by extent.start and
            //     no node spans a top-level block boundary, so the replaced
            //     range is a contiguous run.
            //
            //     INVARIANT/COST NOTE (steps 3a+3b): the overlay is a flat
            //     Vec of ABSOLUTE byte offsets, so after the window splice
            //     every suffix node is shifted by `delta` (the loop below),
            //     and 3b rebuilds + re-matches the whole block-span list.
            //     A keystroke is therefore O(overlay nodes + blocks) with
            //     very small constants (integer adds over a contiguous Vec;
            //     one Vec rebuild of ~(doc/400)-ish spans) — NOT strictly
            //     size-independent. Deliberate tradeoff: relative-offset
            //     trees (CM6-style) make every READ pay pointer-chasing and
            //     accumulation, while this shape keeps `decorations()` a
            //     binary-searchable sorted slice. Counterexample scale: the
            //     shift only starts to rival the window parse itself around
            //     ~10MB documents (~1M overlay nodes ≈ a few hundred µs of
            //     shifting per keystroke); the contract's 100KB reference
            //     doc costs ~10k node shifts ≈ single-digit µs. Revisit
            //     (chunked offsets / a per-suffix pending-delta) only if
            //     docs that size become a target.
            let prefix_len = self
                .overlay
                .partition_point(|n| n.extent.start < region_start);
            let suffix_start = self.overlay.partition_point(|n| n.extent.start < p_pre);
            let fresh: Vec<Node> = parsed
                .nodes
                .into_iter()
                .filter(|n| n.extent.end <= p_rel)
                .map(|mut n| {
                    n.offset(region_start);
                    n
                })
                .collect();
            let fresh_len = fresh.len();
            self.overlay.splice(prefix_len..suffix_start, fresh);
            for n in &mut self.overlay[prefix_len + fresh_len..] {
                n.offset_signed(delta);
            }

            // 3b. Block index: assemble the full new span list (before ++
            //     fresh ++ shifted-after) and let the ordinary `update`
            //     re-match IDs — identical stability semantics to a full
            //     reparse, O(#blocks) with a small constant.
            let blocks = self.block_index.blocks();
            let before_len = blocks.partition_point(|b| b.span.end <= region_start);
            let mut new_spans: Vec<(parser::BlockKind, std::ops::Range<usize>)> =
                Vec::with_capacity(blocks.len() + 4);
            new_spans.extend(blocks[..before_len].iter().map(|b| (b.kind, b.span.clone())));
            new_spans.extend(
                parsed
                    .blocks
                    .iter()
                    .filter(|(_, r)| r.end <= p_rel)
                    .map(|(k, r)| (*k, r.start + region_start..r.end + region_start)),
            );
            new_spans.extend(blocks[m + 1..].iter().map(|b| {
                (
                    b.kind,
                    (b.span.start as isize + delta) as usize
                        ..(b.span.end as isize + delta) as usize,
                )
            }));
            self.block_index.update(new_spans, batch);
            return;
        }
    }
}
