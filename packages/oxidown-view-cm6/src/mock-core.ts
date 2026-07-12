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

// ---------------------------------------------------------------------------
// Surrogate helpers (contract clarification 7 + the lone-surrogate document
// invariant): splice positions may never split a surrogate pair, and no text
// payload may carry an unpaired surrogate into the document.
// ---------------------------------------------------------------------------

function isHighSurrogate(code: number): boolean {
  return code >= 0xd800 && code <= 0xdbff;
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

/** True when `pos` falls between the two code units of a surrogate pair. */
function splitsSurrogatePair(doc: string, pos: number): boolean {
  return (
    pos > 0 &&
    pos < doc.length &&
    isHighSurrogate(doc.charCodeAt(pos - 1)) &&
    isLowSurrogate(doc.charCodeAt(pos))
  );
}

/** Index of the first unpaired surrogate code unit in `text`, or -1. */
function findLoneSurrogate(text: string): number {
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (isHighSurrogate(code)) {
      if (i + 1 < text.length && isLowSurrogate(text.charCodeAt(i + 1))) {
        i++; // well-formed pair
        continue;
      }
      return i;
    }
    if (isLowSurrogate(code)) return i;
  }
  return -1;
}

/** Throw the wasm boundary's InvalidArgs for a bad numeric argument. */
function checkNonNegInt(what: string, v: number): void {
  if (!Number.isInteger(v) || v < 0) {
    throw new Error(`InvalidArgs: ${what} must be a non-negative integer, got ${v}`);
  }
}

function surrogateSplitError(pos: number): Error {
  return new Error(`SurrogateSplit: position ${pos} falls inside a surrogate pair`);
}

/**
 * Minimal single-splice diff between two documents (common prefix/suffix trim).
 * Returns [] when the texts are identical. Trimming never leaves a splice
 * boundary inside a surrogate pair (contract clarification 7 — a pair-splitting
 * splice would be invalid for the view to apply): when a trim boundary would
 * split a pair, it backs off one code unit so the whole pair is replaced.
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
  // Never split a surrogate pair: the prefix boundary splits a pair in `from`
  // iff it splits one in `to` too (the prefixes are identical), so one
  // back-off fixes both sides; same for the suffix boundary.
  if (splitsSurrogatePair(from, start)) start--;
  if (splitsSurrogatePair(from, endFrom)) {
    endFrom++;
    endTo++;
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
  /**
   * Present for ordered-list marker nodes: the whole "1. "/"2) " span,
   * replaced by a widget:ordered (carrying the VIEW-COMPUTED `number` and
   * `delim`) when concealed, or a mark:list-marker (raw source digits) when
   * this LINE-level node's extent is revealed — contract v0.3 amendment,
   * research/07 §0/§1.2. `number` is never the item's raw source digits;
   * see `nextOrderedNumber` below.
   */
  ordered?: { from: number; to: number; number: number; delim: string };
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

// ---------------------------------------------------------------------------
// View-computed ordered-list numbering (contract v0.3 amendment, research/07
// §0/§1.2): a direct-parity transcription of the Rust core's per-open-list
// `list_seq` counter stack (crates/oxidown-core/src/parser.rs), adapted to
// this mock's much simpler LINE-based (not block-tree-based) model. The core
// tracks one counter per currently-OPEN `Tag::List`, pushed/popped at
// Start/End; this mock has no such tree, so it approximates "currently open
// list" with a slot keyed by (quote depth, marker column) — the same
// start+increment semantics, just addressed differently. Persisted across
// `parseLineContent` calls for one `parseDoc` pass (document order).
// ---------------------------------------------------------------------------

interface OrderedSlot {
  delim: string;
  /** Next number to assign to the following sibling at this slot. */
  next: number;
}

/** Per-(quote depth, marker column) running sequence, for one `parseDoc` pass. */
type OrderedSeq = Map<string, OrderedSlot>;

function slotKey(quoteDepth: number, column: number): string {
  return `${quoteDepth}:${column}`;
}

/**
 * A marker (bullet or ordered) just appeared at `column` (quote depth
 * `quoteDepth`): any DEEPER slot (a nested list under whatever was here
 * before) has closed — mirrors the real parser's `End(List)` popping a
 * nested frame once its container line is behind us.
 */
function closeDeeperOrderedSlots(seq: OrderedSeq, quoteDepth: number, column: number): void {
  for (const key of seq.keys()) {
    const sep = key.indexOf(":");
    const d = Number(key.slice(0, sep));
    const c = Number(key.slice(sep + 1));
    if (d === quoteDepth && c > column) seq.delete(key);
  }
}

/**
 * View-computed display number for an ordered marker at (quoteDepth, column)
 * whose raw source digits are `rawDigits` and whose delimiter is `delim`.
 * CONTINUES the slot's running sequence when one is already open there with
 * the SAME delimiter flavor (a `.`/`)` change — or no open sequence at all —
 * STARTS a fresh one at the marker's OWN literal digits, matching
 * CommonMark's "a list's start is its first item's own number", verified
 * directly against pulldown-cmark's behavior for a delimiter/kind change).
 */
function nextOrderedNumber(
  seq: OrderedSeq,
  quoteDepth: number,
  column: number,
  delim: string,
  rawDigits: number,
): number {
  const key = slotKey(quoteDepth, column);
  const existing = seq.get(key);
  if (existing && existing.delim === delim) {
    seq.set(key, { delim, next: existing.next + 1 });
    return existing.next;
  }
  seq.set(key, { delim, next: rawDigits + 1 });
  return rawDigits;
}

/** A bullet marker occupies (quoteDepth, column): any ordered sequence that
 * was open there has ended (marker-kind change starts a new list). */
function closeOrderedSlotForBullet(seq: OrderedSeq, quoteDepth: number, column: number): void {
  seq.delete(slotKey(quoteDepth, column));
}

/** The ordered-marker parts of a matched marker glyph run, read directly
 * from source: raw literal digits (as a number) and the delimiter char. */
function rawOrderedParts(content: string, markerFrom: number): { rawDigits: number; delim: string } {
  const m = /^(\d+)([.)])/.exec(content.slice(markerFrom));
  return m ? { rawDigits: parseInt(m[1], 10), delim: m[2] } : { rawDigits: 1, delim: "." };
}

// ---------------------------------------------------------------------------
// indentList / outdentList (boundary v0.2: marker-width-aware Tab nesting).
// A direct transcription of the Rust core's algorithm (crates/oxidown-core/
// src/commands.rs `plan_list_nesting`) over plain-string line scanning
// instead of the parser overlay — see that module's doc comment for the
// full spec.
// ---------------------------------------------------------------------------

/**
 * Unbounded-leading-space marker match, for indentList/outdentList only —
 * deliberately NOT capped at 3 leading spaces like `LIST_MARKER_RE` (that
 * cap models "does this line still belong to the enclosing container" for
 * the DECORATION parser; the nesting command must recognize markers at any
 * depth).
 */
const LIST_MARKER_ANY_INDENT_RE = /^( *)([-*+]|\d{1,9}[.)]) /;

interface ListLineInfo {
  start: number;
  end: number;
  quoteEnd: number;
  quoteDepth: number;
  /**
   * This line's OWN list marker, when its item begins here: column (after
   * the quote prefix), the spec's fixed token width, and the raw marker
   * glyphs (`-`, `2.`, `10)`, …) for the paragraph-interruption guard's
   * family/number checks.
   */
  marker: { col: number; width: number; glyphs: string } | null;
}

// ---------------------------------------------------------------------------
// Physical-line splitting. Contract v0.2 clarification 5: a line is terminated
// by "\n", "\r\n", or a lone "\r" — every command/reveal line computation must
// treat all three uniformly (matching the Rust core's line splitting).
// ---------------------------------------------------------------------------

/** Length of the line terminator at `pos`: 2 for "\r\n", 1 for "\r"/"\n", 0 otherwise. */
function terminatorLength(doc: string, pos: number): number {
  const ch = doc[pos];
  if (ch === "\r") return doc[pos + 1] === "\n" ? 2 : 1;
  return ch === "\n" ? 1 : 0;
}

function isTerminatorChar(ch: string | undefined): boolean {
  return ch === "\n" || ch === "\r";
}

/** End (exclusive, terminator excluded) of the line beginning at/continuing through `pos`. */
function lineEndFrom(doc: string, pos: number): number {
  let end = pos;
  while (end < doc.length && !isTerminatorChar(doc[end])) end++;
  return end;
}

/** Code-unit range of the physical line containing `pos` (terminator excluded). */
function lineRangeContaining(doc: string, pos: number): { start: number; end: number } {
  let p = Math.max(0, Math.min(pos, doc.length));
  // A position between the "\r" and "\n" of one "\r\n" terminator belongs to
  // the line that terminator ends.
  if (p > 0 && doc[p] === "\n" && doc[p - 1] === "\r") p--;
  let start = p;
  while (start > 0 && !isTerminatorChar(doc[start - 1])) start--;
  return { start, end: lineEndFrom(doc, p) };
}

/** The physical line immediately before `lineStart`, or null at the document start. */
function prevLineRange(doc: string, lineStart: number): { start: number; end: number } | null {
  if (lineStart === 0) return null;
  return lineRangeContaining(doc, lineStart - 1); // lineStart-1 is inside the preceding terminator
}

/**
 * The physical line immediately following the line ending at `lineEnd` (that
 * line's own extent, terminator excluded), or null when `lineEnd` has no
 * terminator (the document's last line).
 */
function nextLineRange(doc: string, lineEnd: number): { start: number; end: number } | null {
  const tl = terminatorLength(doc, lineEnd);
  if (tl === 0) return null; // no terminator = doc end
  return lineRangeContaining(doc, lineEnd + tl);
}

/**
 * Physical lines intersecting `[from, to]`, mirroring CodeMirror's own
 * multi-line command iteration: an empty range (cursor) always yields its
 * containing line; a non-empty range excludes a trailing line touched only
 * at its very start (`to` landing exactly on a line boundary selects none
 * of that line).
 */
function intersectingLines(
  doc: string,
  from: number,
  to: number,
): Array<{ start: number; end: number }> {
  const lines: Array<{ start: number; end: number }> = [];
  const empty = from === to;
  let pos = from;
  for (;;) {
    const line = lineRangeContaining(doc, pos);
    if (empty || to > line.start) lines.push(line);
    if (pos >= to) break;
    const tl = terminatorLength(doc, line.end); // skip "\n" / "\r" / "\r\n"
    if (tl === 0) break; // unterminated last line
    const next = line.end + tl;
    if (next <= pos || next > doc.length) break;
    pos = next;
  }
  return lines;
}

/** This line's blockquote depth (0 outside any) and length of its `> `/`> > `/… run. */
function quotePrefixInfo(line: string): { depth: number; length: number } {
  let depth = 0;
  let rest = line;
  let consumed = 0;
  for (;;) {
    const m = BQ_MARKER_RE.exec(rest);
    if (!m) break;
    depth++;
    consumed += m[0].length;
    rest = rest.slice(m[0].length);
  }
  return { depth, length: consumed };
}

/**
 * This line's list marker relative to `afterQuote` (the text past the quote
 * prefix). `width` is the spec's FIXED-width definition — marker glyphs plus
 * exactly one following space (`- ` = 2, `1. ` = 3, `10. ` = 4; a task
 * item's `- ` is the same 2) — not however much whitespace actually follows
 * the marker in the source.
 */
function listMarkerInfo(afterQuote: string): ListLineInfo["marker"] {
  const m = LIST_MARKER_ANY_INDENT_RE.exec(afterQuote);
  if (!m) return null;
  return { col: m[1].length, width: m[2].length + 1, glyphs: m[2] };
}

function listLineInfo(doc: string, range: { start: number; end: number }): ListLineInfo {
  const lineText = doc.slice(range.start, range.end);
  const q = quotePrefixInfo(lineText);
  const quoteEnd = range.start + q.length;
  const marker = listMarkerInfo(lineText.slice(q.length)); // columns relative to quoteEnd
  return { start: range.start, end: range.end, quoteEnd, quoteDepth: q.depth, marker };
}

/**
 * The ordered-marker parts of a marker glyph run (`2.` → digits "2", delim
 * "."), or null for bullets. `isOne` is numeric ("01" counts as 1, matching
 * CommonMark's start-number semantics).
 */
function orderedMarkerParts(glyphs: string): { digits: string; delim: string; isOne: boolean } | null {
  const m = /^(\d+)([.)])$/.exec(glyphs);
  if (!m) return null;
  return { digits: m[1], delim: m[2], isOne: parseInt(m[1], 10) === 1 };
}

/**
 * Parse one "logical line" of content (heading / list item / plain paragraph
 * text) at document offset `base`, pushing nodes into `nodes`. Shared by the
 * top-level line loop and by blockquote line remainders. `quoteDepth` (0
 * outside any blockquote) and `seq` (the running ordered-sequence state for
 * this `parseDoc` pass) address the view-computed ordered numbering slot —
 * see `nextOrderedNumber`.
 */
function parseLineContent(
  content: string,
  base: number,
  nodes: ParsedNode[],
  quoteDepth: number,
  seq: OrderedSeq,
): void {
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
    // A heading interrupts any open list run at this quote depth.
    seq.clear();
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
    // LINE-level reveal (contract v0.3, matching headings): every marker
    // node spans the WHOLE line, so a caret anywhere on it flags the line
    // revealed and shows all marker constructs (indent, dash, brackets) as
    // raw source together.
    const lineTo = base + content.length;
    nodes.push({
      start: base,
      end: lineTo,
      conceals: depth >= 2 && markerFrom > 0 ? [[base, base + markerFrom]] : [],
      marks: [],
      line: { kind: "line", at: base, style: "list-item", depth },
    });
    // Any list nested DEEPER than this marker's own column (at this quote
    // depth) has closed, regardless of this marker's own kind.
    closeDeeperOrderedSlots(seq, quoteDepth, markerFrom);
    const afterMarker = content.slice(contentFrom);
    const taskM = TASK_RE.exec(afterMarker);
    if (taskM) {
      const checkboxFrom = base + contentFrom;
      const checkboxTo = checkboxFrom + 3; // "[ ]" / "[x]"
      const itemContentFrom = contentFrom + taskM[0].length;
      if (isBullet) {
        // Bullet task items: the `- ` marker conceals (no bullet widget) and
        // reveals in lockstep with the checkbox — both on the same
        // line-spanning node (matches the Rust core: task marker conceals
        // only when is_bullet && task).
        closeOrderedSlotForBullet(seq, quoteDepth, markerFrom);
        nodes.push({
          start: base,
          end: lineTo,
          conceals: [[base + markerFrom, base + contentFrom]],
          marks: [],
          widget: { from: checkboxFrom, to: checkboxTo, checked: taskM[1] !== " " },
        });
      } else {
        // Ordered task items: the marker is independent of the checkbox —
        // it still takes the computed-number ordered path (Rust core:
        // is_bullet == false always does, task or not), while the checkbox
        // is its own separate widget.
        const { rawDigits, delim } = rawOrderedParts(content, markerFrom);
        const number = nextOrderedNumber(seq, quoteDepth, markerFrom, delim, rawDigits);
        nodes.push({
          start: base,
          end: lineTo,
          conceals: [],
          marks: [],
          ordered: { from: base + markerFrom, to: base + contentFrom, number, delim },
        });
        nodes.push({
          start: base,
          end: lineTo,
          conceals: [],
          marks: [],
          widget: { from: checkboxFrom, to: checkboxTo, checked: taskM[1] !== " " },
        });
      }
      parseInline(content.slice(itemContentFrom), base + itemContentFrom, nodes);
      return;
    }
    if (isBullet) {
      closeOrderedSlotForBullet(seq, quoteDepth, markerFrom);
      nodes.push({
        start: base,
        end: lineTo,
        conceals: [],
        marks: [],
        bullet: { from: base + markerFrom, to: base + contentFrom },
      });
    } else {
      const { rawDigits, delim } = rawOrderedParts(content, markerFrom);
      const number = nextOrderedNumber(seq, quoteDepth, markerFrom, delim, rawDigits);
      nodes.push({
        start: base,
        end: lineTo,
        conceals: [],
        marks: [],
        ordered: { from: base + markerFrom, to: base + contentFrom, number, delim },
      });
    }
    parseInline(content.slice(contentFrom), base + contentFrom, nodes);
    return;
  }

  parseInline(content, base, nodes);
  // A genuine paragraph line interrupts any open list run (the mock does not
  // model loose-list/lazy-continuation paragraph absorption) — but a BLANK
  // line must NOT: per CommonMark, blank lines don't close a list (that's
  // exactly what makes a list "loose"), so "1. a\n1. b\n\n1. c" is one list,
  // displaying 1,2,3 — matching the Rust core, which counts straight through
  // blank lines within a list. Only non-blank, non-item content resets the
  // running sequence.
  if (content.trim() !== "") seq.clear();
}

/** Parse the full document into formatted/line/widget nodes. */
export function parseDoc(doc: string): ParsedNode[] {
  const nodes: ParsedNode[] = [];
  // View-computed ordered-list sequence state for this pass (contract v0.3
  // amendment, research/07 §0/§1.2) — see `nextOrderedNumber`.
  const seq: OrderedSeq = new Map();
  let lineStart = 0;
  while (lineStart <= doc.length) {
    const lineEnd = lineEndFrom(doc, lineStart); // "\n" / "\r" / "\r\n" all terminate
    const line = doc.slice(lineStart, lineEnd);

    // Fenced code blocks: consume lines until the matching close fence (or EOF).
    // Never concealed; body content is marked `code` but not inline-parsed.
    const fenceM = FENCE_RE.exec(line);
    if (fenceM) {
      seq.clear(); // a fenced block interrupts any open list run
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
      for (;;) {
        const tl = terminatorLength(doc, cursor);
        if (tl === 0) break; // unterminated: the fence runs to EOF
        const bLineStart = cursor + tl;
        const bLineEnd = lineEndFrom(doc, bLineStart);
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
      const tl = terminatorLength(doc, cursor);
      if (tl === 0) break;
      lineStart = cursor + tl;
      continue;
    }

    // Thematic break: hr line style + the raw dashes concealed (revealed as
    // delim when the cursor is on the line); the view draws the rule.
    if (HR_RE.test(line)) {
      seq.clear(); // a thematic break interrupts any open list run
      nodes.push({
        start: lineStart,
        end: lineEnd,
        conceals: lineEnd > lineStart ? [[lineStart, lineEnd]] : [],
        marks: [],
        line: { kind: "line", at: lineStart, style: "hr" },
      });
      const tl = terminatorLength(doc, lineEnd);
      if (tl === 0) break;
      lineStart = lineEnd + tl;
      continue;
    }

    // Blockquote: depth = count of stripped "> " marker levels; reveal is
    // LINE-level (contract v0.3) — a caret anywhere on the line reveals.
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
      nodes.push({
        start: lineStart,
        end: lineEnd, // whole line: LINE-level reveal (contract v0.3)
        conceals: [[lineStart, offset]],
        marks: [],
        line: { kind: "line", at: lineStart, style: "blockquote", depth },
      });
      parseLineContent(rest, offset, nodes, depth, seq);
      const tl = terminatorLength(doc, lineEnd);
      if (tl === 0) break;
      lineStart = lineEnd + tl;
      continue;
    }

    parseLineContent(line, lineStart, nodes, 0, seq);
    const tl = terminatorLength(doc, lineEnd);
    if (tl === 0) break;
    lineStart = lineEnd + tl;
  }
  return nodes;
}

// ---------------------------------------------------------------------------
// MockCore
// ---------------------------------------------------------------------------

interface AnchorState {
  pos: number;
  bias: "before" | "after";
  /**
   * Stream insertion points are core-INTERNAL anchors: invisible to the
   * public anchor API (resolveAnchor reads them as unknown, dropAnchor
   * no-ops on them), so no boundary caller can disturb an open stream —
   * mirrors the Rust core's AnchorSet public/internal split (anchor.rs).
   */
  internal: boolean;
}

interface StreamSession {
  anchorId: number;
  /**
   * A trailing high surrogate withheld from the last chunk (a chunk boundary
   * may split an astral code point): prepended to the next chunk, or flushed
   * as one U+FFFD by streamClose. The document itself never holds a lone
   * surrogate.
   */
  pending: string;
}

/**
 * Undo units store the full text before the unit (mock simplicity). A stream
 * session's single unit carries its stream id, so later appends of the same
 * stream merge into that unit wherever it sits in the stack (exactly like
 * history.rs `record_stream_append`), and the tag survives the undo↔redo
 * round trip.
 */
interface UndoUnit {
  before: string;
  streamId?: number;
}

interface RedoUnit {
  after: string;
  streamId?: number;
}

export class MockCore implements OxidownCore {
  private doc = "";
  private rev = 0;
  private readonly now: () => number;

  private undoStack: UndoUnit[] = [];
  private redoStack: RedoUnit[] = [];
  /** True while the top of undoStack is an open (coalescable) unit. */
  private hasOpenUnit = false;
  private lastEditTime = -Infinity;
  /** End of the last single-splice edit, in current-doc coordinates; -1 if unusable. */
  private lastEditEnd = -1;
  private lastOrigin: EditOrigin | null = null;

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
    // Document invariant: the mirror never holds an unpaired surrogate (the
    // wasm boundary rejects such payloads before the core sees them).
    if (findLoneSurrogate(text) !== -1) {
      throw new Error("InvalidPayload: text contains an unpaired surrogate");
    }
    this.doc = text;
    this.undoStack = [];
    this.redoStack = [];
    this.hasOpenUnit = false;
    this.lastEditTime = -Infinity;
    this.lastEditEnd = -1;
    this.lastOrigin = null;
    this.composing = false;
    this.anchors.clear();
    this.streams.clear();
    // "Returns revision 0's successor" — revisions stay monotonic across
    // repeated load() calls so stale revision numbers can never be re-issued.
    this.rev++;
    return this.rev;
  }

  applyEdit(baseRevision: number, splices: Splice[], origin: EditOrigin): number {
    checkNonNegInt("baseRevision", baseRevision);
    if (baseRevision !== this.rev) {
      throw new Error(
        `StaleRevision: core is at revision ${this.rev}, caller passed ${baseRevision}`,
      );
    }
    this.validateSplices(splices);
    // Drop no-op splices; an entirely empty/no-op batch changes nothing —
    // revision unchanged, no undo unit (editor.rs apply_edit's early return).
    const batch = splices.filter((s) => s.delete > 0 || s.insert.length > 0);
    if (batch.length === 0) return this.rev;
    const t = this.now();

    if (origin === "user" || origin === "ime" || origin === "paste") {
      const coalesce =
        origin !== "paste" &&
        this.hasOpenUnit &&
        (this.lastOrigin === "user" || this.lastOrigin === "ime") &&
        batch.length === 1 &&
        this.isAdjacent(batch[0]) &&
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

    if (batch.length === 1) {
      const s = batch[0];
      this.lastEditEnd = s.at + s.insert.length;
    } else {
      this.lastEditEnd = -1;
    }
    this.lastEditTime = t;
    this.lastOrigin = origin;

    this.mutateDoc(batch);

    if (this.composing && origin === "ime") {
      // Grow the session range to union in IME-inserted regions that touch
      // it (composition.rs `map_through` with `grow`): compare each splice's
      // POST-edit insert range — mutateDoc already mapped compFrom/compTo
      // into post-edit coordinates, so pre-edit `at` positions must be
      // shifted by the preceding splices' deltas before the union.
      let delta = 0;
      let lo = this.compFrom;
      let hi = this.compTo;
      for (const s of batch) {
        const insStart = s.at + delta;
        const insEnd = insStart + s.insert.length;
        if (insStart <= hi && insEnd >= lo) {
          lo = Math.min(lo, insStart);
          hi = Math.max(hi, insEnd);
        }
        delta += s.insert.length - s.delete;
      }
      this.compFrom = lo;
      this.compTo = Math.max(hi, lo);
    }

    return this.rev;
  }

  undo(): CoreChange | null {
    const unit = this.undoStack.pop();
    if (!unit) return null;
    // Preserve the stream tag across the round trip: redo of a stream unit
    // re-establishes it as the stream's single merge target (history.rs
    // `push_redo`/`push_undo_unit`).
    this.redoStack.push({ after: this.doc, streamId: unit.streamId });
    const splices = diffSplices(this.doc, unit.before);
    this.mutateDoc(splices);
    this.hasOpenUnit = false;
    this.lastOrigin = null;
    this.lastEditEnd = -1;
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
    this.undoStack.push({ before: this.doc, streamId: unit.streamId });
    const splices = diffSplices(this.doc, unit.after);
    this.mutateDoc(splices);
    this.hasOpenUnit = false;
    this.lastOrigin = null;
    this.lastEditEnd = -1;
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
    checkNonNegInt("revision", revision);
    if (revision !== this.rev) {
      throw new Error(
        `StaleRevision: core is at revision ${this.rev}, caller passed ${revision}`,
      );
    }
    checkNonNegInt("from", from);
    checkNonNegInt("to", to);
    if (from > to) {
      throw new Error(`InvalidRange: from ${from} > to ${to}`);
    }
    if (to > this.doc.length) throw this.outOfBounds(to);
    for (const sel of selections) {
      if (
        !Number.isInteger(sel.anchor) ||
        sel.anchor < 0 ||
        !Number.isInteger(sel.head) ||
        sel.head < 0
      ) {
        throw new Error(
          `InvalidPayload: malformed selections: anchor=${sel.anchor} head=${sel.head}`,
        );
      }
      const hi = Math.max(sel.anchor, sel.head);
      if (hi > this.doc.length) throw this.outOfBounds(hi);
    }
    // Query positions snap outward to the nearest code-point boundary rather
    // than erroring (contract clarification 7 — range filters, not mutations).
    const vFrom = this.snapFloor(from);
    const vTo = this.snapCeil(to);
    const sels = selections.map(
      (sel) =>
        [
          this.snapFloor(Math.min(sel.anchor, sel.head)),
          this.snapCeil(Math.max(sel.anchor, sel.head)),
        ] as const,
    );
    // Composition stability rule, PER CONCEAL SPAN (decorations.rs): only
    // conceal spans the composition range touches divert to `mark:delim` —
    // sibling delimiters of the same node stay concealed.
    const compTouch = (f: number, t: number): boolean =>
      this.composing && this.compFrom <= t && this.compTo >= f;

    const nodes = parseDoc(this.doc);
    const out: Decoration[] = [];
    for (const node of nodes) {
      // Strict half-open overlap with the viewport (core parity): nodes that
      // only touch the boundary are excluded.
      if (node.start >= vTo || node.end <= vFrom) continue;

      // Reveal predicate: any selection range (cursor = empty range) that
      // intersects the node's full extent INCLUDING delimiters — touching a
      // boundary counts.
      const revealed = sels.some(([lo, hi]) => lo <= node.end && hi >= node.start);

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
        if (revealed || compTouch(d1[0], d1[1])) {
          out.push({ kind: "mark", from: d1[0], to: d1[1], style: "delim" });
        } else {
          out.push({ kind: "conceal", from: d1[0], to: d1[1] });
        }
        if (revealed || compTouch(d2[0], d2[1])) {
          // v0.2 clarification 4: the "](url)" span reveals as delim/url/delim.
          out.push({ kind: "mark", from: d2[0], to: urlFrom, style: "delim" });
          out.push({ kind: "mark", from: urlFrom, to: urlTo, style: "url" });
          out.push({ kind: "mark", from: urlTo, to: d2[1], style: "delim" });
        } else {
          out.push({ kind: "conceal", from: d2[0], to: d2[1] });
        }
        continue;
      }

      if (node.bullet) {
        // LINE-level reveal: the node spans the item's line, so `revealed`
        // (closed-touch) makes the raw marker editable whenever the cursor
        // is anywhere on the line. A composition touching the marker span
        // itself reveals it too (core parity: decorations.rs ListMarker).
        if (revealed || compTouch(node.bullet.from, node.bullet.to)) {
          out.push({ kind: "mark", from: node.bullet.from, to: node.bullet.to, style: "list-marker" });
        } else {
          out.push({ kind: "widget", from: node.bullet.from, to: node.bullet.to, widget: "bullet" });
        }
        continue;
      }

      if (node.ordered) {
        // Contract v0.3 amendment (research/07 §0/§1.2): concealed ordered
        // markers are a computed-number WIDGET, never the raw source
        // digits; LINE-level reveal (same node-extent discipline as bullet)
        // shows the raw digits as mark:list-marker.
        if (revealed || compTouch(node.ordered.from, node.ordered.to)) {
          out.push({ kind: "mark", from: node.ordered.from, to: node.ordered.to, style: "list-marker" });
        } else {
          out.push({
            kind: "widget",
            from: node.ordered.from,
            to: node.ordered.to,
            widget: "ordered",
            number: node.ordered.number,
            delim: node.ordered.delim,
          });
        }
        continue;
      }

      if (node.widget) {
        // Task-item marker (`- `): reveals IN LOCKSTEP with the checkbox —
        // both key off the node-level `revealed` (extent = the whole line),
        // so the dash and the brackets always show together (core parity).
        for (const [cf, ct] of node.conceals) {
          if (revealed || compTouch(cf, ct)) {
            out.push({ kind: "mark", from: cf, to: ct, style: "delim" });
          } else {
            out.push({ kind: "conceal", from: cf, to: ct });
          }
        }
        if (revealed || compTouch(node.widget.from, node.widget.to)) {
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
        if (revealed || compTouch(cf, ct)) {
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
    checkNonNegInt("from", from);
    checkNonNegInt("to", to);
    if (from > to) throw new Error(`InvalidRange: from ${from} > to ${to}`);
    if (to > this.doc.length) throw this.outOfBounds(to);
    this.composing = true;
    // Query positions snap outward at surrogate pairs rather than erroring.
    this.compFrom = this.snapFloor(from);
    this.compTo = Math.max(this.compFrom, this.snapCeil(to));
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
    if (bias !== "before" && bias !== "after") {
      throw new Error(`InvalidBias: ${JSON.stringify(bias)}`);
    }
    this.checkDocPos("pos", pos);
    // An anchor is a tracked query position: a position inside a surrogate
    // pair snaps toward the anchor's bias (editor.rs create_anchor).
    return this.insertAnchor(pos, bias, false);
  }

  private insertAnchor(pos: number, bias: "before" | "after", internal: boolean): number {
    const snapped = bias === "before" ? this.snapFloor(pos) : this.snapCeil(pos);
    const id = this.nextAnchorId++;
    this.anchors.set(id, { pos: snapped, bias, internal });
    return id;
  }

  resolveAnchor(id: number): number | null {
    checkNonNegInt("id", id);
    // Public resolution: internal (stream) ids read as unknown.
    const a = this.anchors.get(id);
    return a && !a.internal ? a.pos : null;
  }

  /** Core-internal resolution: any live id, internal or public. */
  private resolveInternal(id: number): number | null {
    const a = this.anchors.get(id);
    return a ? a.pos : null;
  }

  dropAnchor(id: number): void {
    checkNonNegInt("id", id);
    // No-op on unknown ids — and on core-internal anchor ids (stream
    // insertion points), which no boundary caller may disturb: public ids
    // and stream-internal state never collide destructively (anchor.rs).
    const a = this.anchors.get(id);
    if (!a || a.internal) return;
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
    // Argument validation mirrors the wasm entry point (crates/oxidown-wasm/
    // src/lib.rs `command`): the first argument's number is checked, then a
    // missing trailing argument is InvalidArgs — all BEFORE dispatch,
    // without mutating anything.
    const requireB = (what: string): number => {
      if (b === undefined || b === null) {
        throw new Error(`InvalidArgs: ${name} requires ${what}`);
      }
      return b;
    };
    const rangeArgs = (): [number, number] => {
      checkNonNegInt("from", a);
      const to = requireB("a `to` position");
      checkNonNegInt("to", to);
      return [a, to];
    };
    switch (name) {
      case "toggleStrong":
        return this.toggleWrap(name, ...rangeArgs(), "**");
      case "toggleEm":
        return this.toggleWrap(name, ...rangeArgs(), "*");
      case "toggleStrike":
        return this.toggleWrap(name, ...rangeArgs(), "~~");
      case "toggleCode":
        return this.toggleWrap(name, ...rangeArgs(), "`");
      case "setHeading":
        checkNonNegInt("pos", a);
        return this.setHeadingCmd(a, requireB("a heading level"));
      case "toggleTask":
        checkNonNegInt("pos", a);
        return this.toggleTaskCmd(a);
      case "indentList":
        return this.indentOutdentList(...rangeArgs(), true);
      case "outdentList":
        return this.indentOutdentList(...rangeArgs(), false);
      case "enter":
        return this.enterCmd(...rangeArgs());
      default:
        // Parity with the wasm core (crates/oxidown-wasm/src/lib.rs): an
        // unrecognized command name is a caller/protocol bug, not a "this
        // command doesn't apply here" `null` — the wasm entry point
        // validates the name BEFORE dispatch and throws `InvalidCommand`
        // without having mutated anything. Returning `null` here (as this
        // mock used to) would let a typo'd command name pass every mock
        // test yet throw against wasm — the mock must fail the same way.
        throw new Error(`InvalidCommand: ${JSON.stringify(name)}`);
    }
  }

  streamOpen(pos: number): number {
    // Strict position (editor.rs stream_open): an insertion point inside a
    // surrogate pair would corrupt text and errors; no clamping — an
    // out-of-bounds position throws like every other mutation position.
    this.strictDocPos(pos);
    const anchorId = this.insertAnchor(pos, "after", true); // internal: untouchable via dropAnchor
    const id = this.nextStreamId++;
    this.streams.set(id, { anchorId, pending: "" });
    return id;
  }

  streamAppend(id: number, chunk: string): CoreChange {
    checkNonNegInt("id", id);
    const session = this.streams.get(id);
    if (!session) {
      throw new Error(`UnknownStream: stream ${id} is unknown or already closed`);
    }
    // Chunk boundaries may split an astral code point: prepend the pending
    // high surrogate from the previous chunk, withhold a new trailing one
    // (streamClose flushes it as U+FFFD). Anything else unpaired is a bad
    // payload. The pending buffer is cleared even when the chunk is rejected
    // (adapter parity).
    let text = session.pending + chunk;
    session.pending = "";
    let withheld = "";
    if (text.length > 0 && isHighSurrogate(text.charCodeAt(text.length - 1))) {
      withheld = text[text.length - 1];
      text = text.slice(0, -1);
    }
    if (findLoneSurrogate(text) !== -1) {
      throw new Error("InvalidPayload: chunk contains an unpaired surrogate");
    }
    session.pending = withheld;
    if (text === "") {
      // Nothing (yet) to insert: no revision bump, no undo unit.
      return { revision: this.rev, splices: [], selection: null };
    }
    return this.applyStreamText(id, session, text);
  }

  streamClose(id: number): void {
    checkNonNegInt("id", id);
    const session = this.streams.get(id);
    if (!session) return; // no-op on unknown/closed id
    if (session.pending !== "") {
      // A dangling high surrogate can never be completed: flush it as one
      // U+FFFD so the document invariant (no lone surrogates) holds.
      session.pending = "";
      this.applyStreamText(id, session, "�");
    }
    this.anchors.delete(session.anchorId); // internal anchor: bypass dropAnchor's guard
    this.streams.delete(id);
    // Close the group so nothing after streamClose accidentally coalesces
    // into the stream's unit (mirrors compositionEnd).
    this.hasOpenUnit = false;
  }

  /**
   * Insert `text` at the stream's (mapped) anchor, merging the edit into the
   * stream's single undo unit wherever it sits in the stack — the snapshot
   * counterpart of history.rs `record_stream_append`. Units created after
   * the stream's unit (interleaved user edits) sit above it; their snapshots
   * must gain the chunk (their undo must not delete streamed text), while
   * the stream unit's own snapshot stays chunk-free (its undo removes the
   * whole stream). The insertion position is cascaded down frame by frame:
   * each unit's `before` IS the doc of the frame below it, so mapping
   * through `diffSplices(frameDoc, before)` translates the position exactly
   * like the Rust core's map-through-old-inverse step.
   */
  private applyStreamText(id: number, session: StreamSession, text: string): CoreChange {
    const pos = this.resolveInternal(session.anchorId);
    if (pos === null) {
      throw new Error(`UnknownStream: stream ${id} lost its insertion anchor`);
    }
    const splice: Splice = { at: pos, delete: 0, insert: text };
    this.redoStack = []; // any new edit clears redo
    let idx = -1;
    for (let k = this.undoStack.length - 1; k >= 0; k--) {
      if (this.undoStack[k].streamId === id) {
        idx = k;
        break;
      }
    }
    if (idx === -1) {
      // First append, or the stream's unit was undone away: fresh unit,
      // tagged so later appends merge into it. It is never coalescible by
      // user/ime edits.
      this.undoStack.push({ before: this.doc, streamId: id });
      this.hasOpenUnit = false;
      this.lastOrigin = "ai";
      this.lastEditEnd = -1;
    } else {
      let p = splice.at;
      let frameDoc = this.doc;
      for (let k = this.undoStack.length - 1; k > idx; k--) {
        const unit = this.undoStack[k];
        const oldBefore = unit.before;
        const inverse = diffSplices(frameDoc, oldBefore);
        // Bias before: an insertion exactly at restored text stays before it,
        // so no other unit's undo ever deletes streamed text.
        const deeper = mapPos(p, inverse, -1);
        unit.before = oldBefore.slice(0, deeper) + text + oldBefore.slice(deeper);
        frameDoc = oldBefore;
        p = deeper;
      }
      // The user-coalescing state tracks the TOP unit, which this append did
      // not replace — only remap the adjacency position through the insert.
      if (this.lastEditEnd >= 0) {
        this.lastEditEnd = mapPos(this.lastEditEnd, [splice], -1);
      }
    }

    this.mutateDoc([splice]);
    return { revision: this.rev, splices: [splice], selection: null };
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
  }

  // ---- shared argument validation (wasm error-message parity) -------------

  private outOfBounds(pos: number): Error {
    return new Error(
      `OutOfBounds: position ${pos} beyond document length ${this.doc.length} (UTF-16 code units)`,
    );
  }

  /** InvalidArgs for malformed numbers, OutOfBounds past the document end. */
  private checkDocPos(what: string, pos: number): void {
    checkNonNegInt(what, pos);
    if (pos > this.doc.length) throw this.outOfBounds(pos);
  }

  /** A MUTATION position: additionally rejects surrogate-pair interiors. */
  private strictDocPos(pos: number, what = "pos"): number {
    this.checkDocPos(what, pos);
    if (splitsSurrogatePair(this.doc, pos)) throw surrogateSplitError(pos);
    return pos;
  }

  /** QUERY positions snap outward to code-point boundaries instead of erroring. */
  private snapFloor(pos: number): number {
    return splitsSurrogatePair(this.doc, pos) ? pos - 1 : pos;
  }

  private snapCeil(pos: number): number {
    return splitsSurrogatePair(this.doc, pos) ? pos + 1 : pos;
  }

  /**
   * Inline toggles must not span multiple leaf blocks (Rust-core guard,
   * throws InvalidArgs). The mock's line-based approximation of a leaf
   * block: consecutive plain paragraph lines are one leaf; any blank,
   * heading, hr, fence, quote, or list-marker line inside a multi-line
   * range means the range crosses a leaf boundary.
   */
  private assertSingleLeafBlock(name: string, from: number, to: number): void {
    const lines = intersectingLines(this.doc, from, to);
    if (lines.length <= 1) return;
    const oneParagraph = lines.every((r) => {
      const text = this.doc.slice(r.start, r.end);
      return (
        text.trim() !== "" &&
        !HEADING_RE.test(text) &&
        !HR_RE.test(text) &&
        !FENCE_RE.test(text) &&
        !BQ_MARKER_RE.test(text) &&
        !LIST_MARKER_ANY_INDENT_RE.test(text)
      );
    });
    if (!oneParagraph) {
      throw new Error(`InvalidArgs: ${name} range spans multiple leaf blocks`);
    }
  }

  /** Toggle a symmetric delimiter pair (`**`, `*`, `~~`, `` ` ``) around [from, to). */
  private toggleWrap(
    name: string,
    fromArg: number,
    toArg: number,
    delim: string,
  ): CoreChange | null {
    this.checkDocPos("from", fromArg);
    this.checkDocPos("to", toArg);
    // Reversed ranges normalize (editor.rs command dispatch: from.min(to)).
    const from = this.strictDocPos(Math.min(fromArg, toArg), "from");
    const to = this.strictDocPos(Math.max(fromArg, toArg), "to");
    this.assertSingleLeafBlock(name, from, to);
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

  private setHeadingCmd(posArg: number, level: number): CoreChange | null {
    checkNonNegInt("level", level);
    if (level > 6) {
      throw new Error(`InvalidArgs: setHeading level must be an integer 0..=6, got ${level}`);
    }
    this.checkDocPos("pos", posArg);
    const pos = this.snapFloor(posArg); // query position: snaps, never errors
    const { start: lineStart, end: lineEnd } = lineRangeContaining(this.doc, pos);
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

  private toggleTaskCmd(posArg: number): CoreChange | null {
    this.checkDocPos("pos", posArg);
    const pos = this.snapFloor(posArg); // query position: snaps, never errors
    const { start: lineStart, end: lineEnd } = lineRangeContaining(this.doc, pos);
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

  /**
   * indentList/outdentList (boundary v0.2: marker-width-aware Tab nesting).
   * A direct transcription of the Rust core's algorithm
   * (crates/oxidown-core/src/commands.rs `plan_list_nesting`) over
   * plain-string line scanning — see that module's doc comment for the full
   * spec, including the subtree-aware affected-line set.
   */
  private indentOutdentList(fromArg: number, toArg: number, indent: boolean): CoreChange | null {
    this.checkDocPos("from", fromArg);
    this.checkDocPos("to", toArg);
    // Reversed ranges normalize; positions are strict (mutation positions).
    const from = this.strictDocPos(Math.min(fromArg, toArg), "from");
    const to = this.strictDocPos(Math.max(fromArg, toArg), "to");
    const doc = this.doc;
    const lines = intersectingLines(doc, from, to).map((r) => listLineInfo(doc, r));

    // Applies iff at least one intersecting line carries a marker.
    const firstIdx = lines.findIndex((l) => l.marker !== null);
    if (firstIdx === -1) return null;
    const first = lines[firstIdx];
    const firstCol = first.marker!.col;
    const firstDepth = first.quoteDepth;

    const noOp = (): CoreChange => ({ revision: this.rev, splices: [], selection: null });

    // Scan upward over consecutive same-quote-depth list-item lines for the
    // nearest qualifying candidate: indent's target allows `<=`, outdent's
    // parent requires strictly `<`. Stops (no candidate) at the first line
    // that isn't itself a list-item line, or whose quote depth differs.
    let target: { col: number; width: number } | null = null;
    let cursor = first.start;
    for (;;) {
      const range = prevLineRange(doc, cursor);
      if (!range) break;
      const info = listLineInfo(doc, range);
      if (info.quoteDepth !== firstDepth) break;
      if (!info.marker) break;
      const { col, width } = info.marker;
      const qualifies = indent ? col <= firstCol : col < firstCol;
      if (qualifies) {
        target = { col, width };
        break;
      }
      cursor = range.start;
    }
    if (!target) return noOp(); // no candidate above: nothing to nest under/from

    let delta: number;
    if (indent) {
      const contentCol = target.col + target.width;
      if (contentCol <= firstCol) return noOp();
      delta = contentCol - firstCol;
    } else {
      // target.col < firstCol by construction (the strict `<` qualifier above).
      delta = firstCol - target.col;
    }

    // Subtree-aware affected set: every intersecting item line, PLUS, for
    // each one, its whole subtree (consecutive following lines at the same
    // quote depth whose marker column is strictly greater than THAT line's
    // own — not the previous line's, so a whole multi-level subtree is
    // captured in one walk). Stops at the first line that fails any of
    // those (a sibling/shallower item, a quote-depth change, or a
    // non-item line — including a blank line).
    const affected = new Map<number, ListLineInfo>();
    for (const line of lines) {
      if (!line.marker) continue;
      const rootCol = line.marker.col;
      if (!affected.has(line.start)) affected.set(line.start, line);
      let cursorEnd = line.end;
      for (;;) {
        const range = nextLineRange(doc, cursorEnd);
        if (!range) break;
        const info = listLineInfo(doc, range);
        if (info.quoteDepth !== line.quoteDepth) break;
        if (!info.marker) break;
        if (info.marker.col <= rootCol) break;
        cursorEnd = range.end;
        if (!affected.has(info.start)) affected.set(info.start, info);
      }
    }

    const splices: Splice[] = [...affected.values()]
      .sort((a, b) => a.start - b.start)
      .flatMap((line) => {
        const col = line.marker!.col;
        if (indent) {
          return [{ at: line.quoteEnd, delete: 0, insert: " ".repeat(delta) }];
        }
        const remove = Math.min(delta, col);
        return remove > 0 ? [{ at: line.quoteEnd, delete: remove, insert: "" }] : [];
      });
    if (splices.length === 0) return noOp();

    // Paragraph-interruption guards (mirror the Rust core exactly — see
    // crates/oxidown-core/src/commands.rs, "Paragraph-interruption guard"):
    // a non-1 ordered item that would START a new list, rather than join an
    // already-open same-delimiter ordered list at its landing column,
    // cannot interrupt a paragraph per CommonMark and would silently
    // degrade to lazy-continuation text. Two lines can end up there:
    // 1. the moved (first affected) line itself — its rewrite goes at index
    //    1 (after the first line's own splice, before the next line's);
    // 2. the first UNAFFECTED item line below the affected set (skipping
    //    adopted descendants) — the edit changed the parse context above it
    //    even though the command never touched it; its rewrite is on a
    //    later line than every whitespace splice, so it appends.
    const newCol = indent ? firstCol + delta : firstCol - delta;
    const rewrite = this.interruptionRewrite(doc, first, newCol, affected, delta, indent);
    if (rewrite) splices.splice(1, 0, rewrite);
    const belowRewrite = this.belowLineRewrite(doc, affected, firstDepth, newCol, delta, indent);
    if (belowRewrite) splices.push(belowRewrite);

    this.pushCommandUndoUnit();
    this.mutateDoc(splices);
    const anchor = mapPos(from, splices, 1);
    const head = mapPos(to, splices, 1);
    return { revision: this.rev, splices, selection: { anchor, head } };
  }

  /**
   * Paragraph-interruption guard for one line (which will sit at
   * `lineColPost` after the edit): the digit-rewrite splice (pre-edit
   * coordinates — the whitespace edits never touch the digits) when its
   * non-1 ordered marker would start a new list rather than join an open
   * one, else null. The landing scan uses POST-EDIT columns: affected
   * lines shift by the batch's per-line change (`+delta` indent,
   * `-min(delta, col)` outdent); unaffected lines keep their pre-edit
   * column. (The first-affected-line check only ever scans above the
   * affected set, where that mapping is the identity; the below-line
   * check's scan crosses the affected set itself.)
   */
  private interruptionRewrite(
    doc: string,
    line: ListLineInfo,
    lineColPost: number,
    affected: Map<number, ListLineInfo>,
    delta: number,
    indent: boolean,
  ): Splice | null {
    const ordered = orderedMarkerParts(line.marker!.glyphs);
    if (!ordered || ordered.isOne) return null; // bullets / "1." never rewrite
    const postCol = (info: ListLineInfo): number => {
      const col = info.marker!.col;
      if (!affected.has(info.start)) return col;
      return indent ? col + delta : col - Math.min(delta, col);
    };
    // Landing scan: skip consecutive same-quote-depth item lines strictly
    // deeper (post-edit) than the checked line's post-edit column; the
    // first line that isn't is the landing.
    let joins = false;
    let cursor = line.start;
    for (;;) {
      const range = prevLineRange(doc, cursor);
      if (!range) break; // document start (unreachable: a target/parent exists above)
      const info = listLineInfo(doc, range);
      if (info.quoteDepth !== line.quoteDepth) break; // outside the quote context
      if (!info.marker) break; // non-item landing: no open list
      const col = postCol(info);
      if (col > lineColPost) {
        cursor = range.start;
        continue;
      }
      // Landing line: joins an open list only at an EQUAL column with the
      // SAME ordered delimiter flavor ('.' vs ')') — a shallower item or a
      // different family means the checked item starts a NEW list, which a
      // non-1 ordered marker cannot do in paragraph-interruption position.
      const landing = orderedMarkerParts(info.marker.glyphs);
      joins = col === lineColPost && landing !== null && landing.delim === ordered.delim;
      break;
    }
    if (joins) return null;
    return {
      at: line.quoteEnd + line.marker!.col,
      delete: ordered.digits.length,
      insert: "1",
    };
  }

  /**
   * Below-context paragraph-interruption guard (mirrors the Rust core's
   * `below_line_rewrite`): walk down from the last affected line over
   * consecutive same-quote-depth item lines, SKIPPING adopted descendants
   * (column strictly greater than the moved line's new column — they nest
   * under the moved block and stay items with it); run the landing-scan
   * check on the first item line at column <= the new column, at its own
   * unchanged column. Stops at a non-item/blank line or quote-depth change.
   */
  private belowLineRewrite(
    doc: string,
    affected: Map<number, ListLineInfo>,
    rootDepth: number,
    rootPostCol: number,
    delta: number,
    indent: boolean,
  ): Splice | null {
    const last = [...affected.values()].sort((a, b) => a.start - b.start).pop();
    if (!last) return null;
    let cursorEnd = last.end;
    for (;;) {
      const range = nextLineRange(doc, cursorEnd);
      if (!range) return null;
      const info = listLineInfo(doc, range);
      if (info.quoteDepth !== rootDepth) return null;
      if (!info.marker) return null;
      if (info.marker.col > rootPostCol) {
        cursorEnd = range.end; // adopted descendant of the moved block
        continue;
      }
      return this.interruptionRewrite(doc, info, info.marker.col, affected, delta, indent);
    }
  }

  /**
   * enter (boundary v0.3): construct-aware Enter — continue a list marker or
   * quote prefix on non-empty content, exit an EMPTY one in a SINGLE press
   * (one level per press). A direct transcription of the Rust core's
   * algorithm (crates/oxidown-core/src/commands.rs `enter`, module doc
   * comment "## enter") over the same line scanning `indentOutdentList`
   * uses. Null when neither construct applies at `from` (plain paragraph,
   * heading, cursor inside the marker/quote prefix region) — the view falls
   * back to the default newline. Unlike indentList/outdentList this never
   * returns an empty-splice no-op: every applicable case produces an edit.
   */
  private enterCmd(fromArg: number, toArg: number): CoreChange | null {
    this.checkDocPos("from", fromArg);
    this.checkDocPos("to", toArg);
    const from = this.strictDocPos(Math.min(fromArg, toArg), "from");
    const to = this.strictDocPos(Math.max(fromArg, toArg), "to");
    const doc = this.doc;
    const line = lineRangeContaining(doc, from);
    const info = listLineInfo(doc, line);
    const isBlank = (s: string): boolean => /^[ \t]*$/.test(s);

    const finish = (splices: Splice[], cursor: number): CoreChange => {
      this.pushCommandUndoUnit();
      this.mutateDoc(splices);
      return { revision: this.rev, splices, selection: { anchor: cursor, head: cursor } };
    };

    if (info.marker) {
      const { col, width, glyphs } = info.marker;
      const markerStart = info.quoteEnd + col;
      // Content start: past the marker token; for a task item, past the
      // "[ ] " run too. Clamped to the line's own end (a bare "-" with no
      // trailing space is still an empty item).
      const taskM = TASK_RE.exec(doc.slice(markerStart + width, line.end));
      const contentStart = Math.min(
        markerStart + width + (taskM ? taskM[0].length : 0),
        line.end,
      );
      if (from < contentStart) return null; // inside the marker's prefix region
      if (!isBlank(doc.slice(contentStart, line.end))) {
        // CONTINUE: "\n" + quote prefix + leading indent + next marker.
        const ordered = orderedMarkerParts(glyphs);
        const nextMarker = ordered
          ? `${parseInt(ordered.digits, 10) + 1}${ordered.delim} `
          : `${glyphs} `;
        const insert = `\n${doc.slice(line.start, markerStart)}${nextMarker}${taskM ? "[ ] " : ""}`;
        const splices: Splice[] = [{ at: from, delete: to - from, insert }];
        return finish(splices, from + insert.length);
      }
      // EXIT/OUTDENT (never inserts a newline — one press, one level).
      if (col > 0) {
        // Outdent this ONE line by the same target-scan/delta arithmetic as
        // indentOutdentList's outdent path, INCLUDING both structural
        // rewrite guards. No subtree: an empty item has none.
        let targetCol: number | null = null;
        let cursor = line.start;
        for (;;) {
          const range = prevLineRange(doc, cursor);
          if (!range) break;
          const prev = listLineInfo(doc, range);
          if (prev.quoteDepth !== info.quoteDepth) break;
          if (!prev.marker) break;
          if (prev.marker.col < col) {
            targetCol = prev.marker.col;
            break;
          }
          cursor = range.start;
        }
        if (targetCol !== null) {
          const delta = col - targetCol;
          const newCol = col - delta;
          const affected = new Map<number, ListLineInfo>([[info.start, info]]);
          const splices: Splice[] = [{ at: info.quoteEnd, delete: delta, insert: "" }];
          const rewrite = this.interruptionRewrite(doc, info, newCol, affected, delta, false);
          if (rewrite) splices.splice(1, 0, rewrite);
          if (from !== to) splices.push({ at: from, delete: to - from, insert: "" });
          // v1 punt (mirrors the Rust core): a selection consuming past this
          // line skips the below-line guard rather than risk overlap.
          if (to <= line.end) {
            const below = this.belowLineRewrite(doc, affected, info.quoteDepth, newCol, delta, false);
            if (below) splices.push(below);
          }
          return finish(splices, mapPos(contentStart, splices, 1));
        }
        // No qualifying parent above: fall through to the marker clear.
      }
      // Top-level: delete the marker token (task brackets included), keep
      // any quote prefix — the line becomes an (empty) paragraph/quote line.
      const splices: Splice[] = [
        { at: markerStart, delete: contentStart - markerStart, insert: "" },
      ];
      if (from !== to) splices.push({ at: from, delete: to - from, insert: "" });
      return finish(splices, mapPos(contentStart, splices, 1));
    }

    if (info.quoteDepth > 0) {
      if (from < info.quoteEnd) return null; // inside the quote markers
      if (!isBlank(doc.slice(info.quoteEnd, line.end))) {
        // QUOTE CONTINUE: "\n" + the line's exact quote prefix.
        const insert = `\n${doc.slice(line.start, info.quoteEnd)}`;
        const splices: Splice[] = [{ at: from, delete: to - from, insert }];
        return finish(splices, from + insert.length);
      }
      // QUOTE EXIT: drop the LAST "> " run element only — one level per
      // press ("> > " -> "> " -> plain), never the whole prefix at once.
      let rest = doc.slice(line.start, line.end);
      let consumed = 0;
      let lastLen = 0;
      for (;;) {
        const m = BQ_MARKER_RE.exec(rest);
        if (!m) break;
        lastLen = m[0].length;
        consumed += lastLen;
        rest = rest.slice(lastLen);
      }
      if (lastLen === 0) return null; // defensive: quoteDepth said otherwise
      const splices: Splice[] = [
        { at: line.start + consumed - lastLen, delete: lastLen, insert: "" },
      ];
      if (from !== to) splices.push({ at: from, delete: to - from, insert: "" });
      return finish(splices, mapPos(info.quoteEnd, splices, 1));
    }

    return null; // neither a list marker nor a quote prefix applies here
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
      // Both ends map with before-bias (composition.rs `map_through`): an
      // insertion exactly at the range end does not extend it — IME edits
      // grow the range explicitly in applyEdit's union step instead.
      this.compFrom = mapPos(this.compFrom, splices, -1);
      this.compTo = Math.max(this.compFrom, mapPos(this.compTo, splices, -1));
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
    // Payload check first (the wasm adapter rejects the payload before the
    // core sees it): no insert may carry an unpaired surrogate into the doc.
    for (const s of splices) {
      if (findLoneSurrogate(s.insert) !== -1) {
        throw new Error("InvalidPayload: splice insert contains an unpaired surrogate");
      }
    }
    let prevEnd = 0;
    for (let i = 0; i < splices.length; i++) {
      const s = splices[i];
      if (!Number.isInteger(s.at) || s.at < 0 || !Number.isInteger(s.delete) || s.delete < 0) {
        throw new Error(
          `InvalidPayload: malformed splices: splice #${i} has at=${s.at} delete=${s.delete}`,
        );
      }
      const end = s.at + s.delete;
      if (i > 0 && s.at < prevEnd) {
        throw new Error(
          `InvalidSplice: splice #${i}: splices must be ascending and non-overlapping ` +
            `(at ${s.at} < previous end ${prevEnd})`,
        );
      }
      if (end > this.doc.length) throw this.outOfBounds(end);
      // Contract clarification 7: a splice boundary inside a surrogate pair
      // would corrupt text — always an error, never snapped.
      if (splitsSurrogatePair(this.doc, s.at)) throw surrogateSplitError(s.at);
      if (splitsSurrogatePair(this.doc, end)) throw surrogateSplitError(end);
      prevEnd = end;
    }
  }
}
