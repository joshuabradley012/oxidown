import {
  Annotation,
  EditorSelection,
  type Extension,
  Prec,
  type Range,
  type Transaction,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  drawSelection,
  EditorView,
  keymap,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import type {
  CoreChange,
  Decoration as CoreDecoration,
  EditOrigin,
  OxidownCore,
  RangeCommandName,
  SelectionRange,
} from "./protocol.js";
import { indentLess, indentMore } from "@codemirror/commands";
import { changesToSplices } from "./splices.js";
import { collectFenceRegions, highlightRegions } from "./highlight.js";
import { oxidownTheme } from "./theme.js";

export { changesToSplices, endOfLastSplice } from "./splices.js";

/**
 * Private annotation tagging transactions produced by core-driven changes
 * (undo/redo, commands, streaming). The change-forwarding path skips these —
 * the core already applied them; echoing them back into applyEdit would
 * double-apply the edit and desync revisions.
 */
const oxidownSkip = Annotation.define<true>();

/**
 * Apply a CoreChange (the shape shared by undo/redo/command/streamAppend) to
 * the view: one transaction, splices tagged with the skip annotation so they
 * are never echoed back into applyEdit.
 *
 * Selection handling is deliberate: when the core supplies a `selection`
 * (undo/redo/most commands), the view places the cursor there and scrolls it
 * into view. When it does NOT (streaming appends, checkbox toggles), the
 * dispatch omits `selection` entirely — CM6's default behavior then maps the
 * user's CURRENT selection through the change instead of moving it. This is
 * what lets the user keep typing at the top of the document while an AI
 * stream appends far below: the stream's edits never touch their cursor.
 */
export function applyCoreChange(view: EditorView, change: CoreChange, userEvent: string): void {
  if (change.splices.length === 0 && !change.selection) return;
  const changes = change.splices.map((s) => ({ from: s.at, to: s.at + s.delete, insert: s.insert }));
  const selection = change.selection
    ? EditorSelection.single(change.selection.anchor, change.selection.head)
    : undefined;
  view.dispatch({
    changes,
    selection,
    annotations: oxidownSkip.of(true),
    scrollIntoView: Boolean(change.selection),
    userEvent,
  });
}

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
// v0.2 adds strike/link/url/list-marker mark styles and blockquote/code-fence/
// code-block/hr line styles — same technique, new vocabulary (docs/boundary-v0.md
// "Expanded decoration vocabulary"). Unknown styles are ignored (forward compat).
const markDecos = {
  strong: Decoration.mark({ class: "ox-strong" }),
  em: Decoration.mark({ class: "ox-em" }),
  code: Decoration.mark({ class: "ox-code" }),
  delim: Decoration.mark({ class: "ox-delim" }),
  strike: Decoration.mark({ class: "ox-strike" }),
  link: Decoration.mark({ class: "ox-link" }),
  url: Decoration.mark({ class: "ox-url" }),
  "list-marker": Decoration.mark({ class: "ox-list-marker" }),
} as const;
/**
 * Concealment = CM6 replace decoration (Obsidian's mechanism). The characters
 * remain in the DOCUMENT (copy/edit/positions all intact) — only their DOM
 * rendering collapses. This replaced the earlier zero-width-box CSS hack,
 * whose hidden-but-laid-out inner text gave coordsAtPos phantom x-positions
 * at conceal boundaries (the caret vanished/floated next to `> ` markers).
 * IME safety: the core's composition-stability rule diverts any conceal
 * intersecting a composition to a delim mark BEFORE composition can touch it.
 */
const concealDeco = Decoration.replace({});
const lineDecos = {
  h1: Decoration.line({ class: "ox-h1" }),
  h2: Decoration.line({ class: "ox-h2" }),
  h3: Decoration.line({ class: "ox-h3" }),
  h4: Decoration.line({ class: "ox-h4" }),
  h5: Decoration.line({ class: "ox-h5" }),
  h6: Decoration.line({ class: "ox-h6" }),
  "code-block": Decoration.line({ class: "ox-code-block" }),
  "code-fence": Decoration.line({ class: "ox-code-fence" }),
  hr: Decoration.line({ class: "ox-hr" }),
} as const;
/** hr line whose dashes are revealed: same class family, rule suppressed. */
const hrRevealedLineDeco = Decoration.line({ class: "ox-hr ox-hr-revealed" });
/**
 * A line whose marker region is being edited: rendered as SOURCE — no
 * decorative padding/bars (no li/bq line classes), and the `ox-src` class
 * neutralizes marker box styling so raw `- `/`1. ` render at natural width
 * (no phantom space that could be mistaken for real characters).
 */
const srcLineDeco = Decoration.line({ class: "ox-src" });

/** List-item lines (ALL depths): per-depth hanging-indent classes (cap 4). */
const listItemLineDecos = new Map<number, Decoration>();
function listItemLineDeco(depth: number): Decoration {
  const capped = Math.max(1, Math.min(depth, 4));
  let deco = listItemLineDecos.get(capped);
  if (!deco) {
    deco = Decoration.line({ class: `ox-list-item ox-li-${capped}` });
    listItemLineDecos.set(capped, deco);
  }
  return deco;
}

/** Blockquote line decorations: per depth (cap 3), with a `gap` variant for
 * lines directly followed by a DEEPER quote line (breathing room before the
 * nested block). */
const blockquoteLineDecos = new Map<string, Decoration>();
function blockquoteLineDeco(depth: number, gap: boolean): Decoration {
  const capped = Math.max(1, Math.min(depth, 3));
  const key = `${capped}${gap ? "g" : ""}`;
  let deco = blockquoteLineDecos.get(key);
  if (!deco) {
    deco = Decoration.line({
      class: `ox-blockquote ox-bq-${capped}${gap ? " ox-bq-gap" : ""}`,
    });
    blockquoteLineDecos.set(key, deco);
  }
  return deco;
}

/**
 * The project's first widget island: a task-list checkbox that replaces the
 * "[ ]"/"[x]" source span. `ignoreEvent` tells CM6 to leave DOM events on
 * this widget alone (it never becomes a cursor position or selection target);
 * the click handler is the only thing that reacts, and it goes through the
 * SAME core-driven-change path as everything else (core.command → CoreChange
 * → applyCoreChange) rather than mutating the DOM/doc directly.
 */
class TaskCheckboxWidget extends WidgetType {
  constructor(
    private readonly checked: boolean,
    private readonly pos: number,
    private readonly core: OxidownCore,
  ) {
    super();
  }

  eq(other: TaskCheckboxWidget): boolean {
    return other.checked === this.checked && other.pos === this.pos;
  }

  toDOM(view: EditorView): HTMLElement {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.className = "ox-task-checkbox";
    // Not focusable: Tab must stay with the editor (it indents), never jump
    // into embedded widgets. Clicking still works.
    input.tabIndex = -1;
    input.checked = this.checked;
    input.addEventListener("click", (event) => {
      // Prevent the browser's own default checkbox toggle: the core is the
      // only source of truth for `checked` — the next decoration rebuild
      // reflects whatever the core returns, not the DOM's own click default.
      event.preventDefault();
      let change: CoreChange | null;
      try {
        change = this.core.command("toggleTask", this.pos);
      } catch (err) {
        console.error(
          "[oxidown] core error during command(toggleTask) — re-loading core from view buffer:",
          err,
        );
        this.core.load(view.state.doc.toString());
        return;
      }
      if (change) applyCoreChange(view, change, "oxidown.command");
    });
    return input;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

/**
 * Unordered-list bullet: replaces the raw `- ` marker span with a `•`. Not
 * interactive — `ignoreEvent` returns false so clicks fall through to the
 * editor and place the cursor normally.
 */
class BulletWidget extends WidgetType {
  eq(): boolean {
    return true; // all bullets are identical
  }

  toDOM(): HTMLElement {
    // The dot itself is drawn by CSS (`.ox-bullet::before`, a circle sized
    // and vertically centered independent of any glyph's font metrics).
    const span = document.createElement("span");
    span.className = "ox-bullet";
    return span;
  }

  ignoreEvent(): boolean {
    return false;
  }
}

function originOf(tr: Transaction, view: EditorView): EditOrigin {
  if (tr.isUserEvent("input.paste") || tr.isUserEvent("input.drop")) return "paste";
  if (view.composing || tr.isUserEvent("input.type.compose")) return "ime";
  return "user";
}

function oxidownPlugin(core: OxidownCore, options: OxidownOptions): Extension {
  const renderDecorations = options.decorations !== false;
  const verifyMirror = options.verifyMirror ?? defaultVerifyMirror;

  const plugin = ViewPlugin.fromClass(
    class OxidownView {
      decorations: DecorationSet = Decoration.none;
      private readonly view: EditorView;
      /** True between mousedown and mouseup: reveal recomputation is frozen. */
      dragging = false;
      /**
       * True during a run of vertical cursor motion (Arrow/Page Up/Down).
       * Reveal recomputation is frozen so line geometry stays stable and
       * CM6's goal column (a remembered visual X) keeps mapping to the same
       * document positions — otherwise conceal/reveal width changes between
       * an ArrowUp and the following ArrowDown make the cursor drift.
       */
      verticalMotion = false;
      private verticalTimer: ReturnType<typeof setTimeout> | null = null;
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
          if (tr.annotation(oxidownSkip)) continue; // core-driven change: already applied core-side
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
          this.endVerticalMotion(); // typing ends the gesture
        }
        // 2) Recompute decorations on doc change, selection change, or
        //    viewport change — at most once per frame, and never while an IME
        //    composition, a mouse drag-selection, or a vertical-motion run is
        //    in progress (all three would shift geometry mid-gesture).
        if (update.docChanged || update.selectionSet || update.viewportChanged) {
          this.dirty = true;
          this.scheduleRebuild();
        }
      }

      destroy() {
        this.destroyed = true;
        if (this.verticalTimer !== null) clearTimeout(this.verticalTimer);
        if (typeof window !== "undefined") {
          window.removeEventListener("mouseup", this.onWindowMouseUp);
        }
      }

      /**
       * Called (from the vertical-motion keymap) BEFORE each Arrow/Page
       * Up/Down command runs. Freezes rebuilds for the duration of the run;
       * a trailing timer performs one rebuild shortly after the run ends so
       * reveal state catches up with wherever the cursor stopped.
       */
      noteVerticalMotion() {
        this.verticalMotion = true;
        if (this.verticalTimer !== null) clearTimeout(this.verticalTimer);
        this.verticalTimer = setTimeout(() => {
          this.verticalTimer = null;
          this.endVerticalMotion();
        }, 250);
      }

      private endVerticalMotion() {
        if (!this.verticalMotion) return;
        this.verticalMotion = false;
        this.dirty = true;
        this.scheduleRebuild();
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

      /** Rebuild now unless frozen (composition/drag/vertical run) — those re-schedule on end. */
      flushRebuild() {
        // Source mode: decorations are disabled entirely. This guard must live
        // HERE (the single build choke point), not only in update() — the
        // mouseup and compositionend paths reach flushRebuild directly.
        if (!renderDecorations) return;
        if (this.destroyed || !this.dirty) return;
        if (this.view.composing || this.dragging || this.verticalMotion) return; // frozen; dirty stays set
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
          // Pre-pass: positions where the core revealed raw source as delim
          // marks. An hr line whose dashes are revealed (delim at the line's
          // own position instead of a conceal) drops its drawn rule so the
          // raw `---` isn't overstruck while being edited.
          const delimStarts = new Set<number>();
          for (const d of decos) {
            if (d.kind === "mark" && d.style === "delim") delimStarts.add(d.from);
          }
          // Blockquote lines directly followed by a DEEPER quote line get a
          // small bottom gap (see blockquoteLineDeco).
          const bqLines = decos
            .filter(
              (d): d is Extract<CoreDecoration, { kind: "line" }> =>
                d.kind === "line" && d.style === "blockquote",
            )
            .sort((a, b) => a.at - b.at);
          const bqGapAts = new Set<number>();
          const srcLines = new Set<number>();
          for (let i = 0; i + 1 < bqLines.length; i++) {
            if ((bqLines[i + 1].depth ?? 1) > (bqLines[i].depth ?? 1)) {
              bqGapAts.add(bqLines[i].at);
            }
          }
          const ranges: Range<Decoration>[] = [];
          for (const d of decos) {
            // Forward compatibility: views MUST ignore decoration styles and
            // widget kinds they don't recognize (docs/boundary-v0.md v0.2).
            if (d.kind === "line") {
              if (d.style === "blockquote" || d.style === "list-item") {
                const line = state.doc.lineAt(Math.min(d.at, state.doc.length));
                if (d.revealed) {
                  // Marker being edited: source geometry (no bars/padding)
                  // plus box-styling neutralization via .ox-src.
                  if (!srcLines.has(line.from)) {
                    srcLines.add(line.from);
                    ranges.push(srcLineDeco.range(line.from));
                  }
                } else if (d.style === "blockquote") {
                  ranges.push(blockquoteLineDeco(d.depth ?? 1, bqGapAts.has(d.at)).range(line.from));
                } else {
                  ranges.push(listItemLineDeco(d.depth ?? 1).range(line.from));
                }
                continue;
              }
              if (d.style === "hr" && delimStarts.has(d.at)) {
                const line = state.doc.lineAt(Math.min(d.at, state.doc.length));
                ranges.push(hrRevealedLineDeco.range(line.from));
                continue;
              }
              const deco = lineDecos[d.style as keyof typeof lineDecos];
              if (!deco) continue; // unrecognized line style: ignore
              const line = state.doc.lineAt(Math.min(d.at, state.doc.length));
              ranges.push(deco.range(line.from));
            } else if (d.kind === "mark") {
              const deco = markDecos[d.style as keyof typeof markDecos];
              if (!deco) continue; // unrecognized mark style: ignore
              if (d.to > d.from) ranges.push(deco.range(d.from, d.to));
            } else if (d.kind === "conceal") {
              // conceal: mark decoration — never a replace; chars stay in the DOM
              if (d.to > d.from) ranges.push(concealDeco.range(d.from, d.to));
            } else if (d.kind === "widget") {
              if (d.widget === "task" && d.to > d.from) {
                ranges.push(
                  Decoration.replace({
                    widget: new TaskCheckboxWidget(d.checked ?? false, d.from, core),
                  }).range(d.from, d.to),
                );
              } else if (d.widget === "bullet" && d.to > d.from) {
                ranges.push(
                  Decoration.replace({ widget: new BulletWidget() }).range(d.from, d.to),
                );
              }
              // unrecognized widget kinds: ignore
            }
          }
          // Syntax highlighting for fenced code: view-side derived state
          // (disposable, never touches the core). Languages load lazily; a
          // load completion schedules one more rebuild to paint them in.
          const regions = collectFenceRegions(decos, state);
          if (regions.length > 0) {
            ranges.push(
              ...highlightRegions(state, regions, () => {
                this.dirty = true;
                this.scheduleRebuild();
              }),
            );
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

  // Observe vertical cursor motion BEFORE the default commands run (Prec.high),
  // and return false so they still execute. Freezing rebuilds for the duration
  // of an Arrow/Page Up/Down run keeps line geometry stable, so CM6's goal
  // column (a remembered visual X) round-trips to the same document position.
  const verticalKeys = ["ArrowUp", "ArrowDown", "PageUp", "PageDown"];
  const observeVertical = (view: EditorView): boolean => {
    view.plugin(plugin)?.noteVerticalMotion();
    return false; // never consume — the default motion command runs next
  };
  const verticalMotionKeymap = Prec.high(
    keymap.of(
      verticalKeys.flatMap((key) => [
        { key, run: observeVertical },
        { key: `Shift-${key}`, run: observeVertical },
      ]),
    ),
  );

  return [plugin, verticalMotionKeymap];
}

function historyKeymap(core: OxidownCore): Extension {
  const run =
    (kind: "undo" | "redo") =>
    (view: EditorView): boolean => {
      const result = kind === "undo" ? core.undo() : core.redo();
      if (result) applyCoreChange(view, result, kind);
      return true; // always consume; never fall through to native undo
    };
  return keymap.of([
    { key: "Mod-z", run: run("undo"), preventDefault: true },
    { key: "Mod-y", run: run("redo"), preventDefault: true },
    { key: "Mod-Shift-z", run: run("redo"), preventDefault: true },
  ]);
}

/**
 * Mod-b/i/Shift-x/e toggle strong/em/strike/code over the current selection
 * via `core.command` — the same core-driven-change path as undo/redo and
 * streaming (command → CoreChange → applyCoreChange).
 */
function commandKeymap(core: OxidownCore): Extension {
  const runToggle =
    (name: RangeCommandName) =>
    (view: EditorView): boolean => {
      const { from, to } = view.state.selection.main;
      let change: CoreChange | null;
      try {
        change = core.command(name, from, to);
      } catch (err) {
        console.error(
          `[oxidown] core error during command(${name}) — re-loading core from view buffer:`,
          err,
        );
        core.load(view.state.doc.toString());
        return true;
      }
      if (change) applyCoreChange(view, change, "oxidown.command");
      return true;
    };
  // Tab/Shift-Tab: marker-width-aware Tab nesting (docs/boundary-v0.md
  // "indentList / outdentList") when the selection touches a list item —
  // nests to the PARENT MARKER'S CONTENT COLUMN (2/3/4 spaces depending on
  // the marker), never a fixed 2-space shift. The core resolves this purely
  // from the line(s) the selection touches, so a cursor anywhere in the
  // item's text (not just at its start) indents the whole item, never
  // inserts spaces at the cursor. Falls back to CM6's own indentMore/
  // indentLess when the command doesn't apply (`null`) — e.g. a plain
  // paragraph. When it DOES apply but there's no movement to make (already
  // top-level, first item of its list, …), the core still returns a
  // CoreChange (empty splices, no selection); `applyCoreChange` is then a
  // no-op, and — unlike the `null` case — that does NOT fall back to
  // indentMore/indentLess (the command applied; it just moved nothing).
  const runIndent =
    (name: "indentList" | "outdentList", fallback: (view: EditorView) => boolean) =>
    (view: EditorView): boolean => {
      const { from, to } = view.state.selection.main;
      let change: CoreChange | null;
      try {
        change = core.command(name, from, to);
      } catch (err) {
        console.error(
          `[oxidown] core error during command(${name}) — re-loading core from view buffer:`,
          err,
        );
        core.load(view.state.doc.toString());
        return true;
      }
      if (change === null) return fallback(view);
      applyCoreChange(view, change, "oxidown.command");
      return true;
    };
  return keymap.of([
    { key: "Mod-b", run: runToggle("toggleStrong"), preventDefault: true },
    { key: "Mod-i", run: runToggle("toggleEm"), preventDefault: true },
    { key: "Mod-Shift-x", run: runToggle("toggleStrike"), preventDefault: true },
    { key: "Mod-e", run: runToggle("toggleCode"), preventDefault: true },
    // Tab indents (falling back to CM6's fixed 2-space indentUnit outside
    // list context) instead of moving focus — the standard editor tradeoff;
    // Escape then Tab leaves the editor.
    {
      key: "Tab",
      run: runIndent("indentList", indentMore),
      shift: runIndent("outdentList", indentLess),
      preventDefault: true,
    },
    // Mod-]/Mod-[ are defaultKeymap's indentMore/indentLess — claim them so
    // every indent gesture goes through the SAME marker-width-aware commands
    // as Tab (a flat 2-space indent on a list line de-nests it: a nested item
    // needs the parent marker's width, e.g. 3 under "1. "). This keymap must
    // be registered before defaultKeymap to win (oxidown() before
    // defaultKeymap in the host's extension array — the documented setup).
    { key: "Mod-]", run: runIndent("indentList", indentMore), preventDefault: true },
    { key: "Mod-[", run: runIndent("outdentList", indentLess), preventDefault: true },
  ]);
}

/**
 * Mod-b / Mod-i / Mod-Shift-x / Mod-e keymap for toggleStrong/toggleEm/
 * toggleStrike/toggleCode over the current selection. Exported standalone so
 * it can be composed elsewhere; included in `oxidown()` by default.
 */
export function oxidownCommands(core: OxidownCore): Extension {
  return commandKeymap(core);
}

/**
 * The Oxidown CM6 integration.
 *
 * - Forwards every document change to `core.applyEdit` (splices in
 *   original-doc coordinates, ascending — CM6 ChangeSet semantics).
 * - Renders core-computed decorations (mark/conceal/line/widget) for the
 *   viewport, recomputing at most once per frame and never during IME
 *   composition or mouse drag-selection (the anti-flicker playbook, research/01 §4).
 * - Binds Mod-z / Mod-y / Mod-Shift-z to CORE-DRIVEN undo/redo, and
 *   Mod-b / Mod-i / Mod-Shift-x / Mod-e to CORE-DRIVEN formatting commands.
 *
 * IMPORTANT: do NOT include CM6's own history extension
 * (`@codemirror/commands` `history()` / `historyKeymap`) alongside this one —
 * the core is the only historian, and two undo systems will fight.
 */
export function oxidown(core: OxidownCore, options: OxidownOptions = {}): Extension {
  return [
    oxidownPlugin(core, options),
    historyKeymap(core),
    oxidownCommands(core),
    oxidownTheme,
    // The native browser caret is painted at full line-box height whenever it
    // sits next to a widget/replace decoration (CM inserts cm-widgetBuffer
    // <img>s there, and Chrome sizes the caret to the line box beside replaced
    // elements) — every concealed marker boundary showed an enlarged caret.
    // drawSelection hides the native caret and draws one from coordsAtPos,
    // which reflects the real text metrics at every position.
    drawSelection(),
  ];
}
