# Oxidown Boundary Protocol — v0 (M0 spike)

The minimum contract between `oxidown-core` and a platform view, for the M0 spike (plan.md §8).
This document is authoritative: if the Rust core and the TypeScript view disagree, the one that
matches this file wins; if this file is wrong, change it in the same PR as the code.

Current contract version: **v0.6** — the base v0 sections below are followed by the v0.1
clarifications, the v0.2 (M1) additions, and inline v0.3/v0.4/v0.5/v0.6 amendments; the "v0.3
changelog" through "v0.6 changelog" sections at the end enumerate exactly what each version
comprises.

**Testing strategy.** The Rust/wasm core (`crates/oxidown-wasm`, wrapped by
`packages/oxidown-view-cm6/src/wasm-core.ts`) is the ONLY implementation of this contract — there
is no longer a second, hand-maintained TypeScript reference core. Every test that asserts
CONTRACT BEHAVIOR (decorations, reveal, commands, numbering, undo/redo/coalescing, streaming,
anchors, composition) runs directly against the real wasm core, loaded Node-side in vitest via
`initSync` over the built `.wasm` bytes (`packages/oxidown-view-cm6/test/wasm-loader.ts`); those
tests fail loudly if `crates/oxidown-wasm/pkg` hasn't been built, rather than skip. A separate,
deliberately dumb `StubCore` (`packages/oxidown-view-cm6/test/stub-core.ts` — a plain text buffer
with no markdown knowledge, whole-text-snapshot undo, and scriptable per-method hooks) exists only
for the CM6 view's WIRING tests — change forwarding, skip annotations, desync recovery, keymap
fallback, and the like — where a fast, fully-scriptable double is more useful than a real parser.
Earlier drafts of this document and its test suite used a third option, a hand-written `MockCore`
that reimplemented this whole contract in TypeScript; it was retired (pre-1.0) once the wasm core
was fast and stable enough for every test to depend on it directly — a from-scratch parity
implementation was pure double-implementation tax, and its behavior drifted from the authoritative
core more than once (e.g. `***x***` nesting, below).

Web-boundary flavor: **all positions in this protocol are UTF-16 code units** (CodeMirror's unit).
The conversion to core-internal UTF-8 byte offsets happens inside `oxidown-core` itself — every
public `Editor` entry point converts on the way in and out (editor.rs `utf16_to_byte*` /
`byte_to_utf16`); the wasm crate passes UTF-16 positions through unchanged. Core internals never
leak bytes.

## Model (restating plan.md §4 invariants for this seam)

1. The view's text buffer (the CM6 doc) is the IME-facing buffer. The core mirrors it exactly.
2. Every text change in the view is forwarded to the core as splices, synchronously, in order.
   There is no other write path: the core never mutates text on its own initiative.
3. `undo`/`redo` are core-driven: the core returns splices that the view must apply verbatim to
   its buffer as a non-history transaction (CM6 history is disabled entirely).
4. All derived output (decorations) is requested by revision. If the view asks with a stale
   revision the core throws/errors; the view catches up and re-asks. No silent staleness.
5. Between `compositionBegin` and `compositionEnd`, decoration output must be *stable* for any
   span intersecting the composition range: no new conceal spans may appear inside it, and
   existing conceal spans inside it must be emitted as `delim` marks instead (revealed).

## TypeScript interface (authoritative shape)

```ts
/** Positions are UTF-16 code units into the current document unless stated otherwise. */
export interface Splice {
  /** Position in the document BEFORE this edit batch (original-doc coordinates). */
  at: number;
  /** Number of code units to delete starting at `at`. */
  delete: number;
  insert: string;
}

export type EditOrigin = "user" | "ime" | "paste" | "undo" | "redo";

export interface DecorationMark {
  kind: "mark";
  from: number;
  to: number;
  style: "strong" | "em" | "code" | "delim";
}
export interface DecorationConceal {
  kind: "conceal";
  from: number;
  to: number; // delimiter chars to conceal — they stay in the DOCUMENT (see "Rules for the view")
}
export interface DecorationLine {
  kind: "line";
  /** Position anywhere on the target line (view resolves to the line). */
  at: number;
  style: "h1" | "h2" | "h3" | "h4" | "h5" | "h6";
}
export type Decoration = DecorationMark | DecorationConceal | DecorationLine;

export interface SelectionRange { anchor: number; head: number; }

export interface OxidownCore {
  /** Create/replace the document. Returns revision 0's successor. */
  load(text: string): number;

  /**
   * Apply an edit batch. `splices` are non-overlapping, ascending, in original-doc coordinates
   * (CM6 ChangeSet semantics). Returns the new revision. Must be O(edit + dirty block), not O(doc).
   */
  applyEdit(baseRevision: number, splices: Splice[], origin: EditOrigin): number;

  /**
   * Core-driven history. Returns splices in CURRENT-doc coordinates for the view to apply,
   * plus the resulting revision, or null if the stack is empty.
   * Coalescing: consecutive `user`/`ime` edits within 500ms and adjacent positions group
   * into one undo unit; `paste` always breaks the group. Coalescing pauses during composition.
   * Depth (v0.4 amendment): the undo stack holds at most 100 units — pushing the 101st
   * silently drops the oldest (matching CM6's own default). The redo stack needs no cap
   * (bounded by undo).
   */
  undo(): { revision: number; splices: Splice[] } | null;
  redo(): { revision: number; splices: Splice[] } | null;

  /**
   * Decorations for the viewport [from, to), computed against `revision` (must be current).
   * Reveal is computed CORE-SIDE: any selection range (a cursor is an empty range) that
   * intersects a formatted node's full extent — including its delimiters — causes that node's
   * `conceal` spans to be omitted and its delimiters emitted as `mark:delim` instead.
   */
  decorations(
    revision: number,
    from: number,
    to: number,
    selections: SelectionRange[],
  ): Decoration[];

  /** IME session. Range in current-doc coordinates; may grow as composition updates arrive. */
  compositionBegin(from: number, to: number): void;
  compositionEnd(): void;

  /** Debug/verification. */
  getText(): string;
  docLength(): number; // UTF-16 code units
  revision(): number;
}
```

## M0 markdown scope

The parser may understand more, but only these emit decorations in M0:

| Construct | Decorations emitted (concealed state) |
|---|---|
| ATX heading `#`–`######` | `line` with `h1`..`h6` on the heading line; `conceal` over the leading hashes **and the following space** |
| Strong `**x**` / `__x__` | `mark:strong` over content; `conceal` over each delimiter pair |
| Emphasis `*x*` / `_x_` | `mark:em` over content; `conceal` over delimiters |
| Inline code `` `x` `` | `mark:code` over content; `conceal` over the backticks |

Revealed state replaces each of that node's `conceal` spans with `mark:delim`.
Nesting (`**bold *italic***`) must work; reveal applies per-node, not per-line.

## Rules for the view (from research/01 §4 — the anti-flicker playbook)

- Concealment is a rendering choice; what is protected are these invariants *(restated as a
  v0.3 amendment — the original rule mandated visual collapse, e.g. `font-size: 0.01em` +
  `letter-spacing`, and forbade removing characters from the DOM)*:
  1. the **document text is the source of truth** — concealed characters remain in the
     document (copy, positions, and edits all see them; the view never mutates text to hide it);
  2. **no vertical layout shift** — line heights must not change between concealed and
     revealed states;
  3. the **caret remains addressable at concealed positions** — a cursor placed inside or at
     the edge of a concealed span must have well-defined, visible coordinates.
  Replace-based concealment (e.g. CM6 `Decoration.replace`, Obsidian's mechanism) that
  preserves those invariants is explicitly permitted; the CM6 view uses it, because the
  visual-collapse CSS hack itself violated invariant 3 (hidden-but-laid-out inner text gave
  `coordsAtPos` phantom x-positions at conceal boundaries — the caret vanished or floated;
  see the rationale at `packages/oxidown-view-cm6/src/extension.ts`'s `concealDeco`).
- Do not rebuild decorations mid mouse-drag; recompute on drag end.
- Do not request/rebuild decorations while `EditorView.composing` is true; recompute on
  composition end. Pair with `compositionBegin`/`compositionEnd` calls into the core.
- Apply `undo`/`redo` splices with an annotation the change-forwarding path recognizes, so they
  are not echoed back into `applyEdit`.
- Hosts must not filter or alter oxidown-annotated transactions (CM6 `changeFilter`/
  `transactionFilter`) — those carry core-driven changes (undo/redo/command/stream) the core has
  already applied; the dev-mode mirror check (`verifyMirror`) verifies these transactions
  immediately (not only on the next forwarded edit), so a host that violates this is caught early.
- **`readOnly` (v0.4 addition):** when `EditorState.readOnly` is true, the view initiates no
  core mutations — widget interactions (checkbox clicks) are inert, and every editing keybinding
  (toggles, `setHeading`, Tab/Shift-Tab, undo/redo, task toggle) returns `false` so hosts see
  the standard CM6 convention. Core-driven changes the HOST initiates (e.g. a stream it runs
  against the same core) are outside this rule — `readOnly` gates the view's own gestures.
- **Core-driven dispatches carry `addToHistory: false` (v0.4 addition):** the view already owns
  history routing (rule above; CM6 history must not be enabled alongside oxidown), and tagging
  every `applyCoreChange` dispatch makes a host that wrongly enables CM6 history degrade
  gracefully (core changes stay out of the rogue history) instead of building a second,
  conflicting undo stack.
- **Mixed batched updates are a desync signal (v0.4 addition):** a single view update that
  batches a doc-changing NON-skip transaction together with a skip-annotated (core-originated)
  one cannot be forwarded — the user splices' coordinates predate a change the core has already
  applied. The view must treat the update as a mirror-desync emergency and recover via the
  sanitized-reload path (see "Error handling"), never forward the splices as-is.

## Performance budget (M0 gate)

`applyEdit` + `decorations` for a ~3k-code-unit viewport on a 100KB document: **< 1ms combined
p95 in the core** (excluding DOM work), measured from the JS side of the wasm boundary.

Enforcement (v0.4 note): CI gates the LOOSE ceilings (`perf_smoke`, `stream_perf` — 5-30x the
budget, sized to absorb shared-runner noise), so an order-of-magnitude regression fails CI.
The 1ms budget itself remains a local trip-wire (`perf_baseline`, informational in CI): shared
runners are too noisy to gate on sub-millisecond wall time.

**Complexity note (amended with the incremental-reparse implementation).** `applyEdit`'s
"O(edit + dirty block), not O(doc)" holds as follows: parse work is bounded by a window of
top-level blocks around the edit (one block of slack above, extended below until the fresh
parse's block boundaries realign with the old ones), plus a small-constant linear pass that
rebases the untouched suffix's cached spans and re-matches block IDs. Edits whose effect
cannot realign with any downstream block boundary — canonically, toggling a code fence open
mid-document, which reinterprets everything below it — degrade, correctness-first, to
re-parsing from the window start to the end of the document (and in rare shapes the whole
document). The fast paths are byte-equivalence-gated against a from-scratch parse
(`crates/oxidown-core/tests/reparse_equivalence.rs`); the known whole-document couplings that
a windowed parse cannot see (link reference definitions, footnote definitions) affect only
constructs M1 does not decorate — the same documented assumption as the streaming append
fast-path below.

## Clarifications (v0.1 — pinned after first implementations)

1. **Composition vs coalescing:** `compositionBegin` closes any open undo group; while composing,
   the 500ms window does not break the group; `compositionEnd` closes the group. A composition
   session is therefore exactly one undo unit.
2. **`load` revisions:** first `load` returns 1; revisions are monotonic across repeated `load`
   calls on the same instance (stale revision numbers are never re-issued).
3. **`***x***`:** per CommonMark, emphasis is the outer node (`<em><strong>x</strong></em>`).
   Views must not depend on which node owns which delimiter characters beyond what the emitted
   spans say.
4. **Undo-coalescing adjacency** *(amended in v0.3 — the original wording, "touches the
   previous edit's end position", was narrower than the pinned rule)*: a consecutive
   `user`/`ime` edit coalesces when it is a single splice falling entirely within (or touching
   the ends of) the region the top undo unit's undo would remove — which covers typing runs,
   insert-at-front, and backspace runs over just-typed text. Multi-splice batches never
   coalesce. A coalesce that shrinks the unit's inverse to a pure no-op (type a character,
   backspace it) drops the unit from history entirely.
5. **`undo`/`redo` origins** never flow through `applyEdit` (views apply history splices under a
   skip annotation). Cores must still treat them defensively as isolated non-coalescing units.
6. **Heading reveal extent** is the whole heading line: a cursor anywhere on the line reveals
   the leading hashes.
7. **Surrogate pairs:** a *splice* position inside a surrogate pair is invalid (throws —
   it would corrupt text). *Query* positions (viewport edges, selection endpoints, composition
   ranges) snap outward to the nearest code-point boundary instead of erroring — they are
   range filters, not mutations.

---

# v0.2 additions (M1)

Additive only — every v0/v0.1 rule above still holds. Views MUST ignore decoration styles and
widget kinds they don't recognize (forward compatibility).

## Expanded decoration vocabulary

```ts
// mark styles (added): "strike" | "link" | "url" | "list-marker"
//   link  = the visible text of a link
//   url   = the destination part, emitted only when the link node is revealed
//   list-marker = bullet/number markers ("- ", "1. ") — the REVEALED-state style; concealed
//                 markers use widget:bullet / widget:ordered instead (v0.3 amendment, below)
// line styles (added): "blockquote" (with depth) | "code-block" | "code-fence" | "hr"
export interface DecorationLineV2 {
  kind: "line";
  at: number;
  style: "h1"|"h2"|"h3"|"h4"|"h5"|"h6"|"blockquote"|"code-block"|"code-fence"|"hr"|"list-item";
  /** Nesting depth (1-based); present for styles "blockquote" and "list-item". */
  depth?: number;
  /**
   * "blockquote"/"list-item" only: the line is revealed (a cursor/selection
   * touches it — see "Marker reveal is LINE-level" below), so the view drops
   * the line's decorative padding/bars and renders source geometry.
   * Omitted from the wire when false.
   */
  revealed?: boolean;
}
export interface DecorationWidget {
  kind: "widget";
  from: number;
  to: number;                 // source range the widget REPLACES visually
  // v0.3 amendment: "ordered" — see "Ordered-list numbering is a computed-value
  // WIDGET" below. number/delim are present together iff widget === "ordered".
  widget: "task" | "bullet" | "ordered";
  checked?: boolean;
  number?: number;  // "ordered" only: the VIEW-COMPUTED display number
  delim?: string;   // "ordered" only: the marker's delimiter, "." or ")"
}
```

M1 emission scope (parser may understand more than it decorates):
- ATX headings — content span TRIMS trailing spaces/tabs (v0.4 amendment, matching CommonMark:
  `# foo   ` marks `foo`, not `foo   `); a closing hash run and the whitespace around it stay
  delimiter territory as before.
- Strikethrough `~~x~~` — mark `strike` + conceal/delim pairs, same reveal rules as strong/em.
- Links `[text](url)` — concealed: `mark:link` over text, conceal `[` and `](url)`.
  Revealed: delimiters as `mark:delim`, destination as `mark:url`. Autolinks: `mark:link` whole.
- Blockquotes — `line:blockquote` per line with depth; `> ` markers conceal, reveal per-line.
- Fenced code blocks — `line:code-fence` on fence lines, `line:code-block` on body lines,
  `mark:code` on body content. The raw fence text (``` + info string) conceals; reveal is
  **BLOCK-level**: a cursor/selection anywhere inside the fenced block (either fence or the
  body) reveals BOTH raw fences as `mark:delim`, so they are editable whenever the block is.
- Lists — CONCEALED ordered markers emit `widget:ordered` (v0.3 amendment — see below), replacing
  the whole marker span with a VIEW-COMPUTED display number; revealed they show as
  `mark:list-marker` (alignment is view styling: fixed-width right-aligned box + tabular
  numerals). **Unordered markers emit `widget:bullet` replacing the whole marker span (`"- "`)**,
  revealed as `mark:list-marker`. Task markers conceal (the checkbox widget represents the item)
  and reveal as `mark:delim`, dash and brackets in lockstep. **Ordered task items** (`1. [ ] a`)
  are the one carve-out from lockstep concealment: the concealed marker emits BOTH
  `widget:ordered` (the number) and `widget:task` (the checkbox), since the ordinal carries
  information the checkbox alone cannot — matching how Obsidian renders numbered task lists.
- **Every list item line** emits `{kind:"line", style:"list-item", depth, revealed?}` (1-based
  depth) at the marker position — the view uses it for hanging indent (wrapped item text aligns
  with the first line's text). Nested items (depth ≥ 2) additionally emit a `conceal` over the
  raw leading indent whitespace (revealed as `mark:delim`); the view supplies exact per-depth
  padding (1.5em per level) so each nested marker starts at its parent's text column.
- **Marker reveal is LINE-level (v0.3 amendment — matching headings).** The reveal extent of
  every line-prefix marker construct — the blockquote `> ` run, the list marker, a task item's
  `- [ ]` pair, and the nested leading indent — is the construct's whole line (terminator
  excluded, closed-interval touch like every reveal extent). A cursor/selection touching ANY
  part of the line therefore reveals ALL of that line's marker constructs together as raw
  source, and every other line is untouched. This replaces v0.2's glyph-adjacency/piecewise
  model: reveal no longer depends on which character the caret touches, and mixed prefixes
  (`> > - item`) reveal all-or-nothing per line. The `revealed` flags on `blockquote` and
  `list-item` line decorations remain PER CONSTRUCT in the schema (forward compatibility), but
  under line-level extents they are always equal on a given line; a revealed line renders in
  full source geometry (the view drops decorative padding/bars/indent and neutralizes marker
  boxes — `ox-src`). Fenced-code reveal stays BLOCK-level; inline marks (strong/em/code/strike/
  link) keep per-node reveal.
- **Ordered-list numbering is a computed-value WIDGET (v0.3 amendment — research/07 §0/§1.2).**
  CommonMark only gives an ordered list's `start` number semantic meaning — display of the
  sibling items is a renderer choice, not something the raw digits are required to spell out
  correctly. Obsidian (v1.8.3) chose to fix this by literally rewriting every sibling's digit in
  the saved source text on every insert/delete, which research/07 documents as the direct cause
  of a year-long tail of renumbering/cross-list-bleed/Tab-interaction bugs — exactly the kind of
  unsolicited byte-level rewrite `plan.md` principle #1 and this document's model rule #2
  ("the core never mutates text on its own initiative") rule out. Oxidown instead computes the
  DISPLAY number in the decoration pipeline and leaves the source untouched:
  - `number` is `start + position-in-run`: the enclosing list's `start` (its first item's own
    literal digits, or 1 if absent) plus a zero-based count of items seen so far in that SAME
    list. `"4." / "5." / "9."` therefore displays `4, 5, 6` (start honored, then strictly
    sequential) and `"1." / "1." / "3."` displays `1, 2, 3` — raw sibling digits are cosmetic.
  - `delim` is the marker's literal delimiter byte, `"."` or `")"`. Per CommonMark, changing
    delimiter (or switching between ordered and bullet) ends the enclosing list and starts a new
    one, so a `)`-flavored run's sequence is independent of any preceding `.`-flavored run at the
    same nesting position — its own counter restarts from ITS OWN first item's literal digits.
  - Nested ordered lists restart their own sequence (their own counter), under bullets, under
    tasks, and inside blockquotes alike — matching the parser's existing per-list-item depth
    tracking, not a new structural concept.
  - Reveal behavior is UNCHANGED from the rest of this section: LINE-level, matching every other
    marker construct. A cursor/selection touching any part of the item's line withholds the
    widget and shows the RAW source digits instead, as `mark:list-marker` — exactly the literal
    bytes on disk, never the computed number. Editing therefore always operates on true source.
  - **Rationale, restated plainly: display numbering is a VIEW computation. The core never
    rewrites source digits** — `indentList`/`outdentList`'s own minimal structural digit rewrites
    (only where CommonMark would otherwise fail to parse the result as list items at all) are the
    one narrow, already-documented exception, and are orthogonal to this cosmetic-numbering
    concern.
  - **Compatibility note:** this is additive to the wire schema (a new `widget` tag), but a view
    that doesn't recognize `"ordered"` and falls back to "ignore unknown widget kinds" per the
    v0.2 forward-compatibility rule above will render NOTHING for a concealed ordered marker
    (not stale/wrong text — an empty gap), since the raw digits are only ever emitted when the
    line is revealed. Every view MUST adopt this widget kind to render ordered lists at all.
    Breaking-in-practice, but accepted pre-1.0 with a single first-party view (`oxidown-view-cm6`).
- Thematic break — `line:hr` on the line, plus `conceal` over the raw dashes (revealed as
  `mark:delim` when the cursor is on the line). The view draws the actual rule on the hr line;
  nested blockquote bars are likewise the view's job (one bar per depth level).
- Headings/strong/em/inline-code unchanged from v0.

Reveal semantics are otherwise unchanged: per-node selection∩extent — for marker constructs
that extent is the whole line (above), so clicking the rendered checkbox, which sits inside
its own line, still reveals/toggles correctly.

## Core-driven changes (generalization of the undo rule)

`undo`/`redo`, `command`, and `streamAppend` all return the SAME shape — splices the view must
apply verbatim under its skip annotation:

```ts
export interface CoreChange {
  revision: number;
  splices: Splice[];                       // current-doc coordinates
  selection?: { anchor: number; head: number } | null;  // optional cursor placement
}
```

## Anchors (public position type, plan §5.3)

```ts
createAnchor(pos: number, bias: "before" | "after"): number;  // anchor id
resolveAnchor(id: number): number | null;   // current position; null if unresolvable
dropAnchor(id: number): void;
```

Anchors survive arbitrary edits (mapped through every splice, bias-aware: "before" stays put
when an insertion lands exactly on it; "after" moves with the insertion). Deleting the anchored
text collapses the anchor to the deletion site; it does not become null in M1.

**Replacement-at-anchor bias (v0.3 clarification, pinned by test).** For a REPLACEMENT splice
(`at = p`, `delete > 0`, non-empty `insert`) with an anchor exactly at `p`, the anchor stays at
`p` — before the inserted text — regardless of bias. An "after"-biased anchor moves with a PURE
insertion at its position, but a replacement's insertion happens at the deletion site the anchor
collapsed to and does not carry the anchor past it. This intentionally differs from CM6's
`assoc: 1` position mapping, which would place the position after the inserted text; both cores
implement the stay-at-`p` behavior.

## Commands

```ts
command(name: "toggleStrong"|"toggleEm"|"toggleStrike"|"toggleCode", from: number, to: number): CoreChange | null;
command(name: "setHeading", pos: number, level: 0|1|2|3|4|5|6): CoreChange | null;   // 0 = paragraph
command(name: "toggleTask", pos: number): CoreChange | null;  // flips an existing task, else PROMOTES the line — see "toggleTask" below
command(name: "indentList"|"outdentList", from: number, to: number): CoreChange | null;
command(name: "enter", from: number, to: number): CoreChange | null;  // v0.3 addition — see "enter" below
// v0.6 additions — see "v0.6 commands" below:
command(name: "toggleQuote"|"toggleLink"|"toggleBulletList"|"toggleOrderedList"|"toggleCodeBlock", from: number, to: number): CoreChange | null;
command(name: "insertHr", pos: number): CoreChange | null;
```

Commands are text transforms computed against the overlay (plan §5.8): they emit minimal
splices (toggle = add or remove delimiters), enter the op log with origin `"command"`, and are
single undo units (never coalesce). Returns null when the command doesn't apply at the target —
`indentList`/`outdentList` are the one exception to "null when nothing happens"; see below.
**`command()` either returns `null`/`CoreChange` or throws WITHOUT mutating the core** — planning
happens entirely before any apply, so a thrown command is not a mirror-desync signal; views must
not resync (`load()`) in response to one (contrast with `applyEdit`/`decorations`, where any
exception IS still a desync emergency per "Error handling" below).

**CRLF positions (v0.4 addition).** A command position argument (`from`/`to`/`pos`) that falls
between the `\r` and `\n` of a CRLF pair is a validation refusal — the delimiter insert would
split one line terminator into two line breaks, adding a line the user never asked for. Both
code units are individually addressable (they are separate chars, so the strict surrogate check
passes), which is why this needs its own rule. Throws `InvalidArgument: position {p} splits a
CRLF sequence` (identical on both cores), no mutation, no revision bump.

### Inline toggles: flanking-safe trimming (v0.4 amendment)

CommonMark's flanking rules mean a `**` inserted before whitespace can never open strong — a
naive toggle over `"a "` in `a b` would emit `**a **b`, which parses as LITERAL asterisks, and a
second toggle over the same content would then STACK another delimiter pair instead of removing
one. The same planner already refuses every context where its delimiters would come out literal
(code spans, multi-block ranges); whitespace edges are that hazard's remaining case, closed by
trimming rather than refusal:

- Before planning, `toggleStrong`/`toggleEm`/`toggleStrike` TRIM a non-empty selection's ends
  inward over whitespace; detection of the already-toggled (OFF/extend) state runs on the
  trimmed range too. The whitespace set is pinned — U+0009, U+000A, U+000C, U+000D, U+0020,
  U+00A0, U+1680, U+2000–U+200A, U+2028, U+2029, U+202F, U+205F, U+3000 — chosen to match
  CommonMark's Unicode-whitespace definition; it is NOT Rust `char::is_whitespace()` and NOT
  JS `\s` (they disagree at U+0085/U+FEFF), so both cores hard-code the same list.
- A selection that trims to empty (whitespace-only) means the toggle DOES NOT APPLY → `null`,
  the standard doesn't-apply signal. No undo unit, no revision bump.
- The returned `selection` covers the trimmed, toggled content, preserving the double-toggle
  guarantee: toggling the returned selection again restores the original bytes exactly.
- Cursor-only toggles (`from == to`) and `toggleCode` are unchanged — code spans have no
  flanking rules; the code planner's existing padding treatment stands.

### `setHeading` (v0.4 clarifications, v0.5 amendment)

- The list-item gate applies at any quote depth: `setHeading` inside a blockquote strips the
  `>` prefix before classifying the target line, so a quoted list item (`> - item`) refuses
  (`null`) exactly like a top-level one — previously the quote wrapper hid the inner list from
  the gate and the marker was swallowed as literal heading text.
- Level 0 (heading → paragraph) removes EVERY delimiter span, an ATX closing hash run included:
  `# foo #` → `foo`, not `foo #`. Releveling (1–6) keeps the closing run, as before.
- **Idempotent press toggles back to a paragraph (v0.5 amendment).** `setHeading(pos, N)` where
  the line is ALREADY exactly level `N` (1–6) behaves exactly like level 0: it removes the
  heading, ALL delimiter spans included. This closes a toolbar-parity gap — clicking H2 on a
  line that is already an H2 used to be a silent no-op (`null`); it now returns the line to a
  paragraph, matching the idempotent-toggle behavior every other formatting button already has.
  "Already level `N`" compares the parsed heading node's OWN level, not a byte-identical prefix
  match, so an irregularly-spaced `"##  x"` (an extra inner space beyond the one required after
  the hashes) still counts as "already level 2" and clears fully — only the delimiter span
  itself is removed; any extra whitespace beyond it is content and is untouched, same as it
  always was for level 0. A DIFFERENT level still replaces the opening delimiter as before
  (never treated as a toggle); level 0 is unchanged (always clears, regardless of the line's
  current level).

### `toggleTask` (v0.5 amendment — Obsidian parity)

`pos` anywhere in an EXISTING task item still flips exactly the `[ ]`/`[x]` checkbox byte
(unchanged: `[X]` also toggles off to `[ ]`, and this path never moves the cursor). What changed
is what happens when `pos` does NOT resolve inside an existing task item: v0.2–v0.4 refused
(`null`) there; v0.5 PROMOTES the line containing `pos` into a task instead, matching Obsidian's
"Toggle checkbox status" command, which converts a plain bullet into a checkbox rather than
no-op'ing (research/07 §1.6):

- **Non-task list item** (bullet or ordered, any nesting depth, any quote depth): the `"[ ] "`
  run is inserted right after the marker token — after the marker's required single space when
  the item has content (the ordinary case), or with its OWN leading space when the marker has
  none (a bare empty item, e.g. `"-"`) so the result is still valid GFM task syntax
  (`"- [ ] "`) rather than the unrecognized `"-[ ] "`. This is resolved per LINE — the line
  carrying its own marker — not the whole possibly-multi-line item; a cursor on a plain list
  item's CONTINUATION line does not promote (the same line-oriented v1 scope every other
  line-based command in this contract has; the flip path above already covers "anywhere in the
  item" for the EXISTING-task case, which remains the contract's pinned guarantee for that case).
- **Plain paragraph or blockquote-content line** (seen through any quote prefix, exactly like
  `setHeading`'s own gate; not blank): `"- [ ] "` is inserted at the content start, i.e. right
  after the quote prefix — empty at top level, preserved verbatim when quoted
  (`"> text"` → `"> - [ ] text"`).
- **Blank line** (including an empty quote line, e.g. `">"`): also promotes, `"- [ ] "` inserted
  after any quote prefix. This is MORE permissive than `setHeading`'s own blank-line refusal —
  deliberately: Obsidian promotes a blank line too, and a toolbar button that sometimes silently
  does nothing on an empty line is a worse experience than a predictable empty task item.
- **Still `null`** on headings, fenced/indented code lines, thematic breaks, and every other
  block kind a checkbox makes no sense on (tables, HTML blocks, footnote definitions, or a
  continuation line inside some OTHER list item) — the same conservative set `setHeading`
  refuses, since a checkbox on a heading or inside a fence is exactly as nonsensical as a hash
  run would be there.

Selection after a promotion maps the original `pos` forward through the inserted text
(after-biased): the insertion always lands at/before `pos`, so the character immediately after
the cursor is unchanged — the cursor stays with its content, shifted right by the inserted
marker/checkbox bytes, rather than landing mid-syntax between the new marker and the new
checkbox. One undo unit, same as every other command. The whole-document itemness invariant
(indentList/outdentList's own acceptance bar) holds here too, in its ADDITIVE direction:
a promotion may give a line itemness it didn't have, but never costs any OTHER line the
itemness it already had.

### `indentList` / `outdentList`

Obsidian-style Tab nesting: indent a list item to its PARENT MARKER'S CONTENT COLUMN — 2 spaces
under `- `, 3 under `1. `, 4 under `10. ` — rather than a fixed 2-space shift (CM6's stock
`indentMore`/`indentLess`, which the view falls back to outside list context). All quantities
below are per PHYSICAL SOURCE LINE.

Definitions:
- **Quote prefix**: a line's blockquote marker run (`> `, `> > `, …), from the parser's per-line
  `BlockQuoteLine` nodes (`> `'s own required trailing space is part of the prefix; further
  spaces before a list marker are not).
- **Marker column**: the column of a list marker's first glyph (`-`, `+`, `*`, or an ordered
  marker's digits), measured AFTER the quote prefix.
- **Marker token width**: marker glyphs plus exactly ONE following space — `- ` = 2, `1. ` = 3,
  `10. ` = 4. For a task item (`- [ ] x`) the token is just `- ` (width 2): the `[ ]` is GFM
  content, not part of the marker, so nesting under a task item needs only 2 spaces. This is a
  FIXED formula — it does not grow with however much whitespace the source actually has after
  the marker (CommonMark tolerates a few extra spaces there without moving the content column).
- **Content column** = marker column + marker token width.

Applies-vs-no-op:
- The command applies iff at least one line intersecting `[from, to]` is a list-item line
  (carries a list marker). If none → does not apply → `null` (the view falls back to
  `indentMore`/`indentLess`).
- When it applies but no movement is possible (see below) it returns a CoreChange that is a
  NO-OP: empty `splices`, no `selection` — and it must NOT create an undo unit or bump the
  revision (unlike every other non-empty command result). The view must still treat this as
  "handled" (do NOT fall back to `indentMore`/`indentLess` just because nothing moved).

First-line delta (batching): the whole edit moves by ONE delta, computed from the FIRST
intersecting item line only. Scan upward from it over consecutive list-item lines at the SAME
quote depth (stop at the first line that either isn't a list-item line, or is one at a different
quote depth — sibling/parent scans never cross a quote boundary; a list inside a quote never
nests relative to items outside it, and vice versa) to find:
- **indentList**'s target: the nearest such line with marker column `<=` the first line's.
  `delta = target's content column − first line's marker column`; `delta <= 0` → no-op. No target
  above (first item of its list) → no-op.
- **outdentList**'s parent: the nearest such line with marker column STRICTLY `<` the first
  line's. `delta = first line's marker column − parent's` (always `> 0`). No parent (already
  top-level) → no-op.

Subtree-aware affected set: it is not just the lines intersecting `[from, to]`. For EVERY
intersecting item line, its whole subtree moves with it by the same single delta above — walk
forward from that line collecting consecutive following lines that are (a) list-item lines,
(b) at the same quote depth as it, (c) with marker column STRICTLY GREATER than **its own**
column (not the previous line's — so a multi-level subtree, several children included, is
captured in one walk; a following line at an EQUAL column is a sibling and stops the walk, not a
descendant). The walk also stops at a quote-depth change or the first non-item line — including a
blank line: v1 does not look past one to see whether list content resumes, so a blank-line-
separated ("loose") continuation is left at its prior depth. The union of every intersecting item
line plus its subtree (deduplicated, applied in document order) is the final affected set:
- indentList inserts `delta` spaces immediately after the quote prefix on every line in it.
- outdentList removes `min(delta, that line's own marker column)` spaces from just after the
  quote prefix on every line in it (clamped independently per line, so a shallower-than-expected
  descendant never goes negative).

Adoption: outdenting an item past a following EQUAL-column sibling makes that sibling (and
anything deeper under it) a CHILD of the outdented item on reparse — its column now exceeds the
moved item's new content column. This is intended, standard outliner behavior: the sibling keeps
its itemness, and a later re-indent of the moved item carries the adoptee along as part of its
subtree (the round trip restores structure, not necessarily the byte-identical original shape).

Renumbering: there is no COSMETIC renumbering of siblings — presenting `1./1./1.` sources as
`1./2./3.` is the view's computed-number territory (research/07). But the command DOES perform a
minimal STRUCTURAL digit rewrite where CommonMark would otherwise refuse to parse the command's
own output as list items:

**Paragraph-interruption guard.** Per CommonMark, an ordered marker whose number != 1 cannot
START a new list in paragraph-interruption position — indenting `2. b` directly under `1. a`'s
open paragraph would make `   2. b` reparse as LAZY CONTINUATION text of item 1, silently
de-listing the moved item (and stranding its carried subtree), after which Shift-Tab finds no
item line and falls back. The same failure can hit a line the command never touched: the edit
restructures the parse context BELOW the affected set — e.g. outdenting `   - bullet` (nested
under `2. ordered`) to top level makes a following `3. ordered` sibling, which used to CONTINUE
the open outer ordered list, now sit against the new top-level bullet list, where its non-1
marker cannot start a list → it de-lists without ever being edited.

To prevent both, after computing the batch (indent AND outdent alike), TWO lines are checked
with one deterministic structural rule (a single rule the command applies directly, not post-hoc
parser validation):

1. the FIRST affected line (the moved item itself), at its new column;
2. the first UNAFFECTED list-item line BELOW the affected set, at its own (unchanged) column —
   found by walking down from the last affected line over consecutive same-quote-depth item
   lines, SKIPPING adopted descendants (column strictly greater than the moved line's new
   column: they nest under the moved block, whose itemness check 1 already covers), and stopping
   at a non-item/blank line or quote-depth change like every other scan in this section.

The landing-scan rule, for a checked line that will sit at column `c` after the edit:

- Only ordered markers with number != 1 are ever candidates (numeric — `01.` counts as 1;
  bullets and `1.`/`1)` are always safe).
- Scan upward from the checked line past consecutive same-quote-depth list-item lines whose
  POST-EDIT marker column is STRICTLY GREATER than `c` (affected lines count at their post-edit
  columns — the batch's per-line shift; unaffected lines at their unchanged ones); land on the
  nearest line with column <= `c`.
- If the landing line is a list-item line at column EQUAL to `c` whose marker is also ordered
  with the SAME delimiter flavor (`.` vs `)`), the checked item JOINS that already-open ordered
  list — any number is valid there — and nothing is rewritten (e.g. Tab on `2. b` in `1. a` /
  `   1. a1` / `2. b` keeps `   2. b`).
- Otherwise (landing on a shallower item, a different marker family — e.g. a bullet list open
  at that column — or the scan broke on a non-item or different-quote-depth line), the checked
  item would START a new list in interruption position: its digits are rewritten to `1`
  (`2.` → `1.`, `10.` → `1.`) as additional splice(s) in the SAME batch and undo unit.

The moved line is text the user explicitly commanded to transform. The below line is not — but
the command restructured that line's parse context, and keeping every pre-edit list item a list
item is part of the command's contract (the whole-document invariant: itemness may never be
destroyed by indent/outdent; marker digits may change). Its displayed number is cosmetic per
CommonMark — view-computed numbering (research/07) renders sequence numbers correctly regardless
of the raw digits. So neither rewrite violates the never-rewrite-unedited-bytes invariant's
intent; only marker digits of at most those two lines are ever touched.

Accepted v1 imprecision: `10.` → `1.` shrinks the marker token width by one, so the moved
item's descendants (shifted by the pre-rewrite delta) may sit one column past the ideal content
column — still valid nesting, just not byte-ideal.

Selection: the cursor/selection maps through the inserted/removed spaces (existing
mapping/anchor machinery), so it stays logically attached to the same character.

Undo: one undo unit for the whole affected set (however many lines it touches, digit rewrites
included); undo restores every line at once.

### `enter` (v0.3 addition)

Construct-aware Enter (research/07 §1.3/§1.4/§2.1): continue a list marker or quote prefix on
non-empty content; exit an EMPTY one in a **single press**. Obsidian requires an awkward
double-Enter (blank-line intermediate) to leave a list from an empty item — research/07 §1.4
documents that even obsidian-outliner replaces it with the one-press outliner mechanic; we ship
the better mechanic directly. Every rule below resolves constructs from the parsed overlay,
never a line regex — the same discipline as `indentList`/`outdentList`, and the reason (per
research/07 §2.1/§2.3) Obsidian's own Enter/Tab heuristics have quote+list interaction bugs
this design structurally cannot.

The view binds Enter (before its default keymap, never while composing — rule 8): `null` →
fall through to the default Enter (plain newline); otherwise apply the CoreChange. Unlike
`indentList`/`outdentList` there is NO applies-but-no-op case: every applicable case produces
real splices, so `enter` returns `null` or a change with a non-empty splice list, never an
empty one.

Let L = the line containing `from` (after normalizing `from <= to`). Vocabulary is the
indentList section's: quote prefix, list marker, marker column, marker token width. **Content
start** = the marker token's end — for a task item, the end of the whole `- [ ] ` run (checkbox
plus its required trailing space); clamped to L's own end (a bare `-` with no trailing space is
still an empty item per CommonMark).

1. **Not applicable → null.** L has neither a list marker nor a quote prefix (plain paragraph,
   heading, code, …), OR `from` sits INSIDE L's prefix region (before the item's content start;
   inside the quote marker run). **v1 punt, documented:** the inside-the-prefix cases fall back
   to the default plain newline rather than attempting anything construct-aware.
2. **Continue** (list item, non-empty content after the marker): replace `[from, to]` with
   `"\n"` + continuation prefix — L's quote prefix + L's leading indent (raw bytes, verbatim) +
   the next marker:
   - bullet: the same glyph L uses (`- `, `* `, `+ `) — byte-faithful to the source flavor;
   - ordered: L's raw source number + 1, same delimiter (`7) ` after `6) `; `9. ` → `10. `,
     width grows naturally). Display numbering is view-computed (v0.3 ordered widget) so any
     digits render right, but the source stays sensible;
   - task (bullet or ordered): append `[ ] ` — new items always start unchecked.
   Text after `to` on L becomes the new item's content (mid-line Enter splits the item — a
   natural consequence, no special casing). Selection lands at the end of the inserted prefix.
3. **Exit/outdent** (list item, content EMPTY — nothing or only whitespace after the marker
   token/checkbox — and `from` at/after content start). No `"\n"` is inserted in either branch:
   ONE Enter press = ONE level of escape.
   - Marker column > 0 (nested, incl. nested-in-quote): OUTDENT one level in place — the same
     delta/parent computation as `outdentList` restricted to this single line (no subtree; an
     empty item has none), INCLUDING both structural rewrite guards (the moved line's
     interruption rewrite and the below-line rewrite — the whole-document itemness invariant
     holds exactly as for `outdentList`). If the upward scan finds no qualifying parent (the
     same v1 blank-line-scan limit as outdentList), the press falls through to the top-level
     branch instead of doing nothing.
   - Else (top-level item): DELETE the marker token — and a task item's brackets+space with
     it — from L, leaving the quote prefix (if any). L becomes an (empty) paragraph/quote line.
4. **Quote continue** (quote prefix, no list marker, non-empty content after the prefix):
   insert `"\n"` + L's exact quote prefix at `[from, to]`.
5. **Quote exit** (quote-only line, empty after the prefix): remove the LAST `> ` run element
   only — ONE level per press: `> > ` → `> ` on the first press, `> ` → plain on the second.
   Never all levels at once (the single-press philosophy applies per level, not per line).
6. **Mixed (list inside quote)**: innermost construct first, piecewise — rules 2/3 keep the
   quote prefix intact in the continuation/outdent; an empty top-level item inside a quote
   clears just the marker and keeps `> ` (rule 5 then applies on the next press).
7. **Selection** (`from != to`): context comes from the pre-edit parse at `from`; the
   replacement covers the selection (delete + continue in one batch, one undo unit). A
   selection extending past L's own end does NOT skip rule 3's below-line guard: the guard's
   downward scan starts past the line containing `to` (accounting for the whole region the
   selection consumes), uses post-edit columns like every other landing scan, and lands on the
   same below-context line a collapsed cursor reaching the byte-identical shape would — the two
   paths emit the same rewrite. Lines the selection deletes outright lose their itemness as a
   direct consequence of the user's own gesture (the same rationale as rule 3 de-listing the
   pressed line itself); the itemness invariant protects every line the press does NOT consume.
8. **Composition:** the view keymap must not intercept Enter while `view.composing` (return
   false — the key belongs to the IME).

Undo: one unit per press (`"command"` origin — never coalesces), like every other command.

**Empty-item parser note (v0.3, shipped with this command).** An empty list item (`- ` with
nothing after it — the exact shape every continue press creates) previously emitted NO overlay
nodes at all (the underlying parser produces no content event to anchor the marker's span), so
it rendered with no bullet/ordered widget and no `list-item` line decoration, and no command
could see its marker. The parser now synthesizes the marker node for empty items directly from
the source bytes — same node shape, LINE-level reveal, ordered items keep their slot in the
view-computed sequence (an empty `2. ` between `1. `/`3. ` still counts). Purely additive:
empty items now decorate and behave like any other item.

### v0.6 commands: toggleQuote / toggleLink / toggleBulletList / toggleOrderedList / insertHr / toggleCodeBlock

Six new commands (M2 web-editor-beta toolbar batch), sharing every established command rule:
one undo unit per press (never coalesces), origin `"command"`, UTF-16 range/position arguments
with the CRLF-split guard, `command()`'s no-mutation-on-throw guarantee, and selection results
that keep the cursor glued to its character (the character after the cursor is unchanged by a
prefix insertion). The whole-document ITEMNESS INVARIANT (indentList/outdentList's acceptance
bar: no command may cost an un-edited line its list itemness) holds for all of them, with one
documented exemption called out under the list toggles.

**`toggleQuote(from, to)`** — line-wise over every line intersecting the range, with STEPPED
remove semantics (research/07 §2.2 — repeated presses unwind one nesting level per press, the
gap Obsidian's on/off-only "Toggle Blockquote" never closed natively):

- **Remove** when EVERY intersecting non-blank line already has quote depth >= 1: each line
  loses its INNERMOST `> ` run element only. A lazy-continuation line counts as quoted for the
  mode decision (it IS inside the quote) but carries no marker run of its own, so remove leaves
  its bytes alone; a selection whose quoted lines are all marker-less returns the
  indentList-style applies-but-no-op empty CoreChange (no undo unit, no revision bump).
- **Add** otherwise: `"> "` is inserted at every intersecting line's start — blank lines
  INCLUDED, so a quote wrapped around a multi-paragraph selection stays one contiguous quote.
  Works on list items / tasks / headings unchanged (the prefix goes before existing content;
  the parse nests the construct).
- **Never `null`**: any position resolves to a line, so the command always applies (the
  no-op case above is the one degenerate shape).
- **Structural guards** (itemness invariant, same digit-rewrite contract as indentList's
  interruption guards): quoting a selection interrupts the list anchoring a non-1 ordered item
  directly below it (the marker would degrade to lazy-continuation text of the freshly quoted
  paragraph), and de-quoting a non-1 ordered item can drop it where it cannot start a list —
  in both cases the affected marker's digits rewrite to `1` in the same batch/undo unit.

**`toggleLink(from, to)`** — single-line only (a range spanning a line terminator → `null`,
the standard v1 scope). Code contexts refuse exactly like the inline toggles (`null`): a range
touching a fenced-code line, or an endpoint strictly inside an inline code span.

- **Unwrap** when the range's closed interval intersects an existing link node: an inline link
  loses its `[` and `](url)` delimiter spans — the text survives, the URL is gone (re-toggling
  wraps with an EMPTY url slot; the text round-trips byte-identically, the URL does not — the
  documented asymmetry). An AUTOLINK (`<url>`) sheds just its `<`/`>` wrappers, keeping the
  destination text. The returned selection covers the surviving text (matching the inline
  toggles' OFF path).
- **Wrap** otherwise: the range becomes `[<selected text>](` + `)` and the returned selection
  is a cursor in the URL slot (between the parens). An EMPTY range inserts `[]()` with the
  cursor in the TEXT slot (between the brackets).
- View binding: Mod-k (the near-universal insert-link key), consuming the key even on `null`
  so Ctrl/Cmd-K never leaks to the browser.

**`toggleBulletList(from, to)` / `toggleOrderedList(from, to)`** — line-wise conversion with
toggle semantics. Blank lines and fenced-code lines pass through untouched in every mode;
`null` only when NO intersecting line is convertible (all blank/code).

- **Flavor rule (pinned)**: task items are BULLET-flavor (`- [ ] x` is a bullet item;
  `1. [ ] x` is ordered-flavor) — the checkbox is GFM content, not marker.
- **Strip** when every convertible line is already an item of the TARGET flavor: leading
  indent, marker token, task brackets, and all post-marker whitespace go (a genuinely plain
  line at any depth — leaving 4+ leading spaces would re-type the line as indented code). This
  is an explicit de-listing gesture: the stripped lines are EXEMPT from the itemness invariant
  exactly like `enter`'s marker-clear; every line the command does not touch keeps its
  itemness (below-line guard).
- **Convert** otherwise: plain lines get a marker prefixed at content start (right after any
  quote prefix); other-flavor items get their marker glyphs REPLACED in place (indent and
  quote prefix kept). Task decisions (pinned): converting to ORDERED strips the task brackets
  (`- [ ] x` → `1. x`); converting an ordered task to BULLET keeps them (`1. [ ] x` →
  `- [ ] x` — still a task, now bullet-flavor); already-target lines are untouched, brackets,
  raw digits and all (never-rewrite-unedited-bytes beats cosmetic uniformity).
- **Ordered numbering**: markers the command WRITES get sequential raw digits restarting at 1
  per contiguous same-column run (blank/code lines, quote-depth changes, and shallower columns
  end a run). Untouched ordered lines feed the counter (raw value + 1) and their `.`/`)`
  delimiter is adopted; the run seeds from the item line directly above the selection at the
  same column — written markers therefore always either start a run at `1` (which may
  interrupt anything) or JOIN an adjacent same-column same-delimiter run, so the conversion
  itself can never de-list its own output. Bullet conversion adopts the run's bullet glyph the
  same way (`*` siblings get `*`, not a list-splitting `-`). Display numbering recomputes
  regardless (v0.3 ordered widget); this is about the source reading sensibly.
- **Below-line guard** (same contract as indentList's `below_line_rewrite`): the first
  unaffected item line below the affected block — skipping adopted descendants in convert
  mode — rewrites its digits to `1` when it is a non-1 ordered marker that no longer joins a
  same-column same-delimiter run. The scan stops at a blank/non-item line or quote-depth
  change, and never runs when the selection's own trailing line is blank/code.
- **Accepted v1 imprecision** (mirror of indentList's `10.` → `1.` note): converting a bullet
  to ordered grows the marker token width by one, so a descendant sitting exactly at the old
  content column may reparse as a SIBLING rather than a child — still a list item (the
  invariant holds); only its nesting depth degrades.

**`insertHr(pos)`** — inserts a thematic break on its own line AFTER the line containing
`pos`. The CommonMark trap this construction guards (one this repo has been bitten by): `---`
directly under paragraph text is a SETEXT-H2 UNDERLINE, not an hr. The splice therefore
guarantees a blank line ABOVE (one extra `"\n"` when the current line is non-blank) and BELOW
(one trailing `"\n"` when a following non-blank line exists — the original terminator supplies
the second newline), so the result always reparses as `ThematicBreak` (pinned by a
reparse-assertion test on exactly the paragraph-adjacent shape, paragraphs above and below
intact). The break is inserted at TOP level: on a quoted line it lands after (outside) the
quote, splitting it — v1 behavior. `null` only on fenced-code lines (literal dashes). The
cursor does not move (the insertion lands at/after `pos`).

**`toggleCodeBlock(from, to)`** —

- **Unwrap** when the range intersects an existing FENCED block (block-level touch semantics,
  matching fence reveal: fence lines or body, anywhere counts): both fence LINES are removed —
  each with one adjoining line terminator, so no stray blank lines are left — and the body
  survives verbatim. An unterminated block loses just its opening fence line.
- **Wrap** otherwise: the intersecting lines are wrapped in backtick fences on their own lines
  above/below. The fence is one backtick longer than the longest leading backtick run (after
  up to 3 leading spaces) among the wrapped lines, minimum three, so a wrapped line can never
  close the new fence early. The selection shifts by the opening fence's length — same
  characters, now INSIDE the block.
- **Quote punt (v1, documented)**: `null` whenever the target sits at quote depth > 0 —
  fences-in-quotes need per-line `> ` prefix surgery on every body line, deferred. INDENTED
  (non-fenced) code blocks are invisible to this command (they emit no overlay nodes); their
  lines wrap like plain text.

## Streaming (plan §5.9)

```ts
streamOpen(pos: number): number;                    // stream id; insertion point becomes an internal anchor
streamAppend(id: number, chunk: string): CoreChange; // splices for the view to apply (skip annotation)
// v0.3 amendment — returns the surrogate-flush change (see below), or null. Previously void.
streamClose(id: number): CoreChange | null;
```

Rules:
- Ops carry origin `"ai"`. An ENTIRE stream session (open→close) is exactly ONE undo unit.
- The user may keep editing while a stream is open; the stream's insertion anchor maps through
  user edits, so concurrent edits above/below the stream point interleave correctly.
  Editing *inside* the already-streamed region while open is not blocked in M1, but the demo
  should not encourage it (review/suggestion mode is deferred per plan §5.9).
- Append fast-path: an append that only extends the open tail block must not force
  full-document work beyond Phase-A parsing; with Phase A this means the decoration/damage
  computation is O(tail block), and the parser call itself stays within the perf budget.
  Corollary: the budget depends on the streamed content containing block boundaries — a
  stream that never closes its tail block (one long paragraph, or a single blank-line-free
  list) pays O(tail block) per append, quadratic in total streamed bytes (known M1 limit,
  characterized in `stream_perf.rs`).
- `streamClose` on an unknown/closed id is a no-op (returns `null`); `streamAppend` on one throws.
  `load()` closes all open streams AND discards any adapter-buffered pending surrogate, so
  `streamClose` on a pre-load id remains a null no-op — it never throws.
- **`streamClose` returns `CoreChange | null` (v0.3 amendment — previously `void`).** When the
  stream's withheld trailing high surrogate is flushed as U+FFFD on close (see "Unpaired
  surrogates in payloads"), the resulting `CoreChange` is RETURNED so the view can apply it
  under its skip annotation like any other core-driven change — before this amendment the flush
  mutated the core but the change was silently dropped, desyncing the view's mirror. Returns
  `null` in the common nothing-pending case. The flush edit belongs to the STREAM'S single undo
  unit (it is the stream's last append), not a unit of its own. The surrogate buffering — and
  therefore the flush — lives at the TS adapter layer: the raw wasm binding never buffers
  (surrogate policy is enforced JS-side before text crosses the boundary), so its own
  `streamClose` has nothing to return; the TS adapter produces the returned change.

## New edit origins

`EditOrigin` gains `"ai"` (stream ops) and `"command"` (command ops). Neither ever coalesces.

## v0.2 clarifications (pinned after first implementations)

1. **`undo`/`redo` return `CoreChange | null`** — the same shape as `command`/`streamAppend`
   (structurally supersets the v0 shape). The core supplies `selection`; views use it instead
   of guessing cursor placement from splice ends.
2. **Stream undo grouping**: chunks of the same uninterrupted stream coalesce into that
   stream's single unit; an interleaved user edit gets its own unit and the stream's unit
   remains sound (reverts the streamed spans, mapped). **Undo order is unit-creation order
   (LIFO)**: a user edit made mid-stream pops before the stream's unit, because its unit was
   created after the stream's unit began.
3. **`list-marker` spans include the required trailing whitespace** (`"- "`, `"1. "`).
   *(v0.4 clarification — the span's end is pinned, resolving an ambiguity in "required":)*
   a non-empty item's marker span runs to the item's CONTENT START (the underlying parser's
   first-content lookahead, clamped to the marker's own line) — ALL post-marker spaces/tabs
   are marker territory, not just the single required one, so `-   spaced` conceals `-   `
   ([0,4)) and the item text renders flush at the content column like every sibling. A task
   item's marker span ends where the `[x]` checkbox begins, extra spaces before the brackets
   included. Empty items (whose marker is synthesized — no content event exists to look
   ahead to) take glyphs + delimiter + the single trailing space if present. This does NOT
   change `indentList`/`outdentList`'s "marker token width", which remains the FIXED
   glyphs+one-space formula by design (see that section).
4. **Link conceal spans** are two spans (`[` and `](url)`); on reveal they are emitted as
   delim/url/delim pieces. An angle-bracketed destination (`[t](<u v>)`) contributes only the
   inner span as `mark:url`; the `<`/`>` wrappers remain in the surrounding `mark:delim` pieces.
5. **Line terminators**: wherever this contract says "physical source line" (line-level reveal
   extents, per-line marker/quote constructs, the line vocabulary of `indentList`/
   `outdentList`/`enter`), a line is terminated by `\n`, `\r\n`, or a **lone `\r`** — matching
   the underlying markdown parser, which treats a bare `\r` as a line ending (a
   `"- a\r- b\r- c"` document is three list items, and each command/reveal computation must
   resolve each item to its own line). Known, accepted upstream quirk: pulldown-cmark's fence
   and blockquote-marker *scanning* does not itself honor a lone `\r` (a fence must open/close
   via `\n` to parse as a fence at all), so those constructs simply cannot arise on
   lone-`\r`-only lines — the core's own line splitting still treats `\r` uniformly.

## Error handling

Stale revision, overlapping splices, or out-of-bounds positions: throw (wasm: `Err` → JS
exception). The view treats any core exception as a mirror-desync emergency: re-`load()` from
the view buffer and log loudly. Never continue silently.

Position arguments are additionally bounded to `u32::MAX` at the wasm boundary (wasm32's
`usize` is 32 bits; a larger value would otherwise silently truncate — `2^32 + 6` becoming
`6` — and edit the wrong range). Since any integer above `u32::MAX` is necessarily beyond the
document, such a position throws the ordinary `OutOfBounds: position X beyond document length
Y (UTF-16 code units)` directly off the core's own document-bounds check. Positions are NEVER
silently truncated, wrapped, or clamped. This applies equally to positions INSIDE structured
JSON payloads (splice `at`/`delete`, selection `anchor`/`head`): malformed values (negative,
non-integral) throw `InvalidPayload` with the `malformed splices` / `malformed selections`
message, while well-formed integers above `u32::MAX` or beyond the document throw the ordinary
`OutOfBounds`.

Validation-refusal error names (v0.3):

- `InvalidArgs` — thrown by the wasm adapter's argument layer, before dispatch, when a raw
  argument is malformed (non-integer or negative numbers, a missing command argument, a
  `setHeading` level outside 0–6 at the boundary).
- `InvalidArgument` — thrown by the core when a value is semantically outside its documented
  domain (e.g. a heading level above 6 at the core API; an inline-toggle range spanning more
  than one leaf block; a command position splitting a CRLF pair — v0.4).
- `InvalidPayload` — thrown by the adapter's payload layer when a STRUCTURED payload is
  malformed before it can cross the boundary: mis-shaped or non-serializable `splices` /
  `selections` (e.g. a negative or non-integer field —
  ``InvalidPayload: malformed splices: invalid value: integer `-1`, expected u32``), and text
  payloads carrying unpaired surrogates (see "Unpaired surrogates in payloads" below).
- `InvalidOrigin` — thrown by `applyEdit` when the `origin` string is not a documented
  `EditOrigin` value.
- `InvalidBias` — thrown by `createAnchor` when the bias is not `"before"` / `"after"`.
- `InvalidCommand` — thrown by `command` when the command name is unknown.

`applyEdit` validates in a pinned order: malformed
`baseRevision` (`InvalidArgs`) → staleness (`StaleRevision`) → splice payload (surrogate
inserts, malformed numbers, ordering, bounds) → origin (`InvalidOrigin`) → apply-time
surrogate-split check (`SurrogateSplit`). A call that is simultaneously stale AND
payload-malformed therefore throws `StaleRevision`.

`decorations` likewise validates in a pinned order (v0.4 clarification): malformed
`revision` (`InvalidArgs`) → staleness (`StaleRevision`) → malformed `from`/`to`
(`InvalidArgs`) → viewport range (`InvalidRange`: `from > to`) → viewport bounds
(`OutOfBounds`) → per-selection malformed fields (`InvalidPayload`) → per-selection bounds
(`OutOfBounds`).

All of these are refusals thrown WITHOUT mutating the core, and how a view treats one depends on
what it has already done (v0.4 amendment — the previous blanket "consumed no-ops from EVERY
entry point" rule was unsound for `applyEdit`; see below):

- From `command`, `decorations`, anchor, and stream entry points, the view has changed nothing:
  the refusal is a consumed no-op. Log quietly and move on — validation refusals (names
  matching `/^Invalid/`) are contract behavior, not faults, and must not be reported at error
  severity — and never fall back to a default action, never resync.
- From `applyEdit`, the view forwards changes it has ALREADY applied to its own buffer, so a
  refusal means the mirrors have already diverged — the refusal itself is the desync signal.
  The view must recover via **sanitized reload**: replace any lone surrogate code unit in the
  view buffer with U+FFFD (1:1, length-preserving) and `load()` the sanitized text into the
  core; if sanitization changed the text, also bring the view document to the sanitized form
  with a skip-annotated, history-exempt dispatch. CM6 forbids dispatching from inside an
  update, so the repair dispatch may be deferred (microtask) after the immediate `load()` —
  length preservation keeps the interim mirror structurally consistent, and the pair must end
  byte-equal. The sanitize step exists because the canonical trigger IS a lone-surrogate
  insertion (`InvalidPayload`, see below) — reloading the raw buffer would throw the same
  refusal inside the recovery path and leave no way back to a consistent pair.

The desync-emergency rule above continues to govern exceptions the core throws once a
well-formed call is underway; recovery for those follows the same sanitized-reload path.

## Unpaired surrogates in payloads (v0.3 addition)

Complementing v0.1 clarification 7 (which governs splice *positions*), the document TEXT
itself never contains an unpaired surrogate code unit:

- `load` and `applyEdit` throw `InvalidPayload: ...` when a text payload carries a lone
  surrogate (enforced at the TS adapter layer, before the text crosses the boundary —
  wasm-bindgen's string conversion would otherwise silently corrupt it to U+FFFD).
- `streamAppend` buffers a TRAILING lone high surrogate per stream (adapter behavior: a
  producer chunking at fixed UTF-16 lengths can split a surrogate pair across chunks); the
  withheld code unit is prepended to the next chunk. A lone surrogate anywhere else in a
  chunk throws `InvalidPayload: ...` (and clears the stream's pending buffer).
- `streamClose` flushes a still-pending high surrogate as one U+FFFD before closing — it can
  never be completed, and the document invariant must hold. The flush's `CoreChange` is
  `streamClose`'s return value (`null` when nothing was pending) — see the Streaming rules.
- Empty chunks — including a chunk fully withheld as a pending surrogate — and all-no-op edit
  batches return with the revision unchanged and create no undo unit.

---

# v0.3 changelog

v0.3 is additive/amending on top of
v0.2 and, following the same in-place convention as the v0.2 clarifications, its items live
inline in the sections above, each tagged "(v0.3 amendment)" or "(v0.3 addition)". The full
list:

- **Line-level marker reveal** — the reveal extent of every line-prefix marker construct
  (blockquote runs, list markers, task marker pairs, nested leading indent) is the construct's
  whole line, replacing v0.2's glyph-adjacency/piecewise model. See "Marker reveal is
  LINE-level" under the M1 emission scope.
- **`widget:ordered`** — view-computed ordered-list numbering; concealed ordered markers are a
  computed-value widget carrying `number`/`delim`, and the core never rewrites source digits.
  See "Ordered-list numbering is a computed-value WIDGET".
- **The `enter` command** — construct-aware Enter (continue a list marker/quote prefix on
  non-empty content; exit an EMPTY one in a single press), plus the empty-item parser note
  shipped with it. See "`enter` (v0.3 addition)".
- **Replace-based concealment permitted** — the conceal rule in "Rules for the view" is
  restated as the invariants it protects (document text is the source of truth; no vertical
  layout shift; caret addressable at concealed positions), and replace decorations preserving
  them are explicitly allowed.
- **Undo-coalescing region rule** — v0.1 clarification 4 amended: a single-splice `user`/`ime`
  edit coalesces when it falls within (or touches the ends of) the top undo unit's undo
  region, not merely when it touches the previous edit's end position.
- **Error names `InvalidArgs` / `InvalidArgument` / `InvalidPayload` / `InvalidOrigin` /
  `InvalidBias` / `InvalidCommand`** — argument-layer, core-level-semantic, payload-layer,
  and per-call domain validation refusals; all consumed no-ops (no resync obligation, from
  any entry point). Position arguments — direct AND payload-embedded — are bounded to
  `u32::MAX` at the wasm boundary (over-u32 values throw the ordinary `OutOfBounds`, never a
  silent truncation). `applyEdit` validation precedence is pinned. See "Error handling".
- **`streamClose(id): CoreChange | null`** — previously `void`; the U+FFFD surrogate-flush
  change produced on close is now returned so the view can apply it (it was silently dropped,
  desyncing the mirror). The flush belongs to the stream's single undo unit. See the Streaming
  rules.
- **Replacement-at-anchor bias pinned** — a replacement splice at an anchor's exact position
  leaves the anchor before the inserted text regardless of bias (differs from CM6 `assoc: 1`).
  See the Anchors section.
- **Unpaired-surrogate payload rules** — see "Unpaired surrogates in payloads" above.
- **Incremental-reparse complexity note** — the amendment under "Performance budget" pinning
  how `applyEdit`'s "O(edit + dirty block)" clause holds (windowed reparse, realignment,
  documented degrade cases).

---

# v0.4 changelog

**The current version of this contract is v0.4** (pinned from the M1 PR review). Same in-place
convention: each item lives inline above, tagged "(v0.4 amendment/addition/clarification)".
The full list:

- **Flanking-safe inline toggles** — `toggleStrong`/`Em`/`Strike` trim selection ends over a
  pinned whitespace set before planning; whitespace-only selections → `null`; double-toggle
  byte-identity is preserved over the returned selection. Closes the case where a
  whitespace-edged toggle emitted flanking-violating (literal) delimiters that STACKED on
  re-toggle. See "Inline toggles: flanking-safe trimming" under Commands.
- **CRLF position guard** — a command position between `\r` and `\n` throws
  `InvalidArgument: position {p} splits a CRLF sequence` (both cores, byte-identical message).
  See "CRLF positions" under Commands.
- **`setHeading` clarifications** — the list-item gate sees through blockquote prefixes
  (`> - item` refuses like `- item`); level 0 removes closing hash runs too (`# foo #` → `foo`).
- **Undo depth cap** — the undo stack holds at most 100 units; the oldest is dropped. See the
  `undo()` interface comment.
- **ATX heading content trim** — trailing spaces/tabs are excluded from the heading content
  span (CommonMark). See the M1 emission scope.
- **`decorations` validation precedence pinned** — identical order on mock and wasm
  (revision `InvalidArgs` → `StaleRevision` → viewport `InvalidArgs`/`InvalidRange`/
  `OutOfBounds` → selections `InvalidPayload` → selection `OutOfBounds`), closing a
  cross-core divergence where the same
  doubly-invalid call threw different names in different handling classes. See "Error handling".
- **Refusal handling split by entry point** — supersedes v0.3's blanket "consumed no-ops from
  any entry point": refusals remain consumed no-ops everywhere EXCEPT `applyEdit`, where the
  view has already applied the change it forwards, so a refusal IS the desync signal and
  triggers **sanitized reload** (lone surrogates → U+FFFD, sync the view doc, then `load()`).
  Validation refusals (`/^Invalid/`) are logged quietly, never at error severity. See "Error
  handling".
- **View rules: `readOnly`, `addToHistory: false`, mixed batched updates** — read-only states
  gate all view-initiated core mutations; core-driven dispatches are tagged out of any rogue
  CM6 history; an update batching user changes after a core-originated change routes through
  desync recovery instead of forwarding stale-coordinate splices. See "Rules for the view".
- **List-marker span end pinned** — a non-empty item's marker span runs to the item's
  content start (all post-marker whitespace included, clamped to the marker's line),
  resolving v0.2 clarification 3's ambiguous "required trailing whitespace". See the
  amendment under that clarification.
- **Perf-gate enforcement note** — CI gates the loose perf ceilings (`perf_smoke`,
  `stream_perf`); the 1ms contract budget stays a local trip-wire (`perf_baseline`,
  informational in CI). See "Performance budget".

---

# v0.5 changelog

**The current version of this contract is v0.5** (M2 web-editor-beta dogfooding fixes). Same
in-place convention: each item lives inline above, tagged "(v0.5 amendment)". The full list:

- **`toggleTask` promotes instead of refusing** — `pos` outside any existing task item used to
  return `null` unconditionally (a documented v1 limitation); it now promotes the line containing
  `pos` into a task item — a non-task list item gets `"[ ] "` inserted after its marker, a plain
  paragraph/blockquote-content line or a blank line gets a fresh `"- [ ] "` marker — matching
  Obsidian's "Toggle checkbox status" behavior (research/07 §1.6). Still `null` on headings,
  code/fence lines, thematic breaks, and other block kinds a checkbox makes no sense on. See
  "`toggleTask` (v0.5 amendment — Obsidian parity)" under Commands.
- **`setHeading` same-level press toggles back to a paragraph** — `setHeading(pos, N)` where the
  line is already exactly level `N` used to no-op (`null`); it now removes the heading exactly
  like level 0 does, making the toolbar's H1–H6 buttons idempotent presses. A different level
  still replaces the prefix as before; level 0 is unchanged. See "`setHeading` (v0.4
  clarifications, v0.5 amendment)" under Commands.

---

# v0.6 changelog

**The current version of this contract is v0.6** (M2 web-editor-beta toolbar batch). Additive
only — six new commands, no changes to any existing rule. Everything lives in the "v0.6
commands" section under Commands; the headline decisions:

- **`toggleQuote`** — stepped blockquote toggle (research/07 §2.2): add one `> ` level to every
  intersecting line (blank lines included, keeping the quote contiguous), or remove ONE level
  per press when every intersecting non-blank line is already quoted. Never `null`; structural
  digit-rewrite guards preserve the itemness invariant in both directions.
- **`toggleLink`** — wrap (`[text](` + `)`, cursor in the URL slot; empty range `[]()`, cursor
  in the text slot) / unwrap (text survives; the URL is dropped — the documented round-trip
  asymmetry; autolinks shed their `<`/`>`). Single-line only; code contexts refuse. View binds
  Mod-k, always consuming the key.
- **`toggleBulletList` / `toggleOrderedList`** — line-wise conversion with toggle semantics: an
  all-target-flavor selection STRIPS back to plain lines (itemness-invariant exemption, like
  `enter`'s marker-clear), anything else CONVERTS in place. Pinned decisions: task items are
  bullet-flavor; converting to ordered strips task brackets; converting to bullet keeps them;
  already-target lines (raw digits included) are never rewritten; written ordered markers get
  sequential digits restarting at 1 per contiguous same-column run with delimiter/glyph
  adoption; the below-line interruption guard carries over from indentList/outdentList.
- **`insertHr`** — a thematic break after the line containing `pos`, with blank lines
  guaranteed above and below so the dashes can never parse as a setext-H2 underline (the
  paragraph-adjacent shape is pinned by a reparse-assertion test).
- **`toggleCodeBlock`** — wrap the intersecting lines in backtick fences (length computed so
  body lines can never close the fence early; selection lands inside the block) / remove both
  fence lines of an intersected block. Quote context is a documented v1 `null` punt.
- All six: one undo unit, origin `"command"`, UTF-16 arguments with the CRLF guard, and the
  whole-document itemness invariant (with the strip exemption above).
