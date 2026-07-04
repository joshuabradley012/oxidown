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

export interface SelectionRange {
  anchor: number;
  head: number;
}

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
