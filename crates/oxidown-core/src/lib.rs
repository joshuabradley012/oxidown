//! # oxidown-core
//!
//! The Oxidown editor core (M0 + M1): a `ropey` rope as the authoritative
//! document, a Phase-A pulldown-cmark overlay with byte-exact spans, a
//! block index with sticky block IDs, an append-only op log with
//! inverted-op undo/redo (incl. AI-stream units), public anchors, editing
//! commands, streaming ingestion, IME composition sessions, and
//! viewport-scoped decoration emission with core-side reveal.
//!
//! Implements `docs/boundary-v0.md` (v0/v0.1 plus the v0.2 M1 additions).
//! Public API positions are UTF-16 code units; internals are UTF-8 bytes
//! and never leak.
//!
//! wasm-safety: this crate never reads clocks (`std::time` panics on
//! wasm32-unknown-unknown) — timestamps are injected via `apply_edit`.

pub mod anchor;
pub mod block_index;
pub mod commands;
pub mod composition;
pub mod decorations;
pub mod editor;
pub mod error;
pub mod history;
pub mod mapping;
pub mod oplog;
pub mod parser;
pub mod text;

pub use commands::Command;
pub use decorations::{BlockStyle, Decoration, MarkStyle, WidgetKind};
pub use editor::{CoreChange, Editor, ReparseCounts, SelectionRange, Splice};
pub use error::CoreError;
pub use mapping::Bias;
pub use oplog::{EditOrigin, Op, OpId};
