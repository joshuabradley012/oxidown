/**
 * Oxidown boundary protocol — v0 (M0 spike).
 *
 * Transcribed EXACTLY from docs/boundary-v0.md ("TypeScript interface
 * (authoritative shape)"). If this file and that document disagree, the
 * document wins; fix this file in the same PR.
 *
 * Web-boundary flavor: all positions in this protocol are UTF-16 code units
 * (CodeMirror's unit). The wasm crate converts to core-internal UTF-8 byte
 * offsets. Core internals never leak bytes.
 */

/** Positions are UTF-16 code units into the current document unless stated otherwise. */
export interface Splice {
  /** Position in the document BEFORE this edit batch (original-doc coordinates). */
  at: number;
  /** Number of code units to delete starting at `at`. */
  delete: number;
  insert: string;
}

// v0.2: EditOrigin gains "ai" (stream ops) and "command" (command ops). Neither
// ever coalesces (docs/boundary-v0.md "New edit origins").
export type EditOrigin = "user" | "ime" | "paste" | "undo" | "redo" | "ai" | "command";

export interface DecorationMark {
  kind: "mark";
  from: number;
  to: number;
  // v0.2 additions: "strike" | "link" | "url" | "list-marker" (see boundary-v0.md
  // "Expanded decoration vocabulary"). Views MUST ignore styles they don't
  // recognize rather than throw, for forward compatibility.
  style: "strong" | "em" | "code" | "delim" | "strike" | "link" | "url" | "list-marker";
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
  // v0.2 additions: "blockquote" | "code-block" | "code-fence" | "hr".
  style:
    | "h1"
    | "h2"
    | "h3"
    | "h4"
    | "h5"
    | "h6"
    | "blockquote"
    | "code-block"
    | "code-fence"
    | "hr"
    | "list-item";
  /** Nesting depth (1-based); present for styles "blockquote" and "list-item". */
  depth?: number;
  /**
   * For "blockquote"/"list-item": the line's marker region is being edited
   * (caret adjacent), so the view drops the line's decorative padding/bars
   * and renders default source geometry.
   */
  revealed?: boolean;
}
/**
 * v0.2 addition: a replace-range island. `from`/`to` is the source range the
 * widget visually REPLACES ("[ ]" / "[x]"); the view renders `widget` in its
 * place. Views MUST ignore widget kinds they don't recognize.
 */
export interface DecorationWidget {
  kind: "widget";
  from: number;
  to: number;
  /**
   * "task": replaces the `[ ]`/`[x]` span (carries `checked`).
   * "bullet": replaces an unordered item's whole marker span (`"- "`); reveal
   * is STRICT interior overlap, so the cursor at the item text's first
   * character never flashes the raw marker.
   */
  widget: "task" | "bullet";
  checked?: boolean;
}
export type Decoration = DecorationMark | DecorationConceal | DecorationLine | DecorationWidget;

export interface SelectionRange {
  anchor: number;
  head: number;
}

/**
 * v0.2: the shape shared by `undo`/`redo`, `command`, and `streamAppend` —
 * splices the view must apply verbatim under its skip annotation, plus an
 * optional cursor placement (docs/boundary-v0.md "Core-driven changes").
 */
export interface CoreChange {
  revision: number;
  splices: Splice[]; // current-doc coordinates
  selection?: { anchor: number; head: number } | null;
}

/** v0.2 command names accepting a (from, to) range. */
export type RangeCommandName = "toggleStrong" | "toggleEm" | "toggleStrike" | "toggleCode";

export interface OxidownCore {
  /** Create/replace the document. Returns revision 0's successor. */
  load(text: string): number;

  /**
   * Apply an edit batch. `splices` are non-overlapping, ascending, in original-doc coordinates
   * (CM6 ChangeSet semantics). Returns the new revision. Must be O(edit + dirty block), not O(doc).
   */
  applyEdit(baseRevision: number, splices: Splice[], origin: EditOrigin): number;

  /**
   * Core-driven history. Returns a CoreChange (splices in CURRENT-doc coordinates for
   * the view to apply, plus the resulting revision and an optional cursor placement),
   * or null if the stack is empty. v0.2: this is the same shape `command` and
   * `streamAppend` return — see "Core-driven changes" below.
   * Coalescing: consecutive `user`/`ime` edits within 500ms and adjacent positions group
   * into one undo unit; `paste` always breaks the group. Coalescing pauses during composition.
   */
  undo(): CoreChange | null;
  redo(): CoreChange | null;

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

  // -------------------------------------------------------------------------
  // v0.2 additions (M1) — additive only; see boundary-v0.md "v0.2 additions"
  // -------------------------------------------------------------------------

  /**
   * Anchors (public position type, plan §5.3). Survive arbitrary edits (mapped
   * through every splice, bias-aware: "before" stays put when an insertion
   * lands exactly on it; "after" moves with the insertion). Deleting the
   * anchored text collapses the anchor to the deletion site — it does not
   * become null in M1.
   */
  createAnchor(pos: number, bias: "before" | "after"): number; // anchor id
  resolveAnchor(id: number): number | null; // current position; null if unresolvable
  dropAnchor(id: number): void;

  /**
   * Commands are text transforms computed against the overlay (plan §5.8):
   * minimal splices (toggle = add/remove delimiters), origin "command",
   * always a single non-coalescing undo unit. Returns null when the command
   * doesn't apply at the target.
   */
  command(name: RangeCommandName, from: number, to: number): CoreChange | null;
  command(name: "setHeading", pos: number, level: 0 | 1 | 2 | 3 | 4 | 5 | 6): CoreChange | null;
  command(name: "toggleTask", pos: number): CoreChange | null; // pos anywhere in the list item

  /**
   * Streaming ingestion (plan §5.9). An ENTIRE stream session (open→close) is
   * exactly ONE undo unit; ops carry origin "ai". The insertion point becomes
   * an internal anchor so concurrent user edits above/below it interleave
   * correctly. `streamClose` on an unknown/closed id is a no-op; `streamAppend`
   * on one throws.
   */
  streamOpen(pos: number): number; // stream id; insertion point becomes an internal anchor
  streamAppend(id: number, chunk: string): CoreChange; // splices for the view to apply (skip annotation)
  streamClose(id: number): void;
}
