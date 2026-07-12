# Oxidown Boundary Protocol — v0 (M0 spike)

The minimum contract between `oxidown-core` and a platform view, for the M0 spike (plan.md §8).
This document is authoritative: if the Rust core and the TypeScript view disagree, the one that
matches this file wins; if this file is wrong, change it in the same PR as the code.

Current contract version: **v0.3** — the base v0 sections below are followed by the v0.1
clarifications, the v0.2 (M1) additions, and inline v0.3 amendments; the "v0.3 changelog"
section at the end enumerates exactly what v0.3 comprises.

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

## Performance budget (M0 gate)

`applyEdit` + `decorations` for a ~3k-code-unit viewport on a 100KB document: **< 1ms combined
p95 in the core** (excluding DOM work), measured from the JS side of the wasm boundary.

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
   spans say. (The MockCore currently emits strong-outer — a known, documented deviation; the
   wasm core is authoritative.)
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
  and reveal as `mark:delim`, dash and brackets in lockstep.
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
command(name: "toggleTask", pos: number): CoreChange | null;  // pos anywhere in the list item
command(name: "indentList"|"outdentList", from: number, to: number): CoreChange | null;
command(name: "enter", from: number, to: number): CoreChange | null;  // v0.3 addition — see "enter" below
```

Commands are text transforms computed against the overlay (plan §5.8): they emit minimal
splices (toggle = add or remove delimiters), enter the op log with origin `"command"`, and are
single undo units (never coalesce). Returns null when the command doesn't apply at the target —
`indentList`/`outdentList` are the one exception to "null when nothing happens"; see below.
**`command()` either returns `null`/`CoreChange` or throws WITHOUT mutating the core** — planning
happens entirely before any apply, so a thrown command is not a mirror-desync signal; views must
not resync (`load()`) in response to one (contrast with `applyEdit`/`decorations`, where any
exception IS still a desync emergency per "Error handling" below).

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
with one deterministic structural rule (identical in the Rust core and the mock — a shared rule,
not post-hoc parser validation, so both cores agree even where their parsers differ in
leniency):

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
- `streamClose` on an unknown/closed id is a no-op (returns `null`); `streamAppend` on one throws.
- **`streamClose` returns `CoreChange | null` (v0.3 amendment — previously `void`).** When the
  stream's withheld trailing high surrogate is flushed as U+FFFD on close (see "Unpaired
  surrogates in payloads"), the resulting `CoreChange` is RETURNED so the view can apply it
  under its skip annotation like any other core-driven change — before this amendment the flush
  mutated the core but the change was silently dropped, desyncing the view's mirror. Returns
  `null` in the common nothing-pending case. The flush edit belongs to the STREAM'S single undo
  unit (it is the stream's last append), not a unit of its own. The surrogate buffering — and
  therefore the flush — lives at the adapter/mock layer: the raw wasm binding never buffers
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
4. **Link conceal spans** are two spans (`[` and `](url)`); on reveal they are emitted as
   delim/url/delim pieces.
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
Y (UTF-16 code units)` — the same error the mock, whose numbers have no 32-bit cliff, reaches
via its document-bounds check. Positions are NEVER silently truncated, wrapped, or clamped.

Three validation-refusal error names (v0.3):

- `InvalidArgs` — thrown by the wasm adapter/mock argument layer, before dispatch, when a raw
  argument is malformed (non-integer or negative numbers, a missing command argument, a
  `setHeading` level outside 0–6 at the boundary).
- `InvalidArgument` — thrown by the core when a value is semantically outside its documented
  domain (e.g. a heading level above 6 at the core API; an inline-toggle range spanning more
  than one leaf block).
- `InvalidPayload` — thrown by the adapter/mock payload layer when a STRUCTURED payload is
  malformed before it can cross the boundary: mis-shaped or non-serializable `splices` /
  `selections` (e.g. a negative or non-integer field — wasm:
  ``InvalidPayload: malformed splices: invalid value: integer `-1`, expected u32``; the mock
  mirrors the name and `malformed splices` / `malformed selections` prefixes), and text
  payloads carrying unpaired surrogates (see "Unpaired surrogates in payloads" below).

All three are refusals thrown WITHOUT mutating the core; callers should treat them as consumed
no-ops (log and move on — never fall back to a default action, never resync), per the
Commands section's no-mutation-on-throw rule. This carve-out applies from EVERY entry point,
`applyEdit` and `decorations` included: the desync-emergency rule above governs exceptions the
core throws once a well-formed call is underway, not these named pre-dispatch refusals.

## Unpaired surrogates in payloads (v0.3 addition)

Complementing v0.1 clarification 7 (which governs splice *positions*), the document TEXT
itself never contains an unpaired surrogate code unit:

- `load` and `applyEdit` throw `InvalidPayload: ...` when a text payload carries a lone
  surrogate (enforced at the adapter/mock layer, before the text crosses the boundary —
  wasm-bindgen's string conversion would otherwise silently corrupt it to U+FFFD).
- `streamAppend` buffers a TRAILING lone high surrogate per stream (adapter/mock behavior: a
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

**The current version of this contract is v0.3.** v0.3 is additive/amending on top of
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
- **Error names `InvalidArgs` / `InvalidArgument` / `InvalidPayload`** — argument-layer,
  core-level-semantic, and payload-layer validation refusals; all consumed no-ops (no resync
  obligation, from any entry point). Position arguments are bounded to `u32::MAX` at the wasm
  boundary (over-u32 values throw the ordinary `OutOfBounds`, never a silent truncation).
  See "Error handling".
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
