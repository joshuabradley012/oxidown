/**
 * MockCore — a deliberately simple reference implementation of the Oxidown
 * boundary protocol (docs/boundary-v0.md), for developing and testing the
 * CM6 view before the real Rust/wasm core exists.
 *
 * Scope: the M0 markdown set only —
 *   - ATX headings `#`–`######` followed by a space
 *   - strong `**x**` / `__x__`
 *   - emphasis `*x*` / `_x_` (a `**` run must not parse as two `*`)
 *   - inline code `` `x` ``
 *   - nesting: `**bold *italic* bold**` and `***both***`
 *
 * It is NOT fast (it reparses the whole document per decorations() call and
 * stores whole-text snapshots per undo unit) and it does NOT handle general
 * markdown. It is meant to be obviously correct on the M0 set.
 *
 * All positions are UTF-16 code units (native JS string indices).
 */

import type {
  Decoration,
  DecorationLine,
  EditOrigin,
  OxidownCore,
  SelectionRange,
  Splice,
} from "./protocol.js";

const COALESCE_MS = 500;

export interface MockCoreOptions {
  /** Injectable clock for deterministic coalescing tests. Defaults to Date.now. */
  now?: () => number;
}

// ---------------------------------------------------------------------------
// Splice helpers
// ---------------------------------------------------------------------------

/** Apply ascending, non-overlapping, original-coordinate splices to a string. */
export function applySplices(doc: string, splices: Splice[]): string {
  const parts: string[] = [];
  let pos = 0;
  for (const s of splices) {
    parts.push(doc.slice(pos, s.at), s.insert);
    pos = s.at + s.delete;
  }
  parts.push(doc.slice(pos));
  return parts.join("");
}

/**
 * Minimal single-splice diff between two documents (common prefix/suffix trim).
 * Returns [] when the texts are identical.
 */
export function diffSplices(from: string, to: string): Splice[] {
  if (from === to) return [];
  let start = 0;
  const minLen = Math.min(from.length, to.length);
  while (start < minLen && from.charCodeAt(start) === to.charCodeAt(start)) start++;
  let endFrom = from.length;
  let endTo = to.length;
  while (
    endFrom > start &&
    endTo > start &&
    from.charCodeAt(endFrom - 1) === to.charCodeAt(endTo - 1)
  ) {
    endFrom--;
    endTo--;
  }
  return [{ at: start, delete: endFrom - start, insert: to.slice(start, endTo) }];
}

/** Map a position through an ascending splice batch (original → new coordinates). */
function mapPos(pos: number, splices: Splice[], assoc: -1 | 1): number {
  let shift = 0;
  for (const s of splices) {
    const end = s.at + s.delete;
    if (end < pos || (end === pos && (s.delete > 0 || assoc === 1))) {
      shift += s.insert.length - s.delete;
    } else if (s.at < pos) {
      // strictly inside the deleted range
      return s.at + shift + (assoc === 1 ? s.insert.length : 0);
    } else {
      break;
    }
  }
  return pos + shift;
}

// ---------------------------------------------------------------------------
// M0 parser (line-based; whole-document; not incremental — mock only)
// ---------------------------------------------------------------------------

interface InlineMark {
  from: number;
  to: number;
  style: "strong" | "em" | "code";
}

/** One formatted node. Reveal is decided per node over its full extent. */
interface ParsedNode {
  /** Full extent including delimiters. */
  start: number;
  end: number;
  /** Delimiter spans: concealed by default, `mark:delim` when revealed. */
  conceals: Array<[number, number]>;
  /** Content marks, always emitted. */
  marks: InlineMark[];
  /** Heading line decoration, always emitted. */
  line?: DecorationLine;
}

function runLength(src: string, i: number, ch: string): number {
  let n = 0;
  while (i + n < src.length && src[i + n] === ch) n++;
  return n;
}

/** Find the next run of `ch` with length >= minLen, starting at or after `from`. */
function findRun(src: string, from: number, ch: string, minLen: number): [number, number] | null {
  let j = from;
  while (j < src.length) {
    if (src[j] === ch) {
      const r = runLength(src, j, ch);
      if (r >= minLen) return [j, r];
      j += r;
    } else {
      j++;
    }
  }
  return null;
}

/** Parse inline constructs of `src` (positions offset by `base`) into `nodes`. */
function parseInline(src: string, base: number, nodes: ParsedNode[]): void {
  let i = 0;
  const n = src.length;
  while (i < n) {
    const ch = src[i];
    if (ch === "`") {
      const close = src.indexOf("`", i + 1);
      if (close !== -1) {
        nodes.push({
          start: base + i,
          end: base + close + 1,
          conceals: [
            [base + i, base + i + 1],
            [base + close, base + close + 1],
          ],
          marks: [{ from: base + i + 1, to: base + close, style: "code" }],
        });
        i = close + 1;
        continue;
      }
      i++;
      continue;
    }
    if (ch === "*" || ch === "_") {
      const run = runLength(src, i, ch);
      if (run >= 2) {
        // Strong: opener = first 2 chars of the run, closer = last 2 chars of
        // the next run of length >= 2. A run of 3 leaves one delimiter char on
        // each side inside the content, so `***x***` parses as strong(em(x)).
        const close = findRun(src, i + run, ch, 2);
        if (close) {
          const [cs, cl] = close;
          const contentFrom = i + 2;
          const contentTo = cs + cl - 2;
          nodes.push({
            start: base + i,
            end: base + cs + cl,
            conceals: [
              [base + i, base + i + 2],
              [base + contentTo, base + cs + cl],
            ],
            marks: [{ from: base + contentFrom, to: base + contentTo, style: "strong" }],
          });
          parseInline(src.slice(contentFrom, contentTo), base + contentFrom, nodes);
          i = cs + cl;
          continue;
        }
      }
      {
        // Emphasis: single-char delimiters.
        const close = findRun(src, i + run, ch, 1);
        if (close) {
          const [cs, cl] = close;
          const closerAt = cs + cl - 1;
          const contentFrom = i + 1;
          if (closerAt > contentFrom) {
            nodes.push({
              start: base + i,
              end: base + closerAt + 1,
              conceals: [
                [base + i, base + i + 1],
                [base + closerAt, base + closerAt + 1],
              ],
              marks: [{ from: base + contentFrom, to: base + closerAt, style: "em" }],
            });
            parseInline(src.slice(contentFrom, closerAt), base + contentFrom, nodes);
            i = closerAt + 1;
            continue;
          }
        }
      }
      i += run;
      continue;
    }
    i++;
  }
}

const HEADING_RE = /^(#{1,6}) /;

/** Parse the full document into formatted nodes. Line-based; M0 constructs only. */
export function parseDoc(doc: string): ParsedNode[] {
  const nodes: ParsedNode[] = [];
  let lineStart = 0;
  while (lineStart <= doc.length) {
    let lineEnd = doc.indexOf("\n", lineStart);
    if (lineEnd === -1) lineEnd = doc.length;
    const line = doc.slice(lineStart, lineEnd);
    const m = HEADING_RE.exec(line);
    if (m) {
      const level = m[1].length;
      nodes.push({
        start: lineStart,
        end: lineEnd,
        conceals: [[lineStart, lineStart + level + 1]],
        marks: [],
        line: { kind: "line", at: lineStart, style: `h${level}` as DecorationLine["style"] },
      });
      parseInline(line.slice(level + 1), lineStart + level + 1, nodes);
    } else {
      parseInline(line, lineStart, nodes);
    }
    if (lineEnd === doc.length) break;
    lineStart = lineEnd + 1;
  }
  return nodes;
}

// ---------------------------------------------------------------------------
// MockCore
// ---------------------------------------------------------------------------

export class MockCore implements OxidownCore {
  private doc = "";
  private rev = 0;
  private readonly now: () => number;

  /** Undo units store the full text before the unit (mock simplicity). */
  private undoStack: Array<{ before: string }> = [];
  private redoStack: Array<{ after: string }> = [];
  /** True while the top of undoStack is an open (coalescable) unit. */
  private hasOpenUnit = false;
  private lastEditTime = -Infinity;
  /** End of the last single-splice edit, in current-doc coordinates; -1 if unusable. */
  private lastEditEnd = -1;
  private lastOrigin: EditOrigin | null = null;

  private composing = false;
  private compFrom = 0;
  private compTo = 0;

  constructor(opts: MockCoreOptions = {}) {
    this.now = opts.now ?? Date.now;
  }

  load(text: string): number {
    this.doc = text;
    this.undoStack = [];
    this.redoStack = [];
    this.hasOpenUnit = false;
    this.lastEditTime = -Infinity;
    this.lastEditEnd = -1;
    this.lastOrigin = null;
    this.composing = false;
    // "Returns revision 0's successor" — revisions stay monotonic across
    // repeated load() calls so stale revision numbers can never be re-issued.
    this.rev++;
    return this.rev;
  }

  applyEdit(baseRevision: number, splices: Splice[], origin: EditOrigin): number {
    if (baseRevision !== this.rev) {
      throw new Error(`stale revision: edit based on ${baseRevision}, current is ${this.rev}`);
    }
    this.validateSplices(splices);
    const t = this.now();
    const newDoc = applySplices(this.doc, splices);

    if (origin === "user" || origin === "ime" || origin === "paste") {
      const coalesce =
        origin !== "paste" &&
        this.hasOpenUnit &&
        (this.lastOrigin === "user" || this.lastOrigin === "ime") &&
        splices.length === 1 &&
        this.isAdjacent(splices[0]) &&
        // Coalescing pauses during composition: the 500ms window does not
        // break a group while an IME session is open.
        (this.composing || t - this.lastEditTime <= COALESCE_MS);
      if (!coalesce) {
        this.undoStack.push({ before: this.doc });
        // A paste is a closed unit: nothing may coalesce into it.
        this.hasOpenUnit = origin !== "paste";
      }
      this.redoStack = [];
    } else {
      // "undo"/"redo" origins are not expected through applyEdit when the view
      // uses core-driven history (the view must NOT echo history splices back).
      // Defensively record them as isolated, non-coalescing units.
      this.undoStack.push({ before: this.doc });
      this.hasOpenUnit = false;
      this.redoStack = [];
    }

    if (splices.length === 1) {
      const s = splices[0];
      this.lastEditEnd = s.at + s.insert.length;
    } else {
      this.lastEditEnd = -1;
    }
    this.lastEditTime = t;
    this.lastOrigin = origin;

    if (this.composing) {
      // Keep the session range valid across edits; grow it to cover IME edits.
      this.compFrom = mapPos(this.compFrom, splices, -1);
      this.compTo = mapPos(this.compTo, splices, 1);
      if (origin === "ime" && splices.length > 0) {
        const first = splices[0];
        const last = splices[splices.length - 1];
        let shift = 0;
        for (let k = 0; k < splices.length - 1; k++) {
          shift += splices[k].insert.length - splices[k].delete;
        }
        this.compFrom = Math.min(this.compFrom, first.at);
        this.compTo = Math.max(this.compTo, last.at + shift + last.insert.length);
      }
    }

    this.doc = newDoc;
    this.rev++;
    return this.rev;
  }

  undo(): { revision: number; splices: Splice[] } | null {
    const unit = this.undoStack.pop();
    if (!unit) return null;
    this.redoStack.push({ after: this.doc });
    const splices = diffSplices(this.doc, unit.before);
    this.doc = unit.before;
    this.rev++;
    this.hasOpenUnit = false;
    this.lastOrigin = null;
    this.lastEditEnd = -1;
    return { revision: this.rev, splices };
  }

  redo(): { revision: number; splices: Splice[] } | null {
    const unit = this.redoStack.pop();
    if (!unit) return null;
    this.undoStack.push({ before: this.doc });
    const splices = diffSplices(this.doc, unit.after);
    this.doc = unit.after;
    this.rev++;
    this.hasOpenUnit = false;
    this.lastOrigin = null;
    this.lastEditEnd = -1;
    return { revision: this.rev, splices };
  }

  decorations(
    revision: number,
    from: number,
    to: number,
    selections: SelectionRange[],
  ): Decoration[] {
    if (revision !== this.rev) {
      throw new Error(`stale revision: decorations requested at ${revision}, current is ${this.rev}`);
    }
    if (from < 0 || to > this.doc.length || from > to) {
      throw new Error(`viewport out of bounds: [${from}, ${to}) in doc of length ${this.doc.length}`);
    }
    const nodes = parseDoc(this.doc);
    const out: Decoration[] = [];
    for (const node of nodes) {
      // Viewport filter: skip nodes that do not intersect [from, to).
      if (node.end < from || node.start > to) continue;

      // Reveal predicate: any selection range (cursor = empty range) that
      // intersects the node's full extent INCLUDING delimiters — touching a
      // boundary counts. Plus the composition stability rule: while an IME
      // session is open, every node intersecting the composition range is
      // revealed (no conceal spans may intersect the composition range).
      const revealed =
        selections.some((sel) => {
          const lo = Math.min(sel.anchor, sel.head);
          const hi = Math.max(sel.anchor, sel.head);
          return lo <= node.end && hi >= node.start;
        }) ||
        (this.composing && this.compFrom <= node.end && this.compTo >= node.start);

      if (node.line) out.push(node.line);
      for (const m of node.marks) {
        out.push({ kind: "mark", from: m.from, to: m.to, style: m.style });
      }
      for (const [cf, ct] of node.conceals) {
        if (revealed) {
          out.push({ kind: "mark", from: cf, to: ct, style: "delim" });
        } else {
          out.push({ kind: "conceal", from: cf, to: ct });
        }
      }
    }
    out.sort((a, b) => {
      const pa = a.kind === "line" ? a.at : a.from;
      const pb = b.kind === "line" ? b.at : b.from;
      return pa - pb;
    });
    return out;
  }

  compositionBegin(from: number, to: number): void {
    this.composing = true;
    this.compFrom = Math.max(0, Math.min(from, this.doc.length));
    this.compTo = Math.max(this.compFrom, Math.min(to, this.doc.length));
    // A composition session forms its own undo unit: close the current group.
    // While the session is open, the 500ms window does not break the group
    // ("coalescing pauses during composition").
    this.hasOpenUnit = false;
  }

  compositionEnd(): void {
    this.composing = false;
    // A composition session forms its own undo unit: close the group so the
    // next edit starts fresh.
    this.hasOpenUnit = false;
  }

  getText(): string {
    return this.doc;
  }

  docLength(): number {
    return this.doc.length;
  }

  revision(): number {
    return this.rev;
  }

  private isAdjacent(s: Splice): boolean {
    return (
      this.lastEditEnd >= 0 && s.at <= this.lastEditEnd && s.at + s.delete >= this.lastEditEnd
    );
  }

  private validateSplices(splices: Splice[]): void {
    let prevEnd = 0;
    for (const s of splices) {
      if (s.at < 0 || s.delete < 0 || s.at + s.delete > this.doc.length) {
        throw new Error(
          `splice out of bounds: at=${s.at} delete=${s.delete} docLength=${this.doc.length}`,
        );
      }
      if (s.at < prevEnd) {
        throw new Error(`splices overlap or are not ascending at position ${s.at}`);
      }
      prevEnd = s.at + s.delete;
    }
  }
}
