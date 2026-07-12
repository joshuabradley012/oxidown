/**
 * Oxidown boundary protocol — v0 through v0.3 (M0 base + M1 additions).
 *
 * Transcribed EXACTLY from docs/boundary-v0.md ("TypeScript interface
 * (authoritative shape)", plus the v0.2 additions and v0.3 amendments). If
 * this file and that document disagree, the document wins; fix this file in
 * the same PR.
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

/**
 * Error names a core may refuse a call with (docs/boundary-v0.md "Error
 * handling"): every thrown error's message starts with one of these names
 * followed by ": ". Views route on the name — `StaleRevision` (and any
 * exception from a call that may have mutated) is a mirror-desync emergency;
 * the validation refusals are thrown BEFORE any mutation.
 *
 * - "StaleRevision"    — baseRevision/revision is not the core's current one
 * - "OutOfBounds"      — a position beyond the document length (direct
 *                        argument OR inside a splices/selections payload)
 * - "InvalidRange"     — from > to on a query range
 * - "InvalidArgs"      — malformed direct numeric argument / command arity
 *                        (the Rust core's own guards spell one refusal
 *                        "InvalidArgument"; assert by /^InvalidArg/ across
 *                        cores)
 * - "InvalidPayload"   — malformed splices/selections payload, or a text
 *                        payload carrying an unpaired surrogate
 * - "InvalidSplice"    — splices not ascending/non-overlapping
 * - "InvalidOrigin"    — unknown EditOrigin string
 * - "InvalidBias"      — createAnchor bias not "before"/"after"
 * - "InvalidCommand"   — unknown command name
 * - "SurrogateSplit"   — a mutation position inside a surrogate pair
 * - "UnknownStream"    — streamAppend on a never-opened/closed stream id
 */
export type CoreErrorName =
  | "StaleRevision"
  | "OutOfBounds"
  | "InvalidRange"
  | "InvalidArgs"
  | "InvalidPayload"
  | "InvalidSplice"
  | "InvalidOrigin"
  | "InvalidBias"
  | "InvalidCommand"
  | "SurrogateSplit"
  | "UnknownStream";

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
  to: number; // delimiter chars to conceal — they stay in the DOCUMENT (see the contract's "Rules for the view")
}
export interface DecorationLine {
  kind: "line";
  /** Position anywhere on the target line (view resolves to the line). */
  at: number;
  // v0.2 additions: "blockquote" | "code-block" | "code-fence" | "hr" | "list-item".
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
   * For "blockquote"/"list-item": the line is revealed — reveal is
   * LINE-level (v0.3), so a cursor/selection touching ANY part of the line
   * reveals all of its marker constructs together, and the view drops the
   * line's decorative padding/bars and renders default source geometry.
   * Omitted from the wire when false.
   */
  revealed?: boolean;
}
/**
 * v0.2 addition: a replace-range island. `from`/`to` is the source range the
 * widget visually REPLACES ("[ ]" / "[x]"); the view renders `widget` in its
 * place. Views MUST ignore widget kinds they don't recognize.
 *
 * v0.3 addition: "ordered" — see boundary-v0.md's v0.3 amendment
 * ("view-computed ordered-list numbering", research/07 §0/§1.2). `number`/
 * `delim` are optional on the wire (forward-compat with v0.2 views, and
 * defensive for any future widget kind that doesn't need them), but are
 * always present together when `widget === "ordered"`.
 */
export interface DecorationWidget {
  kind: "widget";
  from: number;
  to: number;
  /**
   * "task": replaces the `[ ]`/`[x]` span (carries `checked`); withheld as
   * `mark:delim` on reveal, with the item's `- ` run in lockstep.
   * "bullet": replaces an unordered item's whole marker span (glyph +
   * trailing whitespace, e.g. `"- "`); withheld as `mark:list-marker` on
   * reveal. Reveal is LINE-level (contract v0.3, matching every other
   * marker construct): a cursor/selection touching any part of the item's
   * line shows the raw marker instead.
   * "ordered": replaces an ordered item's whole marker span (`"1. "`) with
   * the VIEW-COMPUTED CommonMark sequence number (carried in `number`) plus
   * its delimiter (`delim`, `"."` or `")"`) — the core NEVER rewrites source
   * digits (research/07: Obsidian's renumber-by-rewriting-the-file approach,
   * avoided). Reveal is LINE-level, matching every other marker construct: a
   * cursor anywhere on the item's line withholds the widget in favor of the
   * raw source digits as `mark:list-marker`.
   */
  widget: "task" | "bullet" | "ordered";
  checked?: boolean;
  /** "ordered" only: the view-computed display number. */
  number?: number;
  /** "ordered" only: the marker's delimiter character, `"."` or `")"`. */
  delim?: string;
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

/**
 * v0.2/v0.3 command names accepting a (from, to) range. `indentList`/
 * `outdentList` are marker-width-aware Tab nesting (docs/boundary-v0.md
 * "indentList / outdentList"): they apply only when the range touches a
 * list-item line, and return a NO-OP CoreChange (empty splices) rather than
 * `null` when they apply but no movement is possible. `enter` (v0.3,
 * docs/boundary-v0.md "enter") is construct-aware Enter — continue a list
 * marker/quote prefix on non-empty content, exit an EMPTY one in a SINGLE
 * press: `null` when neither construct applies at the target (the view
 * falls back to the default newline); unlike indentList/outdentList it
 * never returns an empty-splice no-op (every applicable case edits).
 */
export type RangeCommandName =
  | "toggleStrong"
  | "toggleEm"
  | "toggleStrike"
  | "toggleCode"
  | "indentList"
  | "outdentList"
  | "enter";

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
   * doesn't apply at the target — EXCEPT `indentList`/`outdentList`, which
   * return null only when no line in the range carries a list marker; when
   * they apply but no movement is possible they return a CoreChange with
   * empty splices (a no-op the view still shouldn't fall back from).
   */
  command(name: RangeCommandName, from: number, to: number): CoreChange | null;
  command(name: "setHeading", pos: number, level: 0 | 1 | 2 | 3 | 4 | 5 | 6): CoreChange | null;
  command(name: "toggleTask", pos: number): CoreChange | null; // pos anywhere in the list item

  /**
   * Streaming ingestion (plan §5.9). An ENTIRE stream session (open→close) is
   * exactly ONE undo unit; ops carry origin "ai". The insertion point becomes
   * an internal anchor so concurrent user edits above/below it interleave
   * correctly. `streamClose` on an unknown/closed id is a no-op (returns
   * null); `streamAppend` on one throws.
   *
   * `streamClose` may itself edit the document: a trailing high surrogate
   * withheld from the last chunk can never be completed once the stream
   * closes, so it is flushed as one U+FFFD — a final append belonging to the
   * stream's single undo unit. When that happens, the flush's CoreChange is
   * RETURNED and the view MUST apply it exactly like a `streamAppend` result
   * (skip annotation; no selection — the user's cursor maps through).
   * Returns null when no flush was needed (the common case).
   */
  streamOpen(pos: number): number; // stream id; insertion point becomes an internal anchor
  streamAppend(id: number, chunk: string): CoreChange; // splices for the view to apply (skip annotation)
  streamClose(id: number): CoreChange | null; // U+FFFD-flush change to apply, or null

  /**
   * Optional teardown. Releases resources held outside the JS heap (the wasm
   * adapter frees its wasm-bindgen instance); idempotent. The core is
   * unusable afterwards. Implementations with nothing to free may omit it.
   */
  destroy?(): void;
}
