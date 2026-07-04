//! wasm-bindgen boundary for `oxidown-core`, exposing the `OxidownCore`
//! TypeScript interface from docs/boundary-v0.md. Positions are already
//! UTF-16 code units at the core API, so this is a thin shim.
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
//! Timestamps: the core never reads clocks (std::time panics on
//! wasm32-unknown-unknown); `applyEdit` injects `js_sys::Date::now()`.
//!
//! Errors: every core `Err` becomes a thrown JS exception whose message
//! starts with the error name (e.g. "StaleRevision: ..."), per the contract's
//! mirror-desync-emergency handling.
//!
//! No `rand`/`getrandom`: `replica_id` is a constructor parameter
//! (default 1).

use oxidown_core::{CoreError, Decoration, EditOrigin, Editor, HistoryResult, SelectionRange, Splice};
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

fn history_to_js(result: Option<HistoryResult>) -> Result<JsValue, JsError> {
    match result {
        None => Ok(JsValue::NULL),
        Some(r) => to_js(&json!({
            "revision": r.revision,
            "splices": r.splices.iter().map(|s| json!({
                "at": s.at,
                "delete": s.delete,
                "insert": s.insert,
            })).collect::<Vec<_>>(),
        })),
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

    /// `{ revision, splices } | null` — splices in current-doc coordinates.
    pub fn undo(&mut self) -> Result<JsValue, JsError> {
        history_to_js(self.inner.undo())
    }

    /// `{ revision, splices } | null` — splices in current-doc coordinates.
    pub fn redo(&mut self) -> Result<JsValue, JsError> {
        history_to_js(self.inner.redo())
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
