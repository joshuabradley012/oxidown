//! # oxidown-core
//!
//! The Oxidown editor core (M0 spike): a `ropey` rope as the authoritative
//! document, a Phase-A pulldown-cmark overlay with byte-exact spans, an
//! append-only op log with inverted-op undo/redo, IME composition sessions,
//! and viewport-scoped decoration emission with core-side reveal.
//!
//! Implements `docs/boundary-v0.md`. Public API positions are UTF-16 code
//! units; internals are UTF-8 bytes and never leak.
//!
//! wasm-safety: this crate never reads clocks (`std::time` panics on
//! wasm32-unknown-unknown) — timestamps are injected via `apply_edit`.

pub mod composition;
pub mod decorations;
pub mod editor;
pub mod error;
pub mod history;
pub mod oplog;
pub mod parser;
pub mod text;

pub use decorations::{Decoration, MarkStyle};
pub use editor::{Editor, HistoryResult, SelectionRange, Splice};
pub use error::CoreError;
pub use oplog::{EditOrigin, Op, OpId};
