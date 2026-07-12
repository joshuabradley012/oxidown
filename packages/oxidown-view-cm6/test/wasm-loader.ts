/**
 * Test-only loader for the real Rust/wasm core, shared by every vitest suite
 * that asserts contract BEHAVIOR (core-contract.test.ts, wasm-core-boundary.test.ts's
 * precedence probes, and any future suite that needs a live core). The wasm
 * core is now the ONLY implementation of docs/boundary-v0.md — there is no
 * TypeScript reference core to fall back to — so a missing/unbuilt package is
 * a hard test failure, never a silent skip: a skip would let a whole suite go
 * green without ever exercising the real core.
 *
 * `crates/oxidown-wasm/pkg` is built with `wasm-pack --target web` (see
 * package.json's `build:wasm` / ci.yml's "Wasm build + size budget" job),
 * which targets the browser: its default export is an async `init()` that
 * fetches the `.wasm` file, unavailable for a `file://` path under Node. This
 * loader instead uses the SAME pkg's `initSync` export (also part of the
 * `--target web` output) directly over the bytes read from disk — no second
 * `nodejs`-target build, no fetch polyfill — and wraps the resulting class
 * with the PRODUCTION adapter (`adaptWasmCore` from `../src/wasm-core.ts`),
 * so tests exercise the exact same adapter code path the browser view uses.
 */
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { adaptWasmCore } from "../src/wasm-core.js";
import type { OxidownCore } from "../src/protocol.js";

/**
 * Resolve a repo path relative to this module. Under vitest's `node`
 * environment `import.meta.url` is a `file:` URL; under the `jsdom`
 * environment it is an `http:`-flavored URL whose pathname is still the
 * absolute filesystem path — handle both.
 */
function resolveFromHere(relative: string): string {
  const url = new URL(relative, import.meta.url);
  return url.protocol === "file:" ? fileURLToPath(url) : decodeURIComponent(url.pathname);
}

const PKG_JS_PATH = resolveFromHere("../../../crates/oxidown-wasm/pkg/oxidown_wasm.js");
const PKG_WASM_PATH = resolveFromHere("../../../crates/oxidown-wasm/pkg/oxidown_wasm_bg.wasm");

const BUILD_HINT = "run `pnpm build:wasm` from the repo root, then re-run the tests";

interface WasmPkgModule {
  initSync: (arg: { module: BufferSource }) => unknown;
  OxidownCore: new () => Parameters<typeof adaptWasmCore>[0];
}

let cachedModule: WasmPkgModule | null = null;

/**
 * Loads (once per test process) the built wasm pkg and returns a factory
 * that constructs fresh wasm-backed `OxidownCore` instances — one per call.
 *
 * Throws (does not skip) with a message naming `pnpm build:wasm` when the
 * package is missing, or when it's present but fails to load/instantiate.
 */
export async function loadWasmCoreFactory(): Promise<() => OxidownCore> {
  if (!cachedModule) {
    if (!existsSync(PKG_JS_PATH) || !existsSync(PKG_WASM_PATH)) {
      throw new Error(
        `[wasm-loader] crates/oxidown-wasm/pkg is missing (looked for ${PKG_JS_PATH}) — ` +
          `${BUILD_HINT}.`,
      );
    }
    let mod: WasmPkgModule;
    try {
      mod = (await import(/* @vite-ignore */ PKG_JS_PATH)) as WasmPkgModule;
      mod.initSync({ module: readFileSync(PKG_WASM_PATH) });
    } catch (err) {
      throw new Error(
        `[wasm-loader] crates/oxidown-wasm/pkg is present but failed to load/initialize: ` +
          `${String(err)} — the pkg may be stale or partially built; ${BUILD_HINT}.`,
      );
    }
    cachedModule = mod;
  }
  const mod = cachedModule;
  return () => adaptWasmCore(new mod.OxidownCore());
}
