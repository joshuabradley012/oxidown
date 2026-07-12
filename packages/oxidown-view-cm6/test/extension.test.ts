// @vitest-environment jsdom
//
// Integration smoke tests for the CM6 extension against MockCore, under jsdom.
// jsdom cannot do real layout, so these tests focus on the wiring: change
// forwarding, mirror consistency, history transactions not being echoed back,
// and decoration rebuild scheduling.

import { describe, expect, it, vi } from "vitest";
import { EditorState, type Transaction } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { defaultKeymap } from "@codemirror/commands";
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

describe("Enter keymap (construct-aware continue/exit with default-newline fallback)", () => {
  const enterKey = () =>
    new KeyboardEvent("keydown", { key: "Enter", code: "Enter", bubbles: true, cancelable: true });

  function makeViewWithDefaults(doc: string, core: MockCore) {
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

  it("falls back to the default newline in a plain paragraph", async () => {
    const core = new MockCore();
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

  it("continues a list item and exits the empty one — full round trip through CM6", async () => {
    const core = new MockCore();
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
    const core = new MockCore();
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

describe("FIX 1: TaskCheckboxWidget resolves its target from the DOM at click time", () => {
  function makeTaskView(doc: string) {
    const core = new MockCore();
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
    const core = new MockCore();
    const view = makeView("hello world", core);
    view.dispatch({ selection: { anchor: 0, head: 5 } });

    const loadSpy = vi.spyOn(core, "load");
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const cmdSpy = vi.spyOn(core, "command").mockImplementation(() => {
      throw new Error("boom");
    });

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
    cmdSpy.mockRestore();
    errSpy.mockRestore();
    view.destroy();
  });

  it("checkbox click: swallowed without re-loading the core", async () => {
    const core = new MockCore();
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
    const core = new MockCore();
    const doc = "- a\n- b\n";
    const view = makeView(doc, core);
    view.dispatch({ selection: { anchor: doc.indexOf("b") } });

    const loadSpy = vi.spyOn(core, "load");
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const cmdSpy = vi.spyOn(core, "command").mockImplementation(() => {
      throw new Error("boom");
    });

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
    cmdSpy.mockRestore();
    errSpy.mockRestore();
    view.destroy();
  });

  it("Enter (runEnter): swallowed WITHOUT falling back to the default newline", async () => {
    const core = new MockCore();
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
    const cmdSpy = vi.spyOn(core, "command").mockImplementation(() => {
      throw new Error("boom");
    });

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
    cmdSpy.mockRestore();
    errSpy.mockRestore();
    view.destroy();
  });
});

describe("FIX 6: skip-annotated dispatches are mirror-verified immediately", () => {
  it("detects and recovers when a host changeFilter alters a core-driven (skip-annotated) change", async () => {
    const core = new MockCore();
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

  it("does NOT re-check (or false-positive) on an ordinary, unaltered core-driven change", async () => {
    const core = new MockCore();
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

describe("widget DOM identity across unrelated edits", () => {
  it("keeps the SAME checkbox <input> node after typing above the task line", async () => {
    const core = new MockCore();
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
  function makeCapturingView(doc: string, core: MockCore) {
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
    const core = new MockCore();
    const { view, trs } = makeCapturingView("abcdef", core);
    view.dispatch({ changes: { from: 6, to: 6, insert: "XYZ" }, userEvent: "input.type" });
    // Park the cursor somewhere the undo's mapped position would NOT land,
    // so the selection placement is distinguishable from default mapping.
    view.dispatch({ selection: { anchor: 1 } });

    const change = core.undo(); // deletes "XYZ"; selection at the deletion site (6)
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

  it("a CoreChange WITHOUT a selection leaves the user's mapped cursor alone (no scrollIntoView)", async () => {
    const core = new MockCore();
    const doc = "- [ ] task\nelsewhere";
    const { view, trs } = makeCapturingView(doc, core);
    view.dispatch({ selection: { anchor: doc.length } });

    const change = core.command("toggleTask", 2); // selection: null
    expect(change).not.toBeNull();
    expect(change!.selection).toBeNull();
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
    const core = new MockCore();
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
    const core = new MockCore();
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
    const core = new MockCore();
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

describe("formatting keymap happy path (Mod-b / Mod-i / Mod-Shift-x / Mod-e)", () => {
  const cases: Array<[key: string, shift: boolean, delim: string]> = [
    ["b", false, "**"],
    ["i", false, "*"],
    ["x", true, "~~"],
    ["e", false, "`"],
  ];
  for (const [key, shift, delim] of cases) {
    it(`Mod-${shift ? "Shift-" : ""}${key} wraps the selection in ${delim}`, async () => {
      const core = new MockCore();
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
    const core = new MockCore();
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
