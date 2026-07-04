import {
  Annotation,
  EditorSelection,
  type Extension,
  type Range,
  type Transaction,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  keymap,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import type {
  Decoration as CoreDecoration,
  EditOrigin,
  OxidownCore,
  SelectionRange,
} from "./protocol.js";
import { changesToSplices, endOfLastSplice } from "./splices.js";
import { oxidownTheme } from "./theme.js";

export { changesToSplices, endOfLastSplice } from "./splices.js";

/**
 * Private annotation tagging transactions produced by core-driven undo/redo.
 * The change-forwarding path skips these — the core already applied them.
 */
const oxidownHistory = Annotation.define<"undo" | "redo">();

export interface OxidownOptions {
  /**
   * Render live-preview decorations. Set false for source mode: the document
   * keeps syncing with the core (and undo/redo keeps working), but no
   * conceal/mark/line decorations are shown.
   * Default: true.
   */
  decorations?: boolean;
  /**
   * After every applyEdit, verify core.docLength() against the CM6 doc length;
   * on mismatch, re-load() the core from the view buffer and log loudly.
   * Default: true in dev builds (import.meta.env.DEV), else false.
   */
  verifyMirror?: boolean;
}

const defaultVerifyMirror = (() => {
  try {
    return Boolean((import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV);
  } catch {
    return false;
  }
})();

// Decoration builders (marks/conceals are `mark` decorations — characters are
// NEVER removed or replaced in the DOM; conceal is a visual collapse only).
const markDecos = {
  strong: Decoration.mark({ class: "ox-strong" }),
  em: Decoration.mark({ class: "ox-em" }),
  code: Decoration.mark({ class: "ox-code" }),
  delim: Decoration.mark({ class: "ox-delim" }),
} as const;
const concealDeco = Decoration.mark({ class: "ox-conceal" });
const lineDecos = {
  h1: Decoration.line({ class: "ox-h1" }),
  h2: Decoration.line({ class: "ox-h2" }),
  h3: Decoration.line({ class: "ox-h3" }),
  h4: Decoration.line({ class: "ox-h4" }),
  h5: Decoration.line({ class: "ox-h5" }),
  h6: Decoration.line({ class: "ox-h6" }),
} as const;

function originOf(tr: Transaction, view: EditorView): EditOrigin {
  if (tr.isUserEvent("input.paste") || tr.isUserEvent("input.drop")) return "paste";
  if (view.composing || tr.isUserEvent("input.type.compose")) return "ime";
  return "user";
}

function oxidownPlugin(core: OxidownCore, options: OxidownOptions) {
  const renderDecorations = options.decorations !== false;
  const verifyMirror = options.verifyMirror ?? defaultVerifyMirror;

  return ViewPlugin.fromClass(
    class OxidownView {
      decorations: DecorationSet = Decoration.none;
      private readonly view: EditorView;
      /** True between mousedown and mouseup: reveal recomputation is frozen. */
      dragging = false;
      /** A rebuild is wanted (set by triggers, cleared by a successful rebuild). */
      dirty = false;
      /** Microtask guard: at most one rebuild flush per microtask batch/frame. */
      private scheduled = false;
      private destroyed = false;

      private readonly onWindowMouseUp = () => {
        if (this.dragging) {
          this.dragging = false;
          this.dirty = true;
          this.scheduleRebuild();
        }
      };

      constructor(view: EditorView) {
        this.view = view;
        // Establish the mirror. Skip the load when the core already holds this
        // exact text (e.g. re-created by a source-mode toggle) so history and
        // revisions survive reconfiguration.
        const text = view.state.doc.toString();
        if (core.getText() !== text) core.load(text);
        if (renderDecorations) this.decorations = this.buildDecorations();
        if (typeof window !== "undefined") {
          window.addEventListener("mouseup", this.onWindowMouseUp);
        }
      }

      update(update: ViewUpdate) {
        // 1) Forward every doc change to the core — synchronously, in order,
        //    one applyEdit per transaction (splices are in each transaction's
        //    original-doc coordinates, matching ChangeSet semantics).
        for (const tr of update.transactions) {
          if (!tr.docChanged) continue;
          if (tr.annotation(oxidownHistory)) continue; // core-driven history: already applied core-side
          const splices = changesToSplices(tr.changes);
          try {
            core.applyEdit(core.revision(), splices, originOf(tr, this.view));
            if (verifyMirror && core.docLength() !== tr.newDoc.length) {
              throw new Error(
                `mirror desync: core=${core.docLength()} view=${tr.newDoc.length}`,
              );
            }
          } catch (err) {
            // Contract: any core exception is a mirror-desync emergency.
            console.error(
              "[oxidown] core error during applyEdit — re-loading core from view buffer:",
              err,
            );
            core.load(tr.newDoc.toString());
          }
        }

        if (!renderDecorations) return;

        // Keep positions valid immediately; the real rebuild is coalesced.
        if (update.docChanged) {
          this.decorations = this.decorations.map(update.changes);
        }
        // 2) Recompute decorations on doc change, selection change, or
        //    viewport change — at most once per frame, and never while an IME
        //    composition or a mouse drag-selection is in progress.
        if (update.docChanged || update.selectionSet || update.viewportChanged) {
          this.dirty = true;
          this.scheduleRebuild();
        }
      }

      destroy() {
        this.destroyed = true;
        if (typeof window !== "undefined") {
          window.removeEventListener("mouseup", this.onWindowMouseUp);
        }
      }

      // -- rebuild machinery -------------------------------------------------

      private scheduleRebuild() {
        if (this.scheduled) return;
        this.scheduled = true;
        queueMicrotask(() => {
          this.scheduled = false;
          this.flushRebuild();
        });
      }

      /** Rebuild now unless frozen (composition/drag) — those re-schedule on end. */
      flushRebuild() {
        if (this.destroyed || !this.dirty) return;
        if (this.view.composing || this.dragging) return; // frozen; dirty stays set
        this.dirty = false;
        const decos = this.buildDecorations();
        if (decos === this.decorations) return;
        this.decorations = decos;
        // Plugin-provided decorations are only re-read during an update cycle;
        // an empty dispatch triggers one (and cannot re-trigger a rebuild).
        this.view.dispatch({});
      }

      private buildDecorations(): DecorationSet {
        const state = this.view.state;
        const { from, to } = this.view.viewport;
        const selections: SelectionRange[] = state.selection.ranges.map((r) => ({
          anchor: r.anchor,
          head: r.head,
        }));
        let decos: CoreDecoration[];
        try {
          decos = core.decorations(core.revision(), from, to, selections);
        } catch (err) {
          console.error(
            "[oxidown] core error during decorations — re-loading core from view buffer:",
            err,
          );
          core.load(state.doc.toString());
          try {
            decos = core.decorations(core.revision(), from, to, selections);
          } catch (err2) {
            console.error("[oxidown] decorations still failing after resync:", err2);
            return Decoration.none;
          }
        }
        try {
          const ranges: Range<Decoration>[] = [];
          for (const d of decos) {
            if (d.kind === "line") {
              const line = state.doc.lineAt(Math.min(d.at, state.doc.length));
              ranges.push(lineDecos[d.style].range(line.from));
            } else if (d.kind === "mark") {
              if (d.to > d.from) ranges.push(markDecos[d.style].range(d.from, d.to));
            } else {
              // conceal: mark decoration — never a replace; chars stay in the DOM
              if (d.to > d.from) ranges.push(concealDeco.range(d.from, d.to));
            }
          }
          return Decoration.set(ranges, true);
        } catch (err) {
          console.error("[oxidown] invalid decoration payload from core:", err);
          return Decoration.none;
        }
      }
    },
    {
      decorations: (v) => v.decorations,
      eventHandlers: {
        mousedown() {
          // Freeze reveal recomputation for the duration of a drag-selection;
          // window mouseup (registered in the constructor) unfreezes.
          this.dragging = true;
        },
        compositionstart(_event, view) {
          const r = view.state.selection.main;
          core.compositionBegin(r.from, r.to);
        },
        compositionend() {
          core.compositionEnd();
          this.dirty = true;
          // view.composing stays true until CM has processed the final
          // composition transaction; flush once it has settled.
          setTimeout(() => this.flushRebuild(), 0);
        },
      },
    },
  );
}

function historyKeymap(core: OxidownCore): Extension {
  const run =
    (kind: "undo" | "redo") =>
    (view: EditorView): boolean => {
      const result = kind === "undo" ? core.undo() : core.redo();
      if (result && result.splices.length > 0) {
        const changes = result.splices.map((s) => ({
          from: s.at,
          to: s.at + s.delete,
          insert: s.insert,
        }));
        const cursor = endOfLastSplice(result.splices);
        view.dispatch({
          changes,
          selection: cursor === null ? undefined : EditorSelection.cursor(cursor),
          annotations: oxidownHistory.of(kind),
          scrollIntoView: true,
          userEvent: kind,
        });
      }
      return true; // always consume; never fall through to native undo
    };
  return keymap.of([
    { key: "Mod-z", run: run("undo"), preventDefault: true },
    { key: "Mod-y", run: run("redo"), preventDefault: true },
    { key: "Mod-Shift-z", run: run("redo"), preventDefault: true },
  ]);
}

/**
 * The Oxidown CM6 integration.
 *
 * - Forwards every document change to `core.applyEdit` (splices in
 *   original-doc coordinates, ascending — CM6 ChangeSet semantics).
 * - Renders core-computed decorations (mark/conceal/line) for the viewport,
 *   recomputing at most once per frame and never during IME composition or
 *   mouse drag-selection (the anti-flicker playbook, research/01 §4).
 * - Binds Mod-z / Mod-y / Mod-Shift-z to CORE-DRIVEN undo/redo.
 *
 * IMPORTANT: do NOT include CM6's own history extension
 * (`@codemirror/commands` `history()` / `historyKeymap`) alongside this one —
 * the core is the only historian, and two undo systems will fight.
 */
export function oxidown(core: OxidownCore, options: OxidownOptions = {}): Extension {
  return [oxidownPlugin(core, options), historyKeymap(core), oxidownTheme];
}
