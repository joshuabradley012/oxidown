// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { LanguageDescription } from "@codemirror/language";
import { languages as languageRegistry } from "@codemirror/language-data";
import { classHighlighter, highlightTree } from "@lezer/highlight";
import { collectFenceRegions, FenceHighlighter } from "../src/highlight";
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
    const hl = new FenceHighlighter();
    // First pass: language not loaded yet -> no marks, load kicked off.
    let loaded = false;
    const first = hl.highlightRegions(state, regions, () => {
      loaded = true;
    });
    if (first.length === 0) {
      // wait for the async load to complete (bounded)
      for (let i = 0; i < 100 && !loaded; i++) await new Promise((r) => setTimeout(r, 20));
      expect(loaded).toBe(true);
    }
    const second = hl.highlightRegions(state, regions, () => {});
    expect(second.length).toBeGreaterThan(0);
    // Every mark must lie inside the region body.
    for (const r of second) {
      expect(r.from).toBeGreaterThanOrEqual(6);
      expect(r.to).toBeLessThanOrEqual(29);
    }
  });
});

// FIX 3 (efficiency): highlight.ts used to key its parse cache by the WHOLE
// fence body text (`${lang} ${text}`), guaranteeing a miss — and a full,
// non-incremental Lezer parse — on every keystroke inside a fence. The fix
// keeps the previous Tree per fence and reuses it via Lezer's own
// TreeFragment.applyChanges machinery, diffing old/new text with a cheap
// common-prefix/suffix computation. These tests verify (a) the incremental
// path produces byte-identical spans to a from-scratch parse of the same
// final text, and (b) it is actually faster for a keystroke in a large fence.
describe("incremental re-parse (FIX 3: reuse across keystrokes)", () => {
  it("wire-behavior: incremental spans after a mid-fence edit match a from-scratch parse", async () => {
    const desc = LanguageDescription.matchLanguageName(languageRegistry, "js", true);
    expect(desc).not.toBeNull();
    const support = await desc!.load();
    expect(support).toBeTruthy();

    const lines: string[] = [];
    for (let i = 0; i < 400; i++) lines.push(`  const v${i} = ${i} + total;`);
    const before = `function total() {\n  let total = 0;\n${lines.join("\n")}\n  return total;\n}\n`;
    const region0 = { lang: "js", from: 6, to: 6 + before.length };
    const stateBefore = EditorState.create({ doc: "```js\n" + before + "```\n" });
    // Warm this INSTANCE's tree cache at index 0 for "js" with the BEFORE
    // text (a cold, from-scratch parse — there's nothing to reuse yet). The
    // caches are per-FenceHighlighter, so both calls must share `hl`.
    const hl = new FenceHighlighter();
    hl.highlightRegions(stateBefore, [region0], () => {});

    // A single mid-fence edit deep in the body (well past any reasonable
    // common-prefix/suffix window), like one keystroke while typing.
    const marker = "v200 = 200";
    const idx = before.indexOf(marker);
    expect(idx).toBeGreaterThan(0);
    const after = before.slice(0, idx) + "vZZZ = 999" + before.slice(idx + marker.length);
    const region1 = { lang: "js", from: 6, to: 6 + after.length };
    const stateAfter = EditorState.create({ doc: "```js\n" + after + "```\n" });

    // Same index (0) as the warm cache above -> this call takes the
    // incremental path (TreeFragment.applyChanges against the cached tree).
    const incremental = hl.highlightRegions(stateAfter, [region1], () => {}).map((r) => ({
      from: r.from,
      to: r.to,
      cls: (r.value.spec as { class: string }).class,
    }));
    expect(incremental.length).toBeGreaterThan(0);

    // Independent reference: parse the SAME final text from scratch with the
    // same Lezer parser, entirely bypassing this module's caches.
    const refSpans: { from: number; to: number; cls: string }[] = [];
    const tree = support!.language.parser.parse(after);
    highlightTree(tree, classHighlighter, (from, to, cls) => {
      refSpans.push({ from: from + region1.from, to: to + region1.from, cls });
    });

    expect(incremental).toEqual(refSpans);
  });

  it("perf: reports before/after keystroke timing over a ~20KB fence", async () => {
    const desc = LanguageDescription.matchLanguageName(languageRegistry, "javascript", true);
    const support = await desc!.load();
    expect(support).toBeTruthy();

    let body = "function total() {\n  let total = 0;\n";
    let n = 0;
    while (body.length < 20_000) {
      body += `  const v${n} = ${n} + total;\n`;
      n++;
    }
    body += "  return total;\n}\n";

    // A run of "keystrokes": insert one character at a fixed offset each
    // time, growing the text — the same shape as continued typing at one
    // spot inside the fence.
    const KEYSTROKES = 40;
    const insertAt = Math.floor(body.length / 2);
    const texts: string[] = [body];
    for (let i = 0; i < KEYSTROKES; i++) {
      const t = texts[texts.length - 1];
      texts.push(t.slice(0, insertAt) + "x" + t.slice(insertAt));
    }

    // Warm up the JIT once, outside both timings, so neither is unfairly
    // penalized by first-call compilation.
    support!.language.parser.parse(texts[0]);

    // BEFORE (the bug): a full, non-incremental parse of the WHOLE fence
    // body on every keystroke — exactly what the old `${lang} ${text}`-keyed
    // cache guaranteed, since a keystroke always changes `text` (always a miss).
    const beforeStart = Date.now();
    for (const t of texts) support!.language.parser.parse(t);
    const beforeMs = Date.now() - beforeStart;

    // AFTER (the fix): one highlighter instance reusing the same cache slot
    // across the run, so each call after the first is an incremental re-parse.
    const hl = new FenceHighlighter();
    const afterStart = Date.now();
    for (const t of texts) {
      const state = EditorState.create({ doc: "```javascript\n" + t + "```\n" });
      const region = { lang: "javascript", from: 13, to: 13 + t.length };
      hl.highlightRegions(state, [region], () => {});
    }
    const afterMs = Date.now() - afterStart;

    // eslint-disable-next-line no-console
    console.log(
      `[oxidown bench] FIX 3: ${KEYSTROKES} keystrokes over a ~${Math.round(body.length / 1024)}KB fence — ` +
        `from-scratch (before) total=${beforeMs}ms (${(beforeMs / KEYSTROKES).toFixed(3)}ms/keystroke), ` +
        `incremental (after) total=${afterMs}ms (${(afterMs / KEYSTROKES).toFixed(3)}ms/keystroke)`,
    );

    expect(afterMs).toBeLessThan(beforeMs);
  });
});

describe("failed language load: retried on the next rebuild, not a permanent page-wide miss", () => {
  it("retries after a rejected load and paints once a later attempt succeeds", async () => {
    const doc = "```flaky\nconst x = 1; // note\n```\n";
    const state = stateOf(doc);
    const from = doc.indexOf("const");
    const to = doc.indexOf("\n```", from);
    const regions = [{ lang: "flaky", from, to }];

    // First attempt rejects (CDN outage); every later one resolves to the
    // real js support. "flaky" is unique to this test, so the module-global
    // load registry has no prior state for it.
    const realDesc = LanguageDescription.matchLanguageName(languageRegistry, "js", true);
    expect(realDesc).not.toBeNull();
    let attempts = 0;
    const flakyDesc = LanguageDescription.of({
      name: "flaky",
      load: () => {
        attempts++;
        return attempts === 1 ? Promise.reject(new Error("cdn down")) : realDesc!.load();
      },
    });
    const spy = vi.spyOn(LanguageDescription, "matchLanguageName").mockReturnValue(flakyDesc);
    try {
      const hl = new FenceHighlighter();
      // Rebuild 1 kicks off the load, which fails.
      expect(hl.highlightRegions(state, regions, () => {})).toEqual([]);
      for (let i = 0; i < 100 && attempts < 1; i++) await new Promise((r) => setTimeout(r, 10));
      await new Promise((r) => setTimeout(r, 10)); // let the rejection handler run
      expect(attempts).toBe(1);

      // Rebuild 2: the failure was NOT cached as a permanent miss — the
      // load is retried (the old behavior set supports=null here and never
      // called load again until page reload).
      let loaded = false;
      expect(
        hl.highlightRegions(state, regions, () => {
          loaded = true;
        }),
      ).toEqual([]);
      expect(attempts).toBe(2);
      for (let i = 0; i < 100 && !loaded; i++) await new Promise((r) => setTimeout(r, 20));
      expect(loaded).toBe(true);

      // Rebuild 3 paints with the successfully loaded language.
      expect(hl.highlightRegions(state, regions, () => {}).length).toBeGreaterThan(0);
    } finally {
      spy.mockRestore();
    }
  });

  it("an UNKNOWN language stays a remembered miss (no registry match, no retry churn)", () => {
    const doc = "```nosuchlang\nwords\n```\n";
    const state = stateOf(doc);
    const regions = [{ lang: "nosuchlang", from: 14, to: 19 }];
    const hl = new FenceHighlighter();
    expect(hl.highlightRegions(state, regions, () => {})).toEqual([]);
    expect(hl.highlightRegions(state, regions, () => {})).toEqual([]);
  });
});

describe("language-load multi-requester (two editors, one in-flight load)", () => {
  it("notifies EVERY requester's onLoad when the shared load resolves", async () => {
    // Two separate highlighter instances (two editors) request the same
    // language back-to-back: the first call initiates the async load, the
    // second arrives while it is still in flight. BOTH onLoad callbacks must
    // fire on resolution — the in-flight requester used to be silently
    // dropped, leaving that editor unhighlighted until an unrelated rebuild.
    // "python" is untouched by the js/javascript tests above, so it is
    // guaranteed to still be un-loaded (and in-flight) here.
    const doc = "```python\ndef f(x):\n    return x\n```\n";
    const state = stateOf(doc);
    const from = doc.indexOf("def");
    const to = doc.indexOf("\n```");
    const regions = [{ lang: "python", from, to }];

    const a = new FenceHighlighter();
    const b = new FenceHighlighter();
    let loadedA = false;
    let loadedB = false;
    const firstA = a.highlightRegions(state, regions, () => {
      loadedA = true;
    });
    const firstB = b.highlightRegions(state, regions, () => {
      loadedB = true;
    });
    // Neither can paint yet: the load is in flight for both.
    expect(firstA).toEqual([]);
    expect(firstB).toEqual([]);

    for (let i = 0; i < 100 && !(loadedA && loadedB); i++) {
      await new Promise((r) => setTimeout(r, 20));
    }
    expect(loadedA).toBe(true);
    expect(loadedB).toBe(true);

    // And on their rebuild passes, both editors actually get marks.
    expect(a.highlightRegions(state, regions, () => {}).length).toBeGreaterThan(0);
    expect(b.highlightRegions(state, regions, () => {}).length).toBeGreaterThan(0);
  });
});
