# oxidown-core

The Oxidown editor core. Implements the boundary contract in
[`docs/boundary-v0.md`](../../docs/boundary-v0.md) — v0/v0.1 (M0) plus the
v0.2 (M1) additions: the rope is the document, the parse is a disposable
overlay, decorations are derived, undo is core-driven. Public API positions
are **UTF-16 code units**; internals are UTF-8 byte offsets and never leak.

## Module layout

| Module | Responsibility |
|---|---|
| `text` | `ropey` rope wrapper. UTF-16 ⇄ UTF-8 conversion via ropey's utf16 metrics (`utf16_cu_to_char` / `char_to_utf16_cu`), with surrogate-split detection by round-tripping through the char index. `ByteSplice` (the internal splice type), single-byte probes, and line-range lookup live here. |
| `parser` | Phase A (plan.md §5.2): full-document reparse per edit with `pulldown-cmark` 0.13 `into_offset_iter()`. M0 node set: ATX headings h1–h6, strong, emphasis, inline code. M1 adds: strikethrough, links (inline + autolink/email), blockquotes (per-line, with nesting depth), fenced code blocks (fence + body lines), list markers, task-item checkboxes, thematic breaks — see "M1 parser notes" below. `parse_document` produces the overlay **and** the top-level block spans for the block index in ONE pulldown pass per edit. Byte-exact spans are computed from event spans + source bytes, never from event *text* payloads (handles `***bold-italic***` nesting, links inside emphasis, code spans containing delimiter-looking characters, lists inside blockquotes; setext headings are recognized and skipped). |
| `mapping` | Shared position/range mapping through splice batches, parameterized by `Bias` (`Before`/`After` at exact insertion points). One algorithm behind the composition range, anchors, and the block index (`map_pos`, `map_range` extend-biased, `map_range_shrink`). |
| `anchor` | Public anchors (v0.2): id → (byte pos, bias), mapped through every applied batch. Deletion collapses to the deletion site (never null in M1); `load` clears all anchors; ids are never reused. |
| `block_index` | Top-level blocks with sticky `BlockId(replica, counter)` (plan.md §5.3). M1-internal (not on the wasm boundary); consumed by streaming's tail fast path. Identity via shrink-biased span mapping + linear two-pointer overlap matching — see the module docs for the split/merge/replace semantics. |
| `oplog` | Append-only `Op { id: (replica, counter), lamport, parent_counter, origin, splice }` log. Every edit appends, including undo/redo applications. No clocks, no entropy. |
| `history` | Undo/redo as inverted-op stacks. Stack discipline keeps stored inverses valid in current-doc coordinates without mapping — except AI stream units, which merge appends into one (possibly non-top) unit via a frame-preserving cascade (`record_stream_append`; see its docs). Coalescing: single-splice `user`/`ime` edits within 500 ms whose splice falls inside the top unit's replaced region merge; `paste`/`command`/`ai` never coalesce; coalescing pauses during composition and breaks after undo/redo. |
| `composition` | IME session range (bytes), mapped through every edit batch; IME-origin insertions touching the range grow it. |
| `commands` | v0.2 command planners: pure functions from (overlay, source, target) to minimal splices + post-apply selection, or `None` when a command doesn't apply. See "M1 command decisions" below. |
| `decorations` | Filters the cached overlay for a viewport (never reparses). Core-side reveal: closed-interval intersection of any selection with the node's full extent (delimiters included) — so a cursor immediately before/after a delimiter reveals. Composition rule: conceal spans intersecting the session range are emitted as `mark:delim`. M1 adds the `Block` (blockquote/code-fence/code-block/hr line chrome) and `Widget` (task checkbox) decoration variants, kept separate from the M0 `Line` variant so its shape (and every M0 test matching it) is untouched — see "M1 decoration notes" below. |
| `editor` | `Editor`: `load`, `apply_edit` (validated multi-splice batches, ascending original-doc coordinates), `undo`/`redo` (returning `CoreChange` incl. cursor placement per v0.2 clarification 1), `decorations`, `composition_begin/end`, anchors (`create/resolve/drop_anchor`), `command`, streaming (`stream_open/append/close` with the tail fast path), `get_text`, `doc_len_utf16`, `revision`. All UTF-16 conversion happens here. |
| `error` | `CoreError` (`StaleRevision`, `InvalidSplice`, `OutOfBounds`, `SurrogateSplit`, `InvalidRange`, `UnknownStream`). No panics on bad external input. |

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

## M1 parser/decoration notes (node & extent decisions the contract leaves open)

The v0.2 contract (docs/boundary-v0.md) specifies *what* to emit per
construct but leaves several node/extent choices open. Decisions made,
in the order Phase 1 made them:

- **Link node extent** = the whole `[text](url)` span, delimiters included
  (mirrors headings/strong/em/code). `delims` = `[` and `](url)` (2 spans,
  like strong/em); `content` = the link text; a separate `url` field carries
  the destination span, emitted as `mark:url` **only when revealed** (the
  contract's "destination part, emitted only when the link node is
  revealed"). The destination span is found by scanning source bytes from
  the closing `)` backward (paren-depth-matched, so `(parens)` inside a URL
  don't break it) rather than trusting pulldown's `dest_url` payload, which
  may be normalized/percent-decoded differently from the source; a URL
  wrapped in `<...>` is unwrapped to just the inner span, and a trailing
  ` "title"` is excluded from the emitted `mark:url` span. Only
  `LinkType::Inline` is decorated — reference/collapsed/shortcut links and
  wikilinks parse (the parser "understands more than it decorates") but
  emit no M1 node, since the contract only names `[text](url)` and
  autolinks.
- **Autolinks (`<url>`, `<email>`)** have **no delimiters at all**: `content`
  = the whole extent, `delims` = empty, so they always emit `mark:link`
  unconditionally (never conceal/reveal — there's nothing to conceal, the
  visible text *is* the destination), matching "Autolinks: `mark:link`
  whole" literally.
- **Blockquote reveal is per LINE, like headings** — not per the whole
  blockquote node. Each source line inside a blockquote becomes its own
  `BlockQuoteLine(depth)` node whose `extent` is that line (trailing
  newline excluded, exactly like heading extent) and whose `delims` are
  the `> ` marker run(s) *actually present* at that line's start. A cursor
  on one quoted line reveals only that line's markers, independent of
  sibling lines — consistent with "reveal per LINE like headings" and with
  how a view would want to un-hide one line's chrome while editing it.
- **Blockquote depth is computed by span overlap, not by counting `>`
  characters on the line.** Nesting depth intervals are recorded when each
  `BlockQuote` node starts (pulldown's `Start` event already reports the
  *whole* node's span at that point, verified empirically — see
  `examples/span_spike.rs`); a line's depth is the deepest interval that
  *overlaps* it at all. This deliberately diverges from "depth = number of
  literal `>` runs on the line": a lazily-continued line (CommonMark lazy
  continuation lets a line with fewer/no `>` markers still belong to the
  deeper blockquote) gets the *deeper* line style even though its own
  conceal set has fewer (or zero) marker spans to hide — e.g. in
  `"> outer\n> > inner\n> outer again\n"`, line 3 has one literal `>` but is
  lazily continuing the depth-2 paragraph, so it renders as
  `line:blockquote depth=2` with only one concealable marker span. Point-
  containment against the line's *first* byte was tried first and is wrong:
  a nested blockquote's own recorded span starts at *its own* marker, not
  at column 0 of the shared physical line (`"> > inner"`'s inner interval
  starts at the second `>`), so it undercounts depth for exactly the common
  case of a marker that isn't in the first column.
- **Fenced code block lines are derived by scanning raw source bytes**
  within the block's extent (fence line, body lines split on `\n`, and a
  closing fence line only if one is actually present and matches the
  opening fence's char+length), not from pulldown's `Text` event payloads —
  robust to however many `Text` events pulldown emits for the body, and
  byte-exact regardless. Fences (`line:code-fence`) are never concealed in
  M1 per the contract ("Fences stay visible... styled only"); body lines
  get both `line:code-block` and `mark:code` over their content, with no
  delimiters (so no reveal predicate applies to code blocks at all).
- **List item marker extent is found by lookahead to the next parse event**,
  not by re-deriving CommonMark's marker-width rule from bytes: the width
  of `- `/`1. `/`1) ` (and how many spaces of indentation belong to the
  marker vs. count as the item's own indentation) is exactly what pulldown
  has already computed correctly by locating where the item's real content
  begins — reimplementing that byte-scanning rule independently would be
  redundant and a likely source of subtle bugs. This is the one M1
  construct that isn't resolved purely from a single event's span.
- **Task widget reveal extent is the *list item's* marker extent** (bullet
  through the closing `]`, e.g. `"- [ ]"`), not just the checkbox's own span
  (`"[ ]"`) — per the contract ("task widget reveal keys off the list
  item's marker extent"), so that clicking the *rendered* checkbox widget
  (which sits inside that larger byte range) still counts as touching the
  node and reveals it on the next selection-driven recompute.
- **List markers are never reveal-gated at all** — they're modelled as a
  `ListMarker` node with empty `delims`/`content`, handled by its own
  branch in `decorations::compute` that unconditionally emits `mark:list-
  marker` over the node's extent, bypassing the conceal/reveal machinery
  entirely (matches "always visible, styled, never concealed").
- **GFM options enabled**: `ENABLE_STRIKETHROUGH`, `ENABLE_TASKLISTS`,
  `ENABLE_TABLES`, `ENABLE_FOOTNOTES`. Tables and footnotes parse (so they
  don't corrupt surrounding block structure) but emit no M1 node — "parser
  may understand more than it decorates" (plan.md §5.2). `ENABLE_GFM`
  itself (alert-tagged blockquotes: `[!NOTE]` etc.) and
  `ENABLE_SMART_PUNCTUATION` are deliberately **not** enabled — out of the
  M1 markdown scope (plan.md §6) and, for smart punctuation specifically,
  a parse-time text transform we don't want anywhere near span computation.

## M1 command decisions (contract-open choices)

`command` returns `Ok(None)` when a command doesn't apply; applied commands
enter the op log with origin `command`, never coalesce, and return
`CoreChange { revision, splices (current-doc UTF-16), selection }`.

- **Inline toggle OFF** when a same-kind node's closed extent fully contains
  the target range; the *innermost* such node when several nest (`_a *b* c_`
  + toggleEm in `b` unwraps `*b*`). Delimiters strip whatever their source
  flavor (`__`, `_`, longer backtick runs).
- **Inline toggle ON/EXTEND** otherwise: the range unions with every
  same-kind node it *touches* (closed intersection — adjacency merges rather
  than stacking `****`), touched nodes' delimiters strip, one canonical pair
  (`**`, `*`, `~~`) wraps the union. Inline code computes its backtick run
  as longest-run-in-content + 1, space-padded when content starts/ends with
  a backtick (CommonMark). **Double-toggle byte-identity holds for canonical
  flavors**; `__x__` re-wraps as `**x**` (normalization, deliberate).
- **Empty range, nothing touched**: inserts an empty pair, cursor between
  the delimiters (standard toolbar behavior; the pair doesn't parse until
  content exists).
- **setHeading** operates on the line containing `pos`; applies only on
  Paragraph/ATX-Heading/BlockQuote blocks (None on code, lists, tables, HTML
  blocks, blank lines, and setext headings). Inside blockquotes the hashes
  go after that line's `> ` markers. `setHeading(pos, level)` at the current
  level, and `level 0` on a non-heading, are no-ops → `None` (no burned
  revision). Level > 6 errors.
- **toggleTask** accepts `pos` anywhere in the item (the parser records each
  task item's full extent, multi-line items included) and flips exactly the
  one checkbox byte (`[X]` also unchecks). Returns `selection: None` — a
  1-for-1 byte swap never moves the cursor.
- Toggle from/to are strict positions (surrogate split errors — they lead to
  mutations); reversed ranges normalize; setHeading/toggleTask positions are
  query-like and floor-snap.

## M1 anchor decisions

- `create_anchor` positions snap toward the bias inside surrogate pairs
  (floor for `before`, ceil for `after`) — an anchor is a tracked query
  position, not a mutation.
- `load` drops all anchors (a replaced document invalidates every position);
  `resolveAnchor` then returns null, which is also the only other null case
  (unknown/dropped id). Deleting anchored text collapses to the deletion
  site per the contract — never null from edits in M1.
- Anchor ids, like op and block ids, are never reused within an editor.

## M1 streaming decisions

- One stream session = ONE undo unit, implemented by merging every append
  into the stream's unit even when user-edit units sit above it in the
  stack: the append insertion is cascaded down frame-by-frame (each above
  unit's stored inverse is rewritten as if the insertion had always existed
  in its frame — positions shift, delete-spans strictly containing the
  insertion split around it so no foreign unit ever deletes streamed text).
  Undo after close therefore reverts exactly the streamed spans (mapped),
  in one step, without touching user edits made during the stream — the
  clarified "sound behavior".
- Documented edge: undoing the stream's unit *while the stream is open*
  moves it to the redo stack; the next append starts a fresh unit (and
  clears redo). The one-unit guarantee is per uninterrupted-by-undo stream
  life cycle.
- `stream_append` returns `selection: None` — an AI stream never yanks the
  user's cursor.
- Empty chunks are no-ops (no revision burned, no undo unit).
- `stream_open` positions are strict (they become insertion points).
- **Tail fast path**: a single-insertion append landing at/after the LAST
  top-level block's start (when that block starts at a line boundary —
  indented code blocks don't, and fall back) re-parses only
  `[tail_block_start, end)` and splices overlay + block index. Documented
  Phase-A assumption: a standalone parse of that slice is
  decoration-equivalent because top-level markdown blocks are
  prefix-independent at line granularity; the whole-document couplings
  (link reference definitions, footnote definitions) only affect constructs
  M1 doesn't decorate. A fuzz-style test asserts fast-path overlay ==
  full-reparse overlay after streaming markdown across block boundaries.

## Known deviations / Phase-A caveats

1. **`applyEdit` complexity**: the contract says O(edit + dirty block); Phase
   A reparses the whole document per edit, i.e. O(doc). Sanctioned by plan.md
   §5.2 ("full reparse per edit is single-digit-ms up to ~100KB — fine for v1
   and honest about it"); the M0 gate is the measured budget, and Phase B
   (block-incremental parser) replaces this behind the same contract.
   Measured (native, release, incl. the M1 block-index update — still ONE
   pulldown pass per edit): mean ~440µs / p95 ~900µs per apply+decorations
   on a dense ~100 KB doc. Stream appends via the tail fast path: mean
   ~17µs / p95 ~31µs per ~50-char chunk over 2000 chunks.
2. Nothing else — the boundary surface, semantics, and error behavior follow
   `docs/boundary-v0.md` including the v0.2 clarifications (CoreChange
   return shape for undo/redo, stream undo grouping, list-marker spans with
   trailing whitespace, link delim/url/delim reveal pieces).

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

**M1 additions**: `tests/decorations_v2.rs` — per-construct span tests for
strikethrough, links (concealed/revealed, autolink, nested in emphasis),
blockquotes (single/nested/lazy-continuation depth), fenced code (single and
multi-line body), lists (markers, task widgets, reveal/composition
withholding), thematic breaks, including CJK/emoji offset cases analogous to
the M0 suite. `tests/corpus_conformance.rs` + `tests/corpus/cases.rs` — a
vendored, hand-picked ~115-case conformance corpus (network access to
download the official CommonMark/GFM spec suites is unavailable in this
environment; see the module doc there) asserting, per case: no panic,
byte-identical round-trip, and a **well-formed** decoration set (in-bounds
spans; `Conceal`/`Widget` spans — the "exclusive" decoration kinds that
claim their bytes for hide-or-replace — never overlap each other; `Mark`
spans are *not* checked for disjointness since nested constructs legitimately
produce overlapping marks, e.g. `**bold *em* bold**`). It also
differential-tests block **structure** (kinds + document order, not spans)
against `comrak` as an oracle, with one systematic normalization applied to
both sides (list-item direct-paragraph wrapping is a tight/loose rendering
wrinkle the two libraries compute differently at some nesting/task-list
edges, and M1 doesn't decorate that distinction) — see the module doc for
what was tried and ruled out as a real divergence vs. what wasn't.

`tests/anchors.rs` — bias/collapse/drop unit tests plus a seeded property
test (anchors track a sentinel character through random edit scripts).
`tests/commands.rs` — toggle on/off/partial/double-toggle byte-identity,
setHeading and toggleTask applicability, undo-unit granularity, all with
view-mirror verification of returned splices. `tests/streaming.rs` — life
cycle, mirror-verified appends, one-undo-unit semantics with interleaved
user edits (including edits *inside* the streamed region and deletions of
streamed text), anchor mapping, fast-path-vs-full-reparse overlay
equivalence, block-ID stickiness under appends. `tests/block_index.rs` +
unit tests in `block_index.rs` — ID stickiness through edit scripts
(interior edits, splits, merges, deletes, tail updates).

Perf smoke (both ignored by default):

```
cargo test -p oxidown-core --test perf_smoke  -- --ignored --nocapture
cargo test -p oxidown-core --test stream_perf -- --ignored --nocapture
```

One M0 test-helper fix (not a semantics change): `perf_smoke`'s
`nearest_boundary` probed positions with `decorations(pos, pos)`, which
stopped rejecting mid-surrogate positions when contract v0.1 made query
positions snap instead of error — the helper then fed unvalidated positions
to strict `apply_edit`. It now probes with an all-no-op splice batch
(strictly validated, never mutates, never bumps the revision).

## Parser span notes

`examples/span_spike.rs` dumps pulldown's `into_offset_iter()` spans for the
constructs we rely on (kept as documentation of observed behavior):
heading spans start at the first `#` and include the trailing newline;
strong/emphasis spans include delimiters, and `***x***` arrives as
`Emphasis(0..17)` wrapping `Strong(1..16)`; `Code` events span the full node
including backtick runs; setext headings arrive as `Heading` events whose
span starts at the text, not a `#`.
