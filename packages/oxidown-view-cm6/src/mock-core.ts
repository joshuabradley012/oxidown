/**
 * MockCore — a deliberately simple reference implementation of the Oxidown
 * boundary protocol (docs/boundary-v0.md), for developing and testing the
 * CM6 view before the real Rust/wasm core exists.
 *
 * Scope: the M0 markdown set (ATX headings, strong, emphasis, inline code)
 * plus a M1/v0.2 SUBSET of the expanded vocabulary — strikethrough, links
 * (incl. autolinks), blockquotes (depth tracked, "depth 1" is the well-tested
 * case), fenced code blocks, list markers + task-list checkboxes, and
 * thematic breaks. Full GFM breadth lives in the Rust core; this mock only
 * needs to be obviously correct on simple, single-level cases.
 *
 * It is NOT fast (it reparses the whole document per decorations() call and
 * stores whole-text snapshots per undo unit) and it does NOT handle general
 * markdown. All positions are UTF-16 code units (native JS string indices).
 */

import type {
  CoreChange,
  Decoration,
  DecorationLine,
  EditOrigin,
  OxidownCore,
  RangeCommandName,
  SelectionRange,
  Splice,
} from "./protocol.js";
import { endOfLastSplice } from "./splices.js";

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
// M0 + M1(subset) parser (line-based; whole-document; not incremental — mock only)
// ---------------------------------------------------------------------------

interface InlineMark {
  from: number;
  to: number;
  style: "strong" | "em" | "code" | "strike" | "link" | "list-marker";
}

/** One formatted node. Reveal is decided per node over its full extent. */
interface ParsedNode {
  /** Full extent including delimiters (or, for line-scoped nodes, the whole line). */
  start: number;
  end: number;
  /** Delimiter spans: concealed by default, `mark:delim` when revealed. */
  conceals: Array<[number, number]>;
  /** Content marks, always emitted regardless of reveal state. */
  marks: InlineMark[];
  /** Heading/blockquote/code-fence/code-block/hr line decoration, always emitted. */
  line?: DecorationLine;
  /**
   * Present for concealable links: conceals[0] is the "[" span, conceals[1]
   * is the "](url)" span; this field is the url's own [from, to). Revealed
   * rendering splits conceals[1] into delim/url/delim instead of one delim.
   */
  linkUrl?: [number, number];
  /**
   * Present for task list items: the "[ ]"/"[x]" span, replaced by a
   * widget:task decoration when concealed, or a delim mark when revealed.
   */
  widget?: { from: number; to: number; checked: boolean };
  /**
   * Present for unordered-list marker nodes: the whole "- " span, replaced
   * by a widget:bullet when concealed (STRICT interior reveal — the cursor
   * at the item text's first character does not reveal).
   */
  bullet?: { from: number; to: number };
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

const AUTOLINK_RE = /^<((?:https?|ftp):\/\/[^\s<>]+)>/;

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
    if (ch === "~") {
      const run = runLength(src, i, "~");
      if (run >= 2) {
        const close = findRun(src, i + run, "~", 2);
        if (close) {
          const [cs, cl] = close;
          const contentFrom = i + 2;
          const contentTo = cs + cl - 2;
          if (contentTo > contentFrom) {
            nodes.push({
              start: base + i,
              end: base + cs + cl,
              conceals: [
                [base + i, base + i + 2],
                [base + contentTo, base + cs + cl],
              ],
              marks: [{ from: base + contentFrom, to: base + contentTo, style: "strike" }],
            });
            parseInline(src.slice(contentFrom, contentTo), base + contentFrom, nodes);
            i = cs + cl;
            continue;
          }
        }
      }
      i += Math.max(run, 1);
      continue;
    }
    if (ch === "<") {
      const m = AUTOLINK_RE.exec(src.slice(i));
      if (m) {
        nodes.push({
          start: base + i,
          end: base + i + m[0].length,
          conceals: [],
          marks: [{ from: base + i, to: base + i + m[0].length, style: "link" }],
        });
        i += m[0].length;
        continue;
      }
      i++;
      continue;
    }
    if (ch === "[") {
      const closeBracket = src.indexOf("]", i + 1);
      if (closeBracket !== -1 && src[closeBracket + 1] === "(") {
        const closeParen = src.indexOf(")", closeBracket + 2);
        if (closeParen !== -1 && closeParen > closeBracket + 2) {
          const textFrom = i + 1;
          const textTo = closeBracket;
          const urlFrom = closeBracket + 2;
          const urlTo = closeParen;
          nodes.push({
            start: base + i,
            end: base + closeParen + 1,
            conceals: [
              [base + i, base + i + 1],
              [base + textTo, base + closeParen + 1],
            ],
            marks: [{ from: base + textFrom, to: base + textTo, style: "link" }],
            linkUrl: [base + urlFrom, base + urlTo],
          });
          i = closeParen + 1;
          continue;
        }
      }
      i++;
      continue;
    }
    i++;
  }
}

const HEADING_RE = /^(#{1,6}) /;
const HR_RE = /^ {0,3}([-*_])(?: *\1){2,}[ \t]*$/;
const FENCE_RE = /^ {0,3}(`{3,}|~{3,})(.*)$/;
const CLOSE_FENCE_RE = /^ {0,3}(`{3,}|~{3,})\s*$/;
const BQ_MARKER_RE = /^ {0,3}>[ \t]?/;
const LIST_MARKER_RE = /^(\s{0,3})([-*+]|\d{1,9}[.)])(?:[ \t]+)/;
const TASK_RE = /^\[([ xX])\] /;

interface ListMarkerMatch {
  markerFrom: number;
  /** End of the marker GLYPHS (`-`, `1.`) — reveal adjacency stops here. */
  glyphTo: number;
  /** End of the WHOLE matched marker run, e.g. "- " or "1. " (glyph + required space) — matches the spec's literal examples. */
  contentFrom: number;
}

/** Match a list-item marker (bullet or ordered) at the start of `line`. */
function matchListMarker(line: string): ListMarkerMatch | null {
  const m = LIST_MARKER_RE.exec(line);
  if (!m) return null;
  const markerFrom = m[1].length;
  const contentFrom = m[0].length;
  const glyphTo = markerFrom + m[2].length; // end of `-`/`1.` (no trailing ws)
  return { markerFrom, contentFrom, glyphTo };
}

/**
 * Parse one "logical line" of content (heading / list item / plain paragraph
 * text) at document offset `base`, pushing nodes into `nodes`. Shared by the
 * top-level line loop and by blockquote line remainders.
 */
function parseLineContent(content: string, base: number, nodes: ParsedNode[]): void {
  const headingM = HEADING_RE.exec(content);
  if (headingM) {
    const level = headingM[1].length;
    nodes.push({
      start: base,
      end: base + content.length,
      conceals: [[base, base + level + 1]],
      marks: [],
      line: { kind: "line", at: base, style: `h${level}` as DecorationLine["style"] },
    });
    parseInline(content.slice(level + 1), base + level + 1, nodes);
    return;
  }

  const listM = matchListMarker(content);
  if (listM) {
    const { markerFrom, contentFrom, glyphTo } = listM;
    const isBullet = /[-*+]/.test(content[markerFrom]);
    // Depth approximated as floor(indent/2) + 1 (2-space bullet / 3-space
    // ordered nesting, close enough for the mock). EVERY item line emits
    // line:list-item (the view's hanging indent); nested indents conceal.
    const depth = Math.floor(markerFrom / 2) + 1;
    nodes.push({
      // Indent + marker GLYPH region (trailing space excluded): caret
      // adjacency here flags the line revealed AND reveals the indent, so a
      // revealed marker always shows its raw leading spaces.
      start: base,
      end: base + glyphTo,
      conceals: depth >= 2 && markerFrom > 0 ? [[base, base + markerFrom]] : [],
      marks: [],
      line: { kind: "line", at: base, style: "list-item", depth },
    });
    const afterMarker = content.slice(contentFrom);
    const taskM = TASK_RE.exec(afterMarker);
    if (taskM) {
      // Widen the li-line node (just pushed) to the checkbox end so the
      // revealed flag + indent reveal share the task's `- [ ]` extent.
      nodes[nodes.length - 1].end = base + contentFrom + 3;
      // Task items: the `- ` marker conceals (no bullet widget) and reveals
      // in lockstep with the checkbox — LINE-level (node spans the line).
      const checkboxFrom = base + contentFrom;
      const checkboxTo = checkboxFrom + 3; // "[ ]" / "[x]"
      const itemContentFrom = contentFrom + taskM[0].length;
      nodes.push({
        // Leading spaces + `- [ ]` run: adjacency reveal (incl. the nesting
        // spaces), lockstep dash+brackets.
        start: base,
        end: checkboxTo,
        conceals: [[base + markerFrom, base + contentFrom]],
        marks: [],
        widget: { from: checkboxFrom, to: checkboxTo, checked: taskM[1] !== " " },
      });
      parseInline(content.slice(itemContentFrom), base + itemContentFrom, nodes);
      return;
    }
    const marks: InlineMark[] = [];
    if (isBullet) {
      // Bullet node spans leading spaces + marker GLYPH: a caret in the
      // nesting spaces or next to the `-` reveals the raw source (the
      // position after the trailing space does not).
      nodes.push({
        start: base,
        end: base + glyphTo,
        conceals: [],
        marks: [],
        bullet: { from: base + markerFrom, to: base + contentFrom },
      });
    } else {
      marks.push({ from: base + markerFrom, to: base + contentFrom, style: "list-marker" });
    }
    nodes.push({ start: base + markerFrom, end: base + contentFrom, conceals: [], marks });
    parseInline(content.slice(contentFrom), base + contentFrom, nodes);
    return;
  }

  parseInline(content, base, nodes);
}

/** Parse the full document into formatted/line/widget nodes. */
export function parseDoc(doc: string): ParsedNode[] {
  const nodes: ParsedNode[] = [];
  let lineStart = 0;
  while (lineStart <= doc.length) {
    let lineEnd = doc.indexOf("\n", lineStart);
    if (lineEnd === -1) lineEnd = doc.length;
    const line = doc.slice(lineStart, lineEnd);

    // Fenced code blocks: consume lines until the matching close fence (or EOF).
    // Never concealed; body content is marked `code` but not inline-parsed.
    const fenceM = FENCE_RE.exec(line);
    if (fenceM) {
      const fenceChar = fenceM[1][0];
      const fenceLen = fenceM[1].length;
      // BLOCK-level reveal: fence nodes span the whole fenced block, so a
      // cursor anywhere inside it reveals both raw fences. The open node is
      // widened once the close fence is found (below).
      const openIdx = nodes.length;
      nodes.push({
        start: lineStart,
        end: lineEnd,
        conceals: lineEnd > lineStart ? [[lineStart, lineEnd]] : [],
        marks: [],
        line: { kind: "line", at: lineStart, style: "code-fence" },
      });
      let cursor = lineEnd;
      while (cursor < doc.length) {
        const bLineStart = cursor + 1;
        let bLineEnd = doc.indexOf("\n", bLineStart);
        if (bLineEnd === -1) bLineEnd = doc.length;
        const bLine = doc.slice(bLineStart, bLineEnd);
        const closeM = CLOSE_FENCE_RE.exec(bLine);
        if (closeM && closeM[1][0] === fenceChar && closeM[1].length >= fenceLen) {
          nodes.push({
            start: lineStart, // block start: reveal from anywhere inside
            end: bLineEnd,
            conceals: bLineEnd > bLineStart ? [[bLineStart, bLineEnd]] : [],
            marks: [],
            line: { kind: "line", at: bLineStart, style: "code-fence" },
          });
          nodes[openIdx].end = bLineEnd;
          cursor = bLineEnd;
          break;
        }
        nodes.push({
          start: bLineStart,
          end: bLineEnd,
          conceals: [],
          marks: bLineEnd > bLineStart ? [{ from: bLineStart, to: bLineEnd, style: "code" }] : [],
          line: { kind: "line", at: bLineStart, style: "code-block" },
        });
        cursor = bLineEnd;
      }
      if (cursor >= doc.length) break;
      lineStart = cursor + 1;
      continue;
    }

    // Thematic break: hr line style + the raw dashes concealed (revealed as
    // delim when the cursor is on the line); the view draws the rule.
    if (HR_RE.test(line)) {
      nodes.push({
        start: lineStart,
        end: lineEnd,
        conceals: lineEnd > lineStart ? [[lineStart, lineEnd]] : [],
        marks: [],
        line: { kind: "line", at: lineStart, style: "hr" },
      });
      if (lineEnd === doc.length) break;
      lineStart = lineEnd + 1;
      continue;
    }

    // Blockquote: depth = count of stripped "> " marker levels; reveal is
    // Obsidian-style — only when the caret touches the marker run.
    if (BQ_MARKER_RE.test(line)) {
      let depth = 0;
      let rest = line;
      let offset = lineStart;
      for (;;) {
        const m = BQ_MARKER_RE.exec(rest);
        if (!m) break;
        depth++;
        rest = rest.slice(m[0].length);
        offset += m[0].length;
      }
      const glyphRunEnd = lineStart + line.slice(0, offset - lineStart).trimEnd().length;
      nodes.push({
        start: lineStart,
        end: glyphRunEnd, // marker glyphs only: "next to the space" != reveal
        conceals: [[lineStart, offset]],
        marks: [],
        line: { kind: "line", at: lineStart, style: "blockquote", depth },
      });
      parseLineContent(rest, offset, nodes);
      if (lineEnd === doc.length) break;
      lineStart = lineEnd + 1;
      continue;
    }

    parseLineContent(line, lineStart, nodes);
    if (lineEnd === doc.length) break;
    lineStart = lineEnd + 1;
  }
  return nodes;
}

// ---------------------------------------------------------------------------
// MockCore
// ---------------------------------------------------------------------------

interface AnchorState {
  pos: number;
  bias: "before" | "after";
}

interface StreamSession {
  anchorId: number;
}

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
  /** Stream id of the last streamAppend, so a later append can tell whether it continues THIS session's open unit. */
  private lastStreamId: number | null = null;

  private composing = false;
  private compFrom = 0;
  private compTo = 0;

  private anchors = new Map<number, AnchorState>();
  private nextAnchorId = 1;
  private streams = new Map<number, StreamSession>();
  private nextStreamId = 1;

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
    this.lastStreamId = null;
    this.composing = false;
    this.anchors.clear();
    this.streams.clear();
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
      // "ai"/"command" reaching applyEdit directly (not the primary path —
      // command()/streamAppend() manage their own undo units below), and
      // "undo"/"redo" origins are not expected through applyEdit when the
      // view uses core-driven history. Defensively record them as isolated,
      // non-coalescing units — new edit origins never coalesce (v0.2).
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
    this.lastStreamId = null;

    this.mutateDoc(splices);

    if (this.composing && origin === "ime" && splices.length > 0) {
      // Grow the session range to cover IME edits beyond a plain positional
      // shift (keeps the session range valid across edits).
      const first = splices[0];
      const last = splices[splices.length - 1];
      let shift = 0;
      for (let k = 0; k < splices.length - 1; k++) {
        shift += splices[k].insert.length - splices[k].delete;
      }
      this.compFrom = Math.min(this.compFrom, first.at);
      this.compTo = Math.max(this.compTo, last.at + shift + last.insert.length);
    }

    return this.rev;
  }

  undo(): CoreChange | null {
    const unit = this.undoStack.pop();
    if (!unit) return null;
    this.redoStack.push({ after: this.doc });
    const splices = diffSplices(this.doc, unit.before);
    this.mutateDoc(splices);
    this.hasOpenUnit = false;
    this.lastOrigin = null;
    this.lastEditEnd = -1;
    this.lastStreamId = null;
    const cursor = endOfLastSplice(splices);
    return {
      revision: this.rev,
      splices,
      selection: cursor === null ? null : { anchor: cursor, head: cursor },
    };
  }

  redo(): CoreChange | null {
    const unit = this.redoStack.pop();
    if (!unit) return null;
    this.undoStack.push({ before: this.doc });
    const splices = diffSplices(this.doc, unit.after);
    this.mutateDoc(splices);
    this.hasOpenUnit = false;
    this.lastOrigin = null;
    this.lastEditEnd = -1;
    this.lastStreamId = null;
    const cursor = endOfLastSplice(splices);
    return {
      revision: this.rev,
      splices,
      selection: cursor === null ? null : { anchor: cursor, head: cursor },
    };
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

      if (node.line) {
        const flaggable = node.line.style === "blockquote" || node.line.style === "list-item";
        out.push(flaggable && revealed ? { ...node.line, revealed: true } : node.line);
      }
      for (const m of node.marks) {
        out.push({ kind: "mark", from: m.from, to: m.to, style: m.style });
      }

      if (node.linkUrl) {
        // conceals[0] = "[" span; conceals[1] = "](url)" span.
        const [d1, d2] = node.conceals;
        const [urlFrom, urlTo] = node.linkUrl;
        if (revealed) {
          out.push({ kind: "mark", from: d1[0], to: d1[1], style: "delim" });
          out.push({ kind: "mark", from: d2[0], to: urlFrom, style: "delim" });
          out.push({ kind: "mark", from: urlFrom, to: urlTo, style: "url" });
          out.push({ kind: "mark", from: urlTo, to: d2[1], style: "delim" });
        } else {
          out.push({ kind: "conceal", from: d1[0], to: d1[1] });
          out.push({ kind: "conceal", from: d2[0], to: d2[1] });
        }
        continue;
      }

      if (node.bullet) {
        // LINE-level reveal: the node spans the item's line, so `revealed`
        // (closed-touch) makes the raw marker editable whenever the cursor
        // is anywhere on the line.
        if (revealed) {
          out.push({ kind: "mark", from: node.bullet.from, to: node.bullet.to, style: "list-marker" });
        } else {
          out.push({ kind: "widget", from: node.bullet.from, to: node.bullet.to, widget: "bullet" });
        }
        continue;
      }

      if (node.widget) {
        // Task-item marker (`- `): reveals IN LOCKSTEP with the checkbox —
        // both key off the node-level `revealed` (extent = marker..checkbox),
        // so the dash and the brackets always show together (core parity).
        for (const [cf, ct] of node.conceals) {
          if (revealed) {
            out.push({ kind: "mark", from: cf, to: ct, style: "delim" });
          } else {
            out.push({ kind: "conceal", from: cf, to: ct });
          }
        }
        if (revealed) {
          out.push({ kind: "mark", from: node.widget.from, to: node.widget.to, style: "delim" });
        } else {
          out.push({
            kind: "widget",
            from: node.widget.from,
            to: node.widget.to,
            widget: "task",
            checked: node.widget.checked,
          });
        }
        continue;
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

  // ---------------------------------------------------------------------------
  // v0.2 additions
  // ---------------------------------------------------------------------------

  createAnchor(pos: number, bias: "before" | "after"): number {
    const id = this.nextAnchorId++;
    this.anchors.set(id, { pos: Math.max(0, Math.min(pos, this.doc.length)), bias });
    return id;
  }

  resolveAnchor(id: number): number | null {
    const a = this.anchors.get(id);
    return a ? a.pos : null;
  }

  dropAnchor(id: number): void {
    this.anchors.delete(id);
  }

  command(name: RangeCommandName, from: number, to: number): CoreChange | null;
  command(name: "setHeading", pos: number, level: 0 | 1 | 2 | 3 | 4 | 5 | 6): CoreChange | null;
  command(name: "toggleTask", pos: number): CoreChange | null;
  command(
    name: RangeCommandName | "setHeading" | "toggleTask",
    a: number,
    b?: number,
  ): CoreChange | null {
    switch (name) {
      case "toggleStrong":
        return this.toggleWrap(a, b as number, "**");
      case "toggleEm":
        return this.toggleWrap(a, b as number, "*");
      case "toggleStrike":
        return this.toggleWrap(a, b as number, "~~");
      case "toggleCode":
        return this.toggleWrap(a, b as number, "`");
      case "setHeading":
        return this.setHeadingCmd(a, b as 0 | 1 | 2 | 3 | 4 | 5 | 6);
      case "toggleTask":
        return this.toggleTaskCmd(a);
      default:
        return null;
    }
  }

  streamOpen(pos: number): number {
    const clamped = Math.max(0, Math.min(pos, this.doc.length));
    const anchorId = this.createAnchor(clamped, "after");
    const id = this.nextStreamId++;
    this.streams.set(id, { anchorId });
    return id;
  }

  streamAppend(id: number, chunk: string): CoreChange {
    const session = this.streams.get(id);
    if (!session) throw new Error(`streamAppend on unknown/closed stream ${id}`);
    const pos = this.resolveAnchor(session.anchorId);
    if (pos === null) throw new Error(`stream anchor unresolved for stream ${id}`);
    const splice: Splice = { at: pos, delete: 0, insert: chunk };

    // The whole session is one undo unit unless something else (a user edit,
    // a command, another stream) interrupted it since the last chunk of THIS
    // stream — mirrors the generic adjacency-coalescing machinery but keyed
    // on "same stream id" rather than position/time (streamed chunks are not
    // adjacent to each other once the tail has grown).
    const coalesce = this.hasOpenUnit && this.lastOrigin === "ai" && this.lastStreamId === id;
    if (!coalesce) {
      this.undoStack.push({ before: this.doc });
      this.redoStack = [];
    }
    this.hasOpenUnit = true;
    this.lastOrigin = "ai";
    this.lastStreamId = id;
    this.lastEditEnd = -1;

    this.mutateDoc([splice]);
    return { revision: this.rev, splices: [splice], selection: null };
  }

  streamClose(id: number): void {
    const session = this.streams.get(id);
    if (!session) return; // no-op on unknown/closed id
    this.dropAnchor(session.anchorId);
    this.streams.delete(id);
    // Close the group so nothing after streamClose accidentally coalesces
    // into the stream's unit (mirrors compositionEnd).
    this.hasOpenUnit = false;
  }

  // ---------------------------------------------------------------------------
  // Command implementations (naive text transforms; correct on simple cases)
  // ---------------------------------------------------------------------------

  private pushCommandUndoUnit(): void {
    this.undoStack.push({ before: this.doc });
    this.redoStack = [];
    this.hasOpenUnit = false;
    this.lastOrigin = "command";
    this.lastEditEnd = -1;
    this.lastStreamId = null;
  }

  /** Toggle a symmetric delimiter pair (`**`, `*`, `~~`, `` ` ``) around [from, to). */
  private toggleWrap(from: number, to: number, delim: string): CoreChange | null {
    if (from < 0 || to > this.doc.length || from > to) return null;
    const len = delim.length;

    // Case A: selection is exactly the inner content; delimiters sit outside it.
    const before = this.doc.slice(Math.max(0, from - len), from);
    const after = this.doc.slice(to, Math.min(this.doc.length, to + len));
    if (before === delim && after === delim) {
      const splices: Splice[] = [
        { at: from - len, delete: len, insert: "" },
        { at: to, delete: len, insert: "" },
      ];
      this.pushCommandUndoUnit();
      this.mutateDoc(splices);
      return { revision: this.rev, splices, selection: { anchor: from - len, head: to - len } };
    }

    // Case B: selection spans the whole node, delimiters included.
    if (
      to - from >= 2 * len &&
      this.doc.slice(from, from + len) === delim &&
      this.doc.slice(to - len, to) === delim
    ) {
      const innerTo = to - len;
      const splices: Splice[] = [
        { at: from, delete: len, insert: "" },
        { at: innerTo, delete: len, insert: "" },
      ];
      this.pushCommandUndoUnit();
      this.mutateDoc(splices);
      return { revision: this.rev, splices, selection: { anchor: from, head: innerTo - len } };
    }

    // Case C: wrap the range with delimiters.
    const splices: Splice[] = [
      { at: from, delete: 0, insert: delim },
      { at: to, delete: 0, insert: delim },
    ];
    this.pushCommandUndoUnit();
    this.mutateDoc(splices);
    return { revision: this.rev, splices, selection: { anchor: from + len, head: to + len } };
  }

  private setHeadingCmd(pos: number, level: 0 | 1 | 2 | 3 | 4 | 5 | 6): CoreChange | null {
    if (pos < 0 || pos > this.doc.length) return null;
    const lineStart = this.doc.lastIndexOf("\n", Math.max(0, pos - 1)) + 1;
    let lineEnd = this.doc.indexOf("\n", pos);
    if (lineEnd === -1) lineEnd = this.doc.length;
    const line = this.doc.slice(lineStart, lineEnd);
    const m = HEADING_RE.exec(line);
    const currentLevel = m ? m[1].length : 0;
    if (currentLevel === level) return null; // already at the requested level: no-op

    const prefixLen = m ? m[1].length + 1 : 0; // "#".repeat(n) + " "
    const newPrefix = level === 0 ? "" : "#".repeat(level) + " ";
    const splice: Splice = { at: lineStart, delete: prefixLen, insert: newPrefix };
    this.pushCommandUndoUnit();
    this.mutateDoc([splice]);
    const shift = newPrefix.length - prefixLen;
    const cursor = Math.max(lineStart, pos + shift);
    return { revision: this.rev, splices: [splice], selection: { anchor: cursor, head: cursor } };
  }

  private toggleTaskCmd(pos: number): CoreChange | null {
    if (pos < 0 || pos > this.doc.length) return null;
    const lineStart = this.doc.lastIndexOf("\n", Math.max(0, pos - 1)) + 1;
    let lineEnd = this.doc.indexOf("\n", pos);
    if (lineEnd === -1) lineEnd = this.doc.length;
    const line = this.doc.slice(lineStart, lineEnd);
    const listM = matchListMarker(line);
    if (!listM) return null;
    const rest = line.slice(listM.contentFrom);
    const taskM = TASK_RE.exec(rest);
    if (!taskM) return null;

    const checkboxInnerAt = lineStart + listM.contentFrom + 1; // the char inside "[ ]"
    const next = taskM[1] === " " ? "x" : " ";
    const splice: Splice = { at: checkboxInnerAt, delete: 1, insert: next };
    this.pushCommandUndoUnit();
    this.mutateDoc([splice]);
    return { revision: this.rev, splices: [splice], selection: null };
  }

  // ---------------------------------------------------------------------------
  // Shared doc mutation: applies splices, bumps revision, and keeps the
  // composition range + all live anchors mapped through the change — used by
  // every doc-mutating path (applyEdit, undo, redo, command, streamAppend).
  // ---------------------------------------------------------------------------

  private mutateDoc(splices: Splice[]): void {
    this.doc = applySplices(this.doc, splices);
    this.rev++;
    if (this.composing) {
      this.compFrom = mapPos(this.compFrom, splices, -1);
      this.compTo = mapPos(this.compTo, splices, 1);
    }
    for (const a of this.anchors.values()) {
      a.pos = mapPos(a.pos, splices, a.bias === "after" ? 1 : -1);
    }
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
