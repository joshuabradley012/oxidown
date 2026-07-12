//! wasm-bindgen boundary for `oxidown-core`, exposing the `OxidownCore`
//! TypeScript interface from docs/boundary-v0.md (v0/v0.1 plus the v0.2 M1
//! additions). Positions are already UTF-16 code units at the core API, so
//! this is a thin shim.
//!
//! Payload strategy: structured values (splice batches in, decoration/splice
//! batches out) cross the boundary as **one JSON string** per call —
//! `serde_json` on the Rust side, `JSON.parse`/`JSON.stringify` (via
//! `js_sys::JSON`) on the JS side, so callers still see plain JS objects per
//! the contract. Chosen over `serde-wasm-bindgen` because a single string
//! blob beats field-by-field JsValue reflection for batched payloads
//! (research/03-rust-ecosystem.md §6: "many small strings are the pathology,
//! single large blobs are fine") and it keeps this crate's dependency
//! surface to `serde_json` only.
//!
//! Wire shapes (v0.2): `undo`/`redo`/`command`/`streamAppend` all return the
//! `CoreChange` shape `{ revision, splices, selection? }`; M1 line styles
//! serialize as `{ kind: "line", at, style, depth? }` and task checkboxes as
//! `{ kind: "widget", from, to, widget: "task", checked }`.
//!
//! Timestamps: the core never reads clocks (std::time panics on
//! wasm32-unknown-unknown); `applyEdit` injects `js_sys::Date::now()`.
//!
//! Errors: every core `Err` becomes a thrown JS exception whose message
//! starts with the error name (e.g. "StaleRevision: ..."), per the contract's
//! mirror-desync-emergency handling. Every numeric argument crosses as `f64`
//! (never a `u32` wasm-bindgen param, whose JS→wasm coercion silently wraps
//! negatives and truncates fractions) and is validated (finite, integral,
//! non-negative) before conversion; anything else throws "InvalidArgs: ..."
//! — `as usize` would otherwise silently saturate negatives to 0 and NaN
//! to 0. Document POSITIONS are additionally bounded to `u32::MAX` (wasm32's
//! usize is 32 bits, so `as usize` would truncate 2^32+6 to 6 and edit the
//! wrong place); an over-u32 position is necessarily beyond the document and
//! fails with the core's own "OutOfBounds: ..." message, matching the mock,
//! whose numeric layer has no u32 ceiling and reaches its document-bounds
//! check instead. The constructor installs
//! `console_error_panic_hook` so any core panic surfaces its message on the
//! JS console instead of an opaque `RuntimeError: unreachable`.
//!
//! No `rand`/`getrandom`: `replica_id` is a constructor parameter
//! (default 1).

use oxidown_core::{
    Bias, Command, CoreChange, CoreError, Decoration, EditOrigin, Editor, SelectionRange, Splice,
};
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::prelude::*;

#[derive(Deserialize)]
struct SpliceIn {
    at: u32,
    delete: u32,
    insert: String,
}

#[derive(Deserialize)]
struct SelectionIn {
    anchor: u32,
    head: u32,
}

fn core_err(e: CoreError) -> JsError {
    // Display output starts with the error name, e.g. "StaleRevision: ...".
    JsError::new(&e.to_string())
}

/// Validate an `f64` boundary argument (position/level/id): it must be
/// finite, integral, and non-negative — `as` casts would otherwise silently
/// saturate NaN/negatives to 0 and quietly truncate fractions. Returns the
/// "InvalidArgs: ..." message on failure (a plain `String` so native unit
/// tests can exercise it without constructing a `JsError` off-wasm).
fn check_arg(v: f64, what: &str) -> Result<u64, String> {
    if !v.is_finite() || v.fract() != 0.0 || v < 0.0 {
        return Err(format!(
            "InvalidArgs: {what} must be a non-negative integer, got {v}"
        ));
    }
    Ok(v as u64)
}

/// `check_arg`, boundary-flavored: failures become thrown JS exceptions.
fn arg(v: f64, what: &str) -> Result<u64, JsError> {
    check_arg(v, what).map_err(|msg| JsError::new(&msg))
}

/// Largest document position the boundary accepts: it must fit wasm32's
/// 32-bit `usize`. Document lengths (UTF-16 code units of an in-memory
/// string) always fit too, so anything above this is beyond the document.
const MAX_POS: u64 = u32::MAX as u64;

/// The core's OutOfBounds message (`CoreError::OutOfBounds` Display),
/// reproduced verbatim for positions too large to fit the `usize` the
/// `CoreError` variant carries on wasm32. `pos` stays `f64` so the value
/// formats exactly as the caller sent it (integral f64s print digits-only,
/// matching the mock's JS `${pos}`).
fn oob_msg(pos: f64, doc_len: usize) -> String {
    format!("OutOfBounds: position {pos} beyond document length {doc_len} (UTF-16 code units)")
}

/// Position flavor of `check_arg`: additionally rejects integers above
/// `u32::MAX`, which `as usize` on wasm32 would silently TRUNCATE (2^32+6
/// becomes 6 — probe-confirmed to edit the wrong range before this guard).
/// The rejection is the same OutOfBounds the core's own document-bounds
/// check produces, because that is where the mock — whose numeric layer has
/// no u32 ceiling — fails for the same value (mock-core.ts `checkDocPos`).
fn check_pos(v: f64, what: &str, doc_len: usize) -> Result<usize, String> {
    let n = check_arg(v, what)?;
    if n > MAX_POS {
        return Err(oob_msg(v, doc_len));
    }
    Ok(n as usize)
}

/// `check_pos` for a query range (`decorations` / `compositionBegin`), with
/// the mock/core's shared error precedence when an endpoint exceeds
/// `u32::MAX`: malformed numbers first, then `InvalidRange` when
/// `from > to`, otherwise OutOfBounds reported on `to` (`to >= from`, so
/// `to` is over-u32 and is the position the bounds check names).
fn check_query_range(from: f64, to: f64, doc_len: usize) -> Result<(usize, usize), String> {
    let f = check_arg(from, "from")?;
    let t = check_arg(to, "to")?;
    if f > MAX_POS || t > MAX_POS {
        if f > t {
            return Err(format!("InvalidRange: from {from} > to {to}"));
        }
        return Err(oob_msg(to, doc_len));
    }
    Ok((f as usize, t as usize))
}

/// Parse a JsValue (array of objects) by stringifying once and deserializing.
fn from_js<T: for<'de> Deserialize<'de>>(value: &JsValue, what: &str) -> Result<T, JsError> {
    let s: String = js_sys::JSON::stringify(value)
        .map_err(|_| JsError::new(&format!("InvalidPayload: {what} is not JSON-serializable")))?
        .into();
    serde_json::from_str(&s)
        .map_err(|e| JsError::new(&format!("InvalidPayload: malformed {what}: {e}")))
}

/// Serialize to a JSON string and parse it back into a plain JS value.
fn to_js(value: &serde_json::Value) -> Result<JsValue, JsError> {
    js_sys::JSON::parse(&value.to_string())
        .map_err(|_| JsError::new("InternalError: produced invalid JSON"))
}

fn splices_json(splices: &[Splice]) -> serde_json::Value {
    serde_json::Value::Array(
        splices
            .iter()
            .map(|s| {
                json!({
                    "at": s.at,
                    "delete": s.delete,
                    "insert": s.insert,
                })
            })
            .collect(),
    )
}

fn core_change_json(change: &CoreChange) -> serde_json::Value {
    let mut obj = json!({
        "revision": change.revision,
        "splices": splices_json(&change.splices),
    });
    if let Some(sel) = &change.selection {
        obj["selection"] = json!({ "anchor": sel.anchor, "head": sel.head });
    }
    obj
}

fn change_to_js(change: Option<CoreChange>) -> Result<JsValue, JsError> {
    match change {
        None => Ok(JsValue::NULL),
        Some(c) => to_js(&core_change_json(&c)),
    }
}

/// Serialize a decoration batch straight into one JSON string — the hot
/// half of the `decorations()` boundary call. The old path built a
/// `serde_json::Value` tree first (one `Map` plus fresh key/tag `String`
/// allocations per decoration) and measurably cost MORE than computing the
/// decorations themselves (research/08-perf-baseline.md §8); this writer
/// allocates nothing but the output buffer.
///
/// Wire format: byte-identical to the previous `serde_json::Value`
/// serialization — compact separators, keys in ALPHABETICAL order (what
/// serde_json's default `BTreeMap` emitted), same optional-field omission
/// rules. Every field value is an integer, a boolean, or a fixed vocabulary
/// of static tags (`MarkStyle::as_str`/`BlockStyle::as_str`/`h{level}`), so
/// no string escaping is ever needed. Pinned byte-for-byte against the
/// `serde_json` path by the `wire_format` test module below.
fn decorations_json_string(decos: &[Decoration]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(decos.len() * 56 + 2);
    s.push('[');
    for (i, d) in decos.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        match d {
            Decoration::Mark { from, to, style } => {
                let _ = write!(
                    s,
                    "{{\"from\":{from},\"kind\":\"mark\",\"style\":\"{}\",\"to\":{to}}}",
                    style.as_str()
                );
            }
            Decoration::Conceal { from, to } => {
                let _ = write!(s, "{{\"from\":{from},\"kind\":\"conceal\",\"to\":{to}}}");
            }
            Decoration::Line { at, level } => {
                let _ = write!(s, "{{\"at\":{at},\"kind\":\"line\",\"style\":\"h{level}\"}}");
            }
            Decoration::Block { at, style, revealed } => {
                let _ = write!(s, "{{\"at\":{at}");
                if let Some(depth) = style.depth() {
                    let _ = write!(s, ",\"depth\":{depth}");
                }
                s.push_str(",\"kind\":\"line\"");
                if *revealed {
                    s.push_str(",\"revealed\":true");
                }
                let _ = write!(s, ",\"style\":\"{}\"}}", style.as_str());
            }
            Decoration::Widget { from, to, kind } => match kind {
                oxidown_core::WidgetKind::Task { checked } => {
                    let _ = write!(
                        s,
                        "{{\"checked\":{checked},\"from\":{from},\"kind\":\"widget\",\"to\":{to},\"widget\":\"task\"}}"
                    );
                }
                oxidown_core::WidgetKind::Bullet => {
                    let _ = write!(
                        s,
                        "{{\"from\":{from},\"kind\":\"widget\",\"to\":{to},\"widget\":\"bullet\"}}"
                    );
                }
                oxidown_core::WidgetKind::Ordered { number, delim } => {
                    let _ = write!(
                        s,
                        "{{\"delim\":\"{}\",\"from\":{from},\"kind\":\"widget\",\"number\":{number},\"to\":{to},\"widget\":\"ordered\"}}",
                        *delim as char
                    );
                }
            },
        }
    }
    s.push(']');
    s
}

/// The previous `serde_json::Value`-tree construction, kept ONLY as the
/// oracle pinning `decorations_json_string`'s byte-exact wire format (see
/// the `wire_format` test module).
#[cfg(test)]
fn decoration_json(d: &Decoration) -> serde_json::Value {
    match d {
        Decoration::Mark { from, to, style } => json!({
            "kind": "mark",
            "from": from,
            "to": to,
            "style": style.as_str(),
        }),
        Decoration::Conceal { from, to } => json!({
            "kind": "conceal",
            "from": from,
            "to": to,
        }),
        Decoration::Line { at, level } => json!({
            "kind": "line",
            "at": at,
            "style": format!("h{level}"),
        }),
        Decoration::Block { at, style, revealed } => {
            let mut obj = json!({
                "kind": "line",
                "at": at,
                "style": style.as_str(),
            });
            if let Some(depth) = style.depth() {
                obj["depth"] = json!(depth);
            }
            if *revealed {
                obj["revealed"] = json!(true);
            }
            obj
        }
        Decoration::Widget { from, to, kind } => match kind {
            oxidown_core::WidgetKind::Task { checked } => json!({
                "kind": "widget",
                "from": from,
                "to": to,
                "widget": "task",
                "checked": checked,
            }),
            oxidown_core::WidgetKind::Bullet => json!({
                "kind": "widget",
                "from": from,
                "to": to,
                "widget": "bullet",
            }),
            oxidown_core::WidgetKind::Ordered { number, delim } => json!({
                "kind": "widget",
                "from": from,
                "to": to,
                "widget": "ordered",
                "number": number,
                "delim": (*delim as char).to_string(),
            }),
        },
    }
}

#[wasm_bindgen]
pub struct OxidownCore {
    inner: Editor,
}

// Boundary-flavored position validation (plain impl block: these are
// internal helpers, not exports).
impl OxidownCore {
    /// `check_pos` against the live document length; failures thrown.
    fn pos_arg(&self, v: f64, what: &str) -> Result<usize, JsError> {
        check_pos(v, what, self.inner.doc_len_utf16()).map_err(|msg| JsError::new(&msg))
    }

    /// `check_query_range` against the live document length; failures thrown.
    fn query_range(&self, from: f64, to: f64) -> Result<(usize, usize), JsError> {
        check_query_range(from, to, self.inner.doc_len_utf16()).map_err(|msg| JsError::new(&msg))
    }
}

#[wasm_bindgen]
impl OxidownCore {
    /// `replica_id` defaults to 1 (no entropy source in the core by design).
    #[wasm_bindgen(constructor)]
    pub fn new(replica_id: Option<u16>) -> OxidownCore {
        // Panics surface with a message on the JS console instead of an
        // opaque `RuntimeError: unreachable` (idempotent; stderr on native).
        console_error_panic_hook::set_once();
        OxidownCore {
            inner: Editor::new(replica_id.unwrap_or(1)),
        }
    }

    /// Create/replace the document. Returns the new revision.
    pub fn load(&mut self, text: &str) -> f64 {
        self.inner.load(text) as f64
    }

    /// Apply an edit batch (`splices`: `Splice[]` per the contract). Returns
    /// the new revision; throws on stale revision / invalid splices.
    #[wasm_bindgen(js_name = applyEdit)]
    pub fn apply_edit(
        &mut self,
        base_revision: f64,
        splices: JsValue,
        origin: &str,
    ) -> Result<f64, JsError> {
        let splices: Vec<SpliceIn> = from_js(&splices, "splices")?;
        let origin = EditOrigin::parse(origin)
            .ok_or_else(|| JsError::new(&format!("InvalidOrigin: {origin:?}")))?;
        let core_splices: Vec<Splice> = splices
            .into_iter()
            .map(|s| Splice {
                at: s.at as usize,
                delete: s.delete as usize,
                insert: s.insert,
            })
            .collect();
        let base_revision = arg(base_revision, "baseRevision")?;
        let now_ms = js_sys::Date::now();
        self.inner
            .apply_edit(base_revision, &core_splices, origin, now_ms)
            .map(|rev| rev as f64)
            .map_err(core_err)
    }

    /// `CoreChange | null` — splices in current-doc coordinates plus
    /// optional cursor placement (v0.2 shape; supersets the v0 shape).
    pub fn undo(&mut self) -> Result<JsValue, JsError> {
        change_to_js(self.inner.undo())
    }

    /// `CoreChange | null` — splices in current-doc coordinates plus
    /// optional cursor placement (v0.2 shape; supersets the v0 shape).
    pub fn redo(&mut self) -> Result<JsValue, JsError> {
        change_to_js(self.inner.redo())
    }

    /// `Decoration[]` for viewport `[from, to)` against `revision` (must be
    /// current). `selections`: `SelectionRange[]`.
    pub fn decorations(
        &self,
        revision: f64,
        from: f64,
        to: f64,
        selections: JsValue,
    ) -> Result<JsValue, JsError> {
        // Validation order mirrors the mock (mock-core.ts `decorations`):
        // malformed revision → staleness → malformed from/to → range →
        // bounds → selections payload. The core re-checks staleness/range/
        // bounds for values a 32-bit usize can represent; this layer fronts
        // the checks it cannot pass through.
        let revision = arg(revision, "revision")?;
        let current = self.inner.revision();
        if revision != current {
            return Err(core_err(CoreError::StaleRevision {
                current,
                requested: revision,
            }));
        }
        let (from, to) = self.query_range(from, to)?;
        let selections: Vec<SelectionIn> = from_js(&selections, "selections")?;
        let sels: Vec<SelectionRange> = selections
            .into_iter()
            .map(|s| SelectionRange {
                anchor: s.anchor as usize,
                head: s.head as usize,
            })
            .collect();
        let decos = self
            .inner
            .decorations(revision, from, to, &sels)
            .map_err(core_err)?;
        js_sys::JSON::parse(&decorations_json_string(&decos))
            .map_err(|_| JsError::new("InternalError: produced invalid JSON"))
    }

    #[wasm_bindgen(js_name = compositionBegin)]
    pub fn composition_begin(&mut self, from: f64, to: f64) -> Result<(), JsError> {
        let (from, to) = self.query_range(from, to)?;
        self.inner.composition_begin(from, to).map_err(core_err)
    }

    #[wasm_bindgen(js_name = compositionEnd)]
    pub fn composition_end(&mut self) {
        self.inner.composition_end();
    }

    // ---- anchors (v0.2) --------------------------------------------------

    /// `createAnchor(pos, bias)` — bias is `"before"` or `"after"`. Returns
    /// the anchor id.
    #[wasm_bindgen(js_name = createAnchor)]
    pub fn create_anchor(&mut self, pos: f64, bias: &str) -> Result<f64, JsError> {
        // Bias first, then the position — the mock's order (mock-core.ts
        // `createAnchor`).
        let bias = match bias {
            "before" => Bias::Before,
            "after" => Bias::After,
            other => return Err(JsError::new(&format!("InvalidBias: {other:?}"))),
        };
        let pos = self.pos_arg(pos, "pos")?;
        self.inner
            .create_anchor(pos, bias)
            .map(|id| id as f64)
            .map_err(core_err)
    }

    /// Current position of the anchor, or `null` if unresolvable
    /// (unknown/dropped id, or the document was replaced by `load`).
    #[wasm_bindgen(js_name = resolveAnchor)]
    pub fn resolve_anchor(&self, id: f64) -> Result<JsValue, JsError> {
        Ok(match self.inner.resolve_anchor(arg(id, "id")?) {
            Some(pos) => JsValue::from_f64(pos as f64),
            None => JsValue::NULL,
        })
    }

    #[wasm_bindgen(js_name = dropAnchor)]
    pub fn drop_anchor(&mut self, id: f64) -> Result<(), JsError> {
        self.inner.drop_anchor(arg(id, "id")?);
        Ok(())
    }

    // ---- commands (v0.2) -------------------------------------------------

    /// Flattened command entry point:
    /// - `command("toggleStrong"|"toggleEm"|"toggleStrike"|"toggleCode", from, to)`
    /// - `command("setHeading", pos, level)` (level 0–6; 0 = paragraph)
    /// - `command("toggleTask", pos)`
    /// - `command("indentList"|"outdentList", from, to)` (marker-width-aware
    ///   Tab nesting, boundary v0.2)
    /// - `command("enter", from, to)` (construct-aware Enter: list/quote
    ///   continuation, single-press empty-item exit; boundary v0.3)
    ///
    /// Returns `CoreChange | null` (`null` when the command doesn't apply at
    /// the target). Throws on unknown names, missing arguments, or invalid
    /// positions.
    pub fn command(&mut self, name: &str, a: f64, b: Option<f64>) -> Result<JsValue, JsError> {
        let need_b = |what: &str| -> Result<f64, JsError> {
            b.ok_or_else(|| JsError::new(&format!("InvalidArgs: {name} requires {what}")))
        };
        // Range-command argument order mirrors the mock (`rangeArgs` +
        // `checkDocPos` in each command impl): malformed `from` → missing
        // `to` → malformed `to` → over-u32 bounds on `from`, then `to`.
        let range_args = |core: &Self| -> Result<(usize, usize), JsError> {
            arg(a, "from")?;
            let b_val = need_b("a `to` position")?;
            arg(b_val, "to")?;
            Ok((core.pos_arg(a, "from")?, core.pos_arg(b_val, "to")?))
        };
        let cmd = match name {
            "toggleStrong" | "toggleEm" | "toggleStrike" | "toggleCode" => {
                let (from, to) = range_args(self)?;
                match name {
                    "toggleStrong" => Command::ToggleStrong { from, to },
                    "toggleEm" => Command::ToggleEm { from, to },
                    "toggleStrike" => Command::ToggleStrike { from, to },
                    _ => Command::ToggleCode { from, to },
                }
            }
            "indentList" | "outdentList" => {
                let (from, to) = range_args(self)?;
                if name == "indentList" {
                    Command::IndentList { from, to }
                } else {
                    Command::OutdentList { from, to }
                }
            }
            "enter" => {
                let (from, to) = range_args(self)?;
                Command::Enter { from, to }
            }
            "setHeading" => {
                // Mock order: malformed pos → missing/malformed/out-of-range
                // level → pos document bounds (`setHeadingCmd` validates the
                // level before `checkDocPos`).
                arg(a, "pos")?;
                let level = arg(need_b("a heading level")?, "level")?;
                if level > 6 {
                    return Err(JsError::new(&format!(
                        "InvalidArgs: setHeading level must be an integer 0..=6, got {level}"
                    )));
                }
                Command::SetHeading {
                    pos: self.pos_arg(a, "pos")?,
                    level: level as u8,
                }
            }
            "toggleTask" => Command::ToggleTask {
                pos: self.pos_arg(a, "pos")?,
            },
            other => return Err(JsError::new(&format!("InvalidCommand: {other:?}"))),
        };
        change_to_js(self.inner.command(cmd).map_err(core_err)?)
    }

    // ---- streaming (v0.2) ------------------------------------------------

    /// Open a stream at `pos`; the insertion point becomes an internal
    /// after-bias anchor. Returns the stream id.
    #[wasm_bindgen(js_name = streamOpen)]
    pub fn stream_open(&mut self, pos: f64) -> Result<f64, JsError> {
        let pos = self.pos_arg(pos, "pos")?;
        self.inner
            .stream_open(pos)
            .map(|id| id as f64)
            .map_err(core_err)
    }

    /// Append a chunk; returns `CoreChange` (splices for the view to apply
    /// under its skip annotation). Throws `UnknownStream` on never-opened or
    /// closed ids.
    #[wasm_bindgen(js_name = streamAppend)]
    pub fn stream_append(&mut self, id: f64, chunk: &str) -> Result<JsValue, JsError> {
        let change = self
            .inner
            .stream_append(arg(id, "id")?, chunk)
            .map_err(core_err)?;
        change_to_js(Some(change))
    }

    /// Close a stream. No-op on unknown/already-closed ids. Returns nothing:
    /// the boundary contract's `streamClose(id): CoreChange | null` (the
    /// surrogate-flush change, v0.3) is produced by the TS adapter
    /// (wasm-core.ts), which is where trailing-high-surrogate buffering
    /// lives — the core itself never buffers, so it never has a flush edit
    /// to return.
    #[wasm_bindgen(js_name = streamClose)]
    pub fn stream_close(&mut self, id: f64) -> Result<(), JsError> {
        self.inner.stream_close(arg(id, "id")?);
        Ok(())
    }

    // ---- debug/verification ----------------------------------------------

    #[wasm_bindgen(js_name = getText)]
    pub fn get_text(&self) -> String {
        self.inner.get_text()
    }

    /// Document length in UTF-16 code units.
    #[wasm_bindgen(js_name = docLength)]
    pub fn doc_length(&self) -> u32 {
        self.inner.doc_len_utf16() as u32
    }

    pub fn revision(&self) -> f64 {
        self.inner.revision() as f64
    }
}

#[cfg(test)]
mod wire_format {
    //! Pins `decorations_json_string` byte-for-byte against the previous
    //! `serde_json::Value` serialization (compact separators, alphabetical
    //! key order from serde_json's default `BTreeMap`, identical
    //! optional-field omission) — the boundary's JSON.parse contract must
    //! not change shape. Runs natively (no wasm/js_sys involved).

    use oxidown_core::{BlockStyle, Decoration, MarkStyle, WidgetKind};

    use super::{decoration_json, decorations_json_string};

    fn all_variants() -> Vec<Decoration> {
        let mut v = vec![Decoration::Conceal { from: 0, to: 2 }];
        for style in [
            MarkStyle::Strong,
            MarkStyle::Em,
            MarkStyle::Code,
            MarkStyle::Delim,
            MarkStyle::Strike,
            MarkStyle::Link,
            MarkStyle::Url,
            MarkStyle::ListMarker,
        ] {
            v.push(Decoration::Mark { from: 3, to: 12345, style });
        }
        for level in 1..=6 {
            v.push(Decoration::Line { at: 7 * level as usize, level });
        }
        for style in [
            BlockStyle::BlockQuote(1),
            BlockStyle::BlockQuote(3),
            BlockStyle::CodeBlock,
            BlockStyle::CodeFence,
            BlockStyle::ThematicBreak,
            BlockStyle::ListItem(1),
            BlockStyle::ListItem(4),
        ] {
            for revealed in [false, true] {
                v.push(Decoration::Block { at: 42, style, revealed });
            }
        }
        for checked in [false, true] {
            v.push(Decoration::Widget { from: 9, to: 14, kind: WidgetKind::Task { checked } });
        }
        v.push(Decoration::Widget { from: 0, to: 2, kind: WidgetKind::Bullet });
        for (number, delim) in [(1u64, b'.'), (12u64, b')')] {
            v.push(Decoration::Widget { from: 0, to: 4, kind: WidgetKind::Ordered { number, delim } });
        }
        v
    }

    #[test]
    fn writer_matches_value_path_byte_for_byte() {
        let decos = all_variants();
        let value_path =
            serde_json::Value::Array(decos.iter().map(decoration_json).collect()).to_string();
        assert_eq!(decorations_json_string(&decos), value_path);
    }

    #[test]
    fn empty_batch_is_an_empty_array() {
        assert_eq!(decorations_json_string(&[]), "[]");
    }
}

#[cfg(test)]
mod arg_validation {
    //! `check_arg` guards every f64 position/level/id crossing the boundary:
    //! finite, integral, non-negative — or an "InvalidArgs: ..." message
    //! (previously `as usize` silently saturated negatives/NaN to 0).

    use super::check_arg;

    #[test]
    fn accepts_non_negative_integers() {
        assert_eq!(check_arg(0.0, "pos"), Ok(0));
        assert_eq!(check_arg(1.0, "pos"), Ok(1));
        assert_eq!(check_arg(4096.0, "pos"), Ok(4096));
        // Largest exactly-representable f64 integer round-trips.
        assert_eq!(check_arg(9007199254740992.0, "pos"), Ok(1 << 53));
        // Negative zero is still zero.
        assert_eq!(check_arg(-0.0, "pos"), Ok(0));
    }

    #[test]
    fn rejects_negatives() {
        for v in [-1.0, -0.5, -4096.0, f64::MIN] {
            let err = check_arg(v, "pos").unwrap_err();
            assert!(err.starts_with("InvalidArgs: "), "got {err:?}");
            assert!(err.contains("pos"), "got {err:?}");
        }
    }

    #[test]
    fn rejects_non_integral() {
        for v in [0.5, 1.25, 4096.999] {
            let err = check_arg(v, "level").unwrap_err();
            assert!(err.starts_with("InvalidArgs: "), "got {err:?}");
            assert!(err.contains("level"), "got {err:?}");
        }
    }

    #[test]
    fn rejects_non_finite() {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = check_arg(v, "id").unwrap_err();
            assert!(err.starts_with("InvalidArgs: "), "got {err:?}");
            assert!(err.contains("id"), "got {err:?}");
        }
    }

    #[test]
    fn message_names_the_argument_and_value() {
        assert_eq!(
            check_arg(-3.0, "baseRevision").unwrap_err(),
            "InvalidArgs: baseRevision must be a non-negative integer, got -3"
        );
    }
}

#[cfg(test)]
mod pos_validation {
    //! `check_pos`/`check_query_range` guard every document POSITION
    //! crossing the boundary: on wasm32 `as usize` is a 32-bit cast, so an
    //! integer above `u32::MAX` would silently truncate (2^32+6 → 6,
    //! editing the wrong range). Over-u32 positions fail with the core's
    //! own OutOfBounds message — byte-identical to what the mock
    //! (mock-core.ts `checkDocPos`/`outOfBounds`) throws for the same value.

    use super::{check_pos, check_query_range};

    #[test]
    fn accepts_the_full_u32_range() {
        assert_eq!(check_pos(0.0, "pos", 11), Ok(0));
        assert_eq!(check_pos(6.0, "pos", 11), Ok(6));
        assert_eq!(
            check_pos(4294967295.0, "pos", 11),
            Ok(u32::MAX as usize),
            "u32::MAX itself is representable and left to the core's bounds check"
        );
    }

    #[test]
    fn over_u32_positions_are_out_of_bounds_not_truncated() {
        // The probe that motivated the guard: 2^32 + 6 used to wrap to 6.
        assert_eq!(
            check_pos(4294967302.0, "pos", 11).unwrap_err(),
            "OutOfBounds: position 4294967302 beyond document length 11 (UTF-16 code units)"
        );
        // First rejected value: u32::MAX + 1 (would wrap to 0).
        assert_eq!(
            check_pos(4294967296.0, "from", 0).unwrap_err(),
            "OutOfBounds: position 4294967296 beyond document length 0 (UTF-16 code units)"
        );
    }

    #[test]
    fn malformed_positions_are_still_invalid_args() {
        assert_eq!(
            check_pos(-1.0, "pos", 11).unwrap_err(),
            "InvalidArgs: pos must be a non-negative integer, got -1"
        );
        assert_eq!(
            check_pos(1.5, "from", 11).unwrap_err(),
            "InvalidArgs: from must be a non-negative integer, got 1.5"
        );
        assert!(check_pos(f64::NAN, "pos", 11)
            .unwrap_err()
            .starts_with("InvalidArgs: "));
    }

    #[test]
    fn query_range_error_precedence_matches_the_mock() {
        // Malformed endpoints beat everything (`from` first).
        assert_eq!(
            check_query_range(-1.0, 4294967302.0, 11).unwrap_err(),
            "InvalidArgs: from must be a non-negative integer, got -1"
        );
        assert_eq!(
            check_query_range(0.0, 2.5, 11).unwrap_err(),
            "InvalidArgs: to must be a non-negative integer, got 2.5"
        );
        // Reversed range with an over-u32 endpoint: InvalidRange, not
        // OutOfBounds (the mock checks `from > to` before bounds).
        assert_eq!(
            check_query_range(4294967302.0, 5.0, 11).unwrap_err(),
            "InvalidRange: from 4294967302 > to 5"
        );
        // Ordered but over-u32: OutOfBounds reported on `to`.
        assert_eq!(
            check_query_range(0.0, 4294967302.0, 11).unwrap_err(),
            "OutOfBounds: position 4294967302 beyond document length 11 (UTF-16 code units)"
        );
        assert_eq!(
            check_query_range(4294967296.0, 4294967302.0, 3).unwrap_err(),
            "OutOfBounds: position 4294967302 beyond document length 3 (UTF-16 code units)"
        );
        // In-u32-range values pass through untouched — staleness/range/
        // bounds for these stay the core's job.
        assert_eq!(check_query_range(2.0, 9.0, 11), Ok((2, 9)));
        assert_eq!(check_query_range(9.0, 2.0, 11), Ok((9, 2)));
        assert_eq!(check_query_range(0.0, 99.0, 11), Ok((0, 99)));
    }
}
