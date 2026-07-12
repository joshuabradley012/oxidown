import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { defaultKeymap } from "@codemirror/commands";
// NOTE: no history()/historyKeymap here — the Oxidown core is the historian.
import {
  applyCoreChange,
  loadWasmCore,
  oxidown,
  type OxidownCore,
} from "@oxidown/view-cm6";
import { SAMPLE_DOC, STREAM_TEXT, largeDocFiller } from "./sample-doc";
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

    // v0.2 additions: passed straight through (no timing HUD wiring for these
    // — the perf budget in docs/boundary-v0.md only covers applyEdit/decorations).
    createAnchor: (pos, bias) => core.createAnchor(pos, bias),
    resolveAnchor: (id) => core.resolveAnchor(id),
    dropAnchor: (id) => core.dropAnchor(id),
    command: (core.command as OxidownCore["command"]).bind(core),
    streamOpen: (pos) => core.streamOpen(pos),
    streamAppend: (id, chunk) => core.streamAppend(id, chunk),
    streamClose: (id) => core.streamClose(id),

    // Optional teardown: forwarded only when the wrapped core has one (the
    // wasm adapter frees its wasm-bindgen instance), so this proxy's surface
    // matches the wrapped core's exactly.
    ...(core.destroy ? { destroy: () => core.destroy?.() } : {}),
  };
}

// ---------------------------------------------------------------------------
// Core loading: the wasm core is the ONLY core (the TypeScript MockCore is
// retired). A stale `?core=wasm` in a bookmarked URL is harmless — the param
// is simply never read. If the wasm pkg is missing or fails to instantiate,
// the demo shows a clear error instead of silently degrading.
// ---------------------------------------------------------------------------

const banner = document.getElementById("core-banner")!;
const hud = document.getElementById("perf-hud")!;
const sourceToggle = document.getElementById("source-toggle") as HTMLInputElement;
const loadLargeBtn = document.getElementById("load-large") as HTMLButtonElement;
const streamBtn = document.getElementById("stream-btn") as HTMLButtonElement;
const stopStreamBtn = document.getElementById("stop-stream-btn") as HTMLButtonElement;
const streamStatus = document.getElementById("stream-status")!;

const rawCore: OxidownCore | null = await loadWasmCore();
if (!rawCore) {
  banner.textContent = "core: FAILED to load wasm";
  banner.classList.add("fallback");
  const editorEl = document.getElementById("editor")!;
  const error = document.createElement("div");
  error.className = "load-error";
  error.innerHTML =
    "<strong>The Oxidown wasm core failed to load.</strong>" +
    "<p>Build it with <code>pnpm build:wasm</code> (repo root) and reload. " +
    "Details are in the browser console.</p>";
  editorEl.appendChild(error);
  throw new Error("[oxidown demo] wasm core failed to load — no editor created");
}
banner.textContent = "core: wasm";

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

// Dev affordance: expose the view and core for debugging/automation/profiling
// (dev builds only).
if (import.meta.env.DEV) {
  (window as unknown as { __oxidownView?: EditorView }).__oxidownView = view;
  (window as unknown as { __oxidownCore?: OxidownCore }).__oxidownCore = core;
}

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

// ---------------------------------------------------------------------------
// Streaming demo (the headline feature): simulates an LLM answer arriving
// chunk by chunk via core.streamOpen/streamAppend/streamClose — no network.
// Chunk sizes (2-20 chars) and delays (15-40ms) are randomized and
// deliberately misaligned with token/markdown boundaries, so the append
// fast-path and the view's rendering have to stay correct mid-construct
// (an open fence, a half-typed `**bold`, an unfinished link, ...).
//
// THE THING TO TRY: while this streams in, click into the top of the
// document and keep typing. Your edits are never interrupted, and the
// stream keeps appending exactly where it left off — every CoreChange the
// core returns is applied via applyCoreChange with no explicit selection,
// so CM6 maps your existing cursor through the change instead of moving it.
// ---------------------------------------------------------------------------

function randInt(min: number, max: number): number {
  return min + Math.floor(Math.random() * (max - min + 1));
}

let streamId: number | null = null;
let streamTimer: ReturnType<typeof setTimeout> | null = null;
let streamChunks = 0;
let streamStartedAt = 0;

function streamRateLabel(): string {
  const elapsedSec = (performance.now() - streamStartedAt) / 1000;
  const rate = elapsedSec > 0 ? streamChunks / elapsedSec : 0;
  return `${streamChunks} chunks · ~${rate.toFixed(1)}/s`;
}

function endStream(status: "done" | "stopped") {
  if (streamTimer !== null) {
    clearTimeout(streamTimer);
    streamTimer = null;
  }
  if (streamId !== null) {
    try {
      // streamClose may return one final CoreChange (the U+FFFD flush of a
      // surrogate withheld from the last chunk): route it into the editor
      // exactly like a streamAppend result, or the view falls one code unit
      // behind the core doc.
      const change = core.streamClose(streamId);
      if (change) applyCoreChange(view, change, "oxidown.stream");
    } catch (err) {
      // endStream must be safe to call on a broken core (e.g. right after a
      // streamAppend threw) — log and move on so the UI still resets.
      console.error("[oxidown demo] streamClose failed:", err);
    }
    streamId = null;
  }
  streamBtn.disabled = false;
  stopStreamBtn.disabled = true;
  streamStatus.textContent = `stream: ${status} (${streamRateLabel()})`;
}

streamBtn.addEventListener("click", () => {
  if (streamId !== null) return; // already streaming
  streamId = core.streamOpen(view.state.doc.length);
  streamBtn.disabled = true;
  stopStreamBtn.disabled = false;
  streamChunks = 0;
  streamStartedAt = performance.now();
  streamStatus.textContent = "stream: opening…";

  let offset = 0;

  const pump = () => {
    if (streamId === null) return; // stopped mid-flight
    if (offset >= STREAM_TEXT.length) {
      endStream("done");
      return;
    }
    const size = randInt(2, 20);
    const chunk = STREAM_TEXT.slice(offset, offset + size);
    offset += chunk.length;
    streamChunks++;
    try {
      const change = core.streamAppend(streamId, chunk);
      applyCoreChange(view, change, "oxidown.stream");
    } catch (err) {
      console.error("[oxidown demo] streamAppend failed — stopping the stream:", err);
      endStream("stopped");
      return;
    }
    streamStatus.textContent = `stream: streaming (${streamRateLabel()})`;
    streamTimer = setTimeout(pump, randInt(15, 40));
  };
  pump();
});

stopStreamBtn.addEventListener("click", () => endStream("stopped"));
