/**
 * StubCore — a deliberately DUMB, scriptable OxidownCore double for the CM6
 * extension's WIRING tests (extension.test.ts): change forwarding, skip
 * annotations, desync recovery, keymap fallback, payload-cache behavior,
 * changeFilter detection, and fault injection.
 *
 * It knows NOTHING about markdown. Contract BEHAVIOR (decorations, reveal,
 * commands, numbering, real undo semantics, streaming edge cases) is tested
 * against the real wasm core (core-contract.test.ts / conformance.test.ts,
 * via test/wasm-loader.ts) — the ONLY implementation of docs/boundary-v0.md.
 * This stub exists purely so view-wiring tests stay fast, deterministic, and
 * trivially scriptable; it deliberately does not validate anything (no
 * revision staleness, no bounds checks, no surrogate policy) unless a test
 * scripts a throw.
 *
 * What it does implement, minimally:
 *  - a plain text buffer + revision counter (`load`/`applyEdit`/`getText`/
 *    `docLength`/`revision`);
 *  - whole-text-snapshot undo/redo: every `applyEdit` pushes the pre-edit
 *    text (no coalescing — one unit per call); `undo`/`redo` return a single
 *    prefix/suffix-diff splice with a selection at its end;
 *  - decorations: `[]` unless scripted;
 *  - commands: `null` (doesn't apply) unless scripted — exactly what the
 *    keymap fallback tests want;
 *  - streams: an insertion point that maps through later `applyEdit` batches
 *    (position bookkeeping, not markdown), so streaming-wiring tests can
 *    interleave user typing with appends;
 *  - anchors: stored positions, unmapped (no wiring test needs more);
 *  - composition: call recording only.
 *
 * Scriptable hooks, per method name:
 *  - `queueReturn(method, value)` — next call returns `value` (FIFO queue);
 *  - `throwOnce(method, error)`   — next call throws `error` instead;
 *  - `calls`                      — every invocation, in order, with args.
 */
import type {
  CoreChange,
  Decoration,
  EditOrigin,
  OxidownCore,
  SelectionRange,
  Splice,
} from "../src/protocol.js";
import { applySplices } from "../src/splices.js";

type MethodName =
  | "load"
  | "applyEdit"
  | "undo"
  | "redo"
  | "decorations"
  | "compositionBegin"
  | "compositionEnd"
  | "getText"
  | "docLength"
  | "revision"
  | "createAnchor"
  | "resolveAnchor"
  | "dropAnchor"
  | "command"
  | "streamOpen"
  | "streamAppend"
  | "streamClose"
  | "destroy";

export interface RecordedCall {
  method: MethodName;
  args: unknown[];
}

/** Minimal single-splice diff between two texts (common prefix/suffix trim). */
function diffSplice(from: string, to: string): Splice[] {
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

/** Map a position through an ascending original-coordinate splice batch. */
function mapThrough(pos: number, splices: Splice[]): number {
  let shift = 0;
  for (const s of splices) {
    const end = s.at + s.delete;
    if (end <= pos) {
      shift += s.insert.length - s.delete;
    } else if (s.at < pos) {
      return s.at + shift; // inside the deleted range: collapse to its start
    } else {
      break;
    }
  }
  return pos + shift;
}

export class StubCore implements OxidownCore {
  private doc = "";
  private rev = 0;
  private undoStack: string[] = [];
  private redoStack: string[] = [];
  private anchors = new Map<number, number>();
  private nextAnchorId = 1;
  /** Open stream insertion points (mapped through applyEdit batches). */
  private streams = new Map<number, number>();
  private nextStreamId = 1;

  /** Every invocation, in order. */
  readonly calls: RecordedCall[] = [];
  private queuedReturns = new Map<MethodName, unknown[]>();
  private queuedThrows = new Map<MethodName, Error[]>();

  // -- scripting hooks --------------------------------------------------------

  /** Queue `value` as the next return of `method` (FIFO per method). */
  queueReturn(method: MethodName, value: unknown): void {
    const q = this.queuedReturns.get(method) ?? [];
    q.push(value);
    this.queuedReturns.set(method, q);
  }

  /** Make the next call to `method` throw `error` (FIFO per method). */
  throwOnce(method: MethodName, error: Error): void {
    const q = this.queuedThrows.get(method) ?? [];
    q.push(error);
    this.queuedThrows.set(method, q);
  }

  /** Recorded calls to `method`. */
  callsTo(method: MethodName): RecordedCall[] {
    return this.calls.filter((c) => c.method === method);
  }

  /**
   * Record the call, honor a scripted throw, and surface a scripted return
   * via the `scripted` sentinel wrapper (so `undefined`/`null` remain
   * queueable values).
   */
  private hook(method: MethodName, args: unknown[]): { scripted: boolean; value?: unknown } {
    this.calls.push({ method, args });
    const throws = this.queuedThrows.get(method);
    if (throws && throws.length > 0) throw throws.shift()!;
    const returns = this.queuedReturns.get(method);
    if (returns && returns.length > 0) return { scripted: true, value: returns.shift() };
    return { scripted: false };
  }

  // -- OxidownCore ------------------------------------------------------------

  load(text: string): number {
    const h = this.hook("load", [text]);
    if (h.scripted) return h.value as number;
    this.doc = text;
    this.undoStack = [];
    this.redoStack = [];
    this.streams.clear();
    return ++this.rev;
  }

  applyEdit(baseRevision: number, splices: Splice[], origin: EditOrigin): number {
    const h = this.hook("applyEdit", [baseRevision, splices, origin]);
    if (h.scripted) return h.value as number;
    if (splices.length === 0) return this.rev;
    this.undoStack.push(this.doc);
    this.redoStack = [];
    // Map every open stream's insertion point through the batch BEFORE
    // mutating (splices are in pre-edit coordinates).
    for (const [id, pos] of this.streams) this.streams.set(id, mapThrough(pos, splices));
    this.doc = applySplices(this.doc, splices);
    return ++this.rev;
  }

  undo(): CoreChange | null {
    const h = this.hook("undo", []);
    if (h.scripted) return h.value as CoreChange | null;
    const before = this.undoStack.pop();
    if (before === undefined) return null;
    this.redoStack.push(this.doc);
    const splices = diffSplice(this.doc, before);
    this.doc = before;
    const end = splices.length > 0 ? splices[0].at + splices[0].insert.length : 0;
    return { revision: ++this.rev, splices, selection: { anchor: end, head: end } };
  }

  redo(): CoreChange | null {
    const h = this.hook("redo", []);
    if (h.scripted) return h.value as CoreChange | null;
    const after = this.redoStack.pop();
    if (after === undefined) return null;
    this.undoStack.push(this.doc);
    const splices = diffSplice(this.doc, after);
    this.doc = after;
    const end = splices.length > 0 ? splices[0].at + splices[0].insert.length : 0;
    return { revision: ++this.rev, splices, selection: { anchor: end, head: end } };
  }

  decorations(
    revision: number,
    from: number,
    to: number,
    selections: SelectionRange[],
  ): Decoration[] {
    const h = this.hook("decorations", [revision, from, to, selections]);
    if (h.scripted) return h.value as Decoration[];
    return [];
  }

  compositionBegin(from: number, to: number): void {
    this.hook("compositionBegin", [from, to]);
  }

  compositionEnd(): void {
    this.hook("compositionEnd", []);
  }

  getText(): string {
    const h = this.hook("getText", []);
    if (h.scripted) return h.value as string;
    return this.doc;
  }

  docLength(): number {
    const h = this.hook("docLength", []);
    if (h.scripted) return h.value as number;
    return this.doc.length;
  }

  revision(): number {
    const h = this.hook("revision", []);
    if (h.scripted) return h.value as number;
    return this.rev;
  }

  createAnchor(pos: number, bias: "before" | "after"): number {
    const h = this.hook("createAnchor", [pos, bias]);
    if (h.scripted) return h.value as number;
    const id = this.nextAnchorId++;
    this.anchors.set(id, pos);
    return id;
  }

  resolveAnchor(id: number): number | null {
    const h = this.hook("resolveAnchor", [id]);
    if (h.scripted) return h.value as number | null;
    return this.anchors.get(id) ?? null;
  }

  dropAnchor(id: number): void {
    const h = this.hook("dropAnchor", [id]);
    if (h.scripted) return;
    this.anchors.delete(id);
  }

  command(name: string, a: number, b?: number): CoreChange | null {
    const h = this.hook("command", b === undefined ? [name, a] : [name, a, b]);
    if (h.scripted) return h.value as CoreChange | null;
    return null; // "doesn't apply here" — the keymap fallback path
  }

  streamOpen(pos: number): number {
    const h = this.hook("streamOpen", [pos]);
    if (h.scripted) return h.value as number;
    const id = this.nextStreamId++;
    this.streams.set(id, pos);
    return id;
  }

  streamAppend(id: number, chunk: string): CoreChange {
    const h = this.hook("streamAppend", [id, chunk]);
    if (h.scripted) return h.value as CoreChange;
    const pos = this.streams.get(id);
    if (pos === undefined) throw new Error(`UnknownStream: stream ${id} is unknown or already closed`);
    this.undoStack.push(this.doc);
    this.redoStack = [];
    this.doc = this.doc.slice(0, pos) + chunk + this.doc.slice(pos);
    this.streams.set(id, pos + chunk.length);
    return {
      revision: ++this.rev,
      splices: [{ at: pos, delete: 0, insert: chunk }],
      selection: null,
    };
  }

  streamClose(id: number): CoreChange | null {
    const h = this.hook("streamClose", [id]);
    if (h.scripted) return h.value as CoreChange | null;
    this.streams.delete(id);
    return null;
  }

  destroy(): void {
    this.hook("destroy", []);
  }
}
