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
import { oxidown } from "../src/extension";

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
