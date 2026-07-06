# Oxidown Boundary Protocol — v0 (M0 spike)

The minimum contract between `oxidown-core` and a platform view, for the M0 spike (plan.md §8).
This document is authoritative: if the Rust core and the TypeScript view disagree, the one that
matches this file wins; if this file is wrong, change it in the same PR as the code.

Web-boundary flavor: **all positions in this protocol are UTF-16 code units** (CodeMirror's unit).
The wasm crate converts to core-internal UTF-8 byte offsets. Core internals never leak bytes.

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
  to: number; // delimiter chars to visually collapse — views must NOT remove them from the DOM
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

- Conceal by **visual collapse** (e.g. `font-size: 0.01em` + `letter-spacing`), never by removing
  characters from the DOM: line heights must not change between concealed and revealed states.
- Do not rebuild decorations mid mouse-drag; recompute on drag end.
- Do not request/rebuild decorations while `EditorView.composing` is true; recompute on
  composition end. Pair with `compositionBegin`/`compositionEnd` calls into the core.
- Apply `undo`/`redo` splices with an annotation the change-forwarding path recognizes, so they
  are not echoed back into `applyEdit`.

## Performance budget (M0 gate)

`applyEdit` + `decorations` for a ~3k-code-unit viewport on a 100KB document: **< 1ms combined
p95 in the core** (excluding DOM work), measured from the JS side of the wasm boundary.

## Clarifications (v0.1 — pinned after first implementations)

1. **Composition vs coalescing:** `compositionBegin` closes any open undo group; while composing,
   the 500ms window does not break the group; `compositionEnd` closes the group. A composition
   session is therefore exactly one undo unit.
2. **`load` revisions:** first `load` returns 1; revisions are monotonic across repeated `load`
   calls on the same instance (stale revision numbers are never re-issued).
3. **`***x***`:** per CommonMark, emphasis is the outer node (`<em><strong>x</strong></em>`).
   Views must not depend on which node owns which delimiter characters beyond what the emitted
   spans say. (The MockCore currently emits strong-outer — a known, documented deviation; the
   wasm core is authoritative.)
4. **Undo-coalescing adjacency:** a new edit coalesces if its `[at, at + delete]` range touches
   the previous edit's end position. Multi-splice batches never coalesce.
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
//   list-marker = bullet/number markers ("- ", "1. ") — always visible, styled, never concealed
// line styles (added): "blockquote" (with depth) | "code-block" | "code-fence" | "hr"
export interface DecorationLineV2 {
  kind: "line";
  at: number;
  style: "h1"|"h2"|"h3"|"h4"|"h5"|"h6"|"blockquote"|"code-block"|"code-fence"|"hr";
  /** blockquote nesting depth (1-based); present only for style "blockquote". */
  depth?: number;
}
export interface DecorationWidget {
  kind: "widget";
  from: number;
  to: number;                 // source range the widget REPLACES visually
  widget: "task" | "bullet";  // task: "[ ]"/"[x]" (carries checked); bullet: the whole "- " marker (LINE-level reveal)
  checked?: boolean;
}
```

M1 emission scope (parser may understand more than it decorates):
- Strikethrough `~~x~~` — mark `strike` + conceal/delim pairs, same reveal rules as strong/em.
- Links `[text](url)` — concealed: `mark:link` over text, conceal `[` and `](url)`.
  Revealed: delimiters as `mark:delim`, destination as `mark:url`. Autolinks: `mark:link` whole.
- Blockquotes — `line:blockquote` per line with depth; `> ` markers conceal, reveal per-line.
- Fenced code blocks — `line:code-fence` on fence lines, `line:code-block` on body lines,
  `mark:code` on body content. The raw fence text (``` + info string) conceals; reveal is
  **BLOCK-level**: a cursor/selection anywhere inside the fenced block (either fence or the
  body) reveals BOTH raw fences as `mark:delim`, so they are editable whenever the block is.
- Lists — ordered markers emit `mark:list-marker` (always visible; alignment is view styling:
  fixed-width right-aligned box + tabular numerals). **Unordered markers emit `widget:bullet`
  replacing the whole marker span (`"- "`)**, revealed as `mark:list-marker` under STRICT
  interior overlap (`a < end && b > start`) — the cursor at the item text's first character or
  at line start does not flash the raw marker. **Marker reveal is Obsidian-style adjacency**:
  the reveal extent is the marker region itself (closed-interval touch) — the `- ` run, or for
  task items the combined `- [ ]` run (dash and brackets reveal in LOCKSTEP, as `mark:delim`).
  A caret in the item's text does NOT reveal; a caret directly next to the marker does.
- **Every list item line** emits `{kind:"line", style:"list-item", depth, revealed?}` (1-based
  depth) at the marker position — the view uses it for hanging indent (wrapped item text aligns
  with the first line's text). Nested items (depth ≥ 2) additionally emit a `conceal` over the
  raw leading indent whitespace (revealed as `mark:delim`); the view supplies exact per-depth
  padding (1.5em per level) so each nested marker starts at its parent's text column.
- **`revealed: true` on `blockquote`/`list-item` lines** means the line's marker region is
  being edited (caret adjacent): the view drops ALL decorative padding/bars/indent for that
  line and renders default source geometry — the raw markers and real leading spaces sit at
  their true positions, so deeply nested prefixes (e.g. `> > - `) are edited as plain source.
  Blockquote line reveal extents are likewise the marker run only, per line.
- Thematic break — `line:hr` on the line, plus `conceal` over the raw dashes (revealed as
  `mark:delim` when the cursor is on the line). The view draws the actual rule on the hr line;
  nested blockquote bars are likewise the view's job (one bar per depth level).
- Headings/strong/em/inline-code unchanged from v0.

Reveal semantics are unchanged: per-node selection∩extent (task widget reveal = the LIST ITEM's
marker extent, so clicking the rendered checkbox — which sits inside the range — still works).

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

## Commands

```ts
command(name: "toggleStrong"|"toggleEm"|"toggleStrike"|"toggleCode", from: number, to: number): CoreChange | null;
command(name: "setHeading", pos: number, level: 0|1|2|3|4|5|6): CoreChange | null;   // 0 = paragraph
command(name: "toggleTask", pos: number): CoreChange | null;  // pos anywhere in the list item
```

Commands are text transforms computed against the overlay (plan §5.8): they emit minimal
splices (toggle = add or remove delimiters), enter the op log with origin `"command"`, and are
single undo units (never coalesce). Returns null when the command doesn't apply at the target.

## Streaming (plan §5.9)

```ts
streamOpen(pos: number): number;                    // stream id; insertion point becomes an internal anchor
streamAppend(id: number, chunk: string): CoreChange; // splices for the view to apply (skip annotation)
streamClose(id: number): void;
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
- `streamClose` on an unknown/closed id is a no-op; `streamAppend` on one throws.

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
4. **Link conceal spans** are two spans (`[` and `](url)`); on reveal they are emitted as
   delim/url/delim pieces.

## Error handling

Stale revision, overlapping splices, or out-of-bounds positions: throw (wasm: `Err` → JS
exception). The view treats any core exception as a mirror-desync emergency: re-`load()` from
the view buffer and log loudly. Never continue silently.
