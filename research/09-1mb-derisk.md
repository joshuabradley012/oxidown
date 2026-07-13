# M2 De-Risk: Where the 1MB/3MB Walls Actually Are

> Measured July 2026, on the same M4 Pro (macOS, arm64, rustc 1.96.1, Node v24.13.0) as
> `research/08-perf-baseline.md`, against `m2-web-editor-beta`. **Measurement only — no
> optimization changes were made.** This spike quantifies the M1 perf work's known O(doc)
> residues (research/08's "After" section: suffix span-rebase + block-ID rematch, the overlay
> `Vec` splice memmove, `decorations()`'s overlay scan, `reparse_incremental`'s window-parse
> constant) at M2 scale — 1MB and 3MB documents — ahead of building the 1MB virtual-viewport
> gate (`plan.md` §8: "virtual viewport on 1MB docs", keystroke-to-paint p95 < 16ms on 100KB).
> Every number below is native Rust release-mode (`cargo test --release`) unless labeled
> "JS side" (Node, via the real built wasm package, `vitest bench`). Decision-relevant numbers
> were run twice; both runs are reported as a range.

---

## 0. Headline finding, up front

**The Rust/wasm core is not the risk. The core+boundary combined cost at 1MB consumes at most
~2-3% of a 16ms per-keystroke frame budget, and at 3MB still only ~4-5%** (§11) — there is no
core-side reason the 1MB virtual-viewport gate can't be hit, and 3MB has real headroom too. The
one genuine "3MB falls over, 1MB is fine" finding is narrow and already documented: the
non-realigning-edit degrade case (`reparse_incremental` giving up and re-parsing the whole
remaining tail — editor.rs's own worst case, e.g. opening a code fence with no downstream
closer) costs proportional to the **swallowed region's size**, and at 3MB a full-document
swallow costs **16.0-17.2ms mean parse alone (§2)** — i.e. a single pathological edit can, by
itself, consume the *entire* 16ms frame budget before any CM6/DOM/paint work runs, whereas the
same worst case at 1MB tops out around 2-3ms (comfortable). Every other measured term —
`decorations()`, JSON serialization, anchor mapping at 1,000 live anchors, undo/redo, the
`command(indentList)` planner — is flat or cheap up to 3MB. The dominant *keystroke* cost that
does grow with document size is `BlockIndex::update`'s whole-document ID re-match, isolated
directly for the first time in this report (§4): it is **~58-62% of a mid-document
`apply_edit`'s total cost at every size tested**, more than the overlay-node suffix shift
research/08 named alongside it — the actionable, ranked-first lever if documents ever need to
grow meaningfully past 3MB (§12), though not required for the 1MB/3MB gate itself.

---

## 1. Method

- New file: `crates/oxidown-core/tests/perf_1mb_derisk.rs` — 8 `#[ignore]`d tests, same
  conventions as `perf_baseline.rs` (loose ceilings, `OXIDOWN_PERF_ITERS` override, doc
  generation inline, no shared test-util module):
  ```
  cargo test -p oxidown-core --release --test perf_1mb_derisk -- --ignored --nocapture
  ```
- Doc generation: a byte-for-byte copy of `perf_baseline.rs`'s `generate_mixed_doc` (same
  mixed-markdown construct cycle: ATX headings, mixed-inline paragraphs, nested blockquotes,
  fenced code, nested/ordered/task lists), so the 300KB row here is a same-shape cross-check
  against research/08's numbers and the 1MB/3MB rows extend the identical corpus shape two more
  size classes. Three sizes throughout: **300KB** (anchor, matches research/08's largest row),
  **1MB**, **3MB**.
- New file: `packages/oxidown-view-cm6/test/perf-1mb.bench.ts` — a vitest **benchmark** file
  (matches the `**/*.bench.*` glob, not the `**/*.test.*` glob `pnpm test`/`vitest run` collects
  — confirmed via `vitest list`, which shows zero tests from this file, and a full `pnpm -r
  test` run, which is unaffected). Runs only via:
  ```
  cd packages/oxidown-view-cm6 && npx vitest bench test/perf-1mb.bench.ts
  ```
  Uses `test/wasm-loader.ts` (unmodified, per the constraint) to load the real built
  `crates/oxidown-wasm/pkg` and measures the JS side of the boundary directly — same vantage
  point as research/08's "Browser-side numbers" section (wasm-bindgen call overhead + JSON
  serialize/parse included, no DOM/CM6 work). It uses vitest's `bench()` purely as an
  entry point that only fires under `vitest bench`; the callback runs its own manual
  `performance.now()` loop (guarded to execute exactly once) and prints mean/p50/p95/max,
  matching the Rust suite's convention, rather than leaning on tinybench's own statistics
  engine (which would otherwise call the measured closure an unbounded number of times within
  its default time budget, growing the document by an uncontrolled amount mid-measurement).
- **Temporary instrumentation, fully reverted.** To read out `reparse_incremental`'s actual
  convergence-window sizes (§8) — not observable from any existing public API or `ReparseCounts`,
  which only counts *which* strategy fired, not window size — two `println!`s gated behind
  `std::env::var_os("OXIDOWN_WINDOW_PROBE")` were added inside `reparse_incremental` in
  `crates/oxidown-core/src/editor.rs`, exercised via a throwaway test file
  (`crates/oxidown-core/tests/tmp_window_probe.rs`, never committed), then **both were removed**
  — the scratch test file deleted, `editor.rs` reverted with `git checkout --
  crates/oxidown-core/src/editor.rs`. Verified after the fact: `git diff --stat
  crates/oxidown-core/src` is empty; `git status --short crates/oxidown-core/src` is clean.
- No other `src/` file was touched, in either crate or `packages/oxidown-view-cm6/src`. No file
  under `apps/` was touched. No existing test file was modified.

---

## 2. Numbers: `parser::parse_document` full-parse scaling + overlay memory

| size | bytes | nodes | blocks | node-Vec mem est. | mean | p50 | p95 | max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| ~300KB | 307,346 | 13,317 | 3,155 | 1,976KB | 1068.5-1071.5µs | 1058.3-1067.8µs | 1094.3-1235.0µs | 1098.3-1340.2µs |
| ~1MB | 1,048,702 | 45,262 | 10,720 | 6,718KB | 4707.3-5312.8µs | 4649.7-5111.4µs | 4918.6-6428.3µs | 6081.4-6943.7µs |
| ~3MB | 3,145,906 | 134,803 | 31,928 | 20,009KB | 15989.2-17193.6µs | 16877.6-17690.2µs | 19562.2-19901.9µs | 19627.0-24132.9µs |

(ranges are two independent runs, 30 iterations each after warm-up; `node-Vec mem est.` =
`node_count × size_of::<parser::Node>()`, measured `152` bytes/node — a lower bound, since each
node's `delims: Vec<Range<usize>>` carries its own small heap allocation not counted here.)

**Mild, real superlinearity, not a cliff.** Per-KB cost: ~3.56-3.57µs/KB at 300KB, ~4.60-5.19µs/KB
at 1MB, ~5.20-5.60µs/KB at 3MB — roughly a 40-55% per-KB cost increase from 300KB to 3MB (vs.
research/08's essentially-flat ~3.9-4.1µs/KB across 3KB-300KB). Node/block counts scale
cleanly linearly with size (blocks/KB stays ~10.4-10.5 throughout: 10.51, 10.47, 10.39 at
300KB/1MB/3MB respectively), so this isn't a parser complexity blowup — most likely
allocator/cache effects as the working set (a 20MB `Vec<Node>` at 3MB) stops fitting comfortably
in cache. **This is the "1MB fine, 3MB somewhat worse per byte" pattern for raw parse cost** —
real, worth watching if documents grow past 3MB, but nowhere near a wall at the sizes tested
(3MB's own p95, 19.6-19.9ms, only matters for the *load-time* full parse or the documented
non-realigning-edit degrade case, §8 — never for an ordinary keystroke, §3-4).

**Memory**: overlay-node memory alone is ~6.5x the raw document size at every scale tested
(1,976KB/307KB≈6.4x @300KB; 6,718KB/1,024KB≈6.6x @1MB; 20,009KB/3,073KB≈6.5x @3MB) — consistent
and linear, not alarming alone, but a real line item: a 3MB document carries ~20MB of overlay
nodes, on top of the rope, the block index, the op log/undo stack (100-unit cap), and whatever
the JS/CM6 side holds.

---

## 3. Numbers: `apply_edit` (1-char insert) by position, 300KB/1MB/3MB

| size | position | mean | p50 | p95 | max |
|---|---|--:|--:|--:|--:|
| ~300KB | start | 46.9-47.0µs | 46.7-46.9µs | 47.7-51.2µs | 53.2-54.4µs |
| ~300KB | middle | 38.2-38.6µs | 37.2-37.4µs | 43.9-44.8µs | 47.1-57.2µs |
| ~300KB | end | 8.4-8.7µs | 8.3-8.4µs | 9.1-9.5µs | 13.4-13.7µs |
| ~1MB | start | 156.0-156.7µs | 153.1-154.1µs | 169.0-170.2µs | 174.2-196.9µs |
| ~1MB | middle | 118.6-119.9µs | 116.8-117.5µs | 128.5-129.4µs | 144.6-156.2µs |
| ~1MB | end | 25.4-25.6µs | 25.1-25.2µs | 25.7-26.4µs | 38.0-43.1µs |
| ~3MB | start | 482.8-483.5µs | 480.2-481.3µs | 517.0-530.9µs | 548.8-581.6µs |
| ~3MB | middle | 361.4-368.6µs | 359.8-368.6µs | 384.3-389.9µs | 389.0-403.9µs |
| ~3MB | end | 96.9-100.3µs | 94.0-96.6µs | 118.4-126.9µs | 217.5-221.9µs |

(ranges are two independent runs, 100 iterations each.)

**START is the worst position, not middle — a refinement of research/08's framing.** Once the
tail fast path and incremental reparse both existed (research/08's "After" section), position no
longer selects between "full reparse" and "cheap path" — but among the cheap paths, cost is
proportional to **how much of the document sits downstream of the edit** (the overlay-suffix
shift + block-index rematch both touch everything after the edit point, and — for block IDs —
arguably everything, see §4). A start-of-document edit has to shift/rematch nearly the *whole*
document; a middle edit, about half; an end-of-document edit (which takes the tail path,
`reparse_tail`, not `reparse_incremental` — see editor.rs) touches almost nothing. Ratio at 3MB:
start/end ≈ 4.8-5.0x.

**Isolating the doc-size-dependent bookkeeping term** (middle mean − end mean; the task's
"doc-end edit where the suffix is empty" isolation, generalized here across sizes — see the
caveat below):

| size | middle − end | ÷ half-doc-size (KB) | implied µs/KB |
|---|--:|--:|--:|
| ~300KB | 29.8-29.9µs | ~150KB | ~0.199µs/KB |
| ~1MB | 93.2-94.4µs | ~512KB | ~0.182-0.184µs/KB |
| ~3MB | 264.5-268.4µs | ~1536KB | ~0.172-0.175µs/KB |

The implied per-KB constant is remarkably stable (~0.17-0.20µs/KB, if anything *slightly*
shrinking at larger sizes — no evidence of superlinearity here, unlike raw parse cost above).
This is roughly double research/08's casual "~0.1µs/KB" estimate (their number wasn't
delta-isolated the same way) but still tiny in absolute terms: even at 3MB this term is under
0.3ms. **Caveat**: "end" is not a true zero baseline — it takes `reparse_tail`, which has its
own documented O(overlay) cost (`overlay.retain(...)`, editor.rs's own COST NOTE on
`reparse_tail`), not `reparse_incremental`'s suffix-splice specifically. Both are small,
linear-in-doc-size bookkeeping passes with different constants (see §4 for the *directly*
isolated, unambiguous decomposition of the incremental path's own two terms).

---

## 4. Numbers: `BlockIndex::update` in isolation — the dominant bookkeeping term

Times `BlockIndex::update` **alone** (no parsing, no `Editor` at all): seed a fresh `BlockIndex`
with the real block spans from a full parse at each size (untimed), then time exactly one
`.update()` call for a synthetic "typed one character into an existing paragraph" batch — every
block at/after the edit point shifts by +1 byte, matching what `map_range_shrink` produces for
that scenario; this is the realistic common case (no block boundary changes).

| size | block count | mean | p50 | p95 | max | implied ns/block |
|---|--:|--:|--:|--:|--:|--:|
| ~300KB | 3,155 | 22.0-22.5µs | 21.8-22.2µs | 22.3-24.7µs | 26.3-28.1µs | 6.97-7.13 |
| ~1MB | 10,720 | 71.4-73.1µs | 69.0-74.2µs | 76.1-76.8µs | 84.2-89.9µs | 6.66-6.82 |
| ~3MB | 31,928 | 217.2-221.5µs | 215.5-220.5µs | 235.7-236.9µs | 313.2-344.1µs | 6.80-6.94 |

**Cleanly linear, ~6.7-7.1ns/block, remarkably constant across a 10x range of block counts** —
`match_spans`'s two-pointer overlap merge is exactly the O(old + new blocks) its own doc comment
promises, with no hidden superlinear term. Since block density is itself stable (~10.4-10.5
blocks/KB, §2), this converts to **~70-75ns/KB of document**, i.e. `BlockIndex::update`'s own
per-KB constant is close to research/08's "~0.1µs/KB" guess for the *combined* bookkeeping pass
— meaning `BlockIndex::update` alone accounts for most of that guess.

**Comparing this isolated number against §3's mid-document `apply_edit` totals is the key new
finding of this report:**

| size | `BlockIndex::update` alone | `apply_edit` middle (total) | share |
|---|--:|--:|--:|
| ~300KB | 22.0-22.5µs | 38.2-38.6µs | ~57-59% |
| ~1MB | 71.4-73.1µs | 118.6-119.9µs | ~60-62% |
| ~3MB | 217.2-221.5µs | 361.4-368.6µs | ~59-61% |

**Block-ID re-matching is consistently 57-62% of a mid-document keystroke's total cost, at
every size tested** — a bigger share than the overlay-node suffix shift research/08 named
alongside it (the remaining ~38-43%, plus the window-parse itself, which is negligible — see
§8). This isn't visible from `apply_edit` timing alone; it required isolating `BlockIndex::update`
directly, which `editor.rs`'s own step-3b doc comment doesn't distinguish from the overlay splice
("assemble the full new span list... let the ordinary `update` re-match IDs... O(#blocks) with a
small constant" is accurate, but reads as a minor addendum to step 3a's overlay-shift discussion,
when in measured fact it's the *larger* of the two terms).

**FIXED (2026-07-03)** — see §12 item 2 for the windowed `BlockIndex::update_range` fix and
before/after numbers; this section's numbers are left as originally measured (the "before" half
of that comparison).

---

## 5. Numbers: `decorations()`, ~3k-CU middle viewport — confirmed flat, no regression

| size | mean | p50 | p95 | max |
|---|--:|--:|--:|--:|
| ~300KB | 32.2-33.2µs | 31.4-32.2µs | 37.5-39.2µs | 58.8-59.4µs |
| ~1MB | 34.1-35.8µs | 33.3-34.2µs | 38.2-39.8µs | 54.9-57.8µs |
| ~3MB | 35.6-36.6µs | 34.6-35.0µs | 38.6-41.6µs | 58.1-60.1µs |

**Flat across a 10x size range (300KB→3MB): ~32-37µs mean regardless of document size.** This is
a *positive* update to research/08 §6, which characterized `decorations()` as scanning the
*entire* cached overlay per call (a real, if small, O(total node count) liability, ~59% growth
observed there from 3KB→300KB) and ranked fixing it last (§10 item 5, "flagged as a scaling risk
to watch, not an active fire"). Reading `decorations.rs` today shows that scan replaced with a
`block_floor` windowing function (`blocks.partition_point(...)`, O(log blocks)) — its own doc
comment cites a since-fixed regression ("the previous backward byte scan... degraded to O(doc)...
measured ~2.4ms/call on a 2MB blank-line-free blockquote"), landed in the M1 PR-review pass
(`git log` traces it to `3207028`/`8fd1c1e`/`e363b94`, after research/08's baseline write-up).
**`decorations()` is confirmed viewport-bound, not document-bound, at 1MB and 3MB alike — no
action needed here for the M2 gate.**

---

## 6. Numbers: `command(IndentList)` and undo/redo — track `apply_edit` exactly

Both go through the same `apply_plan` → `reparse_incremental` path as a mid-document
`apply_edit`, so their numbers track it closely (as expected — no surprises):

| size | `command(IndentList)` mid-doc | undo (mid-doc edit) | redo |
|---|--:|--:|--:|
| ~300KB | 37.8-39.7µs | 37.6µs | 37.6-37.7µs |
| ~1MB | 121.1-122.4µs | 117.4-118.5µs | 119.7-121.4µs |
| ~3MB | 354.9-359.5µs | 361.4-384.0µs | 361.2-387.5µs |

(mean, two runs each, 80 iterations.) All three sit within a few percent of §3's mid-document
`apply_edit` numbers at the same size — confirming the shared bottleneck (§4's `BlockIndex`
rematch) dominates regardless of which entry point drives the edit.

---

## 7. Numbers: anchor-mapping cost, 0 vs. 1,000 live anchors

`apply_edit`, mid-document, with 0 vs. 1,000 anchors scattered across the document
(`AnchorSet::map_through`'s documented O(anchors × batch) cost — a single-splice batch here, so
O(anchors)):

| size | 0 anchors | 1,000 anchors | delta |
|---|--:|--:|--:|
| ~300KB | 44.4µs | 42.9µs | within noise (−1.5µs) |
| ~1MB | 115.8µs | 118.2µs | within noise (+2.4µs) |
| ~3MB | 358.1µs | 357.9µs | within noise (−0.2µs) |

(second run's numbers shown; the first run's 300KB/1MB rows carried a ~2-3x cold-start
artifact — being the first test executed in that pass — resolved on the second, warmed-up run,
which matches §3's independently-measured middle-position baseline almost exactly at every
size: a useful cross-check that the two test functions are measuring the same thing.) **1,000
live anchors add no measurable cost at any size tested, up to 3MB** — `AnchorSet`'s own module
doc calling this "small... either way" holds comfortably at this scale.

---

## 8. Numbers: `reparse_incremental` convergence-window sizes (temporary probe, §1)

Real window sizes (bytes re-parsed, `region_start..P`) for 20 ordinary single-character inserts
scattered evenly across a 1MB document (no block-boundary change):

```
108, 108, 108, 160, 185, 188, 188, 216, 216, 230,
230, 230, 230, 298, 298, 298, 298, 298, 405, 409   (sorted; bytes)
```

Mean ≈ **244 bytes**, max 409 bytes — **0.024%-0.04% of the 1MB document.** This directly
confirms the architectural claim in `docs/boundary-v0.md`'s amended performance-budget note:
parse work really is bounded to a small window around the edit; every doc-size-scaling cost
measured in this report (§3, §4) is bookkeeping (block rematch, node-offset shift), not parsing.

**A mid-line fence-open that happens to be closed by the next naturally-occurring fence**
(this corpus cycles a fenced-code block roughly every ~1,800 bytes) converged at **439 bytes** —
still small, because the corpus's short average block length means a real realigning boundary
reappears quickly even after a locally disruptive edit.

**The genuine worst case — engineered directly**: a copy of the 1MB doc with every fence
delimiter in the back half byte-length-preservingly neutered (`"```" → "xxx"`), then a fence
opened at the exact midpoint (no closer exists anywhere before EOF). This produced the
documented degrade:
```
WINDOW_PROBE degrade region_start=524198 len_post=1048706 window_bytes=524508
```
**A window of 524,508 bytes — essentially the entire remaining half of the document** — exactly
`reparse_incremental`'s own documented fallback ("a fence opened by the edit swallows the rest of
the slice... degrade to `reparse_tail`"). This is the mechanism behind §11's one real risk
flag: the swallowed region gets a **full `parser::parse_document` pass** (§2's numbers apply
directly — at 3MB, a full-document swallow costs 16.0-17.2ms mean by itself).

---

## 9. Numbers: wasm-boundary JSON serialization at 1MB (native replica)

Same direct-writer replica as `perf_baseline.rs` §8/(f) (`oxidown-wasm`'s current
`decorations_json_string`), timed natively on the same ~3k-CU/1MB-doc viewport used in §5:

- **291 decorations** in the viewport → **15,277-byte** JSON payload.
- Serialization alone: **mean 8.0-8.2µs, p95 8.6-11.0µs, max 17.6-21.1µs.**

Matches research/08's "After" 100KB number almost exactly (291 vs. 281 decorations, 15,277 vs.
14,051 bytes, 8.0-8.2µs vs. 8.5µs mean) — **confirmed flat/size-independent**, as designed
(bound by viewport decoration count, not document size). No action needed.

---

## 10. Numbers: the JS side of the wasm boundary at 1MB/3MB (Node, real wasm pkg)

Direct calls against the real wasm-loaded core (`test/wasm-loader.ts`, unmodified), same vantage
point as research/08's "Browser-side numbers" (wasm-bindgen call overhead + JSON serialize/parse
included; no DOM/CM6). Two independent runs; a ~100KB row is kept as a continuity check against
research/08's own JS-side numbers (0.7/0.8ms applyEdit med/p95, 0.2/0.3ms decorations, 0.5ms
combined p95 — measured in a live Chrome tab, so a somewhat higher bar than this Node-native
harness, noted below).

| size | metric | mean | p50 | p95 | max |
|---|---|--:|--:|--:|--:|
| ~100KB | `load(text)` | 1880.0-1885.8µs | 1747.8-1781.5µs | 7016.5-7878.4µs | 7016.5-7878.4µs |
| ~100KB | `applyEdit` mid-doc | 32.5-33.2µs | 21.5-23.0µs | 44.3-46.0µs | 1444.3-1622.6µs |
| ~100KB | `decorations` 3k-CU | 170.1-170.4µs | 152.2-155.2µs | 229.5-252.0µs | 889.9-1016.7µs |
| ~100KB | combined | 202.7-203.6µs | 174.8-179.2µs | 271.2-300.9µs | 2359.0-2639.3µs |
| ~1MB | `load(text)` | 9367.8-9624.5µs | 9192.2-9545.7µs | 11087.2-11708.4µs | 11087.2-11708.4µs |
| ~1MB | `applyEdit` mid-doc | 158.6-161.7µs | 152.7-154.9µs | 171.5-205.5µs | 333.4-334.3µs |
| ~1MB | `decorations` 3k-CU | 152.5-157.3µs | 143.4-148.0µs | 171.9-209.8µs | 809.3-889.9µs |
| ~1MB | combined | 311.1-319.0µs | 301.4-302.8µs | 333.5-431.3µs | 984.3-1041.2µs |
| ~3MB | `load(text)` | 27567.6-28659.3µs | 27333.1-28202.5µs | 31671.8-33760.5µs | 31671.8-33760.5µs |
| ~3MB | `applyEdit` mid-doc | 446.0-466.9µs | 444.4-453.2µs | 486.6-610.0µs | 563.5-654.4µs |
| ~3MB | `decorations` 3k-CU | 170.2-181.3µs | 154.9-161.1µs | 193.5-216.0µs | 1257.9-1278.8µs |
| ~3MB | combined | 616.2-648.2µs | 598.9-614.2µs | 712.4-822.8µs | 1691.7-1725.3µs |

(Occasional large `max` outliers — e.g. 100KB `applyEdit` max 1.4-1.6ms against a 33-44µs
mean — are consistent with JIT warm-up/GC pauses in a tight Node loop, not a real per-call
cost; p95/mean are the load-bearing numbers.) The 100KB row is directionally consistent with
research/08's own JS-side numbers (`applyEdit` mean 32.5-33.2µs here vs. their 0.04ms/40µs
median in a live browser tab; combined mean ~203µs here vs. their 0.5ms p95 in-browser) — this
harness runs in raw Node (no DOM, no CM6, no browser JIT/GC profile), so it's a reasonable but
**optimistic** lower bound on real browser-tab numbers, the same caveat research/08 attached to
its own native-Rust proxy relative to the JS/browser vantage point.

**The boundary tax, isolated (JS combined − native combined, §3+§5):**

| size | native combined (apply-mid + decorations, mean) | JS combined (mean) | boundary tax | tax as % of JS total |
|---|--:|--:|--:|--:|
| ~100KB (research/08 "After") | ~48.5µs | ~203.15µs | ~154.7µs | ~76% |
| ~1MB | ~154.3µs (119.3+35.0) | ~315.1µs | ~160.8µs | ~51% |
| ~3MB | ~401.1µs (365.0+36.1) | ~632.2µs | ~231.1µs | ~37% |

**The boundary tax is roughly a fixed per-call cost (~155-230µs), so its *relative* share
shrinks as the document grows** (76%→51%→37%) even as core cost grows — the inverse of research/
08's framing at 100KB (where the tax was already the majority of total cost) but the *same*
underlying mechanism (wasm-bindgen call/marshaling overhead + JSON round-trip, research/08 §10
item 3). Absolute tax does grow somewhat from 1MB→3MB (+44%) — plausibly the JSON round-trip's
own scaling with something call-shape-related; not investigated further here (out of scope for
a measurement-only spike), flagged for whoever next touches the boundary serialization path.

**`load()` cost — the document-open experience, not a keystroke cost:** ~9.4-9.6ms at 1MB,
~27.6-28.7ms at 3MB. Subtracting native `parse_document` (§2, ~4.7-5.3ms @1MB, ~16.0-17.2ms
@3MB) leaves a **string-marshaling boundary tax of ~4.3-4.9ms @1MB (~45-47% of load time) and
~11.5-12.7ms @3MB (~40-46%)** — converting a UTF-16 JS string of that size into the Rust core's
UTF-8 rope is itself O(size), unlike the roughly-fixed per-keystroke marshaling tax above. This
is a one-time cost (once per document open, not per keystroke) but a real, synchronous
main-thread stall: **opening a 3MB document costs close to 2 frames (28ms) of blocking time
before virtual-viewport rendering even starts** — worth flagging to whoever builds the
virtual-viewport UI, since no amount of viewport virtualization touches this step.

---

## 11. Verdict against the M2 gate

**Question:** is the 1MB virtual-viewport gate (and the keystroke-to-paint p95 < 16ms budget) in
reach given the current core architecture? **Yes — with one narrow, already-documented
exception at 3MB, not 1MB.**

Budget remaining for CM6/DOM/virtual-viewport/paint work, per keystroke, using the JS-side
combined `applyEdit`+`decorations` numbers (§10) as the authoritative "core+boundary" share
(`docs/boundary-v0.md`'s own vantage point) against a 16ms frame:

| size | core+boundary combined (mean / p95) | % of 16ms budget consumed (p95) | remaining for CM6/paint |
|---|--:|--:|--:|
| ~100KB | 202.7-203.6 / 271.2-300.9µs | ~1.7-1.9% | ~15.70-15.73ms |
| ~1MB | 311.1-319.0 / 333.5-431.3µs | ~2.1-2.7% | ~15.57-15.67ms |
| ~3MB | 616.2-648.2 / 712.4-822.8µs | ~4.5-5.1% | ~15.18-15.29ms |

**Ordinary keystrokes leave 95-98% of the 16ms budget untouched by the core+boundary, at every
size tested up to 3MB.** Even the observed `max` outliers (JIT/GC noise, up to ~2.6ms at 100KB
in this harness) leave well over half the budget. This is a comfortable, not a marginal, verdict
— there is no core-side reason to delay virtual-viewport UI work pending further core
optimization.

**The one real exception**, from §8: the documented non-realigning-edit degrade case. At 1MB, a
full-tail swallow costs on the order of parsing ~500KB (interpolating §2: roughly 2.3-2.6ms) —
still comfortably inside budget even added to everything else. **At 3MB, a full- (or
near-full-) document swallow costs the *entire* parse-scaling number in §2 — 16.0-17.2ms mean,
19.6-19.9ms p95 — which alone meets or exceeds the whole 16ms frame budget**, before any
CM6/DOM/paint cost is added. This requires a specific adversarial edit shape (opening an
unterminated fence/list/etc. with **no realigning boundary anywhere in the rest of the
document** — not "any fence open," which this report's own probe showed usually reconverges
within a few hundred bytes because ordinary documents have recurring structure, §8) — uncommon
in interactive typing, more plausible via paste or a large AI-streamed insert landing mid-fence.
**This is the one place where "1MB is fine, 3MB falls over"** in an absolute, gate-relevant
sense; everything else in this report is a percentage-point margin story, not a wall.

---

## 12. Ranked list: fix before virtual-viewport UI vs. already fine

1. **[Real, narrow, not urgent for THIS gate] The non-realigning-edit full-tail degrade, at
   3MB.** §8/§11: a rare but real edit shape can single-handedly consume the entire 16ms budget
   at 3MB (comfortable at 1MB). Two directions, neither implemented here (out of scope —
   measurement only): (a) move such a reparse off the synchronous input-handling path
   (background/idle-time, showing stale decorations briefly rather than blocking), or (b) accept
   the documented tradeoff as-is for M2 (it needs a specific adversarial shape, and the existing
   `reparse_equivalence` correctness gate already exercises it without a perf assertion). Flagged
   for awareness before virtual-viewport UI ships; does not block starting that work.
2. **[The actionable lever, if targeting docs meaningfully past 3MB] `BlockIndex::update`'s
   whole-document ID rematch.** §4: consistently ~58-62% of a mid-document keystroke's total
   cost at every size tested (22-222µs across 300KB-3MB) — the single largest quantified,
   cleanly-isolated term in this report. Currently harmless in absolute terms (headroom is
   30-45x even at 3MB, §11), but the first thing to optimize (e.g., re-match only blocks inside
   the reparse window, splicing the untouched before/after block sublists through unmatched) if
   documents grow past the range this spike covers. **Not required for the 1MB/3MB M2 gate.**

   > **FIXED — after numbers (2026-07-03).** Implemented exactly the lever named above:
   > `BlockIndex::update_range` (`block_index.rs`), a windowed counterpart to `update` wired into
   > `reparse_incremental`'s step 3b in place of the old "assemble before ++ fresh ++
   > shifted-after, call the whole-document `update`" construction. Blocks before the window keep
   > their `Block` entries and IDs untouched (no allocation, no rematch call); blocks in the
   > window are rematched with the exact same `match_spans` heuristic, restricted to that
   > sub-slice; blocks after the window shift their spans in place by the batch's net delta, IDs
   > retained. Proven (not just tested) equivalent to the old whole-list `update` call: the
   > "one block of slack" window-start and "old block end past the dirty region" convergence-point
   > invariants `reparse_incremental` already establishes for the overlay splice mean no candidate
   > match edge can ever cross a window boundary, so splitting the single `match_spans` call into
   > independent prefix/window/suffix pieces changes nothing about the result — see
   > `BlockIndex::update_range`'s doc comment for the full argument. `update` itself is untouched
   > and still used for `load`/full reparse.
   >
   > Isolated bench (`perf_1mb_derisk.rs`, `block_index_update_range_scaling_1mb_3mb`, realistic
   > 2-block window, two runs, release mode):
   >
   > | size | `update` (unchanged, mean / p95) | `update_range` (new, mean / p95) | speedup (mean) |
   > |---|--:|--:|--:|
   > | ~300KB | 22.8-23.1 / 23.4-25.4µs | 0.5 / 0.5-0.6µs | ~46x |
   > | ~1MB | 74.1-80.7 / 79.8-87.1µs | 2.1-2.2 / 2.2µs | ~36x |
   > | ~3MB | 214.6-221.0 / 230.0-241.2µs | 6.2 / 6.2-6.3µs | ~35x |
   >
   > End-to-end `apply_edit` (mid-document, `apply_edit_position_scaling_1mb_3mb`, two runs):
   >
   > | size | BEFORE (§3, mean) | AFTER (mean) | speedup | new `update_range` share of total |
   > |---|--:|--:|--:|--:|
   > | ~300KB | 38.2-38.6µs | 14.1-14.2µs | ~2.7x | ~3.5% |
   > | ~1MB | 118.6-119.9µs | 40.4-41.7µs | ~2.9x | ~5.2-5.4% |
   > | ~3MB | 361.4-368.6µs | 118.4-119.0µs | ~3.1x | ~5.2% |
   >
   > The dominant ~58-62% term is gone, not just shrunk — `apply_edit` at the worst (start) position
   > drops similarly (~2x: 3MB start mean 482.8-483.5µs → 231.3-236.9µs). What remains is almost
   > entirely item #3 below (the overlay suffix shift), unchanged by this fix. A tight perf
   > assertion (50µs p95, vs. `update`'s own 100-220µs measured cost at the same sizes) was added
   > alongside the isolated bench specifically so a regression back to full-document rematch trips
   > locally. Full workspace tests (debug + release), clippy `--all-targets -D warnings`, and all
   > four `--ignored` perf suites stay green; the fuzzed `reparse_equivalence` gate was extended
   > with an id-stability property test (`untouched_sentinel_blocks_keep_their_ids_under_incremental_fuzz`)
   > since block IDs aren't comparable against a from-scratch parse. Diff confined to
   > `crates/oxidown-core/{src/block_index.rs,src/editor.rs,tests/perf_1mb_derisk.rs,tests/reparse_equivalence.rs}`;
   > the wasm boundary is untouched (`BlockIndex` was already internal, not exposed over it).
3. **[Same status as #2, smaller share] The overlay `Vec`'s suffix offset-shift.** ~38-43% of a
   mid-document keystroke (the remainder of §3's `apply_edit` cost after subtracting §4's
   isolated `BlockIndex::update` number), ~2-2.4ns/node — editor.rs's own doc comment already
   names this tradeoff and its own revisit condition ("only if docs that size become a target");
   this report's data says 1MB/3MB do not yet meet that condition.
4. **[Worth noting, not blocking, not this gate's target] `load()`/document-open cost.** §10:
   ~9.5ms at 1MB, ~28ms at 3MB (JS side), ~45-47% of which is the wasm string-marshaling
   boundary tax, not the Rust parse. This is a one-time, not per-keystroke, cost — orthogonal to
   virtual-viewport rendering (which doesn't touch the load step) but relevant context for
   whoever scopes the "open a 1MB/3MB document" experience: it will cost close to 1-2 frames of
   synchronous main-thread time regardless of how the viewport itself is virtualized, unless the
   load step itself is moved off the main thread or chunked.
5. **[Worth noting, not blocking] Overlay memory, ~6.5x amplification.** §2: a 3MB document
   carries ~20MB of `Vec<Node>` overlay alone. Linear, consistent, not alarming by itself, but a
   real line item alongside the rope, block index, undo stack, and CM6/DOM state.
6. **[Confirmed fine, no action] `decorations()` — flat 300KB→3MB.** §5: the O(total node
   count) linear-scan risk research/08 flagged and ranked last (§10 item 5) has since been fixed
   (`block_floor` via `partition_point`, landed in the M1 PR-review pass, predating this spike)
   and holds up through 3MB. Nothing to do here.
7. **[Confirmed fine, no action] Decoration JSON serialization — flat at 1MB (native).** §9:
   8.0-8.2µs mean, matching research/08's 100KB number almost exactly. Viewport-bound as
   designed.
8. **[Confirmed fine, no action] Anchor mapping at 1,000 live anchors.** §7: no measurable
   overhead at any size tested, up to 3MB.
9. **[Confirmed fine, no action] `command(indentList)`, undo, redo.** §6: track `apply_edit`'s
   mid-document cost exactly (same underlying path); no separate risk.
10. **[Noted, not actioned, largest single component at 100KB-1MB, but 30-40x headroom] The
    JS/wasm boundary-crossing tax on `applyEdit`+`decorations`.** §10: dominates total combined
    cost at 100KB (~76%) and 1MB (~51%), shrinking in relative share (not absolute size) as
    documents grow (~37% at 3MB) because it's roughly a fixed per-call cost while core cost
    keeps growing. Not a de-risk item for M2 (headroom is enormous, §11), but the place a future
    optimization pass would have the most leverage if the budget ever tightens — same finding as
    research/08 §10 item 3, now confirmed to persist at 1MB/3MB scale rather than being purely a
    100KB-specific artifact.

---

## Appendix: reproducing these numbers

```
# Native Rust suite (this report's §2-9):
cargo test -p oxidown-core --release --test perf_1mb_derisk -- --ignored --nocapture

# Deeper local run:
OXIDOWN_PERF_ITERS=300 cargo test -p oxidown-core --release --test perf_1mb_derisk -- --ignored --nocapture

# JS-side wasm-boundary bench (this report's §10) — requires the wasm pkg built
# (pnpm build:wasm), which it was for this run:
cd packages/oxidown-view-cm6 && npx vitest bench test/perf-1mb.bench.ts

# Confirm the bench file is invisible to the normal test run:
cd packages/oxidown-view-cm6 && npx vitest list | grep perf-1mb   # (no output)
pnpm -r test                                                       # unaffected

# Full workspace gate:
cargo test --workspace
```

Files added for this report:
- `crates/oxidown-core/tests/perf_1mb_derisk.rs` (new — the native benchmark suite, §2-9,
  ignored by default; does not affect `cargo test --workspace`).
- `packages/oxidown-view-cm6/test/perf-1mb.bench.ts` (new — the JS-side wasm-boundary bench,
  §10; matches vitest's benchmark glob only, invisible to `pnpm test`/`vitest run`).
- No file under `crates/oxidown-core/src`, `crates/oxidown-wasm/src`, or
  `packages/oxidown-view-cm6/src` was modified — `git diff --stat` against each is empty. (A
  temporary, since-fully-reverted instrumentation pass touched `editor.rs` locally to gather
  §8's window-size numbers; see §1's method note.) No file under `apps/` was touched. No
  existing test file was modified.
