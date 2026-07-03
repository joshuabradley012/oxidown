# Prior Art: "One Editor Core, Many Platforms"

> Research compiled July 2026 for the Oxidown plan refresh. ~60 primary sources; load-bearing claims independently re-verified.

## Executive summary

1. **Nobody has shipped a shared *editor UI* across native mobile + web except via a web editor.** Every project that tried splitting "editor brain" from "platform text view" either abandoned the split (xi-editor, Lockbook v1), triple-implemented the editor (Anytype), double-implemented it (AppFlowy: Dart + React/Slate), or put the whole editor in a webview (Notion, Slite, Obsidian, Quip's JS editor).
2. **Sharing a *non-UI* core works and is production-proven** — for sync, data model, parsing, search: Quip (C++), PSPDFKit/Nutrient (C++ → WASM), 1Password (Rust), Firefox (Rust/UniFFI), AppFlowy (Rust), Anytype (Go). The costs that killed the failures (Dropbox, Slack Libslack) were *organizational*: bindings tooling, debugging across FFI, hiring.
3. **The editing surface itself resists the core/view split.** The two most direct datapoints — xi-editor's retrospective and Lockbook's pivot — both concluded that text input (IME, autocorrect, selection, composition) is too entangled with the document model to put an async or FFI boundary between them.
4. **IME is the universal editor-killer regardless of stack.** Zed (custom Rust rendering), appflowy_editor (Dart), super_editor (Dart), Compose-on-iOS — all carry multi-year open IME bug tails. Webview editors (ProseMirror/CodeMirror) are the only ones that *inherit* a decade of solved IME edge cases.
5. **Streaming AI text is solved for read-only rendering (repair-the-tail + block memoization), unsolved-but-converging for editable documents** — the industry answer is structured block operations + diff/suggestion review (BlockNote, Tiptap AI Toolkit, Cursor), not raw markdown append.
6. **The browser editor is the artifact that survives.** Paper → web-only (Oct 2025), Quip → retired (2027), Coda → absorbed; Notion's webview editor outlived its native shells' rewrites.

---

## 1. AppFlowy and Anytype

### AppFlowy: Rust core + Dart editor… and then a second React editor for web
- Flutter owns presentation; Rust owns infrastructure (event-dispatch over Dart FFI with protobuf). Stated rationale: hedge Flutter platform risk ([design post](https://appflowy.com/blog/tech-design-flutter-rust)).
- **Editor logic is Dart, not Rust** ([appflowy_editor](https://github.com/AppFlowy-IO/appflowy-editor), 98% Dart: Slate-inspired node tree + Quill Delta text, transactions, selection, rendering). Rust only stores/syncs.
- **The hedge got exercised — against Flutter Web**: [AppFlowy-Web](https://github.com/AppFlowy-IO/AppFlowy-Web) (launched Apr 2025) is React/TypeScript with a **second editor implementation in Slate + Yjs**. The Yjs-compatible CRDT document is the interchange layer.
- Pain (2025-2026, open): Korean IME broken, out-of-order characters, no composition underline, large-doc scroll perf, chasing Flutter releases.
- **Lesson:** the Rust/Dart seam held at "document data + commands," but the moment web mattered, the *editor* had to be built a second time.

### Anytype: Go middleware everywhere, editor UI built three times
- [anytype-heart](https://github.com/anyproto/anytype-heart) (Go) compiles via gomobile to Android AAR / iOS XCFramework (in-process) and runs as gRPC child process for Electron. **Editing is middleware RPC**: clients translate keystrokes into protobuf commands (`BlockCreate`, `BlockSplit`, `BlockTextSetText`, `BlockListTurnInto`…) and render event streams back.
- **Three editors**: TS/React (Electron), Swift from-scratch block editor, Kotlin RecyclerView block adapter. No browser app.
- Documented cost: chronic feature-parity lag (official per-platform feature matrix; community threads). Sustainable but expensive: four full client teams.
- **Lesson:** a command/event protobuf seam makes N native editors *possible*, not *cheap*. Partial win: native platform text views inside block holders dodge the from-scratch IME tail.

## 2. React Native

### Expensify react-native-live-markdown
- **The shared parser is JS (worklet), not C++** — the C++/ObjC++ layer is JSI/JNI glue invoking `parseExpensiMark` synchronously on the UI thread. Per-platform: ~12 custom Android Spannable spans; iOS NSAttributedString/TextKit 2 formatting; a fully separate web implementation with hand-rolled undo. The component's value is a plain markdown string — **decorated markdown source, not WYSIWYG** ([repo](https://github.com/Expensify/react-native-live-markdown)).
- Very active but tightly scoped: Fabric-only, per-RN-minor patch directories, 4,000-char parse cap, h1-only headings. Known cursor/paste bugs.
- **Software Mansion's successor family** ([enriched.swmansion.com](https://enriched.swmansion.com/)): react-native-enriched (fully native uncontrolled input) and react-native-enriched-markdown (Jan 2026; **md4c C parser** natively, WASM on web, native text rendering, no WebView). Their Filament case study states they *abandoned an initial WebView WYSIWYG approach* (slow to mount, inefficient scrolling/keyboard interactions). Across both generations: **parsing is shared; range/attribute application, cursor/selection, and undo are rebuilt per platform.**

### 10tap-editor (tentap)
Tiptap in a WebView + typed native↔web bridge; recommended in Expo's rich-text guide; apparently dormant since Nov 2025. Issue-tracker pattern: the ProseMirror editor inside the WebView is solid; nearly every pain point is the **keyboard/focus/scroll boundary**. Context: Lexical has no native RN story; Discord used "shared parser spec, per-platform span application." Expo's verdict: "There is no one-size-fits-all solution for rich text editing in React Native."

## 3. Incumbents

### Notion — the strongest success case for "one web editor, native everything-else"
Cordova → RN-as-webview-shell (removed ~2020) → incremental native rewrite around the editor: native Home tab, native Search, Compose + Baseline Profiles ([2x faster launch](https://www.notion.com/blog/notion-on-android-is-now-more-than-twice-as-fast-to-launch)). Per [Pragmatic Engineer (Dec 2024)](https://newsletter.pragmaticengineer.com/p/notion-going-native-on-ios-and-android): ~11 mobile engineers for 10M+ users; "most of Notion's apps are fully native, save for the editor." Made tolerable by data-layer engineering: SQLite on desktop, WASM SQLite + OPFS in browser.

### Craft — all-native's price is platform absence
One Swift codebase ~99% shared across Apple platforms, no SwiftUI/AutoLayout, custom canvas layout, ~4 engineers ([interview](https://newsletter.pragmaticengineer.com/p/design-first-software-engineering)). But: Windows took 3 years / "over 10,000 hours"; **Android reached beta only Nov 2025** — 5+ years after launch.

### Quip — the canonical shared-C++-core
Shared **C++ "Syncer"** (LevelDB, offline, protobuf) on desktop+mobile; and critically: "**All the document editors across all devices run on the same JavaScript libraries**" ([Bret Taylor](https://medium.com/@btaylor/react-with-c-building-the-quip-mac-and-windows-apps-c63155c1531b), [small-teams post](https://quip.com/blog/building-great-products-with-small-teams)). So Quip = shared C++ *sync* core + shared *JS editor* + native shells; 8 platforms, 13-14 engineers. Died for business reasons (Salesforce EOL, no renewals after Mar 2027).

### Dropbox Paper + the Dropbox C++ postmortem
Paper: web-first OT editor; mobile/desktop apps discontinued Oct 2025 — web-only. The canonical shared-code postmortem ([Guthmann 2019](https://dropbox.tech/mobile/the-not-so-hidden-cost-of-sharing-code-between-ios-and-android)): Dropbox abandoned its C++ mobile core — custom tooling, degraded debugging, platform differences, hiring. Slack's Libslack died the same way. Counterpoint: **PSPDFKit/Nutrient** is the standing success — C++ document core wrapped by native views everywhere + WASM in browsers — it works because the core is a rendering/document engine, **not UI and not text input**.

### Microsoft Loop / Slite / Coda / Linear
Fluid Framework 2 (SharedTree DDS) powers Loop/Whiteboard/Teams Live Share; Loop is web-tech on all platforms. Slite: Slate editor in a WebView inside RN. Coda: web-first to the end (acquired by Grammarly). Linear: web = ProseMirror + Yjs on a custom sync engine; mobile (Sep 2024) went **fully native Swift/Kotlin with a deliberately reduced editor** — chose to implement twice rather than webview.

## 4. Flutter super_editor / Superlist
Not 1.0 after 6 years (0.3.0-dev.52, June 2026); repo moved to Flutter Bounty Hunters (client-funded: Superlist, ClickUp…). They had to get text-editing deltas added to Flutter itself and built their own text layout. **The cost is IME, forever**: Windows IME "completely broken" open since Jan 2024; Japanese composition caret bugs in Mar 2026; active IME plumbing work six years in. The most serious attempt at "one editor implementation everywhere" outside the browser; pre-1.0, funding-fragile, permanent IME tail.

## 5. Zed / GPUI
Zed 1.0 April 29, 2026 (~1M lines of Rust); Windows GA Oct 2025; **no mobile, no stated plans** (community ports hit wasmtime/JIT-on-iOS blockers). Markdown is GPU-rendered (two internal renderers — known duplication). Cautionary evidence: every OS text-input path re-fought (Japanese IME 2024, Windows IME wave 2025); accessibility acknowledged as beyond-1.0. Custom rendering means zero free platform a11y/IME. The reusable part is the core design: [Rope & SumTree](https://zed.dev/blog/zed-decoded-rope-sumtree), [text coordinate systems](https://zed.dev/blog/zed-decoded-text-coordinate-systems), anchors.

## 6. Compose Multiplatform / KMP in 2026
iOS stable May 2025 (CMP 1.8); web (Kotlin/Wasm) beta Sept 2025; **1.11 (May 2026) added an experimental native UIView-based text input on iOS** — the fix and the confession: Skia-rendered text input on iOS never fully matched native IME/selection. Google backs KMP for shared logic (Google Docs iOS runs KMP in production). No ProseMirror-class rich text library exists (compose-rich-editor still RC). Verdict: could replace Rust for *logic*, but large editable text surfaces on iOS are exactly CMP's least-mature area, and web locks to beta Kotlin/Wasm.

## 7. Tauri v2 mobile in 2026
2.0 stable Oct 2024 with iOS/Android; 2.11.5 July 2026; forward bet is the experimental Servo/Verso runtime. Rust core in-process is Tauri's native model. Mobile works with recipes, not blockers (WKWebView keyboard/safe-area workarounds documented; push/IAP community-plugin-only). No marquee >1M-install Tauri mobile app found. **The existence proof for the webview-editor architecture is Obsidian (Capacitor)**: same CM6 live-preview editor on desktop + mobile, chosen because CM6 is one of the only editors that works decently on mobile — inheriting ProseMirror/CM6's decade of Safari/Android IME workarounds instead of re-fighting them.

## 8. AI-native streaming (2025-2026)

**Read-only rendering is a solved commodity:**
- **[Streamdown](https://github.com/vercel/streamdown)** (Vercel, Aug 2025; powers AI SDK Elements): block-level memoization + tail repair via **[remend](https://vercel.com/changelog/new-npm-package-for-automatic-recovery-of-broken-streaming-markdown)** ("self-healing markdown": close unclosed `**`, `[text](`, fences before parsing). Known open failure: fenced code blocks buffer-and-dump ([#473](https://github.com/vercel/streamdown/issues/473)).
- **llm-ui**: dormant, but its idea survives: deliberate **throttling/lookahead** — lag rendering behind the token stream so the parser has enough context — as the alternative to repair.
- **[thetarnav/streaming-markdown](https://github.com/thetarnav/streaming-markdown)**: true optimistic append-only streaming parser (never mutates rendered DOM), endorsed by [Chrome's render-LLM-responses guidance](https://developer.chrome.com/docs/ai/render-llm-responses) (which also warns per-chunk full-buffer innerHTML sanitizing is an XSS trap). The fundamental tension: buffering trades latency for correctness; repair trades correctness for latency.
- **No incremental CommonMark parser exists in Rust** — a genuine gap/opportunity.

**Streaming into *editable* documents converged on structured operations + diff review, not raw append:**
- **Tiptap** `streamContent` + AI Toolkit beta (Oct 2025): schema-aware stream tools, markdown-vs-doc diffing, accept/reject review UI.
- **BlockNote AI**: the LLM emits **`add`/`update`/`delete` block operations** streaming into the document with a "user-reviewing" accept/reject state — the clearest public implementation.
- **Cursor** streams **diffs, not markdown**: full-file speculative rewrite at ~1000 tok/s ([instant apply](https://cursor.com/blog/instant-apply)) — rewrite-then-diff beat surgical patches.
- ChatGPT's composer is ProseMirror; canvas is CodeMirror 6.
- **Design takeaway:** make "AI stream" a first-class *transaction source* in the core — structured ops with deferred undo-grouping and a review/accept state — rather than text appended through the keyboard path.

## 9. "Rust core + native views" for text editing specifically

- **xi-editor is the canonical failure** ([retrospective](https://raphlinus.github.io/xi/2020/06/27/xi-retrospective.html)): async core/view protocols created races; scrolling took months; CRDT added complexity without payoff; JSON serialization "shockingly slow" in Swift. The xi lineage (Lapce + Floem) rebuilt as in-process all-Rust.
- **Lockbook is the one team that shipped exactly this architecture — and pivoted** ([Parth Mehrotra, "Lockbook's Editor"](https://parthmehrotra.substack.com/p/lockbooks-editor); repo active 2026): started with Rust core + native editor views (Markwon on Android, TextKit on Apple). It failed — TextKit slow on large docs, missing affordances, "we would have to replicate our efforts on our other platforms." They replaced the native editors with **one Rust/egui editor rendered via wgpu, embedded in native shells**; they implement **UITextInput / InputMethodManager per platform** so autocorrect/dictation/IME still work. "Easily achieving 120fps on massive documents." Shared: all editor logic + rendering. Per-platform: input-method bridges and GPU surface plumbing.
- **The pattern works when the core is a state machine, not a live surface:** [Ferrostar](https://stadiamaps.com/blog/ferrostar-building-a-cross-platform-navigation-sdk-in-rust-part-1/) (Rust nav core → UniFFI → SwiftUI/Compose views) with stated warnings: no mutable references across FFI (interior mutability shapes the core API), UniFFI breaking changes, packaging pain. 1Password validates Rust core + thin native UIs for app logic at scale (their own Typeshare, not UniFFI). [UniFFI for React Native turbo modules](https://hacks.mozilla.org/2024/12/introducing-uniffi-for-react-native-rust-powered-turbo-modules/) (Dec 2024) exists.
- **Nobody drives keystroke-level text editing through UniFFI.** Searched; none found. Given per-call FFI overhead and the xi lesson, this absence looks structural — the right split is: native buffer owns keystrokes; Rust core is the synchronous authority reconciled per-edit, not per-keystroke-event.

## 10. Synthesis

**Where the seam holds (put in the Rust core):** document model + block semantics, markdown parse/serialize, sync, search, undo model, AI-stream transaction application. Every successful shared core sits behind a **command/event API** at this altitude — Anytype's protobuf surface is a working reference design.

**Where the seam breaks (don't split it):** the live editing surface — keystroke→composition→selection→layout. The three working resolutions:
1. **Web editor in webviews everywhere** (Notion, Obsidian, Slite, Quip; Tauri v2 makes this a Rust-core-in-process story) — inherits solved IME; costs native feel at the keyboard/scroll boundary.
2. **N native editors over a shared command core** (Anytype, Linear mobile) — best feel, dodges from-scratch IME via platform text views, permanent feature-parity tax.
3. **One custom-rendered editor embedded in native shells** (Lockbook Rust/wgpu; Zed desktop) — one implementation, real 120fps, but you own IME/accessibility forever; no one has taken this to all four platforms plus web.

**Streaming:** treat AI insertion as structured operations with diff/review state in the core; tail-repair or lookahead-throttling in the renderer; the incremental Rust markdown parser is genuinely novel infrastructure worth owning.
