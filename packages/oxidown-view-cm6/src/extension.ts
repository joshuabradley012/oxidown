import {
  Annotation,
  EditorSelection,
  type Extension,
  Prec,
  type Range,
  Transaction,
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
import { collectFenceRegions, FenceHighlighter } from "./highlight.js";
import { oxidownTheme } from "./theme.js";

export { changesToSplices, endOfLastSplice } from "./splices.js";

/**
 * Annotation tagging transactions produced by core-driven changes (undo/redo,
 * commands, streaming). The change-forwarding path skips these — the core
 * already applied them; echoing them back into applyEdit would double-apply
 * the edit and desync revisions. Exported so host changeFilters/
 * transactionFilters can recognize oxidown-annotated transactions (which the
 * contract requires them to leave intact), and so tests can construct
 * multi-transaction updates containing core-driven changes.
 */
export const oxidownSkip = Annotation.define<true>();

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
 *
 * Every core-originated dispatch also carries `addToHistory: false`: the core
 * is the only historian (see oxidown()'s doc comment), but a host that
 * wrongly enables CM6's own history() must not accumulate a SECOND undo
 * record of changes the core already tracks in its own units — the two
 * histories would fight over the same edits.
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
    annotations: [oxidownSkip.of(true), Transaction.addToHistory.of(false)],
    scrollIntoView: Boolean(change.selection),
    userEvent,
  });
}

/**
 * Replace each LONE (unpaired) surrogate code unit with U+FFFD, leaving valid
 * surrogate pairs untouched. The cores enforce the no-lone-surrogate document
 * invariant (`load`/`applyEdit` refuse text payloads carrying one), so any
 * view buffer headed into `core.load()` must be sanitized first — see
 * recoverDesyncedMirror below.
 */
export function sanitizeSurrogates(s: string): string {
  let out = "";
  let copied = 0; // everything before this index is already in `out`
  for (let i = 0; i < s.length; i++) {
    const code = s.charCodeAt(i);
    const high = code >= 0xd800 && code <= 0xdbff;
    if (high && i + 1 < s.length) {
      const next = s.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        i++; // valid pair: keep both units
        continue;
      }
    }
    const low = code >= 0xdc00 && code <= 0xdfff;
    if (high || low) {
      out += s.slice(copied, i) + "�";
      copied = i + 1;
    }
  }
  return copied === 0 ? s : out + s.slice(copied);
}

/**
 * Desync-emergency recovery shared by every "re-load the core from the view
 * buffer" call site (applyEdit/decorations/composition/undo-redo catches, the
 * skip-annotated mirror check, and the mixed-batch bail-out in update()).
 *
 * `core.load()` enforces the no-lone-surrogate document invariant, so loading
 * the RAW view buffer can itself throw (`InvalidPayload`) precisely when the
 * desync was CAUSED by a lone-surrogate insertion — and a throw inside a
 * recovery catch block escapes uncaught, crashing the caller with no further
 * recovery path. Instead the buffer is sanitized (lone surrogates → U+FFFD;
 * a 1:1 code-unit replacement, so lengths are preserved) and the core loads
 * the sanitized text, which cannot be refused. When sanitization changed the
 * buffer, the view document must converge on the same U+FFFD text: it is
 * repaired via a skip-annotated dispatch (never forwarded back into
 * applyEdit, and outside CM6 history) followed by one more `core.load` in
 * case an edit landed in between. That repair is deferred to a microtask
 * because several call sites run inside a ViewUpdate (or the plugin
 * constructor), where dispatching is not allowed — the interim window is
 * safe: sanitization preserves length, so the mirror is structurally
 * consistent until the repair lands. The clean-buffer path (the
 * overwhelmingly common case) stays fully synchronous, exactly like the
 * previous bare `core.load(text)`.
 *
 * `text` defaults to the view's current doc; update() passes each
 * transaction's own `tr.newDoc` so a batched update recovers against the
 * right intermediate state.
 */
function recoverDesyncedMirror(core: OxidownCore, view: EditorView, text?: string): void {
  const raw = text ?? view.state.doc.toString();
  const sanitized = sanitizeSurrogates(raw);
  core.load(sanitized);
  if (sanitized === raw) return;
  console.error(
    "[oxidown] view buffer contains lone surrogates — sanitized to U+FFFD; repairing the view document to match",
  );
  queueMicrotask(() => {
    // Recompute against the CURRENT doc: another edit (or another queued
    // repair) may have landed between the failure and this microtask.
    // Dispatching on a destroyed view is a CM6 no-op, so no guard is needed.
    const now = view.state.doc.toString();
    const fixed = sanitizeSurrogates(now);
    if (fixed !== now) {
      view.dispatch({
        changes: { from: 0, to: now.length, insert: fixed },
        annotations: [oxidownSkip.of(true), Transaction.addToHistory.of(false)],
        userEvent: "oxidown.recover",
      });
    }
    if (core.getText() !== fixed) core.load(fixed);
  });
}

/**
 * True for a core validation REFUSAL (contract: thrown before any mutation,
 * with a CoreErrorName message prefix like `InvalidRange: ...`): these are
 * expected, contract-mandated "no" answers — e.g. a toggle over a multi-block
 * selection — not integration failures, so they must not spam console.error.
 * Matched by string prefix (on both `err.name` and the message's leading
 * name) rather than the CoreErrorName union, so new Invalid* refusals keep
 * working without a type change.
 */
function isValidationRefusal(err: unknown): boolean {
  if (!(err instanceof Error)) return false;
  return /^Invalid/.test(err.name) || /^Invalid\w*:/.test(err.message);
}

/**
 * Shared wrapper for every `core.command(...)` call site (Mod-b/i/Shift-x/e
 * toggles, Tab/Shift-Tab indent/outdent, Enter, Mod-L/checkbox-widget
 * toggleTask) — replaces four separately hand-rolled try/command/catch
 * blocks (a confirmed duplication). Exported so a host can drive the exact
 * same command → CoreChange → applyCoreChange path from something that
 * isn't a keybinding at all — e.g. a toolbar button (see the web demo) —
 * without re-implementing the validation-refusal/error-swallowing policy
 * below.
 *
 * On a thrown exception this logs and SWALLOWS it rather than treating it as
 * a mirror-desync emergency: `command()` is transactional — planning
 * happens entirely before any apply (the wasm entry point validates the
 * command name BEFORE dispatch) — so it either returns null/CoreChange or
 * throws WITHOUT having mutated the core (docs/boundary-v0.md "Commands").
 * Calling `core.load()` here would needlessly wipe undo history/anchors even
 * though the mirror was never actually broken. This is deliberately
 * DIFFERENT from the applyEdit/decorations catch blocks elsewhere in this
 * file, which still treat any exception as a desync emergency (a docChanged
 * transaction may have partially diverged the buffers) per the contract's
 * general "Error handling" rule — only `command()` carries the
 * no-mutation-on-throw guarantee.
 *
 * Returns `{ ok: true, change }` on a normal call (`change` may itself be
 * `null` — a legitimate "doesn't apply here" result some callers fall back
 * from), or `{ ok: false }` when the command threw. Callers must treat that
 * distinctly from a legitimate `null`: never fall back to a default command
 * just because the core errored.
 */
export function runCoreCommand(
  name: string,
  invoke: () => CoreChange | null,
): { ok: true; change: CoreChange | null } | { ok: false } {
  try {
    return { ok: true, change: invoke() };
  } catch (err) {
    if (isValidationRefusal(err)) {
      // A contract-mandated refusal (Invalid* name, thrown before any
      // mutation) — e.g. toggleStrong over a multi-block selection. This is
      // an expected "no", not an integration failure: keep it off
      // console.error so hosts' error monitoring stays quiet.
      console.debug(`[oxidown] command(${name}) refused by core validation (no mutation):`, err);
    } else {
      console.error(
        `[oxidown] core error during command(${name}) — command() is transactional and did not mutate the core; ignoring:`,
        err,
      );
    }
    return { ok: false };
  }
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
  /**
   * Include CM6's `drawSelection()` in the bundle (see the rationale comment
   * inside `oxidown()`: the native caret renders at full line-box height next
   * to widget/replace decorations). Set false when the host composes its own
   * drawSelection (CM6 dedupes by config, so a doubled one usually works, but
   * a host may need different drawSelection options — e.g. cursorBlinkRate —
   * which WOULD conflict). Default: true.
   */
  drawSelection?: boolean;
}

const defaultVerifyMirror = (() => {
  try {
    return Boolean((import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV);
  } catch {
    return false;
  }
})();

// Decoration builders. Marks are `mark` decorations; conceal is a `replace`
// decoration (see concealDeco below) — either way the characters stay in the
// DOCUMENT; conceal is a visual collapse only.
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
    private readonly core: OxidownCore,
  ) {
    super();
  }

  // Deliberately position-independent (matching OrderedMarkerWidget): the
  // click handler resolves its target from the DOM at click time (posAtDOM
  // below), so `checked` is this widget's entire identity. Comparing a
  // construction-time position here would make every edit above a task line
  // (d.from shifts) destroy and recreate every checkbox below it — DOM
  // churn, lost hover state, dropped clicks — for no correctness gain.
  eq(other: TaskCheckboxWidget): boolean {
    return other.checked === this.checked;
  }

  toDOM(view: EditorView): HTMLElement {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.className = "ox-task-checkbox";
    // Not focusable: Tab must stay with the editor (it indents), never jump
    // into embedded widgets. Clicking still works.
    input.tabIndex = -1;
    // tabIndex=-1 removes it from the tab order but not the accessibility
    // tree — give screen readers a name for what the click toggles.
    input.setAttribute("aria-label", "task checkbox");
    input.checked = this.checked;
    input.addEventListener("mousedown", (event) => {
      // Chrome focuses form controls on mousedown; the post-toggle
      // decoration rebuild then replaces the focused input, dropping focus
      // to <body> and killing subsequent typing. preventDefault keeps focus
      // in the editor (the standard CM6 checkbox-widget pattern) — the
      // click event below still fires and performs the toggle.
      event.preventDefault();
    });
    input.addEventListener("click", (event) => {
      // Prevent the browser's own default checkbox toggle: the core is the
      // only source of truth for `checked` — the next decoration rebuild
      // reflects whatever the core returns, not the DOM's own click default.
      event.preventDefault();
      // Read-only editor: the click must not edit the document (and the
      // preventDefault above already stopped the DOM checkbox from lying
      // about its state).
      if (view.state.readOnly) return;
      // Resolve the CURRENT position from the DOM at click time — never a
      // position captured at CONSTRUCTION. RangeSet.map repositions this
      // widget's decoration range on every doc change without touching
      // this instance's own fields (and rebuilds are microtask-deferred,
      // frozen during composition/drag), so a click that lands after an
      // edit above this task line — especially mid-composition — could
      // toggle the WRONG task (or silently no-op)
      // if we used a stale constructor value. `posAtDOM` reads the
      // widget's LIVE position from the view's current decoration set
      // instead; toggleTask resolves leniently from "anywhere in the list
      // item" so this doesn't need to be exact, just on the right line.
      const live = view.posAtDOM(input);
      const pos = Math.max(0, Math.min(live, view.state.doc.length));
      const outcome = runCoreCommand("toggleTask", () => this.core.command("toggleTask", pos));
      if (outcome.ok && outcome.change) applyCoreChange(view, outcome.change, "oxidown.command");
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

/**
 * Ordered-list marker: replaces the raw "1. "/"2) " marker span with the
 * VIEW-COMPUTED display number (contract v0.3 amendment, research/07
 * §0/§1.2) — CommonMark only gives a list's `start` number meaning, so the
 * core computes each item's position-in-run and the view renders THAT,
 * never the item's raw source digits (which stay untouched on disk; the
 * source is only shown when the line is revealed, as raw `mark:list-marker`
 * text). Not interactive — `ignoreEvent` returns false so clicks fall
 * through to the editor and place the cursor normally (matching
 * BulletWidget).
 */
class OrderedMarkerWidget extends WidgetType {
  constructor(
    private readonly number: number,
    private readonly delim: string,
  ) {
    super();
  }

  eq(other: OrderedMarkerWidget): boolean {
    return other.number === this.number && other.delim === this.delim;
  }

  toDOM(): HTMLElement {
    // Unlike the bullet's pseudo-element dot, this box's content IS text —
    // see theme.ts's `.ox-ordered-marker` for the measured height/baseline
    // treatment (a text baseline behaves differently from a centered dot).
    // The widget replaces the WHOLE marker span, required trailing space
    // included (boundary-v0.md clarification 3: "list-marker spans include
    // the required trailing whitespace") — rendered as a trailing NBSP
    // (never collapsed/trimmed by the whitespace algorithm, unlike a plain
    // space at an inline-block's own trailing edge) so it participates in
    // `text-align: right` exactly like the revealed raw-text mark does,
    // reliably reproducing the gap before the item text.
    const span = document.createElement("span");
    span.className = "ox-ordered-marker";
    span.textContent = `${this.number}${this.delim} `;
    return span;
  }

  ignoreEvent(): boolean {
    return false;
  }
}

/**
 * Field-wise equality over two decoration payloads (same order, same items).
 * Used to skip rebuilds when a cursor-only invalidation produced an identical
 * payload. Unknown future kinds compare unequal — that only costs a rebuild,
 * never a stale render (forward compatibility, boundary-v0.md v0.2).
 */
function payloadsEqual(a: CoreDecoration[], b: CoreDecoration[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i];
    const y = b[i];
    if (x.kind !== y.kind) return false;
    switch (x.kind) {
      case "mark": {
        const m = y as typeof x;
        if (x.from !== m.from || x.to !== m.to || x.style !== m.style) return false;
        break;
      }
      case "conceal": {
        const c = y as typeof x;
        if (x.from !== c.from || x.to !== c.to) return false;
        break;
      }
      case "line": {
        const l = y as typeof x;
        if (x.at !== l.at || x.style !== l.style || x.depth !== l.depth || x.revealed !== l.revealed) {
          return false;
        }
        break;
      }
      case "widget": {
        const w = y as typeof x;
        if (
          x.from !== w.from ||
          x.to !== w.to ||
          x.widget !== w.widget ||
          x.checked !== w.checked ||
          x.number !== w.number ||
          x.delim !== w.delim
        ) {
          return false;
        }
        break;
      }
      default:
        return false;
    }
  }
  return true;
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
      /** True between mousedown and the gesture's end (mouseup, or dragend/
       * drop for a native drag-and-drop): reveal recomputation is frozen. */
      dragging = false;
      /**
       * True during a run of vertical cursor motion (ArrowUp/ArrowDown).
       * Reveal recomputation is frozen so line geometry stays stable and
       * CM6's goal column (a remembered visual X) keeps mapping to the same
       * document positions — otherwise conceal/reveal width changes between
       * an ArrowUp and the following ArrowDown make the cursor drift.
       * Page keys are deliberately NOT included: decorations only cover the
       * previously-fetched viewport, so freezing a PageUp/PageDown that jumps
       * past the render margin shows raw undecorated text for the freeze
       * window, and the goal-column rationale doesn't apply to page motion.
       */
      verticalMotion = false;
      private verticalTimer: ReturnType<typeof setTimeout> | null = null;
      /** A rebuild is wanted (set by triggers, cleared by a successful rebuild). */
      dirty = false;
      /** Microtask guard: rebuild triggers within one task coalesce into a
       * single flush (queueMicrotask coalesces per task, not per frame). */
      private scheduled = false;
      /**
       * Fenced-code parse/tree caches, PER plugin instance (created with it,
       * released with it) — see FenceHighlighter's doc for why these must
       * not be module-global. Only the language-load registry is shared.
       */
      private readonly highlighter = new FenceHighlighter();
      private destroyed = false;
      /**
       * The core payload behind the current `decorations` set. Cursor-only
       * invalidations very often produce an IDENTICAL payload (line-level
       * reveal: caret moves within a line, or between lines with no marker
       * constructs, change nothing) — comparing against this skips the
       * RangeSet rebuild and the re-render dispatch entirely.
       */
      private lastPayload: CoreDecoration[] | null = null;
      /**
       * The core revision `lastPayload` was fetched at. Positions in
       * `lastPayload` are absolute: any doc change invalidates the
       * comparison (a post-edit payload could coincidentally equal a
       * pre-edit one while meaning different text) — `core.revision()`
       * bumps on every doc-mutating path in both cores and does NOT bump on
       * selection changes, which is exactly the invalidation this cache
       * skip wants. The skip-compare below is valid iff `core.revision()`
       * still equals this value; `null` (no payload cached yet) can never
       * match a real revision, so it starts "always stale" the same way the
       * old hand-paired boolean did. This replaces a `payloadMaybeStale`
       * boolean that had to be kept in lockstep by hand at several call
       * sites — a structural invalidation key instead of a manually-set flag.
       */
      private lastPayloadRevision: number | null = null;

      /**
       * Releases the mousedown freeze and flushes the deferred rebuild.
       * Registered window-level for "mouseup" AND "dragend"/"drop": dragging
       * an EXISTING selection hands the gesture to native HTML5 drag-and-drop,
       * which ends with dragend/drop and never fires mouseup — without those
       * two, the freeze would persist (flushRebuild early-returns) until the
       * next click.
       */
      private readonly onDragGestureEnd = () => {
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
        // revisions survive reconfiguration. Routed through the surrogate-safe
        // recovery: a host-supplied initial doc carrying a lone surrogate must
        // not make load() throw out of the constructor.
        const text = view.state.doc.toString();
        if (core.getText() !== text) recoverDesyncedMirror(core, view, text);
        if (renderDecorations) this.decorations = this.buildDecorations();
        if (typeof window !== "undefined") {
          window.addEventListener("mouseup", this.onDragGestureEnd);
          window.addEventListener("dragend", this.onDragGestureEnd);
          window.addEventListener("drop", this.onDragGestureEnd);
        }
      }

      update(update: ViewUpdate) {
        // 1) Forward every doc change to the core — synchronously, in order,
        //    one applyEdit per transaction (splices are in each transaction's
        //    original-doc coordinates, matching ChangeSet semantics).
        //
        // The LAST doc-changing transaction is the only one whose newDoc is
        // the update's final doc — the skip-annotated mirror check below must
        // only run there. The core applied every core-driven change BEFORE
        // this update ran, so when a host batches several transactions into
        // one update, core.docLength() is already the FINAL length while a
        // non-last tr.newDoc is an intermediate state: comparing those would
        // false-positive and needlessly wipe undo history/anchors via load().
        let lastDocChanged: Transaction | null = null;
        let sawSkipDocChange = false;
        let sawPlainDocChange = false;
        for (const tr of update.transactions) {
          if (!tr.docChanged) continue;
          lastDocChanged = tr;
          if (tr.annotation(oxidownSkip)) sawSkipDocChange = true;
          else sawPlainDocChange = true;
        }
        if (sawSkipDocChange && sawPlainDocChange) {
          // A single update batching a core-driven (skip-annotated) change
          // TOGETHER with a plain doc-changing transaction cannot be forwarded
          // splice-by-splice: the core applied the skip change(s) when they
          // were produced (BEFORE this update ran), so a plain transaction's
          // splices — expressed against whatever view doc it was built on —
          // are in the WRONG coordinates for the core's current doc.
          // Forwarding them would silently corrupt the mirror (verifyMirror
          // off) or wipe history twice (verifyMirror on). Treat the whole
          // batch as a desync emergency instead: one recovery reload against
          // the update's final doc.
          console.error(
            "[oxidown] a single update batched a core-driven (skip-annotated) change with a " +
              "plain doc-changing transaction — user splices cannot be forwarded safely; " +
              "re-loading core from view buffer",
          );
          recoverDesyncedMirror(core, this.view, update.state.doc.toString());
        } else {
          for (const tr of update.transactions) {
            if (!tr.docChanged) continue;
            if (tr.annotation(oxidownSkip)) {
              // Core-driven change (undo/redo/command/stream): the core
              // already applied this edit itself before the view dispatched
              // the transaction, so there's nothing to forward. But a host
              // changeFilter/transactionFilter that altered the transaction in
              // flight (the contract requires hosts not do this to
              // oxidown-annotated transactions) would otherwise desync core
              // and view silently until the NEXT forwarded edit happened to
              // notice via the check below — and only then if verifyMirror is
              // on. Run the same length check immediately instead of waiting:
              // the core already applied the change, so lengths must match
              // right now — but only against the update's FINAL doc (the last
              // doc-changing transaction; see `lastDocChanged` above), never a
              // batched update's intermediate per-transaction doc.
              if (verifyMirror && tr === lastDocChanged && core.docLength() !== tr.newDoc.length) {
                console.error(
                  `[oxidown] mirror desync on a core-driven change (core=${core.docLength()} view=${tr.newDoc.length}) — ` +
                    "a host changeFilter/transactionFilter may have altered an oxidown-annotated " +
                    "transaction; re-loading core from view buffer:",
                );
                recoverDesyncedMirror(core, this.view, tr.newDoc.toString());
              }
              continue;
            }
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
              // recoverDesyncedMirror (not a bare core.load) because the
              // failed edit may itself be WHY the view buffer is unloadable —
              // a lone-surrogate insertion makes load(raw buffer) throw the
              // same InvalidPayload right back, uncaught, crashing the plugin.
              console.error(
                "[oxidown] core error during applyEdit — re-loading core from view buffer:",
                err,
              );
              recoverDesyncedMirror(core, this.view, tr.newDoc.toString());
            }
          }
        }

        if (!renderDecorations) return;

        // Keep positions valid immediately; the real rebuild is coalesced.
        // (No explicit "mark stale" step needed here: applyEdit above — or,
        // for a skip-annotated core-driven change, the core mutation that
        // produced it — already bumped core.revision(), which is what the
        // skip-compare in flushRebuild checks against.)
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
          window.removeEventListener("mouseup", this.onDragGestureEnd);
          window.removeEventListener("dragend", this.onDragGestureEnd);
          window.removeEventListener("drop", this.onDragGestureEnd);
        }
      }

      /**
       * Called (from the vertical-motion keymap) BEFORE each ArrowUp/
       * ArrowDown command runs. Freezes rebuilds for the duration of the run;
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
        const payload = this.fetchPayload();
        const revision = core.revision();
        // Identical payload + unchanged doc (same core revision as when
        // `lastPayload` was cached) → the current decorations are already
        // exactly right: skip the RangeSet rebuild AND the re-render
        // dispatch (the bulk of a cursor-move rebuild's cost).
        if (
          payload !== null &&
          this.lastPayloadRevision === revision &&
          this.lastPayload !== null &&
          payloadsEqual(this.lastPayload, payload)
        ) {
          return;
        }
        if (payload !== null) {
          this.lastPayload = payload;
          this.lastPayloadRevision = revision;
        } else {
          this.lastPayload = null; // resync failed: never skip off a bad cache
          this.lastPayloadRevision = null;
        }
        const decos = payload === null ? Decoration.none : this.buildDecorationSet(payload);
        if (decos === this.decorations) return;
        this.decorations = decos;
        // Plugin-provided decorations are only re-read during an update cycle;
        // an empty dispatch triggers one (and cannot re-trigger a rebuild).
        this.view.dispatch({});
      }

      /** Constructor-time build: fetch + construct + seed the payload cache. */
      private buildDecorations(): DecorationSet {
        const payload = this.fetchPayload();
        if (payload === null) return Decoration.none;
        this.lastPayload = payload;
        this.lastPayloadRevision = core.revision();
        return this.buildDecorationSet(payload);
      }

      /** Core decorations for the current viewport (with desync recovery). */
      private fetchPayload(): CoreDecoration[] | null {
        const state = this.view.state;
        const { from, to } = this.view.viewport;
        const selections: SelectionRange[] = state.selection.ranges.map((r) => ({
          anchor: r.anchor,
          head: r.head,
        }));
        try {
          return core.decorations(core.revision(), from, to, selections);
        } catch (err) {
          console.error(
            "[oxidown] core error during decorations — re-loading core from view buffer:",
            err,
          );
          recoverDesyncedMirror(core, this.view, state.doc.toString());
          try {
            return core.decorations(core.revision(), from, to, selections);
          } catch (err2) {
            console.error("[oxidown] decorations still failing after resync:", err2);
            return null;
          }
        }
      }

      private buildDecorationSet(decos: CoreDecoration[]): DecorationSet {
        const state = this.view.state;
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
              // conceal: a replace decoration — the chars stay in the
              // DOCUMENT (positions/copy/edit intact) but their DOM
              // rendering collapses. See concealDeco's rationale block for
              // why replace, not a CSS-hidden mark.
              if (d.to > d.from) ranges.push(concealDeco.range(d.from, d.to));
            } else if (d.kind === "widget") {
              if (d.widget === "task" && d.to > d.from) {
                ranges.push(
                  Decoration.replace({
                    widget: new TaskCheckboxWidget(d.checked ?? false, core),
                  }).range(d.from, d.to),
                );
              } else if (d.widget === "bullet" && d.to > d.from) {
                ranges.push(
                  Decoration.replace({ widget: new BulletWidget() }).range(d.from, d.to),
                );
              } else if (d.widget === "ordered" && d.to > d.from && d.number !== undefined && d.delim) {
                ranges.push(
                  Decoration.replace({
                    widget: new OrderedMarkerWidget(d.number, d.delim),
                  }).range(d.from, d.to),
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
              ...this.highlighter.highlightRegions(state, regions, () => {
                // Invalidate the payload cache BEFORE scheduling: a language
                // arriving with no other event in between leaves the CORE
                // payload (and core.revision()) unchanged, so flushRebuild's
                // identical-payload skip would swallow the repaint — the
                // cache only keys on the core payload, not on highlighter
                // state. Nulling it forces the rebuild through to
                // buildDecorationSet, which re-runs the highlighter (now
                // with the loaded language — or retrying a FAILED load; see
                // highlight.ts's supportFor).
                this.lastPayload = null;
                this.lastPayloadRevision = null;
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
      // KNOWN TRADEOFF (evaluated, deliberately kept): height-affecting line
      // decorations (.ox-h1's 1.6em font, .ox-bq-gap padding, .ox-li-*
      // padding) are provided from this ViewPlugin, and CM6 reads plugin
      // decorations only AFTER viewport/height estimation — so on heading-
      // heavy documents, estimated line heights are corrected in the
      // post-paint measure cycle (scroll-position estimation jitter). The
      // canonical placement for height-affecting decorations is a StateField
      // (read before estimation). Converting is NOT contained here, because
      // the machinery is built around plugin-owned decorations:
      //   - the constructor builds the first set synchronously against the
      //     freshly-loaded mirror and the LIVE viewport; a StateField's
      //     create() runs at state creation, before either exists, so the
      //     first paint would be undecorated and flash in a microtask later;
      //   - fetchPayload is viewport-scoped, so the field would still only
      //     be populated by plugin-dispatched StateEffects (the flushRebuild
      //     dispatch at the bottom of this class carrying the new set), and
      //     every freeze path (composition/drag/vertical-motion) plus the
      //     identical-payload skip and the "decos === this.decorations"
      //     short-circuit would need re-plumbing onto effect dispatches.
      // The conversion would take: a StateEffect<DecorationSet> + StateField
      // mapping through changes, provided via EditorView.decorations.from(
      // field), the flushRebuild dispatch carrying the effect instead of
      // being empty, and a deferred (post-construction) initial dispatch.
      decorations: (v) => v.decorations,
      eventHandlers: {
        mousedown() {
          // Freeze reveal recomputation for the duration of a drag gesture;
          // window mouseup/dragend/drop (registered in the constructor)
          // unfreeze — see onDragGestureEnd.
          this.dragging = true;
        },
        compositionstart(_event, view) {
          const r = view.state.selection.main;
          try {
            core.compositionBegin(r.from, r.to);
          } catch (err) {
            // Contract "Error handling": like the applyEdit/decorations call
            // sites (and unlike command(), which is transactional), any
            // exception here is a mirror-desync emergency — log loudly and
            // re-load the core from the view buffer.
            console.error(
              "[oxidown] core error during compositionBegin — re-loading core from view buffer:",
              err,
            );
            recoverDesyncedMirror(core, view);
            this.dirty = true; // rebuilt after the composition settles
          }
        },
        compositionend(_event, view) {
          try {
            core.compositionEnd();
          } catch (err) {
            // Same desync-emergency discipline as compositionstart above.
            console.error(
              "[oxidown] core error during compositionEnd — re-loading core from view buffer:",
              err,
            );
            recoverDesyncedMirror(core, view);
          }
          // Runs whether or not compositionEnd threw: the catch-up rebuild
          // must still be scheduled (and after a resync, doubly so).
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
  // of an ArrowUp/ArrowDown run keeps line geometry stable, so CM6's goal
  // column (a remembered visual X) round-trips to the same document position.
  // PageUp/PageDown are NOT frozen (see the `verticalMotion` field's doc).
  const verticalKeys = ["ArrowUp", "ArrowDown"];
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
      // Read-only editor: undo/redo would edit the document. Return false
      // (CM6 convention: the command doesn't apply) WITHOUT touching the
      // core's history stacks, so a host binding may still claim the key.
      if (view.state.readOnly) return false;
      let result: CoreChange | null;
      try {
        result = kind === "undo" ? core.undo() : core.redo();
      } catch (err) {
        // Contract "Error handling": unlike command() (transactional,
        // no-mutation-on-throw — see runCoreCommand), undo/redo mutate the
        // core BEFORE returning splices, so an exception leaves the mirror
        // in an unknown state. Treat it like the applyEdit/decorations
        // sites: a desync emergency — log loudly and re-load from the view
        // buffer (surrogate-safe; see recoverDesyncedMirror).
        console.error(
          `[oxidown] core error during ${kind} — re-loading core from view buffer:`,
          err,
        );
        recoverDesyncedMirror(core, view);
        return true;
      }
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
  // Read-only editor: every binding here would edit the document, so each
  // runner returns false (CM6 convention: the command doesn't apply here)
  // WITHOUT dispatching anything to the core — a later keymap/host binding
  // may still claim the key.
  const runToggle =
    (name: RangeCommandName) =>
    (view: EditorView): boolean => {
      if (view.state.readOnly) return false;
      const { from, to } = view.state.selection.main;
      const outcome = runCoreCommand(name, () => core.command(name, from, to));
      if (outcome.ok && outcome.change) applyCoreChange(view, outcome.change, "oxidown.command");
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
      if (view.state.readOnly) return false;
      const { from, to } = view.state.selection.main;
      const outcome = runCoreCommand(name, () => core.command(name, from, to));
      // An exception is NOT the same as a legitimate `null` — it must never
      // fall back to indentMore/indentLess (that could apply an entirely
      // unrelated edit); treat it as handled, like every other command-throw
      // site.
      if (!outcome.ok) return true;
      if (outcome.change === null) return fallback(view);
      applyCoreChange(view, outcome.change, "oxidown.command");
      return true;
    };
  // Enter: construct-aware continuation/exit (docs/boundary-v0.md "enter",
  // v0.3; research/07 §1.3/§1.4/§2.1). The core continues a list marker or
  // quote prefix on non-empty content, and exits an EMPTY one in a SINGLE
  // press (one level per press — no Obsidian double-Enter quirk). `null` =
  // no list/quote context at the cursor → return false so the default Enter
  // (a plain newline, via defaultKeymap registered after this keymap) runs.
  // Never intercept while an IME composition is active: Enter then belongs
  // to the composition (confirming a candidate), not to us.
  const runEnter = (view: EditorView): boolean => {
    if (view.composing || view.state.readOnly) return false;
    const { from, to } = view.state.selection.main;
    const outcome = runCoreCommand("enter", () => core.command("enter", from, to));
    // Same distinction as runIndent: a thrown command is handled-and-ignored,
    // never treated as "doesn't apply" (which would insert a plain newline
    // on top of whatever state the core is actually in).
    if (!outcome.ok) return true;
    if (outcome.change === null) return false; // default Enter runs
    applyCoreChange(view, outcome.change, "oxidown.command");
    return true;
  };
  // Mod-Shift-Enter / Mod-L: two keyboard paths to the SAME task-checkbox
  // toggle (a11y — the widget's click handler must not be the only way to
  // toggle). Both go through core.command("toggleTask") targeted at the
  // cursor's line (toggleTask resolves leniently from anywhere in the item,
  // like the checkbox click). `null` (not a task line) falls through — see
  // each binding below for what "falls through" means for that specific key.
  // Mod-Shift-Enter is not bound by defaultKeymap (which uses Mod-Enter for
  // insertBlankLine) nor elsewhere in this file.
  const runToggleTask = (view: EditorView): boolean => {
    if (view.state.readOnly) return false;
    const pos = view.state.selection.main.head;
    const outcome = runCoreCommand("toggleTask", () => core.command("toggleTask", pos));
    if (!outcome.ok) return true; // thrown: handled-and-ignored, never "doesn't apply"
    if (outcome.change === null) return false;
    applyCoreChange(view, outcome.change, "oxidown.command");
    return true;
  };
  return keymap.of([
    { key: "Mod-b", run: runToggle("toggleStrong"), preventDefault: true },
    { key: "Mod-i", run: runToggle("toggleEm"), preventDefault: true },
    { key: "Mod-Shift-x", run: runToggle("toggleStrike"), preventDefault: true },
    { key: "Mod-e", run: runToggle("toggleCode"), preventDefault: true },
    // Enter continues/exits list markers and quote prefixes through the core
    // (null → falls through to the default newline). No preventDefault: when
    // the binding declines (plain paragraph, composing), the event must stay
    // fully default so CM6's own Enter path handles it.
    { key: "Enter", run: runEnter },
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
    // Keyboard path for the task checkbox (see runToggleTask above).
    { key: "Mod-Shift-Enter", run: runToggleTask, preventDefault: true },
    // Obsidian-parity shortcut for the same toggle (research/07 §1.6:
    // Obsidian's default "Toggle checkbox status" hotkey since 1.0, moved off
    // Mod-Enter to free that combo for link-opening). `preventDefault: true`
    // is set on the BINDING itself (not returned by runToggleTask), so CM6
    // eats the browser's default for this key regardless of whether
    // toggleTask actually applies (runToggleTask still returns `false` on a
    // legitimate `null`, per the same "doesn't apply here" convention every
    // other command site uses — that only affects whether a LATER keymap
    // layer gets a chance at the key, never the browser). This is
    // deliberate: browsers reserve Cmd/Ctrl-L unconditionally for focusing
    // the location/address bar, so if we only prevented default when the
    // command actually produced a change, pressing Mod-L on a plain
    // paragraph (or any non-task line) would leak the keystroke straight to
    // the browser chrome instead of being silently consumed by the editor.
    { key: "Mod-l", run: runToggleTask, preventDefault: true },
  ]);
}

/**
 * The core-driven command keymap:
 * - Mod-b / Mod-i / Mod-Shift-x / Mod-e toggle strong/em/strike/code over
 *   the current selection;
 * - Enter continues/exits list markers and quote prefixes (falling through
 *   to the default newline when neither applies);
 * - Tab / Shift-Tab and Mod-] / Mod-[ run marker-width-aware
 *   indentList/outdentList (falling back to CM6's indentMore/indentLess
 *   outside list context);
 * - Mod-Shift-Enter / Mod-L both toggle the task checkbox on the cursor's
 *   line (Mod-Shift-Enter is the keyboard-accessible counterpart of the
 *   widget click; Mod-L matches Obsidian's default "Toggle checkbox status"
 *   hotkey — research/07 §1.6 — and unconditionally eats the browser's own
 *   Cmd/Ctrl-L location-bar shortcut, applicable or not).
 *
 * **toggleTask on a non-task target (v0.5, Obsidian parity):** Obsidian's
 * "Toggle checkbox status" run on a plain bullet CONVERTS it into a task
 * item (research/07 §1.6) rather than no-op'ing, and `toggle_task`
 * (crates/oxidown-core/src/commands.rs) now matches — see the module doc
 * comment's "## toggleTask" section there, and docs/boundary-v0.md's
 * `toggleTask` (v0.5 amendment) entry, for the exact promotion rules (a
 * non-task list item, a plain paragraph/blockquote line, or a blank line
 * all promote; headings/fences/hr stay `null`). Both bindings above need NO
 * changes for this — they already forward whatever `core.command("toggleTask",
 * pos)` returns, so a promotion CoreChange applies through the exact same
 * `applyCoreChange` path a flip does.
 *
 * Exported standalone so it can be composed elsewhere; included in
 * `oxidown()` by default.
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
 * - Binds Mod-z / Mod-y / Mod-Shift-z to CORE-DRIVEN undo/redo,
 *   Mod-b / Mod-i / Mod-Shift-x / Mod-e to CORE-DRIVEN formatting commands,
 *   Tab / Shift-Tab / Mod-] / Mod-[ to marker-width-aware list nesting,
 *   Enter to construct-aware list/quote continuation (single-press exit on
 *   empty items; plain paragraphs fall through to the default newline), and
 *   Mod-Shift-Enter / Mod-L to the task-checkbox toggle on the cursor's line
 *   (see oxidownCommands's doc comment for the non-task-item behavior).
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
    // which reflects the real text metrics at every position. Opt out via
    // `drawSelection: false` when the host composes its own (see
    // OxidownOptions).
    options.drawSelection !== false ? drawSelection() : [],
  ];
}
