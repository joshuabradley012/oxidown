import { describe, expect, it } from "vitest";
import { MockCore, applySplices } from "../src/mock-core";
import type { Decoration, SelectionRange, Splice } from "../src/protocol";

function makeCore(text: string) {
  let t = 0;
  const core = new MockCore({ now: () => t });
  const clock = {
    advance(ms: number) {
      t += ms;
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

describe("MockCore decorations — M0 set", () => {
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

  it("***both*** parses as strong+em over the same content", () => {
    const doc = "***both*** end";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(14));
    expect(marks(ds, "strong")).toEqual([{ kind: "mark", from: 2, to: 8, style: "strong" }]);
    expect(marks(ds, "em")).toEqual([{ kind: "mark", from: 3, to: 7, style: "em" }]);
    expect(conceals(ds)).toEqual([
      { kind: "conceal", from: 0, to: 2 },
      { kind: "conceal", from: 2, to: 3 },
      { kind: "conceal", from: 7, to: 8 },
      { kind: "conceal", from: 8, to: 10 },
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

describe("MockCore reveal predicate", () => {
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

describe("MockCore text mirror and revisions", () => {
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

  it("throws on stale revision for applyEdit and decorations", () => {
    const { core } = makeCore("abc");
    const rev = core.revision();
    core.applyEdit(rev, [{ at: 0, delete: 0, insert: "x" }], "user");
    expect(() => core.applyEdit(rev, [{ at: 0, delete: 0, insert: "y" }], "user")).toThrow(
      /stale/,
    );
    expect(() => core.decorations(rev, 0, 1, cursor(0))).toThrow(/stale/);
    // current revision works
    expect(() => core.decorations(core.revision(), 0, 1, cursor(0))).not.toThrow();
  });

  it("throws on out-of-bounds and overlapping splices", () => {
    const { core } = makeCore("abc");
    expect(() =>
      core.applyEdit(core.revision(), [{ at: 2, delete: 5, insert: "" }], "user"),
    ).toThrow(/bounds/);
    expect(() =>
      core.applyEdit(
        core.revision(),
        [
          { at: 1, delete: 2, insert: "" },
          { at: 2, delete: 1, insert: "x" },
        ],
        "user",
      ),
    ).toThrow(/overlap|ascending/);
  });

  it("revisions increase monotonically, including across load()", () => {
    const core = new MockCore();
    const r1 = core.load("a");
    expect(r1).toBe(1); // revision 0's successor
    const r2 = core.applyEdit(r1, [{ at: 0, delete: 0, insert: "b" }], "user");
    expect(r2).toBe(r1 + 1);
    const r3 = core.load("fresh");
    expect(r3).toBeGreaterThan(r2);
  });
});

describe("MockCore undo/redo and coalescing", () => {
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

  it("non-adjacent edits do not coalesce even within the window", () => {
    const { core, clock } = makeCore("");
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "a" }], "user");
    clock.advance(10);
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "z" }], "user");
    expect(core.getText()).toBe("za");
    core.undo();
    expect(core.getText()).toBe("a");
    core.undo();
    expect(core.getText()).toBe("");
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

describe("MockCore composition stability rule", () => {
  it("conceal spans intersecting the composition range are emitted as delim marks", () => {
    const doc = "**bold** x";
    const { core } = makeCore(doc);
    // Selection parked away from the node; without composition it conceals.
    const before = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(conceals(before)).toHaveLength(2);

    core.compositionBegin(4, 4); // inside the strong node
    const during = core.decorations(core.revision(), 0, doc.length, cursor(10));
    expect(conceals(during)).toEqual([]);
    expect(marks(during, "delim")).toEqual([
      { kind: "mark", from: 0, to: 2, style: "delim" },
      { kind: "mark", from: 6, to: 8, style: "delim" },
    ]);

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
    core.compositionBegin(8, 8); // inside the strong node [4, 12)
    // a user edit earlier in the doc shifts everything right by 3
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "123" }], "user");
    const ds = core.decorations(core.revision(), 0, core.docLength(), cursor(0));
    // strong node now at [7, 15); composition range must have shifted with it
    expect(conceals(ds)).toEqual([]);
    expect(marks(ds, "delim")).toEqual([
      { kind: "mark", from: 7, to: 9, style: "delim" },
      { kind: "mark", from: 13, to: 15, style: "delim" },
    ]);
    core.compositionEnd();
  });
});
