// @vitest-environment jsdom
//
// Integration smoke tests for the CM6 extension under jsdom. jsdom cannot do
// real layout, so these tests focus on the wiring: change forwarding, mirror
// consistency, history transactions not being echoed back, and decoration
// rebuild scheduling.
//
// Two cores back these tests:
//  - StubCore (test/stub-core.ts): a deliberately dumb, scriptable double —
//    plain text buffer, snapshot undo, empty decorations, per-method fault
//    injection. Used wherever the test exercises VIEW wiring (splice
//    forwarding, skip annotations, recovery paths, keymap fallback, freeze
//    scheduling) and the core's markdown knowledge is irrelevant.
//  - the REAL wasm core (test/wasm-loader.ts, production adapter): used
//    wherever the test genuinely needs real decorations or real command
//    results (widgets, reveal-driven payload changes, surrogate refusals,
//    real toggles/undo interplay). The loader fails loudly if the pkg is
//    missing (`pnpm build:wasm`).

import { describe, expect, it, vi } from "vitest";
import { EditorState, Transaction } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { defaultKeymap, history, undoDepth } from "@codemirror/commands";
import { StubCore } from "./stub-core";
import { loadWasmCoreFactory } from "./wasm-loader";
import { applyCoreChange, oxidown, oxidownSkip, sanitizeSurrogates } from "../src/extension";
import type { Decoration, OxidownCore } from "../src/protocol";

const makeWasmCore = await loadWasmCoreFactory();

// jsdom implements Range but none of its layout methods. CM6's rAF-driven
// measure cycle (reached since drawSelection joined the extension bundle)
// calls Range.getClientRects mid-frame; without this stub the throw escapes
// as an UNHANDLED error in whatever test happens to be running when jsdom's
// animation-frame timer fires — a CI-only race (locally the process usually
// exits first) that fails the run even with all tests passing.
const emptyRect = { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 };
(Range.prototype as unknown as { getClientRects: () => unknown }).getClientRects = () =>
  Object.assign([], { item: () => null });
(Range.prototype as unknown as { getBoundingClientRect: () => unknown }).getBoundingClientRect =
  () => ({ ...emptyRect, toJSON: () => emptyRect });

function makeView(doc: string, core: OxidownCore) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      extensions: [oxidown(core, { verifyMirror: true })],
    }),
  });
  return view;
}

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe("oxidown extension wiring (jsdom, StubCore)", () => {
  it("loads the view buffer into the core and forwards edits as splices", async () => {
    const core = new StubCore();
    const view = makeView("hello **world**", core);
    expect(core.getText()).toBe("hello **world**");

    view.dispatch({ changes: { from: 5, to: 5, insert: "," } });
    expect(core.getText()).toBe("hello, **world**");
    expect(core.docLength()).toBe(view.state.doc.length);

    // multi-range transaction → ascending splices in one applyEdit
    view.dispatch({
      changes: [
        { from: 0, to: 1, insert: "H" },
        { from: 6, to: 7, insert: "!" },
      ],
    });
    expect(core.getText()).toBe(view.state.doc.toString());
    // The whole batch crossed as ONE applyEdit call with two splices.
    const lastApply = core.callsTo("applyEdit").at(-1)!;
    expect(lastApply.args[1]).toEqual([
      { at: 0, delete: 1, insert: "H" },
      { at: 6, delete: 1, insert: "!" },
    ]);
    await flush();
    view.destroy();
  });

  it("recovers from a core error by re-loading the mirror (and logs loudly)", async () => {
    const core = new StubCore();
    const view = makeView("abc", core);
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    // Desync the core behind the view's back: the next forwarded edit leaves
    // the mirror lengths disagreeing, so the verifyMirror check fails and the
    // extension must re-load() from the view.
    core.applyEdit(core.revision(), [{ at: 0, delete: 3, insert: "" }], "user");
    view.dispatch({ changes: { from: 3, to: 3, insert: "d" } });
    expect(core.getText()).toBe("abcd");
    expect(core.getText()).toBe(view.state.doc.toString());
    expect(errSpy).toHaveBeenCalled();
    errSpy.mockRestore();
    await flush();
    view.destroy();
  });

  it("rebuilds decorations at most once per microtask batch", async () => {
    const core = new StubCore();
    const spy = vi.spyOn(core, "decorations");
    const view = makeView("# Title\n\n**bold** text", core);
    const initialCalls = spy.mock.calls.length; // constructor build
    expect(initialCalls).toBeGreaterThan(0);

    // several dispatches in the same task...
    view.dispatch({ changes: { from: 22, to: 22, insert: "a" } });
    view.dispatch({ changes: { from: 23, to: 23, insert: "b" } });
    view.dispatch({ changes: { from: 24, to: 24, insert: "c" } });
    await flush();
    // ...coalesce into a single rebuild
    expect(spy.mock.calls.length).toBe(initialCalls + 1);
    view.destroy();
  });

  it("core-driven undo/redo dispatches are not echoed back into applyEdit", async () => {
    const core = new StubCore();
    const applySpy = vi.spyOn(core, "applyEdit");
    const view = makeView("", core);

    view.dispatch({ changes: { from: 0, to: 0, insert: "hello" }, userEvent: "input.type" });
    expect(core.getText()).toBe("hello");
    const applyCallsAfterTyping = applySpy.mock.calls.length;

    // The keymap handler is what tags the transaction; simulate a plain
    // (untagged) dispatch and confirm it IS forwarded, proving the skip logic
    // depends on the annotation:
    const result = core.undo();
    expect(result).not.toBeNull();
    view.dispatch({
      changes: result!.splices.map((s) => ({ from: s.at, to: s.at + s.delete, insert: s.insert })),
    });
    expect(applySpy.mock.calls.length).toBeGreaterThan(applyCallsAfterTyping);
    await flush();
    view.destroy();
  });

  it("keyboard undo/redo round-trips the document through the core", async () => {
    const core = new StubCore();
    const view = makeView("", core);
    view.dispatch({ changes: { from: 0, to: 0, insert: "abc" }, userEvent: "input.type" });
    expect(core.getText()).toBe("abc");
    const applySpy = vi.spyOn(core, "applyEdit");

    // Fire the actual keymap binding via a keydown on the content DOM.
    const undoKey = new KeyboardEvent("keydown", {
      key: "z",
      code: "KeyZ",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    view.contentDOM.dispatchEvent(undoKey);
    expect(view.state.doc.toString()).toBe("");
    expect(core.getText()).toBe("");

    const redoKey = new KeyboardEvent("keydown", {
      key: "y",
      code: "KeyY",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    view.contentDOM.dispatchEvent(redoKey);
    expect(view.state.doc.toString()).toBe("abc");
    expect(core.getText()).toBe("abc");
    // history transactions carry the private annotation: never echoed back
    expect(applySpy).not.toHaveBeenCalled();
    await flush();
    view.destroy();
  });
});

describe("reveal-driven payload cache (wasm core)", () => {
  it("skips the re-render when a cursor-only move leaves the payload unchanged", async () => {
    const core = makeWasmCore();
    const view = makeView("plain text here\n**bold** span line\n", core);
    await flush();
    type P = { decorations: unknown };
    const plugin = (view as unknown as { plugins: { value: unknown }[] }).plugins
      .map((p) => p.value)
      .filter((v): v is P => !!v && typeof v === "object" && "decorations" in v)[0];
    // settle any initial rebuild so we start from a cached payload
    view.dispatch({ selection: { anchor: 1 } });
    await flush();
    const before = plugin.decorations;

    // Caret moves WITHIN the plain first line: decorations cannot change —
    // the RangeSet must keep its identity (rebuild + dispatch skipped).
    view.dispatch({ selection: { anchor: 3 } });
    await flush();
    view.dispatch({ selection: { anchor: 9 } });
    await flush();
    expect(plugin.decorations).toBe(before);

    // Caret into the **bold** span on line 2: reveal flips, payload differs,
    // a real rebuild must happen.
    view.dispatch({ selection: { anchor: "plain text here\n".length + 3 } });
    await flush();
    expect(plugin.decorations).not.toBe(before);

    // After a DOC change, identical-looking payloads must not be trusted:
    // the rebuild runs again (payload cache invalidated).
    const afterBold = plugin.decorations;
    view.dispatch({ changes: { from: 6, to: 6, insert: "x" }, userEvent: "input.type" });
    await flush();
    expect(plugin.decorations).not.toBe(afterBold);
    view.destroy();
  });
});

describe("Tab/Shift-Tab keymap (indentList/outdentList with indentMore/indentLess fallback)", () => {
  const tabKey = (shift = false) =>
    new KeyboardEvent("keydown", { key: "Tab", code: "Tab", shiftKey: shift, bubbles: true, cancelable: true });

  it("falls back to indentMore in a plain paragraph (not a list) — StubCore returns null", async () => {
    // StubCore's command() always answers null ("doesn't apply"), which is
    // exactly the keymap's fallback path — no markdown knowledge needed.
    const core = new StubCore();
    const view = makeView("plain paragraph", core);
    view.dispatch({ selection: { anchor: 3 } });

    view.contentDOM.dispatchEvent(tabKey());
    await flush();

    // CM6's default indentMore behavior: 2 spaces at the line start.
    expect(view.state.doc.toString()).toBe("  plain paragraph");
    expect(core.getText()).toBe(view.state.doc.toString());
    view.destroy();
  });

  it("indents the whole item when the cursor is in the middle of the item's text (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "- a\n- b\n";
    const view = makeView(doc, core);
    // Cursor on the "b" character itself, not at the line/item start.
    view.dispatch({ selection: { anchor: doc.indexOf("b") } });

    view.contentDOM.dispatchEvent(tabKey());
    await flush();

    expect(view.state.doc.toString()).toBe("- a\n  - b\n");
    expect(core.getText()).toBe(view.state.doc.toString());
    view.destroy();
  });

  it("Shift-Tab outdents via outdentList and reverses an indent (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "- a\n  - b\n";
    const view = makeView(doc, core);
    view.dispatch({ selection: { anchor: doc.indexOf("b") } });

    view.contentDOM.dispatchEvent(tabKey(true));
    await flush();

    expect(view.state.doc.toString()).toBe("- a\n- b\n");
    expect(core.getText()).toBe(view.state.doc.toString());
    view.destroy();
  });

  it("a no-movement no-op (first item of a list) does not fall back to indentMore (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "- a\n- b\n";
    const view = makeView(doc, core);
    view.dispatch({ selection: { anchor: doc.indexOf("a") } });

    view.contentDOM.dispatchEvent(tabKey());
    await flush();

    // Applies (list context) but there's no target to nest under — must NOT
    // insert indentMore's fixed 2 spaces instead.
    expect(view.state.doc.toString()).toBe(doc);
    view.destroy();
  });

  it("indenting a non-1 ordered item applies the digit-rewrite batch cleanly through CM6 (wasm)", async () => {
    // The paragraph-interruption guard adds a digit-rewrite splice that
    // TOUCHES the indent splice (both anchored at the line start when the
    // item is at column 0) — this exercises that batch through a real CM6
    // dispatch.
    const core = makeWasmCore();
    const doc = "1. a\n2. b\n";
    const view = makeView(doc, core);
    view.dispatch({ selection: { anchor: doc.indexOf("b") } });

    view.contentDOM.dispatchEvent(tabKey());
    await flush();

    expect(view.state.doc.toString()).toBe("1. a\n   1. b\n");
    expect(core.getText()).toBe(view.state.doc.toString());
    view.destroy();
  });
});

describe("Enter keymap (construct-aware continue/exit with default-newline fallback)", () => {
  const enterKey = () =>
    new KeyboardEvent("keydown", { key: "Enter", code: "Enter", bubbles: true, cancelable: true });

  function makeViewWithDefaults(doc: string, core: OxidownCore) {
    // Mirror the documented host setup: oxidown() BEFORE defaultKeymap, so
    // the core-driven Enter wins where it applies and CM6's own
    // insertNewline* handles the null fallback.
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    return new EditorView({
      parent,
      state: EditorState.create({
        doc,
        extensions: [oxidown(core, { verifyMirror: true }), keymap.of(defaultKeymap)],
      }),
    });
  }

  it("falls back to the default newline in a plain paragraph (StubCore returns null)", async () => {
    const core = new StubCore();
    const view = makeViewWithDefaults("plain paragraph", core);
    view.dispatch({ selection: { anchor: 5 } });

    view.contentDOM.dispatchEvent(enterKey());
    await flush();

    // CM6's own insertNewlineAndIndent ran (it eats the whitespace after
    // the cursor while reindenting — its stock behavior): the core-driven
    // binding declined (null) and did NOT swallow the key.
    expect(view.state.doc.toString()).toBe("plain\nparagraph");
    expect(core.getText()).toBe(view.state.doc.toString());
    view.destroy();
  });

  it("continues a list item and exits the empty one — full round trip through CM6 (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "- buy milk\n";
    const view = makeViewWithDefaults(doc, core);
    view.dispatch({ selection: { anchor: doc.indexOf("milk") + 4 } });

    // Press 1: continue — new "- " item, cursor after its marker.
    view.contentDOM.dispatchEvent(enterKey());
    await flush();
    expect(view.state.doc.toString()).toBe("- buy milk\n- \n");
    expect(view.state.selection.main.head).toBe("- buy milk\n- ".length);
    expect(core.getText()).toBe(view.state.doc.toString());

    // Press 2: the item is empty — single-press exit clears the marker
    // (no newline inserted; no double-Enter quirk).
    view.contentDOM.dispatchEvent(enterKey());
    await flush();
    expect(view.state.doc.toString()).toBe("- buy milk\n\n");
    expect(core.getText()).toBe(view.state.doc.toString());
    view.destroy();
  });

  it("does not intercept Enter while an IME composition is active", async () => {
    const core = new StubCore();
    const commandSpy = vi.spyOn(core, "command");
    const view = makeViewWithDefaults("- item\n", core);
    view.dispatch({ selection: { anchor: 6 } });
    // Force the composing state (jsdom cannot run a real IME session; the
    // guard reads view.composing, so shadow the getter on the instance).
    Object.defineProperty(view, "composing", { get: () => true });

    view.contentDOM.dispatchEvent(enterKey());
    await flush();

    expect(commandSpy).not.toHaveBeenCalledWith("enter", expect.anything(), expect.anything());
    view.destroy();
  });
});

describe("source mode (decorations: false)", () => {
  function makeSourceView(doc: string, core: OxidownCore) {
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    return new EditorView({
      parent,
      state: EditorState.create({
        doc,
        extensions: [oxidown(core, { decorations: false, verifyMirror: true })],
      }),
    });
  }

  it("never builds decorations — not even via the mouseup rebuild path", async () => {
    const core = new StubCore();
    const spy = vi.spyOn(core, "decorations");
    const view = makeSourceView("# Title\n\n**bold** text", core);
    expect(spy).not.toHaveBeenCalled();

    // Regression: clicking/highlighting used to repaint the decorated view.
    // mousedown flows through CM6's event handlers; mouseup is window-level.
    view.contentDOM.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    window.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    await flush();
    view.dispatch({ changes: { from: 0, to: 0, insert: "x" } });
    await flush();
    expect(spy).not.toHaveBeenCalled();
    // and edits still sync to the core
    expect(core.getText()).toBe(view.state.doc.toString());
    view.destroy();
  });
});

describe("vertical-motion freeze (goal-column stability)", () => {
  it("suppresses rebuilds during an Arrow run and catches up after the trailing delay", async () => {
    vi.useFakeTimers();
    try {
      const core = new StubCore();
      const spy = vi.spyOn(core, "decorations");
      const view = makeView("line one\n\n**bold text is** the *thing*", core);
      await vi.runAllTimersAsync(); // settle constructor build
      const before = spy.mock.calls.length;

      // Simulate what the Prec.high keymap does before each vertical command,
      // then the selection change the command produces.
      type P = { noteVerticalMotion(): void };
      const plugins = (view as unknown as { plugins: { value: unknown }[] }).plugins
        .map((p) => p.value)
        .filter((v): v is P => !!v && typeof (v as P).noteVerticalMotion === "function");
      expect(plugins.length).toBe(1);
      const plugin = plugins[0];

      plugin.noteVerticalMotion();
      view.dispatch({ selection: { anchor: 9 } }); // "moved up"
      plugin.noteVerticalMotion();
      view.dispatch({ selection: { anchor: 14 } }); // "moved back down"
      await vi.advanceTimersByTimeAsync(0);
      // Frozen: no rebuild happened for either selection change.
      expect(spy.mock.calls.length).toBe(before);

      // Trailing timer (250ms) ends the run and performs ONE catch-up rebuild.
      await vi.advanceTimersByTimeAsync(300);
      expect(spy.mock.calls.length).toBe(before + 1);
      view.destroy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("typing ends the freeze immediately", async () => {
    vi.useFakeTimers();
    try {
      const core = new StubCore();
      const spy = vi.spyOn(core, "decorations");
      const view = makeView("**bold**", core);
      await vi.runAllTimersAsync();
      const before = spy.mock.calls.length;

      type P = { noteVerticalMotion(): void };
      const plugin = (view as unknown as { plugins: { value: unknown }[] }).plugins
        .map((p) => p.value)
        .find((v): v is P => !!v && typeof (v as P).noteVerticalMotion === "function")!;

      plugin.noteVerticalMotion();
      view.dispatch({ changes: { from: 8, to: 8, insert: "x" } });
      await vi.advanceTimersByTimeAsync(0);
      // docChanged ends the gesture and rebuilds without waiting 250ms.
      expect(spy.mock.calls.length).toBeGreaterThan(before);
      view.destroy();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("v0.2 additions (M1)", () => {
  it("applyCoreChange (commands/streaming) is not echoed back into applyEdit (wasm)", async () => {
    const core = makeWasmCore();
    const applySpy = vi.spyOn(core, "applyEdit");
    const view = makeView("hello world", core);
    const callsAfterSetup = applySpy.mock.calls.length;

    const change = core.command("toggleStrong", 6, 11);
    expect(change).not.toBeNull();
    applyCoreChange(view, change!, "oxidown.command");
    expect(view.state.doc.toString()).toBe("hello **world**");
    expect(core.getText()).toBe("hello **world**");
    // The change came from the core already — forwarding it back into
    // applyEdit would double-apply it and desync revisions.
    expect(applySpy.mock.calls.length).toBe(callsAfterSetup);

    // A plain (untagged) dispatch of an ordinary edit IS forwarded — proving
    // the skip logic depends on the annotation, not some other heuristic.
    view.dispatch({ changes: { from: 0, to: 0, insert: "X" } });
    expect(applySpy.mock.calls.length).toBeGreaterThan(callsAfterSetup);
    await flush();
    view.destroy();
  });

  it("streaming appends (applyCoreChange with no selection) never move the user's cursor", async () => {
    const core = new StubCore();
    const view = makeView("top\n\nbottom", core);
    // Park the cursor at the top of the document, as if the user were typing there.
    view.dispatch({ selection: { anchor: 0 } });

    const id = core.streamOpen(view.state.doc.length);
    const change = core.streamAppend(id, " more");
    expect(change.selection).toBeNull();
    applyCoreChange(view, change, "oxidown.stream");
    core.streamClose(id);

    expect(view.state.doc.toString()).toBe("top\n\nbottom more");
    // The stream's edit landed far below the cursor; the cursor must not move.
    expect(view.state.selection.main.anchor).toBe(0);
    await flush();
    view.destroy();
  });

  it("task widget renders a checkbox; clicking it dispatches toggleTask via the CoreChange path (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "- [ ] buy milk\nelsewhere";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        // Cursor on a DIFFERENT line (reveal is line-level) so the task
        // starts concealed (widget rendered) rather than revealed.
        selection: { anchor: doc.length },
        extensions: [oxidown(core, { verifyMirror: true })],
      }),
    });
    await flush();

    const checkbox = view.contentDOM.querySelector(
      "input.ox-task-checkbox",
    ) as HTMLInputElement | null;
    expect(checkbox).not.toBeNull();
    expect(checkbox!.checked).toBe(false);

    const applySpy = vi.spyOn(core, "applyEdit");
    checkbox!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await flush();

    expect(core.getText()).toBe("- [x] buy milk\nelsewhere");
    expect(view.state.doc.toString()).toBe("- [x] buy milk\nelsewhere");
    // The toggle went through core.command → CoreChange → applyCoreChange,
    // never through the ordinary applyEdit change-forwarding path.
    expect(applySpy).not.toHaveBeenCalled();

    const checkboxAfter = view.contentDOM.querySelector(
      "input.ox-task-checkbox",
    ) as HTMLInputElement | null;
    expect(checkboxAfter!.checked).toBe(true);
    view.destroy();
  });

  it("task checkbox prevents mousedown default (Chrome focus steal) and carries an aria-label (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "- [ ] buy milk\nelsewhere";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        selection: { anchor: doc.length },
        extensions: [oxidown(core, { verifyMirror: true })],
      }),
    });
    await flush();

    const checkbox = view.contentDOM.querySelector(
      "input.ox-task-checkbox",
    ) as HTMLInputElement | null;
    expect(checkbox).not.toBeNull();

    // Chrome focuses form controls on mousedown; the post-toggle rebuild
    // then replaces the focused input and focus falls to <body>, killing
    // typing. The widget must preventDefault the mousedown (standard CM6
    // checkbox-widget pattern) so focus never leaves the editor.
    const mousedown = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    checkbox!.dispatchEvent(mousedown);
    expect(mousedown.defaultPrevented).toBe(true);

    // Accessible name for screen readers; tabIndex stays -1 (Tab must stay
    // in the editor — it indents — so the widget is never a tab stop).
    expect(checkbox!.getAttribute("aria-label")).toBe("task checkbox");
    expect(checkbox!.tabIndex).toBe(-1);

    // The click path still toggles normally after the prevented mousedown.
    checkbox!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await flush();
    expect(core.getText()).toBe("- [x] buy milk\nelsewhere");
    view.destroy();
  });

  it("ordered marker widget renders the computed number, replaced by raw digits when the line is revealed (wasm)", async () => {
    // Contract v0.3 amendment (research/07 §0/§1.2): a concealed ordered
    // marker is a widget rendering the VIEW-COMPUTED number, never raw
    // source digits.
    const core = makeWasmCore();
    const doc = "1. one\n2. two\nelsewhere";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        // Cursor on a DIFFERENT line (reveal is line-level) so both markers
        // start concealed (widgets rendered) rather than revealed.
        selection: { anchor: doc.length },
        extensions: [oxidown(core, { verifyMirror: true })],
      }),
    });
    await flush();

    // .trim() strips the widget's trailing NBSP (the required marker
    // whitespace, rendered as a non-collapsing space — see extension.ts):
    // the assertion cares about the displayed digits+delim, not that detail.
    const markerText = () =>
      Array.from(view.contentDOM.querySelectorAll(".ox-ordered-marker")).map(
        (el) => el.textContent?.trim(),
      );
    expect(markerText()).toEqual(["1.", "2."]);

    // Move the cursor onto the FIRST item's line: its widget is replaced by
    // raw source digits (mark:list-marker, plain text in the DOM); the
    // second item's widget is untouched.
    view.dispatch({ selection: { anchor: 3 } });
    await flush();
    expect(markerText()).toEqual(["2."]);
    expect(view.contentDOM.textContent).toContain("1. one");
    view.destroy();
  });

  it("ordered marker widgets display the view-computed sequence, not raw digits (wasm)", async () => {
    // "1./1./3." must DISPLAY 1,2,3 (research/07 §0: CommonMark only fixes
    // the list's start number; sibling digits are cosmetic).
    const core = makeWasmCore();
    const doc = "1. a\n1. b\n3. c\nelsewhere";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        selection: { anchor: doc.length },
        extensions: [oxidown(core, { verifyMirror: true })],
      }),
    });
    await flush();

    const markerText = () =>
      Array.from(view.contentDOM.querySelectorAll(".ox-ordered-marker")).map(
        (el) => el.textContent?.trim(),
      );
    expect(markerText()).toEqual(["1.", "2.", "3."]);
    view.destroy();
  });

  it("an unknown decoration style/widget kind from the core is ignored without crashing", async () => {
    const core = new StubCore();
    const view = makeView("hello world", core);
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const fake: Decoration[] = [
      { kind: "mark", from: 0, to: 5, style: "future-style" as never },
      { kind: "line", at: 0, style: "future-line" as never },
      { kind: "widget", from: 0, to: 1, widget: "future-widget" as never, checked: false },
      { kind: "mark", from: 6, to: 11, style: "strong" }, // a known style alongside unknown ones
    ];
    const decoSpy = vi.spyOn(core, "decorations").mockReturnValue(fake);

    view.dispatch({ selection: { anchor: 6 } });
    await flush();
    expect(errSpy).not.toHaveBeenCalled();

    // The view keeps working normally afterward.
    decoSpy.mockRestore();
    view.dispatch({ changes: { from: 11, to: 11, insert: "!" } });
    expect(core.getText()).toBe("hello world!");

    errSpy.mockRestore();
    await flush();
    view.destroy();
  });
});

describe("hr rule suppression while editing (wasm)", () => {
  it("swaps ox-hr for ox-hr-revealed when the cursor is on the hr line", async () => {
    // Blank line before the dashes: "---" directly under a paragraph would
    // be its setext underline, not an hr (CommonMark).
    const core = makeWasmCore();
    const view = makeView("before\n\n---\nafter", core);
    await flush();
    // Concealment is a replace decoration: the raw `---` is NOT in the DOM
    // when concealed — find the line by its class instead of its text.
    const hrLine = () =>
      view.contentDOM.querySelector(".ox-hr") as HTMLElement;
    // Cursor away from the hr: rule class present, revealed class absent.
    view.dispatch({ selection: { anchor: 0 } });
    await flush();
    expect(hrLine().classList.contains("ox-hr")).toBe(true);
    expect(hrLine().classList.contains("ox-hr-revealed")).toBe(false);
    // Cursor on the hr line (inside "---"): revealed class appears.
    view.dispatch({ selection: { anchor: 9 } });
    await flush();
    expect(hrLine().classList.contains("ox-hr-revealed")).toBe(true);
    view.destroy();
  });
});

describe("FIX 1: TaskCheckboxWidget resolves its target from the DOM at click time (wasm)", () => {
  function makeTaskView(doc: string) {
    const core = makeWasmCore();
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        // Cursor away from the task line (reveal is line-level) so the task
        // starts concealed (widget rendered) rather than revealed.
        selection: { anchor: doc.length },
        extensions: [oxidown(core, { verifyMirror: true })],
      }),
    });
    return { core, view };
  }

  it("toggles the correct task after an edit ABOVE it, clicked before any decoration rebuild flushes", async () => {
    const doc = "line one\n- [ ] task\n";
    const { core, view } = makeTaskView(doc);
    await flush();

    const checkbox = view.contentDOM.querySelector(
      "input.ox-task-checkbox",
    ) as HTMLInputElement | null;
    expect(checkbox).not.toBeNull();

    // Insert text ABOVE the task line — shifts its position in the doc.
    // Deliberately do NOT await flush(): the decoration rebuild (which would
    // reconstruct the widget with an up-to-date constructor `pos`) is
    // microtask-deferred, so the SAME widget instance — carrying the `pos`
    // captured before this insertion — is still mounted; only RangeSet.map
    // has repositioned its range. A click right now must still resolve to
    // the CURRENT task position (via the DOM), not the stale one.
    view.dispatch({ changes: { from: 0, to: 0, insert: "more text\n" } });

    checkbox!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));

    expect(core.getText()).toBe("more text\nline one\n- [x] task\n");
    expect(view.state.doc.toString()).toBe(core.getText());
    await flush();
    view.destroy();
  });

  it("still resolves correctly mid-composition, when rebuilds are frozen", async () => {
    const doc = "line one\n- [ ] task\n";
    const { core, view } = makeTaskView(doc);
    await flush();

    const checkbox = view.contentDOM.querySelector(
      "input.ox-task-checkbox",
    ) as HTMLInputElement | null;
    expect(checkbox).not.toBeNull();

    // Shadow the composing state (jsdom cannot run a real IME session; same
    // technique as the "does not intercept Enter while composing" test
    // above) — rebuilds are frozen for the duration, exactly like the
    // anti-flicker rule requires.
    Object.defineProperty(view, "composing", { get: () => true });

    view.dispatch({ changes: { from: 0, to: 0, insert: "more text\n" } });
    // No flush: even if a rebuild were scheduled, `composing` freezes it.

    checkbox!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));

    expect(core.getText()).toBe("more text\nline one\n- [x] task\n");
    view.destroy();
  });
});

describe("FIX 4: a thrown command() is logged and swallowed, never a mirror-desync resync", () => {
  it("Mod-b (runToggle): swallowed without re-loading the core or touching the doc", async () => {
    const core = new StubCore();
    const view = makeView("hello world", core);
    view.dispatch({ selection: { anchor: 0, head: 5 } });

    const loadSpy = vi.spyOn(core, "load");
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    core.throwOnce("command", new Error("boom"));

    const boldKey = new KeyboardEvent("keydown", {
      key: "b",
      code: "KeyB",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    view.contentDOM.dispatchEvent(boldKey);
    await flush();

    expect(errSpy).toHaveBeenCalled();
    // command() is transactional (never mutates before throwing): a throw is
    // NOT a mirror-desync emergency, so no re-load.
    expect(loadSpy).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe("hello world");
    errSpy.mockRestore();
    view.destroy();
  });

  it("checkbox click: swallowed without re-loading the core (wasm renders the widget)", async () => {
    const core = makeWasmCore();
    const doc = "- [ ] buy milk\nelsewhere";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        selection: { anchor: doc.length },
        extensions: [oxidown(core, { verifyMirror: true })],
      }),
    });
    await flush();
    const checkbox = view.contentDOM.querySelector(
      "input.ox-task-checkbox",
    ) as HTMLInputElement | null;
    expect(checkbox).not.toBeNull();

    const loadSpy = vi.spyOn(core, "load");
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const cmdSpy = vi.spyOn(core, "command").mockImplementation(() => {
      throw new Error("boom");
    });

    checkbox!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await flush();

    expect(errSpy).toHaveBeenCalled();
    expect(loadSpy).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe(doc);
    cmdSpy.mockRestore();
    errSpy.mockRestore();
    view.destroy();
  });

  it("Tab (runIndent): swallowed WITHOUT falling back to indentMore (an error is not `null`)", async () => {
    const core = new StubCore();
    const doc = "- a\n- b\n";
    const view = makeView(doc, core);
    view.dispatch({ selection: { anchor: doc.indexOf("b") } });

    const loadSpy = vi.spyOn(core, "load");
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    core.throwOnce("command", new Error("boom"));

    const tabKey = new KeyboardEvent("keydown", {
      key: "Tab",
      code: "Tab",
      bubbles: true,
      cancelable: true,
    });
    view.contentDOM.dispatchEvent(tabKey);
    await flush();

    expect(errSpy).toHaveBeenCalled();
    expect(loadSpy).not.toHaveBeenCalled();
    // Must NOT have fallen back to indentMore's fixed 2-space indent: an
    // exception is handled-and-ignored, not "doesn't apply here".
    expect(view.state.doc.toString()).toBe(doc);
    errSpy.mockRestore();
    view.destroy();
  });

  it("Enter (runEnter): swallowed WITHOUT falling back to the default newline", async () => {
    const core = new StubCore();
    const doc = "- item\n";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        extensions: [oxidown(core, { verifyMirror: true }), keymap.of(defaultKeymap)],
      }),
    });
    view.dispatch({ selection: { anchor: 6 } });

    const loadSpy = vi.spyOn(core, "load");
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    core.throwOnce("command", new Error("boom"));

    const enterKey = new KeyboardEvent("keydown", {
      key: "Enter",
      code: "Enter",
      bubbles: true,
      cancelable: true,
    });
    view.contentDOM.dispatchEvent(enterKey);
    await flush();

    expect(errSpy).toHaveBeenCalled();
    expect(loadSpy).not.toHaveBeenCalled();
    // Must NOT have fallen back to a plain newline: an exception is
    // handled-and-ignored, not "no list/quote context here".
    expect(view.state.doc.toString()).toBe(doc);
    errSpy.mockRestore();
    view.destroy();
  });
});

describe("history keymap error doctrine (undo/redo wrapped like every other core call site)", () => {
  const modKey = (key: string) =>
    new KeyboardEvent("keydown", {
      key,
      code: `Key${key.toUpperCase()}`,
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });

  for (const [kind, key, method] of [
    ["undo", "z", "undo"],
    ["redo", "y", "redo"],
  ] as const) {
    it(`a thrown core.${kind}() is logged and recovered (mirror re-load), never an uncaught crash`, async () => {
      const core = new StubCore();
      const view = makeView("abc", core);
      view.dispatch({ changes: { from: 3, to: 3, insert: "d" }, userEvent: "input.type" });
      await flush();

      const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
      const loadSpy = vi.spyOn(core, "load");
      core.throwOnce(method, new Error("boom"));

      // Must not throw out of the keymap handler.
      expect(() => view.contentDOM.dispatchEvent(modKey(key))).not.toThrow();

      expect(errSpy).toHaveBeenCalled();
      // Unlike command() (transactional), undo/redo may have mutated the
      // core before throwing: desync emergency → re-load from the view.
      expect(loadSpy).toHaveBeenCalledWith(view.state.doc.toString());
      expect(core.getText()).toBe(view.state.doc.toString());

      errSpy.mockRestore();
      await flush();
      view.destroy();
    });
  }
});

describe("composition call sites are guarded (desync-emergency discipline)", () => {
  it("a throwing compositionBegin is logged and recovered via mirror re-load, never an uncaught crash", async () => {
    const core = new StubCore();
    const view = makeView("abc", core);
    await flush();

    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const loadSpy = vi.spyOn(core, "load");
    core.throwOnce("compositionBegin", new Error("boom"));

    expect(() =>
      view.contentDOM.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true })),
    ).not.toThrow();

    expect(errSpy).toHaveBeenCalled();
    // Unlike command() (transactional), compositionBegin may have partially
    // mutated core state: desync emergency → re-load from the view buffer.
    expect(loadSpy).toHaveBeenCalledWith(view.state.doc.toString());
    expect(core.getText()).toBe(view.state.doc.toString());

    errSpy.mockRestore();
    await flush();
    view.destroy();
  });

  it("a throwing compositionEnd still recovers AND schedules the catch-up rebuild", async () => {
    const core = new StubCore();
    const view = makeView("**bold** text", core);
    await flush();

    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const loadSpy = vi.spyOn(core, "load");
    const decoSpy = vi.spyOn(core, "decorations");
    core.throwOnce("compositionEnd", new Error("boom"));
    const before = decoSpy.mock.calls.length;

    expect(() =>
      view.contentDOM.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true })),
    ).not.toThrow();

    expect(errSpy).toHaveBeenCalled();
    expect(loadSpy).toHaveBeenCalledWith(view.state.doc.toString());
    // The throw must not skip the dirty flag / deferred flush: the catch-up
    // rebuild still runs once the composition has settled.
    await flush();
    expect(decoSpy.mock.calls.length).toBeGreaterThan(before);
    expect(core.getText()).toBe(view.state.doc.toString());

    errSpy.mockRestore();
    view.destroy();
  });
});

describe("Mod-Shift-Enter keymap (keyboard path for the task-checkbox toggle)", () => {
  const toggleKey = () =>
    new KeyboardEvent("keydown", {
      key: "Enter",
      code: "Enter",
      ctrlKey: true,
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });

  it("toggles the task on the cursor's line via core.command('toggleTask') (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "- [ ] buy milk\nelsewhere";
    const view = makeView(doc, core);
    // Cursor in the middle of the item's text — toggleTask resolves the
    // whole line, exactly like the checkbox click path.
    view.dispatch({ selection: { anchor: doc.indexOf("milk") } });
    const applySpy = vi.spyOn(core, "applyEdit");

    view.contentDOM.dispatchEvent(toggleKey());
    await flush();

    expect(view.state.doc.toString()).toBe("- [x] buy milk\nelsewhere");
    expect(core.getText()).toBe(view.state.doc.toString());
    // Same core-driven-change path as the widget click: command →
    // CoreChange → applyCoreChange, never echoed through applyEdit.
    expect(applySpy).not.toHaveBeenCalled();

    // And back: unchecking works from the same binding.
    view.contentDOM.dispatchEvent(toggleKey());
    await flush();
    expect(view.state.doc.toString()).toBe("- [ ] buy milk\nelsewhere");
    view.destroy();
  });

  it("does nothing on a non-task line (null falls through; no crash, no edit)", async () => {
    const core = new StubCore();
    const doc = "plain paragraph";
    const view = makeView(doc, core);
    view.dispatch({ selection: { anchor: 3 } });

    view.contentDOM.dispatchEvent(toggleKey());
    await flush();

    expect(view.state.doc.toString()).toBe(doc);
    expect(core.getText()).toBe(doc);
    view.destroy();
  });

  it("a thrown toggleTask is swallowed like every other command site (no resync, no edit)", async () => {
    const core = new StubCore();
    const doc = "- [ ] task\n";
    const view = makeView(doc, core);
    view.dispatch({ selection: { anchor: 2 } });

    const loadSpy = vi.spyOn(core, "load");
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    core.throwOnce("command", new Error("boom"));

    view.contentDOM.dispatchEvent(toggleKey());
    await flush();

    expect(errSpy).toHaveBeenCalled();
    expect(loadSpy).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe(doc);
    errSpy.mockRestore();
    view.destroy();
  });
});

describe("FIX 6: skip-annotated dispatches are mirror-verified immediately", () => {
  it("detects and recovers when a host changeFilter alters a core-driven (skip-annotated) change", async () => {
    const core = new StubCore();
    const initial = "0123456789";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: initial,
        extensions: [
          oxidown(core, { verifyMirror: true }),
          // Misbehaving host (contract violation: "hosts must not filter/
          // alter oxidown-annotated transactions"): silently drops PART of
          // any transaction's changes. Verified empirically against
          // @codemirror/state's ChangeSet.filter: with this range, a splice
          // at position 2 is dropped while one at position 9 survives —
          // `changeFilter` (unlike `transactionFilter`) preserves the
          // transaction's annotations, so the oxidownSkip tag survives too,
          // exactly like a real changeFilter would.
          EditorState.changeFilter.of(() => [0, 5]),
        ],
      }),
    });
    await flush();
    expect(core.getText()).toBe(initial);

    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const loadSpy = vi.spyOn(core, "load");

    // Simulate a core-driven change with two splices (standing in for
    // whatever real command/undo/stream produced a multi-splice batch) — the
    // core has ALREADY applied both, so we set its buffer directly.
    const fullyApplied = "01X2345678Y9"; // both splices applied
    core.load(fullyApplied);
    applyCoreChange(
      view,
      {
        revision: core.revision(),
        splices: [
          { at: 2, delete: 0, insert: "X" },
          { at: 9, delete: 0, insert: "Y" },
        ],
      },
      "oxidown.test",
    );

    // The changeFilter dropped the FIRST splice: the view's doc only got
    // "Y" ("012345678Y9", 11 chars), while the core has both ("01X...Y9",
    // 12 chars) — a length mismatch. Before FIX 6, nothing checks this until
    // the NEXT forwarded (non-skip) edit; now it's caught immediately.
    expect(view.state.doc.toString()).toBe("012345678Y9");
    expect(errSpy).toHaveBeenCalled();
    expect(loadSpy).toHaveBeenCalledWith(view.state.doc.toString());
    // Recovery: re-loading the core from the view buffer makes the mirror
    // agree again (on the view's surviving text).
    expect(core.getText()).toBe(view.state.doc.toString());
    errSpy.mockRestore();
    view.destroy();
  });

  it("does NOT false-positive when a batched update carries a skip-annotated transaction that isn't last (wasm)", async () => {
    // A host may deliver several transactions in ONE ViewUpdate
    // (view.update([...]) / a batching dispatch). Core-driven changes were
    // applied to the core BEFORE the update runs, so while iterating the
    // batch, core.docLength() is already the FINAL length — comparing it
    // against a NON-last skip-annotated transaction's intermediate newDoc
    // used to false-positive and wipe undo history/anchors via load().
    const core = makeWasmCore();
    const view = makeView("abc", core);
    await flush();

    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const loadSpy = vi.spyOn(core, "load");

    // Two core-driven changes, batched into one update. Both are already in
    // the core when the update runs (exactly like two applyCoreChange
    // dispatches a host batched together).
    const c1 = core.command("toggleStrong", 0, 3)!; // core: "**abc**"
    const tr1 = view.state.update({
      changes: c1.splices.map((s) => ({ from: s.at, to: s.at + s.delete, insert: s.insert })),
      annotations: oxidownSkip.of(true),
    });
    const c2 = core.command("setHeading", 0, 2)!; // core: "## **abc**"
    const tr2 = tr1.state.update({
      changes: c2.splices.map((s) => ({ from: s.at, to: s.at + s.delete, insert: s.insert })),
      annotations: oxidownSkip.of(true),
    });
    view.update([tr1, tr2]);

    expect(view.state.doc.toString()).toBe("## **abc**");
    expect(core.getText()).toBe("## **abc**");
    // No desync was reported and the core was never re-loaded...
    expect(errSpy).not.toHaveBeenCalled();
    expect(loadSpy).not.toHaveBeenCalled();
    // ...so the undo history survived intact (a false-positive load() wipes it).
    expect(core.undo()).not.toBeNull();
    expect(core.getText()).toBe("**abc**");
    expect(core.undo()).not.toBeNull();
    expect(core.getText()).toBe("abc");

    errSpy.mockRestore();
    // Destroy BEFORE yielding: the undo probes above (deliberately not
    // dispatched to the view) left core behind the view buffer, so the
    // pending decoration rebuild must not run against it.
    view.destroy();
    await flush();
  });

  it("does NOT re-check (or false-positive) on an ordinary, unaltered core-driven change (wasm)", async () => {
    const core = makeWasmCore();
    const view = makeView("hello world", core);
    await flush();

    const loadSpy = vi.spyOn(core, "load");
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    const change = core.command("toggleStrong", 6, 11);
    expect(change).not.toBeNull();
    applyCoreChange(view, change!, "oxidown.command");
    await flush();

    expect(view.state.doc.toString()).toBe("hello **world**");
    expect(core.getText()).toBe(view.state.doc.toString());
    expect(errSpy).not.toHaveBeenCalled();
    expect(loadSpy).not.toHaveBeenCalled();
    errSpy.mockRestore();
    view.destroy();
  });
});

describe("widget DOM identity across unrelated edits (wasm)", () => {
  it("keeps the SAME checkbox <input> node after typing above the task line", async () => {
    const core = makeWasmCore();
    const doc = "line one\n- [ ] task\n";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        // Cursor away from the task line (reveal is line-level) so the task
        // stays concealed (widget rendered) throughout.
        selection: { anchor: doc.length },
        extensions: [oxidown(core, { verifyMirror: true })],
      }),
    });
    await flush();
    const before = view.contentDOM.querySelector("input.ox-task-checkbox");
    expect(before).not.toBeNull();

    // Type on the FIRST line (shifts the widget's document position) and let
    // the deferred rebuild flush completely. The rebuilt widget compares
    // eq() to the mounted one — `checked` unchanged — so CM6 must reuse the
    // existing DOM node rather than destroy/recreate it (which would drop
    // hover state and swallow an in-flight click).
    view.dispatch({ changes: { from: 0, to: 0, insert: "x" }, userEvent: "input.type" });
    await flush();
    await flush();

    const after = view.contentDOM.querySelector("input.ox-task-checkbox");
    expect(after).toBe(before);
    view.destroy();
  });
});

describe("CoreChange selection placement", () => {
  function makeCapturingView(doc: string, core: OxidownCore) {
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const trs: Transaction[] = [];
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        extensions: [
          oxidown(core, { verifyMirror: true }),
          EditorView.updateListener.of((u) => trs.push(...u.transactions)),
        ],
      }),
    });
    return { view, trs };
  }

  it("a CoreChange WITH a selection moves the cursor there and requests scrollIntoView", async () => {
    const core = new StubCore();
    const { view, trs } = makeCapturingView("abcdef", core);
    view.dispatch({ changes: { from: 6, to: 6, insert: "XYZ" }, userEvent: "input.type" });
    // Park the cursor somewhere the undo's mapped position would NOT land,
    // so the selection placement is distinguishable from default mapping.
    view.dispatch({ selection: { anchor: 1 } });

    const change = core.undo(); // deletes "XYZ"; StubCore's selection lands at the splice end (6)
    expect(change).not.toBeNull();
    expect(change!.selection).not.toBeNull();
    trs.length = 0;
    applyCoreChange(view, change!, "undo");

    expect(view.state.doc.toString()).toBe("abcdef");
    expect(view.state.selection.main.anchor).toBe(change!.selection!.anchor);
    expect(view.state.selection.main.head).toBe(change!.selection!.head);
    expect(view.state.selection.main.anchor).not.toBe(1); // not the parked cursor
    expect(trs.length).toBe(1);
    expect(trs[0].scrollIntoView).toBe(true);
    await flush();
    view.destroy();
  });

  it("a CoreChange WITHOUT a selection leaves the user's mapped cursor alone (no scrollIntoView) (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "- [ ] task\nelsewhere";
    const { view, trs } = makeCapturingView(doc, core);
    view.dispatch({ selection: { anchor: doc.length } });

    const change = core.command("toggleTask", 2); // no selection on the wire
    expect(change).not.toBeNull();
    expect(change!.selection ?? null).toBeNull();
    trs.length = 0;
    applyCoreChange(view, change!, "oxidown.command");

    expect(view.state.doc.toString()).toBe("- [x] task\nelsewhere");
    // Same-length replacement above the cursor: the mapped position is the
    // original one — the change must not have moved it.
    expect(view.state.selection.main.anchor).toBe(doc.length);
    expect(trs.length).toBe(1);
    expect(trs[0].scrollIntoView).toBe(false);
    await flush();
    view.destroy();
  });
});

describe("streaming cursor preservation (AT / AFTER the insertion point, interleaved typing)", () => {
  it("cursor AT the insertion point does not ride the appended text", async () => {
    const core = new StubCore();
    const view = makeView("top\nbottom", core);
    const end = view.state.doc.length;
    view.dispatch({ selection: { anchor: end } });

    const id = core.streamOpen(end);
    applyCoreChange(view, core.streamAppend(id, " more"), "oxidown.stream");
    core.streamClose(id);

    expect(view.state.doc.toString()).toBe("top\nbottom more");
    // CM6's default mapping keeps an empty selection BEFORE text inserted
    // exactly at it: the cursor must not be dragged to the chunk's end.
    expect(view.state.selection.main.anchor).toBe(end);
    await flush();
    view.destroy();
  });

  it("cursor AFTER the insertion point shifts by exactly the chunk length", async () => {
    const core = new StubCore();
    const view = makeView("start\nend", core);
    const cursor = "start\nen".length; // inside "end", after the stream point
    view.dispatch({ selection: { anchor: cursor } });

    const id = core.streamOpen("start".length);
    applyCoreChange(view, core.streamAppend(id, "XYZ"), "oxidown.stream");
    core.streamClose(id);

    expect(view.state.doc.toString()).toBe("startXYZ\nend");
    expect(view.state.selection.main.anchor).toBe(cursor + "XYZ".length);
    await flush();
    view.destroy();
  });

  it("user typing during the stream interleaves: both edits land, cursor stays with the user", async () => {
    const core = new StubCore();
    const view = makeView("top\nbottom", core);
    view.dispatch({ selection: { anchor: 3 } }); // end of "top"

    const id = core.streamOpen(view.state.doc.length);
    applyCoreChange(view, core.streamAppend(id, " one"), "oxidown.stream");

    // The user keeps typing at THEIR cursor while the stream is open — an
    // ordinary forwarded edit (applyEdit) that the stream's anchor must map
    // through. Real keyboard input places the cursor after the inserted
    // character explicitly, so this dispatch does too.
    view.dispatch({
      changes: { from: 3, to: 3, insert: "!" },
      selection: { anchor: 4 },
      userEvent: "input.type",
    });
    expect(core.getText()).toBe("top!\nbottom one");

    applyCoreChange(view, core.streamAppend(id, " two"), "oxidown.stream");
    core.streamClose(id);

    expect(view.state.doc.toString()).toBe("top!\nbottom one two");
    expect(core.getText()).toBe(view.state.doc.toString());
    // The cursor sits after the "!" the user typed, untouched by the stream.
    expect(view.state.selection.main.anchor).toBe(4);
    await flush();
    view.destroy();
  });
});

describe("formatting keymap happy path (Mod-b / Mod-i / Mod-Shift-x / Mod-e) (wasm)", () => {
  const cases: Array<[key: string, shift: boolean, delim: string]> = [
    ["b", false, "**"],
    ["i", false, "*"],
    ["x", true, "~~"],
    ["e", false, "`"],
  ];
  for (const [key, shift, delim] of cases) {
    it(`Mod-${shift ? "Shift-" : ""}${key} wraps the selection in ${delim}`, async () => {
      const core = makeWasmCore();
      const view = makeView("hello world", core);
      view.dispatch({ selection: { anchor: 6, head: 11 } });

      view.contentDOM.dispatchEvent(
        new KeyboardEvent("keydown", {
          key,
          code: `Key${key.toUpperCase()}`,
          ctrlKey: true,
          shiftKey: shift,
          bubbles: true,
          cancelable: true,
        }),
      );
      await flush();

      expect(view.state.doc.toString()).toBe(`hello ${delim}world${delim}`);
      expect(core.getText()).toBe(view.state.doc.toString());
      // The CoreChange's selection keeps the (shifted) content selected.
      expect(view.state.selection.main.anchor).toBe(6 + delim.length);
      expect(view.state.selection.main.head).toBe(11 + delim.length);
      view.destroy();
    });
  }
});

describe("drag freeze released by dragend (native drag-and-drop, no mouseup)", () => {
  it("defers the rebuild during the drag and flushes it on dragend", async () => {
    const core = new StubCore();
    const spy = vi.spyOn(core, "decorations");
    const view = makeView("hello **world**", core);
    await flush(); // settle the constructor build + any initial rebuild
    const before = spy.mock.calls.length;

    // Dragging an existing selection: mousedown freezes rebuilds...
    view.contentDOM.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));

    // ...a core change arrives mid-drag (e.g. a streamed append)...
    const id = core.streamOpen(view.state.doc.length);
    applyCoreChange(view, core.streamAppend(id, " x"), "oxidown.stream");
    core.streamClose(id);
    await flush();
    expect(spy.mock.calls.length).toBe(before); // frozen: rebuild deferred

    // ...and the native HTML5 drag ends with dragend — mouseup NEVER fires.
    window.dispatchEvent(new Event("dragend"));
    await flush();
    expect(spy.mock.calls.length).toBe(before + 1); // deferred rebuild flushed
    view.destroy();
  });
});

describe("S7: surrogate-safe desync recovery", () => {
  it("sanitizeSurrogates replaces lone surrogates with U+FFFD and keeps valid pairs", () => {
    expect(sanitizeSurrogates("plain ascii")).toBe("plain ascii");
    expect(sanitizeSurrogates("pair: \u{1F600}!")).toBe("pair: \u{1F600}!");
    expect(sanitizeSurrogates("a\uD800b")).toBe("a�b"); // lone high, mid-string
    expect(sanitizeSurrogates("a\uDC00b")).toBe("a�b"); // lone low, mid-string
    expect(sanitizeSurrogates("ab\uD800")).toBe("ab�"); // lone high at the end
    expect(sanitizeSurrogates("\uDC00ab")).toBe("�ab"); // lone low at the start
    // A high directly before a valid pair is itself lone.
    expect(sanitizeSurrogates("\uD800😀")).toBe("�\u{1F600}");
    // Sanitization is 1:1 per code unit: lengths never change.
    expect(sanitizeSurrogates("x𐀀\uDC00y").length).toBe(5);
  });

  it("recovers from a dispatched lone-surrogate insertion: no crash, core and view converge on U+FFFD (wasm)", async () => {
    // The real adapter refuses the unpaired surrogate (InvalidPayload); the
    // recovery path must sanitize the buffer before the reload.
    const core = makeWasmCore();
    const view = makeView("abc", core);
    await flush();
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    // The forwarded applyEdit refuses the unpaired surrogate (InvalidPayload);
    // the old recovery then called core.load(raw view buffer), which threw the
    // SAME refusal straight out of the catch block — an uncaught crash with no
    // recovery path. Now the buffer is sanitized before the reload.
    expect(() =>
      view.dispatch({ changes: { from: 3, to: 3, insert: "\uD800" }, userEvent: "input.type" }),
    ).not.toThrow();

    // The core loaded the sanitized text synchronously...
    expect(core.getText()).toBe("abc�");
    // ...and the deferred (microtask) repair dispatch converges the view
    // document on the same U+FFFD text — mirror equal, no crash.
    await flush();
    expect(view.state.doc.toString()).toBe("abc�");
    expect(core.getText()).toBe(view.state.doc.toString());
    expect(errSpy).toHaveBeenCalled(); // still a loudly-logged desync emergency

    // The editor keeps working normally afterwards.
    view.dispatch({ changes: { from: 0, to: 0, insert: "x" }, userEvent: "input.type" });
    expect(core.getText()).toBe("xabc�");
    expect(core.getText()).toBe(view.state.doc.toString());
    errSpy.mockRestore();
    await flush();
    view.destroy();
  });

  it("the repair transaction is skip-annotated and outside CM6 history (wasm)", async () => {
    const core = makeWasmCore();
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const trs: Transaction[] = [];
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: "abc",
        extensions: [
          oxidown(core, { verifyMirror: true }),
          EditorView.updateListener.of((u) => trs.push(...u.transactions)),
        ],
      }),
    });
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const applySpy = vi.spyOn(core, "applyEdit");

    view.dispatch({ changes: { from: 3, to: 3, insert: "\uDC00" }, userEvent: "input.type" });
    const applyCallsAfterFailure = applySpy.mock.calls.length; // the failed forward
    trs.length = 0;
    await flush();

    // Exactly one repair transaction, skip-annotated (never echoed back into
    // applyEdit) and tagged addToHistory: false.
    const repairs = trs.filter((t) => t.docChanged);
    expect(repairs.length).toBe(1);
    expect(repairs[0].annotation(oxidownSkip)).toBe(true);
    expect(repairs[0].annotation(Transaction.addToHistory)).toBe(false);
    expect(applySpy.mock.calls.length).toBe(applyCallsAfterFailure);
    expect(view.state.doc.toString()).toBe("abc�");
    expect(core.getText()).toBe("abc�");
    errSpy.mockRestore();
    view.destroy();
  });
});

describe("S8: async language-load repaint (full pipeline) (wasm)", () => {
  it("paints tok-* marks once the lazily-loaded language resolves, with NO other events", async () => {
    const core = makeWasmCore();
    // A fenced block whose language ("js") loads asynchronously on first use.
    const doc = "```js\nconst x = 1; // note\n```\n";
    const view = makeView(doc, core);

    type P = { decorations: { iter(): { value: unknown; next(): void } } };
    const plugin = (view as unknown as { plugins: { value: unknown }[] }).plugins
      .map((p) => p.value)
      .filter((v): v is P => !!v && typeof v === "object" && "decorations" in v)[0];
    expect(plugin).toBeTruthy();
    const hasTok = () => {
      const iter = plugin.decorations.iter();
      while (iter.value) {
        const spec = (iter.value as { spec?: { class?: string } }).spec;
        if (spec?.class && /(^|\s)tok-/.test(spec.class)) return true;
        iter.next();
      }
      return false;
    };

    // Deliberately NO dispatches from here on: the load resolving must be
    // enough. Its onLoad callback invalidates the payload cache before
    // scheduling the rebuild — the CORE payload (and core.revision()) are
    // unchanged, so without that invalidation flushRebuild's
    // identical-payload skip would swallow the repaint forever.
    for (let i = 0; i < 250 && !hasTok(); i++) {
      await new Promise((r) => setTimeout(r, 20));
    }
    expect(hasTok()).toBe(true);
    view.destroy();
  });
});

describe("S9: readOnly editors never dispatch core edits", () => {
  function makeReadOnlyView(doc: string, core: OxidownCore, anchor?: number) {
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    return new EditorView({
      parent,
      state: EditorState.create({
        doc,
        selection: anchor !== undefined ? { anchor } : undefined,
        extensions: [
          oxidown(core, { verifyMirror: true }),
          keymap.of(defaultKeymap),
          EditorState.readOnly.of(true),
        ],
      }),
    });
  }

  const key = (init: KeyboardEventInit) =>
    new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });

  it("formatting toggles (Mod-b) return false without calling core.command", async () => {
    const core = new StubCore();
    const doc = "hello world";
    const view = makeReadOnlyView(doc, core);
    view.dispatch({ selection: { anchor: 6, head: 11 } }); // selection changes are not edits
    const cmdSpy = vi.spyOn(core, "command");

    view.contentDOM.dispatchEvent(key({ key: "b", code: "KeyB", ctrlKey: true }));
    await flush();

    expect(cmdSpy).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe(doc);
    expect(core.getText()).toBe(doc);
    view.destroy();
  });

  it("Tab/Shift-Tab neither run indentList/outdentList nor fall back to indentMore/indentLess", async () => {
    const core = new StubCore();
    const doc = "- a\n- b\n";
    const view = makeReadOnlyView(doc, core, doc.indexOf("b"));
    const cmdSpy = vi.spyOn(core, "command");

    view.contentDOM.dispatchEvent(key({ key: "Tab", code: "Tab" }));
    view.contentDOM.dispatchEvent(key({ key: "Tab", code: "Tab", shiftKey: true }));
    await flush();

    expect(cmdSpy).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe(doc);
    view.destroy();
  });

  it("Enter and Mod-Shift-Enter return false without dispatching", async () => {
    const core = new StubCore();
    const doc = "- [ ] task\n";
    const view = makeReadOnlyView(doc, core, 6);
    const cmdSpy = vi.spyOn(core, "command");

    view.contentDOM.dispatchEvent(key({ key: "Enter", code: "Enter" }));
    view.contentDOM.dispatchEvent(
      key({ key: "Enter", code: "Enter", ctrlKey: true, shiftKey: true }),
    );
    await flush();

    expect(cmdSpy).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe(doc);
    view.destroy();
  });

  it("undo/redo keys never touch the core's history stacks", async () => {
    const core = new StubCore();
    const doc = "abc";
    const view = makeReadOnlyView(doc, core);
    const undoSpy = vi.spyOn(core, "undo");
    const redoSpy = vi.spyOn(core, "redo");

    view.contentDOM.dispatchEvent(key({ key: "z", code: "KeyZ", ctrlKey: true }));
    view.contentDOM.dispatchEvent(key({ key: "y", code: "KeyY", ctrlKey: true }));
    view.contentDOM.dispatchEvent(key({ key: "z", code: "KeyZ", ctrlKey: true, shiftKey: true }));
    await flush();

    expect(undoSpy).not.toHaveBeenCalled();
    expect(redoSpy).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe(doc);
    view.destroy();
  });

  it("checkbox clicks are ignored (no toggleTask dispatch, no edit) (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "- [ ] buy milk\nelsewhere";
    // Cursor on a different line so the widget is rendered (reveal is line-level).
    const view = makeReadOnlyView(doc, core, doc.length);
    await flush();

    const checkbox = view.contentDOM.querySelector(
      "input.ox-task-checkbox",
    ) as HTMLInputElement | null;
    expect(checkbox).not.toBeNull();
    const cmdSpy = vi.spyOn(core, "command");

    checkbox!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await flush();

    expect(cmdSpy).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe(doc);
    expect(core.getText()).toBe(doc);
    expect(checkbox!.checked).toBe(false); // the DOM checkbox didn't lie either
    view.destroy();
  });
});

describe("S10: validation refusals are logged quietly (no console.error)", () => {
  const boldKey = () =>
    new KeyboardEvent("keydown", {
      key: "b",
      code: "KeyB",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });

  it("multi-block Mod-b (a contract validation refusal) produces no console.error (wasm)", async () => {
    const core = makeWasmCore();
    const doc = "para one\n\npara two";
    const view = makeView(doc, core);
    // Selection spanning two paragraphs: the core refuses the toggle with an
    // Invalid* validation error (thrown before any mutation) by contract.
    view.dispatch({ selection: { anchor: 0, head: doc.length } });

    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const dbgSpy = vi.spyOn(console, "debug").mockImplementation(() => {});

    view.contentDOM.dispatchEvent(boldKey());
    await flush();

    expect(errSpy).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe(doc); // refused: no edit
    expect(core.getText()).toBe(doc);
    errSpy.mockRestore();
    dbgSpy.mockRestore();
    view.destroy();
  });

  it("routes on the Invalid* prefix: InvalidArgument goes to console.debug, other names stay loud", async () => {
    const core = new StubCore();
    const view = makeView("hello world", core);
    view.dispatch({ selection: { anchor: 0, head: 5 } });

    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const dbgSpy = vi.spyOn(console, "debug").mockImplementation(() => {});

    // A refusal spelled with the core's own guard name (message prefix, no
    // CoreErrorName type needed): quiet.
    core.throwOnce(
      "command",
      new Error("InvalidArgument: from must be a non-negative integer, got -1"),
    );
    view.contentDOM.dispatchEvent(boldKey());
    expect(errSpy).not.toHaveBeenCalled();
    expect(dbgSpy).toHaveBeenCalled();

    // Any non-Invalid* name keeps the existing loud doctrine.
    core.throwOnce("command", new Error("UnknownStream: boom"));
    view.contentDOM.dispatchEvent(boldKey());
    expect(errSpy).toHaveBeenCalled();

    errSpy.mockRestore();
    dbgSpy.mockRestore();
    await flush();
    view.destroy();
  });
});

describe("S11: a mixed skip + user batched update routes through desync recovery (wasm)", () => {
  it("does not forward user splices computed against the wrong core doc", async () => {
    const core = makeWasmCore();
    const view = makeView("abc", core);
    await flush();

    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const loadSpy = vi.spyOn(core, "load");
    const applySpy = vi.spyOn(core, "applyEdit");

    // A host batches (view.update([...])) a plain USER transaction — built
    // against the view's doc "abc" — together with a core-driven change the
    // core has ALREADY applied (command() mutates the core when called).
    // Forwarding the user splice at position 3 against the core's
    // now-different doc "**abc**" would land it INSIDE the delimiters
    // ("**axbc**"): silent corruption when verifyMirror is off.
    const tr1 = view.state.update({
      changes: { from: 3, to: 3, insert: "x" },
      userEvent: "input.type",
    });
    const c = core.command("toggleStrong", 0, 3)!; // core: "abc" -> "**abc**"
    const tr2 = tr1.state.update({
      changes: c.splices.map((s) => ({ from: s.at, to: s.at + s.delete, insert: s.insert })),
      annotations: oxidownSkip.of(true),
    });
    view.update([tr1, tr2]);

    // The user splice was never forwarded splice-by-splice...
    expect(applySpy).not.toHaveBeenCalled();
    // ...the whole batch was treated as a desync emergency instead: one
    // recovery reload against the update's FINAL doc.
    expect(view.state.doc.toString()).toBe("**abc**x");
    expect(loadSpy).toHaveBeenCalledWith("**abc**x");
    expect(core.getText()).toBe(view.state.doc.toString());
    expect(errSpy).toHaveBeenCalled(); // loudly reported

    errSpy.mockRestore();
    await flush();
    view.destroy();
  });

  it("(control) an all-skip batched update still forwards nothing and loads nothing", async () => {
    // The mixed-case detector must not regress the legitimate all-skip batch
    // (already covered in FIX 6, re-asserted here against the new pre-scan).
    const core = makeWasmCore();
    const view = makeView("abc", core);
    await flush();

    const loadSpy = vi.spyOn(core, "load");
    const applySpy = vi.spyOn(core, "applyEdit");

    const c1 = core.command("toggleStrong", 0, 3)!;
    const tr1 = view.state.update({
      changes: c1.splices.map((s) => ({ from: s.at, to: s.at + s.delete, insert: s.insert })),
      annotations: oxidownSkip.of(true),
    });
    const c2 = core.command("setHeading", 0, 2)!;
    const tr2 = tr1.state.update({
      changes: c2.splices.map((s) => ({ from: s.at, to: s.at + s.delete, insert: s.insert })),
      annotations: oxidownSkip.of(true),
    });
    view.update([tr1, tr2]);

    expect(view.state.doc.toString()).toBe("## **abc**");
    expect(core.getText()).toBe("## **abc**");
    expect(applySpy).not.toHaveBeenCalled();
    expect(loadSpy).not.toHaveBeenCalled();
    view.destroy();
    await flush();
  });
});

describe("S12: drawSelection opt-out + core-change history tagging", () => {
  it("bundles drawSelection by default; `drawSelection: false` omits it", () => {
    const core1 = new StubCore();
    const view1 = makeView("abc", core1);
    expect(view1.dom.querySelector(".cm-cursorLayer")).not.toBeNull();
    view1.destroy();

    const core2 = new StubCore();
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view2 = new EditorView({
      parent,
      state: EditorState.create({
        doc: "abc",
        extensions: [oxidown(core2, { verifyMirror: true, drawSelection: false })],
      }),
    });
    expect(view2.dom.querySelector(".cm-cursorLayer")).toBeNull();
    view2.destroy();
  });

  it("applyCoreChange transactions carry addToHistory: false — a wrongly-enabled CM6 history records nothing (wasm)", async () => {
    const core = makeWasmCore();
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const trs: Transaction[] = [];
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: "hello world",
        extensions: [
          oxidown(core, { verifyMirror: true }),
          // The host mistake the tagging defends against: CM6's own history
          // alongside the core historian (explicitly documented as wrong).
          history(),
          EditorView.updateListener.of((u) => trs.push(...u.transactions)),
        ],
      }),
    });

    const change = core.command("toggleStrong", 6, 11);
    expect(change).not.toBeNull();
    trs.length = 0;
    applyCoreChange(view, change!, "oxidown.command");

    expect(view.state.doc.toString()).toBe("hello **world**");
    const docTrs = trs.filter((t) => t.docChanged);
    expect(docTrs.length).toBe(1);
    expect(docTrs[0].annotation(Transaction.addToHistory)).toBe(false);
    // The second history recorded nothing to undo.
    expect(undoDepth(view.state)).toBe(0);
    await flush();
    view.destroy();
  });
});
