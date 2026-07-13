import type { ChangeSet } from "@codemirror/state";
import type { Splice } from "./protocol.js";

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
 * Convert a CM6 ChangeSet into the boundary protocol's `Splice[]`:
 * ascending, non-overlapping, in original-document coordinates —
 * exactly what `ChangeSet.iterChanges` yields.
 */
export function changesToSplices(changes: ChangeSet): Splice[] {
  const splices: Splice[] = [];
  changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    splices.push({ at: fromA, delete: toA - fromA, insert: inserted.toString() });
  });
  return splices;
}

/**
 * Position at the end of the last splice, in NEW-document coordinates
 * (where the cursor goes after applying core-driven undo/redo splices).
 * Returns null for an empty batch.
 */
export function endOfLastSplice(splices: Splice[]): number | null {
  if (splices.length === 0) return null;
  let shift = 0;
  for (let i = 0; i < splices.length - 1; i++) {
    shift += splices[i].insert.length - splices[i].delete;
  }
  const last = splices[splices.length - 1];
  return last.at + shift + last.insert.length;
}
