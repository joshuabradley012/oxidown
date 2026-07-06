// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { collectFenceRegions, highlightRegions } from "../src/highlight";
import type { Decoration } from "../src/protocol";

function stateOf(doc: string) {
  return EditorState.create({ doc });
}

describe("collectFenceRegions", () => {
  it("assembles open fence + body lines + close fence into one region", () => {
    const doc = "```ts\nconst x = 1;\nlet y = 2;\n```\nafter";
    const decos: Decoration[] = [
      { kind: "line", at: 0, style: "code-fence" },
      { kind: "line", at: 6, style: "code-block" },
      { kind: "line", at: 19, style: "code-block" },
      { kind: "line", at: 30, style: "code-fence" },
    ];
    const regions = collectFenceRegions(decos, stateOf(doc));
    expect(regions).toEqual([{ lang: "ts", from: 6, to: 29 }]);
  });

  it("skips fences without a language and handles unterminated fences", () => {
    const doc = "```\nplain\n```\n```js\nlet a;\n";
    const decos: Decoration[] = [
      { kind: "line", at: 0, style: "code-fence" },
      { kind: "line", at: 4, style: "code-block" },
      { kind: "line", at: 10, style: "code-fence" },
      { kind: "line", at: 14, style: "code-fence" }, // opens js, never closes
      { kind: "line", at: 20, style: "code-block" },
    ];
    const regions = collectFenceRegions(decos, stateOf(doc));
    expect(regions).toEqual([{ lang: "js", from: 20, to: 26 }]);
  });
});

describe("highlightRegions (async language load)", () => {
  it("loads javascript lazily and emits tok-* marks on the rebuild pass", async () => {
    const doc = "```js\nconst x = \"hi\"; // note\n```\n";
    const state = stateOf(doc);
    const regions = [{ lang: "js", from: 6, to: 29 }];
    // First pass: language not loaded yet -> no marks, load kicked off.
    let loaded = false;
    const first = highlightRegions(state, regions, () => {
      loaded = true;
    });
    if (first.length === 0) {
      // wait for the async load to complete (bounded)
      for (let i = 0; i < 100 && !loaded; i++) await new Promise((r) => setTimeout(r, 20));
      expect(loaded).toBe(true);
    }
    const second = highlightRegions(state, regions, () => {});
    expect(second.length).toBeGreaterThan(0);
    // Every mark must lie inside the region body.
    for (const r of second) {
      expect(r.from).toBeGreaterThanOrEqual(6);
      expect(r.to).toBeLessThanOrEqual(29);
    }
  });
});
