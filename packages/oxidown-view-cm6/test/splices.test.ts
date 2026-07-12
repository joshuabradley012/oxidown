import { describe, expect, it } from "vitest";
import { ChangeSet, Text } from "@codemirror/state";
import { applySplices, changesToSplices, endOfLastSplice } from "../src/splices";

/** mulberry32 — small deterministic PRNG for property-style tests. */
function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const ALPHABET = [
  "a", "b", "c", " ", "\n", "*", "_", "`", "#",
  "日", "本", "語", "😀", "🎉", "é",
];

function randomText(rnd: () => number, maxLen: number): string {
  const len = Math.floor(rnd() * maxLen);
  let s = "";
  for (let i = 0; i < len; i++) s += ALPHABET[Math.floor(rnd() * ALPHABET.length)];
  return s;
}

/** Random non-overlapping ascending change specs over a doc of length `docLen`. */
function randomChangeSpecs(rnd: () => number, docLen: number) {
  const count = 1 + Math.floor(rnd() * 4);
  // pick 2*count cut points, sort, pair them up into disjoint ranges
  const cuts = Array.from({ length: count * 2 }, () => Math.floor(rnd() * (docLen + 1))).sort(
    (x, y) => x - y,
  );
  const specs: { from: number; to: number; insert: string }[] = [];
  for (let i = 0; i < count; i++) {
    const from = cuts[2 * i];
    const to = cuts[2 * i + 1];
    specs.push({ from, to, insert: randomText(rnd, 8) });
  }
  return specs;
}

describe("changesToSplices", () => {
  it("simple insert / delete / replace", () => {
    const doc = "hello world";
    const cs = ChangeSet.of([{ from: 5, to: 5, insert: "," }], doc.length);
    expect(changesToSplices(cs)).toEqual([{ at: 5, delete: 0, insert: "," }]);

    const cs2 = ChangeSet.of([{ from: 0, to: 5, insert: "goodbye" }], doc.length);
    expect(changesToSplices(cs2)).toEqual([{ at: 0, delete: 5, insert: "goodbye" }]);
  });

  it("multi-range changes come out ascending in original coordinates", () => {
    const doc = "abcdefghij";
    const cs = ChangeSet.of(
      [
        { from: 8, to: 9, insert: "Y" },
        { from: 1, to: 3, insert: "X" },
      ],
      doc.length,
    );
    const splices = changesToSplices(cs);
    expect(splices).toEqual([
      { at: 1, delete: 2, insert: "X" },
      { at: 8, delete: 1, insert: "Y" },
    ]);
    expect(applySplices(doc, splices)).toBe(cs.apply(Text.of([doc])).toString());
  });

  it("property: splices applied to a plain JS string reproduce the new doc", () => {
    for (let seed = 1; seed <= 12; seed++) {
      const rnd = mulberry32(seed * 1337);
      const doc = randomText(rnd, 400);
      const specs = randomChangeSpecs(rnd, doc.length);
      const cs = ChangeSet.of(specs, doc.length);
      const viaCm = cs.apply(Text.of(doc.split("\n"))).toString();
      const viaSplices = applySplices(doc, changesToSplices(cs));
      expect(viaSplices).toBe(viaCm);
    }
  });

  it("property: random edit scripts stay in sync step by step", () => {
    for (let seed = 1; seed <= 6; seed++) {
      const rnd = mulberry32(seed * 7919);
      let cmDoc = Text.of(randomText(rnd, 200).split("\n"));
      let mirror = cmDoc.toString();
      for (let step = 0; step < 15; step++) {
        const cs = ChangeSet.of(randomChangeSpecs(rnd, cmDoc.length), cmDoc.length);
        mirror = applySplices(mirror, changesToSplices(cs));
        cmDoc = cs.apply(cmDoc);
        expect(mirror).toBe(cmDoc.toString());
      }
    }
  });
});

describe("endOfLastSplice", () => {
  it("returns null for an empty batch", () => {
    expect(endOfLastSplice([])).toBeNull();
  });

  it("accounts for length shift from earlier splices", () => {
    expect(
      endOfLastSplice([
        { at: 0, delete: 2, insert: "xyz" }, // shift +1
        { at: 5, delete: 1, insert: "" },
      ]),
    ).toBe(6);
    expect(endOfLastSplice([{ at: 3, delete: 0, insert: "ab" }])).toBe(5);
  });
});
