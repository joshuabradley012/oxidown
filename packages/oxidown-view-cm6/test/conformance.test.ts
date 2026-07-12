/**
 * Cross-core conformance suite — the SAME cases run against every core
 * behind the `OxidownCore` interface:
 *
 *  - MockCore, always;
 *  - the real Rust/wasm core, IF `crates/oxidown-wasm/pkg` has been built
 *    (wasm-pack --target web). It is loaded Node-side via `initSync` on the
 *    raw .wasm bytes and wrapped with the PRODUCTION adapter
 *    (`adaptWasmCore`), so the JS-side surrogate policy applies to both
 *    cores identically. A missing/unloadable pkg SKIPS the wasm side with a
 *    log line — it never fails the suite.
 *
 * The cases pin the boundary-contract parity semantics (docs/boundary-v0.md
 * plus the M1 review's pinned clarifications): error-message prefixes and,
 * where both cores share the exact string, verbatim messages; surrogate
 * handling; stream undo grouping; empty/no-op revision behavior; viewport
 * strictness; CR/CRLF line handling; and a decoration-equivalence spot
 * check. Each case is written ONCE against a core factory.
 *
 * Known cross-core spelling difference, asserted by prefix: the Rust core's
 * multi-leaf-block toggle guard throws `InvalidArgument: ...` (CoreError
 * variant) while the wasm arg layer and the mock use `InvalidArgs: ...` for
 * argument validation — the shared assertion is /^InvalidArg/.
 */
import { describe, expect, it } from "vitest";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { MockCore } from "../src/mock-core";
import { adaptWasmCore } from "../src/wasm-core";
import type { Decoration, OxidownCore, RangeCommandName } from "../src/protocol";

type CoreFactory = () => OxidownCore;

// ---------------------------------------------------------------------------
// wasm-side loading (Node): initSync over the raw bytes — the pkg is built
// with --target web, whose default init() wants fetch(URL), unavailable for
// file:// in Node. Same pkg the view loads in the browser (wasm-core.ts).
// ---------------------------------------------------------------------------

async function loadWasmFactory(): Promise<CoreFactory | null> {
  const jsPath = fileURLToPath(
    new URL("../../../crates/oxidown-wasm/pkg/oxidown_wasm.js", import.meta.url),
  );
  const wasmPath = fileURLToPath(
    new URL("../../../crates/oxidown-wasm/pkg/oxidown_wasm_bg.wasm", import.meta.url),
  );
  if (!existsSync(jsPath) || !existsSync(wasmPath)) return null;
  try {
    const mod = (await import(/* @vite-ignore */ jsPath)) as {
      initSync: (arg: { module: BufferSource }) => unknown;
      OxidownCore: new () => Parameters<typeof adaptWasmCore>[0];
    };
    mod.initSync({ module: readFileSync(wasmPath) });
    return () => adaptWasmCore(new mod.OxidownCore());
  } catch (err) {
    console.log(
      "[conformance] crates/oxidown-wasm/pkg present but failed to load — wasm side skipped:",
      err,
    );
    return null;
  }
}

const wasmFactory = await loadWasmFactory();

const factories: Array<[string, CoreFactory]> = [["MockCore", () => new MockCore()]];
if (wasmFactory) {
  factories.push(["WasmCore", wasmFactory]);
} else {
  console.log(
    "[conformance] wasm side skipped (build crates/oxidown-wasm with wasm-pack to enable it)",
  );
  describe.skip("core conformance: WasmCore (crates/oxidown-wasm/pkg not built)", () => {
    it("skipped — wasm pkg absent", () => {});
  });
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

const HIGH = "\uD83D"; // first half of 😀 (U+1F600 = D83D DE00)
const LOW = "\uDE00";
const EMOJI = HIGH + LOW;

/** Run `fn`, expecting a throw; return the error message. */
function thrownMessage(fn: () => unknown): string {
  try {
    fn();
  } catch (err) {
    return err instanceof Error ? err.message : String(err);
  }
  throw new Error("expected the call to throw, but it returned");
}

/** True when `pos` splits a surrogate pair of `doc`. */
function splitsPair(doc: string, pos: number): boolean {
  return (
    pos > 0 &&
    pos < doc.length &&
    doc.charCodeAt(pos - 1) >= 0xd800 &&
    doc.charCodeAt(pos - 1) <= 0xdbff &&
    doc.charCodeAt(pos) >= 0xdc00 &&
    doc.charCodeAt(pos) <= 0xdfff
  );
}

/** Order- and optional-field-insensitive canonical form of a decoration list. */
function normalize(ds: Decoration[]): string[] {
  return ds
    .map((d) => {
      const o = { ...d } as Record<string, unknown>;
      for (const key of ["revealed", "depth", "checked", "number", "delim"]) {
        if (o[key] === undefined || o[key] === false) delete o[key];
      }
      return JSON.stringify(o, Object.keys(o).sort());
    })
    .sort();
}

// ---------------------------------------------------------------------------
// the cases — written once, run per core
// ---------------------------------------------------------------------------

for (const [coreName, makeCore] of factories) {
  describe(`core conformance: ${coreName}`, () => {
    const boot = (text: string): OxidownCore => {
      const core = makeCore();
      core.load(text);
      return core;
    };

    // ---- error prefixes and argument validation --------------------------

    it("stale revision throws StaleRevision with the shared message", () => {
      const core = boot("abc");
      const rev = core.revision();
      core.applyEdit(rev, [{ at: 0, delete: 0, insert: "x" }], "user");
      expect(thrownMessage(() => core.applyEdit(rev, [], "user"))).toBe(
        `StaleRevision: core is at revision ${core.revision()}, caller passed ${rev}`,
      );
      expect(thrownMessage(() => core.decorations(rev, 0, 0, []))).toBe(
        `StaleRevision: core is at revision ${core.revision()}, caller passed ${rev}`,
      );
    });

    it("out-of-bounds splice end throws OutOfBounds with the shared message", () => {
      const core = boot("abc");
      expect(
        thrownMessage(() =>
          core.applyEdit(core.revision(), [{ at: 2, delete: 5, insert: "" }], "user"),
        ),
      ).toBe("OutOfBounds: position 7 beyond document length 3 (UTF-16 code units)");
    });

    it("overlapping/unordered splices throw InvalidSplice", () => {
      const core = boot("abcdef");
      expect(
        thrownMessage(() =>
          core.applyEdit(
            core.revision(),
            [
              { at: 1, delete: 2, insert: "" },
              { at: 2, delete: 1, insert: "x" },
            ],
            "user",
          ),
        ),
      ).toBe(
        "InvalidSplice: splice #1: splices must be ascending and non-overlapping (at 2 < previous end 3)",
      );
    });

    it("a splice boundary inside a surrogate pair throws SurrogateSplit", () => {
      const core = boot(`a${EMOJI}b`); // pair occupies [1, 3)
      for (const splices of [
        [{ at: 2, delete: 0, insert: "x" }], // `at` splits
        [{ at: 0, delete: 2, insert: "" }], // delete end splits
      ]) {
        expect(thrownMessage(() => core.applyEdit(core.revision(), splices, "user"))).toBe(
          "SurrogateSplit: position 2 falls inside a surrogate pair",
        );
      }
      expect(core.getText()).toBe(`a${EMOJI}b`);
    });

    it("lone-surrogate payloads throw InvalidPayload on load and applyEdit", () => {
      const core = makeCore();
      expect(thrownMessage(() => core.load(`bad${HIGH}doc`))).toBe(
        "InvalidPayload: text contains an unpaired surrogate",
      );
      core.load("ok");
      expect(
        thrownMessage(() =>
          core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: LOW }], "user"),
        ),
      ).toBe("InvalidPayload: splice insert contains an unpaired surrogate");
      expect(core.getText()).toBe("ok");
    });

    it("negative / non-integral numeric arguments throw InvalidArgs", () => {
      const core = boot("hello world");
      expect(thrownMessage(() => core.applyEdit(-1, [], "user"))).toBe(
        "InvalidArgs: baseRevision must be a non-negative integer, got -1",
      );
      expect(thrownMessage(() => core.decorations(1.5, 0, 0, []))).toBe(
        "InvalidArgs: revision must be a non-negative integer, got 1.5",
      );
      expect(thrownMessage(() => core.command("toggleStrong", -1, 3))).toBe(
        "InvalidArgs: from must be a non-negative integer, got -1",
      );
      expect(thrownMessage(() => core.command("toggleStrong", 1.5, 3))).toBe(
        "InvalidArgs: from must be a non-negative integer, got 1.5",
      );
      expect(thrownMessage(() => core.command("toggleTask", -2))).toBe(
        "InvalidArgs: pos must be a non-negative integer, got -2",
      );
      expect(core.getText()).toBe("hello world"); // command() throws WITHOUT mutating
    });

    it("setHeading level validation throws InvalidArgs", () => {
      const core = boot("Title");
      expect(
        thrownMessage(() => core.command("setHeading", 0, 7 as 0 | 1 | 2 | 3 | 4 | 5 | 6)),
      ).toBe("InvalidArgs: setHeading level must be an integer 0..=6, got 7");
      expect(
        thrownMessage(() => core.command("setHeading", 0, 2.5 as 0 | 1 | 2 | 3 | 4 | 5 | 6)),
      ).toBe("InvalidArgs: level must be a non-negative integer, got 2.5");
      expect(
        thrownMessage(() => core.command("setHeading", 0, -1 as 0 | 1 | 2 | 3 | 4 | 5 | 6)),
      ).toBe("InvalidArgs: level must be a non-negative integer, got -1");
      expect(core.getText()).toBe("Title");
    });

    it("a missing trailing command argument throws InvalidArgs", () => {
      const core = boot("hello world");
      const partial = core.command.bind(core) as (name: string, a: number) => unknown;
      expect(thrownMessage(() => partial("toggleStrong", 0))).toBe(
        "InvalidArgs: toggleStrong requires a `to` position",
      );
      expect(thrownMessage(() => partial("setHeading", 0))).toBe(
        "InvalidArgs: setHeading requires a heading level",
      );
    });

    it("an unknown command name throws InvalidCommand without mutating", () => {
      const core = boot("hello world");
      expect(
        thrownMessage(() => core.command("bogus" as RangeCommandName, 0, 1)),
      ).toBe('InvalidCommand: "bogus"');
      expect(core.getText()).toBe("hello world");
    });

    it("out-of-bounds command positions throw OutOfBounds (no null, no clamp)", () => {
      const core = boot("hello world"); // length 11
      expect(thrownMessage(() => core.command("toggleStrong", 0, 99))).toBe(
        "OutOfBounds: position 99 beyond document length 11 (UTF-16 code units)",
      );
      expect(thrownMessage(() => core.command("toggleTask", 99))).toBe(
        "OutOfBounds: position 99 beyond document length 11 (UTF-16 code units)",
      );
      expect(core.getText()).toBe("hello world");
    });

    it("reversed command ranges normalize (from > to behaves as min/max)", () => {
      const a = boot("hello world");
      a.command("toggleStrong", 11, 6);
      expect(a.getText()).toBe("hello **world**");

      const b = boot("- a\n- b\n");
      b.command("indentList", 6, 4);
      expect(b.getText()).toBe("- a\n  - b\n");
    });

    it("an inline toggle across multiple leaf blocks is refused (InvalidArg*)", () => {
      const doc = "para one\n\npara two";
      const core = boot(doc);
      const rev = core.revision();
      // Mock spells it `InvalidArgs: ...`; the Rust core's guard throws the
      // CoreError variant `InvalidArgument: ...` — prefix-match both.
      expect(thrownMessage(() => core.command("toggleStrong", 0, doc.length))).toMatch(
        /^InvalidArg/,
      );
      expect(core.getText()).toBe(doc); // thrown commands never mutate
      expect(core.revision()).toBe(rev);
    });

    // ---- clamp → throw (createAnchor / streamOpen / compositionBegin) ----

    it("createAnchor / streamOpen / compositionBegin throw OutOfBounds instead of clamping", () => {
      const core = boot("abc");
      const oob = "OutOfBounds: position 9 beyond document length 3 (UTF-16 code units)";
      expect(thrownMessage(() => core.createAnchor(9, "before"))).toBe(oob);
      expect(thrownMessage(() => core.streamOpen(9))).toBe(oob);
      expect(thrownMessage(() => core.compositionBegin(0, 9))).toBe(oob);
      expect(thrownMessage(() => core.compositionBegin(3, 1))).toBe(
        "InvalidRange: from 3 > to 1",
      );
    });

    it("streamOpen inside a surrogate pair throws SurrogateSplit (mutation position)", () => {
      const core = boot(EMOJI);
      expect(thrownMessage(() => core.streamOpen(1))).toBe(
        "SurrogateSplit: position 1 falls inside a surrogate pair",
      );
    });

    it("decorations validates the viewport and selection endpoints", () => {
      const core = boot("abc");
      const rev = core.revision();
      expect(thrownMessage(() => core.decorations(rev, 3, 1, []))).toBe(
        "InvalidRange: from 3 > to 1",
      );
      expect(thrownMessage(() => core.decorations(rev, 0, 9, []))).toBe(
        "OutOfBounds: position 9 beyond document length 3 (UTF-16 code units)",
      );
      expect(
        thrownMessage(() => core.decorations(rev, 0, 3, [{ anchor: 99, head: 0 }])),
      ).toBe("OutOfBounds: position 99 beyond document length 3 (UTF-16 code units)");
    });

    // ---- empty / no-op edits ---------------------------------------------

    it("an empty or all-no-op edit batch leaves the revision unchanged and makes no undo unit", () => {
      const core = boot("abc");
      const rev = core.revision();
      expect(core.applyEdit(rev, [], "user")).toBe(rev);
      expect(core.applyEdit(rev, [{ at: 1, delete: 0, insert: "" }], "user")).toBe(rev);
      expect(core.revision()).toBe(rev);
      expect(core.undo()).toBeNull();
      expect(core.getText()).toBe("abc");
    });

    it("streamAppend of an empty chunk is a no-op (no revision bump)", () => {
      const core = boot("abc");
      const id = core.streamOpen(3);
      const rev = core.revision();
      const change = core.streamAppend(id, "");
      expect(change.splices).toEqual([]);
      expect(change.revision).toBe(rev);
      expect(core.revision()).toBe(rev);
      core.streamClose(id);
      expect(core.getText()).toBe("abc");
    });

    // ---- streaming surrogate reassembly ----------------------------------

    it("a surrogate pair split across chunks reassembles; the withheld half bumps nothing", () => {
      const core = boot("");
      const id = core.streamOpen(0);
      const rev = core.revision();
      const first = core.streamAppend(id, HIGH); // trailing high surrogate: withheld
      expect(first.splices).toEqual([]);
      expect(first.revision).toBe(rev);
      expect(core.revision()).toBe(rev);
      expect(core.getText()).toBe("");
      const second = core.streamAppend(id, `${LOW}!`);
      expect(core.getText()).toBe(`${EMOJI}!`);
      expect(second.splices).toEqual([{ at: 0, delete: 0, insert: `${EMOJI}!` }]);
      expect(core.revision()).toBe(rev + 1); // exactly one real append
      core.streamClose(id);
    });

    it("an interior lone surrogate in a chunk throws InvalidPayload and clears the buffer", () => {
      const core = boot("");
      const id = core.streamOpen(0);
      core.streamAppend(id, HIGH); // withheld
      expect(thrownMessage(() => core.streamAppend(id, `${HIGH}x`))).toBe(
        "InvalidPayload: chunk contains an unpaired surrogate",
      );
      // The buffer was cleared on the throw: the next chunk starts fresh.
      core.streamAppend(id, "ok");
      core.streamClose(id);
      expect(core.getText()).toBe("ok");
    });

    it("streamClose flushes a still-pending high surrogate as one U+FFFD", () => {
      const core = boot("");
      const id = core.streamOpen(0);
      core.streamAppend(id, "a");
      core.streamAppend(id, HIGH); // withheld, never completed
      core.streamClose(id);
      expect(core.getText()).toBe("a�");
      expect(thrownMessage(() => core.streamAppend(id, "x"))).toBe(
        `UnknownStream: stream ${id} is unknown or already closed`,
      );
    });

    it("streamAppend on a never-opened id throws UnknownStream; streamClose no-ops", () => {
      const core = boot("x");
      expect(thrownMessage(() => core.streamAppend(999, "a"))).toBe(
        "UnknownStream: stream 999 is unknown or already closed",
      );
      expect(() => core.streamClose(999)).not.toThrow();
    });

    // ---- stream undo grouping --------------------------------------------

    it("an uninterrupted stream session is exactly one undo unit", () => {
      const core = boot("X");
      const id = core.streamOpen(1);
      core.streamAppend(id, "a");
      core.streamAppend(id, "b");
      core.streamAppend(id, "c");
      core.streamClose(id);
      expect(core.getText()).toBe("Xabc");
      core.undo();
      expect(core.getText()).toBe("X");
      expect(core.undo()).toBeNull();
    });

    it("stream undo grouping: one unit per stream, undo in unit-creation order", () => {
      // append-A / user-edit / append-B: the FIRST undo removes the USER
      // edit (its unit was created after the stream's unit began), the
      // SECOND removes the whole stream (A+B together). Boundary v0.2
      // clarification 2 / history.rs record_stream_append.
      const core = boot("head\n\ntail");
      const id = core.streamOpen(core.docLength());
      core.streamAppend(id, "A");
      core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "USER" }], "user");
      core.streamAppend(id, "B");
      core.streamClose(id);
      expect(core.getText()).toBe("USERhead\n\ntailAB");

      core.undo();
      expect(core.getText()).toBe("head\n\ntailAB");
      core.undo();
      expect(core.getText()).toBe("head\n\ntail");
      expect(core.undo()).toBeNull();

      core.redo();
      expect(core.getText()).toBe("head\n\ntailAB");
      core.redo();
      expect(core.getText()).toBe("USERhead\n\ntailAB");
      expect(core.redo()).toBeNull();
    });

    // ---- undo splices never split surrogate pairs ------------------------

    it("undo/redo splices never place a boundary inside a surrogate pair", () => {
      const core = boot(`x${EMOJI}y`);
      // Replace 😀 (U+1F600) with 😁 (U+1F601): the two differ only in the
      // LOW surrogate, so a naive prefix/suffix trim would split the pair.
      core.applyEdit(core.revision(), [{ at: 1, delete: 2, insert: "😁" }], "user");
      const before = core.getText();
      const change = core.undo();
      expect(change).not.toBeNull();
      for (const s of change!.splices) {
        expect(splitsPair(before, s.at)).toBe(false);
        expect(splitsPair(before, s.at + s.delete)).toBe(false);
      }
      expect(core.getText()).toBe(`x${EMOJI}y`);
    });

    // ---- anchors: public ids never disturb stream-internal state ---------

    it("dropAnchor cannot disturb an open stream's internal anchor", () => {
      const core = boot("doc");
      const publicId = core.createAnchor(1, "before");
      const stream = core.streamOpen(3);
      // Probing every plausible id: dropping is a silent no-op for unknown
      // AND stream-internal ids (never a crash), and the stream survives.
      for (let id = 0; id <= 10; id++) {
        expect(() => core.dropAnchor(id)).not.toThrow();
      }
      expect(core.resolveAnchor(publicId)).toBeNull(); // the public anchor did drop
      const change = core.streamAppend(stream, "!");
      expect(change.splices).toEqual([{ at: 3, delete: 0, insert: "!" }]);
      core.streamClose(stream);
      expect(core.getText()).toBe("doc!");
    });

    // ---- viewport strictness ---------------------------------------------

    it("viewport overlap is strictly half-open: boundary-touching nodes are excluded", () => {
      const doc = "**a**\nplain\n**b**"; // strong nodes at [0, 5) and [12, 17)
      const core = boot(doc);
      const rev = core.revision();
      // [5, 12) touches both nodes' boundaries but overlaps neither.
      expect(core.decorations(rev, 5, 12, [])).toEqual([]);
      // [0, 5): the first node only.
      const head = core.decorations(rev, 0, 5, []);
      expect(normalize(head)).toEqual(
        normalize([
          { kind: "conceal", from: 0, to: 2 },
          { kind: "mark", from: 2, to: 3, style: "strong" },
          { kind: "conceal", from: 3, to: 5 },
        ]),
      );
      // [0, 12): still excludes the second node (starts exactly at `to`).
      expect(normalize(core.decorations(rev, 0, 12, []))).toEqual(normalize(head));
    });

    // ---- composition reveal is per conceal span --------------------------

    it("composition reveals only the conceal spans it touches", () => {
      const doc = "**bold** x"; // delimiter spans [0, 2) and [6, 8)
      const core = boot(doc);
      core.compositionBegin(0, 1); // touches only the opening span
      const ds = core.decorations(core.revision(), 0, doc.length, []);
      const delims = ds.filter((d) => d.kind === "mark" && d.style === "delim");
      const conceals = ds.filter((d) => d.kind === "conceal");
      expect(delims).toEqual([{ kind: "mark", from: 0, to: 2, style: "delim" }]);
      expect(conceals).toEqual([{ kind: "conceal", from: 6, to: 8 }]);
      core.compositionEnd();
    });

    // ---- CR / CRLF line handling (contract v0.2 clarification 5) ----------

    it("setHeading resolves lines split by \\r\\n and lone \\r", () => {
      const a = boot("one\r\ntwo");
      a.command("setHeading", 6, 2); // inside "two"
      expect(a.getText()).toBe("one\r\n## two");

      const b = boot("a\rb");
      b.command("setHeading", 2, 1); // on "b"
      expect(b.getText()).toBe("a\r# b");
    });

    it("enter continues a list item across \\r\\n and lone \\r line endings", () => {
      const a = boot("- a\r\n- b");
      a.command("enter", 8, 8); // end of "- b"
      expect(a.getText()).toBe("- a\r\n- b\n- ");

      const b = boot("- a\r- b");
      b.command("enter", 7, 7);
      expect(b.getText()).toBe("- a\r- b\n- ");
    });

    it("lone-\\r-separated list items decorate as separate items", () => {
      const doc = "- a\r- b";
      const core = boot(doc);
      const ds = core.decorations(core.revision(), 0, doc.length, []);
      const widgets = ds.filter((d) => d.kind === "widget" && d.widget === "bullet");
      expect(normalize(widgets)).toEqual(
        normalize([
          { kind: "widget", from: 0, to: 2, widget: "bullet" },
          { kind: "widget", from: 4, to: 6, widget: "bullet" },
        ]),
      );
      const lines = ds.filter((d) => d.kind === "line" && d.style === "list-item");
      expect(lines.map((l) => (l.kind === "line" ? l.at : -1)).sort((x, y) => x - y)).toEqual([
        0, 4,
      ]);
    });
  });
}

// ---------------------------------------------------------------------------
// cross-core decoration equivalence — needs both cores at once, so it lives
// outside the per-factory loop (and skips with the wasm side).
// ---------------------------------------------------------------------------

(wasmFactory ? describe : describe.skip)("cross-core decoration equivalence", () => {
  const FIXTURE = [
    "# Title",
    "plain **bold** and *em* and `code` and ~~strike~~",
    "- bullet item",
    "- [ ] task item",
    "1. ordered item",
    "> quoted line",
    "", // blank: ends the quote (a following line would lazily continue it)
    "see [text](url) end",
    "", // blank: keeps "---" a thematic break (it would otherwise setext-underline the paragraph)
    "---",
    "",
  ].join("\n");

  it("both cores emit the same decoration set for a mixed fixture (concealed state)", () => {
    const mock: OxidownCore = new MockCore();
    const wasm = wasmFactory!();
    mock.load(FIXTURE);
    wasm.load(FIXTURE);
    const fromMock = normalize(mock.decorations(mock.revision(), 0, FIXTURE.length, []));
    const fromWasm = normalize(wasm.decorations(wasm.revision(), 0, FIXTURE.length, []));
    expect(fromMock).toEqual(fromWasm);
  });
});
