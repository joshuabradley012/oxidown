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
//! mirror-desync-emergency handling.
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
        Decoration::Block { at, style } => {
            let mut obj = json!({
                "kind": "line",
                "at": at,
                "style": style.as_str(),
            });
            if let Some(depth) = style.depth() {
                obj["depth"] = json!(depth);
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
        },
    }
}

#[wasm_bindgen]
pub struct OxidownCore {
    inner: Editor,
}

#[wasm_bindgen]
impl OxidownCore {
    /// `replica_id` defaults to 1 (no entropy source in the core by design).
    #[wasm_bindgen(constructor)]
    pub fn new(replica_id: Option<u16>) -> OxidownCore {
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
        let now_ms = js_sys::Date::now();
        self.inner
            .apply_edit(base_revision as u64, &core_splices, origin, now_ms)
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
        from: u32,
        to: u32,
        selections: JsValue,
    ) -> Result<JsValue, JsError> {
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
            .decorations(revision as u64, from as usize, to as usize, &sels)
            .map_err(core_err)?;
        let payload = serde_json::Value::Array(decos.iter().map(decoration_json).collect());
        to_js(&payload)
    }

    #[wasm_bindgen(js_name = compositionBegin)]
    pub fn composition_begin(&mut self, from: u32, to: u32) -> Result<(), JsError> {
        self.inner
            .composition_begin(from as usize, to as usize)
            .map_err(core_err)
    }

    #[wasm_bindgen(js_name = compositionEnd)]
    pub fn composition_end(&mut self) {
        self.inner.composition_end();
    }

    // ---- anchors (v0.2) --------------------------------------------------

    /// `createAnchor(pos, bias)` — bias is `"before"` or `"after"`. Returns
    /// the anchor id.
    #[wasm_bindgen(js_name = createAnchor)]
    pub fn create_anchor(&mut self, pos: u32, bias: &str) -> Result<f64, JsError> {
        let bias = match bias {
            "before" => Bias::Before,
            "after" => Bias::After,
            other => return Err(JsError::new(&format!("InvalidBias: {other:?}"))),
        };
        self.inner
            .create_anchor(pos as usize, bias)
            .map(|id| id as f64)
            .map_err(core_err)
    }

    /// Current position of the anchor, or `null` if unresolvable
    /// (unknown/dropped id, or the document was replaced by `load`).
    #[wasm_bindgen(js_name = resolveAnchor)]
    pub fn resolve_anchor(&self, id: f64) -> JsValue {
        match self.inner.resolve_anchor(id as u64) {
            Some(pos) => JsValue::from_f64(pos as f64),
            None => JsValue::NULL,
        }
    }

    #[wasm_bindgen(js_name = dropAnchor)]
    pub fn drop_anchor(&mut self, id: f64) {
        self.inner.drop_anchor(id as u64);
    }

    // ---- commands (v0.2) -------------------------------------------------

    /// Flattened command entry point:
    /// - `command("toggleStrong"|"toggleEm"|"toggleStrike"|"toggleCode", from, to)`
    /// - `command("setHeading", pos, level)` (level 0–6; 0 = paragraph)
    /// - `command("toggleTask", pos)`
    ///
    /// Returns `CoreChange | null` (`null` when the command doesn't apply at
    /// the target). Throws on unknown names, missing arguments, or invalid
    /// positions.
    pub fn command(&mut self, name: &str, a: f64, b: Option<f64>) -> Result<JsValue, JsError> {
        let need_b = |what: &str| -> Result<f64, JsError> {
            b.ok_or_else(|| JsError::new(&format!("InvalidArgs: {name} requires {what}")))
        };
        let cmd = match name {
            "toggleStrong" | "toggleEm" | "toggleStrike" | "toggleCode" => {
                let from = a as usize;
                let to = need_b("a `to` position")? as usize;
                match name {
                    "toggleStrong" => Command::ToggleStrong { from, to },
                    "toggleEm" => Command::ToggleEm { from, to },
                    "toggleStrike" => Command::ToggleStrike { from, to },
                    _ => Command::ToggleCode { from, to },
                }
            }
            "setHeading" => {
                let level = need_b("a heading level")?;
                if !(0.0..=6.0).contains(&level) || level.fract() != 0.0 {
                    return Err(JsError::new(&format!(
                        "InvalidArgs: setHeading level must be an integer 0..=6, got {level}"
                    )));
                }
                Command::SetHeading {
                    pos: a as usize,
                    level: level as u8,
                }
            }
            "toggleTask" => Command::ToggleTask { pos: a as usize },
            other => return Err(JsError::new(&format!("InvalidCommand: {other:?}"))),
        };
        change_to_js(self.inner.command(cmd).map_err(core_err)?)
    }

    // ---- streaming (v0.2) ------------------------------------------------

    /// Open a stream at `pos`; the insertion point becomes an internal
    /// after-bias anchor. Returns the stream id.
    #[wasm_bindgen(js_name = streamOpen)]
    pub fn stream_open(&mut self, pos: u32) -> Result<f64, JsError> {
        self.inner
            .stream_open(pos as usize)
            .map(|id| id as f64)
            .map_err(core_err)
    }

    /// Append a chunk; returns `CoreChange` (splices for the view to apply
    /// under its skip annotation). Throws `UnknownStream` on never-opened or
    /// closed ids.
    #[wasm_bindgen(js_name = streamAppend)]
    pub fn stream_append(&mut self, id: f64, chunk: &str) -> Result<JsValue, JsError> {
        let change = self.inner.stream_append(id as u64, chunk).map_err(core_err)?;
        change_to_js(Some(change))
    }

    /// Close a stream. No-op on unknown/already-closed ids.
    #[wasm_bindgen(js_name = streamClose)]
    pub fn stream_close(&mut self, id: f64) {
        self.inner.stream_close(id as u64);
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
