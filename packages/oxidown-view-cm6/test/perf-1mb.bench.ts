/**
 * M2 de-risk: JS-side wasm-boundary bench at 1MB/3MB (written for
 * research/09-1mb-derisk.md). NOT a test: this file matches vitest's
 * BENCHMARK include glob (`**\/*.{bench,benchmark}.*`), not its TEST include
 * glob (`**\/*.{test,spec}.*`) — `pnpm test` (`vitest run`) never collects
 * or executes it; it only runs via the separate `vitest bench` command:
 *
 *   pnpm --filter @oxidown/view-cm6 exec vitest bench test/perf-1mb.bench.ts
 *
 * Method: same vantage point as research/08-perf-baseline.md's "Browser-side
 * numbers" section — direct calls against a real wasm-loaded core via
 * `test/wasm-loader.ts` (timing includes wasm-bindgen call overhead + JSON
 * serialize/parse of the splices/decorations payloads; no DOM/CM6 work is
 * involved, so this is the boundary tax ALONE, layered on top of the native
 * Rust core costs measured in crates/oxidown-core/tests/perf_1mb_derisk.rs).
 * A ~100KB row is kept as a continuity anchor against research/08's existing
 * JS-side numbers (0.7/0.8ms applyEdit, 1.1ms→0.5ms combined p95); the
 * 1MB/3MB rows are this spike's actual target.
 *
 * This uses vitest's `bench()` purely as an ENTRY POINT that only executes
 * under `vitest bench` (the JS analogue of a Rust `#[ignore]`d test) — the
 * callback below does its own manual `performance.now()` timing loop and
 * prints mean/p50/p95/max, matching crates/oxidown-core/tests/
 * perf_1mb_derisk.rs's convention exactly, rather than leaning on tinybench's
 * own statistics engine (which would otherwise call the measured function an
 * UNBOUNDED number of times within its default ~500ms time budget, growing
 * the document by an uncontrolled amount mid-measurement — bad for a bench
 * whose whole point is "cost AT a pinned document size"). The `ran` guard
 * makes the real work execute exactly once regardless of how many times
 * vitest's runner invokes the callback.
 */
import { bench } from "vitest";
import { performance } from "node:perf_hooks";
import { loadWasmCoreFactory } from "./wasm-loader";
import type { SelectionRange } from "../src/protocol";

const makeWasmCore = await loadWasmCoreFactory();

/** Same mixed-markdown corpus shape as the Rust suite's `generate_mixed_doc`
 * (crates/oxidown-core/tests/perf_1mb_derisk.rs) — not byte-identical (this
 * is JS `string.length`, UTF-16 code units, not UTF-8 bytes), just the same
 * cyclic construct mix at a comparable size, so the two reports' scaling
 * stories are describing the same kind of document. */
function generateMixedDoc(targetLen: number): string {
  let doc =
    "# Oxidown perf corpus\n\nGenerated mixed-markdown corpus for M2 de-risk profiling.\n\n";
  let i = 0;
  while (doc.length < targetLen) {
    switch (i % 6) {
      case 0:
        doc +=
          `## Section ${i}\n\nLorem **ipsum ${i}** dolor *sit* amet, ` +
          "`consectetur` adipiscing elit. Some 你好 CJK and an emoji 😀 mixed " +
          "in with __strong__ text and _emphasis_ plus ***bold italic*** runs, " +
          `a [link](https://example.com/${i}) and an autolink ` +
          `<https://oxidown.dev/${i}> to exercise the parser.\n\n`;
        break;
      case 1:
        doc +=
          `Plain paragraph ${i} with no formatting at all, just words words ` +
          "words to pad out prose content between the richer constructs.\n\n";
        break;
      case 2:
        doc +=
          `> Quoted line one at section ${i}.\n> > A nested quote with ` +
          "**bold** and `code`.\n> Back to depth one.\n\n";
        break;
      case 3:
        doc +=
          `\`\`\`rust\nfn section_${i}() -> u32 {\n    // a comment\n` +
          `    let x = ${i};\n    x * 2\n}\n\`\`\`\n\n`;
        break;
      case 4:
        doc +=
          `- item one at ${i}\n- item two with **bold**\n  - nested item alpha\n` +
          "  - nested item beta\n- [ ] a task item\n- [x] a completed task\n" +
          "1. ordered one\n2. ordered two\n\n";
        break;
      default:
        doc +=
          `### Subsection ${i}\n\nAnother paragraph with ~~strikethrough~~ ` +
          "and a mix of *style* to round out the cycle.\n\n";
    }
    i++;
  }
  return doc;
}

interface Stats {
  mean: number;
  p50: number;
  p95: number;
  max: number;
}

function stats(samplesMs: number[]): Stats {
  const s = [...samplesMs].sort((a, b) => a - b);
  const n = s.length;
  const mean = s.reduce((a, b) => a + b, 0) / n;
  return { mean, p50: s[Math.floor(n / 2)], p95: s[Math.floor((n * 95) / 100)], max: s[n - 1] };
}

function fmt(s: Stats): string {
  const us = (ms: number) => (ms * 1000).toFixed(1).padStart(9);
  return `mean ${us(s.mean)}us  p50 ${us(s.p50)}us  p95 ${us(s.p95)}us  max ${us(s.max)}us`;
}

function measureSize(label: string, targetLen: number, iterations: number): void {
  const doc = generateMixedDoc(targetLen);

  // load(): the document-open experience — fresh core per sample (a small,
  // fixed sample count; this is a one-shot operation, not a per-keystroke
  // one, so it doesn't need hundreds of iterations).
  const loadSamples: number[] = [];
  const loadIters = Math.min(iterations, 20);
  for (let i = 0; i < loadIters; i++) {
    const core = makeWasmCore();
    const t0 = performance.now();
    core.load(doc);
    loadSamples.push(performance.now() - t0);
    core.destroy();
  }

  // applyEdit + decorations, mid-doc, on ONE warm core across all samples —
  // the per-keystroke shape (same convention as the Rust suite: the doc
  // grows by one char per sample, negligible drift at this scale: at most
  // a few hundred code units into a 100KB-3MB document).
  const core = makeWasmCore();
  let rev = core.load(doc);
  const applySamples: number[] = [];
  const decoSamples: number[] = [];
  const combinedSamples: number[] = [];
  const viewportCu = 3000;
  for (let i = 0; i < iterations; i++) {
    const len = core.docLength();
    const pos = Math.floor(len / 2);

    const t0 = performance.now();
    rev = core.applyEdit(rev, [{ at: pos, delete: 0, insert: "x" }], "user");
    const applyMs = performance.now() - t0;

    const vpFrom = Math.max(0, pos - viewportCu / 2);
    const vpTo = Math.min(core.docLength(), vpFrom + viewportCu);
    const sel: SelectionRange[] = [{ anchor: pos + 1, head: pos + 1 }];

    const t1 = performance.now();
    const decos = core.decorations(rev, vpFrom, vpTo, sel);
    const decoMs = performance.now() - t1;
    if (decos.length < 0) throw new Error("unreachable"); // keep `decos` live, avoid dead-code elim

    applySamples.push(applyMs);
    decoSamples.push(decoMs);
    combinedSamples.push(applyMs + decoMs);
  }
  core.destroy();

  console.log(`\n=== ${label} (~${Math.round(doc.length / 1024)}KB, JS side of the wasm boundary) ===`);
  console.log(`load():             ${fmt(stats(loadSamples))}`);
  console.log(`applyEdit mid-doc:  ${fmt(stats(applySamples))}`);
  console.log(`decorations 3k-CU:  ${fmt(stats(decoSamples))}`);
  console.log(`combined:           ${fmt(stats(combinedSamples))}`);
}

let ran = false;

bench(
  "m2 de-risk: wasm boundary at 100KB/1MB/3MB (non-statistical — see console output for the actual numbers)",
  () => {
    if (ran) return; // run the real measurement exactly once
    ran = true;
    console.log(
      "\n############ M2 de-risk: JS-side wasm boundary, 100KB/1MB/3MB ############",
    );
    measureSize("~100KB", 100 * 1024, 200);
    measureSize("~1MB", 1024 * 1024, 150);
    measureSize("~3MB", 3 * 1024 * 1024, 80);
  },
  { iterations: 1, time: 0, warmupIterations: 0, warmupTime: 0 },
);
