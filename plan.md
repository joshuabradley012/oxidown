# Oxidown: Cross-Platform Markdown-Native Editor — Plan v2

**A Rust-core hybrid live-preview markdown editor: the markdown text is the document, rendering is a derived decoration layer, and AI streaming is a first-class input source.**

> v2, July 2026. Supersedes the v1 draft. Grounded in the six research reports under [research/](research/) and the annotated landscape in [examples.md](examples.md). Where v1 and v2 disagree, v2 is deliberate — §1 lists the reversals and why.

---

## 1. What changed from v1, and why

v1 had the right instincts (Rust core, thin platform views, markdown-first) but contained contradictions that the research resolves:

| # | v1 said | v2 says | Why |
|---|---------|---------|-----|
| 1 | "Markdown IS the data model" — then defined a lossy Block/Inline AST as the document | **The markdown text (rope) IS the document.** The parse tree is a derived, disposable overlay | v1's AST drops `*` vs `_`, fence style, indentation — its own "round-trip without loss" exit criterion was unachievable. Text-as-truth makes losslessness free (research/01) |
| 2 | WYSIWYG rendering (`to_html()` → contentEditable) with a toolbar | **Hybrid live preview**: decorations + syntax concealment, reveal-on-cursor (Obsidian/Bear model), with source mode as a toggle | The 2026 market consensus for markdown-native tools; dissolves round-trip, IME, and corruption risks; WYSIWYG-over-markdown is the dying middle (Typora tax, marktext dead) (research/01) |
| 3 | React re-renders `dangerouslySetInnerHTML` per keystroke | **The platform text widget owns the IME-facing buffer; the core reconciles per-edit** | innerHTML-per-keystroke destroys selection and aborts IME composition — the canonical broken pattern every serious editor exists to avoid (research/02 §1) |
| 4 | IME handling scheduled for week 15 ("polish") | **Composition sessions are a day-1 core primitive** | If the core assumes it can always push state down, IME is unfixable later. Android input ≈ composition; the "authority flip" must be in the API (research/02, 04) |
| 5 | Public API positions are byte offsets (`Position(usize)`) | **Opaque anchors are the public position type**; byte offsets never escape the core | Offsets are valid against exactly one document version and are UTF-8-specific; JS/Kotlin speak UTF-16. Anchors are also the door to collaboration (research/05 §7) |
| 6 | Undo = command + hand-written inverse | **Append-only operation log**: ID'd, origin-tagged, invertible splices with parent versions; undo = inverted ops filtered by origin | The eg-walker result: a well-designed single-user op log is already 80% of a collaborative document. Snapshot/inverse-command undo forecloses that (research/05) |
| 7 | Custom `mdeditor-swift` + UDL files | **One `oxidown-ffi` crate, uniffi proc-macros** (no UDL); wasm-bindgen for web | uniffi consensus moved to proc-macros; one crate serves Swift + Kotlin (research/03 §5) |
| 8 | No streaming story (despite streaming-architecture.md) | **AI streaming is a first-class transaction source** with an append fast-path and open-tail parsing | This is the "new-gen" differentiator; nobody has an incremental Rust markdown parser or a good streaming-into-editable-doc story (research/06 §8) |
| 9 | CommonMark-only; tables/tasks/strikethrough cut from v1 | **GFM is the baseline** (tasks, strikethrough, autolinks in v1; tables render-only in v1) | GFM is what users mean by "markdown" in 2026; tasks/strikethrough are nearly free. Tables *editing* is genuinely hard (Obsidian took 4 years) — staged deliberately |
| 10 | 34-week schedule to 4 platforms | **Milestone gates, no fake weeks; web → desktop → iOS → Android**, with an explicit webview fallback decision for mobile | Bear took ~4 years for Apple-only; iA Writer quit Android. Honest phasing with kill/pivot criteria beats fiction |

---

## 2. Vision

Build the definitive markdown editing engine — one Rust core that makes every platform's editor feel native, fast, and byte-faithful to the file.

### Product principles

1. **File over app.** The `.md` file (or markdown string) is the document. Zero normalization: if you didn't edit a byte, we didn't change it. Clean git diffs are a feature.
2. **Hybrid live preview.** Formatted text inline, syntax concealed until the cursor arrives (Obsidian/Bear/Lettera model). Source mode is a first-class toggle, not an apology.
3. **Streaming-native.** AI/LLM text streams into a *live, editable* document as a first-class operation source — not a read-only renderer bolted to the side. (This absorbs the goals of [streaming-architecture.md](streaming-architecture.md).)
4. **One brain, native fingers.** The Rust core owns document, parse, decorations, history, and streaming. Each platform's *native text stack* owns keystrokes, IME, selection UI, and accessibility — because that's the seam that works (research/06 §10).
5. **Collab-ready, not collab-built.** v1 is single-user, but the op log, anchors, and IDs are designed so collaboration is an added module, not a rewrite (research/05 §7).

### Explicitly not building

- A block-database workspace (Notion/AnyType lane — they abandon markdown-as-truth for good reasons that aren't ours).
- Non-markdown formatting (colored text, arbitrary nesting). Extensions must be *syntax* (e.g. `==highlight==`), never schema.
- A pure-Rust-rendered UI on mobile/web in v1 (no AccessKit on iOS/web; winit mobile IME still beta — research/03 §7). Re-evaluate for desktop in 2027; Lockbook proves the escape hatch exists if the native-view strategy fails.

---

## 3. The defining decision: hybrid live preview

Four editing models exist in the market (research/01 §1). We choose **concealment-style hybrid live preview** over WYSIWYG-over-markdown. The evidence:

- **Market**: Obsidian made it the default at ~1M-user scale; Bear spent 4 years building a native engine for it and in 2026 spun it out as a standalone product (Lettera); the pattern is now commoditized as CM6 libraries and Neovim plugins. Meanwhile WYSIWYG-over-.md stagnated (Typora), died (marktext), or fled markdown entirely (Logseq DB, SiYuan, AppFlowy).
- **Fidelity**: we never serialize, so we can never normalize. The Typora prettify/renumber saga structurally cannot happen.
- **IME**: the core's write API is text splices, so the platform input stack (IME, autocorrect, dictation) drives the document directly. Decorations are cosmetic and deferrable during composition — the exact property that makes composition survivable (research/01 §8, 02 §1).
- **Robustness**: decorations are derived, disposable state. A decoration bug renders wrong; it cannot corrupt the document. In AST editors, editing bugs corrupt documents.
- **Collab economics**: plain-text merge machinery (mature) instead of Peritext-class rich-text CRDTs (frontier).

**What we accept, eyes open** (mitigations in §6/§8):
- Structured blocks (tables, images, math) need **widget islands** with bidirectional source mapping — the hardest UX in this model (Obsidian shipped table editing 4 years in).
- Document-global features (footnotes, link-reference validation) fight block-local incrementality — v1 ships a cheap global link/footnote index pass or accepts Obsidian-style gaps, explicitly.
- Reveal polish (no layout shift, no flicker on drag, composition freezing) must be implemented per platform view. The playbook is public (research/01 §4); it is still work × N platforms.
- Concurrent *formatting* merges at markdown-source level have known intent failures (`**The **fox** jumped.**`) — acceptable for the multi-device/casual-collab tier we'd ship first; Google-Docs-grade concurrent formatting would require a structural layer we deliberately defer (research/05 §8).

---

## 4. Architecture

```
                    ┌──────────────────────────────────────────────────┐
                    │              oxidown-core (Rust)                 │
                    │                                                  │
                    │  Rope (ropey) ── authoritative document text     │
                    │  Parser ── block-incremental, span-preserving    │
                    │  Overlay ── block index + inline trees + spans   │
                    │  OpLog ── ID'd, origin-tagged, invertible splices│
                    │  Anchors ── stable positions (op id + offset)    │
                    │  History ── inverted ops, origin-filtered        │
                    │  Composition ── IME session state machine        │
                    │  Streaming ── AI-source transactions, open tail  │
                    │  Decorations ── viewport-scoped span emission    │
                    └───────────────┬──────────────────────────────────┘
                     in-process, synchronous, per-edit granularity
        ┌───────────────┬───────────┴────────┬──────────────────┐
        ▼               ▼                    ▼                  ▼
  wasm-bindgen     (same .wasm)         uniffi (proc-macro) uniffi
        │               │                    │                  │
┌───────────────┐ ┌───────────────┐ ┌────────────────┐ ┌────────────────┐
│ Web: CM6 view │ │ Desktop: Tauri│ │ iOS/macOS:     │ │ Android:       │
│ CM6 doc =     │ │ v2 shell over │ │ UITextView +   │ │ BasicTextField │
│ IME buffer;   │ │ the web view; │ │ TextKit 2;     │ │ (state-based) +│
│ decorations ← │ │ core linked   │ │ NSTextStorage  │ │ OutputTransform│
│ core spans    │ │ natively      │ │ mirror         │ │ -ation styling │
└───────────────┘ └───────────────┘ └────────────────┘ └────────────────┘
```

### The five invariants

Everything else is negotiable; these are not. Each encodes a researched failure mode:

1. **In-process, synchronous core.** Views get synchronous reads; no IPC, no async between keystroke and document. (xi-editor's fatal flaw — research/03 §3.)
2. **The platform text widget owns the hot (IME-facing) buffer.** The core is the authoritative *document*, reconciled per-edit — the view is never re-rendered from scratch, and the composition region is never touched from outside. (Google rewrote BasicTextField over this; CM6/Lexical converged on it — research/02 §1, 04 §2.5.)
3. **Coarse boundary, batched payloads.** One call in per edit (splice + intent metadata); one batched result out (damage list + viewport decoration spans + selection/anchor updates). Never per-node or per-span crossings; bulk text as bytes. (uniffi ~1–4µs/call, Kotlin/JNA worse; Automerge's popPatches pattern — research/03 §5–6.)
4. **All derived state is disposable.** Parse trees, decorations, block index: all re-derivable from rope + oplog. Only the rope and the oplog are persisted truth.
5. **Composition inverts authority.** While an IME session is open, the view leads and the core follows (provisional splices, deferred normalization/decorations on the composed range, paused undo coalescing); the core re-asserts at commit. (The "authority flip" every surviving editor implements — research/02 §1.4.)

### Mirror-consistency discipline

The platform buffer and the core rope are the same text by construction (every splice applied to both, synchronously). Debug builds verify with rolling checksums per edit; release builds checksum on save/idle. Divergence = bug = resync from core + telemetry, never silent.

---

## 5. Core design (`oxidown-core`)

### 5.1 Text store

`ropey` 1.6 (migrate to 2.0 when stable — byte-indexed, matching our span math). We need its cheap copy-on-write snapshots (background parse/render reads a snapshot while the UI thread edits) more than raw splice speed. UTF-8 byte offsets internally; **UTF-16 code-unit conversion at every FFI boundary** (ropey's `metric_utf16`) because JS/Swift-strings/Kotlin views are UTF-16-oriented. Grapheme-cluster iteration via `unicode-segmentation` for cursor motion.

### 5.2 Parser — two phases, one contract

The parser's contract: given the rope and an edit, produce (a) a **block index** (block kind + byte span + stable block ID per top-level block, nested where needed), (b) **inline trees per dirty block** with byte-accurate spans for every delimiter and content run, and (c) a **damage list** (which blocks changed). It never produces text — rendering always slices the original rope by span. Serialization does not exist; the document is already markdown.

- **Phase A (build now):** `pulldown-cmark` 0.13 with GFM options + `into_offset_iter()` for byte-exact spans, wrapped in our block-index layer. Full reparse per edit is single-digit-ms up to ~100KB documents — fine for v1 and honest about it (dprint-plugin-markdown ships on exactly this span-slicing approach).
- **Phase B (the moat):** a hand-written **block-incremental, lossless markdown parser** — the lezer-markdown algorithm (block-granular tree fragment reuse; per-block inline parse) in Rust, spec-validated against comrak/CommonMark+GFM suites. No such crate exists (research/03 §2) — this is Oxidown's core IP and what makes 1M-line documents and streaming cheap. Like lezer-markdown, we deliberately skip whole-document semantics in the hot path and maintain a separate cheap **global index** (link refs, footnotes) updated per damage list.

Phase A and B are swappable behind the same contract; Phase B lands when profiling demands it or streaming makes it urgent, not before.

### 5.3 Identity, positions, anchors

- **Stable block IDs** — `(replica_id, counter)`, assigned when a block first appears, sticky across edits via the op log (an edit inside a block keeps its ID; a split allocates one new ID). Persisted in the sidecar (§5.5), never written into the markdown.
- **Anchors are the only public position type**: opaque `Anchor { op_id, offset, bias }` — a position expressed against the insertion that created the text, with before/after bias (Peritext's insight; Zed's mechanism). Selections, decoration spans handed to views, scroll positions, and future comments are all anchors. `resolve(anchor) -> byte_offset` and `anchor_at(offset, bias)` are core calls; resolved offsets are transient render-layer values valid for exactly one revision.
- Raw byte offsets appear in the FFI only as *revision-stamped* values inside a single edit/decoration exchange.

### 5.4 Operations, transactions, history

All mutation flows through transactions that append to an **operation log**:

```rust
struct Op {
    id: OpId,                    // (replica_id, counter) — replica_id exists from day 1
    lamport: u64,
    parent: Version,             // version this op was generated against (single-user: previous op)
    origin: Origin,              // User | Ime | Paste | Command(kind) | AiStream(session) | Plugin | RemotePeer(reserved)
    kind: OpKind,                // Splice { at: usize, delete: usize, insert: SmolStr } — positions valid at generation time
}
```

- Ops are **serializable, invertible splices** with intent granularity (never coarse snapshot diffs). The log is persisted run-length/columnar (keystrokes come in runs; ~1 byte/keystroke in practice) alongside a text snapshot for fast load; log truncation is a feature, not an accident.
- A `ChangeSet`-style `map`/`compose` algebra (CodeMirror's law: `A.compose(B.map(A)) == B.compose(A.map(B, true))`) is the contract that later buys server-rebased collab for free — and is what keeps anchors and decoration spans valid across edits today.
- **Undo/redo = inverted ops, filtered by origin, coalesced by time window (~500ms) and broken by selection jumps/block boundaries.** Stored inverses are mapped across subsequent changes (ProseMirror-history's design). Selection state is captured per entry. Never snapshot-restore (research/05 §7.4). One AI stream session = one undo unit. Coalescing pauses during composition.
- **Why this matters beyond undo:** this exact structure — untransformed ops + IDs + parent versions — *is* an eg-walker event graph. Figma Code Layers, Zed DeltaDB, and Loro all converged here. We get their upgrade path for the cost of a well-designed undo system.

### 5.5 Persistence

The `.md` file is the document — always writable, always complete, byte-exact. The op log + block IDs + anchors live in an optional **sidecar** (app-managed store; e.g. SQLite on native, IndexedDB/OPFS on web). Losing the sidecar loses history and stable IDs, never content. This is the "file over app" contract enforced architecturally.

### 5.6 Composition sessions (IME)

First-class core API:

```
composition_begin(anchor_range) -> SessionId
composition_update(session, provisional_text)   // provisional splice; decorations frozen on range
composition_commit(session, final_text)         // real op enters the log (origin: Ime)
composition_cancel(session)
```

During a session: no decoration changes intersecting the composed range are emitted; normalization/auto-format transforms are deferred; undo coalescing pauses; anchors inside the range are pinned. Views map platform events onto this (`compositionstart/update/end` on web; `NSTextInputClient` marked text on Apple; the composing region via `InputTransformation`/state observation on Android). Essentially all Android soft-keyboard input arrives as composition — this API *is* Android support.

### 5.7 Decorations and reveal

The core emits, for a requested viewport (± margin), a batched set of decoration spans over anchors:

- `Mark { span, style }` — styled text runs (bold/italic/code/heading levels/link text…)
- `Conceal { span }` — syntax delimiters to hide (view decides *how*: width-collapse, not removal, to keep line metrics stable)
- `Line { block_id, kind }` — block-level chrome (quote bars, list gutters, code-block background, heading size)
- `Widget { span, kind, payload }` — replace-range islands: images, math, and (v1.x) the table editor

**Reveal is computed core-side** from the selection: any selection∩node intersection (including delimiters) returns that node's spans un-concealed. One implementation of the tricky predicate; N thin renderers. The anti-flicker rules from the field (no rebuild during drag gestures; never touch composition ranges; invalidate per damage list only) are part of the view contract, documented once and tested per platform (research/01 §4).

**Rendering never round-trips HTML.** `to_html()` exists only as an export/preview utility, never in the editing path.

### 5.8 Commands

Editing commands (`toggle_bold`, `set_heading(level)`, `toggle_task`, `indent_list_item`, list-continuation-on-return, smart delimiter pairing…) are **text transforms implemented in the core**: they read the overlay, compute the minimal splices (e.g. insert/remove `**` at the right boundaries, renumber only the edited list run), and emit ops with `origin: Command`. Views expose them as toolbar/keyboard/menu affordances and get the resulting damage like any other edit. `formatting_at(selection)` drives toolbar state, computed from the overlay.

Input intents follow the W3C `inputType` taxonomy where applicable so every platform adapter translates into the same core vocabulary.

### 5.9 Streaming ingestion (the differentiator)

```
stream_open(at: Anchor, origin_session) -> StreamId
stream_append(id, chunk)     // append fast-path
stream_close(id)             // finalize; whole session = one undo unit
```

- **Append fast-path:** an appending stream only ever dirties the **open tail block**; closed blocks above it are final and never reparsed (the streaming case of the incremental parser — with Phase A, we cheaply special-case "edit at end"). Cost per chunk is O(tail block), not O(document).
- **Tail policy:** the open block renders via lookahead-throttling or remend-style repair (unclosed `**`, half-typed links, open fences) — a *view* policy over honest parser output, never a mutation of the document. Code fences stream line-by-line into the open block (the known Streamdown failure we can beat, because we own the parser).
- **Concurrent editing:** the user can edit *above* the stream insertion point while streaming; the stream's insertion anchor maps through user edits via the op algebra. Edits inside the streaming region are blocked until close (v1) — same choice BlockNote AI made with its review state.
- **Review mode (v1.x):** streams can open in a `suggestion` state — rendered but pending accept/reject as a whole — following the BlockNote/Tiptap-AI convergence.
- Transport (SSE etc.) stays app-side, per [streaming-architecture.md](streaming-architecture.md); the core consumes chunks from any transport.

---

## 6. Markdown scope

**Baseline: CommonMark + GFM.** pulldown-cmark options give most of this at near-zero cost.

### v1 — full editing
Paragraphs, ATX + setext headings, bold/italic (both delimiters, preserved as typed), inline code, code fences (info string, syntax highlighting view-side), links & autolinks, images (widget island, placeholder → async load), blockquotes (nested), ordered/unordered lists (nested; marker style preserved), **task lists** (checkbox toggle = text edit), **strikethrough**, thematic breaks, hard breaks (both syntaxes, preserved).

### v1 — render, don't structure-edit yet
- **Tables**: parsed, rendered as a widget island, source-editable on reveal. Cell-grid editing UX is v1.x (Obsidian's history says: do it deliberately or badly).
- **Footnotes**: parsed and marked; resolved rendering only in preview/export (viewport-local vs document-global tension, accepted à la Obsidian, revisited with the Phase-B global index).
- **Raw HTML**: preserved byte-exactly, rendered as literal styled text (never interpreted — also the XSS-safe default). Frontmatter: preserved, shown as a metadata block.

### Explicitly deferred
Definition lists, sub/superscript, highlight (`==`) and other extension syntaxes (the extension *mechanism* is Phase-B parser plugins), MDX/JSX, WikiLinks (fits the model; product decision later), math (render via widget island when prioritized).

---

## 7. Platform strategy

Ship order: **Web → Desktop (Tauri) → iOS → Android.** Each platform reuses the same core contract: mirror protocol + decoration spans + composition sessions.

### 7.1 Web (first)

**CodeMirror 6 as the view substrate**, not a bespoke contentEditable stack. CM6's doc is the IME-facing buffer (invariant 2); its update loop feeds splices to the WASM core; core damage/decoration output maps to CM6 `RangeSet`s (mark/replace/line/widget — a 1:1 vocabulary match with §5.7). CM6's own markdown mode and history are disabled — Oxidown is parser and historian.

Why CM6 and not from scratch: it is the only web editing surface with a decade of paid-down IME/mobile-browser workarounds (it's what Obsidian ships on mobile), its decoration and viewport systems are exactly our model, and our core still owns everything that makes the product (parse, reveal semantics, history, streaming). We can revisit a bespoke view (or EditContext, Chromium-only, as progressive enhancement in 2027) once the product exists. The `@oxidown/react` package wraps this with toolbar/theming; the core runs on the main thread (µs-scale calls) with an option to move to a Worker + patch mirror for huge documents.

### 7.2 Desktop (nearly free)

**Tauri v2** shell over the same web view, with `oxidown-core` linked **natively in-process** (not WASM) and the same protocol bridged over Tauri's invoke layer, falling back to WASM-in-webview if the bridge granularity disappoints. File-system access, watch-and-reload, and the sidecar store come with the shell. (Obsidian-on-Capacitor proves the shape; Tauri makes it Rust-native. Linux WebKitGTK jank is a known tax.) A GPUI/native-Rust desktop view is a 2027 option, not a commitment.

### 7.3 iOS / macOS

**UITextView on TextKit 2**, with an `NSTextStorage` subclass mirroring the core document (apply core deltas → `edited(range:changeInLength:)`). Decorations map to attributed-string attributes + TextKit 2 rendering surfaces; WWDC 2026's viewport/attachment-reuse APIs cover gutters, code-block chrome, and stable inline image views. Marked-text (composition) maps to §5.6 sessions. Do **not** attempt a custom `NSTextContentManager` (documented dead end), and treat the iOS 26 SwiftUI `TextEditor` as a companion/prototype surface only (inline-only, no blocks/attachments). lexical-ios is the architecture reference for reconciler-into-TextKit; STTextView is the reference for TextKit 2 sharp edges.

### 7.4 Android (last, and gated)

**State-based `BasicTextField`** (`TextFieldState` mirror) + **`OutputTransformation`** for styling/concealment (display-only, automatic offset mapping — the IME always sees raw text, which is exactly our model) + `InputTransformation` for list-continuation/smart-pairs; images/tables/embeds as sibling composables in a block layout (inline content in text fields is unsupported). Undo delegated to the core, platform undo disabled.

**Gate:** before building, re-validate against a **webview fallback** (our web editor in a WebView shell — the Obsidian/Joplin/Notion route). If the native spike can't pass the CJK/Gboard/Samsung/SwiftKey matrix within budget, ship the webview on Android first and revisit. iA Writer's Android exit and Notion's still-webview editor earn this humility.

### 7.5 The permanent line item

IME/keyboard QA is not a phase; it's a standing cost on every platform (CJK composition, Gboard/Samsung/SwiftKey divergence, autocorrect, dictation, voice input). Device-farm runs of a scripted composition suite are part of CI from the first mobile milestone.

---

## 8. Milestones (gates, not weeks)

**M0 — Skateboard (de-risk spike).** Rope + pulldown-cmark span layer + op log skeleton; CM6 web view with conceal/reveal for emphasis + headings; typing, undo, **CJK composition on desktop browsers + Android Chrome**. *Gate: composition survives concealment; reveal has no layout shift; core boundary stays under 1ms/edit on a 100KB doc. This spike is allowed to kill or reshape the architecture.*

**M1 — Core v1.** Full §5 surface: overlay + block IDs, anchors, history (origin-filtered), composition sessions, decoration emission, commands, streaming fast-path (Phase-A parser). *Gate: CommonMark+GFM spec suites pass (comrak as oracle); property tests — parse→no-op-edit→byte-identical, random-edit fuzzing with mirror checksums; core ≤ ~400KB gz as WASM.*

**M2 — Web editor beta.** CM6 view complete (reveal polish, widget islands for images/code fences, toolbar, virtual viewport on 1MB docs), `@oxidown/react` published, streaming demo: LLM streams into a doc the user is simultaneously editing. *Gate: Chrome/Firefox/Safari + Android Chrome + iOS Safari; keystroke-to-paint p95 < 16ms on 100KB docs; a11y audit of the CM6 surface.*

**M3 — Desktop.** Tauri shell, native-linked core, file workflows (open/watch/save byte-exact), sidecar store. *Gate: cold start < 1s; 10MB document usable (Phase-B parser decision point).*

**M4 — iOS/macOS.** TextKit 2 view per §7.3, feature parity minus tables-editing. *Gate: composition suite passes (Japanese/Korean/Chinese + dictation); TestFlight-quality feel on 120Hz devices.*

**M5 — Android.** §7.4 after its gate. *Gate: the keyboard matrix, on the device farm.*

**Parallel track (starts ~M2): Phase-B incremental parser** — block-incremental, lossless, streaming-optimized; swapped in behind the M1 contract when it beats Phase A on the 10MB and streaming benchmarks.

**v1.x after all gates:** table cell-editing island, footnote live rendering (global index), suggestion/review mode for streams, extension syntax mechanism, WikiLinks, math.

**v2 candidates:** multi-device sync via source-level text merge (Obsidian-Relay-grade, riding the op log), then — only as a committed product decision — structural collab (research/05 §8), plugin API surface, GPUI desktop view.

---

## 9. Testing & quality gates

- **Conformance:** CommonMark 0.31 + GFM spec suites against the overlay (comrak as the oracle for divergence triage).
- **Losslessness (the flagship invariant):** property tests — open→save is byte-identical; any command followed by its undo is byte-identical; random edit sequences preserve mirror checksums (rope vs platform buffer) across all views.
- **Fuzzing:** cargo-fuzz on the parser (arbitrary bytes; arbitrary edit scripts against a model implementation); differential fuzz Phase A vs Phase B during the swap.
- **Composition suite:** scripted IME scenarios (compose/commit/cancel; conceal-adjacent composition; composition during stream) run per platform on device farm.
- **Performance benches in CI:** keystroke latency (p50/p95), parse time vs doc size, boundary payload sizes, streaming chunk cost, memory on the 10MB doc.
- **Anchor/oplog laws:** the compose/map algebra property-tested (the ChangeSet law), anchor stability across arbitrary interleaved edits, undo/redo/redo-after-copy invariants (Figma's "redo back to present must not change the document").

---

## 10. Top risks

| Risk | Signal | Mitigation |
|---|---|---|
| Reveal/conceal UX polish is harder than the playbook suggests | M0/M2 feel "jumpy"; layout shifts | The public playbook (research/01 §4) is encoded in the view contract; budget polish explicitly; source-mode toggle is always shippable |
| Android native view can't reach quality | M5 gate failures on keyboard matrix | Pre-committed webview fallback (§7.4); the web editor is mobile-hardened by then |
| Phase-B parser is a research project | Slips past M3 needs | Phase A is shippable indefinitely for ≤ ~1MB docs; Phase B is an upgrade, not a dependency |
| TextKit 2 sharp edges (viewport/height bugs) | M4 scroll jank | STTextView's issue history as a map; WWDC26 APIs; worst case constrain doc size/virtualize at block level |
| One-person-deep dependencies (CM6/ProseMirror-world, ropey) | Bus factor | Vendored forks acceptable; our core owns the semantics, views are replaceable by design |
| Scope creep toward block-workspace features | "Just add databases" | §2 non-goals; extensions must be syntax; Logseq's split is the cautionary tale |

---

## 11. Workspace layout

```
oxidown/
├── Cargo.toml                    # workspace
├── crates/
│   ├── oxidown-core/             # rope, parser contract, overlay, oplog, anchors,
│   │                             # history, composition, decorations, streaming, commands
│   ├── oxidown-parser-pulldown/  # Phase A implementation of the parser contract
│   ├── oxidown-parser-inc/       # Phase B (block-incremental, lossless) — later
│   ├── oxidown-wasm/             # wasm-bindgen boundary (batched patch protocol)
│   └── oxidown-ffi/              # uniffi proc-macro boundary (Swift + Kotlin)
├── packages/
│   ├── oxidown-view-cm6/         # CM6 view adapter (decorations, reveal, widgets)
│   ├── oxidown-react/            # React wrapper + toolbar/theming
│   └── oxidown-streaming/        # stream-source helpers (SSE → stream_append)
├── apps/
│   ├── desktop/                  # Tauri v2 shell
│   ├── ios/                      # UITextView/TextKit2 view + demo app
│   └── android/                  # Compose BasicTextField view + demo app
├── research/                     # the six reports backing this plan
├── examples.md                   # annotated landscape
├── streaming-architecture.md     # transport-layer notes (app-side)
└── plan.md                       # this document
```

---

## 12. Resolved & open questions

**Resolved by this revision** (v1 §9 items): collaboration model → op-log now, merge algorithm deferred with doors open (§5.4); mobile keyboards → composition sessions + platform-native surfaces (§5.6, §7); rendering → decorations, never HTML (§5.7); extension API → syntax-level parser plugins in Phase B, never schema (§6).

**Still open, deliberately:**
1. **Licensing/business shape** — MIT core + paid cloud (sync/collab/AI) is the 2026 ecosystem consensus (Tiptap/BlockNote/Plate); decide before M2 publishes packages.
2. **Theming contract** — CSS variables on web are obvious; the cross-platform token story (core-emitted style keys → platform themes) needs a design pass at M2.
3. **Worker-vs-main-thread WASM default** — measure at M2 with real documents before choosing.
4. **Sidecar store schema** — sketch at M1, harden when sync (v2) gives it a second consumer.
5. **Name/branding of the parser crate** if Phase B is published standalone (it's independently valuable — no incremental CommonMark parser exists in Rust).

---

*Living document. When evidence contradicts this plan, update the plan — and note it in §1 the way v1's reversals are noted, so the reasoning trail survives.*
