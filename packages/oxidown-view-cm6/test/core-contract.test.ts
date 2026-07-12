/**
 * Contract-behavior suite, run against the REAL Rust/wasm core — the only
 * implementation of docs/boundary-v0.md. Ported from the retired MockCore's
 * test file (test/mock-core.test.ts, deleted together with src/mock-core.ts):
 * every assertion that pinned CONTRACT behavior (decorations, reveal,
 * commands, numbering, undo/redo/coalescing, streaming, anchors, composition)
 * lives on here 1:1; the handful of spots where the mock deviated from the
 * authoritative core (noted inline, e.g. `***x***` nesting) now assert the
 * CORE's behavior.
 *
 * Clock control: the mock took an injected `now()`; the wasm core reads
 * `Date.now()` through js_sys, so the undo-coalescing suites fake the Date
 * global instead (vi.useFakeTimers + setSystemTime) — same determinism,
 * production code path.
 *
 * Loading: test/wasm-loader.ts — fails LOUDLY (naming `pnpm build:wasm`)
 * when crates/oxidown-wasm/pkg is missing; never skips.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { loadWasmCoreFactory } from "./wasm-loader";
import { applySplices } from "../src/splices";
import type { Decoration, RangeCommandName, SelectionRange, Splice } from "../src/protocol";

const makeWasmCore = await loadWasmCoreFactory();

// Fake ONLY the Date global (the coalescing window reads Date.now via
// js_sys::Date); timers stay real — nothing here schedules any.
beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"], now: 0 });
});
afterEach(() => {
  vi.useRealTimers();
});

function makeCore(text: string) {
  const core = makeWasmCore();
  const clock = {
    advance(ms: number) {
      vi.setSystemTime(Date.now() + ms);
    },
  };
  core.load(text);
  return { core, clock };
}

/** Cursor selection helper. */
const cursor = (pos: number): SelectionRange[] => [{ anchor: pos, head: pos }];

const conceals = (ds: Decoration[]) => ds.filter((d) => d.kind === "conceal");
const marks = (ds: Decoration[], style?: string) =>
  ds.filter((d) => d.kind === "mark" && (style === undefined || d.style === style));
const lines = (ds: Decoration[]) => ds.filter((d) => d.kind === "line");

describe("core decorations — M0 set", () => {
  it("ATX headings h1–h6: line decoration + conceal over hashes and following space", () => {
    for (let level = 1; level <= 6; level++) {
      const hashes = "#".repeat(level);
      const doc = `${hashes} Title\n\npark cursor here`;
      const { core } = makeCore(doc);
      const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
      const line = lines(ds);
      expect(line).toEqual([{ kind: "line", at: 0, style: `h${level}` }]);
      expect(conceals(ds)).toEqual([{ kind: "conceal", from: 0, to: level + 1 }]);
    }
  });

  it("`#hashes` without a space is not a heading; 7 hashes is not a heading", () => {
    const { core } = makeCore("#nospace\n####### seven\n\nx");
    const ds = core.decorations(core.revision(), 0, core.docLength(), cursor(core.docLength()));
    expect(lines(ds)).toEqual([]);
    expect(conceals(ds)).toEqual([]);
  });

  it("strong **x**: content mark + delimiter conceals", () => {
    const doc = "a **b** c";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(9));
    // cursor at 9 does not touch the node extent [2, 7]
    expect(marks(ds, "strong")).toEqual([{ kind: "mark", from: 4, to: 5, style: "strong" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 2, to: 4 },
      { kind: "conceal", from: 5, to: 7 },
    ]);
  });

  it("strong __x__ with underscores", () => {
    const doc = "__x__ y";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(7));
    expect(marks(ds, "strong")).toEqual([{ kind: "mark", from: 2, to: 3, style: "strong" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 0, to: 2 },
      { kind: "conceal", from: 3, to: 5 },
    ]);
  });

  it("emphasis *x* and _x_; ** does not parse as two *", () => {
    for (const d of ["*", "_"]) {
      const doc = `${d}x${d} tail`;
      const { core } = makeCore(doc);
      const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
      expect(marks(ds, "em")).toEqual([{ kind: "mark", from: 1, to: 2, style: "em" }]);
      expect(marks(ds, "strong")).toEqual([]);
    }
    // `**x**` must be strong, never em
    const { core } = makeCore("**x** tail");
    const ds = core.decorations(core.revision(), 0, 10, cursor(10));
    expect(marks(ds, "em")).toEqual([]);
    expect(marks(ds, "strong")).toEqual([{ kind: "mark", from: 2, to: 3, style: "strong" }]);
  });

  it("inline code `x`", () => {
    const doc = "a `c` b";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(7));
    expect(marks(ds, "code")).toEqual([{ kind: "mark", from: 3, to: 4, style: "code" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 2, to: 3 },
      { kind: "conceal", from: 4, to: 5 },
    ]);
  });

  it("nesting **bold *italic* bold**", () => {
    const doc = "**bold *italic* bold** end";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(26));
    expect(marks(ds, "strong")).toEqual([{ kind: "mark", from: 2, to: 20, style: "strong" }]);
    expect(marks(ds, "em")).toEqual([{ kind: "mark", from: 8, to: 14, style: "em" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 0, to: 2 },
      { kind: "conceal", from: 7, to: 8 },
      { kind: "conceal", from: 14, to: 15 },
      { kind: "conceal", from: 20, to: 22 },
    ]);
  });

  it("***both*** parses as em OUTSIDE strong (CommonMark; v0.1 clarification 3)", () => {
    // ADAPTED from the mock port: the retired MockCore emitted strong-outer
    // here — a documented deviation the contract explicitly flagged. The
    // authoritative core follows CommonMark: <em><strong>x</strong></em>,
    // so the em mark spans [1, 9) and the strong mark sits inside at [3, 7).
    const doc = "***both*** end";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(14));
    expect(marks(ds, "em")).toEqual([{ kind: "mark", from: 1, to: 9, style: "em" }]);
    expect(marks(ds, "strong")).toEqual([{ kind: "mark", from: 3, to: 7, style: "strong" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 0, to: 1 },
      { kind: "conceal", from: 1, to: 3 },
      { kind: "conceal", from: 7, to: 9 },
      { kind: "conceal", from: 9, to: 10 },
    ]);
  });

  it("CJK content: positions are UTF-16 code units", () => {
    const doc = "**太字** と斜体";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    expect(marks(ds, "strong")).toEqual([{ kind: "mark", from: 2, to: 4, style: "strong" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 0, to: 2 },
      { kind: "conceal", from: 4, to: 6 },
    ]);
  });

  it("emoji content: astral chars count as two code units", () => {
    const doc = "*a\u{1F600}b* z"; // *a😀b* z
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    expect(marks(ds, "em")).toEqual([{ kind: "mark", from: 1, to: 5, style: "em" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 0, to: 1 },
      { kind: "conceal", from: 5, to: 6 },
    ]);
  });

  it("viewport filtering: nodes outside [from, to) are omitted", () => {
    const doc = "**a**\nplain\n**b**";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, 5, cursor(8));
    // second strong node starts at 12 — outside the viewport
    for (const d of ds) {
      const pos = d.kind === "line" ? d.at : d.from;
      expect(pos).toBeLessThanOrEqual(5);
    }
    expect(marks(ds, "strong")).toEqual([{ kind: "mark", from: 2, to: 3, style: "strong" }]);
  });
});

describe("core reveal predicate", () => {
  const doc = "a **b** c"; // strong node extent [2, 7)
  const revealDs = (pos: number) => {
    const { core } = makeCore(doc);
    return core.decorations(core.revision(), 0, doc.length, cursor(pos));
  };

  it("cursor inside the node reveals: conceals become delim marks", () => {
    const ds = revealDs(4);
    expect(conceals(ds)).toEqual([]);
    expect(marks(ds, "delim")).toEqual([
      { kind: "mark", from: 2, to: 4, style: "delim" },
      { kind: "mark", from: 5, to: 7, style: "delim" },
    ]);
    // content mark stays
    expect(marks(ds, "strong")).toEqual([{ kind: "mark", from: 4, to: 5, style: "strong" }]);
  });

  it("boundary positions: touching the extent (incl. delimiters) reveals", () => {
    expect(conceals(revealDs(2))).toEqual([]); // at start of opening delimiter
    expect(conceals(revealDs(7))).toEqual([]); // at end of closing delimiter
  });

  it("positions just outside the extent do not reveal", () => {
    expect(conceals(revealDs(1))).toHaveLength(2);
    expect(conceals(revealDs(8))).toHaveLength(2);
  });

  it("non-empty selection ranges reveal on intersection", () => {
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, [{ anchor: 0, head: 3 }]);
    expect(conceals(ds)).toEqual([]);
  });

  it("reveal is per-node, not per-line", () => {
    const nested = "**bold *italic* bold** end";
    const { core } = makeCore(nested);
    // cursor at 3: inside strong extent [0,22) but outside em extent [7,15)
    const ds = core.decorations(core.revision(), 0, nested.length, cursor(3));
    expect(marks(ds, "delim")).toEqual([
      { kind: "mark", from: 0, to: 2, style: "delim" },
      { kind: "mark", from: 20, to: 22, style: "delim" },
    ]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 7, to: 8 },
      { kind: "conceal", from: 14, to: 15 },
    ]);
    // cursor at 9: inside both — everything revealed
    const ds2 = core.decorations(core.revision(), 0, nested.length, cursor(9));
    expect(conceals(ds2)).toEqual([]);
    expect(marks(ds2, "delim")).toHaveLength(4);
  });

  it("heading reveal: hashes become a delim mark when the cursor is on the line", () => {
    const doc2 = "## Title\n\ntail";
    const { core } = makeCore(doc2);
    const on = core.decorations(core.revision(), 0, doc2.length, cursor(4));
    expect(conceals(on)).toEqual([]);
    expect(marks(on, "delim")).toEqual([{ kind: "mark", from: 0, to: 3, style: "delim" }]);
    // line decoration is always emitted
    expect(lines(on)).toEqual([{ kind: "line", at: 0, style: "h2" }]);
    const off = core.decorations(core.revision(), 0, doc2.length, cursor(12));
    expect(conceals(off)).toEqual([{ kind: "conceal", from: 0, to: 3 }]);
  });
});

describe("core text mirror and revisions", () => {
  it("round-trips getText through applyEdit batches", () => {
    const { core } = makeCore("hello world");
    let expected = "hello world";
    const batches: Splice[][] = [
      [{ at: 0, delete: 5, insert: "goodbye" }],
      [
        { at: 0, delete: 1, insert: "G" },
        { at: 8, delete: 5, insert: "🌍 world" },
      ],
      [{ at: 2, delete: 0, insert: "od" }],
    ];
    for (const splices of batches) {
      core.applyEdit(core.revision(), splices, "user");
      expected = applySplices(expected, splices);
      expect(core.getText()).toBe(expected);
      expect(core.docLength()).toBe(expected.length);
    }
  });

  it("throws StaleRevision for applyEdit and decorations", () => {
    const { core } = makeCore("abc");
    const rev = core.revision();
    core.applyEdit(rev, [{ at: 0, delete: 0, insert: "x" }], "user");
    expect(() => core.applyEdit(rev, [{ at: 0, delete: 0, insert: "y" }], "user")).toThrow(
      `StaleRevision: core is at revision ${core.revision()}, caller passed ${rev}`,
    );
    expect(() => core.decorations(rev, 0, 1, cursor(0))).toThrow(/^StaleRevision:/);
    // current revision works
    expect(() => core.decorations(core.revision(), 0, 1, cursor(0))).not.toThrow();
  });

  it("throws OutOfBounds / InvalidSplice on bad splices", () => {
    const { core } = makeCore("abc");
    expect(() =>
      core.applyEdit(core.revision(), [{ at: 2, delete: 5, insert: "" }], "user"),
    ).toThrow("OutOfBounds: position 7 beyond document length 3 (UTF-16 code units)");
    expect(() =>
      core.applyEdit(
        core.revision(),
        [
          { at: 1, delete: 2, insert: "" },
          { at: 2, delete: 1, insert: "x" },
        ],
        "user",
      ),
    ).toThrow(/^InvalidSplice: .*ascending and non-overlapping/);
  });

  it("an empty or all-no-op batch leaves the revision unchanged and creates no undo unit", () => {
    const { core } = makeCore("abc");
    const rev = core.revision();
    expect(core.applyEdit(rev, [], "user")).toBe(rev);
    expect(core.applyEdit(rev, [{ at: 1, delete: 0, insert: "" }], "user")).toBe(rev);
    expect(core.revision()).toBe(rev);
    expect(core.undo()).toBeNull(); // no undo unit was created
    expect(core.getText()).toBe("abc");
  });

  it("splice boundaries inside a surrogate pair throw SurrogateSplit", () => {
    const { core } = makeCore("a😀b"); // 😀 = code units [1, 3)
    for (const splices of [
      [{ at: 2, delete: 0, insert: "x" }], // at splits the pair
      [{ at: 0, delete: 2, insert: "" }], // delete end splits the pair
    ]) {
      expect(() => core.applyEdit(core.revision(), splices, "user")).toThrow(
        "SurrogateSplit: position 2 falls inside a surrogate pair",
      );
    }
    expect(core.getText()).toBe("a😀b");
  });

  it("lone-surrogate payloads throw InvalidPayload on load and applyEdit", () => {
    const core = makeWasmCore();
    expect(() => core.load("bad\uD800doc")).toThrow(
      "InvalidPayload: text contains an unpaired surrogate",
    );
    core.load("ok");
    expect(() =>
      core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "\uDC00" }], "user"),
    ).toThrow("InvalidPayload: splice insert contains an unpaired surrogate");
    expect(core.getText()).toBe("ok");
  });

  it("revisions increase monotonically, including across load()", () => {
    const core = makeWasmCore();
    const r1 = core.load("a");
    expect(r1).toBe(1); // revision 0's successor
    const r2 = core.applyEdit(r1, [{ at: 0, delete: 0, insert: "b" }], "user");
    expect(r2).toBe(r1 + 1);
    const r3 = core.load("fresh");
    expect(r3).toBeGreaterThan(r2);
  });
});

describe("core undo/redo and coalescing", () => {
  it("coalesces adjacent user edits within 500ms into one unit", () => {
    const { core, clock } = makeCore("");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "a" }], "user");
    clock.advance(100);
    core.applyEdit(core.revision(), [{ at: 1, delete: 0, insert: "b" }], "user");
    clock.advance(100);
    core.applyEdit(core.revision(), [{ at: 2, delete: 0, insert: "c" }], "user");
    clock.advance(1000); // breaks the group
    core.applyEdit(core.revision(), [{ at: 3, delete: 0, insert: "d" }], "user");
    expect(core.getText()).toBe("abcd");

    const u1 = core.undo();
    expect(u1).not.toBeNull();
    expect(core.getText()).toBe("abc");
    expect(u1!.splices).toEqual([{ at: 3, delete: 1, insert: "" }]);
    expect(u1!.revision).toBe(core.revision());

    const u2 = core.undo();
    expect(core.getText()).toBe("");
    expect(u2!.splices).toEqual([{ at: 0, delete: 3, insert: "" }]);
    expect(core.undo()).toBeNull();
  });

  it("redo restores units; undo/redo splices are in current-doc coordinates", () => {
    const { core, clock } = makeCore("");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "abc" }], "user");
    clock.advance(1000);
    core.applyEdit(core.revision(), [{ at: 3, delete: 0, insert: "def" }], "user");
    core.undo();
    core.undo();
    expect(core.getText()).toBe("");
    const r1 = core.redo();
    expect(core.getText()).toBe("abc");
    expect(r1!.splices).toEqual([{ at: 0, delete: 0, insert: "abc" }]);
    const r2 = core.redo();
    expect(core.getText()).toBe("abcdef");
    expect(r2!.splices).toEqual([{ at: 3, delete: 0, insert: "def" }]);
    expect(core.redo()).toBeNull();
  });

  it("backspace (deletion adjacent to last edit end) coalesces", () => {
    const { core, clock } = makeCore("");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "ab" }], "user");
    clock.advance(50);
    core.applyEdit(core.revision(), [{ at: 1, delete: 1, insert: "" }], "user");
    expect(core.getText()).toBe("a");
    core.undo();
    expect(core.getText()).toBe("");
  });

  it("paste always breaks the group (before and after)", () => {
    const { core, clock } = makeCore("");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "a" }], "user");
    clock.advance(10);
    core.applyEdit(core.revision(), [{ at: 1, delete: 0, insert: "XYZ" }], "paste");
    clock.advance(10);
    core.applyEdit(core.revision(), [{ at: 4, delete: 0, insert: "b" }], "user");
    expect(core.getText()).toBe("aXYZb");
    core.undo();
    expect(core.getText()).toBe("aXYZ");
    core.undo();
    expect(core.getText()).toBe("a");
    core.undo();
    expect(core.getText()).toBe("");
  });

  it("insert-at-front coalesces (v0.3 region rule); an edit away from the unit's region does not", () => {
    // ADAPTED from the mock port: the retired mock kept the ORIGINAL v0.1
    // adjacency wording ("touches the previous edit's end position") for
    // this case and asserted two units for a@0 then z@0. The v0.3-amended
    // contract rule — a single splice falling within/touching the ends of
    // the region the top unit's undo would remove, explicitly covering
    // insert-at-front — makes them ONE unit, and the core implements that.
    const a = makeCore("");
    a.core.applyEdit(a.core.revision(), [{ at: 0, delete: 0, insert: "a" }], "user");
    a.clock.advance(10);
    a.core.applyEdit(a.core.revision(), [{ at: 0, delete: 0, insert: "z" }], "user");
    expect(a.core.getText()).toBe("za");
    const u = a.core.undo();
    expect(u!.splices).toEqual([{ at: 0, delete: 2, insert: "" }]); // one unit
    expect(a.core.getText()).toBe("");
    expect(a.core.undo()).toBeNull();

    // An edit NOT touching the unit's undo region stays its own unit.
    const b = makeCore("xxxx");
    b.core.applyEdit(b.core.revision(), [{ at: 0, delete: 0, insert: "a" }], "user");
    b.clock.advance(10);
    b.core.applyEdit(b.core.revision(), [{ at: 3, delete: 0, insert: "z" }], "user");
    expect(b.core.getText()).toBe("axxzxx");
    b.core.undo();
    expect(b.core.getText()).toBe("axxxx");
    b.core.undo();
    expect(b.core.getText()).toBe("xxxx");
  });

  it("a new edit clears the redo stack", () => {
    const { core, clock } = makeCore("");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "a" }], "user");
    core.undo();
    clock.advance(1000);
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "b" }], "user");
    expect(core.redo()).toBeNull();
    expect(core.getText()).toBe("b");
  });

  it("coalescing pauses during composition: the 500ms window does not break a session", () => {
    const { core, clock } = makeCore("");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "ab" }], "user");
    clock.advance(1000);
    core.compositionBegin(2, 2);
    core.applyEdit(core.revision(), [{ at: 2, delete: 0, insert: "か" }], "ime");
    clock.advance(2000); // way past the window — composition keeps the group open
    core.applyEdit(core.revision(), [{ at: 2, delete: 1, insert: "漢字" }], "ime");
    core.compositionEnd();
    expect(core.getText()).toBe("ab漢字");
    core.undo();
    expect(core.getText()).toBe("ab");
    core.undo();
    expect(core.getText()).toBe("");
  });

  it("compositionEnd closes the group: the next edit is a new unit", () => {
    const { core, clock } = makeCore("");
    core.compositionBegin(0, 0);
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "か" }], "ime");
    core.compositionEnd();
    clock.advance(10);
    core.applyEdit(core.revision(), [{ at: 1, delete: 0, insert: "x" }], "user");
    core.undo();
    expect(core.getText()).toBe("か");
    core.undo();
    expect(core.getText()).toBe("");
  });
});

describe("core undo/redo use the exact recorded batches, not a collapsed diff", () => {
  // A multi-splice unit collapsed through a whole-text prefix/suffix diff
  // becomes ONE splice; every live position strictly inside it (anchors, an
  // open stream's insertion anchor, the composition range) teleports to its
  // start when mapped. undo()/redo() must map through the recorded exact
  // batches instead (history.rs) — the retired mock's coarse-diff cascade
  // bug is exactly what these pin against.

  it("undo of a multi-splice command returns the exact inverse batch and restores anchors", () => {
    const { core } = makeCore("a bold c");
    const id = core.createAnchor(4, "before"); // inside "bold"
    core.command("toggleStrong", 2, 6); // splices: insert ** at 2 and at 6
    expect(core.getText()).toBe("a **bold** c");
    expect(core.resolveAnchor(id)).toBe(6);

    const change = core.undo();
    // The exact recorded inverse (two deletions), not one collapsed splice
    // {at: 2, delete: 8, insert: "bold"}.
    expect(change!.splices).toEqual([
      { at: 2, delete: 2, insert: "" },
      { at: 8, delete: 2, insert: "" },
    ]);
    expect(core.getText()).toBe("a bold c");
    expect(core.resolveAnchor(id)).toBe(4); // the collapsed diff resolved 2
  });

  it("redo re-applies the exact forward batch, keeping anchors aligned", () => {
    const { core } = makeCore("a bold c");
    const id = core.createAnchor(4, "before");
    core.command("toggleStrong", 2, 6);
    core.undo();
    const change = core.redo();
    expect(change!.splices).toEqual([
      { at: 2, delete: 0, insert: "**" },
      { at: 6, delete: 0, insert: "**" },
    ]);
    expect(core.getText()).toBe("a **bold** c");
    expect(core.resolveAnchor(id)).toBe(6);
    // And a second round trip stays stable (the redo re-recorded the exact
    // inverse on the undo stack).
    core.undo();
    expect(core.resolveAnchor(id)).toBe(4);
  });

  it("an open stream's insertion anchor survives undo of a multi-splice command at the right spot", () => {
    const { core } = makeCore("a bold c");
    const sid = core.streamOpen(4); // between "bo" and "ld"
    core.command("toggleStrong", 2, 6);
    core.undo(); // strips the delimiters; the stream point must return to 4
    const change = core.streamAppend(sid, "X");
    expect(change.splices).toEqual([{ at: 4, delete: 0, insert: "X" }]);
    expect(core.getText()).toBe("a boXld c");
    core.streamClose(sid);
  });

  it("coalesced typing runs undo as one single-splice unit", () => {
    const { core, clock } = makeCore("");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "a" }], "user");
    clock.advance(10);
    core.applyEdit(core.revision(), [{ at: 1, delete: 0, insert: "b" }], "user"); // coalesces
    const change = core.undo();
    expect(change!.splices).toEqual([{ at: 0, delete: 2, insert: "" }]);
    expect(core.getText()).toBe("");
  });
});

describe("core composition stability rule", () => {
  it("conceal spans TOUCHED by the composition range are emitted as delim marks (per-span)", () => {
    const doc = "**bold** x"; // delimiter spans [0, 2) and [6, 8)
    const { core } = makeCore(doc);
    // Selection parked away from the node; without composition it conceals.
    const before = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(conceals(before)).toHaveLength(2);

    // A composition range touching BOTH delimiter spans reveals both.
    core.compositionBegin(2, 6);
    const during = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(conceals(during)).toEqual([]);
    expect(marks(during, "delim")).toEqual([
      { kind: "mark", from: 0, to: 2, style: "delim" },
      { kind: "mark", from: 6, to: 8, style: "delim" },
    ]);
    core.compositionEnd();

    // Per-conceal-span (decorations.rs): a composition range strictly inside
    // the CONTENT touches neither delimiter span, so both stay concealed.
    core.compositionBegin(4, 4);
    const inside = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(conceals(inside)).toHaveLength(2);
    core.compositionEnd();

    // And a range touching only the OPENING span reveals just that one —
    // sibling delimiters of the same node are not dragged along.
    core.compositionBegin(0, 1);
    const partial = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(marks(partial, "delim")).toEqual([{ kind: "mark", from: 0, to: 2, style: "delim" }]);
    expect(conceals(partial)).toEqual([{ kind: "conceal", from: 6, to: 8 }]);
    core.compositionEnd();

    const after = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(conceals(after)).toHaveLength(2);
  });

  it("a composition range away from a node does not reveal it", () => {
    const doc = "**bold** x";
    const { core } = makeCore(doc);
    core.compositionBegin(9, 9);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(conceals(ds)).toHaveLength(2);
    core.compositionEnd();
  });

  it("no NEW conceal spans appear inside the composition range as IME text completes a node", () => {
    const { core } = makeCore("x");
    core.compositionBegin(1, 1);
    core.applyEdit(core.revision(), [{ at: 1, delete: 0, insert: "*あ*" }], "ime");
    // an em node now exists inside the (grown) composition range — it must be
    // emitted revealed, not concealed
    const ds = core.decorations(core.revision(), 0, core.docLength(), cursor(0));
    expect(conceals(ds)).toEqual([]);
    expect(marks(ds, "delim")).toHaveLength(2);
    expect(marks(ds, "em")).toEqual([{ kind: "mark", from: 2, to: 3, style: "em" }]);
    core.compositionEnd();
    // after the session, normal conceal behavior returns
    const after = core.decorations(core.revision(), 0, core.docLength(), cursor(0));
    expect(conceals(after)).toHaveLength(2);
  });

  it("the composition range maps through edits earlier in the document", () => {
    const doc = "abc **bold** x";
    const { core } = makeCore(doc);
    core.compositionBegin(4, 6); // over the strong node's opening delimiter [4, 6)
    // a user edit earlier in the doc shifts everything right by 3
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "123" }], "user");
    const ds = core.decorations(core.revision(), 0, core.docLength(), cursor(2));
    // strong node now at [7, 15); the composition range shifted to [7, 9)
    // with it, still revealing exactly the opening delimiter span (per-span
    // composition reveal) while the closing span stays concealed.
    expect(marks(ds, "delim")).toEqual([{ kind: "mark", from: 7, to: 9, style: "delim" }]);
    expect(conceals(ds)).toEqual([{ kind: "conceal", from: 13, to: 15 }]);
    core.compositionEnd();
  });
});

// ---------------------------------------------------------------------------
// v0.2 (M1) additions
// ---------------------------------------------------------------------------

describe("core decorations — M1 vocabulary (v0.2)", () => {
  it("strikethrough ~~x~~: content mark + delimiter conceals", () => {
    const doc = "a ~~b~~ c";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(9));
    expect(marks(ds, "strike")).toEqual([{ kind: "mark", from: 4, to: 5, style: "strike" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 2, to: 4 },
      { kind: "conceal", from: 5, to: 7 },
    ]);
  });

  it("blockquote (depth 1): LINE-level reveal, matching headings (v0.3)", () => {
    // A blank line separates the quote from "world": without it, CommonMark
    // lazy continuation makes "world" part of the quote (asserted below) —
    // the retired mock never modeled lazy continuation.
    const doc = "> hello\n\nworld";
    const { core } = makeCore(doc);
    // Cursor on the OTHER line: markers concealed, line not revealed.
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    expect(lines(ds)).toEqual([{ kind: "line", at: 0, style: "blockquote", depth: 1 }]);
    expect(conceals(ds)).toEqual([{ kind: "conceal", from: 0, to: 2 }]);

    // Caret anywhere on the quote line — marker, text, or line end — shows
    // raw markers + revealed-flagged line (the view drops the bar/padding
    // to show source geometry).
    for (const pos of [0, 1, 2, 4, 7]) {
      const revealed = core.decorations(core.revision(), 0, doc.length, cursor(pos));
      expect(conceals(revealed)).toEqual([]);
      expect(marks(revealed, "delim")).toEqual([{ kind: "mark", from: 0, to: 2, style: "delim" }]);
      expect(lines(revealed)).toEqual([
        { kind: "line", at: 0, style: "blockquote", depth: 1, revealed: true },
      ]);
    }
  });

  it("blockquote lazy continuation: an unmarked next line is part of the quote (CommonMark)", () => {
    // ADAPTED from the mock port: the mock treated "world" as a plain
    // paragraph; per CommonMark it lazily continues the quote's paragraph,
    // so the core emits a blockquote line decoration for it too — and
    // LINE-level reveal applies per line (the continuation line has no
    // marker to reveal, but its `revealed` flag still drops the bar).
    const doc = "> hello\nworld";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    expect(lines(ds)).toEqual([
      { kind: "line", at: 0, style: "blockquote", depth: 1 },
      { kind: "line", at: 8, style: "blockquote", depth: 1, revealed: true },
    ]);
    expect(conceals(ds)).toEqual([{ kind: "conceal", from: 0, to: 2 }]);
    // Cursor on the marked first line reveals ITS marker; the continuation
    // line keeps its (marker-less) unrevealed decoration.
    const onFirst = core.decorations(core.revision(), 0, doc.length, cursor(4));
    expect(marks(onFirst, "delim")).toEqual([{ kind: "mark", from: 0, to: 2, style: "delim" }]);
    expect(lines(onFirst)).toEqual([
      { kind: "line", at: 0, style: "blockquote", depth: 1, revealed: true },
      { kind: "line", at: 8, style: "blockquote", depth: 1 },
    ]);
  });

  it("fenced code block: fence lines styled + raw fences concealed, revealed per line", () => {
    const doc = "```js\nconst x = 1;\n```\nafter";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    expect(lines(ds)).toEqual([
      { kind: "line", at: 0, style: "code-fence" },
      { kind: "line", at: 6, style: "code-block" },
      { kind: "line", at: 19, style: "code-fence" },
    ]);
    expect(marks(ds, "code")).toEqual([{ kind: "mark", from: 6, to: 18, style: "code" }]);
    // The raw ``` fences conceal (the styled fence line is the block's edge).
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 0, to: 5 },
      { kind: "conceal", from: 19, to: 22 },
    ]);
    // BLOCK-level reveal: a cursor anywhere inside the block (here in the
    // body) reveals BOTH raw fences for editing.
    const revealed = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(marks(revealed, "delim")).toEqual([
      { kind: "mark", from: 0, to: 5, style: "delim" },
      { kind: "mark", from: 19, to: 22, style: "delim" },
    ]);
    expect(conceals(revealed)).toEqual([]);
  });

  it("thematic break: hr line + concealed dashes, revealed as delim on the line", () => {
    // ADAPTED from the mock port: a blank line must precede the dashes —
    // per CommonMark, "---" directly under a paragraph is that paragraph's
    // SETEXT-heading underline, not a thematic break (the retired mock
    // parsed it as an hr regardless; setext headings are outside the M1
    // decoration scope, so that shape emits nothing).
    const doc = "before\n\n---\nafter";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    expect(lines(ds)).toEqual([{ kind: "line", at: 8, style: "hr" }]);
    expect(conceals(ds)).toEqual([{ kind: "conceal", from: 8, to: 11 }]);
    // Cursor on the hr line reveals the raw dashes as a delim mark.
    const revealed = core.decorations(core.revision(), 0, doc.length, cursor(9));
    expect(marks(revealed, "delim")).toEqual([{ kind: "mark", from: 8, to: 11, style: "delim" }]);
    expect(conceals(revealed)).toEqual([]);
  });

  it("bullets AND ordered markers are both widgets with LINE-level reveal", () => {
    // Contract v0.3 amendment (research/07 §0/§1.2): concealed ordered
    // markers are a computed-number widget too, not a plain mark — the core
    // never rewrites source digits.
    const doc = "- item\n1. other";
    const { core } = makeCore(doc);
    // Cursor on the SECOND line: the first item's bullet stays a widget, and
    // the second item's ordered marker reveals (raw digits, mark:list-marker).
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(12));
    expect(ds.filter((d) => d.kind === "widget")).toEqual([
      { kind: "widget", from: 0, to: 2, widget: "bullet" },
    ]);
    expect(marks(ds, "list-marker")).toEqual([
      { kind: "mark", from: 7, to: 10, style: "list-marker" },
    ]);
    // Every item line carries a list-item line decoration (hanging indent);
    // the cursor's own line is flagged revealed (LINE-level, v0.3).
    expect(lines(ds)).toEqual([
      { kind: "line", at: 0, style: "list-item", depth: 1 },
      { kind: "line", at: 7, style: "list-item", depth: 1, revealed: true },
    ]);
    // Caret anywhere on the bullet line — marker, text, or line end —
    // reveals the raw bullet marker and flags the line; the ordered marker
    // (untouched line) now renders as its computed-number widget instead.
    for (const pos of [0, 1, 2, 4, 6]) {
      const revealed = core.decorations(core.revision(), 0, doc.length, cursor(pos));
      expect(revealed.filter((d) => d.kind === "widget")).toEqual([
        { kind: "widget", from: 7, to: 10, widget: "ordered", number: 1, delim: "." },
      ]);
      expect(marks(revealed, "list-marker")).toEqual([
        { kind: "mark", from: 0, to: 2, style: "list-marker" },
      ]);
      expect(lines(revealed)[0]).toEqual({
        kind: "line",
        at: 0,
        style: "list-item",
        depth: 1,
        revealed: true,
      });
    }
    expect(conceals(ds)).toEqual([]);
  });

  it("ordered markers display sequential numbers ignoring raw digits", () => {
    // "1./1./3." must DISPLAY 1,2,3 — research/07 §0: CommonMark only fixes
    // the list's start number; sibling digits are cosmetic.
    const { core } = makeCore("1. a\n1. b\n3. c\n");
    const ds = core.decorations(core.revision(), 0, core.docLength(), []);
    const widgets = ds.filter(
      (d): d is Extract<Decoration, { kind: "widget"; widget: "ordered" }> =>
        d.kind === "widget" && d.widget === "ordered",
    );
    expect(widgets.map((w) => w.number)).toEqual([1, 2, 3]);
  });

  it("ordered list start number is honored", () => {
    // "4./5./9." displays 4,5,6.
    const { core } = makeCore("4. a\n5. b\n9. c\n");
    const ds = core.decorations(core.revision(), 0, core.docLength(), []);
    const widgets = ds.filter(
      (d): d is Extract<Decoration, { kind: "widget"; widget: "ordered" }> =>
        d.kind === "widget" && d.widget === "ordered",
    );
    expect(widgets.map((w) => w.number)).toEqual([4, 5, 6]);
  });

  it("a delimiter change starts a new, sequence-independent ordered list", () => {
    const { core } = makeCore("1. a\n2) b\n");
    const ds = core.decorations(core.revision(), 0, core.docLength(), []);
    const widgets = ds.filter(
      (d): d is Extract<Decoration, { kind: "widget"; widget: "ordered" }> =>
        d.kind === "widget" && d.widget === "ordered",
    );
    expect(widgets.map((w) => [w.number, w.delim])).toEqual([
      [1, "."],
      [2, ")"],
    ]);
  });

  it("a nested ordered list restarts its own sequence", () => {
    const { core } = makeCore("1. a\n   1. nested\n   2. nested2\n2. b\n");
    const ds = core.decorations(core.revision(), 0, core.docLength(), []);
    const widgets = ds.filter(
      (d): d is Extract<Decoration, { kind: "widget"; widget: "ordered" }> =>
        d.kind === "widget" && d.widget === "ordered",
    );
    expect(widgets.map((w) => w.number)).toEqual([1, 1, 2, 2]);
  });

  it("a blank line does NOT reset ordered numbering (loose list)", () => {
    // Per CommonMark a blank line doesn't close a list — that's exactly what
    // makes it "loose" — so "1. a\n1. b\n\n1. c" is ONE list and must display
    // 1,2,3 (the core counts straight through blank lines).
    const { core } = makeCore("1. a\n1. b\n\n1. c\n");
    const ds = core.decorations(core.revision(), 0, core.docLength(), []);
    const widgets = ds.filter(
      (d): d is Extract<Decoration, { kind: "widget"; widget: "ordered" }> =>
        d.kind === "widget" && d.widget === "ordered",
    );
    expect(widgets.map((w) => w.number)).toEqual([1, 2, 3]);
  });

  it("a real paragraph line (non-blank, non-item) still resets ordered numbering", () => {
    const { core } = makeCore("1. a\n1. b\n\nplain paragraph\n\n1. c\n");
    const ds = core.decorations(core.revision(), 0, core.docLength(), []);
    const widgets = ds.filter(
      (d): d is Extract<Decoration, { kind: "widget"; widget: "ordered" }> =>
        d.kind === "widget" && d.widget === "ordered",
    );
    expect(widgets.map((w) => w.number)).toEqual([1, 2, 1]);
  });

  it("task list item: widget:task (concealed) or delims (LINE-level reveal)", () => {
    const doc = "- [ ] buy milk\nsecond line";
    const { core } = makeCore(doc);
    // Cursor on the SECOND line: the task line is concealed — the leading
    // "- " conceals entirely (no bullet widget); the checkbox represents it.
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    const widgets = ds.filter((d) => d.kind === "widget");
    expect(widgets).toEqual([
      { kind: "widget", from: 2, to: 5, widget: "task", checked: false },
    ]);
    expect(ds.filter((d) => d.kind === "conceal" && d.from === 0 && d.to === 2).length).toBe(1);

    // Reveal extent is the whole line — a cursor inside the marker region
    // withholds the task widget.
    const revealedDs = core.decorations(core.revision(), 0, doc.length, cursor(3));
    expect(revealedDs.filter((d) => d.kind === "widget" && d.widget === "task")).toEqual([]);
    // Lockstep reveal: the dash AND the brackets show together as delims.
    expect(marks(revealedDs, "delim")).toEqual([
      { kind: "mark", from: 0, to: 2, style: "delim" },
      { kind: "mark", from: 2, to: 5, style: "delim" },
    ]);
  });

  it("checked task items report checked: true", () => {
    const doc = "- [x] done\nelsewhere";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    const widgets = ds.filter((d) => d.kind === "widget" && d.widget === "task");
    expect(widgets).toEqual([{ kind: "widget", from: 2, to: 5, widget: "task", checked: true }]);
  });

  it("link [text](url) concealed: mark:link over text + two conceal spans", () => {
    const doc = "see [docs](https://example.com) now";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    expect(marks(ds, "link")).toEqual([{ kind: "mark", from: 5, to: 9, style: "link" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 4, to: 5 },
      { kind: "conceal", from: 9, to: 31 },
    ]);
    expect(marks(ds, "url")).toEqual([]);
  });

  it("link revealed: delimiters as mark:delim, destination as mark:url", () => {
    const doc = "see [docs](https://example.com) now";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(6));
    expect(conceals(ds)).toEqual([]);
    expect(marks(ds, "delim")).toEqual([
      { kind: "mark", from: 4, to: 5, style: "delim" },
      { kind: "mark", from: 9, to: 11, style: "delim" },
      { kind: "mark", from: 30, to: 31, style: "delim" },
    ]);
    expect(marks(ds, "url")).toEqual([{ kind: "mark", from: 11, to: 30, style: "url" }]);
    expect(marks(ds, "link")).toEqual([{ kind: "mark", from: 5, to: 9, style: "link" }]);
  });

  it("autolink <url>: mark:link over the whole span, never concealed", () => {
    const doc = "go to <https://example.com> now";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    expect(marks(ds, "link")).toEqual([{ kind: "mark", from: 6, to: 27, style: "link" }]);
    expect(conceals(ds)).toEqual([]);
  });
});

describe("core anchors (v0.2)", () => {
  it("before-bias anchor stays put when an insertion lands exactly on it", () => {
    const { core } = makeCore("abcdef");
    const id = core.createAnchor(3, "before");
    core.applyEdit(core.revision(), [{ at: 3, delete: 0, insert: "XYZ" }], "user");
    expect(core.resolveAnchor(id)).toBe(3);
  });

  it("after-bias anchor moves with an insertion landing exactly on it", () => {
    const { core } = makeCore("abcdef");
    const id = core.createAnchor(3, "after");
    core.applyEdit(core.revision(), [{ at: 3, delete: 0, insert: "XYZ" }], "user");
    expect(core.resolveAnchor(id)).toBe(6);
  });

  it("anchors shift with edits earlier in the document regardless of bias", () => {
    const { core } = makeCore("abcdef");
    const before = core.createAnchor(4, "before");
    const after = core.createAnchor(4, "after");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "123" }], "user");
    expect(core.resolveAnchor(before)).toBe(7);
    expect(core.resolveAnchor(after)).toBe(7);
  });

  it("deleting the anchored text collapses the anchor to the deletion site (not null)", () => {
    const { core } = makeCore("abcdef");
    const id = core.createAnchor(3, "after");
    core.applyEdit(core.revision(), [{ at: 1, delete: 4, insert: "" }], "user"); // deletes "bcde"
    expect(core.resolveAnchor(id)).not.toBeNull();
    expect(core.resolveAnchor(id)).toBe(1);
  });

  it("dropAnchor makes resolveAnchor return null", () => {
    const { core } = makeCore("abc");
    const id = core.createAnchor(1, "before");
    core.dropAnchor(id);
    expect(core.resolveAnchor(id)).toBeNull();
  });
});

describe("core command (v0.2)", () => {
  it("toggleStrong wraps a plain selection, then double-toggle is byte-identical", () => {
    const { core } = makeCore("hello world");
    const c1 = core.command("toggleStrong", 6, 11); // "world"
    expect(c1).not.toBeNull();
    expect(core.getText()).toBe("hello **world**");
    const sel = c1!.selection!;
    const c2 = core.command("toggleStrong", sel.anchor, sel.head);
    expect(c2).not.toBeNull();
    expect(core.getText()).toBe("hello world"); // byte-identical to the original
  });

  it("toggleEm/toggleStrike/toggleCode wrap and unwrap symmetrically", () => {
    const cases: Array<[RangeCommandName, string]> = [
      ["toggleEm", "*"],
      ["toggleStrike", "~~"],
      ["toggleCode", "`"],
    ];
    for (const [name, delim] of cases) {
      const { core } = makeCore("hello world");
      const c1 = core.command(name, 6, 11);
      expect(core.getText()).toBe(`hello ${delim}world${delim}`);
      const sel = c1!.selection!;
      core.command(name, sel.anchor, sel.head);
      expect(core.getText()).toBe("hello world");
    }
  });

  it("commands are single, non-coalescing undo units", () => {
    const { core } = makeCore("hello world");
    core.command("toggleStrong", 6, 11);
    core.undo();
    expect(core.getText()).toBe("hello world");
  });

  it("setHeading sets, no-ops when already at the level, and clears back to a paragraph", () => {
    const { core } = makeCore("Title\n\ntail");
    const c1 = core.command("setHeading", 2, 2);
    expect(c1).not.toBeNull();
    expect(core.getText()).toBe("## Title\n\ntail");
    expect(core.command("setHeading", 2, 2)).toBeNull(); // already level 2: no-op
    core.command("setHeading", 2, 0);
    expect(core.getText()).toBe("Title\n\ntail");
  });

  it("toggleTask flips the checkbox in place and is idempotent", () => {
    const { core } = makeCore("- [ ] buy milk");
    const c1 = core.command("toggleTask", 5);
    expect(c1).not.toBeNull();
    expect(core.getText()).toBe("- [x] buy milk");
    core.command("toggleTask", 5);
    expect(core.getText()).toBe("- [ ] buy milk");
  });

  it("toggleTask returns null when not inside a task item", () => {
    const { core } = makeCore("plain paragraph");
    expect(core.command("toggleTask", 3)).toBeNull();
  });

  it("an unknown command name THROWS InvalidCommand, never null", () => {
    // An unrecognized name is a caller/protocol bug — the core throws
    // `InvalidCommand` (validated before dispatch), never a silent null
    // indistinguishable from "doesn't apply here".
    const { core } = makeCore("hello world");
    expect(() => core.command("bogusCommand" as unknown as RangeCommandName, 0, 1)).toThrow(
      /InvalidCommand/,
    );
    // And it must not have mutated anything on the way to throwing —
    // command() is transactional (docs/boundary-v0.md "Commands").
    expect(core.getText()).toBe("hello world");
  });
});

describe("core indentList/outdentList (v0.2, marker-width-aware Tab nesting)", () => {
  it("bullet under bullet indents by 2", () => {
    const { core } = makeCore("- a\n- b\n");
    core.command("indentList", 6, 6);
    expect(core.getText()).toBe("- a\n  - b\n");
  });

  it("ordered under ordered indents by 3, double-digit marker by 4", () => {
    // Digits rewrite to "1": a non-1 ordered marker cannot interrupt the
    // parent item's paragraph (the paragraph-interruption guard — see the
    // dedicated describe block below), so "   2. b" would de-list.
    const a = makeCore("1. a\n2. b\n").core;
    a.command("indentList", 8, 8);
    expect(a.getText()).toBe("1. a\n   1. b\n");

    const b = makeCore("10. a\n11. b\n").core;
    b.command("indentList", 10, 10);
    expect(b.getText()).toBe("10. a\n    1. b\n");
  });

  it("bullet under ordered indents by 3", () => {
    const { core } = makeCore("1. a\n- b\n");
    core.command("indentList", 7, 7);
    expect(core.getText()).toBe("1. a\n   - b\n");
  });

  it("task under task indents by 2 (checkbox is content, not marker)", () => {
    const { core } = makeCore("- [ ] a\n- [ ] b\n");
    core.command("indentList", 14, 14);
    expect(core.getText()).toBe("- [ ] a\n  - [ ] b\n");
  });

  it("nesting inside a quote stays relative to the quote prefix", () => {
    const { core } = makeCore("> 1. a\n> 2. b\n");
    core.command("indentList", 12, 12);
    // 3 spaces after "> ", digits guard-rewritten to "1".
    expect(core.getText()).toBe("> 1. a\n>    1. b\n");
  });

  it("does not nest across a quote boundary", () => {
    const { core } = makeCore("> - a\n- b\n");
    const rev = core.revision();
    const change = core.command("indentList", 8, 8);
    expect(change).not.toBeNull();
    expect(change!.splices).toEqual([]);
    expect(core.getText()).toBe("> - a\n- b\n");
    expect(core.revision()).toBe(rev);
  });

  it("multi-line selection moves together by the first line's delta", () => {
    const { core } = makeCore("- a\n- b\n- c\n");
    const change = core.command("indentList", 4, 11);
    expect(change!.splices.length).toBe(2);
    expect(core.getText()).toBe("- a\n  - b\n  - c\n");
  });

  it("first item of a list is a no-op that applies (not null) and burns no revision", () => {
    const { core } = makeCore("- a\n- b\n");
    const rev = core.revision();
    const change = core.command("indentList", 2, 2);
    expect(change).not.toBeNull();
    expect(change!.splices).toEqual([]);
    // `selection` is optional on the wire (protocol: `selection?: ... | null`);
    // the wasm core omits it for a no-op change.
    expect(change!.selection ?? null).toBeNull();
    expect(core.revision()).toBe(rev);
    expect(core.getText()).toBe("- a\n- b\n");
  });

  it("a non-list range does not apply", () => {
    const { core } = makeCore("plain paragraph\n");
    expect(core.command("indentList", 3, 3)).toBeNull();
  });

  it("outdent reverses each indent case", () => {
    for (const [indented, from, to, flat] of [
      ["- a\n  - b\n", 8, 8, "- a\n- b\n"],
      ["1. a\n   1. b\n", 11, 11, "1. a\n1. b\n"],
      ["10. a\n    1. b\n", 13, 13, "10. a\n1. b\n"],
      ["1. a\n   - b\n", 10, 10, "1. a\n- b\n"],
      ["- [ ] a\n  - [ ] b\n", 16, 16, "- [ ] a\n- [ ] b\n"],
      ["> 1. a\n>    1. b\n", 15, 15, "> 1. a\n> 1. b\n"],
    ] as Array<[string, number, number, string]>) {
      const { core } = makeCore(indented);
      core.command("outdentList", from, to);
      expect(core.getText()).toBe(flat);
    }
  });

  it("outdent at top level is a no-op", () => {
    const { core } = makeCore("- a\n- b\n");
    const rev = core.revision();
    const change = core.command("outdentList", 6, 6);
    expect(change).not.toBeNull();
    expect(change!.splices).toEqual([]);
    expect(core.revision()).toBe(rev);
  });

  it("outdent clamps to a line's own leading space count", () => {
    const { core } = makeCore("- p\n  - a\n - c\n");
    const change = core.command("outdentList", 8, 13);
    expect(change!.splices.map((s) => s.delete)).toEqual([2, 1]);
    expect(core.getText()).toBe("- p\n- a\n- c\n");
  });

  it("undo restores a multi-line indent in one step", () => {
    const { core } = makeCore("- a\n- b\n- c\n");
    core.command("indentList", 4, 11);
    expect(core.getText()).toBe("- a\n  - b\n  - c\n");
    core.undo();
    expect(core.getText()).toBe("- a\n- b\n- c\n");
    core.redo();
    expect(core.getText()).toBe("- a\n  - b\n  - c\n");
  });

  // --- subtree-aware affected set -----------------------------------------

  it("indenting a parent moves its whole subtree with it", () => {
    const { core } = makeCore("- x\n- p\n  - c1\n  - c2\n");
    const change = core.command("indentList", 6, 6);
    expect(change!.splices.length).toBe(3);
    expect(core.getText()).toBe("- x\n  - p\n    - c1\n    - c2\n");
  });

  it("outdent reverses a subtree move", () => {
    const { core } = makeCore("- x\n  - p\n    - c1\n    - c2\n");
    const change = core.command("outdentList", 8, 8);
    expect(change!.splices.length).toBe(3);
    expect(core.getText()).toBe("- x\n- p\n  - c1\n  - c2\n");
  });

  it("subtree does not include a following sibling (equal marker column)", () => {
    const { core } = makeCore("- x\n- p\n  - c1\n- sibling\n");
    const change = core.command("indentList", 6, 6);
    expect(change!.splices.length).toBe(2);
    expect(core.getText()).toBe("- x\n  - p\n    - c1\n- sibling\n");
  });

  it("subtree walk stops at a blank line", () => {
    const { core } = makeCore("- x\n- p\n  - c1\n\n  - c2\n");
    const change = core.command("indentList", 6, 6);
    expect(change!.splices.length).toBe(2);
    expect(core.getText()).toBe("- x\n  - p\n    - c1\n\n  - c2\n");
  });

  it("subtree inside a quote respects quote depth", () => {
    const { core } = makeCore("> - x\n> - p\n>   - c1\n- outside\n");
    const change = core.command("indentList", 10, 10);
    expect(change!.splices.length).toBe(2);
    expect(core.getText()).toBe("> - x\n>   - p\n>     - c1\n- outside\n");
  });

  // --- paragraph-interruption guard ----------------------------------------
  //
  // A non-1 ordered marker cannot START a list in paragraph-interruption
  // position (CommonMark): the moved line's digits rewrite to "1" unless it
  // lands where a same-delimiter ordered list is already open.

  it("chained Tab then Shift-Tab on the flagship repro (one core, no reload)", () => {
    const doc =
      "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n3. ordered three\n";
    const { core } = makeCore(doc);

    const change = core.command("indentList", 21, 21)!;
    expect(core.getText()).toBe(
      "1. ordered one\n   1. ordered two\n      1. nested ordered item\n      - a bullet nested under an ordered item\n3. ordered three\n",
    );
    expect(change.splices.length).toBe(4); // 3 indents + 1 digit rewrite

    // Shift-Tab at the returned selection restores the nesting structure
    // (numbers may differ from the original bytes — structure, not bytes).
    const sel = change.selection!;
    const out = core.command("outdentList", sel.anchor, sel.head);
    expect(out).not.toBeNull();
    expect(out!.splices.length).toBeGreaterThan(0);
    expect(core.getText()).toBe(
      "1. ordered one\n1. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n3. ordered three\n",
    );
  });

  it("joining an open ordered sublist keeps the number", () => {
    const { core } = makeCore("1. a\n   1. a1\n2. b\n");
    core.command("indentList", 17, 17);
    expect(core.getText()).toBe("1. a\n   1. a1\n   2. b\n");
  });

  it("landing on a bullet family at the same column rewrites to 1", () => {
    const { core } = makeCore("1. a\n   - a1\n2. b\n");
    core.command("indentList", 16, 16);
    expect(core.getText()).toBe("1. a\n   - a1\n   1. b\n");
  });

  it("outdent rejoining an open ordered list keeps the number", () => {
    const { core } = makeCore("1. a\n   1. b\n      1. b1\n   3. c\n");
    core.command("outdentList", 31, 31);
    expect(core.getText()).toBe("1. a\n   1. b\n      1. b1\n3. c\n");
  });

  it("outdent onto a bullet parent rewrites to 1", () => {
    const { core } = makeCore("- a\n  1. b\n  2. c\n");
    core.command("outdentList", 16, 16);
    expect(core.getText()).toBe("- a\n  1. b\n1. c\n");
  });

  it("the rewrite shares the command's single undo unit", () => {
    const { core } = makeCore("1. a\n2. b\n");
    core.command("indentList", 8, 8);
    expect(core.getText()).toBe("1. a\n   1. b\n");
    core.undo();
    expect(core.getText()).toBe("1. a\n2. b\n"); // digits AND indent restored together
  });

  // --- below-context interruption guard ------------------------------------
  //
  // The edit can change the parse context of a line BELOW the affected set
  // that the command never touched: the same landing-scan check runs on the
  // first unaffected item line below (skipping adopted descendants).

  const ORDERED_TORTURE =
    "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n   - [x] a task nested under an ordered item\n3. ordered three\n";

  it("outdenting the nested bullet rewrites the below '3.' it re-contexted and adopts the task", () => {
    const { core } = makeCore(ORDERED_TORTURE);
    const pos = ORDERED_TORTURE.indexOf("a bullet");
    core.command("outdentList", pos, pos);
    expect(core.getText()).toBe(
      "1. ordered one\n2. ordered two\n   1. nested ordered item\n- a bullet nested under an ordered item\n   - [x] a task nested under an ordered item\n1. ordered three\n",
    );
  });

  it("outdenting the nested task rewrites the below '3.' the same way", () => {
    const { core } = makeCore(ORDERED_TORTURE);
    const pos = ORDERED_TORTURE.indexOf("a task");
    core.command("outdentList", pos, pos);
    expect(core.getText()).toBe(
      "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n- [x] a task nested under an ordered item\n1. ordered three\n",
    );
  });

  it("the below line is byte-untouched when there is no interruption hazard", () => {
    const pos = "1. a\n   - b\n- c\n".indexOf("b");
    // A bullet below: never rewritten.
    const a = makeCore("1. a\n   - b\n- c\n").core;
    a.command("outdentList", pos, pos);
    expect(a.getText()).toBe("1. a\n- b\n- c\n");
    // An ordered "1." below: already safe.
    const b = makeCore("1. a\n   - b\n1. c\n").core;
    b.command("outdentList", pos, pos);
    expect(b.getText()).toBe("1. a\n- b\n1. c\n");
    // Below line continues the moved line's open same-flavor list.
    const c = makeCore("1. a\n   1. b\n2. c\n").core;
    c.command("outdentList", pos, pos);
    expect(c.getText()).toBe("1. a\n1. b\n2. c\n");
  });

  it("user sequence: Shift-Tab x2 then Tab x2 on the nested bullet (one core)", () => {
    const { core } = makeCore(ORDERED_TORTURE);
    let pos = ORDERED_TORTURE.indexOf("a bullet");
    const track = (c: { selection?: { head: number } | null } | null, p: number) =>
      c?.selection ? c.selection.head : p;

    let c = core.command("outdentList", pos, pos);
    pos = track(c, pos);
    expect(core.getText()).toBe(
      "1. ordered one\n2. ordered two\n   1. nested ordered item\n- a bullet nested under an ordered item\n   - [x] a task nested under an ordered item\n1. ordered three\n",
    );

    c = core.command("outdentList", pos, pos); // top level: applies-but-no-op
    expect(c!.splices).toEqual([]);
    pos = track(c, pos);

    c = core.command("indentList", pos, pos); // re-nest; adopted task carried
    pos = track(c, pos);
    expect(core.getText()).toBe(
      "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n      - [x] a task nested under an ordered item\n1. ordered three\n",
    );

    c = core.command("indentList", pos, pos); // under the nested ordered item
    pos = track(c, pos);
    expect(core.getText()).toBe(
      "1. ordered one\n2. ordered two\n   1. nested ordered item\n      - a bullet nested under an ordered item\n         - [x] a task nested under an ordered item\n1. ordered three\n",
    );
  });

  it("user sequence: Shift-Tab x2 then Tab x2 on the nested task (one core)", () => {
    const { core } = makeCore(ORDERED_TORTURE);
    let pos = ORDERED_TORTURE.indexOf("a task");
    const track = (c: { selection?: { head: number } | null } | null, p: number) =>
      c?.selection ? c.selection.head : p;

    let c = core.command("outdentList", pos, pos);
    pos = track(c, pos);
    expect(core.getText()).toBe(
      "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n- [x] a task nested under an ordered item\n1. ordered three\n",
    );

    c = core.command("outdentList", pos, pos);
    expect(c!.splices).toEqual([]);
    pos = track(c, pos);

    c = core.command("indentList", pos, pos);
    pos = track(c, pos);
    expect(core.getText()).toBe(
      "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n   - [x] a task nested under an ordered item\n1. ordered three\n",
    );

    c = core.command("indentList", pos, pos); // under the bullet sibling (+2)
    pos = track(c, pos);
    expect(core.getText()).toBe(
      "1. ordered one\n2. ordered two\n   1. nested ordered item\n   - a bullet nested under an ordered item\n     - [x] a task nested under an ordered item\n1. ordered three\n",
    );
  });
});

describe("core enter (v0.3, construct-aware Enter)", () => {
  // crates/oxidown-core/src/commands.rs `enter` (module doc comment "##
  // enter"): continue on non-empty content, single-press exit on empty
  // constructs, null when neither applies.

  // -- continue -------------------------------------------------------------

  it("continues each bullet flavor with the source glyph", () => {
    for (const glyph of ["-", "*", "+"]) {
      const doc = `${glyph} a\n`;
      const { core } = makeCore(doc);
      const pos = doc.indexOf("a") + 1;
      core.command("enter", pos, pos);
      expect(core.getText()).toBe(`${glyph} a\n${glyph} \n`);
    }
  });

  it("increments an ordered marker's raw source digits, same delimiter", () => {
    const { core } = makeCore("6) a\n");
    core.command("enter", 4, 4);
    expect(core.getText()).toBe("6) a\n7) \n");
  });

  it("grows the digit width naturally (9. -> 10.)", () => {
    const { core } = makeCore("9. a\n");
    const change = core.command("enter", 4, 4);
    expect(core.getText()).toBe("9. a\n10. \n");
    expect(change!.selection).toEqual({ anchor: 9, head: 9 });
  });

  it("task continuation always starts unchecked", () => {
    const a = makeCore("- [ ] a\n").core;
    a.command("enter", 7, 7);
    expect(a.getText()).toBe("- [ ] a\n- [ ] \n");

    const b = makeCore("- [x] a\n").core;
    b.command("enter", 7, 7);
    expect(b.getText()).toBe("- [x] a\n- [ ] \n");
  });

  it("keeps the quote prefix when continuing a list inside a quote", () => {
    const { core } = makeCore("> - a\n");
    core.command("enter", 5, 5);
    expect(core.getText()).toBe("> - a\n> - \n");
  });

  it("continues a plain quote line with its exact prefix", () => {
    const { core } = makeCore("> text\n");
    core.command("enter", 6, 6);
    expect(core.getText()).toBe("> text\n> \n");
  });

  it("a nested item continues at its own indent", () => {
    const doc = "- a\n  - b\n";
    const { core } = makeCore(doc);
    core.command("enter", 9, 9);
    expect(core.getText()).toBe("- a\n  - b\n  - \n");
  });

  it("mid-line Enter splits the item (trailing text becomes the new item)", () => {
    const { core } = makeCore("- hello world\n");
    core.command("enter", "- hello ".length, "- hello ".length);
    expect(core.getText()).toBe("- hello \n- world\n");
  });

  it("a selection is deleted and continued in one batch", () => {
    const { core } = makeCore("- hello world\n");
    const change = core.command("enter", 2, "- hello".length);
    expect(core.getText()).toBe("- \n-  world\n");
    expect(change!.splices.length).toBe(1);
  });

  // -- exit (single press per level) ----------------------------------------

  it("an empty nested item outdents ONE level per press, no newline inserted", () => {
    const doc = "- a\n  - b\n  - \n";
    const { core } = makeCore(doc);
    const change = core.command("enter", doc.length - 1, doc.length - 1);
    expect(core.getText()).toBe("- a\n  - b\n- \n");
    expect(change!.splices.every((s) => !s.insert.includes("\n"))).toBe(true);
  });

  it("outdenting an empty nested task fires the below-line rewrite guard", () => {
    // Outdenting "   - [ ] " to top level puts the untouched "3. c" against
    // the new bullet list, where the guard's landing-scan says a non-1
    // ordered marker cannot start a list.
    const doc = "1. a\n2. b\n   - [ ] x\n   - [ ] \n3. c\n";
    const { core } = makeCore(doc);
    const pos = doc.indexOf("   - [ ] \n") + "   - [ ] ".length;
    const change = core.command("enter", pos, pos);
    expect(core.getText()).toBe("1. a\n2. b\n   - [ ] x\n- [ ] \n1. c\n");
    expect(change!.splices.length).toBe(2);
  });

  it("an empty top-level item clears its marker (no newline)", () => {
    const { core } = makeCore("- a\n- \n");
    const change = core.command("enter", 6, 6);
    expect(core.getText()).toBe("- a\n\n");
    expect(change!.selection).toEqual({ anchor: 4, head: 4 });
  });

  it("an empty top-level task clears marker AND brackets", () => {
    const doc = "- [ ] a\n- [ ] \n";
    const { core } = makeCore(doc);
    core.command("enter", doc.length - 1, doc.length - 1);
    expect(core.getText()).toBe("- [ ] a\n\n");
  });

  it("an empty quote line drops ONE level per press ('> > ' -> '> ' -> plain)", () => {
    const { core } = makeCore("> > x\n> > \n");
    core.command("enter", "> > x\n> > ".length, "> > x\n> > ".length);
    expect(core.getText()).toBe("> > x\n> \n");
    core.command("enter", "> > x\n> ".length, "> > x\n> ".length);
    expect(core.getText()).toBe("> > x\n\n");
  });

  it("an empty top-level item inside a quote clears the marker but keeps '> '", () => {
    const doc = "> - a\n> - \n";
    const { core } = makeCore(doc);
    core.command("enter", doc.length - 1, doc.length - 1);
    expect(core.getText()).toBe("> - a\n> \n");
  });

  it("an empty nested item inside a quote outdents within the quote", () => {
    const doc = "> - a\n>   - b\n>   - \n";
    const { core } = makeCore(doc);
    core.command("enter", doc.length - 1, doc.length - 1);
    expect(core.getText()).toBe("> - a\n>   - b\n> - \n");
  });

  it("continues an ordered task with incremented digits and fresh brackets", () => {
    const { core } = makeCore("1. [x] a\n");
    core.command("enter", 8, 8);
    expect(core.getText()).toBe("1. [x] a\n2. [ ] \n");
  });

  // -- null (falls back to the view's default newline) -----------------------

  it("returns null on a plain paragraph", () => {
    const { core } = makeCore("plain text\n");
    expect(core.command("enter", 5, 5)).toBeNull();
  });

  it("returns null when the cursor sits inside the marker/quote prefix", () => {
    const a = makeCore("- item\n").core;
    expect(a.command("enter", 1, 1)).toBeNull();
    const b = makeCore("> text\n").core;
    expect(b.command("enter", 1, 1)).toBeNull();
  });

  it("returns null on a heading line", () => {
    const { core } = makeCore("# Heading\n");
    expect(core.command("enter", 5, 5)).toBeNull();
  });

  // -- undo ------------------------------------------------------------------

  it("each press is a single, non-coalescing undo unit", () => {
    const { core } = makeCore("- a\n");
    core.command("enter", 3, 3);
    expect(core.getText()).toBe("- a\n- \n");
    core.command("enter", 6, 6); // exit the fresh empty item
    expect(core.getText()).toBe("- a\n\n");
    core.undo();
    // One press per undo step.
    expect(core.getText()).toBe("- a\n- \n");
    core.undo();
    expect(core.getText()).toBe("- a\n");
  });
});

describe("core streaming (v0.2)", () => {
  it("streamOpen/Append/Close: appends land at the (mapped) insertion anchor", () => {
    const { core } = makeCore("head\n");
    const id = core.streamOpen(core.docLength());
    const c1 = core.streamAppend(id, "Hello");
    expect(c1.splices).toEqual([{ at: 5, delete: 0, insert: "Hello" }]);
    const c2 = core.streamAppend(id, ", world");
    expect(c2.splices).toEqual([{ at: 10, delete: 0, insert: ", world" }]);
    core.streamClose(id);
    expect(core.getText()).toBe("head\nHello, world");
  });

  it("streamAppend never moves the selection — user edits elsewhere are unaffected", () => {
    const { core } = makeCore("");
    const id = core.streamOpen(0);
    const change = core.streamAppend(id, "abc");
    // `selection` is optional on the wire; streaming changes omit it.
    expect(change.selection ?? null).toBeNull();
  });

  it("an entire stream session is one undo unit when uninterrupted", () => {
    const { core } = makeCore("X");
    const id = core.streamOpen(1);
    core.streamAppend(id, "a");
    core.streamAppend(id, "b");
    core.streamAppend(id, "c");
    core.streamClose(id);
    expect(core.getText()).toBe("Xabc");
    core.undo();
    expect(core.getText()).toBe("X");
  });

  it("an interleaved user edit gets its own unit; the STREAM stays one unit (creation-order undo)", () => {
    // Boundary v0.2 clarification 2 (history.rs `record_stream_append`): an
    // entire stream (open→close) is ONE undo unit even with user edits
    // interleaved, and undo order is unit-CREATION order (LIFO by creation).
    // The user edit's unit was created AFTER the stream's unit began, so it
    // pops first; the second undo then reverts the whole stream (A+B
    // together).
    const { core, clock } = makeCore("head\n\ntail");
    const id = core.streamOpen(core.docLength());
    core.streamAppend(id, "A");
    clock.advance(10);
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "USER" }], "user");
    core.streamAppend(id, "B");
    core.streamClose(id);
    expect(core.getText()).toBe("USERhead\n\ntailAB");

    core.undo(); // the user edit's unit (created mid-stream) pops first
    expect(core.getText()).toBe("head\n\ntailAB");
    core.undo(); // the stream's single unit reverts BOTH chunks at once
    expect(core.getText()).toBe("head\n\ntail");
    expect(core.undo()).toBeNull();

    // Redo round trip restores the same units in reverse.
    core.redo();
    expect(core.getText()).toBe("head\n\ntailAB");
    core.redo();
    expect(core.getText()).toBe("USERhead\n\ntailAB");
    expect(core.redo()).toBeNull();
  });

  it("the undo cascade maps stream positions through MULTI-SPLICE user batches exactly (multi-cursor)", () => {
    // A single multi-cursor applyEdit batch with splices on BOTH sides of
    // the stream insertion point. The cascade must map the append position
    // through the RECORDED splice batch (history.rs record_stream_append) —
    // a coarse prefix/suffix diff would collapse the whole batch to one
    // splice spanning both edits and teleport a stream position strictly
    // inside that span to the splice's start.
    const { core, clock } = makeCore("aaaa bbbb cccc");
    const id = core.streamOpen(7); // between "bb" and "bb"
    core.streamAppend(id, "Y"); // creates the stream's (single) undo unit
    expect(core.getText()).toBe("aaaa bbYbb cccc");
    clock.advance(10);
    // ONE user batch, splices on BOTH sides of the stream point (anchor 8).
    core.applyEdit(
      core.revision(),
      [
        { at: 2, delete: 0, insert: "11" },
        { at: 12, delete: 0, insert: "22" },
      ],
      "user",
    );
    expect(core.getText()).toBe("aa11aa bbYbb c22ccc");
    // Second chunk: its position must cascade through the user unit's
    // RECORDED two-splice batch (down to 8 in that unit's `before` frame),
    // not through a coarse whole-text diff (which spans [2, 16) here and
    // would collapse the position to 2).
    const change = core.streamAppend(id, "Z");
    expect(change.splices).toEqual([{ at: 10, delete: 0, insert: "Z" }]);
    expect(core.getText()).toBe("aa11aa bbYZbb c22ccc");
    core.streamClose(id);

    core.undo(); // the user batch pops; the streamed text stays put EXACTLY
    expect(core.getText()).toBe("aaaa bbYZbb cccc");
    core.undo(); // the stream's single unit reverts both chunks
    expect(core.getText()).toBe("aaaa bbbb cccc");
    expect(core.undo()).toBeNull();

    // Redo round trip is exact too.
    core.redo();
    expect(core.getText()).toBe("aaaa bbYZbb cccc");
    core.redo();
    expect(core.getText()).toBe("aa11aa bbYZbb c22ccc");
    expect(core.redo()).toBeNull();
  });

  it("the cascade stays exact across SEVERAL interleaved multi-splice batches and appends", () => {
    const { core, clock } = makeCore("head middle tail");
    const id = core.streamOpen("head m".length); // 6, inside "middle"
    core.streamAppend(id, "A");
    clock.advance(10);
    // Batch 1: splices before and after the stream point.
    core.applyEdit(
      core.revision(),
      [
        { at: 0, delete: 4, insert: "H" },
        { at: 12, delete: 0, insert: "!" },
      ],
      "user",
    );
    core.streamAppend(id, "B");
    clock.advance(600); // never coalesce with the previous batch
    // Batch 2: another straddling multi-splice batch.
    core.applyEdit(
      core.revision(),
      [
        { at: 2, delete: 1, insert: "" },
        { at: core.docLength(), delete: 0, insert: "$" },
      ],
      "user",
    );
    core.streamAppend(id, "C");
    core.streamClose(id);

    const finalText = core.getText();
    // Drain the history fully, then replay it: every frame must round-trip.
    const frames: string[] = [finalText];
    for (;;) {
      const change = core.undo();
      if (change === null) break;
      frames.push(core.getText());
    }
    expect(frames[frames.length - 1]).toBe("head middle tail");
    const replay: string[] = [];
    for (;;) {
      const change = core.redo();
      if (change === null) break;
      replay.push(core.getText());
    }
    expect(replay).toEqual(frames.slice(0, -1).reverse());
    expect(core.getText()).toBe(finalText);
    // The first undo (batch 2) must keep ALL streamed chunks intact and in
    // order between the surviving neighbors.
    core.undo();
    expect(core.getText().replace(/[^ABC]/g, "")).toBe("ABC");
  });

  it("a user edit INSIDE the streamed region never loses text to the stream's undo", () => {
    // The cascade must keep every unit's snapshot sound: undoing the user
    // edit first, then the stream, restores the exact original document.
    const { core, clock } = makeCore("X");
    const id = core.streamOpen(1);
    core.streamAppend(id, "abc"); // "Xabc"
    clock.advance(10);
    core.applyEdit(core.revision(), [{ at: 2, delete: 1, insert: "" }], "user"); // "Xac"
    core.streamAppend(id, "de"); // appended at the mapped anchor -> "Xacde"
    core.streamClose(id);
    expect(core.getText()).toBe("Xacde");
    core.undo(); // user deletion restored (streamed text untouched)
    expect(core.getText()).toBe("Xabcde");
    core.undo(); // the whole stream reverts
    expect(core.getText()).toBe("X");
  });

  it("streamAppend on an unknown/closed id throws; streamClose on one is a no-op", () => {
    const { core } = makeCore("x");
    expect(() => core.streamAppend(999, "a")).toThrow();
    const id = core.streamOpen(1);
    core.streamClose(id);
    expect(() => core.streamAppend(id, "a")).toThrow();
    expect(() => core.streamClose(id)).not.toThrow();
    expect(() => core.streamClose(999)).not.toThrow();
  });

  it("the stream anchor maps through a user edit earlier in the document", () => {
    const { core } = makeCore("head\n");
    const id = core.streamOpen(core.docLength()); // anchor at 5
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "XXXX" }], "user"); // shifts anchor to 9
    const change = core.streamAppend(id, "Z");
    expect(change.splices).toEqual([{ at: 9, delete: 0, insert: "Z" }]);
    core.streamClose(id);
    expect(core.getText()).toBe("XXXXhead\nZ");
  });
});

// ---------------------------------------------------------------------------
// Parser fidelity and command semantics pinned by the M1 review (S1–S5, S13)
// ---------------------------------------------------------------------------

describe("core parser fidelity (S13a–c): flanking rules and code spans", () => {
  const ds = (doc: string, sel: SelectionRange[] = []) => {
    const { core } = makeCore(doc);
    return core.decorations(core.revision(), 0, doc.length, sel);
  };

  it("intraword `_` never emphasizes: a_snake_case_word", () => {
    const d = ds("a_snake_case_word");
    expect(marks(d)).toEqual([]);
    expect(conceals(d)).toEqual([]);
  });

  it("space-flanked ** is inert: `a ** b ** c` has no strong (corpus parity, cases.rs:45)", () => {
    const d = ds("a ** b ** c");
    expect(marks(d)).toEqual([]);
    expect(conceals(d)).toEqual([]);
  });

  it("intraword `*` still emphasizes (only `_` has the intraword rule)", () => {
    const d = ds("a*b*c");
    expect(marks(d, "em")).toEqual([{ kind: "mark", from: 2, to: 3, style: "em" }]);
  });

  it("intraword `__` never strongs, word-edged `__` does", () => {
    expect(marks(ds("a__b__c"))).toEqual([]);
    expect(marks(ds("__b__ c"), "strong")).toEqual([
      { kind: "mark", from: 2, to: 3, style: "strong" },
    ]);
  });

  it("punctuation before a closer still closes: **foo.** and *(em)*", () => {
    expect(marks(ds("**foo.** x"), "strong")).toEqual([
      { kind: "mark", from: 2, to: 6, style: "strong" },
    ]);
    expect(marks(ds("*(em)* x"), "em")).toEqual([{ kind: "mark", from: 1, to: 5, style: "em" }]);
  });

  it("space-flanked ~~ is inert; word-edged ~~ still strikes; 3+ tildes never strike", () => {
    expect(marks(ds("a ~~ b ~~ c"))).toEqual([]);
    expect(marks(ds("~~s~~ x"), "strike")).toEqual([
      { kind: "mark", from: 2, to: 3, style: "strike" },
    ]);
    expect(marks(ds("a ~~~x~~~ b"))).toEqual([]);
  });

  it("em spans a nested strong instead of closing on its opener: *foo **bar** baz*", () => {
    const d = ds("*foo **bar** baz*");
    expect(marks(d, "em")).toEqual([{ kind: "mark", from: 1, to: 16, style: "em" }]);
    expect(marks(d, "strong")).toEqual([{ kind: "mark", from: 7, to: 10, style: "strong" }]);
  });

  it("S13b: multi-backtick delimiters pair EQUAL-length runs — say ``x`` ok is ONE span", () => {
    const d = ds("say ``x`` ok");
    expect(marks(d, "code")).toEqual([{ kind: "mark", from: 6, to: 7, style: "code" }]);
    expect(conceals(d)).toEqual([
      { kind: "conceal", from: 4, to: 6 },
      { kind: "conceal", from: 7, to: 9 },
    ]);
  });

  it("S13b: a shorter run inside stays content: ``code with ` backtick``", () => {
    const d = ds("``code with ` backtick``");
    expect(marks(d, "code")).toEqual([{ kind: "mark", from: 2, to: 22, style: "code" }]);
    expect(conceals(d)).toEqual([
      { kind: "conceal", from: 0, to: 2 },
      { kind: "conceal", from: 22, to: 24 },
    ]);
  });

  it("S13b: an unmatched backtick run is literal text", () => {
    const d = ds("text `unterminated code");
    expect(marks(d)).toEqual([]);
    expect(conceals(d)).toEqual([]);
  });

  it("S13c: code spans scan first — emphasis delimiters inside are inert: *a `b*` c*", () => {
    const d = ds("*a `b*` c*");
    expect(marks(d, "code")).toEqual([{ kind: "mark", from: 4, to: 6, style: "code" }]);
    expect(marks(d, "em")).toEqual([{ kind: "mark", from: 1, to: 9, style: "em" }]);
    expect(conceals(d)).toEqual([
      { kind: "conceal", from: 0, to: 1 },
      { kind: "conceal", from: 3, to: 4 },
      { kind: "conceal", from: 6, to: 7 },
      { kind: "conceal", from: 9, to: 10 },
    ]);
  });
});

describe("core list marker span (S13d, v0.4): glyphs plus ALL following spaces/tabs", () => {
  const widgets = (doc: string) => {
    const { core } = makeCore(doc);
    return core
      .decorations(core.revision(), 0, doc.length, [])
      .filter((x) => x.kind === "widget");
  };

  it("single space: `- item` → bullet widget [0, 2)", () => {
    expect(widgets("- item")).toEqual([{ kind: "widget", from: 0, to: 2, widget: "bullet" }]);
  });

  it("`-   spaced item`: ALL post-marker spaces are marker territory → [0, 4)", () => {
    expect(widgets("-   spaced item")).toEqual([
      { kind: "widget", from: 0, to: 4, widget: "bullet" },
    ]);
  });

  it("five spaces → [0, 6); six spaces hit the indented-code boundary → still [0, 6)", () => {
    expect(widgets("-     five spaces")).toEqual([
      { kind: "widget", from: 0, to: 6, widget: "bullet" },
    ]);
    expect(widgets("-      six spaces")).toEqual([
      { kind: "widget", from: 0, to: 6, widget: "bullet" },
    ]);
  });

  it("tabs count as marker whitespace: `- \\t tab item` → [0, 4)", () => {
    expect(widgets("- \t tab item")).toEqual([
      { kind: "widget", from: 0, to: 4, widget: "bullet" },
    ]);
  });

  it("`1.   spaced ordered`: the ordered widget covers `1.   ` → [0, 5)", () => {
    expect(widgets("1.   spaced ordered")).toEqual([
      { kind: "widget", from: 0, to: 5, widget: "ordered", number: 1, delim: "." },
    ]);
  });

  it("spaced task `- [ ]   x`: marker conceal ends at `[`; post-checkbox spaces are content", () => {
    const doc = "- [ ]   spaced task";
    const { core } = makeCore(doc);
    const d = core.decorations(core.revision(), 0, doc.length, []);
    expect(d.filter((x) => x.kind === "widget")).toEqual([
      { kind: "widget", from: 2, to: 5, widget: "task", checked: false },
    ]);
    expect(conceals(d)).toEqual([{ kind: "conceal", from: 0, to: 2 }]);
  });

  it("an EMPTY item keeps glyphs + ONE trailing space: `-   ` → [0, 2)", () => {
    expect(widgets("-   ")).toEqual([{ kind: "widget", from: 0, to: 2, widget: "bullet" }]);
  });

  it("Enter's content start is the FIXED marker token end (glyphs + one space), not the ws-run end", () => {
    // ADAPTED from the mock port: the mock reused the S13d all-whitespace
    // lookahead for `enter`'s prefix gate and returned null at position 3.
    // Per the contract, `enter`'s "content start = the marker token's end"
    // uses the indentList section's FIXED token (glyphs + exactly one
    // space), so position 3 — inside the extra spaces — is already at/after
    // content start: the press CONTINUES (splitting the item mid-line). The
    // S13d all-whitespace rule governs the decoration SPAN only.
    const { core } = makeCore("-   spaced\n");
    const mid = core.command("enter", 3, 3);
    expect(mid).not.toBeNull();
    expect(core.getText()).toBe("-  \n-  spaced\n");
    core.undo();
    core.command("enter", 10, 10); // at the item's end: continues the list
    expect(core.getText()).toBe("-   spaced\n- \n");
  });
});

describe("core toggle whitespace trimming (S1)", () => {
  it("a whitespace-edged selection trims before wrapping, double-toggle is byte-identical", () => {
    const { core } = makeCore("a b");
    const c1 = core.command("toggleStrong", 0, 2);
    expect(core.getText()).toBe("**a** b");
    expect(c1!.splices).toEqual([
      { at: 0, delete: 0, insert: "**" },
      { at: 1, delete: 0, insert: "**" },
    ]);
    expect(c1!.selection).toEqual({ anchor: 2, head: 3 });
    const c2 = core.command("toggleStrong", c1!.selection!.anchor, c1!.selection!.head);
    expect(c2).not.toBeNull();
    expect(core.getText()).toBe("a b");
    expect(c2!.selection).toEqual({ anchor: 0, head: 1 });
  });

  it("a selection over a trailing newline trims onto the line: 'ab\\ncd' 0..3", () => {
    const { core } = makeCore("ab\ncd");
    core.command("toggleStrong", 0, 3);
    expect(core.getText()).toBe("**ab**\ncd");
  });

  it("a whitespace-only selection does not apply: null, no undo unit, no revision bump", () => {
    const { core } = makeCore("a   b");
    const rev = core.revision();
    expect(core.command("toggleStrong", 1, 4)).toBeNull();
    expect(core.command("toggleEm", 2, 3)).toBeNull();
    expect(core.command("toggleStrike", 1, 4)).toBeNull();
    expect(core.getText()).toBe("a   b");
    expect(core.revision()).toBe(rev);
    expect(core.undo()).toBeNull();
  });

  it("OFF detection operates on the trimmed range", () => {
    const { core } = makeCore("x **a** y");
    core.command("toggleStrong", 1, 8); // " **a** " — trims to the whole node
    expect(core.getText()).toBe("x a y");
  });

  it("exotic contract whitespace (NBSP, ideographic space) trims too", () => {
    const { core } = makeCore(" a　");
    core.command("toggleEm", 0, 3);
    expect(core.getText()).toBe(" *a*　");
  });

  it("toggleCode does NOT trim — the code planner pads space-edged content instead", () => {
    // ADAPTED from the mock port: the mock emitted bare backticks (`a `b);
    // the core's planner pads a space-edged selection ("` a  `") so
    // CommonMark's both-ends space stripping round-trips the exact content
    // — the v0.4 contract text pins "the code planner's existing padding
    // treatment stands".
    const { core } = makeCore("a b");
    const c = core.command("toggleCode", 0, 2);
    expect(core.getText()).toBe("` a  `b");
    expect(c!.splices).toEqual([
      { at: 0, delete: 0, insert: "` " },
      { at: 2, delete: 0, insert: " `" },
    ]);
  });

  it("cursor-only toggles are unchanged (no trimming path)", () => {
    const { core } = makeCore("ab");
    const c = core.command("toggleStrong", 1, 1);
    expect(core.getText()).toBe("a****b");
    expect(c!.selection).toEqual({ anchor: 3, head: 3 });
  });
});

describe("core CRLF split guard (S2)", () => {
  it("a command position between \\r and \\n throws the pinned InvalidArgument, mutating nothing", () => {
    const { core } = makeCore("one\r\ntwo");
    const rev = core.revision();
    const msg = "InvalidArgument: position 4 splits a CRLF sequence";
    for (const run of [
      () => core.command("toggleStrong", 4, 8),
      () => core.command("toggleStrong", 0, 4),
      () => core.command("toggleEm", 4, 4),
      () => core.command("toggleCode", 4, 6),
      () => core.command("toggleStrike", 4, 6),
      () => core.command("setHeading", 4, 2),
      () => core.command("toggleTask", 4),
      () => core.command("indentList", 4, 4),
      () => core.command("outdentList", 4, 4),
      () => core.command("enter", 4, 4),
    ]) {
      expect(run).toThrow(msg);
    }
    expect(core.getText()).toBe("one\r\ntwo");
    expect(core.revision()).toBe(rev);
    expect(core.undo()).toBeNull();
  });

  it("positions on either side of a CRLF pair still work", () => {
    const { core } = makeCore("one\r\ntwo");
    core.command("setHeading", 5, 1);
    expect(core.getText()).toBe("one\r\n# two");
  });
});

describe("core setHeading block gate and level-0 removal (S3)", () => {
  it("S3a: refuses a list item inside a blockquote: '> - item'", () => {
    const { core } = makeCore("> - item");
    const rev = core.revision();
    expect(core.command("setHeading", 5, 2)).toBeNull();
    expect(core.getText()).toBe("> - item");
    expect(core.revision()).toBe(rev);
  });

  it("S3a: refuses list items / hr / fences / blank lines at top level the same way", () => {
    for (const doc of ["- item", "1. item", "---", "```js", "   "]) {
      const { core } = makeCore(doc);
      expect(core.command("setHeading", 1, 2), doc).toBeNull();
      expect(core.getText()).toBe(doc);
    }
  });

  it("S3a: blockquote CONTENT promotes after the quote markers", () => {
    const { core } = makeCore("> text");
    core.command("setHeading", 4, 1);
    expect(core.getText()).toBe("> # text");
  });

  it("S3b: level 0 deletes the closing hash run too: '# foo #' → 'foo'", () => {
    const { core } = makeCore("# foo #");
    const change = core.command("setHeading", 3, 0);
    expect(core.getText()).toBe("foo");
    expect(change!.splices).toEqual([
      { at: 0, delete: 2, insert: "" },
      { at: 5, delete: 2, insert: "" },
    ]);
    expect(change!.selection).toEqual({ anchor: 1, head: 1 });
  });

  it("S3b: '## x ##' → 'x'", () => {
    const { core } = makeCore("## x ##");
    core.command("setHeading", 4, 0);
    expect(core.getText()).toBe("x");
  });

  it("S3b: releveling is unchanged — only the opening delimiter is rewritten", () => {
    const { core } = makeCore("# foo #");
    core.command("setHeading", 3, 3);
    expect(core.getText()).toBe("### foo #");
  });
});

describe("core undo depth cap (S4)", () => {
  it("caps at 100 units, dropping the OLDEST: 101 units → 100 undos, first unit gone", () => {
    const { core } = makeCore("");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "A" }], "paste");
    for (let k = 0; k < 100; k++) {
      core.applyEdit(core.revision(), [{ at: core.docLength(), delete: 0, insert: "x" }], "paste");
    }
    let undos = 0;
    while (core.undo() !== null) undos++;
    expect(undos).toBe(100);
    expect(core.getText()).toBe("A"); // the oldest unit fell off the stack
  });

  it("exactly 100 units still undo all the way back", () => {
    const { core } = makeCore("");
    for (let k = 0; k < 100; k++) {
      core.applyEdit(core.revision(), [{ at: core.docLength(), delete: 0, insert: "x" }], "paste");
    }
    let undos = 0;
    while (core.undo() !== null) undos++;
    expect(undos).toBe(100);
    expect(core.getText()).toBe("");
  });
});

describe("core heading trailing whitespace + closing run (S5)", () => {
  it("'# foo   ': trailing spaces are not heading content", () => {
    const doc = "# foo   ";
    const { core } = makeCore(doc);
    const d = core.decorations(core.revision(), 0, doc.length, []);
    expect(lines(d)).toEqual([{ kind: "line", at: 0, style: "h1" }]);
    expect(conceals(d)).toEqual([{ kind: "conceal", from: 0, to: 2 }]);
    expect(marks(d)).toEqual([]);
  });

  it("inline content still parses inside the trimmed span: '# foo **b**   '", () => {
    const doc = "# foo **b**   ";
    const { core } = makeCore(doc);
    const d = core.decorations(core.revision(), 0, doc.length, []);
    expect(marks(d, "strong")).toEqual([{ kind: "mark", from: 8, to: 9, style: "strong" }]);
  });

  it("an ATX closing hash run conceals as a second delimiter span", () => {
    const doc = "# foo #\ntail";
    const { core } = makeCore(doc);
    const d = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(conceals(d)).toEqual([
      { kind: "conceal", from: 0, to: 2 },
      { kind: "conceal", from: 5, to: 7 },
    ]);
  });
});
