import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { defaultKeymap } from "@codemirror/commands";
// NOTE: no history()/historyKeymap here — the Oxidown core is the historian.
import { MockCore, loadWasmCore, oxidown, type OxidownCore } from "@oxidown/view-cm6";
import { SAMPLE_DOC, largeDocFiller } from "./sample-doc";
import "./style.css";

// ---------------------------------------------------------------------------
// Perf HUD: wrap the core in a timing proxy
// ---------------------------------------------------------------------------

interface PerfSample {
  applyEdit: number | null;
  decorations: number | null;
}

function withTiming(core: OxidownCore, onSample: (s: PerfSample) => void): OxidownCore {
  const last: PerfSample = { applyEdit: null, decorations: null };
  return {
    load: (text) => core.load(text),
    applyEdit: (rev, splices, origin) => {
      const t0 = performance.now();
      const r = core.applyEdit(rev, splices, origin);
      last.applyEdit = performance.now() - t0;
      onSample(last);
      return r;
    },
    undo: () => core.undo(),
    redo: () => core.redo(),
    decorations: (rev, from, to, selections) => {
      const t0 = performance.now();
      const r = core.decorations(rev, from, to, selections);
      last.decorations = performance.now() - t0;
      onSample(last);
      return r;
    },
    compositionBegin: (from, to) => core.compositionBegin(from, to),
    compositionEnd: () => core.compositionEnd(),
    getText: () => core.getText(),
    docLength: () => core.docLength(),
    revision: () => core.revision(),
  };
}

// ---------------------------------------------------------------------------
// Core selection: ?core=wasm tries the wasm build, falls back to MockCore
// ---------------------------------------------------------------------------

const banner = document.getElementById("core-banner")!;
const hud = document.getElementById("perf-hud")!;
const sourceToggle = document.getElementById("source-toggle") as HTMLInputElement;
const loadLargeBtn = document.getElementById("load-large") as HTMLButtonElement;

const wantWasm = new URLSearchParams(location.search).get("core") === "wasm";
let rawCore: OxidownCore | null = null;
if (wantWasm) {
  rawCore = await loadWasmCore();
}
if (rawCore) {
  banner.textContent = "core: wasm";
} else {
  rawCore = new MockCore();
  if (wantWasm) {
    banner.textContent = "core: mock (wasm unavailable — fell back)";
    banner.classList.add("fallback");
  } else {
    banner.textContent = "core: mock (add ?core=wasm to try the wasm build)";
  }
}

const fmt = (ms: number | null) => (ms === null ? "—" : `${ms.toFixed(2)}ms`);
const core = withTiming(rawCore, (s) => {
  hud.textContent = `applyEdit ${fmt(s.applyEdit)} · decorations ${fmt(s.decorations)}`;
});

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

const oxidownConf = new Compartment();

const view = new EditorView({
  parent: document.getElementById("editor")!,
  state: EditorState.create({
    doc: SAMPLE_DOC,
    extensions: [
      oxidownConf.of(oxidown(core)),
      keymap.of(defaultKeymap),
      EditorView.lineWrapping,
    ],
  }),
});
view.focus();

// Source-mode toggle: swap out live-preview decorations; document syncing and
// core-driven undo/redo keep working (the plugin re-attaches without reloading
// the core, so history survives the toggle).
sourceToggle.addEventListener("change", () => {
  view.dispatch({
    effects: oxidownConf.reconfigure(
      oxidown(core, { decorations: !sourceToggle.checked }),
    ),
  });
  view.focus();
});

// Perf testing: append ~200 filler paragraphs through the normal edit path.
loadLargeBtn.addEventListener("click", () => {
  view.dispatch({
    changes: { from: view.state.doc.length, insert: largeDocFiller(200) },
  });
  loadLargeBtn.disabled = true;
  view.focus();
});
