// @vitest-environment jsdom
//
// Integration smoke tests for the CM6 extension against MockCore, under jsdom.
// jsdom cannot do real layout, so these tests focus on the wiring: change
// forwarding, mirror consistency, history transactions not being echoed back,
// and decoration rebuild scheduling.

import { describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { MockCore } from "../src/mock-core";
import { applyCoreChange, oxidown } from "../src/extension";
import type { Decoration } from "../src/protocol";

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

function makeView(doc: string, core: MockCore) {
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

describe("oxidown extension wiring (jsdom)", () => {
  it("loads the view buffer into the core and forwards edits as splices", async () => {
    const core = new MockCore();
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
    await flush();
    view.destroy();
  });

  it("recovers from a core error by re-loading the mirror (and logs loudly)", async () => {
    const core = new MockCore();
    const view = makeView("abc", core);
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    // Desync the core behind the view's back: the next forwarded edit is based
    // on a revision/coordinates the core can't apply cleanly, or the mirror
    // check fails — either way the extension must re-load() from the view.
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
    const core = new MockCore();
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

  it("skips the re-render when a cursor-only move leaves the payload unchanged", async () => {
    const core = new MockCore();
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

  it("core-driven undo/redo dispatches are not echoed back into applyEdit", async () => {
    const core = new MockCore();
    const applySpy = vi.spyOn(core, "applyEdit");
    const view = makeView("", core);

    view.dispatch({ changes: { from: 0, to: 0, insert: "hello" }, userEvent: "input.type" });
    expect(core.getText()).toBe("hello");
    const applyCallsAfterTyping = applySpy.mock.calls.length;

    // Trigger the Mod-z binding by invoking core.undo + the same dispatch the
    // keymap performs is covered in the browser; here we verify the annotation
    // path: a transaction that the keymap would produce must be skipped.
    const result = core.undo();
    expect(result).not.toBeNull();
    // The keymap handler is what tags the transaction; simulate a plain
    // (untagged) dispatch and confirm it IS forwarded, proving the skip logic
    // depends on the annotation:
    view.dispatch({
      changes: result!.splices.map((s) => ({ from: s.at, to: s.at + s.delete, insert: s.insert })),
    });
    expect(applySpy.mock.calls.length).toBeGreaterThan(applyCallsAfterTyping);
    await flush();
    view.destroy();
  });

  it("keyboard undo/redo round-trips the document through the core", async () => {
    const core = new MockCore();
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

describe("Tab/Shift-Tab keymap (indentList/outdentList with indentMore/indentLess fallback)", () => {
  const tabKey = (shift = false) =>
    new KeyboardEvent("keydown", { key: "Tab", code: "Tab", shiftKey: shift, bubbles: true, cancelable: true });

  it("falls back to indentMore in a plain paragraph (not a list)", async () => {
    const core = new MockCore();
    const view = makeView("plain paragraph", core);
    view.dispatch({ selection: { anchor: 3 } });

    view.contentDOM.dispatchEvent(tabKey());
    await flush();

    // CM6's default indentMore behavior: 2 spaces at the line start.
    expect(view.state.doc.toString()).toBe("  plain paragraph");
    expect(core.getText()).toBe(view.state.doc.toString());
    view.destroy();
  });

  it("indents the whole item when the cursor is in the middle of the item's text", async () => {
    const core = new MockCore();
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

  it("Shift-Tab outdents via outdentList and reverses an indent", async () => {
    const core = new MockCore();
    const doc = "- a\n  - b\n";
    const view = makeView(doc, core);
    view.dispatch({ selection: { anchor: doc.indexOf("b") } });

    view.contentDOM.dispatchEvent(tabKey(true));
    await flush();

    expect(view.state.doc.toString()).toBe("- a\n- b\n");
    expect(core.getText()).toBe(view.state.doc.toString());
    view.destroy();
  });

  it("a no-movement no-op (first item of a list) does not fall back to indentMore", async () => {
    const core = new MockCore();
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

  it("indenting a non-1 ordered item applies the digit-rewrite batch cleanly through CM6", async () => {
    // The paragraph-interruption guard adds a digit-rewrite splice that
    // TOUCHES the indent splice (both anchored at the line start when the
    // item is at column 0) — this exercises that batch through a real CM6
    // dispatch, not just the mock's own string splicing.
    const core = new MockCore();
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

describe("source mode (decorations: false)", () => {
  function makeSourceView(doc: string, core: MockCore) {
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
    const core = new MockCore();
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
      const core = new MockCore();
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
      const core = new MockCore();
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
  it("applyCoreChange (commands/streaming) is not echoed back into applyEdit", async () => {
    const core = new MockCore();
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
    const core = new MockCore();
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

  it("task widget renders a checkbox; clicking it dispatches toggleTask via the CoreChange path", async () => {
    const core = new MockCore();
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

  it("ordered marker widget renders the computed number, replaced by raw digits when the line is revealed", async () => {
    // Contract v0.3 amendment (research/07 §0/§1.2): a concealed ordered
    // marker is a widget rendering the VIEW-COMPUTED number, never raw
    // source digits.
    const core = new MockCore();
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

  it("ordered marker widgets display the view-computed sequence, not raw digits", async () => {
    // "1./1./3." must DISPLAY 1,2,3 (research/07 §0: CommonMark only fixes
    // the list's start number; sibling digits are cosmetic).
    const core = new MockCore();
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

    // .trim() strips the widget's trailing NBSP (the required marker
    // whitespace, rendered as a non-collapsing space — see extension.ts):
    // the assertion cares about the displayed digits+delim, not that detail.
    const markerText = () =>
      Array.from(view.contentDOM.querySelectorAll(".ox-ordered-marker")).map(
        (el) => el.textContent?.trim(),
      );
    expect(markerText()).toEqual(["1.", "2.", "3."]);
    view.destroy();
  });

  it("an unknown decoration style/widget kind from the core is ignored without crashing", async () => {
    const core = new MockCore();
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

describe("hr rule suppression while editing", () => {
  it("swaps ox-hr for ox-hr-revealed when the cursor is on the hr line", async () => {
    const core = new MockCore();
    const view = makeView("before\n---\nafter", core);
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
    view.dispatch({ selection: { anchor: 8 } });
    await flush();
    expect(hrLine().classList.contains("ox-hr-revealed")).toBe(true);
    view.destroy();
  });
});
