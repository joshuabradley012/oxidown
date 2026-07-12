//! Error type for the core. No panics on bad external input — every public
//! entry point validates and returns one of these instead.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// The caller's revision does not match the core's current revision.
    StaleRevision { current: u64, requested: u64 },
    /// A splice in an edit batch is malformed (overlapping, unordered, overflow).
    InvalidSplice { index: usize, detail: String },
    /// A position or range end lies beyond the end of the document.
    OutOfBounds { pos: usize, len: usize },
    /// A UTF-16 position falls between the two code units of a surrogate pair.
    SurrogateSplit { pos: usize },
    /// A range with `from > to`.
    InvalidRange { from: usize, to: usize },
    /// An argument outside its documented domain (e.g. a heading level
    /// above 6). Not a range/position error: the value itself is invalid.
    InvalidArgument { detail: String },
    /// `stream_append` on an id that was never opened or is already closed.
    UnknownStream { id: u64 },
}

impl CoreError {
    /// Stable machine-readable name, used as the prefix of thrown JS errors.
    pub fn name(&self) -> &'static str {
        match self {
            CoreError::StaleRevision { .. } => "StaleRevision",
            CoreError::InvalidSplice { .. } => "InvalidSplice",
            CoreError::OutOfBounds { .. } => "OutOfBounds",
            CoreError::SurrogateSplit { .. } => "SurrogateSplit",
            CoreError::InvalidRange { .. } => "InvalidRange",
            CoreError::InvalidArgument { .. } => "InvalidArgument",
            CoreError::UnknownStream { .. } => "UnknownStream",
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreError::StaleRevision { current, requested } => write!(
                f,
                "StaleRevision: core is at revision {current}, caller passed {requested}"
            ),
            CoreError::InvalidSplice { index, detail } => {
                write!(f, "InvalidSplice: splice #{index}: {detail}")
            }
            CoreError::OutOfBounds { pos, len } => write!(
                f,
                "OutOfBounds: position {pos} beyond document length {len} (UTF-16 code units)"
            ),
            CoreError::SurrogateSplit { pos } => write!(
                f,
                "SurrogateSplit: position {pos} falls inside a surrogate pair"
            ),
            CoreError::InvalidRange { from, to } => {
                write!(f, "InvalidRange: from {from} > to {to}")
            }
            CoreError::InvalidArgument { detail } => {
                write!(f, "InvalidArgument: {detail}")
            }
            CoreError::UnknownStream { id } => {
                write!(f, "UnknownStream: stream {id} is unknown or already closed")
            }
        }
    }
}

impl std::error::Error for CoreError {}
