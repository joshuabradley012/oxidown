# Web Rich-Text / WYSIWYG Editor Frameworks: State of the Art

> Research compiled July 2026 for the Oxidown plan refresh. Version dates are npm-registry publish dates unless noted.

---

## 0. Headline findings (what changed in 2025–2026)

1. **ProseMirror left GitHub.** In April 2026 Marijn Haverbeke archived the GitHub repos and moved all ProseMirror development to his self-hosted Forgejo at [code.haverbeke.berlin](https://code.haverbeke.berlin/prosemirror/prosemirror). npm publishing continues normally; the project is healthy (view 1.42.0 on July 1, 2026).
2. **Marijn released "Wordgard 0.1" on July 2, 2026** — an explicitly-not-ProseMirror-2.0 rich text library encoding nine years of ProseMirror lessons: beforeinput-first input handling, delta-style changes instead of Step classes, relaxed schemas with programmatic correction, selective undo ([announcement](https://marijnhaverbeke.nl/blog/wordgard-0.1.html)). The single best "lessons learned" document for a new editor core.
3. **Lexical still has not reached 1.0** — latest is **0.46.0 (June 26, 2026)** with monthly minors; Meta investment is strong (new Extension API, DOM render/import extensions, isolated editable regions) ([releases](https://github.com/facebook/lexical/releases)).
4. **Tiptap v3 went stable mid-July 2025** (3.27.1 by June 2026) and shipped **official bidirectional markdown** (`@tiptap/markdown`, Oct 14, 2025, MarkedJS-based). It simultaneously killed its free cloud tier and moved to document-based pricing, while open-sourcing 10 formerly-Pro extensions and an MIT UI-components library.
5. **Remirror is officially in maintenance mode**, pointing new projects to **ProseKit**. Quill is effectively dormant (no release since Nov 2024); Editor.js is patch-only with collaboration still unshipped; Slate is in stewardship mode, consumed almost entirely via Plate.
6. **No new "Rust-core web rich-text editor framework" emerged**; the flagship attempt (matrix-rich-text-editor) was archived at matrix-org in Sept 2024 and continues only at maintenance level under element-hq.
7. **EditContext API** (input decoupled from contentEditable) shipped in Chromium 121 (Jan 2024), is a W3C Working Draft (June 2026), got a Firefox Intent-to-Prototype in May 2026, has no Safari signal — and its first real adopters are **code** editors (VS Code 1.101 default-on May 2025; CodeMirror 6.28+), not rich-text frameworks ([spec](https://www.w3.org/TR/edit-context/), [caniuse](https://caniuse.com/mdn-api_editcontext)).

---

## 1. Why "re-render innerHTML on every keystroke" fails

A naive editor keeps a model, and on each keystroke sets `innerHTML = render(model)`. This fails for at least six concrete reasons:

1. **Selection destruction.** `window.getSelection()` anchors to *specific DOM text nodes + offsets*. Re-rendering destroys those nodes; the caret collapses or jumps. Restoring selection after every render is itself unreliable — CKEditor calls the native Selection API "broken" ([ContentEditable — The Good, the Bad and the Ugly](https://ckeditor.com/blog/ContentEditable-The-Good-the-Bad-and-the-Ugly/)).
2. **IME composition breakage.** During composition (CJK IMEs, Android soft keyboards, dead keys, dictation) the IME holds references into the DOM text node being composed. Chrome: "Making changes to the area of the DOM being edited while there's an active IME composition can cause the composition to be prematurely canceled" ([Chrome EditContext blog](https://developer.chrome.com/blog/introducing-editcontext-api)). Premature commit/cancel is the signature cause of doubled/lost text on Android GBoard (e.g. Slate [#5019](https://github.com/ianstormtaylor/slate/issues/5019), the multi-year [#2062 saga](https://github.com/ianstormtaylor/slate/issues/2062)).
3. **Performance:** O(document) DOM rebuild + full layout/paint per keystroke instead of O(edit).
4. **Accessibility:** screen readers track caret and live regions via the accessibility tree; wholesale subtree replacement makes AT re-announce or lose context.
5. **Spellcheck state** attaches to text nodes and resets on replacement.
6. **Focus and scroll anchoring** reset when the focused subtree is replaced.

Foundational reading: Nick Santos, ["Why ContentEditable is Terrible"](https://medium.engineering/why-contenteditable-is-terrible-122d8a40e480).

### How ProseMirror reconciles (DOM-read-back bias)
ProseMirror treats the DOM as a **projection** of an immutable document ([guide](https://prosemirror.net/docs/guide/)): a **ViewDesc tree** maps DOM ↔ document positions; on state update it diffs old vs new doc and "leaves the parts of the DOM that correspond to unchanged nodes alone." **Typing is deliberately left to the browser**; a **DOMObserver** (MutationObserver + selectionchange) detects what the browser did, re-parses *just that DOM region*, diffs against the doc, and dispatches a transaction. `EditorView.composing` gates redraws; the view avoids touching the composed region until `compositionend`. IME fixes still land monthly in 2026 ([changelog](https://prosemirror.net/docs/changelog/)).

### How Lexical reconciles (model-first bias)
Lexical intercepts **beforeinput** and applies most edits to the model first, then diffs model → DOM: **double-buffered EditorState** (frozen current + mutable pending, commit swaps); **flat NodeMap** (`Map<NodeKey, LexicalNode>`) with copy-on-write cloning of dirty nodes; the reconciler walks only dirty subtrees and applies keyed DOM mutations. **Composition is explicitly not intercepted** ("we let the browser do its own thing"); a `compositionKey` excludes the composing node from transforms until composition ends. During reconciliation Lexical **disconnects the MutationObserver, applies its writes, restores selection from the model, then re-observes**. The source is a museum of platform heuristics (`ANDROID_COMPOSITION_LATENCY = 30ms`, keyCode 229 detection, an iOS Korean 10-key IME that fires no composition events at all). ([editor-state docs](https://lexical.dev/docs/concepts/editor-state), [Dani Guardiola's deep dive](https://dio.la/article/lexical-state-updates))

### The general lesson (the "authority flip")
Every serious editor converges on the same protocol:

> The DOM (or native text view) is a **projection** of the model. Input events are **intents** interpreted against the model. **During IME composition, authority temporarily inverts** — the model must wait for and follow the DOM/native input session rather than drive it, then re-assert authority at composition end.

Marijn's 2026 verdict in Wordgard: **beforeinput-first is now viable** ("just handles beforeinput events for everything except composition text input… avoids a whole class of messy workarounds"), while Android still requires the composition/read-back path.

### Android specifically
Marijn's ["Contenteditable on Android is the Absolute Worst"](https://discuss.prosemirror.net/t/contenteditable-on-android-is-the-absolute-worst/3810) is the canonical reference: virtual keyboards deliver everything as composition with empty `key`/`keyCode`, beforeinput is spec-cancelable but "browsers often don't care enough about the spec," and each keyboard (GBoard, Samsung, SwiftKey) has distinct bugs. Documented workarounds: synthetic key events inferred from mutations, **"cursor parking"**, inputType inference. Fixes were still landing in ProseMirror in 2025–2026.

### EditContext API (mid-2026 status)
[EditContext](https://www.w3.org/TR/edit-context/) decouples text-input services from the DOM. Chromium: shipped Jan 2024 (~69% global support). Firefox: Intent to Prototype May 2026. Safari: no signal. Adopters: VS Code default-on since 1.101; CodeMirror 6.28+ (Chrome-only). **No adoption in ProseMirror, Lexical, or CKEditor** — rich-text frameworks still need contentEditable's free caret, selection UI, and a11y tree. Treat as a 2027 progressive enhancement, not a foundation. Related: Google Docs went full canvas (2021) with a parallel hidden a11y DOM — viable only with Google-scale investment.

---

## 2. Framework profiles

### ProseMirror
- **Model:** persistent immutable tree governed by a **schema**; **inline content is flat** with marks as metadata sets; **flat integer positions** by token counting. Markdown is a serialization, not truth. Marijn's ["Addressing Editor Content"](https://marijnhaverbeke.nl/blog/addressing-editor-content.html) (Sept 2025) defends integer offsets + change maps over stable-ID schemes (tombstone bloat) — directly relevant to a Rust core's addressing design.
- **Change model:** serializable, invertible **Steps** each yielding a **StepMap**; **Mapping** composes maps with bias control; rebasing machinery underlies collab, history, decorations.
- **Undo:** `prosemirror-history` — **inverted steps + position maps**; remote changes contribute map-only items; a `rebased()` hook re-inverts stored items; 500ms event grouping. The reference design for **collab-aware undo**.
- **Collab:** `prosemirror-collab` = central-authority rebasing — proven at the NYT. CRDT route: y-prosemirror (wart: remote Yjs edits arrive as whole-doc-replacing transactions, [#113](https://github.com/yjs/y-prosemirror/issues/113)); automerge-prosemirror and loro-prosemirror both exist but are 0.x/beta.
- **Markdown round-trip:** `prosemirror-markdown` parses via markdown-it, serializes via hand-written CommonMark serializer. **Normalizing, not byte-faithful.**
- **Health 2026:** excellent; ~80% of changes are browser/IME workarounds. Single maintainer, sponsor-funded; **OpenAI became a sponsor** (ChatGPT Canvas is ProseMirror/CodeMirror). Used by NYT, Atlassian, GitLab (via Tiptap), The Guardian.
- **Key lesson:** the Step/StepMap/rebase layer is pure data-structure logic and ideal to port; the view layer is where a decade of maintenance actually went.

### Lexical (Meta)
- **Status:** v0.46.0 (June 2026), still pre-1.0 after 4+ years. Powers Facebook, Workplace, Messenger, WhatsApp, Instagram web text editing. Adopters: Payload CMS default editor; 37signals' **Lexxy** (Oct 2025) replaced Trix in Rails Action Text.
- **Model:** EditorState = flat **NodeMap** + selection; double-buffered; copy-on-write via `getWritable()`; JSON-serializable.
- **Undo:** `@lexical/history` = **EditorState snapshot references** (cheap via structural sharing) — **incompatible with collab** (delegates to Yjs UndoManager; the seam has real bugs, [#6614](https://github.com/facebook/lexical/issues/6614)).
- **@lexical/markdown:** transformer-based, lossy by design; escaping round-trip bugs still being fixed in 2026.
- **lexical-ios:** alive but low-intensity; "pre-release with no guarantee of support"; community fork actively diverging. **No Meta lexical-android exists.** Architecturally it proves the reconciler pattern ports off the DOM (same state-tree diffed onto a TextKit backing store).
- **Key lesson for a Rust core:** Lexical's design is the most Rust-portable — keys instead of pointers (no ownership cycles), copy-on-write (`Arc::make_mut`), double-buffered commit protocol, dirty-key diffing with a `key → platform node` map per view adapter.

### Tiptap
- v3 stable July 2025; ProseMirror JSON under a schema/extension/command DSL. Official `@tiptap/markdown` since Oct 2025 (MarkedJS-based, "early release… may have edge cases") — illustrates the cost of adding markdown 8 years into an HTML-first schema. Free cloud tier eliminated June 2025, document-based pricing; core MIT. Strongest commercial momentum (~12.5M downloads/week for `@tiptap/core`; vendor-claimed users include LinkedIn, GitLab, Axios, Anthropic).

### Milkdown
- Plugin-driven WYSIWYG **markdown framework on ProseMirror + remark**: markdown → mdast → PM doc; serialization reverses through remark-stringify. **At runtime the ProseMirror doc is source of truth** — round-trip is normalizing, same fidelity class as prosemirror-markdown. v7.21.2 (June 2026), healthy but bus-factor ~1. **Crepe** = batteries-included editor. Key lesson: the best existing blueprint for a **markdown-schema mapping layer** (mdast ↔ editor tree with per-node spec pairs).

### BlockNote
- Notion-style **typed block tree** on Tiptap/ProseMirror, React UI included; v0.51.4 (June 2026); maintainers are significant Yjs contributors. **Markdown deliberately second-class and honest:** `blocksToMarkdownLossy()` — a good API-honesty pattern. Core MPL-2.0; XL packages (AI, export) dual GPL/commercial.

### Plate
- `platejs` 53.x (June 2026), still **built on Slate** but abstracts it; shadcn-compatible component registry; heavy AI investment. **`@platejs/markdown`** on unified/remark + remark-mdx — custom elements serialize as **MDX/JSX and parse back**, the strongest custom-node round-trip story in the cohort. Caveat: extremely fast breaking-major cadence; inherits Slate's weaker Android/IME layer.

### Slate
- Alive but slow (0.124/0.125, 2026); volunteer-driven; "Currently in beta" after ~9 years. Model: JSON tree; ~9 invertible **operations**. Historically worst-in-class Android (full input rewrite to beforeinput in [PR #4988](https://github.com/ianstormtaylor/slate/pull/4988)). Adopt only via Plate.

### remirror → ProseKit
- remirror officially in maintenance mode; README points to **ProseKit** ([prosekit.dev](https://prosekit.dev/)) — framework-agnostic headless PM toolkit by ocavue; React/Vue/Svelte/Solid/Preact adapters; pluggable **Yjs or Loro** collab; v0.21.4 (June 2026), still 0.x. Lesson: headless core + thin adapters wins; monolithic React-first wrappers die.

### MDXEditor
- The only mainstream editor whose **external contract is markdown**: markdown in → mdast → **Lexical** state (authoritative during editing) → serialized back. v4.0.4 (June 2026), ~973k downloads/week, MIT. Round-trip: normalized, not byte-identical. Closest existing analog to a markdown-native product.

### Editor.js / Quill v2
- Editor.js: JSON block output, patch-only cadence, collaboration promised for years and never shipped. Quill 2: **Delta** flat op-list (trivially diffable/invertible, OT-friendly; hierarchy ceiling — tables never first-class); v2.0.3 Nov 2024, **19+ months without a release** — dormant despite 3.8M downloads/week.

---

## 3. New/notable entrants a late-2025 list would miss

| Project | What it is | Status (July 2026) |
|---|---|---|
| **Wordgard** ([announcement](https://marijnhaverbeke.nl/blog/wordgard-0.1.html)) | Marijn's from-scratch successor-adjacent library: beforeinput-first, delta-style token changes with limited OT and **selective undo**, relaxed schema + programmatic correction, CM6-style facet extensions | v0.1, July 2, 2026; experimental "for at least a year" |
| **ProseKit** ([prosekit.dev](https://prosekit.dev/)) | Framework-agnostic headless PM toolkit; remirror's anointed successor | v0.21.4, very active, 0.x |
| **OverType** ([overtype.dev](https://overtype.dev/), [HN](https://news.ycombinator.com/item?id=44932651)) | Transparent `<textarea>` overlaid pixel-perfectly on a rendered markdown preview; **markdown IS the document**; undo/IME/Android are 100% native browser | v2.4.0 (Jun 2026), 3.7k stars, MIT. Constraint: monospace only. Architecturally provocative: sidesteps contentEditable entirely |
| **BlockSuite** | AFFiNE's CRDT-native (Yjs) block document framework | npm frozen at 0.22.4 since Jul 2025 — development AFFiNE-internal. Watch, don't build on |
| **Lexxy** (37signals) | Rails Action Text editor on Lexical | Shipped Oct 2025 |
| **Novel**, **Edra** | Tiptap-based Notion-style starters | Novel effectively abandoned (Jan 2025); Edra quiet |

Also: WordPress/Gutenberg **cut real-time collaboration from WordPress 7.0**.

---

## 4. At-a-glance comparison

| Framework | Doc model | Markdown is… | Input strategy | Undo model | Health 7/2026 | License |
|---|---|---|---|---|---|---|
| ProseMirror | Immutable tree + schema, flat marks, integer positions | Serialization (normalizing) | DOM-read-back (MutationObserver + re-parse), composition-gated | Inverted Steps + maps, collab-aware | Excellent | MIT |
| Lexical | Flat NodeMap, double-buffered EditorState | Serialization (transformer-based, lossy) | beforeinput model-first; composition left to browser | State snapshots; Yjs UndoManager in collab | Excellent; still 0.x | MIT |
| Tiptap v3 | ProseMirror JSON | Serialization (MarkedJS, "early") | Inherits PM | Inherits PM | Excellent; commercial | MIT core + paid cloud |
| Milkdown | PM doc via remark/mdast bridge | Interchange format (runtime truth = PM doc) | Inherits PM | Inherits PM | Good; bus-factor ~1 | MIT |
| BlockNote | Typed block tree on Tiptap/PM | Explicitly lossy export | Inherits PM | Inherits PM / Yjs | Good | MPL-2.0 + dual XL |
| Plate | Slate JSON tree, abstracted | remark+MDX round-trip (best custom-node story) | Inherits Slate (weakest Android) | slate-history inversion | Very active, fast-breaking | MIT + Pro |
| Slate | JSON tree, 9 invertible ops | N/A (community) | beforeinput after Android rewrite | Operation inversion | Stewardship | MIT |
| MDXEditor | Lexical state, markdown contract | **The contract** (normalizing) | Inherits Lexical | Inherits Lexical | Good | MIT |
| Quill 2 | Delta op-list + Parchment | Not native | Own event layer | Delta invert | **Dormant** | BSD-3 |
| OverType | Raw markdown in a textarea | **The document** | Native textarea | Native browser | New, active | MIT |

---

## 5. Rust-core prior art highlights

- **xi-editor** ([retrospective](https://raphlinus.github.io/xi/2020/06/27/xi-retrospective.html)): "the process separation between front-end and core was not a good idea"; async made everything harder; CRDT-as-core was over-engineering for single-user. Also: Robert Lord, ["Text Editing Hates You Too"](https://lord.io/text-editing-hates-you-too/).
- **matrix-rich-text-editor** ([repo](https://github.com/matrix-org/matrix-rich-text-editor)): Rust core + wasm-bindgen/UniFFI wrappers for Web/iOS/Android — exactly the proposed architecture — **archived at matrix-org Sept 2024**, maintenance-level at element-hq. Study its issue tracker for cross-boundary reconciliation pain.
- **Ensemble (2025)** — screenplay editor, Rust/WASM core + canvas view ([writeup](https://brutaldocs.com/@pete/how-i-built-ensemble)): typed events in, serialized RenderFrame out; synchronous per-keystroke loop (cache-hit render 37 µs); conclusion: custom Rust/WASM editors are "hard to justify unless your document has structure off-the-shelf editors actively fight against" — a markdown-native editor arguably qualifies.
- **CodeMirror 6** — the reference in-process core/view separation: immutable EditorState, pure transactions, `ChangeSet.mapPos`, browser/IME mutates DOM first then read-back reconciliation. Obsidian-style live preview is built exactly this way.
- **Zed** — rope over **SumTree** (copy-on-write B+ tree with monoid summaries), CRDT buffer with **Anchors** (positions tied to the insertion that created them), layered coordinate maps ([Rope & SumTree](https://zed.dev/blog/zed-decoded-rope-sumtree)).
- **AppFlowy** — the hot editing path (appflowy-editor) is in **Dart**; Rust owns persistence/sync. Under shipping pressure they kept the keystroke loop inside the UI toolkit and made Rust the source of *durable* truth, not the per-keystroke arbiter.
- **Linebender:** Parley 0.7 (Nov 2025) added Android-IME editor features and a PlainEditor; **Xilem/Masonry still had no rich-text editing widget as of Q1 2026** — the pure-Rust-view route remains immature.

### Markdown parsing in Rust

| Parser | Positions | Lossless? | Markdown output | Status |
|---|---|---|---|---|
| **pulldown-cmark** | byte ranges via `into_offset_iter()` | Pull events, no AST | 3rd-party `pulldown-cmark-to-cmark` (normalizing) | Active, 0.13.x |
| **comrak** | AST with `sourcepos` | AST, not full CST | Built-in CommonMark renderer with active round-trip fixes | Very active |
| **markdown-rs** | unist positions on every mdast node | Token-level lossless claim; no serializer | — | 1.0.0 Apr 2025; dormant since |
| **tree-sitter-markdown** | Full CST, byte ranges, **incremental** | CST by construction | N/A (source unchanged) | Maintained; highlight-grade fidelity, not spec-exact |

**No Rust markdown parser offers a lossless typed CST (rowan-style) with incremental reparse and spec-exact fidelity.**

---

## 6. Synthesized architectural lessons

1. **Pick one source of truth deliberately.** Nobody in the PM/Lexical family treats markdown as runtime truth; all serialize a typed tree and all normalize on round-trip. The two proven markdown-truth architectures: (a) **CM6/Obsidian-style** — markdown rope = truth, rich text = decoration layer over source spans (round-trip becomes a non-problem); (b) **MDXEditor/Milkdown-style** — markdown is the contract, an editor tree is runtime truth, round-trip is normalizing and fidelity is a test-suite discipline.
2. **Synchronous, in-process core API.** Xi is the definitive negative result for async core↔view; Ensemble, CM6, Lexical, Zed are the positive results. FFI/WASM calls per keystroke are microsecond-cheap if payloads are coarse (event in → minimal delta out).
3. **Lexical's state design is the most Rust-portable** (flat keyed map, copy-on-write, double buffering, dirty-key diffing).
4. **Port ProseMirror's change layer, not its view layer.** But heed Wordgard's revisions: a **delta/token change format** beats polymorphic Step classes (simpler enums, easier FFI serialization, enables limited OT and selective undo); relaxed schema + programmatic correction beats rigid content regexes.
5. **Design input as intents; build the IME "authority flip" into the core API from day one.** The core needs a first-class **composition session**: while active, the view feeds provisional text, the core defers transforms/normalization on the composed range, history coalescing pauses, the core re-asserts at commit. If the core assumes it can always push state down, IME support is unfixable later.
6. **Selection lives in the core as anchors**; the view owns only the visual caret during composition.
7. **Undo behind a trait, chosen consciously:** snapshots (cheap, single-user) vs inverted ops + rebasing (collab-compatible). If collab is on the roadmap, snapshot undo must be swappable.
8. **Don't rebuild the CRDT layer:** Automerge 3, Loro, Yrs are production-grade Rust with bindings.
9. **Budget honestly:** the browser adapter absorbed the majority of ProseMirror's decade of maintenance; Android is the worst platform by unanimous testimony. Plan the Android adapter at a multiple of iOS/web.
10. **Ecosystem patterns:** headless core + thin per-framework adapters wins (ProseKit); monoliths stagnate (remirror, Editor.js, Quill). BlockNote's explicit `blocksToMarkdownLossy()` naming is an API-honesty pattern worth copying.
