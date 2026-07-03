# Reference Landscape

Annotated map of prior art for Oxidown, refreshed July 2026. Full findings live in [research/](research/). Items marked **★** are load-bearing references for our architecture; items marked *(new)* were not on the original list.

## Hybrid live-preview editors (markdown text = source of truth) — our lane

- **★ Obsidian Live Preview** — the reference implementation: CM6 decorations over Lezer syntax tree, syntax reveal on selection intersection, widget islands for tables (shipped 1.5 after 4 years). ~1M users on this model. Known structural gaps: footnotes (document-global vs viewport-local), Android polish.
- **★ Bear 2 / Panda / Lettera** *(new: Lettera, June 2026)* — Shiny Frog's custom native hybrid engine, now a standalone macOS markdown file editor. Proof of the model's product value and of its cost (~4 years, Apple-only).
- **★ CodeMirror 6 + lezer-markdown** — the substrate: block-incremental hand-written parser emitting cheap packed trees, viewport-only decoration, mapped (not recomputed) RangeSets, composition-safe update discipline. Our web view builds on this.
- **Atomic Editor** *(new, 2026)* — Obsidian-style live preview as a reusable CM6 npm package; "no layout shift" design goal; the reveal playbook commoditized. Also **codemirror-live-markdown** *(new)* with a detailed public design doc.
- **iA Writer** — the purist pole (syntax styled, never hidden). Killed its Android app in 2023 ("asteroid field") — a warning about Android costs.
- **Typora** — WYSIWYG-over-markdown boundary case; the canonical case study of the round-trip normalization tax (table prettify/list renumber complaints → Strict Mode).
- **marktext** — abandoned 2022; bespoke DOM hybrid engines without institutional backing rot.
- **Zettlr 3** (rebuilt on CM6), **Inkdrop** (CM6 + lezer-markdown fork), **Vditor/Lute/SiYuan**, **Toast UI / ToastMark** (custom parser built specifically for source-position sync — validating our parser priorities).
- **render-markdown.nvim / markview.nvim** *(new)* — hybrid live preview inside Neovim via tree-sitter-markdown; the model is substrate-independent.
- **OverType** *(new, 2025)* — transparent textarea over rendered preview; markdown IS the document; IME/undo 100% native browser. Degenerate but instructive.

## Web WYSIWYG frameworks (AST = source of truth) — study, don't adopt

- **★ ProseMirror** — moved off GitHub to self-hosted Forgejo (Apr 2026), healthy. Steal: Steps/StepMaps/Mapping (invertible ops + position mapping), collab-aware undo (inverted steps rebased over remote changes), "Addressing Editor Content" (offsets vs stable IDs). Its view layer absorbed a decade of IME workarounds — the cost we're routing around.
- **★ Wordgard** *(new, July 2026)* — Marijn's next-gen library; nine years of ProseMirror lessons distilled: beforeinput-first, delta-style changes instead of Step classes, relaxed schema + programmatic correction, selective undo. Required reading for our op-log design.
- **★ Lexical** — 0.46, still pre-1.0; powers Meta's apps. Steal: flat keyed NodeMap + copy-on-write + double-buffered state + dirty-key reconciliation (the most Rust-portable design). **lexical-ios** proves the reconciler pattern works on TextKit (but is dormant OSS — reference, not dependency).
- **Tiptap v3** *(stable July 2025)* — official bidirectional markdown only since Oct 2025, "early"; free cloud tier killed. Lesson: retrofitting markdown onto an HTML-first schema 8 years in is expensive.
- **Milkdown** — best blueprint for an mdast ↔ editor-tree mapping layer. Runtime truth is still the PM doc; round-trip normalizes.
- **MDXEditor** — closest analog to a markdown-native product on the WYSIWYG side (markdown is the contract, Lexical is runtime truth, output is normalized).
- **BlockNote** — `blocksToMarkdownLossy()` API honesty worth copying; **BlockNote AI** streams add/update/delete block ops with accept/reject review — reference for our streaming design.
- **Plate** (Slate abstracted; MDX round-trip for custom nodes), **Slate** (stewardship mode; historically worst Android IME), **remirror → ProseKit** *(new)* (headless + thin adapters wins; monoliths die), **Editor.js / Quill 2** (stagnant/dormant — drop from consideration), **novel** (abandoned Jan 2025).

## Rust ecosystem

- **★ ropey** — our text store (1.6 now; 2.0 goes byte-indexed, in beta). crop (faster, slow trickle), jumprope (dormant, no cheap clones), lapce-xi-rope. Zed's SumTree rope is the best design study (GPL, unpublished). Cheap COW snapshots are the load-bearing feature.
- **★ pulldown-cmark** — phase-A parser: byte-exact spans via `into_offset_iter()`, GFM options, what Zed and Helix both use. Not incremental, not lossless-serializing — we slice the rope by span instead of serializing.
- **comrak** — spec-complete (652/652 CommonMark + 670 GFM), sourcepos overhauled late 2025, ~7x slower; our conformance oracle in tests.
- **markdown-rs** — token-level positions but dormant since 1.0. **tree-sitter-markdown** — the only incremental option but self-admittedly spec-inaccurate; highlight-grade only. **jotdown** — Djot, not markdown.
- **★ The gap**: no Rust parser has byte-spans + incremental + lossless CST together. Lezer-markdown (JS) and typst-syntax are the proven designs to port. This is Oxidown's core IP opportunity.
- **★ xi-editor retrospective** — the canonical negative result: never put an async/process boundary between core and view; CRDT-by-default was over-engineering. Its successors (Zed, Lapce) are all in-process.
- **Zed** (SumTree, anchors, in-process CRDT, DeltaDB; 1.0 Apr 2026; no mobile), **Lapce/Floem** (slowing), **Helix** (ropey + tree-house).
- **uniffi** — proc-macro mode, ~1-4µs/call (fine per-edit, fatal per-span); Kotlin rides slow JNA — batch accordingly. **BoltFFI** *(new)* — zero-copy challenger; crux migrated to it. **wasm-bindgen** — strings copy+transcode; one coarse call per edit, patches out (Automerge's `popPatches` pattern).
- **Rust GUI (egui/GPUI/Slint/Dioxus/Makepad/iced/Xilem)** — not shippable for text-heavy mobile in 2026 (AccessKit has no iOS/web adapter; winit mobile IME in beta). Revisit 2027.
- **Typst** — the premier Rust-core + incremental-parse + WASM precedent (typst.app recompiles per keystroke).

## Native platform stacks

- **★ iOS: TextKit 2 + NSTextStorage mirror** — the realistic editing surface; WWDC 2026 added viewport/attachment-reuse APIs aimed exactly at gutters/code-block chrome/inline images. Custom NSTextContentManager is a documented dead end; STTextView is the best open reference (and its author's honest critique of TextKit 2 is required reading).
- **iOS 26 SwiftUI TextEditor + AttributedString** *(new, WWDC 2025)* — real, inline-only (no attachments/blocks/custom layout); prototype surface, not the flagship editor.
- **★ Android: state-based BasicTextField + OutputTransformation** — synchronous `TextFieldState`, display-only styling with automatic offset mapping (Aug 2025 added `addStyle`); inline images unsupported → block layout for embeds. The composing region is sacred; Gboard/Samsung/SwiftKey each break the contract differently.
- **Markwon** (render-only standard, View-based), **Markor** (custom highlighter over EditText), **compose-rich-editor** (CMP, still RC).
- **Down / Ink / swift-markdown-ui** — superseded for us (parsing lives in the Rust core); **Textual** *(new)* — MarkdownUI successor, rendering only.

## Cross-platform prior art (one core, many platforms)

- **★ Lockbook** *(new)* — the one team that shipped "Rust core + native editor views" and pivoted: now one Rust/egui/wgpu editor embedded in native shells, implementing UITextInput/InputMethodManager per platform. The honest bookend to xi.
- **★ Anytype** — Go core in-process on mobile, editing as protobuf commands (`BlockSplit`, `BlockTextSetText`), three hand-built native editors; works, costs a team per platform (chronic feature-parity lag).
- **AppFlowy** — Rust core, but editor logic is Dart; when web mattered they built a second editor in Slate. The seam held at "data + commands," not the editing surface.
- **Expensify react-native-live-markdown** — decorated-markdown-source in RN; shared parser, per-platform span application + separate web implementation. Successor: **react-native-enriched-markdown** *(new, Software Mansion 2026)* — md4c natively, WASM on web, abandoned WebView approach.
- **Quip** — shared C++ sync core + shared JS editor + native shells; 8 platforms, ~14 engineers; retired 2027 for business reasons.
- **Notion** — native everything *except* the editor (still webview, ~11 mobile engineers, 10M+ users). **Craft** — all-native Apple; Windows took 3 years, Android beta only Nov 2025. **Linear** — mobile went native with a deliberately reduced editor.
- **Dropbox C++ postmortem / Slack Libslack** — shared-core failures were organizational (tooling, debugging, hiring). **PSPDFKit/Nutrient** — the standing success: shared core is a document engine, not UI/text-input.
- **super_editor / Superlist** — 6 years, pre-1.0, permanent IME tail; the cautionary tale for "one editor implementation everywhere" outside the browser.
- **Tauri v2** — mobile stable-ish; the pragmatic Rust-core-in-process + web-editor path (Obsidian proves the shape on Capacitor).
- **Compose Multiplatform** — iOS stable 2025, but 1.11's experimental *native* text input is the confession that Skia text input on iOS wasn't good enough.

## Collaboration (future) — design for, don't build

- **★ eg-walker** (Gentle & Kleppmann, EuroSys 2025) — event graph of untransformed ops + transient CRDT state only under concurrency. Adopted by Figma Code Layers (2025); same family as Zed DeltaDB (2026); Loro built on it. **A well-designed single-user op log is already 80% of an eg-walker document.**
- **★ Peritext** (Ink & Switch) — rich-text formatting as marks with before/after anchors + expand rules; implemented by Automerge 2.2+ and Loro. Also documents exactly why raw-markdown-source merging produces intent failures (`**The **fox** jumped.**`).
- **Loro** (1.13, Peritext + eg-walker + movable trees, Rust-native, young ecosystem), **Automerge 3** (2025: >10x memory cut; marks + block markers), **Yjs/yrs** (most battle-tested; v14 adds attributed changesets; not Peritext), **cola / diamond-types** (reference designs, not dependencies).
- **Production split 2026**: server-ordered log + rebasing dominates shipped products (Google Docs, Linear, tldraw, ProseMirror/CM collab); CRDT owns local-first (Zed, Anytype, Obsidian Relay, Notion offline).

## Streaming AI text (first-class for us)

- **★ Streamdown + remend** (Vercel, 2025) — block memoization + self-healing tail repair; read-only. Known failure: streaming into fenced code blocks.
- **★ BlockNote AI / Tiptap AI Toolkit** — streaming into *editable* docs = structured ops + accept/reject review, not raw append.
- **Cursor instant-apply** — rewrite-then-diff at ~1000 tok/s beats surgical patches for edits to existing content.
- **thetarnav/streaming-markdown** — optimistic append-only streaming parser (never mutates rendered DOM); endorsed by Chrome's LLM-rendering guidance.
- **Gap**: no incremental CommonMark parser exists in Rust — our streaming fast-path and our incremental parser are the same investment.

## Required reading

- xi-editor retrospective — https://raphlinus.github.io/xi/2020/06/27/xi-retrospective.html
- Lockbook's editor pivot — https://parthmehrotra.substack.com/p/lockbooks-editor
- Wordgard 0.1 announcement — https://marijnhaverbeke.nl/blog/wordgard-0.1.html
- Addressing Editor Content (positions vs IDs) — https://marijnhaverbeke.nl/blog/addressing-editor-content.html
- Contenteditable on Android is the Absolute Worst — https://discuss.prosemirror.net/t/contenteditable-on-android-is-the-absolute-worst/3810
- Peritext essay — https://www.inkandswitch.com/peritext/
- Eg-walker paper — https://arxiv.org/abs/2409.14252
- Zed Decoded: Rope & SumTree — https://zed.dev/blog/zed-decoded-rope-sumtree
- Figma: Building Code Layers — https://www.figma.com/blog/building-figmas-code-layers/
- Text Editing Hates You Too — https://lord.io/text-editing-hates-you-too/
- TextKit 2: The Promised Land — https://blog.krzyzanowskim.com/2025/08/14/textkit-2-the-promised-land/
- How Figma's multiplayer works — https://www.figma.com/blog/how-figmas-multiplayer-technology-works/
- lezer-markdown (the parser design to port) — https://github.com/lezer-parser/markdown
- codemirror-live-markdown design doc — https://github.com/blueberrycongee/codemirror-live-markdown/blob/main/CODEMIRROR_LIVE_PREVIEW_DESIGN.md
