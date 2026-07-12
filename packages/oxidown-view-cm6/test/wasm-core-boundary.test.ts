/**
 * JS-side boundary guards of the wasm adapter — everything testable without
 * the wasm binary (src/wasm-core.ts exports its pure pieces for exactly
 * this). The invariant under test: core text never contains an unpaired
 * surrogate, because wasm-bindgen's &str conversion would silently corrupt
 * one to U+FFFD instead of erroring.
 *
 * The adapter-level cases drive `adaptWasmCore` against a fake
 * `WasmCoreInstance` that records what actually crosses the "boundary".
 */
import { describe, expect, it } from "vitest";
import {
  StreamSurrogateBuffer,
  adaptWasmCore,
  assertNoUnpairedSurrogates,
  hasUnpairedSurrogate,
  splitTrailingHighSurrogate,
} from "../src/wasm-core";
import type { CoreChange } from "../src/protocol";

const HIGH = "\uD83D"; // first half of 😀 (U+1F600 = D83D DE00)
const LOW = "\uDE00";
const EMOJI = HIGH + LOW;

/**
 * Fake wasm instance: records every string that would cross the boundary
 * and maintains an append-only "document" for the streaming cases.
 */
function fakeInner() {
  let doc = "";
  let revision = 0;
  const calls: { method: string; text: string }[] = [];
  const change = (): CoreChange => ({ revision: ++revision, splices: [] });
  return {
    doc: () => doc,
    calls,
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
        doc += chunk;
        return change();
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
    core.streamClose(id);
    expect(doc()).toBe(`a${EMOJI}b`);
    expect(doc()).not.toContain("�");
  });

  it("flushes a still-pending high surrogate as U+FFFD on streamClose", () => {
    const { inner, doc, calls } = fakeInner();
    const core = adaptWasmCore(inner);
    const id = core.streamOpen(0);
    core.streamAppend(id, `x${HIGH}`);
    core.streamClose(id);
    expect(doc()).toBe("x�");
    // The flush is one final append, then the close.
    expect(calls.map((c) => c.method)).toEqual(["streamAppend", "streamAppend", "streamClose"]);
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

  it("destroy frees the wasm instance exactly once", () => {
    const { inner, calls } = fakeInner();
    const core = adaptWasmCore(inner);
    core.destroy?.();
    core.destroy?.(); // double-destroy guarded
    expect(calls.filter((c) => c.method === "free")).toHaveLength(1);
  });
});
