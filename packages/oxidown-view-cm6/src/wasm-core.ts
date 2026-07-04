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
 *       undo(): { revision, splices } | null | undefined
 *       redo(): { revision, splices } | null | undefined
 *       decorations(revision, from, to, selections: JsValue): Decoration[]
 *       compositionBegin(from: number, to: number): void
 *       compositionEnd(): void
 *       getText(): string
 *       docLength(): number
 *       revision(): number
 *   - splices/selections/decorations cross the boundary as plain JS values
 *     (serde-wasm-bindgen style); errors surface as thrown JS exceptions.
 */

import type {
  Decoration,
  EditOrigin,
  OxidownCore,
  SelectionRange,
  Splice,
} from "./protocol.js";

/** Method surface we expect on the wasm-bindgen class instance. */
interface WasmCoreInstance {
  load(text: string): number;
  applyEdit(baseRevision: number, splices: unknown, origin: string): number;
  undo(): { revision: number; splices: Splice[] } | null | undefined;
  redo(): { revision: number; splices: Splice[] } | null | undefined;
  decorations(revision: number, from: number, to: number, selections: unknown): Decoration[];
  compositionBegin(from: number, to: number): void;
  compositionEnd(): void;
  getText(): string;
  docLength(): number;
  revision(): number;
}

/** Thin adapter: normalizes null/undefined and keeps payloads as plain values. */
function adaptWasmCore(inner: WasmCoreInstance): OxidownCore {
  return {
    load: (text: string) => inner.load(text),
    applyEdit: (baseRevision: number, splices: Splice[], origin: EditOrigin) =>
      inner.applyEdit(baseRevision, splices, origin),
    undo: () => inner.undo() ?? null,
    redo: () => inner.redo() ?? null,
    decorations: (revision: number, from: number, to: number, selections: SelectionRange[]) =>
      inner.decorations(revision, from, to, selections),
    compositionBegin: (from: number, to: number) => inner.compositionBegin(from, to),
    compositionEnd: () => inner.compositionEnd(),
    getText: () => inner.getText(),
    docLength: () => inner.docLength(),
    revision: () => inner.revision(),
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
