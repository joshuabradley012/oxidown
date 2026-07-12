/**
 * ADAPTER STUB for the real Rust/wasm core — ALL GUESSING IS ISOLATED IN THIS
 * FILE. The orchestrator will reconcile this with the actual wasm-bindgen API
 * once `crates/oxidown-wasm` builds.
 *
 * Assumed wasm-pack output (`--target web`, out-dir `pkg`):
 *   - module `crates/oxidown-wasm/pkg/oxidown_wasm.js`
 *   - default export = async init function (instantiates the .wasm)
 *   - an exported class named `OxidownCore` (or `WasmCore` / `Core`) with a
 *     zero-arg constructor and methods matching the boundary protocol's names
 *     (wasm-bindgen camelCases snake_case Rust methods, or the crate uses
 *     `js_name` to the same effect):
 *       load(text: string): number
 *       applyEdit(baseRevision: number, splices: JsValue /* Splice[] *\/, origin: string): number
 *       undo(): CoreChange | null | undefined
 *       redo(): CoreChange | null | undefined
 *       decorations(revision, from, to, selections: JsValue): Decoration[]
 *       compositionBegin(from: number, to: number): void
 *       compositionEnd(): void
 *       getText(): string
 *       docLength(): number
 *       revision(): number
 *
 *   v0.2 (M1) additions — same guessing style, assumed camelCase 1:1 with the
 *   contract (docs/boundary-v0.md "v0.2 additions"):
 *       createAnchor(pos: number, bias: string): number
 *       resolveAnchor(id: number): number | null | undefined
 *       dropAnchor(id: number): void
 *       command(name: string, a: number, b?: number): CoreChange | null | undefined
 *         — TS's overloaded `command(name, from, to)` / `command(name,
 *           pos, level)` / `command(name, pos)` signatures don't exist at
 *           the JS/wasm-bindgen call boundary; we ASSUME the exported Rust
 *           fn is a single method taking the variant's positional args with
 *           the trailing one optional (`b?`), dispatched by `name` Rust-side.
 *           If the real crate instead exports one method per command name
 *           (`toggleStrong`, `setHeading`, ...), only this file's `command`
 *           wrapper needs to change.
 *       streamOpen(pos: number): number
 *       streamAppend(id: number, chunk: string): CoreChange
 *       streamClose(id: number): void
 *         — the PROTOCOL's streamClose returns `CoreChange | null` (the
 *           U+FFFD flush of a withheld surrogate); that flush lives entirely
 *           in this adapter (it forwards one final streamAppend and returns
 *           its CoreChange), so the wasm instance's own streamClose stays
 *           void.
 *
 *   splices/selections/decorations cross the boundary as one JSON string per
 *   call (`serde_json` Rust-side, `js_sys::JSON` JS-side — see the crate doc
 *   in crates/oxidown-wasm/src/lib.rs), so callers still see plain JS values;
 *   errors surface as thrown JS exceptions whose messages start with the
 *   error name ("StaleRevision: ...", "OutOfBounds: ...", "InvalidArgs: ...").
 *
 * Adapter responsibilities beyond forwarding:
 *   - Unpaired-surrogate policy, enforced JS-SIDE before crossing the
 *     boundary (wasm-bindgen's &str conversion silently corrupts lone
 *     surrogates to U+FFFD — the core document must never contain one):
 *     `load` and `applyEdit` insert texts containing an unpaired surrogate
 *     throw "InvalidPayload: ...". `streamAppend` buffers a TRAILING lone
 *     high surrogate per stream (a producer chunking at fixed UTF-16 lengths
 *     can split a surrogate pair across chunks); the withheld code unit is
 *     prepended to the next chunk, and `streamClose` flushes a still-pending
 *     one as U+FFFD before closing — RETURNING that flush's CoreChange (null
 *     when nothing was pending) so the view can apply it and stay in sync
 *     with the core document. A lone surrogate anywhere else in a
 *     chunk throws "InvalidPayload: ..."; the stream's buffer is cleared
 *     whenever an append throws, and ALL streams' buffers are cleared by
 *     `load` (the core clears its streams then, and the buffer must follow).
 *     `applyEdit` replicates the mock's malformed-baseRevision and staleness
 *     checks ahead of its surrogate check so the cross-core validation
 *     precedence (revision checks before payload checks) holds even though
 *     the surrogate check itself cannot cross the boundary.
 *   - `destroy()`: frees the underlying wasm-bindgen instance (its `free()`
 *     releases the Rust-side allocation); idempotent, guards double-free.
 */

import type {
  CoreChange,
  Decoration,
  EditOrigin,
  OxidownCore,
  SelectionRange,
  Splice,
} from "./protocol.js";

// ---------------------------------------------------------------------------
// Unpaired-surrogate guards — pure functions (exported for unit tests; see
// test/wasm-core-boundary.test.ts). Positions/lengths are UTF-16 code units,
// so "well-formed" here means: every 0xD800–0xDBFF unit is immediately
// followed by a 0xDC00–0xDFFF unit, and no 0xDC00–0xDFFF unit stands alone.
// ---------------------------------------------------------------------------

const isHighSurrogate = (unit: number): boolean => unit >= 0xd800 && unit <= 0xdbff;
const isLowSurrogate = (unit: number): boolean => unit >= 0xdc00 && unit <= 0xdfff;

/** True if `text` contains a lone (unpaired) surrogate code unit. */
export function hasUnpairedSurrogate(text: string): boolean {
  for (let i = 0; i < text.length; i++) {
    const unit = text.charCodeAt(i);
    if (isHighSurrogate(unit)) {
      if (i + 1 < text.length && isLowSurrogate(text.charCodeAt(i + 1))) {
        i++; // valid pair — skip the low half
      } else {
        return true;
      }
    } else if (isLowSurrogate(unit)) {
      return true;
    }
  }
  return false;
}

/**
 * Throw `InvalidPayload: ${what} contains an unpaired surrogate` if `text`
 * is not well-formed UTF-16. Called BEFORE any text crosses the wasm
 * boundary, where lone surrogates would silently become U+FFFD.
 */
export function assertNoUnpairedSurrogates(text: string, what: string): void {
  if (hasUnpairedSurrogate(text)) {
    throw new Error(`InvalidPayload: ${what} contains an unpaired surrogate`);
  }
}

/**
 * Split a stream chunk into the part safe to send now and a withheld
 * TRAILING lone high surrogate (at most one code unit) that may pair with
 * the start of the next chunk. Any other lone surrogate throws
 * "InvalidPayload: ..." — only a trailing high surrogate can still be
 * completed by future input.
 */
export function splitTrailingHighSurrogate(chunk: string): { send: string; pending: string } {
  let send = chunk;
  let pending = "";
  // A high surrogate as the final unit is unpaired by construction.
  if (chunk.length > 0 && isHighSurrogate(chunk.charCodeAt(chunk.length - 1))) {
    send = chunk.slice(0, -1);
    pending = chunk.slice(-1);
  }
  assertNoUnpairedSurrogates(send, "chunk");
  return { send, pending };
}

/**
 * Per-stream reassembly buffer for surrogate pairs split across
 * `streamAppend` chunks (a producer chunking at fixed UTF-16 lengths splits
 * emoji). Keyed by stream id; holds at most one pending high surrogate per
 * stream. Exported for unit tests.
 */
export class StreamSurrogateBuffer {
  private pending = new Map<number, string>();

  /**
   * Absorb `chunk` and return the text to forward to the core right now
   * (possibly `""`, which the core treats as a no-op append). Throws
   * "InvalidPayload: ..." on a lone surrogate anywhere but the chunk's tail;
   * the stream's buffer is cleared before the throw propagates.
   */
  push(id: number, chunk: string): string {
    const combined = (this.pending.get(id) ?? "") + chunk;
    let send: string;
    let pending: string;
    try {
      ({ send, pending } = splitTrailingHighSurrogate(combined));
    } catch (err) {
      this.pending.delete(id);
      throw err;
    }
    if (pending) {
      this.pending.set(id, pending);
    } else {
      this.pending.delete(id);
    }
    return send;
  }

  /**
   * Drain the stream's buffer for `streamClose`: returns `"\uFFFD"` if a
   * high surrogate was still pending (it can never be completed now), else
   * `""`. Always clears the buffer.
   */
  takeFlush(id: number): string {
    const pending = this.pending.get(id) ?? "";
    this.pending.delete(id);
    return pending ? "\uFFFD" : "";
  }

  /** Drop any pending unit for `id` (stream errored). */
  clear(id: number): void {
    this.pending.delete(id);
  }

  /**
   * Drop EVERY stream's pending unit. Called on `load()`: the core clears
   * all streams when the document is replaced, so the adapter's buffer must
   * die with them — otherwise a later `streamClose` on a pre-load id would
   * flush a stale U+FFFD append into a dead stream and throw UnknownStream
   * out of a call the contract pins as a no-op returning null.
   */
  clearAll(): void {
    this.pending.clear();
  }
}

/** Method surface we expect on the wasm-bindgen class instance. */
interface WasmCoreInstance {
  /** wasm-bindgen's per-instance deallocator. */
  free(): void;
  load(text: string): number;
  applyEdit(baseRevision: number, splices: unknown, origin: string): number;
  undo(): CoreChange | null | undefined;
  redo(): CoreChange | null | undefined;
  decorations(revision: number, from: number, to: number, selections: unknown): Decoration[];
  compositionBegin(from: number, to: number): void;
  compositionEnd(): void;
  getText(): string;
  docLength(): number;
  revision(): number;

  // v0.2 additions
  createAnchor(pos: number, bias: string): number;
  resolveAnchor(id: number): number | null | undefined;
  dropAnchor(id: number): void;
  command(name: string, a: number, b?: number): CoreChange | null | undefined;
  streamOpen(pos: number): number;
  streamAppend(id: number, chunk: string): CoreChange;
  streamClose(id: number): void;
}

/**
 * Thin adapter: normalizes null/undefined, keeps payloads as plain values,
 * and enforces the unpaired-surrogate policy JS-side (see header). Exported
 * for unit tests (which pass a fake `WasmCoreInstance`); production callers
 * use `loadWasmCore`.
 */
export function adaptWasmCore(inner: WasmCoreInstance): OxidownCore {
  const streamBuffer = new StreamSurrogateBuffer();
  let destroyed = false;
  return {
    load: (text: string) => {
      assertNoUnpairedSurrogates(text, "text");
      // Core-side load clears every stream; the adapter's per-stream
      // surrogate buffer must be cleared in the same breath (see
      // StreamSurrogateBuffer.clearAll) — a stale pending unit would turn a
      // post-load streamClose (contract: no-op returning null) into an
      // UnknownStream throw via its U+FFFD flush.
      streamBuffer.clearAll();
      return inner.load(text);
    },
    applyEdit: (baseRevision: number, splices: Splice[], origin: EditOrigin) => {
      // Validation precedence parity with the mock (mock-core.ts
      // `applyEdit`): malformed baseRevision → staleness → payload checks.
      // The unpaired-surrogate check MUST run JS-side (before the strings
      // cross the boundary, where lone surrogates silently corrupt), so the
      // two revision checks it must not preempt are replicated here, message
      // parity included; the wasm entry point re-runs both harmlessly. A
      // simultaneously-stale AND payload-malformed call must be
      // StaleRevision (desync-resync class), never InvalidPayload (consumed
      // no-op class), on both cores.
      if (!Number.isInteger(baseRevision) || baseRevision < 0) {
        throw new Error(
          `InvalidArgs: baseRevision must be a non-negative integer, got ${baseRevision}`,
        );
      }
      const current = inner.revision();
      if (baseRevision !== current) {
        throw new Error(
          `StaleRevision: core is at revision ${current}, caller passed ${baseRevision}`,
        );
      }
      for (const splice of splices) {
        assertNoUnpairedSurrogates(splice.insert, "splice insert");
      }
      return inner.applyEdit(baseRevision, splices, origin);
    },
    undo: () => inner.undo() ?? null,
    redo: () => inner.redo() ?? null,
    decorations: (revision: number, from: number, to: number, selections: SelectionRange[]) =>
      inner.decorations(revision, from, to, selections),
    compositionBegin: (from: number, to: number) => inner.compositionBegin(from, to),
    compositionEnd: () => inner.compositionEnd(),
    getText: () => inner.getText(),
    docLength: () => inner.docLength(),
    revision: () => inner.revision(),

    createAnchor: (pos: number, bias: "before" | "after") => inner.createAnchor(pos, bias),
    resolveAnchor: (id: number) => inner.resolveAnchor(id) ?? null,
    dropAnchor: (id: number) => inner.dropAnchor(id),

    // Overloaded at the TS surface; a single (name, a, b?) call underneath.
    command: ((name: string, a: number, b?: number) =>
      inner.command(name, a, b) ?? null) as OxidownCore["command"],

    streamOpen: (pos: number) => inner.streamOpen(pos),
    streamAppend: (id: number, chunk: string) => {
      // `push` withholds a trailing lone high surrogate (and prepends one
      // withheld earlier); `send` may be "" — the core no-ops on it and
      // returns an unchanged CoreChange. Any error (validation or core)
      // drops the stream's pending unit.
      const send = streamBuffer.push(id, chunk); // clears its buffer on throw
      try {
        return inner.streamAppend(id, send);
      } catch (err) {
        streamBuffer.clear(id);
        throw err;
      }
    },
    streamClose: (id: number): CoreChange | null => {
      // A still-pending high surrogate can never be completed: flush it as
      // U+FFFD (one final append) before closing, and RETURN the flush's
      // CoreChange so the caller can apply it to the view (dropping it would
      // silently desync the core doc from the view by one code unit). Close
      // even if the flush throws; null when nothing was pending.
      const flush = streamBuffer.takeFlush(id);
      let change: CoreChange | null = null;
      try {
        if (flush) change = inner.streamAppend(id, flush);
      } finally {
        inner.streamClose(id);
      }
      return change;
    },

    destroy: () => {
      if (destroyed) return; // guard double-free
      destroyed = true;
      inner.free();
    },
  };
}

// Relative to this module (src/ or dist/, both two levels below the repo
// root's packages/ dir): ../../../crates/oxidown-wasm/pkg/oxidown_wasm.js
const WASM_PKG_PATH = "../../../crates/oxidown-wasm/pkg/oxidown_wasm.js";

/**
 * Try to load the real wasm core. Returns null (with a console.warn) when the
 * wasm pkg has not been built yet — callers should fall back to MockCore.
 */
export async function loadWasmCore(): Promise<OxidownCore | null> {
  try {
    const mod: Record<string, unknown> = await import(
      /* @vite-ignore */ WASM_PKG_PATH
    );
    // wasm-pack --target web exposes a default init() that must run first.
    if (typeof mod.default === "function") {
      await (mod.default as () => Promise<unknown>)();
    } else if (typeof mod.init === "function") {
      await (mod.init as () => Promise<unknown>)();
    }
    const CoreClass = (mod.OxidownCore ?? mod.WasmCore ?? mod.Core) as
      | (new () => WasmCoreInstance)
      | undefined;
    if (!CoreClass) {
      console.warn(
        "[oxidown] wasm pkg loaded but no OxidownCore/WasmCore/Core export found; falling back",
      );
      return null;
    }
    return adaptWasmCore(new CoreClass());
  } catch (err) {
    console.warn(
      "[oxidown] wasm core not available (build crates/oxidown-wasm with wasm-pack to enable it):",
      err,
    );
    return null;
  }
}
