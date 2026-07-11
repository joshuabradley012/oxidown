import { describe, expect, it } from "vitest";
import { MockCore, applySplices } from "../src/mock-core";
import type { Decoration, RangeCommandName, SelectionRange, Splice } from "../src/protocol";

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

// ---------------------------------------------------------------------------
// v0.2 (M1) additions
// ---------------------------------------------------------------------------

describe("MockCore decorations — M1 subset (v0.2)", () => {
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
    const doc = "> hello\nworld";
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
    const doc = "before\n---\nafter";
    const { core } = makeCore(doc);
    const ds = core.decorations(core.revision(), 0, doc.length, cursor(doc.length));
    expect(lines(ds)).toEqual([{ kind: "line", at: 7, style: "hr" }]);
    expect(conceals(ds)).toEqual([{ kind: "conceal", from: 7, to: 10 }]);
    // Cursor on the hr line reveals the raw dashes as a delim mark.
    const revealed = core.decorations(core.revision(), 0, doc.length, cursor(8));
    expect(marks(revealed, "delim")).toEqual([{ kind: "mark", from: 7, to: 10, style: "delim" }]);
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

  it("ordered markers display sequential numbers ignoring raw digits (mock parity)", () => {
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

  it("ordered list start number is honored (mock parity)", () => {
    // "4./5./9." displays 4,5,6.
    const { core } = makeCore("4. a\n5. b\n9. c\n");
    const ds = core.decorations(core.revision(), 0, core.docLength(), []);
    const widgets = ds.filter(
      (d): d is Extract<Decoration, { kind: "widget"; widget: "ordered" }> =>
        d.kind === "widget" && d.widget === "ordered",
    );
    expect(widgets.map((w) => w.number)).toEqual([4, 5, 6]);
  });

  it("a delimiter change starts a new, sequence-independent ordered list (mock parity)", () => {
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

  it("a nested ordered list restarts its own sequence (mock parity)", () => {
    const { core } = makeCore("1. a\n   1. nested\n   2. nested2\n2. b\n");
    const ds = core.decorations(core.revision(), 0, core.docLength(), []);
    const widgets = ds.filter(
      (d): d is Extract<Decoration, { kind: "widget"; widget: "ordered" }> =>
        d.kind === "widget" && d.widget === "ordered",
    );
    expect(widgets.map((w) => w.number)).toEqual([1, 1, 2, 2]);
  });

  it("FIX 2: a blank line does NOT reset ordered numbering (loose list, parity with the Rust core)", () => {
    // Per CommonMark a blank line doesn't close a list — that's exactly what
    // makes it "loose" — so "1. a\n1. b\n\n1. c" is ONE list and must display
    // 1,2,3, matching the wasm core (which counts straight through blank
    // lines). Before the fix, the mock treated the blank line like any other
    // non-item line and cleared the running sequence, producing [1,2,1].
    const { core } = makeCore("1. a\n1. b\n\n1. c\n");
    const ds = core.decorations(core.revision(), 0, core.docLength(), []);
    const widgets = ds.filter(
      (d): d is Extract<Decoration, { kind: "widget"; widget: "ordered" }> =>
        d.kind === "widget" && d.widget === "ordered",
    );
    expect(widgets.map((w) => w.number)).toEqual([1, 2, 3]);
  });

  it("FIX 2 control: a real paragraph line (non-blank, non-item) still resets ordered numbering", () => {
    // Unlike a blank line, actual paragraph CONTENT between list items is not
    // modeled as loose-list continuation by this mock — it still closes the
    // running sequence, so a fresh "1. c" starts its own list at 1.
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

    // Reveal extent = the LIST ITEM's marker extent [0, 5) — a cursor inside
    // it withholds the task widget.
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

describe("MockCore anchors (v0.2)", () => {
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

describe("MockCore command (v0.2)", () => {
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

  it("FIX 4: an unknown command name THROWS (parity with wasm's InvalidCommand), never null", () => {
    // Before the fix, an unrecognized name fell through to `default: return
    // null`, indistinguishable from a command that legitimately doesn't
    // apply here — every mock test would stay green for a future typo'd
    // command name even though the real wasm core throws `InvalidCommand`
    // per call (validated before dispatch). The mock must fail the same way.
    const { core } = makeCore("hello world");
    expect(() => core.command("bogusCommand" as unknown as RangeCommandName, 0, 1)).toThrow(
      /InvalidCommand/,
    );
    // And it must not have mutated anything on the way to throwing —
    // command() is transactional (docs/boundary-v0.md "Commands").
    expect(core.getText()).toBe("hello world");
  });
});

describe("MockCore indentList/outdentList (v0.2, marker-width-aware Tab nesting)", () => {
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
    expect(change!.selection).toBeNull();
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

  // --- paragraph-interruption guard (parity with the Rust core) -----------
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

  // --- below-context interruption guard (parity with the Rust core) -------
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

describe("MockCore enter (v0.3, construct-aware Enter)", () => {
  // Parity with crates/oxidown-core/src/commands.rs `enter` (module doc
  // comment "## enter"): continue on non-empty content, single-press exit
  // on empty constructs, null when neither applies.

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
    // Same shape as the Rust test: outdenting "   - [ ] " to top level puts
    // the untouched "3. c" against the new bullet list, where the guard's
    // landing-scan says a non-1 ordered marker cannot start a list.
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

describe("MockCore streaming (v0.2)", () => {
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
    expect(change.selection).toBeNull();
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

  it("a user edit interleaved between appends gets its own unit; each still undoes cleanly", () => {
    const { core, clock } = makeCore("head\n\ntail");
    const id = core.streamOpen(core.docLength());
    core.streamAppend(id, "A");
    clock.advance(10);
    core.applyEdit(core.revision(), [{ at: 0, delete: 0, insert: "USER" }], "user");
    core.streamAppend(id, "B");
    core.streamClose(id);
    expect(core.getText()).toBe("USERhead\n\ntailAB");

    // undo unwinds in strict temporal order: last stream chunk, then the
    // interleaved user edit, then the first stream chunk — the user's edit is
    // never corrupted by, or merged into, the stream's own unit(s).
    core.undo();
    expect(core.getText()).toBe("USERhead\n\ntailA");
    core.undo();
    expect(core.getText()).toBe("head\n\ntailA");
    core.undo();
    expect(core.getText()).toBe("head\n\ntail");
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
