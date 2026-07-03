# Hybrid Live-Preview Markdown Editing: State of the Art

> Research compiled July 2026 for the Oxidown plan refresh. Covers the "hybrid live preview" school — markdown **text as the single source of truth**, with formatting rendered in-place via decorations/syntax concealment — versus WYSIWYG editors that hold a rich AST/DOM and serialize to markdown.

---

## 1. Taxonomy: four editing models actually in the market

| Model | Source of truth | Syntax visible? | Examples |
|---|---|---|---|
| Split-pane preview | Text | Always (left pane) | HackMD/CodiMD, Zettlr (pre-3.0 partially), VS Code, Dillinger |
| Syntax-highlighted plain text ("WYSIWYM") | Text | Always, styled inline | iA Writer, Ulysses (Markdown XL tokens) |
| **Hybrid live preview (concealment)** | **Text** | **Only near cursor** | Obsidian Live Preview, Bear 2/Panda/Lettera, Zettlr 3, Inkdrop, render-markdown.nvim |
| WYSIWYG over markdown | AST/DOM (serialized to .md) | Never (raw via escape hatch) | Typora, Milkdown/Crepe, Toast UI, Vditor WYSIWYG mode, SiYuan Protyle, BlockNote |

Typora sits at the boundary: files stay `.md`, but the *editing state* is a rendered contenteditable DOM ("hybrid view"/instant-render on an AST), which historically caused source-normalization churn (see §5).

---

## 2. Obsidian Live Preview: the reference implementation

Obsidian's editor is CodeMirror 6; Live Preview is the **default editing mode**, defined officially as "shows formatted text inline while hiding most Markdown syntax… when your cursor enters formatted content, the underlying syntax becomes visible for editing," with Source mode kept for users who want "all Markdown syntax exactly as written" ([Obsidian Help: Edit and read](https://obsidian.md/help/edit-and-read)).

**Mechanics (as reverse-engineered by the plugin community).** A ViewPlugin/StateField walks the Lezer syntax tree for the visible viewport and emits CodeMirror `Decoration`s: *mark* decorations style text and tag delimiter tokens (`cm-formatting-*`), *replace/widget* decorations swap ranges for rendered DOM (images, tables, math, embeds). Concealment of `**`, `#`, `[]()` etc. is decoration-driven, keyed off selection overlap ([forum: How to configure CodeMirror to work like Live Preview](https://forum.obsidian.md/t/how-to-configure-codemirror-to-work-like-live-preview/43047); [nothingislost/obsidian-codemirror-options](https://github.com/nothingislost/obsidian-codemirror-options), which implemented the same idea on CM5/CM6 before core LP shipped; [OlegWock's Emera writeup](https://sinja.io/blog/how-i-built-notebook-in-obisidian-emera) on registering StateFields that "provide decorations… rendering components directly in the editor in place of original code blocks").

**Known limitations (persistent, structural):**
- **Footnotes don't render in LP** (only in Reading view), and footnotes inside tables/callouts render as bare numbers — a documented years-old gap ([forum](https://forum.obsidian.md/t/footnotes-are-not-rendered-in-live-preview-mode/75904), [obsidian-help#399](https://github.com/obsidianmd/obsidian-help/issues/399), [bug: footnotes in tables/callouts](https://forum.obsidian.md/t/footnotes-in-tables-and-callouts-live-preview/114880)). Root cause: footnotes require *document-global* resolution, which fights viewport-local decoration computation.
- **Large tables are slow** in LP ([forum](https://forum.obsidian.md/t/large-tables-slow-to-render-in-live-preview/85013)); tables originally degraded to raw source on edit until Obsidian 1.5 (Nov 2023) shipped a dedicated **widget-island table editor** — cell-by-cell WYSIWYG editing while "tables remain stored as plain text Markdown" ([changelog 1.5.0](https://obsidian.md/changelog/2023-11-20-desktop-v1.5.0/)). This is the canonical proof that hybrid editors handle grid-structured blocks by embedding a mini-WYSIWYG widget mapped back to source.
- **Mobile/Android rough edges:** links not consistently tappable in LP on Android ([bug](https://forum.obsidian.md/t/links-are-not-consistently-active-in-live-preview-mobile-android-phone-app/87384)); replaced-image widgets can trap editing on Android ([bug](https://forum.obsidian.md/t/no-editing-possible-with-live-preview-of-images-on-android-makes/101101)); CJK IME flicker as decorations rebuild during composition (echoed in community reimplementations, below).

Despite this, Obsidian (~1M users, reportedly ~$25M ARR with a 7-person team) made hybrid LP the mainstream default for markdown-native tools ([Decoder interview with CEO Steph "kepano" Ango](https://www.daniel.pizza/links/decoder-obsidian-kepano/), [BigGo](https://finance.biggo.com/news/iVboYp0Bga3fZL9MJEv_), ["File over app"](https://stephango.com/file-over-app)).

---

## 3. CodeMirror 6: the substrate

### Lezer + lezer-markdown incremental parsing
- **lezer-markdown is not an LR/Lezer-runtime parser** — "Markdown can't really be parsed that way." It's a hand-written, **single-pass, incremental** parser that *emits Lezer-format trees* and **reuses tree fragments across edits at block granularity** ([lezer-parser/markdown](https://github.com/lezer-parser/markdown)). Practical consequence: an edit inside one paragraph reparses roughly that block; unchanged sibling blocks are reused as opaque fragments. Inline structure is parsed per-block, so inline reparse cost is bounded by block size.
- To stay single-pass/incremental it **skips whole-document semantics**: e.g., it "doesn't validate link references, so it'll parse `[a][b]` as a link even if no `[b]` reference is declared" — the same class of compromise behind Obsidian's footnote gap. Extensions (GFM tables/strikethrough/tasklists/autolink, sub/superscript, emoji, custom `BlockParser`/`InlineParser` with precedence) are first-class ([repo](https://github.com/lezer-parser/markdown); [@codemirror/lang-markdown](https://github.com/codemirror/lang-markdown)).
- **Tree representation is extremely cheap:** Lezer keeps a flat post-order log during parsing ("as cheap as appending a few numbers to an array") and stores fine structure in packed **buffer trees, 64 bits per node**, so trees for large docs are small and fast to rebuild/reuse ([Marijn Haverbeke, "Lezer", Sept 2019](https://marijnhaverbeke.nl/blog/lezer.html)). Error recovery is GLR-style with "badness"-scored branches (matters for always-broken mid-keystroke text). Marijn positions Lezer as "a bit less advanced than tree-sitter in some areas, a bit more advanced in others," designed for browser-size constraints.
- **Rendering is viewport-limited:** CM6 "detect[s] which part of the content is currently visible… and only render[s] that plus a margin," while tracking estimated/measured heights for the whole doc; background parsing similarly runs ahead of the viewport lazily ([System Guide](https://codemirror.net/docs/guide/)). Big-doc speed = block-incremental parse × viewport-only decorate/render.

### Decoration system
Four decoration types — **mark** (style a range), **widget** (insert DOM at a position), **replace** (hide/substitute a range), **line** — supplied via facets as immutable `RangeSet`s that are **mapped through document changes** rather than recomputed ([System Guide](https://codemirror.net/docs/guide/)). Hybrid LP uses all four: marks for styled text + delimiter tagging, replace for concealment and rendered islands (images/tables/math), line decorations for headings/quotes. Marijn's Sept 2025 post ["Addressing Editor Content"](https://marijnhaverbeke.nl/blog/addressing-editor-content.html) is the best current treatment of the underlying problem — offset positions (simple, but must be remapped on every edit) vs stable IDs (tombstones, expensive lookup) vs ordered IDs — directly relevant to any Rust core that must keep decoration spans valid across edits.

### IME/input philosophy (Marijn's writings)
- Core principle: **let the browser/OS input stack drive; read changes back from the DOM.** CM6's content element has "a DOM mutation observer registered on it, and any changes made in there will result in the editor parsing them as document changes" ([System Guide](https://codemirror.net/docs/guide/)) — i.e., input (typing, IME, autocorrect, dictation, spellcheck replacement) is *observed*, not intercepted.
- On composition: "During composition, if you mess with the DOM around the composition, or with the selection, you will **abort** the composition, making your editor pretty much unusable to IME users." CM6's refinement over CM5's "freeze everything" approach: "the part of the document that is being composed is left alone as long as its content isn't changed by outside code, but the editor's update cycle proceeds as normal" — so autocomplete/linting stay live during composition ([CodeMirror 6 Status Update, 2019](https://marijnhaverbeke.nl/blog/codemirror-6-progress.html)). Critically: **essentially all Android virtual-keyboard input arrives as composition**, so Android correctness ≈ composition correctness.
- **Direct implication for hybrid LP:** decoration changes that touch the composition range abort IME. Obsidian-style editors must defer decoration rebuilds around active composition — the documented source of CJK flicker bugs in LP clones ([codemirror-live-markdown design notes](https://github.com/blueberrycongee/codemirror-live-markdown)).
- **Mobile quality:** CM6 is widely considered the only serious web code/text editor on mobile, but Android issues persist in the tracker (touch selection glitches in Chrome/WebView [codemirror/dev#645](https://github.com/codemirror/dev/issues/645); repeated changelog workarounds for Chrome Android composition-end and scroll-into-view bugs, Mobile Safari's "fragile composition handling" — [changelog](https://codemirror.net/docs/changelog/)).

### An instructive historical tension
In 2020, Marijn himself told a developer asking about WYSIWYG-markdown-in-CodeMirror that tables were "probably not going to work" and recommended **ProseMirror** as "less fighting against the library" ([discuss thread](https://discuss.codemirror.net/t/implementing-wysiwyg-markdown-editor-in-codemirror/2403)). Obsidian then shipped exactly that on CM6 at million-user scale — including tables, via widget islands. Lesson: the hybrid model on a code-editor substrate was harder than it looked but is now a solved, well-trodden pattern with public reference implementations.

---

## 4. Cursor-reveal mechanics — the accumulated playbook

From Obsidian, [codemirror-live-markdown's design doc](https://github.com/blueberrycongee/codemirror-live-markdown/blob/main/CODEMIRROR_LIVE_PREVIEW_DESIGN.md), and [Atomic Editor](https://github.com/kenforthewin/atomic-editor) (2026):

1. **Reveal predicate = selection/range intersection.** For each formatted node, if any selection range (cursor counts as empty range) intersects the node's *full* extent (including delimiters), render source; otherwise conceal delimiters and style content. codemirror-live-markdown: "any intersection shows source." Obsidian reveals per-node, not per-line; Bear/Lettera behave the same; Typora reveals per inline-span/per-block.
2. **Conceal without layout collapse.** Naïve `display:none` or replace-decorations cause line-height jumps and cursor drift. Proven tricks: animate delimiter `max-width: 0 → 4ch` for inline marks; shrink block markers to `font-size: 0.01em` instead of removing them (keeps DOM/line metrics stable); Atomic Editor's headline design goal is "**no layout shifts** — each line keeps a stable height regardless of cursor position."
3. **Don't rebuild decorations mid-gesture.** Suppress reveal recomputation during mouse drag ("drag-selection skip"/Atomic's "mouse-freeze guard") or the moving reveal boundary fights the selection and causes flicker/cursor-drift.
4. **Don't touch the composition range** during IME (see §3), or composition aborts/duplicates.
5. **Narrow invalidation.** Atomic Editor rebuilds decorations only for lines whose content changed — "O(change size) even in large documents" — plus virtualized rendering; codemirror-live-markdown caches math/table positions and batch-renders KaTeX via `requestIdleCallback` with per-formula caching.
6. **Widget islands with `ignoreEvent()` boundaries** for tables/images/math: clicks inside the rendered widget stay in the widget's own editing UI; clicking its edge/outside re-materializes source. Obsidian 1.5's table editor is the mature version of this.

---

## 5. Native apps: what's publicly known

**Typora** ([site](https://typora.io)). Closed-source Electron app by Abner Lee; the archetype of "instant render" — a contenteditable DOM driven by real-time AST parsing, merged edit/preview ([overview](https://rywalker.com/research/typora), [HN discussion](https://news.ycombinator.com/item?id=21461174)). Editing model is closer to WYSIWYG-over-markdown than decoration-over-text: the rendered DOM is the working state; markdown source is regenerated. Consequences visible in its support docs and tracker: **source fidelity was historically imperfect** — auto-prettifying tables, renumbering ordered lists, whitespace normalization — enough that users on git complained, and Typora later "no longer automatically prettif[ies] tables or renumber[s] ordered list items," added a "prettify" opt-in and a **Strict Mode** toggle for parser behavior ([whitespace/line-break docs](https://support.typora.io/Line-Break/), [Strict Mode](https://support.typora.io/Strict-Mode/), [typora-issues#1316](https://github.com/typora/typora-issues/issues/1316)). This is the clearest public case study of the WYSIWYG round-trip fidelity tax.

**Bear 2 / Panda / Lettera (Shiny Frog).** Bear 2's editor (codename **Panda**, previewed 2021) was "completely rewritten from pixel to bit" as a **custom native text engine** to support "Markdown hiding, tables, nested styles, and folding" on macOS/iOS/iPadOS ([Panda check-in](https://blog.bear.app/2021/06/checking-in-on-panda-the-next-editor-for-bear/), [Bear 2 launch, July 2023](https://blog.bear.app/2023/07/bear-2-is-here/)). The team was candid about difficulty: Markdown "wasn't designed for integrated editing… seemingly simple actions like selecting text and making it bold [are] very complex to program when considering all edge cases" ([2022 retrospective](https://blog.bear.app/2023/01/the-bear-team-looks-back-at-2022-and-forward-to-23/)). It took ~4 years from Panda preview to Bear 2. **June 18, 2026: the engine shipped standalone as "Lettera,"** a native macOS markdown file editor — "Markdown syntax hides when you are not editing, so you can write and preview in one place," CommonMark, tables/math/attachments, folder & file modes ([announcement](https://blog.bear.app/2026/06/introducing-lettera-a-native-markdown-editor-for-mac-now-in-beta/)). Notably Bear stores notes in a database (markdown *text* in SQLite), while Lettera edits `.md` files directly — same hybrid text-truth engine either way. Apple-only: the custom engine has never crossed to Android/Windows/web.

**iA Writer** ([ia.net/writer](https://ia.net/writer)). The purist pole: syntax is **styled but never hidden** (headers/bold markers remain visible; formatting applied inline), plus a separate parts-of-speech "Syntax Highlight" feature ([docs](https://ia.net/writer/support/editor/syntax-highlight)). Parser internals are proprietary (native per-platform implementations). Strategically important 2025 datapoint: **iA killed its Android app**, citing Google Play/Drive policy churn but also that "developing for Android, you navigate an asteroid field. Bugs surface across thousands of device types, Android versions, and flavors…" plus very low Android conversion ([Thurrott](https://www.thurrott.com/mobile/android/310882/ia-writer-abandons-android-citing-google-play-policy-changes), [Android Police](https://www.androidpolice.com/popular-writing-app-goes-offline-on-android-after-struggles-with-google/)). Cross-platform native text editing is expensive enough that even a top-tier vendor retreated.

**Ulysses.** WYSIWYM on its own **Markdown XL** dialect (28 tags); text-with-meaning is the source of truth, but stored in a library, not portable .md files, and some constructs (links, footnotes, images) collapse into token "buttons" rather than plain text ([Markdown XL docs](https://help.ulysses.app/en_US/dive-into-editing/markdown-xl)). Apple-only; a warning about dialect lock-in: Markdown XL round-trips imperfectly to standard markdown.

---

## 6. Web/OSS landscape

- **marktext** — Electron, custom "Muya" engine, Typora-like realtime rendering; **effectively abandoned** (last real release March 2022; ["Has this project been abandoned?" #3597](https://github.com/marktext/marktext/issues/3597)). Cautionary tale: a bespoke DOM-based hybrid engine without institutional backing rotted.
- **Zettlr 3.x** — rebuilt on **CodeMirror 6**; maintainer called the CM6 API "a much better API and a godsent AST," though it required rebuilding "the complete editor from the ground up" ([Zettlr 3.0.0 release notes](https://www.zettlr.com/post/zettlr-300-released), [PR #3776](https://github.com/Zettlr/Zettlr/pull/3776)). Now does partial LP (rendering links/images/citations inline) over text-truth.
- **HackMD/CodiMD** — collaborative editing keeps the **split-pane** model; CodiMD 2.6.0 (June 2025) still bumps **CodeMirror 5** (5.65.8) ([release notes](https://github.com/hackmdio/codimd/releases)); WYSIWYG has been a request since 2017 without shipping ([#375](https://github.com/hackmdio/codimd/issues/375)). Split-pane survives where collaboration + plain-text CRDT/OT simplicity dominates.
- **Vditor / Lute / SiYuan** — [Vditor](https://github.com/Vanessa219/vditor) ships three modes in one component: WYSIWYG, **IR ("instant rendering," explicitly Typora-like)**, and split view, powered by **Lute**, a structured markdown engine in Go (compiled to JS) with a kramdown-ish AST. The same team's [SiYuan](https://github.com/siyuan-note/siyuan) went full **block WYSIWYG** (Protyle editor, `.sy` block format, Go kernel) — i.e., the team that built the best OSS Typora clone chose the AST/block model when building a Notion-class product.
- **Toast UI Editor** — dual-mode (markdown pane ↔ WYSIWYG) with mode switching; they wrote **ToastMark**, a custom markdown parser with source-position info specifically to sync panes and support live features ([tui.editor](https://github.com/nhn/tui.editor), ["The Need For A New Markdown Parser and Why"](https://toastui.medium.com/the-need-for-a-new-markdown-parser-and-why-e6a7f1826137)). Maintenance has slowed notably since ~2023.
- **Milkdown/Crepe** — "plugin driven WYSIWYG markdown editor framework… inspired by Typora, built on ProseMirror and Remark" ([repo](https://github.com/Milkdown/milkdown)); the healthiest OSS WYSIWYG-over-markdown option, but AST-truth: raw markdown is regenerated via remark serialization. The codemirror-live-markdown authors explicitly note ProseMirror/Tiptap "resist Live Preview since they don't store raw Markdown."
- **Inkdrop** — commercial notes app on CM6 + a lezer-markdown fork ([craftzdog/lezer-markdown](https://github.com/craftzdog/lezer-markdown)); author reports "Lezer is super fast and more versatile to support new context-aware features" ([roadmap vol.6](https://www.devas.life/the-roadmap-of-inkdrop-vol-6/)).
- **Terminal world (strong 2024–26 signal):** [tree-sitter-markdown](https://github.com/tree-sitter-grammars/tree-sitter-markdown) (two-grammar design: block grammar + separate inline grammar injected via `set_included_ranges`; self-admittedly "not recommended… where correctness is important" — built for highlighting) powers **hybrid live preview inside Neovim**: [render-markdown.nvim](https://neovimcraft.com/plugin/MeanderingProgrammer/render-markdown.nvim/) and [markview.nvim](https://neovimcraft.com/plugin/OXY2DEV/markview.nvim/) ("hybrid editing mode — edit and preview at the same time") both very popular. Concealment-over-text is now expected even in TUIs — the model is substrate-independent.
- **Typst ecosystem** — not markdown, but the premier "Rust core + text source + live render" precedent: incremental **reparse of edited segments with local span renumbering**, memoized evaluation via **comemo**, millisecond recompiles feeding a CodeMirror-6-based web editor and WASM builds ([architecture doc](https://github.com/typst/typst/blob/main/docs/dev/architecture.md), [typst.ts](https://github.com/Myriad-Dreamin/typst.ts)). Typst's UX is split-view rather than inline concealment, but it validates the Rust-core/incremental/plain-text architecture at production quality.

---

## 7. 2024–2026 newcomers and direction of travel

**Toward hybrid text-truth:**
- **Lettera** (Shiny Frog, June 2026) — Panda engine unbundled as a pure markdown-file editor ([announcement](https://blog.bear.app/2026/06/introducing-lettera-a-native-markdown-editor-for-mac-now-in-beta/)).
- **Atomic Editor** (2026, [Show HN](https://news.ycombinator.com/item?id=48345201), [repo](https://github.com/kenforthewin/atomic-editor)) — reusable npm package: "Obsidian-style inline live preview" for CM6 with raw markdown as source of truth, WYSIWYG tables, wikilinks, narrow invalidation, no-layout-shift design. The reveal playbook is now commoditized as libraries; also [codemirror-live-markdown](https://github.com/blueberrycongee/codemirror-live-markdown) with its unusually detailed public design doc, and even a Swift+CM6 macOS sticky-notes app ([markdown-sticky-notes](https://github.com/jaesuny/markdown-sticky-notes)).
- **Haptic** (Sept 2024, [repo](https://github.com/chroxify/haptic)) — local-first, web/Tauri markdown-file notes.
- **Ferrite** ([repo](https://github.com/OlaProeis/Ferrite)) — Rust: egui + **ropey** rope + **comrak** parser, live-preview markdown editing; small but proof the stack composes in pure Rust.

**Toward AST/block/database (the counter-current):**
- **Logseq** split into two products because dual-maintaining markdown files vs database "slowed development… every feature… considered twice"; the flagship is now the **database version**, markdown relegated to "Logseq OG" ([official announcement](https://logseq.io/page/b2ad9ce1-9cb7-4436-8083-54cb4516d324/df4dc09d-0a12-4c87-904e-22a9bf4c350a), [discussion](https://discuss.logseq.com/t/why-the-database-version-and-how-its-going/26744)).
- **AppFlowy** — Flutter block editor with a node/delta document model; markdown is an import/export **codec** (two-level block+inline decoder), not the source of truth ([appflowy-editor docs](https://deepwiki.com/AppFlowy-IO/appflowy-editor/7.2-markdown-codec), [Flutter+Rust architecture](https://appflowy.com/blog/tech-design-flutter-rust)). Its Rust backend does storage/sync, **not** text editing.
- **Reor** (AI notes) picked **BlockNote** (ProseMirror block WYSIWYG) over a markdown editor ([repo](https://github.com/reorproject/reor)); Anytype, Capacities, SiYuan likewise block-model.

**Reading:** tools whose identity is *interoperable files* converge on hybrid LP; tools whose identity is *structured workspace/database* abandon markdown-as-truth entirely. The middle (WYSIWYG editing of .md files, à la Typora/marktext) is being squeezed — marktext died, Typora is stable-but-slow-moving, while Obsidian/Bear/Lettera and the CM6 library ecosystem grow.

---

## 8. Key conclusions

### Hybrid (text-truth + decorations) vs WYSIWYG (AST-truth)

| Dimension | Hybrid live preview | WYSIWYG / AST |
|---|---|---|
| Markdown fidelity | **Trivially lossless** — you never serialize; byte-exact files, clean git diffs | Round-trip tax: normalization churn (Typora's table prettify/list renumber history), dialect drift, comments/HTML/frontmatter edge cases |
| Editing UX | Power users love it; syntax flash on reveal can feel "jumpy"; layout-shift risk; ambiguous cursor positions at conceal boundaries | Smoothest for non-technical users; structural ops (drag blocks, table cells) natural; but "markdown-invisible" states confuse markdown-literate users |
| Implementation complexity | Parser + decoration mapping + reveal rules + widget islands. Hard parts: layout stability, IME interaction, tables. Now **well-documented territory** (Obsidian, Atomic, codemirror-live-markdown) | Schema design, transactions/steps, node views, serialization, position mapping — ProseMirror-class machinery; every feature touches the schema |
| IME | Text-truth means you can defer *cosmetic* decoration updates during composition without touching the document; platform-native input stacks work directly on text | AST editors intercept/transact input; composition across node boundaries is notoriously hard (ProseMirror carries years of IME workarounds) |
| Mobile | Inherits platform text-input behavior; Android composition still the pain point; reveal-on-tap targets are small | Toolbars/blocks are touch-friendlier; but contenteditable-based WYSIWYG on Android is its own bug farm |
| Collaboration | Plain-text CRDT/OT (mature, simple: Yjs Text, diamond-types, Loro) | Rich-text CRDT (Peritext-class) or schema-aware OT — much harder |
| Non-markdown features | Hard ceiling: anything must round-trip through text (or widget islands) | Unlimited (and that's how you drift from markdown) |

### How CM6/Lezer keeps large docs fast
Three-layer strategy: (1) **block-granular incremental parsing** — lezer-markdown reuses unchanged block subtrees as fragments, deliberately dropping whole-document semantics (link-ref validation) to remain single-pass; (2) **compact trees** — packed 64-bit-per-node buffers, cheap to build/discard; (3) **viewport-limited work** — only visible content (+margin) is rendered/decorated, with height estimation for the rest; decoration `RangeSet`s are *mapped* through edits rather than recomputed.

### Is hybrid simpler and more robust for a Rust core? (Yes, with known losses)
**Proposed shape:** rope (ropey/crop) + block-incremental markdown parser + decoration-span emitter → per-platform views (CM6-style web view; UITextView/TextKit 2 attributes + concealment on iOS; Android EditText/Spannable or custom; desktop native or shared).

Why it's simpler/more robust than a WYSIWYG AST core:
1. **No serialization layer at all** — the file is the state; fidelity, git-friendliness, and interop are free (the Typora fidelity saga simply cannot happen).
2. **The core's write API is just text edits** (`replace(range, str)`), so every platform's *native* input stack — including IME, autocorrect, dictation — can drive it directly. Marijn's central IME lesson (don't disturb the composition; treat input as observed text change) maps perfectly onto a text-truth core, whereas an AST core must transact input events. This is the single biggest robustness win.
3. **Decorations are derived, disposable state.** If decoration mapping ever bugs out, you re-derive from the parse; the document can't be corrupted. In AST editors, editing bugs corrupt the document.
4. **Collaboration = plain-text CRDT** (diamond-types, Loro, yrs) instead of Peritext-class rich-text CRDTs.
5. **Prior art de-risks each piece:** Typst proves Rust incremental-parse + span-stability + WASM at scale; tree-sitter-markdown proves the two-pass block/inline incremental grammar shape (though its accuracy caveats argue for a **lezer-markdown-style hand-written block-incremental parser in Rust** — pulldown-cmark/comrak/markdown-rs give offsets but are not incremental, so plan to write this; it's the same well-understood algorithm lezer-markdown implements in ~few kLOC).

What would be lost / must be mitigated:
- **Structured-block UX needs escape hatches:** cell-level table editing, image resize, embeds require widget islands with bidirectional source mapping (Obsidian 1.5 shows the pattern and the cost — it took them 4+ years to ship tables well).
- **Document-global features** (footnote/link-ref resolution, numbered-heading counters) fight block-local incrementality — budget a cheap global "link/footnote index" pass, or accept Obsidian-style gaps.
- **No non-markdown formatting, ever** (colored text, arbitrary nesting) — a feature-ceiling decision to make explicitly; extensions must be syntax (Obsidian's callouts, `==highlight==`) rather than schema.
- **Reveal UX polish is genuinely hard:** layout stability, cursor behavior at conceal boundaries, gesture/IME freezing — the playbook exists (§4) but each platform view must reimplement it; per-platform text stacks (TextKit 2, Android spans) have far less community lore for concealment than CM6 does. Android remains the industry-wide weak spot — treat it as the primary risk and test CJK composition from day one.
- **Position mapping discipline** in the core: decoration spans, cursors, and collaborative metadata all need offset-mapping through edits — see [Marijn's 2025 analysis](https://marijnhaverbeke.nl/blog/addressing-editor-content.html) of offsets vs stable IDs before choosing.

**Bottom line:** the hybrid model turns the hardest problems of a cross-platform editor (fidelity, IME, collaboration, corruption) into non-problems, and concentrates the remaining difficulty in one place — decoration/reveal polish per platform view — which is exactly where a thin-view/fat-Rust-core architecture wants the difficulty to live. The market (Obsidian's dominance, Bear→Lettera, the CM6 library boom) says this is also the model markdown users choose.
