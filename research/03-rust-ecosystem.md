# Rust Editor-Core Ecosystem

> Research compiled July 2026 for the Oxidown plan refresh. Version numbers verified against crates.io/GitHub at research time.

## Executive summary

1. **Text storage**: Ropey remains the default (Helix uses it; 2.0 goes byte-indexed but is still in beta). Crop and jumprope are faster on real editing traces but crop is a slow trickle and jumprope is dormant. Zed's SumTree rope is the best design study but is GPL and unpublished.
2. **Markdown parsing**: **No Rust crate delivers byte-accurate spans + incremental reparse + lossless CST simultaneously.** pulldown-cmark gives byte spans (via `into_offset_iter()`), tree-sitter-md gives incremental (but spec-inaccurate), nothing gives lossless round-trip. Building a rowan/cstree-style markdown CST (the Typst/rust-analyzer/Lezer pattern) is an open gap — and probably the core differentiator.
3. **xi-editor's lesson is the single most important architectural input**: the fatal mistake was not Rust or the rope but making the view↔core boundary asynchronous and cross-process, and using a CRDT to paper over that. A "Rust core + thin views" design must give views *synchronous in-process reads*.
4. **Zed/Lapce/Helix** all converge on rope + tree-sitter; both Zed and Helix independently picked pulldown-cmark 0.13 for markdown rendering. Zed's CRDT works because collaboration was a committed day-one requirement and it lives in-process.
5. **FFI**: uniffi (0.32, June 2026) is the safe multi-language default; per-call cost ~1–4 µs means per-keystroke calls are fine, per-character loops are not. Watch BoltFFI (crux migrated to it in 2026).
6. **WASM**: per-keystroke calls returning full HTML are proven sane for documents up to ~hundreds of KB (typst.app recompiles a full typesetter per keystroke). The rules: one coarse call per edit, batch patches out, never per-node callbacks.
7. **Full-Rust UI everywhere is not shippable in mid-2026** for a text-input-heavy app. Fatal gaps: no AccessKit iOS/web adapters, winit mobile IME still in beta rework, canvas-web hostile to IME/a11y. The pragmatic architecture is Rust core + platform-native text surfaces.

---

## 1. Text storage: ropey vs crop vs jumprope vs xi-rope

| Crate | Latest | Maintenance (Jul 2026) | Indexing | Graphemes | Notes |
|---|---|---|---|---|---|
| **ropey** | 1.6.1 stable; **2.0.0-beta.1** (Aug 2025) | Active | v1: chars; **v2: bytes** | Via examples + `unicode-segmentation` | ~8.6M downloads; ~10% memory overhead; O(1) clones |
| **crop** | 0.4.3 (Apr 2025) | Slow trickle | Bytes | Optional feature | ~3–4x faster than ropey 1.x on real traces; O(1) clones |
| **jumprope** | 1.1.2 (May 2023) | **Dormant** | Chars | No | Fastest raw throughput; **no cheap clones** |
| **xi-rope** | 0.3.0 (2019) | Dead upstream; lives as `lapce-xi-rope` | — | — | ~80x slower than jumprope in jumprope's benchmarks |
| **Zed rope** | unpublished | Active in-tree | Bytes (SumTree dims) | Via unicode-segmentation | **GPL-3.0, `publish = false`** — study, don't depend |

- **Ropey 2.0 is not final** (beta.1 Aug 2025; commits through Mar 2026). Headline change: primary indexing chars→bytes; secondary metrics (`metric_chars`, `metric_utf16`, `metric_lines_*`) opt-in features ([docs.rs](https://docs.rs/ropey/2.0.0-beta.1/ropey/)). No grapheme API in any rope — all delegate to `unicode-segmentation`.
- **Performance**: crop's benchmarks (real editing traces): automerge-paper — crop 12.4 ms / jumprope 12.5 ms / ropey-1 44.1 ms / String 108.6 ms ([crop README](https://github.com/noib3/crop)). All published numbers compare against ropey 1.x; nobody has benchmarked ropey 2.0-beta.
- **What real editors use**: Helix → ropey 1.6.1 (simd); Lapce → lapce-xi-rope; Zed → own SumTree rope (128-byte `ArrayString` chunks, u128 bitmask metadata: +250% coordinate-conversion throughput — [Zed Decoded: Rope & SumTree](https://zed.dev/blog/zed-decoded-rope-sumtree), [Rope Optimizations](https://zed.dev/blog/zed-decoded-rope-optimizations-part-1)).
- **Recommendation**: ropey (1.6.1 now, 2.0 on release — byte indexing matches markdown-span math). Design note from every serious editor: **immutable/COW snapshots are the load-bearing feature**, not raw edit throughput.

## 2. Markdown parsing

The three requirements — (a) byte-accurate spans, (b) incremental reparse, (c) lossless CST round-trip — are **not jointly satisfied by any existing crate**:

| Library (Jul 2026) | (a) Byte spans | (b) Incremental | (c) Lossless round-trip | Maintenance |
|---|---|---|---|---|
| **pulldown-cmark** 0.13.4 | **Yes** — `into_offset_iter() -> (Event, Range<usize>)` byte-exact | No | No — events normalize setext/ATX, bullet chars, `*` vs `_`, fence style | Active, ~114M downloads |
| **comrak** 0.53.0 (Jul 2026) | Near — per-node `Sourcepos` (1-based line + UTF-8-byte column) | No | No — semantic AST; `format_commonmark` normalizes | Very active, monthly |
| **markdown-rs** 1.0.0 (Apr 2025) | Yes-ish — unist `Position` (verify byte vs char semantics) | No | No — mdast drops markers/heading style | **Dormant — zero commits since 1.0.0** |
| **tree-sitter-md** 0.5.3 (Feb 2026) | Yes — node byte ranges | **Yes — only incremental option** | Concrete tree, but "lots of inaccuracies" vs CommonMark | Active |
| **jotdown** 0.10.0 | Yes | No | No | Active; **Djot, not markdown** |

Details:
- **pulldown-cmark** is the ecosystem default (Zed and Helix both pin 0.13). GFM via `Options` (tables, tasklists, strikethrough, footnotes, math, alerts) but **no GFM autolink literals** ([#494](https://github.com/pulldown-cmark/pulldown-cmark/issues/494)). `pulldown-cmark-to-cmark` re-serializes events but is normalizing.
- **comrak** passes 652/652 CommonMark 0.31.2 + all 670 GFM spec tests; sourcepos got a major overhaul in v0.44–0.47 (Sep–Oct 2025). ~7x slower than pulldown-cmark ([1Password markdown-benchmarks](https://github.com/1Password/markdown-benchmarks)).
- **Lossless formatting practice**: [mdformat](https://github.com/hukkin/mdformat) guarantees *semantic* losslessness by re-parsing its own output and asserting render-equivalence — a validation trick worth stealing. [dprint-plugin-markdown](https://github.com/dprint/dprint-plugin-markdown) is built on pulldown-cmark and **slices original source by span rather than trusting event text** — proof pulldown's spans are production-grade.
- **The gap**: no rowan/cstree-based markdown CST exists on crates.io. The proven architecture for "all three properties" is Typst's hand-written lossless CST with minimal-range incremental reparse ("largely adapted from Rust Analyzer" — [typst-syntax](https://lib.rs/crates/typst-syntax)), and in JS, [Lezer's markdown parser](https://github.com/lezer-parser/markdown). Substrates: [rowan](https://github.com/rust-analyzer/rowan), [cstree](https://crates.io/crates/cstree).
- **Pragmatic recipe** (what shipping tools do): pulldown-cmark byte spans as semantic truth + slice the original rope by span (never regenerate text you didn't edit); full reparse per edit is fine for typical docs (pulldown parses ~100KB in low-single-digit ms); tree-sitter-md only for highlight-grade overlays; mdformat's reparse-and-compare check on any serialization. For true lossless + incremental, budget to **build the CST layer** on rowan/cstree.

## 3. Lessons from xi-editor

Primary source: Raph Levien's [xi-editor retrospective](https://raphlinus.github.io/xi/2020/06/27/xi-retrospective.html) (Jun 2020). Load-bearing conclusion, verbatim: *"while the process split with plug-ins is supportable (similar to the Language Server protocol), I now firmly believe that the process separation between front-end and core was not a good idea."*

1. **Async UI fails on observable intermediate states, not throughput.** Word wrap during live resize produced un-fixable tearing races. Even the "success story" (async scroll-ahead) "took months to get right. By contrast, if we just had the text available as an in-process data structure for the UI to query, it would have been quite straightforward."
2. **CRDT was a one-size-fits-all answer to problems that are actually different.** Syntax highlighting: "any form of OT or CRDT is overkill." IME: "an impedance mismatch between platform IME protocols and a CRDT." Auto-indent: "doesn't fit nicely in the CRDT model at all."
3. **CRDT complexity leaks into every feature** — transpose needed an invented "drift" mechanic; soft spans were his "personal most regretted."
4. **Speculative collaboration support was YAGNI**: "either commit to doing collaborative editing, and marshal the resources... or have it out of scope."
5. **What he'd build instead**: "a simpler, largely synchronous model, that still of course has enough revision tracking to get good results with asynchronous peers like the language server."
6. **Serialization costs bite**: "JSON in Swift is shockingly slow."
7. **Monoliths are better for community** — gate-keeping around architectural rework killed contributions.
8. **The rope holds up.**

Corroboration: CodeMirror 6 rejected CRDTs for the same reasons ([Marijn Haverbeke](https://marijnhaverbeke.nl/blog/collaborative-editing-cm.html)); Matthew Weidner's server-reconciliation posts ([mattweidner.com](https://mattweidner.com/2025/05/21/text-without-crdts.html)). Zed is the honest counterpoint — CRDTs as a committed day-one collab product, **in-process, in a single synchronous core** ([Zed CRDT blog](https://zed.dev/blog/crdts)).

**Implications**: view↔core boundary = in-process API with synchronous reads, never IPC; editing semantics synchronous; async only at genuinely-async peers (LSP-like services, spellcheck) via revision-tracking + discard-and-recompute; CRDT only if collaboration is committed; treat serialization cost at any FFI boundary as a first-class budget item.

## 4. Zed, Lapce, Helix architectures

| | Text | Syntax | Rendering/UI | Markdown |
|---|---|---|---|---|
| **Zed** | Homegrown SumTree rope + in-house CRDT (anchors, Lamport clocks) | tree-sitter 0.26 | **GPUI** — published on crates.io Oct 2025 (v0.2.x, Apache-2.0) | tree-sitter-markdown for highlighting; **pulldown-cmark 0.13** → native GPUI elements for preview |
| **Lapce** | `lapce-xi-rope` fork | tree-sitter 0.22 | **Floem** (pre-1.0) | No dedicated story |
| **Helix** | ropey 1.6.1 | tree-sitter via own **tree-house** binding | Terminal only | tree-sitter-markdown grammar + pulldown-cmark for LSP hover |

- **Zed's SumTree** is "the soul of Zed": B+-tree with customizable summed Dimensions; `Arc`-wrapped immutable leaves make snapshots cheap, powering background work and collab; underpins 20+ subsystems.
- **DeltaDB**: Zed's CRDT/operation-based version-control layer recording every edit with stable operation identity; $32M Series B led by Sequoia (Aug 2025); early access June 2026 ([zed.dev/deltadb](https://zed.dev/deltadb)).
- **Lapce is slowing** (energy diverted to Floem/Lapdev). **Helix**: Steel plugin PR still an open draft.
- Takeaway: three independent teams converged on rope + tree-sitter + pulldown-cmark, and none puts a process boundary between view and core.

## 5. FFI: uniffi and alternatives

**uniffi** (0.32.0, Jun 2026; pre-1.0, active "FFI-1.0" redesign):
- **Proc-macro vs UDL**: ecosystem consensus is **proc-macro for new projects** (matrix-rust-sdk: attributes "preferred where applicable").
- **Async**: Rust `async fn` → Swift `async`/Kotlin `suspend`, production-proven in Element X; caveats: no built-in cancellation, Swift 6 strict-concurrency friction.
- **Overhead**: ~1.4 µs per no-op Swift call (from BoltFFI's adversarial benchmarks — plausible); objects cross as one Arc pointer (cheap), Records/strings copied through a RustBuffer every crossing. New in 0.32: zero-copy `&[u8]` args.
- **Kotlin is the weak leg**: bindings ride JNA — ~1.3–4 µs/call vs ~57–260 ns for JNI (~15–24x slower, [java-native-benchmark](https://github.com/zakgof/java-native-benchmark)); JNI replacement is an open, not-started issue. [gobley](https://gobley.dev/) provides Kotlin Multiplatform uniffi bindings but lags mainline.
- **Production users**: Firefox, Matrix/Element X, Nord, Automerge-swift, LiveKit.

**Alternatives**: flutter_rust_bridge v2 (only if Flutter); cxx (C++ hosts); swift-bridge (no-serialization-overhead, Swift-only); Swift 6.3's official Android SDK (Mar 2026) is the "share Swift instead of Rust" strategy, early-preview. **BoltFFI** (v0.27, Jun 2026, MIT): zero-copy bindgen claiming up to 1,000x faster than UniFFI; **crux deprecated its uniffi bindgen in favor of BoltFFI** — very young, watch it.

**Granularity**: per-keystroke FFI is comfortably in budget — even 50–100 uniffi calls per keystroke ≈ 0.1–0.4 ms against an 8–16 ms frame. What kills you: **per-character/per-span loops** (100k crossings ≈ 140–400 ms) and hidden per-call work (Matrix's regressions came from an FFI constructor hitting the database per item). Best practice: document behind an object handle; one call in per edit, one batched result out (damage list / state diff); bulk text as `&[u8]`; change notifications pushed as diff streams. [crux](https://github.com/redbadger/crux) is the formalized version of this.

## 6. WASM boundary

- **Raw calls are solved; glue and strings are not.** JS↔wasm calls: ~2.5–5 ns raw; a wasm-bindgen call with glue ≈ ~100 ns. Strings always copy + transcode UTF-8↔UTF-16 (~1–2 µs per small string) — **many small strings are the pathology, single large blobs are fine** (hundreds of MB/s).
- **Per-keystroke → WASM → full HTML string is sane** for docs to ~hundreds of KB (crossing + parse + encode for 100 KB ≈ 2–3 ms; the real bottleneck is DOM-side). Evidence: [markdown-wasm](https://github.com/rsms/markdown-wasm) beats marked/markdown-it *through* the string boundary (its README warns per-code-block JS callbacks tank performance); **typst.app runs a full typesetting compiler per keystroke in wasm** with ~26 ms incremental compiles via memoization ([comemo](https://github.com/typst/comemo)). For large docs, return changed-block diffs.
- **Automerge's lesson is the canonical protocol design**: don't materialize state across the boundary per read — `enablePatches()`/`popPatches()` keeps a JS mirror updated by batched patches per transaction; Automerge 2.0 perf journey 500 s → 0.66 s; 3.0 cut memory >10x.
- **Correction to folk wisdom**: current [crdt-benchmarks](https://github.com/dmonad/crdt-benchmarks) show ywasm edging out pure-JS Yjs on fine-grained ops — the durable arguments against wasm CRDTs are **bundle size and memory** (yjs 20 KB gz vs ywasm 214 KB / automerge 604 KB gz), not per-op boundary cost.
- **Serialization**: for large object graphs, one JSON string + `JSON.parse` often beats serde-wasm-bindgen field-by-field; binary formats (postcard/bincode) as a single `Uint8Array` are cheapest.
- **Don't wait for platform fixes**: component model not shipped in any browser (realistically 2027); stringref is dead; threads still need COOP/COEP — run the core in one Worker instead. Realistic core size: ~150–400 KB gz after `wasm-opt -Oz`.
- **Precedents**: Figma (C++), Photoshop web, typst.app. Sobering counterpoint: shipping browser text editors (CodeMirror/ProseMirror/Monaco) remain JS, and Lezer was built instead of tree-sitter-wasm because the wasm build is slower and heavier. WASM wins for compute-dense cores with coarse interfaces — which is exactly what a markdown core should be.

## 7. Full-Rust UI in 2026: honest assessment

**Verdict: pure-Rust rendering across desktop + mobile + web is not shippable at quality for a text-input-heavy app in mid-2026.** Desktop-only is proven (Zed). The blockers concentrate exactly where an editor hurts: IME composition, accessibility, mobile keyboards. Best independent evidence: the [2025 Survey of Rust GUI Libraries](https://www.boringcactus.com/2025/04/13/2025-survey-of-rust-gui-libraries.html) (hands-on Japanese IME + Windows Narrator tests).

Cross-cutting: **winit 0.31** (reworked IME API, mobile lifecycle) still in beta; Android soft-keyboard input still swallows characters ([#4067](https://github.com/rust-windowing/winit/issues/4067)). **AccessKit** has mature Windows/macOS/Linux adapters and a fast-moving Android adapter but **no iOS and no web adapter** ([accesskit.dev](https://accesskit.dev/)). Text *layout* is solved (Parley/cosmic-text); text *editing widgets* and IME plumbing are not.

| Framework | Verdict for a text-heavy cross-platform app |
|---|---|
| egui | No (IME bug tail, canvas web, no mobile) |
| Dioxus 0.7 | **Pragmatic winner if Rust shells wanted** — but text input is the platform WebView's; native renderer Blitz has no IME yet |
| GPUI 0.2.x | Desktop-only existence proof; essentially no a11y (Zed invisible to VoiceOver) |
| Floem | No |
| Slint 1.17 | Best pure-Rust pick if scope-reduced; text-input widgets still not exposed to a11y APIs ([#2895](https://github.com/slint-ui/slint/issues/2895)) |
| Makepad 1.0 | No (zero a11y), though it does ship real mobile apps |
| iced 0.14 | First-ever IME support Dec 2025; desktop-Linux credible, not cross-platform |
| Xilem 0.4 | 2027+ architecture to watch |

Loud absence of evidence: **no shipped text-heavy pure-Rust-rendered app exists on iOS/Android app stores at consumer quality**. Graphite deliberately keeps its UI in Svelte/DOM over a Rust core.

## Synthesis

1. **Architecture**: single Rust core crate (rope + lossless markdown CST + edit engine), compiled three ways: (a) native lib via uniffi proc-macros for Swift/Kotlin, (b) wasm-bindgen for web, (c) linked directly into any Rust desktop shell. Views are thin but get **synchronous reads**.
2. **Boundary protocol**: one call per edit in; batched patches/damage-lists out; document behind a handle; bulk text as bytes; never per-node/per-span crossings.
3. **Parsing**: pulldown-cmark spans + rope slicing gets to market; the rowan/cstree lossless incremental markdown CST (Typst/Lezer pattern) is the unbuilt piece — plan it as owned IP, not a dependency.
4. **UI**: platform-native text surfaces in 2026; re-evaluate pure-Rust rendering in 2027 when AccessKit iOS/web, winit 0.31, and Parley-everywhere land.
5. **Collaboration**: decide now, per Levien — if it's in, do it Zed-style (in-process, committed); if not, simple revision tracking and don't buy the complexity.

Key sources: [xi retrospective](https://raphlinus.github.io/xi/2020/06/27/xi-retrospective.html) · [Zed Decoded: Rope & SumTree](https://zed.dev/blog/zed-decoded-rope-sumtree) · [Zed CRDTs](https://zed.dev/blog/crdts) · [pulldown-cmark OffsetIter](https://docs.rs/pulldown-cmark/latest/pulldown_cmark/struct.OffsetIter.html) · [comrak releases](https://github.com/kivikakk/comrak/releases) · [uniffi design principles](https://mozilla.github.io/uniffi-rs/latest/internals/design_principles.html) · [Automerge 2.0](https://automerge.org/blog/automerge-2/) · [2025 Rust GUI survey](https://www.boringcactus.com/2025/04/13/2025-survey-of-rust-gui-libraries.html) · [accesskit.dev](https://accesskit.dev/)
