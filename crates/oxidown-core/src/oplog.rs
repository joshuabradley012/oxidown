//! Append-only operation log. Every edit — user, IME, paste, undo, redo —
//! appends ops here; the log is never rewritten. This is the eg-walker-shaped
//! skeleton from plan.md §5.4: untransformed splices + IDs + parent versions.
//!
//! No wall-clock time lives here: the core never calls `SystemTime`/`Instant`
//! (they panic on wasm32-unknown-unknown). Timestamps are injected by the
//! caller into `apply_edit` and only used for undo coalescing, not stored.

use crate::text::ByteSplice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOrigin {
    User,
    Ime,
    Paste,
    Undo,
    Redo,
    /// M1: command ops (`toggleStrong`, `setHeading`, …). Never coalesces.
    Command,
    /// M1: AI stream ops (`stream_append`). Never coalesces via the normal
    /// path — stream chunks merge into their stream's single undo unit
    /// through `History::record_stream_append` instead.
    Ai,
}

impl EditOrigin {
    /// Parse the boundary string form
    /// ("user" | "ime" | "paste" | "undo" | "redo" | "command" | "ai").
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(EditOrigin::User),
            "ime" => Some(EditOrigin::Ime),
            "paste" => Some(EditOrigin::Paste),
            "undo" => Some(EditOrigin::Undo),
            "redo" => Some(EditOrigin::Redo),
            "command" => Some(EditOrigin::Command),
            "ai" => Some(EditOrigin::Ai),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            EditOrigin::User => "user",
            EditOrigin::Ime => "ime",
            EditOrigin::Paste => "paste",
            EditOrigin::Undo => "undo",
            EditOrigin::Redo => "redo",
            EditOrigin::Command => "command",
            EditOrigin::Ai => "ai",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpId {
    pub replica: u16,
    pub counter: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub id: OpId,
    pub lamport: u64,
    /// Counter of the op this one was generated against (0 for the first op).
    pub parent_counter: u64,
    pub origin: EditOrigin,
    /// The splice in byte coordinates, valid at generation time (i.e. against
    /// the document state produced by the parent op).
    pub splice: ByteSplice,
}

#[derive(Debug)]
pub struct OpLog {
    replica: u16,
    ops: Vec<Op>,
    next_counter: u64,
    lamport: u64,
}

impl OpLog {
    pub fn new(replica: u16) -> Self {
        Self {
            replica,
            ops: Vec::new(),
            next_counter: 1,
            lamport: 0,
        }
    }

    pub fn append(&mut self, origin: EditOrigin, splice: ByteSplice) -> OpId {
        let counter = self.next_counter;
        self.next_counter += 1;
        self.lamport += 1;
        let id = OpId {
            replica: self.replica,
            counter,
        };
        self.ops.push(Op {
            id,
            lamport: self.lamport,
            parent_counter: counter - 1,
            origin,
            splice,
        });
        id
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Drop the log contents (used by `load`, which replaces the document).
    /// Counters stay monotonic so op IDs are never reused within a session.
    pub fn clear(&mut self) {
        self.ops.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_parents_are_sequential() {
        let mut log = OpLog::new(7);
        let a = log.append(
            EditOrigin::User,
            ByteSplice {
                at: 0,
                delete: 0,
                insert: "a".into(),
            },
        );
        let b = log.append(
            EditOrigin::Paste,
            ByteSplice {
                at: 1,
                delete: 0,
                insert: "b".into(),
            },
        );
        assert_eq!(a, OpId { replica: 7, counter: 1 });
        assert_eq!(b, OpId { replica: 7, counter: 2 });
        assert_eq!(log.ops()[0].parent_counter, 0);
        assert_eq!(log.ops()[1].parent_counter, 1);
        assert_eq!(log.ops()[1].lamport, 2);
    }

    #[test]
    fn clear_keeps_counters_monotonic() {
        let mut log = OpLog::new(1);
        log.append(
            EditOrigin::User,
            ByteSplice {
                at: 0,
                delete: 0,
                insert: "a".into(),
            },
        );
        log.clear();
        let id = log.append(
            EditOrigin::User,
            ByteSplice {
                at: 0,
                delete: 0,
                insert: "b".into(),
            },
        );
        assert_eq!(id.counter, 2);
    }
}
