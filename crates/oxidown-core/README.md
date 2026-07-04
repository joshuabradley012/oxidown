# oxidown-core

The Oxidown editor core (M0 spike). Implements the boundary contract in
[`docs/boundary-v0.md`](../../docs/boundary-v0.md): the rope is the document,
the parse is a disposable overlay, decorations are derived, undo is
core-driven. Public API positions are **UTF-16 code units**; internals are
UTF-8 byte offsets and never leak.

## Module layout

| Module | Responsibility |
|---|---|
| `text` | `ropey` rope wrapper. UTF-16 ⇄ UTF-8 conversion via ropey's utf16 metrics (`utf16_cu_to_char` / `char_to_utf16_cu`), with surrogate-split detection by round-tripping through the char index. `ByteSplice` (the internal splice type) lives here. |
| `parser` | Phase A (plan.md §5.2): full-document reparse per edit with `pulldown-cmark` 0.13 `into_offset_iter()`. Extracts the M0 node set only — ATX headings h1–h6, strong, emphasis, inline code — with byte-exact delimiter spans computed from event spans + source bytes (handles `***bold-italic***` nesting; setext headings are recognized and skipped). |
| `oplog` | Append-only `Op { id: (replica, counter), lamport, parent_counter, origin, splice }` log. Every edit appends, including undo/redo applications. No clocks, no entropy. |
| `history` | Undo/redo as inverted-op stacks. Stack discipline keeps stored inverses valid in current-doc coordinates without mapping. Coalescing: single-splice `user`/`ime` edits within 500 ms whose splice falls inside the top unit's replaced region merge; `paste` never coalesces; coalescing pauses during composition and breaks after undo/redo. |
| `composition` | IME session range (bytes), mapped through every edit batch; IME-origin insertions touching the range grow it. |
| `decorations` | Filters the cached overlay for a viewport (never reparses). Core-side reveal: closed-interval intersection of any selection with the node's full extent (delimiters included) — so a cursor immediately before/after a delimiter reveals. Composition rule: conceal spans intersecting the session range are emitted as `mark:delim`. |
| `editor` | `Editor`: `load`, `apply_edit` (validated multi-splice batches, ascending original-doc coordinates), `undo`/`redo`, `decorations`, `composition_begin/end`, `get_text`, `doc_len_utf16`, `revision`. All UTF-16 conversion happens here. |
| `error` | `CoreError` (`StaleRevision`, `InvalidSplice`, `OutOfBounds`, `SurrogateSplit`, `InvalidRange`). No panics on bad external input. |

wasm-safety: the core never calls `SystemTime`/`Instant` (they panic on
`wasm32-unknown-unknown`). `apply_edit` takes an injected `now_ms: f64`;
`replica_id` is a constructor parameter (no `rand`/`getrandom`).

## Implementation choices within the contract

These are interpretations where the contract text leaves latitude, not
deviations:

- **Heading extent excludes the trailing newline**, so a cursor at the start
  of the following line does not reveal the heading; a cursor at the end of
  the heading line does.
- **Reveal intersection is boundary-inclusive** (a cursor touching either end
  of a node's extent reveals it) — matching CM6/Obsidian feel.
- **Heading delimiter** = the `#` run plus one following space *or tab*
  (CommonMark allows both).
- **Inline-code content keeps CommonMark padding spaces** (`` ` x ` `` →
  content ~`" x "`): they are document bytes; only the backtick runs conceal.
- **Empty edit batches (or all-no-op splices) return the current revision
  unchanged** rather than burning a revision.
- **`load` on a used editor keeps the revision monotonic** (it returns
  `previous + 1`, which is `1` — "revision 0's successor" — on a fresh core).
- **Undo coalescing "positionally adjacent"** = the new single splice lies
  within (or touches the ends of) the region the top undo unit would remove:
  covers typing runs, insert-at-front, and backspacing over just-typed text.

## Known deviations / Phase-A caveats

1. **`applyEdit` complexity**: the contract says O(edit + dirty block); Phase
   A reparses the whole document per edit, i.e. O(doc). Sanctioned by plan.md
   §5.2 ("full reparse per edit is single-digit-ms up to ~100KB — fine for v1
   and honest about it"); the M0 gate is the measured budget, and Phase B
   (block-incremental parser) replaces this behind the same contract.
   Measured (native, release): ~1 ms per apply+decorations on a dense 123 KB
   doc, dominated by the pulldown event walk.
2. Nothing else — the boundary surface, semantics, and error behavior follow
   `docs/boundary-v0.md`.

## Tests

`cargo test -p oxidown-core` covers: contract decoration spans (headings
1–6, strong/em both delimiter flavors, code with multi-backtick runs,
`***x***` nesting, CJK/emoji/combining-mark offsets where bytes ≠ UTF-16),
reveal boundary positions, byte-identical round-trips over a hostile corpus
(CRLF, unclosed delimiters, setext lookalikes, escapes, zero-width chars),
random-splice mirror consistency against a plain `String`, undo/redo random
scripts with view-mirror verification of returned splices (including redo
correctness after multiple undos), coalescing rules, and composition
stability/growth.

Perf smoke (ignored by default):

```
cargo test -p oxidown-core --test perf_smoke -- --ignored --nocapture
```

## Parser span notes

`examples/span_spike.rs` dumps pulldown's `into_offset_iter()` spans for the
constructs we rely on (kept as documentation of observed behavior):
heading spans start at the first `#` and include the trailing newline;
strong/emphasis spans include delimiters, and `***x***` arrives as
`Emphasis(0..17)` wrapping `Strong(1..16)`; `Code` events span the full node
including backtick runs; setext headings arrive as `Heading` events whose
span starts at the text, not a `#`.
