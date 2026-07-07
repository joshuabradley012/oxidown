# Perf Baseline: Reparse Cost, the Boundary Budget, and Where the Time Goes

> Measured July 2026 against `m1-gfm-anchors-commands-streaming`, in response to the "editor feels a little slow" report. **Measurement only — no optimization changes were made.** All numbers below are native Rust, **release mode** (`cargo test --release`), on an Apple M4 Pro (macOS, arm64, rustc 1.96.1). Every number carries the usual caveats of a single machine, a handful of hundred iterations, and `Instant`-based sampling — they are a baseline and a scaling signal, not a certified benchmark. Where a number is decision-relevant (the 100KB/300KB contract check), it was run twice; both runs are reported.

---

## 0. Headline finding, up front

**`Editor::apply_edit` has no fast path at all, for any edit position.** The tail fast path (`tail_fast_path_region` / `reparse_tail`) exists in `crates/oxidown-core/src/editor.rs`, but it is wired into exactly one caller: `stream_append` (the AI-streaming ingestion API). The interactive, every-keystroke path — `apply_edit`, called by the view for every user edit — unconditionally calls `reparse_with`, a full `parser::parse_document` pass over the **entire current document**, regardless of whether the edit is at the start, middle, or end. This was confirmed two ways:

1. **Code reading**: `apply_edit` (editor.rs line 173) calls `self.reparse_with(&batch)` unconditionally — no position check, no branch to `reparse_tail`. Only `stream_append` (line 416-421) checks `tail_fast_path_region` first.
2. **Measurement**: inserting one character at doc START, MIDDLE, and END is statistically indistinguishable at every document size tested (e.g. at 300KB: start 1311µs, middle 1272µs, end 1263µs mean — a ~4% spread, well inside run-to-run noise). If the tail fast path were active for end-of-doc edits, END would look like `stream_append`'s ~10µs, not ~1.3ms.

This directly contradicts this task's own starting assumption ("`apply_edit` uses a tail fast path ONLY for appends at/after the last top-level block") — that description matches `stream_append`, not `apply_edit`. **Every normal keystroke, anywhere in the document, pays the full O(doc) reparse.** `docs/boundary-v0.md` line 64 requires `applyEdit` to be "O(edit + dirty block), not O(doc)" — the current implementation does not meet this, at any document size. It happens to still be fast enough in absolute terms at typical sizes (see §6), which is presumably why this hasn't been visibly broken — but it is not the O(edit + dirty block) the contract calls for, and it gets linearly worse as documents grow.

The one place the fast path *is* wired up (`stream_append`) proves the mechanism works: streaming 2000 chunks into a 100KB→200KB-growing document costs a flat **mean 10µs, p95 15µs** per append (`stream_perf.rs`, re-run for this report) — about **40x cheaper** than `apply_edit`'s ~400µs at comparable size, and the cost does not grow as the document grows during the run. That's the empirical proof that "wire the existing fast path into more of `apply_edit`" is a real, already-validated lever, not a hopeful guess.

---

## 1. Method

- New file: `crates/oxidown-core/tests/perf_baseline.rs` — 6 `#[ignore]`d tests, run via:
  ```
  cargo test -p oxidown-core --release --test perf_baseline -- --ignored --nocapture
  ```
  Iteration counts default to fast/CI-friendly values (80-300 depending on test) and scale up via `OXIDOWN_PERF_ITERS=<n>` for a deeper local run. All assertions are loose ceilings (10-40x the observed p95), matching `perf_smoke.rs`/`stream_perf.rs` convention — they exist so the suite stays a runnable regression trip-wire, not a tight gate.
- Doc generation: `generate_mixed_doc(target_bytes)`, a richer sibling of `perf_smoke.rs`'s `generate_doc` — cycles through ATX headings, mixed-inline paragraphs (bold/italic both flavors, code, strike, link, autolink, CJK, emoji), plain paragraphs, nested blockquotes, fenced code blocks, and lists (nested bullets, ordered, tasks), matching the shape of `apps/web-demo/src/sample-doc.ts`'s `SAMPLE_DOC` without depending on it (constraint: don't touch `apps/`). A small nested-list snippet containing a `PERF_INDENT_TARGET` marker is spliced in at the document's midpoint for the `indentList` benchmark.
- Four sizes tested throughout: **~3KB** (demo-sample shape), **~30KB**, **~100KB** (the contract's reference size), **~300KB**.
- **No `src/` file was modified.** Every API used (`parser::parse_document`, `parser::ParseResult.nodes`, `Editor::apply_edit`/`decorations`/`command`, `Decoration`/`BlockStyle`/`MarkStyle`/`WidgetKind`) was already `pub`. The only non-test change anywhere is a `serde_json` **dev-dependency** added to `crates/oxidown-core/Cargo.toml`, needed to replicate `oxidown-wasm`'s exact JSON-serialization step natively (§7) — `oxidown-wasm` itself depends on `wasm-bindgen`/`js-sys`, which don't link off `wasm32`, so that crate's code cannot run in a native test; the JSON-shape logic is small enough to faithfully duplicate (field names/shapes copied 1:1 from `crates/oxidown-wasm/src/lib.rs`'s `decoration_json`).

---

## 2. Reparse-frequency audit (code reading)

Every call site of `reparse_with` / `reparse_tail` in `crates/oxidown-core/src/editor.rs`:

| Caller | Path taken | Frequency in normal use |
|---|---|---|
| `load` | `reparse_with` (full) | Once per document open/replace. |
| `apply_edit` | `reparse_with` (full) — **always**, no position check | **Once per keystroke** — the interactive typing path. |
| `undo` | `reparse_with` (full) | Once per undo. |
| `redo` | `reparse_with` (full) | Once per redo. |
| `apply_plan` (called by every `command(...)` variant: toggleStrong/Em/Strike/Code, setHeading, toggleTask, indentList/outdentList) | `reparse_with` (full) | Once per command invocation (toolbar click, Tab/Shift-Tab, checkbox click). |
| `stream_append` | `tail_fast_path_region` → `reparse_tail` (tail-only) **if** the insertion is at/after the last top-level block AND that block starts at a line boundary; else falls back to `reparse_with` (full) | Once per AI-stream chunk — **the only path with a real fast path**, and it's exclusive to the AI-streaming API, never touched by `apply_edit`. |

**For a typical typing session (single-character inserts mid-document): every keystroke → `apply_edit` → unconditional `reparse_with` → one `self.text.text()` full-rope-to-`String` copy, followed by one full `parser::parse_document` pass over that copy.** Position within the document is irrelevant to which path fires — there is exactly one path, always full.

### Per-call allocations worth flagging

- **`reparse_with`** (editor.rs:558): `let text = self.text.text();` — `TextBuffer::text()` is `self.rope.to_string()` (text.rs:119-121), an **O(doc) copy of the entire rope into a fresh `String`**, on every full reparse (i.e. every keystroke, every undo/redo, every command).
- **`Editor::command`** (editor.rs:306): `let src = self.text.text();` — a **second, independent full-document copy**, made *before* the edit is even applied, purely so the command planners (`commands.rs`) have a `&str` to slice from. Confirmed by reading every planner (`toggle_inline`, `set_heading`, `indent_list`, etc.): they only ever index small local ranges out of `src` (`src[pos..d.start]`, `src[d.start..d.end]`, …) — none of them scan or copy the whole string. **This copy is pure waste**: a `command()` call on a 300KB document allocates and memcpy's the whole 300KB string just to read a few bytes near the cursor, then `apply_plan`'s `reparse_with` immediately makes a *third* full-document allocation (a fresh post-edit copy) for the reparse itself.
- **`Editor::decorations`** does **not** call `self.text.text()` — the specific worry named in the task ("does `self.text.text()` copy the whole rope" in `decorations()`) does not apply here; `decorations()` only does bounded `utf16_to_byte_floor/ceil` conversions on the viewport/selection endpoints (each O(log doc) via ropey, not O(doc)). It does have its own, smaller allocation issues (§6): `decorations::compute` builds its output `Vec<Decoration>` with `Vec::new()` (no capacity hint → repeated reallocation as it grows), and — more importantly — it **iterates the entire cached overlay** (every `Node` in the whole document, not just the viewport) doing a cheap range-check-and-`continue` for everything outside the viewport. That's O(total node count) iteration overhead per call, with a very small per-node constant (confirmed by the flat-ish decorations-only numbers in §6), but it's an unbounded-by-viewport cost that will keep growing as documents scale past what's tested here.

---

## 3. Numbers: `parser::parse_document` scaling (isolates reparse cost)

| size | bytes | overlay nodes | mean | p50 | p95 | max |
|---|--:|--:|--:|--:|--:|--:|
| ~3KB | 3,184 | 131 | 11.8-12.6µs | 11.7-12.1µs | 12.9-15.5µs | 14.3-21.8µs |
| ~30KB | 30,828 | 1,347 | 103.5-105.5µs | 101.9-104.0µs | 119.9-121.2µs | 122.5-125.8µs |
| ~100KB | 102,459 | 4,463 | 359.5-376.6µs | 354.8-368.3µs | 378.2-424.1µs | 403.4-460.2µs |
| ~300KB | 307,344 | 13,317 | 1203.8-1246.7µs | 1197.0-1223.6µs | 1236.5-1400.7µs | 1326.0-1594.4µs |

(ranges are two independent runs; 80 iterations each after a warm-up)

**Scaling is linear**, both in time and in node count — roughly 100x the bytes (3KB→300KB) produces ~100x the parse time (11.8µs→1204µs, ~102x) and ~102x the node count (131→13,317). Per-KB cost holds at roughly **3.9-4.1µs/KB** across two orders of magnitude, i.e. no evidence of a discontinuity or superlinear blowup within this range — `pulldown-cmark`'s single-pass parse genuinely is O(n) here. This confirms `parse_document` itself is *not* the architectural problem; **the problem is that it's called on the whole document on every keystroke instead of on just the edited region.**

---

## 4. Numbers: `apply_edit` (1-char insert) by position

| size | position | mean | p50 | p95 | max |
|---|---|--:|--:|--:|--:|
| ~3KB | start | 15.9µs | 15.7µs | 20.0µs | 31.2µs |
| ~3KB | middle | 14.2µs | 14.0µs | 15.2µs | 19.4µs |
| ~3KB | end | 14.7µs | 14.3µs | 16.8µs | 21.7µs |
| ~30KB | start | 119.2µs | 115.0µs | 131.5µs | 149.8µs |
| ~30KB | middle | 116.2µs | 114.0µs | 128.6µs | 143.6µs |
| ~30KB | end | 117.8µs | 113.8µs | 130.6µs | 146.5µs |
| ~100KB | start | 402.7µs | 396.6µs | 446.0µs | 529.7µs |
| ~100KB | middle | 404.0µs | 399.2µs | 451.2µs | 493.0µs |
| ~100KB | end | 397.8µs | 392.2µs | 439.3µs | 521.0µs |
| ~300KB | start | 1311.2µs | 1285.4µs | 1439.6µs | 1489.1µs |
| ~300KB | middle | 1272.4µs | 1253.6µs | 1334.7µs | 1390.4µs |
| ~300KB | end | 1263.5µs | 1251.3µs | 1335.4µs | 1605.0µs |

**Start/middle/end are statistically indistinguishable at every size** (spreads of 2-4%, inside sampling noise) — the headline finding in §0, now with numbers. Compare to `stream_append`'s flat **mean 10µs / p95 15µs** (re-measured via `stream_perf.rs`, unmodified) on a document in the same 100-200KB range: the tail fast path, where it's actually wired up, is **~40x cheaper** than the full reparse every `apply_edit` call pays today.

**Reparse's share of `apply_edit`'s total cost** (parse_document mean ÷ apply_edit-middle mean): 83% at 3KB, 89% at 30KB, **89% at 100KB**, 95% at 300KB. The full reparse is not just *a* cost in `apply_edit` — it *is* `apply_edit`, increasingly so as the document grows.

---

## 5. Numbers: `applyEdit` + `decorations` combined, by size (the contract's exact shape)

Mid-document single-char insert, immediately followed by a `decorations()` call for a ~3k-CU viewport centered on the edit, with one cursor selection — the literal shape of the boundary contract's performance budget, generalized from `perf_smoke.rs`'s single 100KB check to all four sizes. Two independent runs:

| size | run | mean | p50 | p95 | max |
|---|---|--:|--:|--:|--:|
| ~3KB | 1 | 41.4µs | 39.7µs | 50.8µs | 74.6µs |
| ~3KB | 2 | 47.1µs | 45.8µs | 52.5µs | 96.8µs |
| ~30KB | 1 | 140.8µs | 137.4µs | 157.2µs | 165.3µs |
| ~30KB | 2 | 154.0µs | 153.0µs | 175.2µs | 232.0µs |
| **~100KB** | 1 | **422.4µs** | 421.5µs | **442.4µs** | 464.2µs |
| **~100KB** | 2 | **415.3µs** | 411.2µs | **437.3µs** | 530.3µs |
| **~300KB** | 1 | **1316.3µs** | 1304.5µs | **1398.1µs** | 1497.1µs |
| **~300KB** | 2 | **1353.0µs** | 1335.3µs | **1530.8µs** | 2206.9µs |

Cross-check against the pre-existing `perf_smoke.rs` (unmodified, its own simpler doc generator, same 100KB nominal size, 300 iterations, random position wander instead of fixed-middle): **mean 424µs, p95 747µs, max 1078µs**. Same order of magnitude and same conclusion (comfortably under the 1ms core-side proxy at 100KB), but its p95 sits notably closer to the 1ms line than the fixed-middle numbers above — a reminder that p95 from a few hundred samples on a shared laptop carries real noise, and that position-within-document doesn't matter (§4) but *which* random positions land in a given run does perturb the tail.

---

## 6. Numbers: `decorations()` alone, ~3k-CU middle viewport

| size | mean | p50 | p95 | max |
|---|--:|--:|--:|--:|
| ~3KB | 25.3µs | 24.5µs | 32.1µs | 48.8µs |
| ~30KB | 26.9µs | 26.2µs | 32.5µs | 47.7µs |
| ~100KB | 31.6µs | 30.8µs | 36.8µs | 50.4µs |
| ~300KB | 40.2µs | 39.2µs | 47.5µs | 83.7µs |

Never reparses (as designed), and the absolute cost is small — but it is **not flat**: a 100x growth in document size (and overlay node count, 131→13,317) produces a ~59% growth in mean cost (25.3µs→40.2µs). That's the O(total node count) linear-scan overhead named in §2 showing up empirically: small today, but a real scaling liability the contract's "O(edit + dirty block)" framing doesn't currently apply to (`decorations()` isn't in that clause, but the same architectural instinct — bound cost by viewport size, not doc size — is being violated here too, just more cheaply).

---

## 7. Numbers: `command(IndentList)` on a nested list item, mid-document

Same shape as `apply_edit` (goes through `apply_plan` → `reparse_with`), plus `command()`'s extra full-text copy (§2):

| size | mean | p50 | p95 | max | vs. apply_edit-middle mean |
|---|--:|--:|--:|--:|---|
| ~3KB | 15.4µs | 14.4µs | 20.4µs | 28.8µs | +8.5% |
| ~30KB | 118.5µs | 115.2µs | 137.1µs | 151.5µs | +2.0% |
| ~100KB | 407.6µs | 402.8µs | 435.9µs | 462.6µs | +0.9% |
| ~300KB | 1348.1µs | 1340.5µs | 1439.7µs | 1621.3µs | +5.9% |

The extra full-document copy in `command()` (on top of the one inside `reparse_with`) adds a small, noisy 1-9% on top of the already-dominant reparse cost — real and free to remove, but not the main event.

---

## 8. Numbers: wasm-boundary JSON serialization (native Rust replication)

`crates/oxidown-wasm/src/lib.rs` builds one `serde_json::Value` per `Decoration` (`decoration_json`) into an array, then serializes the whole array to a JSON **string** in one shot (`to_js`'s `value.to_string()`, later `JSON.parse`'d on the JS side) — "single large blob" by design (per that file's own module doc, citing `research/03-rust-ecosystem.md`). That mapping was replicated field-for-field in `perf_baseline.rs` (no `wasm-bindgen`/`js-sys` needed) and timed in isolation, for the same ~3k-CU/100KB-doc viewport used in §6:

- **281 decorations** in the viewport → **14,051-byte** JSON payload.
- Serialization alone: **mean 45.3µs, p50 44.0µs, p95 53.2µs, max 78.3µs**.

This is the most surprising individual number in this report: **the JSON-string serialization step (45.3µs) costs *more* than computing the decorations in the first place (31.6µs, §6)** — a ~1.4x tax, on top of the core compute, on every viewport/decoration refresh (scrolling, cursor moves, any redraw — not just keystrokes). It's driven by the `json!` macro heap-allocating a fresh `String` for every `"kind"`/`"style"`/`"widget"` tag on every decoration (even though the tag set is a small fixed vocabulary of `&'static str`s) plus one `serde_json::Map` per decoration, then a full tree walk to flatten it all into one string. None of this includes the JS-side `JSON.parse` or any DOM/CM6 work — that's the browser-side half named in the placeholder section below.

---

## 9. Contract verdict

Two distinct budgets in `docs/boundary-v0.md`, both under suspicion; both checked:

### 9a. Complexity: `applyEdit` "must be O(edit + dirty block), not O(doc)" (line 64)

**FAILS, unconditionally, at every size tested.** §0 and §4 show `apply_edit`'s cost is insensitive to edit position and scales linearly with total document size (matching `parse_document`'s own linear scaling almost exactly) — the textbook signature of O(doc), not O(edit + dirty block). The only code path in this crate that actually achieves O(edit + dirty-tail-block) is `stream_append`'s fast path, and it is unreachable from the interactive typing API.

### 9b. Latency: "`applyEdit` + `decorations`... < 1ms combined p95 in the core... measured from the JS side" (line 127-128)

This is officially a JS-side measurement; the numbers here are a **native-Rust proxy that excludes the wasm/JS boundary crossing and all DOM work**, so they should be read as an optimistic lower bound, not the contract check itself (see the placeholder section below for the real one).

| size | p95 (core-only, this report) | vs. 1ms budget |
|---|--:|---|
| ~3KB | ~51-53µs | **PASS**, ~95% margin |
| ~30KB | ~157-175µs | **PASS**, ~83% margin |
| **~100KB (reference size)** | **~437-442µs** | **PASS today, ~56-58% margin** — but see below |
| ~300KB | **~1398-1531µs** | **FAIL**, 40-53% over budget |

At the contract's own reference size (100KB) the core-only number passes today. But layering in the one additional cost this report *did* measure natively — JSON serialization of the decorations payload (§8, ~45-53µs, not included in the table above) — brings the 100KB core+serialization total to roughly **482-495µs, ~50% of the 1ms budget consumed before a single JS/DOM operation runs.** Extrapolating the ~30KB→300KB growth rate (≈4.6µs added latency per additional KB above 100KB) puts the **p95-crosses-1ms point at roughly 200-250KB** — a document size well within reach of normal long-form notes, imported files, or a session's worth of AI-streamed content left in the doc. The margin at the reference size is real but shrinking, and it shrinks for architectural reasons (§9a) that don't go away by tuning constants.

**Bottom line**: the *latency* budget is not on fire today at typical (≤100KB) sizes, but the *complexity* budget already is, and it is the complexity violation that will turn into a latency violation as soon as users' documents grow past the size class this was tuned against.

---

## 10. Ranked optimization candidates (by measured impact)

1. **Wire the existing tail fast path into `apply_edit` for end-of-document edits.** The mechanism already exists (`tail_fast_path_region` + `reparse_tail`) and is already proven in production use by `stream_append` at **~40x cheaper** than full reparse (10µs vs ~400µs at 100KB-scale). Currently `apply_edit` never calls it. This is the single highest-leverage, lowest-risk change available — it directly attacks the cost that is **83-95% of `apply_edit`'s total time** (§4) and **85-92% of the full applyEdit+decorations combined cost at 100KB/300KB** (§5 vs §3), for the (extremely common) case of typing at the end of a document/section. It does not help arbitrary mid-document edits.
2. **Give `apply_edit` a real incremental/block-local reparse for non-tail edits**, using the block index that already exists (`BlockIndex`, "consumed by streaming's fast path... eventually the sidecar/sync story" per its own module doc) to identify the dirty top-level block and reparse only that block plus re-bias everything after it — the actual fix for the O(edit + dirty block) contract clause (line 64) for the general case, not just appends. Bigger lift than #1, but it's the only way to make the *complexity* verdict (§9a) pass rather than continuing to pass the *latency* verdict on borrowed time as documents grow (§9b).
3. **Stop the wasm boundary's JSON serialization from costing more than the computation it's serializing.** §8 measured serialization (45.3µs) exceeding decorations compute (31.6µs) at the contract's own reference size — a ~140% tax on every viewport refresh, not just keystrokes. Concretely: avoid a fresh heap `String` allocation per decoration per field (the `json!` macro's handling of `&'static str` tags), and/or move off the "stringify then `JSON.parse`" round trip toward a format that avoids re-allocating the whole fixed vocabulary of style/kind tags per call (e.g. integer-coded kinds/styles decoded to strings on the JS side, or `serde-wasm-bindgen`/typed arrays for the position fields, which are the bulk of the payload's 14KB). Second-highest ranked because it's measured, surprising, and on a hotter path than keystrokes (every scroll/cursor-move decoration refresh pays it too).
4. **Drop `Editor::command`'s redundant full-document copy** (`let src = self.text.text();`, editor.rs:306). Every planner in `commands.rs` only slices small local ranges out of it (confirmed by reading `toggle_inline`/`set_heading`/`indent_list`/etc. — none scans or copies the whole string), so this is a 100%-wasted O(doc) allocation+copy on every single command invocation, on top of the (already dominant, and separately necessary) copy inside `reparse_with`. Measured as a real but modest 1-9% tax on top of full-reparse cost (§7) — worth fixing for its own sake (it's free, no design work required — pass a `&TextBuffer`/rope reference instead of materializing a `String`), but ranked below #1-3 because reparse cost dwarfs it at every size tested.
5. **Bound `decorations()`'s per-call cost by viewport size, not total document size.** Currently `decorations::compute` linearly scans the *entire* cached overlay (every node in the whole document) to find the ones overlapping the viewport (§2, §6) — cheap per node today (25.3µs→40.2µs mean across a 100x size range), but unbounded, and it's the one part of this whole picture that scales with the overlay directly rather than with any editing action. Since the overlay is produced by a single forward parse (nodes in document order), a viewport query could binary-search a sorted-by-start index instead of scanning linearly. Ranked last: it is real, but by far the smallest number measured (tens of microseconds even at 300KB), and the current linear scan's constant is small enough that it's not yet a practical problem — flagged as a scaling risk to watch, not an active fire. Pair with pre-sizing `compute`'s output `Vec::new()` (no capacity hint today) while touching this code.

---

## Browser-side numbers

Measured from the JS side of the wasm boundary (the contract's official vantage point,
`docs/boundary-v0.md` "Performance budget"), in the live demo (`?core=wasm`, Vite dev server,
Chrome, same M4 Pro). Method: direct calls against the dev-exposed `window.__oxidownCore`
(timing includes wasm-bindgen call overhead + JSON serialize/parse) and `EditorView.dispatch`
(+ `view.measure()` to force CM6's synchronous measure/draw cycle; paint/composite excluded —
the profiling tab was backgrounded, where rAF never fires). Medians / p95 over 40–200
iterations. Doc shapes match §1's mixed-markdown generator in structure.

| Doc size | `applyEdit` mid-doc (JS-side) | `decorations` 3k-CU viewport | keystroke: dispatch only | keystroke incl. CM measure/draw |
|---|---|---|---|---|
| 3.5KB (demo sample) | ~0.04 / 0.1 ms | 0.3 / 0.5 ms | 0.3 / 0.9 ms | 0.3 / 2.1 ms |
| 100KB | 0.7 / 0.8 ms | 0.2 / 0.3 ms | 0.8 / 1.6 ms | — |
| 292KB | 2.1 / 3.3 ms | 0.3 / 0.3 ms | 2.3 / 2.8 ms | 3.0 / 3.6 ms |

Corroborating observations, consistent with the native numbers:
- **JS-side `applyEdit` ≈ native `apply_edit` + ~0.3ms wasm/JSON overhead** at 100KB
  (0.7ms vs 0.36–0.44ms native) — the boundary tax is real but secondary to the reparse.
- **Combined contract check from JS: 1.1ms p95 at 100KB — FAILS the <1ms budget** at exactly
  the contract's document size (native-only passes at ~0.44µs+31µs because it excludes the
  boundary tax §8 measures; the contract's vantage point is the JS side, so the JS number is
  the authoritative verdict). 3.6ms at 292KB.
- `decorations` is flat (~0.3ms) at every size — viewport-bound, as designed. Payload for the
  sample viewport: 309 decorations, ~14.5KB of JSON.
- Pure cursor movement (selection dispatch + the rebuild's decorations query): 0.4 / 0.7 ms
  at 292KB — cheap; per-keystroke cost is not selection-driven.
- At 292KB the core reparse is ~70% of total keystroke cost including CM's measure/draw.
- The demo-sample numbers are fast enough that any *perceived* sluggishness on the small doc
  is either frame-level rendering (not measurable from a background tab) or was experienced
  on a grown document (post-streaming / "Load large doc"). Worth re-testing subjectively
  after the reparse fix lands.

---

## Appendix: reproducing these numbers

```
cargo test -p oxidown-core --release --test perf_baseline -- --ignored --nocapture
cargo test -p oxidown-core --release --test perf_smoke    -- --ignored --nocapture
cargo test -p oxidown-core --release --test stream_perf    -- --ignored --nocapture

# Deeper local run (more iterations, better tail-percentile confidence):
OXIDOWN_PERF_ITERS=1000 cargo test -p oxidown-core --release --test perf_baseline -- --ignored --nocapture
```

Files added/changed for this report:
- `crates/oxidown-core/tests/perf_baseline.rs` (new — the benchmark suite described above).
- `crates/oxidown-core/Cargo.toml` (added `serde_json` under `[dev-dependencies]`, test-only, needed for §8's native replication of the wasm crate's JSON step).
- No file under `src/` was modified. No file under `packages/` or `apps/` was modified.
