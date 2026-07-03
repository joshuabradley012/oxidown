# Native Mobile Text-Editing Stacks (iOS / Android)

> Research compiled July 2026 for the Oxidown plan refresh.

## Executive summary

- **iOS**: The WWDC 2025 rich-text `TextEditor` (iOS 26) is real and surprisingly capable for *inline* formatting — including custom `AttributedString` attributes — but it has no story for block elements (code blocks, list UI, images/attachments) or custom layout. For a serious markdown hybrid editor, **UIKit `UITextView` on TextKit 2 with an `NSTextStorage` bridge** is still the realistic surface, and WWDC 2026 added extensibility APIs squarely aimed at that use case. TextKit 2 works but has documented viewport/height-estimation bugs even in 2025-2026.
- **Android**: The state-based `BasicTextField` (ex-BasicTextField2, stable since ~Aug 2025) plus `OutputTransformation`/`TextFieldBuffer.addStyle()` is a genuinely good foundation for in-place markdown styling; inline images inside the field remain unsupported, pushing toward a block-based layout for embeds. IME/soft-keyboard behavior is the #1 risk and the reason Google rewrote its own text field.
- **Architecture**: "Native text views + Rust core as source of truth" is *proven possible* (Anytype ships exactly this pattern with a Go core; AppFlowy with Rust; Meta's lexical-ios uses the same reconciler idea) but it is expensive: the shared core unifies the model/parser/sync, **not** the two hard platform-specific editor UIs. The pragmatic mobile route (WebView + CodeMirror 6/ProseMirror) is what Obsidian, Joplin, and Notion's editor still use in 2026 — and users notice (Notion). Text-first markdown apps that win on feel (Bear, iA Writer) are native.

---

## Part 1 — iOS / macOS

### 1.1 SwiftUI TextEditor + AttributedString (iOS 26, WWDC 2025)

What shipped ([WWDC25 session 280](https://developer.apple.com/videos/play/wwdc2025/280/), [Apple sample](https://developer.apple.com/documentation/swiftui/building-rich-swiftui-text-experiences)):

- `TextEditor` now binds to `AttributedString` (iOS/iPadOS/macOS/visionOS 26), with built-in formatting UI, keyboard shortcuts, Markdown-aware `AttributedString` construction.
- **Selection**: new `AttributedTextSelection` binding; selections expose `typingAttributes`. Mutations go through `transformAttributes(in:)` with the selection passed `inout` because **any mutation invalidates all `AttributedString` indices** — stale indices give undefined behavior.
- **Custom attributes**: fully supported — custom `AttributedStringKey`s + [`AttributedTextFormattingDefinition`](https://developer.apple.com/documentation/swiftui/attributedtextformattingdefinition) with value constraints. Exactly the hook for markdown-semantic attributes.
- Live restyling as you type is workable (real-time pattern-detector demos exist).

**Hard walls for a markdown editor**: no text attachments / inline images / embedded views (Genmoji only); no block-element machinery (code-block backgrounds, list continuation-on-return, checklists, tables, drag handles); no layout-manager access from SwiftUI; min-deployment iOS 26; no edit-interception granularity ("user hit return inside a list item"). **Verdict**: fast prototype surface or comment-box-grade editing; not the flagship editor surface yet.

### 1.2 TextKit 2 + custom content storage

- **The abstraction is a trap in practice**: `NSTextContentManager` looks pluggable, but `UITextView`/`NSTextView` require an `NSTextStorage`-backed `NSTextContentStorage`; plugging a custom content manager into framework text views crashes/is unsupported ([Apple forums 690859](https://developer.apple.com/forums/thread/690859), [STTextView #79](https://github.com/krzyzanowskim/STTextView/discussions/79)).
- **Maturity, per the people who use it hardest**: Marcin Krzyżanowski (STTextView author), Aug 2025: unstable scrolling, unreliable `usageBoundsForTextContainer` height estimates, fragile end-of-document workarounds, common regressions — Apple's own TextEdit exhibits the same glitches. His conclusion after 4 years: TextKit 2 "might not be the best tool… especially when it comes to text editing UI" ([blog](https://blog.krzyzanowskim.com/2025/08/14/textkit-2-the-promised-land/)).
- **WWDC 2026 moved things forward for extending framework text views** ([session 370](https://developer.apple.com/videos/play/wwdc2026/370/)): public `NSTextViewportLayoutControllerDelegate` conformance on UITextView/NSTextView; `NSTextViewportRenderingSurface` protocols for per-fragment rendering views; text-attachment view-provider **reuse policies** (`.onEditingInlineParagraphs`, `.onScrollingOutOfViewport`) so inline attachment views survive edits/scrolling; enumeration-skipping for collapsed sections. Demos: line numbers, collapsible sections — the primitives a markdown editor needs for gutters, code-block chrome, stable inline images, folding. Custom content storage still not addressed.

**Implication**: you will not point TextKit at a Rust rope directly. The proven pattern is an `NSTextStorage` subclass acting as a mirror/proxy of the Rust document (apply Rust deltas → `edited(_:range:changeInLength:)`), keeping TextKit's buffer as the IME-facing copy.

### 1.3 lexical-ios (Meta)

Swift port of Lexical's editor-state/reconciler philosophy on top of TextKit; MIT; pre-release, used in production at Meta (Workplace); effectively **dormant as an OSS project** (last release 0.2 Nov 2023; community fork actively diverging). Lessons: (a) "external editor-state as source of truth + reconciler that diffs into NSTextStorage" ships at Meta scale — validation for a Rust-core-reconciles-into-native-view design; (b) don't depend on the repo unless prepared to own it.

### 1.4 Library landscape

| Library | What it is | 2026 status | Fit |
|---|---|---|---|
| [Runestone](https://github.com/simonbs/Runestone) | Plain-text editor framework (iOS), tree-sitter highlighting, custom line layout | Active | Source-mode markdown; not rich/WYSIWYG |
| [STTextView](https://github.com/krzyzanowskim/STTextView) | TextKit 2-native text view replacement, plugin system | Active | Best open reference for TextKit 2 editing internals |
| [lexical-ios](https://github.com/facebook/lexical-ios) | Lexical-style editor on TextKit | Dormant (fork active) | Architecture reference |
| [swift-markdown-ui](https://github.com/gonzalezreal/swift-markdown-ui) | SwiftUI markdown rendering | Maintenance mode | Rendering only |
| [Textual](https://github.com/gonzalezreal/textual) | SwiftUI rich-text rendering engine (MarkdownUI successor): attachments, selection, custom `MarkupParser` | Active, v0.5.0 | Rendering/preview only |
| [swift-markdown](https://github.com/swiftlang/swift-markdown) + swift-cmark | Apple's GFM parser w/ source ranges | Active | Fine, but the parser lives in the Rust core |

### 1.5 How serious iOS markdown editors are built

- **Bear (Shiny Frog)**: fully native; built an entirely new text-editing system (Panda) — a custom markdown editor with syntax concealment; multi-year effort. Even a best-in-class team treated this as *building a text-editing system*, not integrating a library.
- **iA Writer**: native text views; deliberately source-markdown-with-highlighting (no concealment), sidestepping most hybrid complexity.
- **Craft**: native Apple-stack, own "native engine," block-based.
- **Obsidian mobile**: **not native** — Capacitor wrapper around the web app; editor is customized CodeMirror 6 live preview. CM6 is repeatedly cited as "one of the only web editors that works decently on mobile."
- Pattern: **text-first markdown apps go native with syntax concealment on TextKit; block/database apps go web-editor-in-native-shell.**

---

## Part 2 — Android

### 2.1 State-based BasicTextField in 2026

- **Why it exists**: the old `value`/`onValueChange` field had unfixable sync problems — any asynchrony between the IME's edit and state application caused dropped/duplicated text, cursor jumps ([Effective state management for TextField](https://medium.com/androiddevelopers/effective-state-management-for-textfield-in-compose-d6e5b070fbe5), [migration guide](https://developer.android.com/develop/ui/compose/text/migrate-state-based)).
- **Now**: `BasicTextField(state:)` with `TextFieldState` is stable. Edits are synchronous via `state.edit { }` on a `TextFieldBuffer`; built-in undo/redo; state can live in a ViewModel.
- **`OutputTransformation`** formats text for display *without touching the underlying state* (automatic offset mapping — the classic `VisualTransformation.offsetMapping` crash factory is gone), and since Aug 2025 supports `TextFieldBuffer.addStyle(SpanStyle/ParagraphStyle)` — paint markdown styling over the buffer declaratively ([What's new in Compose, Aug '25](https://android-developers.googleblog.com/2025/08/whats-new-in-jetpack-compose-august-25.html)). `InputTransformation` intercepts edits pre-commit (list continuation, smart pairs).
- **Hard limitation — inline content/images**: `InlineTextContent` works in `Text`, **not** in `BasicTextField`. Consequence: images/embeds must be separate composables between text blocks (block-editor layout) or custom-drawn.
- **Verdict**: solid for a styled-markdown editing surface (single buffer, span styling, transformations, undo); *not* a rich-document widget. You own paragraph/block chrome (code-block backgrounds via `drawBehind`, list gutters).

### 2.2 compose-rich-editor (Mohamed Rejeb)

Rich text editor for Compose Multiplatform (Android/iOS/Desktop/Web); `RichTextState`; HTML and Markdown import/export; 1.0.0-rc14, actively developed into April 2026 ([repo](https://github.com/MohamedRejeb/compose-rich-editor)). Best off-the-shelf Compose starting point and a good architecture reference — but WYSIWYG, not markdown-hybrid; still RC; you'd fork/own it for a flagship product.

### 2.3 Markwon and the View world

[Markwon](https://github.com/noties/Markwon) remains the standard for **rendering** markdown as native `Spanned` on `TextView` (commonmark-java, no WebView). Its editor module does highlight-as-you-type on `EditText` — syntax highlighting, not concealment. Slow release cadence; View-based, not Compose. [Markor](https://github.com/gsantner/markor) proves the native route: custom syntax highlighter over EditText, source-mode + preview — nobody does full concealment natively on Android at quality.

### 2.4 What serious Android markdown editors do

- **Obsidian**: Capacitor WebView + CodeMirror 6.
- **Joplin**: React Native shell, editor = CodeMirror 6 in a WebView, shared between desktop and mobile.
- **Notion**: native Kotlin shell, **editor still a WebView** — "most of Notion's apps are fully native, save for the editor" ([Pragmatic Engineer](https://newsletter.pragmaticengineer.com/p/notion-going-native-on-ios-and-android)).
- **iA Writer**: abandoned Android entirely in 2023 — "developing for Android, you navigate an asteroid field."

### 2.5 IME / soft-keyboard pitfalls — concrete guidance

1. **The composing region is sacred.** IMEs maintain an in-flight composition independent of selection; if your editor rewrites text or moves selection while composition is active, keyboards misbehave (canonical failure: composed text re-duplicated per keystroke). Buffer model-driven restyling until composition ends, or restrict changes to spans that don't alter text content.
2. **Never let model→view sync be asynchronous.** This is precisely why Google rewrote `BasicTextField`. If a Rust core is authoritative, apply edits synchronously to the native buffer first, then reconcile with the core — never round-trip the keystroke through the core before the view updates.
3. **If you build a custom view**: subclass `BaseInputConnection`, never implement `InputConnection` raw; replicate `EditText` semantics exactly. Expect keyboards to violate the contract differently (Gboard vs Samsung vs SwiftKey on `deleteSurroundingText`, batch edits, backspace-as-key-event).
4. **Styling must not change text**: `OutputTransformation` deliberately keeps `TextFieldState` untouched — the correct mental model for concealment-style styling (decorations are view-side; the IME sees raw text).
5. **Test matrix is a product requirement**: Gboard + Samsung + SwiftKey × autocorrect on/off × voice input × Korean/Japanese/Chinese composition × OEM skins. Budget continuous device-farm testing.
6. Prefer stock `BasicTextField` internals (it owns the `InputConnection`); avoid focus hacks that restart input.

---

## Part 3 — Is "native text views + Rust core as source of truth" realistic?

**Yes — with the right division of labor. It is the expensive, high-ceiling option.**

**Proof it ships:**
- **Anytype**: shared Go core ("anytype-heart") + fully native Kotlin/Swift clients including native block editors. Substitute Rust+UniFFI for Go+gRPC and this is the blueprint.
- **AppFlowy**: Rust core (data, persistence, CRDT) + Flutter UI — shared-core validation, though not native text views.
- **lexical-ios at Meta**: external editor-state reconciled into TextKit ships in Meta apps.

**Hard-won lessons:**
- **xi-editor's retrospective**: embed the Rust core **in-process and synchronous**, not as a service; keep the keystroke path native.
- **The native text widget must own the hot buffer.** On both platforms the IME talks to the platform widget; the Rust core is the authoritative *document*, reconciled by diffs outside composition windows. iOS: `NSTextStorage` proxy under `UITextView`. Android: `TextFieldState` mirror + `OutputTransformation` styling; images/tables as sibling composables.
- **Budget honestly**: the shared core covers parsing, document model, sync, search, undo semantics — perhaps 40% of the editor problem. Concealment logic, selection semantics, toolbars, IME hardening, and block chrome must be built per platform. Bear needed a multi-year custom system for *one* ecosystem.

**The pragmatic alternative** (most of the market in 2026): WebView editor in native shell (Obsidian, Joplin, Notion; Evernote's unified editor). CodeMirror 6 was explicitly engineered for mobile IME survival — years of composition war stories already paid for. Costs: startup/memory overhead, non-native feel (scroll physics, selection handles, context menus), permanent performance-complaint tax. Middle road: Compose Multiplatform stable on iOS since 1.8 (May 2025) — one editor UI + Rust core, at the cost of a non-native text stack on iOS.

**Bottom line**: if the product's identity is *the editing feel* (Bear/iA class), native views + Rust core is realistic and differentiating — plan for UITextView/TextKit2 + state-based BasicTextField, block-based layout for embeds, and a large permanent IME/QA line item. If the identity is features/blocks/collab breadth, ship the web editor in the shell first and go native surface-by-surface — the Notion/Evernote path, with its known ceiling.
