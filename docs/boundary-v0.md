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

## Error handling

Stale revision, overlapping splices, or out-of-bounds positions: throw (wasm: `Err` → JS
exception). The view treats any core exception as a mirror-desync emergency: re-`load()` from
the view buffer and log loudly. Never continue silently.
