/**
 * JS-side boundary guards of the wasm adapter — everything testable without
 * the wasm binary (src/wasm-core.ts exports its pure pieces for exactly
 * this). The invariant under test: core text never contains an unpaired
 * surrogate, because wasm-bindgen's &str conversion would silently corrupt
 * one to U+FFFD instead of erroring.
 *
 * The adapter-level cases drive `adaptWasmCore` against a fake
 * `WasmCoreInstance` that records what actually crosses the "boundary".
 *
 * The one exception to "without the wasm binary": the S6 probes at the
 * bottom pin the wasm layer's `decorations()` validation PRECEDENCE against
 * the mock's canonical order, which only means anything against the real
 * crate — they load `crates/oxidown-wasm/pkg` exactly like the conformance
 * suite (skip locally when unbuilt, mandatory in CI).
 */
import { describe, expect, it } from "vitest";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  StreamSurrogateBuffer,
  adaptWasmCore,
  assertNoUnpairedSurrogates,
  hasUnpairedSurrogate,
  splitTrailingHighSurrogate,
} from "../src/wasm-core";
import { MockCore } from "../src/mock-core";
import type { CoreChange, OxidownCore } from "../src/protocol";

const HIGH = "\uD83D"; // first half of 😀 (U+1F600 = D83D DE00)
const LOW = "\uDE00";
const EMOJI = HIGH + LOW;

/**
 * Fake wasm instance: records every string that would cross the boundary
 * and maintains an append-only "document" for the streaming cases.
 * `failNextStreamAppend` arms a one-shot throw from INSIDE the instance's
 * streamAppend (recorded as having crossed, mutating nothing) — the core-
 * side failure mode (e.g. UnknownStream) as opposed to the adapter's own
 * validation throws.
 */
function fakeInner() {
  let doc = "";
  let revision = 0;
  let streamAppendError: Error | null = null;
  const calls: { method: string; text: string }[] = [];
  const change = (at: number, insert: string): CoreChange => ({
    revision: ++revision,
    splices: [{ at, delete: 0, insert }],
    selection: null,
  });
  return {
    doc: () => doc,
    calls,
    failNextStreamAppend: (err: Error) => {
      streamAppendError = err;
    },
    inner: {
      free: () => {
        calls.push({ method: "free", text: "" });
      },
      load: (text: string) => {
        calls.push({ method: "load", text });
        doc = text;
        return ++revision;
      },
      applyEdit: () => ++revision,
      undo: () => null,
      redo: () => null,
      decorations: () => [],
      compositionBegin: () => {},
      compositionEnd: () => {},
      getText: () => doc,
      docLength: () => doc.length,
      revision: () => revision,
      createAnchor: () => 1,
      resolveAnchor: () => null,
      dropAnchor: () => {},
      command: () => null,
      streamOpen: () => 1,
      streamAppend: (_id: number, chunk: string) => {
        calls.push({ method: "streamAppend", text: chunk });
        if (streamAppendError) {
          const err = streamAppendError;
          streamAppendError = null;
          throw err; // before mutating: a refused append enters no document
        }
        const at = doc.length;
        doc += chunk;
        return change(at, chunk);
      },
      streamClose: (id: number) => {
        calls.push({ method: "streamClose", text: String(id) });
      },
    },
  };
}

describe("hasUnpairedSurrogate / assertNoUnpairedSurrogates", () => {
  it("accepts well-formed text, including paired surrogates", () => {
    for (const text of ["", "plain ascii", `a${EMOJI}b`, EMOJI.repeat(3), "日本語"]) {
      expect(hasUnpairedSurrogate(text)).toBe(false);
      expect(() => assertNoUnpairedSurrogates(text, "text")).not.toThrow();
    }
  });

  it("detects lone surrogates in every position", () => {
    for (const text of [HIGH, LOW, `a${HIGH}b`, `a${LOW}b`, `${EMOJI}${HIGH}`, `${LOW}${EMOJI}`, HIGH + HIGH, LOW + LOW]) {
      expect(hasUnpairedSurrogate(text)).toBe(true);
    }
  });

  it("throws with the InvalidPayload prefix, naming the payload", () => {
    expect(() => assertNoUnpairedSurrogates(HIGH, "text")).toThrow(
      "InvalidPayload: text contains an unpaired surrogate",
    );
  });
});

describe("splitTrailingHighSurrogate", () => {
  it("passes well-formed chunks through untouched", () => {
    expect(splitTrailingHighSurrogate(`a${EMOJI}b`)).toEqual({ send: `a${EMOJI}b`, pending: "" });
    expect(splitTrailingHighSurrogate("")).toEqual({ send: "", pending: "" });
  });

  it("withholds a trailing lone high surrogate", () => {
    expect(splitTrailingHighSurrogate(`a${HIGH}`)).toEqual({ send: "a", pending: HIGH });
    // Nothing left to send when the chunk IS the high surrogate.
    expect(splitTrailingHighSurrogate(HIGH)).toEqual({ send: "", pending: HIGH });
  });

  it("does not withhold the low half of a complete trailing pair", () => {
    expect(splitTrailingHighSurrogate(`ab${EMOJI}`)).toEqual({ send: `ab${EMOJI}`, pending: "" });
  });

  it("throws InvalidPayload on lone surrogates anywhere else", () => {
    for (const chunk of [`a${LOW}b`, `a${HIGH}b`, LOW, HIGH + HIGH, `${LOW}${EMOJI}`]) {
      expect(() => splitTrailingHighSurrogate(chunk)).toThrow(/^InvalidPayload: /);
    }
  });
});

describe("StreamSurrogateBuffer", () => {
  it("reassembles an emoji split across two chunks", () => {
    const buf = new StreamSurrogateBuffer();
    expect(buf.push(1, `a${HIGH}`)).toBe("a");
    expect(buf.push(1, `${LOW}b`)).toBe(`${EMOJI}b`);
  });

  it("keys pending units by stream id", () => {
    const buf = new StreamSurrogateBuffer();
    expect(buf.push(1, HIGH)).toBe("");
    expect(buf.push(2, "x")).toBe("x"); // stream 2 unaffected by stream 1's pending unit
    expect(buf.push(1, LOW)).toBe(EMOJI);
  });

  it("flushes a pending high surrogate as U+FFFD exactly once", () => {
    const buf = new StreamSurrogateBuffer();
    buf.push(7, `x${HIGH}`);
    expect(buf.takeFlush(7)).toBe("�");
    expect(buf.takeFlush(7)).toBe(""); // cleared
  });

  it("clears the pending unit when a push throws", () => {
    const buf = new StreamSurrogateBuffer();
    buf.push(1, HIGH);
    // pending HIGH + "z" = interior lone high surrogate -> invalid
    expect(() => buf.push(1, "z")).toThrow(/^InvalidPayload: /);
    expect(buf.takeFlush(1)).toBe(""); // buffer was dropped, nothing to flush
  });
});

describe("adaptWasmCore boundary guards (fake wasm instance)", () => {
  it("KILLER REPRO: `a😀b` chunked at fixed UTF-16 lengths arrives intact", () => {
    const { inner, doc } = fakeInner();
    const core = adaptWasmCore(inner);
    const id = core.streamOpen(0);
    // length-2 chunking of "a😀b" splits the emoji: ["a\uD83D", "\uDE00b"]
    core.streamAppend(id, `a${HIGH}`);
    core.streamAppend(id, `${LOW}b`);
    // No pending unit at close: nothing to flush, so nothing to return.
    expect(core.streamClose(id)).toBeNull();
    expect(doc()).toBe(`a${EMOJI}b`);
    expect(doc()).not.toContain("�");
  });

  it("flushes a still-pending high surrogate as U+FFFD on streamClose and returns the flush's CoreChange", () => {
    const { inner, doc, calls } = fakeInner();
    const core = adaptWasmCore(inner);
    const id = core.streamOpen(0);
    core.streamAppend(id, `x${HIGH}`);
    const flush = core.streamClose(id);
    expect(doc()).toBe("x�");
    // The flush is one final append, then the close.
    expect(calls.map((c) => c.method)).toEqual(["streamAppend", "streamAppend", "streamClose"]);
    // The final append's CoreChange is returned to the caller (dropping it
    // would leave the view one code unit behind the core doc).
    expect(flush).not.toBeNull();
    expect(flush!.splices).toEqual([{ at: 1, delete: 0, insert: "�" }]);
  });

  it("streamAppend throws InvalidPayload on an interior lone surrogate; nothing crosses", () => {
    const { inner, doc } = fakeInner();
    const core = adaptWasmCore(inner);
    const id = core.streamOpen(0);
    expect(() => core.streamAppend(id, `a${LOW}b`)).toThrow(
      "InvalidPayload: chunk contains an unpaired surrogate",
    );
    expect(doc()).toBe("");
  });

  it("load rejects lone surrogates before crossing the boundary", () => {
    const { inner, calls } = fakeInner();
    const core = adaptWasmCore(inner);
    expect(() => core.load(`bad${HIGH}`)).toThrow(
      "InvalidPayload: text contains an unpaired surrogate",
    );
    expect(calls).toEqual([]); // never reached the wasm instance
    expect(core.load(`ok${EMOJI}`)).toBeGreaterThan(0);
  });

  it("applyEdit rejects a lone surrogate in any splice insert", () => {
    const { inner } = fakeInner();
    const core = adaptWasmCore(inner);
    core.load("abc");
    expect(() =>
      core.applyEdit(1, [{ at: 0, delete: 0, insert: "fine" }, { at: 1, delete: 0, insert: LOW }], "user"),
    ).toThrow("InvalidPayload: splice insert contains an unpaired surrogate");
    // Well-formed inserts still go through.
    expect(core.applyEdit(1, [{ at: 0, delete: 0, insert: EMOJI }], "user")).toBeGreaterThan(0);
  });

  it("load clears every stream's pending surrogate (streamClose after load stays a null no-op)", () => {
    const { inner, calls } = fakeInner();
    const core = adaptWasmCore(inner);
    const id = core.streamOpen(0);
    core.streamAppend(id, `a${HIGH}`); // trailing high surrogate withheld in the adapter
    core.load("fresh"); // core-side load clears streams; the buffer must follow
    // No stale U+FFFD flush is appended to the (now dead) stream id.
    expect(core.streamClose(id)).toBeNull();
    expect(calls.filter((c) => c.method === "streamAppend").map((c) => c.text)).toEqual(["a"]);
  });

  it("applyEdit checks baseRevision/staleness BEFORE the surrogate payload check (mock precedence)", () => {
    const { inner } = fakeInner();
    const core = adaptWasmCore(inner);
    core.load("abc"); // fake revision -> 1
    // Malformed baseRevision wins over the bad payload.
    expect(() => core.applyEdit(-1, [{ at: 0, delete: 0, insert: HIGH }], "user")).toThrow(
      "InvalidArgs: baseRevision must be a non-negative integer, got -1",
    );
    // Staleness wins over the bad payload.
    expect(() => core.applyEdit(7, [{ at: 0, delete: 0, insert: HIGH }], "user")).toThrow(
      "StaleRevision: core is at revision 1, caller passed 7",
    );
    // A current revision still hits the surrogate check.
    expect(() => core.applyEdit(1, [{ at: 0, delete: 0, insert: HIGH }], "user")).toThrow(
      "InvalidPayload: splice insert contains an unpaired surrogate",
    );
  });

  it("destroy frees the wasm instance exactly once", () => {
    const { inner, calls } = fakeInner();
    const core = adaptWasmCore(inner);
    core.destroy?.();
    core.destroy?.(); // double-destroy guarded
    expect(calls.filter((c) => c.method === "free")).toHaveLength(1);
  });

  it("clears the pending surrogate when the INNER streamAppend throws (core-side failure, not just validation)", () => {
    const { inner, calls, failNextStreamAppend } = fakeInner();
    const core = adaptWasmCore(inner);
    const id = core.streamOpen(0);
    // push withholds the trailing HIGH and forwards "a" — which the (armed)
    // core rejects. The adapter's catch must drop the withheld unit: the
    // append failed, so nothing can ever legitimately pair with it.
    failNextStreamAppend(new Error("UnknownStream: stream 1 is unknown or already closed"));
    expect(() => core.streamAppend(id, `a${HIGH}`)).toThrow(/^UnknownStream: /);
    // No stale pending unit survives: close finds nothing to flush (a stale
    // HIGH would surface here as a second, U+FFFD-carrying append).
    expect(core.streamClose(id)).toBeNull();
    expect(calls.filter((c) => c.method === "streamAppend").map((c) => c.text)).toEqual(["a"]);
  });

  it("streamClose closes and returns null even when the U+FFFD flush append throws", () => {
    const { inner, doc, calls, failNextStreamAppend } = fakeInner();
    const core = adaptWasmCore(inner);
    const id = core.streamOpen(0);
    core.streamAppend(id, `x${HIGH}`); // HIGH withheld in the adapter
    failNextStreamAppend(new Error("UnknownStream: stream 1 is unknown or already closed"));
    // Contract: streamClose NEVER throws. The failed flush entered no
    // document, so returning null desyncs nothing.
    expect(core.streamClose(id)).toBeNull();
    // The flush was attempted, then the close still crossed.
    expect(calls.map((c) => c.method)).toEqual(["streamAppend", "streamAppend", "streamClose"]);
    expect(doc()).toBe("x");
    // The dropped unit is gone for good: a later close flushes nothing.
    expect(core.streamClose(id)).toBeNull();
    expect(calls.filter((c) => c.method === "streamAppend")).toHaveLength(2);
  });

  it("a reused stream id never inherits a stale pending high surrogate", () => {
    const { inner, calls } = fakeInner();
    const core = adaptWasmCore(inner);
    // Session 1 on id 1 ends with a withheld HIGH; close flushes it (U+FFFD)
    // and clears the buffer.
    const first = core.streamOpen(0);
    core.streamAppend(first, `a${HIGH}`);
    expect(core.streamClose(first)).not.toBeNull();
    // Session 2 reuses the same id (the fake always issues 1). Its leading
    // low surrogate must be REJECTED as lone — pairing it with session 1's
    // HIGH would fabricate a code point the producer never sent.
    const second = core.streamOpen(0);
    expect(second).toBe(first);
    expect(() => core.streamAppend(second, `${LOW}b`)).toThrow(/^InvalidPayload: /);
    expect(calls.filter((c) => c.method === "streamAppend").map((c) => c.text)).toEqual([
      "a",
      "�",
    ]);
  });
});

// ---------------------------------------------------------------------------
// S6 probes: decorations() validation precedence against the REAL wasm crate.
// The mock's order (mock-core.ts `decorations`) is canonical: malformed
// revision → staleness → malformed from/to → range (from > to) → bounds on
// `to` → selections payload (malformed, then per-selection bounds). The wasm
// layer parses its selections JSON before calling into the core, so unless it
// ALSO fronts the range/bounds checks (lib.rs `check_query_range`), a bad
// selections payload would preempt an invalid range there — these probes run
// the same call against both cores and pin the mock's answer. Loading mirrors
// conformance.test.ts: initSync over the raw pkg bytes, local skip when the
// pkg is unbuilt, mandatory under CI (which builds the pkg first — a silent
// skip would let CI go green without the real binary).
// ---------------------------------------------------------------------------

const REQUIRE_WASM = Boolean(process.env.CI);

async function loadWasmFactory(): Promise<(() => OxidownCore) | null> {
  const jsPath = fileURLToPath(
    new URL("../../../crates/oxidown-wasm/pkg/oxidown_wasm.js", import.meta.url),
  );
  const wasmPath = fileURLToPath(
    new URL("../../../crates/oxidown-wasm/pkg/oxidown_wasm_bg.wasm", import.meta.url),
  );
  if (!existsSync(jsPath) || !existsSync(wasmPath)) {
    if (REQUIRE_WASM) {
      throw new Error(
        "[wasm-core-boundary] CI requires the wasm side: crates/oxidown-wasm/pkg is missing " +
          "(build it with wasm-pack before running this suite)",
      );
    }
    return null;
  }
  try {
    const mod = (await import(/* @vite-ignore */ jsPath)) as {
      initSync: (arg: { module: BufferSource }) => unknown;
      OxidownCore: new () => Parameters<typeof adaptWasmCore>[0];
    };
    mod.initSync({ module: readFileSync(wasmPath) });
    return () => adaptWasmCore(new mod.OxidownCore());
  } catch (err) {
    if (REQUIRE_WASM) {
      throw new Error(
        `[wasm-core-boundary] CI requires the wasm side: crates/oxidown-wasm/pkg is present but failed to load: ${String(err)}`,
      );
    }
    console.log(
      "[wasm-core-boundary] crates/oxidown-wasm/pkg present but failed to load — wasm probes skipped:",
      err,
    );
    return null;
  }
}

const wasmFactory = await loadWasmFactory();

const probeCores: Array<[string, () => OxidownCore]> = [["MockCore", () => new MockCore()]];
if (wasmFactory) {
  probeCores.push(["WasmCore", wasmFactory]);
} else {
  console.log(
    "[wasm-core-boundary] S6 wasm probes skipped (build crates/oxidown-wasm with wasm-pack to enable them)",
  );
  describe.skip("decorations validation precedence, S6 probes: WasmCore (pkg not built)", () => {
    it("skipped — wasm pkg absent", () => {});
  });
}

/** Run `fn`, expecting a throw; return the error message. */
function thrownMessage(fn: () => unknown): string {
  try {
    fn();
  } catch (err) {
    return err instanceof Error ? err.message : String(err);
  }
  throw new Error("expected the call to throw, but it returned");
}

for (const [coreName, makeCore] of probeCores) {
  describe(`decorations validation precedence, S6 probes: ${coreName}`, () => {
    const boot = (): { core: OxidownCore; rev: number } => {
      const core = makeCore();
      const rev = core.load("hello world"); // 11 UTF-16 code units
      return { core, rev };
    };

    it("range check (from 9 > to 2) beats a MALFORMED selections payload", () => {
      const { core, rev } = boot();
      expect(thrownMessage(() => core.decorations(rev, 9, 2, [{ anchor: -1, head: 0 }]))).toBe(
        "InvalidRange: from 9 > to 2",
      );
    });

    it("bounds check on `to` beats a MALFORMED selections payload", () => {
      const { core, rev } = boot();
      expect(thrownMessage(() => core.decorations(rev, 0, 99, [{ anchor: -1, head: 0 }]))).toBe(
        "OutOfBounds: position 99 beyond document length 11 (UTF-16 code units)",
      );
    });

    it("range check beats an OUT-OF-BOUNDS selection", () => {
      const { core, rev } = boot();
      expect(thrownMessage(() => core.decorations(rev, 9, 2, [{ anchor: 99, head: 0 }]))).toBe(
        "InvalidRange: from 9 > to 2",
      );
    });
  });
}
