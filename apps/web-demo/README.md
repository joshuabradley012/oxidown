# Oxidown web demo (M0)

Full-page CodeMirror 6 editor wired to an Oxidown core through the boundary
protocol (`docs/boundary-v0.md`). By default it runs against the TypeScript
`MockCore`; add `?core=wasm` to the URL to try the real Rust/wasm core once
`crates/oxidown-wasm/pkg` has been built (it falls back to the mock with a
visible banner otherwise).

## Run

From the repo root:

```sh
pnpm install
pnpm dev          # builds @oxidown/view-cm6, then serves this app
```

Open the printed URL (default http://localhost:5173). `pnpm -r build` builds
everything; `pnpm -r test` runs the library tests.

## What to test manually

### Reveal / conceal behavior

- Click into `**bold text**`, `*italic*`, `` `inline code` ``, and the
  headings: the syntax delimiters should appear (dimmed) when the cursor or a
  selection touches the node — including its delimiters — and collapse again
  when the cursor leaves.
- Nesting is per-node: in `**bold with *italic* inside**`, placing the cursor
  in the bold-but-not-italic part reveals only the `**` pair; the inner `*`
  pair stays concealed until the cursor enters the italic span.
- Watch line heights: revealing/concealing must cause **no vertical layout
  shift** (delimiters are collapsed with a tiny font-size, never removed from
  the DOM).
- Drag-select across several formatted spans: while the mouse button is down,
  reveal state must not flicker; it recomputes once on mouse-up.

### Undo / redo (core-driven; CM6 history is disabled)

- Cmd/Ctrl-Z undoes, Cmd/Ctrl-Shift-Z or Ctrl-Y redoes.
- Typed runs coalesce (~500 ms + adjacency) into one undo unit; a paste is
  always its own unit; the cursor lands at the end of the undone/redone range.

### IME (macOS Japanese input)

1. Add a Japanese input source: System Settings → Keyboard → Input Sources →
   add "Japanese – Romaji" (Hiragana).
2. Switch to Hiragana (Ctrl-Space or the input menu) and click into the
   Japanese paragraph (「日本語の段落」).
3. Type e.g. `nihongo` — it appears as underlined *marked text*
   (にほんご), then press Space to convert (日本語) and Return to commit.
4. While the marked text is active, decorations must not rebuild (no flicker,
   and the composition must never be aborted mid-way). Try composing directly
   next to / inside `**太字**` — its delimiters stay revealed and stable for
   the whole session.
5. After committing, the whole composition should undo as one unit.

### Source mode & perf

- Toggle "Source mode": all decorations disappear (plain markdown), typing
  and undo keep working, and toggling back restores live preview without
  losing history.
- Click "Load large doc" to append ~200 filler paragraphs, then type and
  scroll: the perf HUD (top right) shows the last `applyEdit` and
  `decorations` timings measured from the JS side of the core boundary.
