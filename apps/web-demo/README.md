# Oxidown web demo (M0 + M1)

Full-page CodeMirror 6 editor wired to the Oxidown Rust/wasm core through the
boundary protocol (`docs/boundary-v0.md`, v0 + the v0.2/M1 additions). The
wasm core is the only core — the TypeScript `MockCore` that older revisions
fell back to is retired — so `crates/oxidown-wasm/pkg` must be built before
running the demo; if it's missing or fails to load, the page shows a clear
error instead of an editor (no silent fallback). A stale `?core=wasm` query
param in old bookmarks is harmless and ignored.

## Run

From the repo root:

```sh
pnpm install
pnpm build:wasm   # builds crates/oxidown-wasm → pkg (requires wasm-pack)
pnpm dev          # builds @oxidown/view-cm6, then serves this app
```

Open the printed URL (default http://localhost:5173). `pnpm -r build` builds
everything; `pnpm -r test` runs the library tests (they also need the wasm
pkg — the behavior suites run against the real core and fail loudly without
it).

## What to test manually

### Streaming — the thing to try first

Click **"Stream AI text"**: a hardcoded ~55-line markdown answer (headings,
bold/italic, a fenced code block, a task list, a blockquote) is delivered
through `core.streamOpen` / `streamAppend` / `streamClose` in randomly-sized
chunks (2–20 chars) at randomized delays (15–40ms) — deliberately misaligned
with token or markdown boundaries, so you'll see things like an unterminated
` ``` ` fence or a half-typed `**bold` render honestly for a moment before the
next chunk completes them.

**While it's streaming, click into the top of the document and keep
typing.** Your own edits are never interrupted, never coalesced with the
stream's undo unit, and the stream keeps appending exactly where it left off
underneath your cursor — the core maps the stream's insertion point through
your edits, and the view never moves your selection to follow the stream
(only explicit core-driven changes like undo/redo/commands do that). The
stream status + chunk rate are shown in the header; "Stop" closes the stream
early (whatever streamed so far stays, as one undo unit — Cmd/Ctrl-Z removes
it all in one step).

### Formatting commands

Select some text and press **Mod-B** (bold), **Mod-I** (italic),
**Mod-Shift-X** (strikethrough), or **Mod-E** (inline code) to toggle
delimiters via `core.command(...)` instead of typing them by hand. For
canonical delimiter flavors (`**`, `*`, `~~`, matching backtick runs),
toggling twice returns the exact original bytes (round-trip tested in the
library suite); non-canonical flavors normalize on the way back — e.g.
`__x__` deliberately re-wraps as `**x**` (see `crates/oxidown-core/README.md`).

### Task lists

Type a GFM task item (`- [ ] like this`) and click the rendered checkbox: it
calls `core.command("toggleTask", pos)` and applies the result the same way
undo/redo and streaming do — the checkbox is the project's first "widget
island" (a CM6 replace decoration wrapping a real `<input type="checkbox">`).
Clicking it never moves the text cursor.

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
  the DOM). This also holds for the M1 additions below: blockquote markers,
  code-fence/code-block lines, and the task checkbox widget never change a
  line's height between concealed/revealed states.
- Drag-select across several formatted spans: while the mouse button is down,
  reveal state must not flicker; it recomputes once on mouse-up.
- M1 vocabulary: `~~strikethrough~~`, `[links](url)` (cursor in reveals the
  destination as a separate styled span), `> blockquotes`, fenced code blocks,
  list markers, and thematic breaks (`---`) all render live; try placing the
  cursor inside a link to see the URL appear.

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
