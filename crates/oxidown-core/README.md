# oxidown-core

The Oxidown editor core. Implements the boundary contract in
[`docs/boundary-v0.md`](../../docs/boundary-v0.md) — v0/v0.1 (M0) plus the
v0.2 additions and v0.3 amendments (M1; the contract's current version is
v0.3): the rope is the document, the parse is a disposable
overlay, decorations are derived, undo is core-driven. Public API positions
are **UTF-16 code units**; internals are UTF-8 byte offsets and never leak.

## Module layout

| Module | Responsibility |
|---|---|
| `text` | `ropey` rope wrapper. UTF-16 ⇄ UTF-8 conversion via ropey's utf16 metrics (`utf16_cu_to_char` / `char_to_utf16_cu`), with surrogate-split detection by round-tripping through the char index. `ByteSplice` (the internal splice type), single-byte probes, and line-range lookup live here. |
| `parser` | Phase A parsing (plan.md §5.2) with `pulldown-cmark` 0.13 `into_offset_iter()`; per edit the `editor` invokes it on an incremental window, a tail slice, or the full document (see `editor`). M0 node set: ATX headings h1–h6, strong, emphasis, inline code. M1 adds: strikethrough, links (inline + autolink/email), blockquotes (per-line, with nesting depth), fenced code blocks (fence + body lines), list markers, task-item checkboxes, thematic breaks — see "M1 parser notes" below. `parse_document` produces the overlay **and** the top-level block spans for the block index in ONE pulldown pass per edit. Byte-exact spans are computed from event spans + source bytes, never from event *text* payloads (handles `***bold-italic***` nesting, links inside emphasis, code spans containing delimiter-looking characters, lists inside blockquotes; setext headings are recognized and skipped). |
| `mapping` | Shared position/range mapping through splice batches, parameterized by `Bias` (`Before`/`After` at exact insertion points). One algorithm behind the composition range, anchors, and the block index (`map_pos`, `map_range` extend-biased, `map_range_shrink`). |
| `anchor` | Public anchors (v0.2): id → (byte pos, bias), mapped through every applied batch. Deletion collapses to the deletion site (never null in M1); `load` clears all anchors; ids are never reused. |
| `block_index` | Top-level blocks with sticky `BlockId(replica, counter)` (plan.md §5.3). M1-internal (not on the wasm boundary); consumed by streaming's tail fast path. Identity via shrink-biased span mapping + linear two-pointer overlap matching — see the module docs for the split/merge/replace semantics. |
| `oplog` | Append-only `Op { id: (replica, counter), lamport, parent_counter, origin, splice }` log. Every edit appends, including undo/redo applications. No clocks, no entropy. |
| `history` | Undo/redo as inverted-op stacks. Stack discipline keeps stored inverses valid in current-doc coordinates without mapping — except AI stream units, which merge appends into one (possibly non-top) unit via a frame-preserving cascade (`record_stream_append`; see its docs). Coalescing: single-splice `user`/`ime` edits within 500 ms whose splice falls inside the top unit's replaced region merge; `paste`/`command`/`ai` never coalesce; coalescing pauses during composition and breaks after undo/redo. |
| `composition` | IME session range (bytes), mapped through every edit batch; IME-origin insertions touching the range grow it. |
| `commands` | v0.2/v0.3 command planners: pure functions from (overlay, source, target) to minimal splices + post-apply selection, or `None` when a command doesn't apply. See "M1 command decisions" below. |
| `decorations` | Filters the cached overlay for a viewport (never reparses). Core-side reveal: closed-interval intersection of any selection with the node's reveal extent — the full extent, delimiters included, for inline nodes (so a cursor immediately before/after a delimiter reveals); the whole line for line-prefix marker constructs and the whole fenced block for fence lines (v0.3). Composition rule: conceal spans intersecting the session range are emitted as `mark:delim`. M1 adds the `Block` (blockquote/code-fence/code-block/hr/list-item line chrome) and `Widget` (task checkbox, bullet, ordered) decoration variants, kept separate from the M0 `Line` variant so its shape (and every M0 test matching it) is untouched — see "M1 decoration notes" below. |
| `editor` | `Editor`: `load`, `apply_edit` (validated multi-splice batches, ascending original-doc coordinates), `undo`/`redo` (returning `CoreChange` incl. cursor placement per v0.2 clarification 1), `decorations`, `composition_begin/end`, anchors (`create/resolve/drop_anchor`), `command`, streaming (`stream_open/append/close` with the tail fast path), `get_text`, `doc_len_utf16`, `revision`. Owns the reparse strategies (`reparse_incremental` / `reparse_tail` / `reparse_with`, counted by `reparse_counts`) — see "Reparse architecture" below. All UTF-16 conversion happens here. |
| `error` | `CoreError` (`StaleRevision`, `InvalidSplice`, `OutOfBounds`, `SurrogateSplit`, `InvalidRange`, `InvalidArgument`, `UnknownStream`). No panics on bad external input. |

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
  (CommonMark allows both). CommonMark's optional CLOSING sequence —
  space/tab, a run of `#`, then only spaces/tabs to end of line (`# foo #`;
  `# foo#` has no preceding space and is content) — is a SECOND delimiter
  span, concealing/revealing with the same line-level semantics as the
  opening run.
- **Inline-code content keeps CommonMark padding spaces** (`` ` x ` `` →
  content ~`" x "`): they are document bytes; only the backtick runs conceal.
- **Empty edit batches (or all-no-op splices) return the current revision
  unchanged** rather than burning a revision.
- **`load` on a used editor keeps the revision monotonic** (it returns
  `previous + 1`, which is `1` — "revision 0's successor" — on a fresh core).
- **Undo coalescing** — the new single splice must lie within (or touch the
  ends of) the region the top undo unit would remove (v0.1 clarification 4
  as amended in v0.3): covers typing runs, insert-at-front, and backspacing
  over just-typed text.

## M1 parser/decoration notes (node & extent decisions the contract leaves open)

The v0.2/v0.3 contract (docs/boundary-v0.md) specifies *what* to emit per
construct but leaves several node/extent choices open. Decisions made,
in the order Phase 1 made them:

- **Link node extent** = the whole `[text](url)` span, delimiters included
  (mirrors headings/strong/em/code). `delims` = `[` and `](url)` (2 spans,
  like strong/em); `content` = the link text; a separate `url` field carries
  the destination span, emitted as `mark:url` **only when revealed** (the
  contract's "destination part, emitted only when the link node is
  revealed"). The destination span is located by a FORWARD parse rather
  than trusting pulldown's `dest_url` payload (which may be normalized/
  percent-decoded differently from the source): the opening `[` is matched
  to its `]` by bracket depth (skipping backslash escapes and backtick code
  spans, whose contents may hold unbalanced brackets), then
  `(<ws> destination <ws> title? )` is parsed forward — a backward
  paren-depth scan from the closing `)` was tried first and mis-pairs
  parens inside quoted titles (`[t](u "a)b")`, `[t](u "(a")` — both
  valid). A URL wrapped in `<...>` is unwrapped to just the inner span,
  and a trailing title (any CommonMark flavor: `"…"`, `'…'`, or `(…)`) is
  excluded from the emitted `mark:url` span. Anything the forward parse
  can't follow is defensively dropped rather than mis-spanned. Only
  `LinkType::Inline` is decorated — reference/collapsed/shortcut links and
  wikilinks parse (the parser "may understand more than it decorates") but
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
  sibling lines — consistent with the contract's line-level marker reveal
  (v0.3, which matches heading semantics) and with how a view would want
  to un-hide one line's chrome while editing it.
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
  byte-exact regardless. The enclosing containers' per-line prefix
  (blockquote `> ` runs and/or the opening fence's own list-item indent) is
  stripped BEFORE classifying fence vs. body lines and before emitting
  spans: without it, `> ``` ` fails closing-fence detection and falls
  through as a body line, a list-nested closing fence can carry >= 4
  leading spaces (defeating the 3-space allowance), and body `mark:code`
  would cover marker/indent bytes that belong to the containers' own
  decorations. Fence lines emit `line:code-fence`, and the raw fence text
  (``` + info string) CONCEALS with BLOCK-level reveal per the contract: a
  cursor/selection anywhere inside the fenced block (either fence or the
  body — the fence nodes' reveal extent is the whole fence-to-fence range)
  reveals both raw fences as `mark:delim`, so they are editable whenever
  the block is. Body lines get both `line:code-block` and `mark:code` over
  their content, with no delimiters of their own.
- **List item marker extent is found by lookahead to the next parse event**,
  not by re-deriving CommonMark's marker-width rule from bytes: the width
  of `- `/`1. `/`1) ` (and how many spaces of indentation belong to the
  marker vs. count as the item's own indentation) is exactly what pulldown
  has already computed correctly by locating where the item's real content
  begins — reimplementing that byte-scanning rule independently would be
  redundant and a likely source of subtle bugs. The lookahead is CLAMPED to
  the marker's own line: when the marker line has no content of its own
  (`"-\n  foo"`), the next event starts on a later line and the raw
  lookahead would sweep the terminator and the following line's indent into
  the marker span — a bullet widget would then conceal the newline and
  visually merge two lines. An EMPTY item (`"- \n"`, a bare `"-"`) produces
  no anchoring event at all, so its marker token is SYNTHESIZED from the
  source bytes at `End(Item)` — empty items still decorate, keep their slot
  in the ordered sequence, and stay visible to the `enter` command. This is
  the one M1 construct that isn't resolved purely from a single event's
  span.
- **Task widget reveal extent is the item's whole first line** (v0.3
  line-level marker reveal, matching headings): a cursor/selection touching
  any part of the line — including clicking the *rendered* checkbox widget,
  which sits inside it — reveals the checkbox as `mark:delim`, with the
  item's `- ` marker concealing/revealing in lockstep.
- **List markers reveal-gate as widgets, LINE-level** — modelled as a
  `ListMarker` node with empty `delims`/`content` whose reveal extent is
  the item's whole first line, handled by its own branch in
  `decorations::compute`: concealed, an unordered marker emits
  `widget:bullet` and an ordered marker `widget:ordered` (carrying the
  view-computed sequence number + delimiter, v0.3) over the whole marker
  span; revealed (cursor/selection anywhere on the line) or under active
  composition, the raw marker emits as `mark:list-marker` instead. A task
  item's `- ` run is the exception: it conceals/reveals as `mark:delim` in
  lockstep with its checkbox (the widget alone represents the item). Every
  item line additionally emits a `line:list-item` decoration with depth and
  `revealed`; nested items (depth >= 2) get a conceal (revealed:
  `mark:delim`) over their raw leading indent.
- **Strikethrough delimiter runs are read from the source**: GFM parses
  both `~x~` and `~~x~~` as strikethrough, so the run length (1 or 2) is
  taken from the bytes, never assumed to be 2. (The canonical wrap the
  `toggleStrike` command inserts is still `~~`.)
- **GFM options enabled**: `ENABLE_STRIKETHROUGH`, `ENABLE_TASKLISTS`,
  `ENABLE_TABLES`, `ENABLE_FOOTNOTES`. Tables and footnotes parse (so they
  don't corrupt surrounding block structure) but emit no M1 node — "parser
  may understand more than it decorates" (the contract's M1 emission
  scope). `ENABLE_GFM`
  itself (alert-tagged blockquotes: `[!NOTE]` etc.) and
  `ENABLE_SMART_PUNCTUATION` are deliberately **not** enabled — out of the
  M1 markdown scope (plan.md §6) and, for smart punctuation specifically,
  a parse-time text transform we don't want anywhere near span computation.

## M1 command decisions (contract-open choices)

`command` returns `Ok(None)` when a command doesn't apply; applied commands
enter the op log with origin `command`, never coalesce, and return
`CoreChange { revision, splices (current-doc UTF-16), selection }`.

- **Inline toggles refuse multi-block ranges**: a from/to spanning more than
  one leaf block throws `InvalidArgument` instead of planning — the wrapped
  text could never parse as one inline node, and a re-toggle would stack
  delimiters. A thrown command never mutates (contract: views treat it as a
  consumed no-op, not a desync), so the caller can tell "refused" from
  "didn't apply" (`None`).
- **Inline toggle OFF** when a same-kind node's closed extent fully contains
  the target range; the *innermost* such node when several nest (`_a *b* c_`
  + toggleEm in `b` unwraps `*b*`). Delimiters strip whatever their source
  flavor (`__`, `_`, `~`, longer backtick runs).
- **Inline toggle ON/EXTEND** otherwise: the range unions with every
  same-kind node it *touches* (closed intersection — adjacency merges rather
  than stacking `****`), touched nodes' delimiters strip, one canonical pair
  (`**`, `*`, `~~`) wraps the union. Inline code computes its backtick run
  as longest-run-in-content + 1, space-padded when content starts/ends with
  a backtick or a space (but is not all spaces) — the shapes CommonMark
  unpads at render time. **Double-toggle byte-identity holds for canonical
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
  behavior v0.2 clarification 2 pins.
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
  indented code blocks don't — and the first-line merge-hazard guards
  shared with `apply_edit` pass; see "Reparse architecture" below;
  otherwise the append falls back to the incremental reparse) re-parses
  only `[tail_block_start, end)` and splices overlay + block index. Documented
  Phase-A assumption: a standalone parse of that slice is
  decoration-equivalent because top-level markdown blocks are
  prefix-independent at line granularity; the whole-document couplings
  (link reference definitions, footnote definitions) only affect constructs
  M1 doesn't decorate. A fuzz-style test asserts fast-path overlay ==
  full-reparse overlay after streaming markdown across block boundaries.

## Reparse architecture (the contract's complexity clause)

The contract's "O(edit + dirty block), not O(doc)" clause holds via three
strategies, dispatched per text change in `editor.rs` (`reparse_counts`
exposes which fired, so tests can assert the fast paths actually run rather
than silently full-reparsing):

- **Tail fast path** (`reparse_tail`): a single-splice edit or stream append
  landing at/after the last top-level block's start (with guards against the
  de-interruption/setext/indent-capture merge hazards — see
  `tail_edit_fast_path_region`'s doc comment) reparses only
  `[tail_block_start, end)` and splices overlay + block index.
- **Windowed incremental reparse** (`reparse_incremental`): everything else —
  `apply_edit`, `undo`/`redo`, and every `command` route here. Dirty window =
  one top-level block of slack above the edit, extended below until the fresh
  parse's block boundaries realign with the old ones; fresh nodes/blocks are
  spliced into the cached overlay, the untouched suffix's spans are rebased
  by the edit's delta, and block IDs are re-matched through the ordinary
  `BlockIndex::update`.
- **Degrade cases**: edits whose effect cannot realign with any downstream
  block boundary (canonically, toggling a code fence open mid-document)
  reparse from the window start to the end of the document — correctness
  first. `reparse_with` (full document) remains only for `load` and as the
  no-block-index fallback.

Honest asymptotics: parse work is O(edit + dirty window); the suffix
rebase/ID re-match bookkeeping remains O(doc) with a small constant
(~0.1µs/KB measured). Every strategy is equivalence-gated node-for-node /
span-for-span against a from-scratch parse
(`tests/reparse_equivalence.rs` — fuzzed, runs un-ignored in CI). The known
whole-document couplings a windowed parse cannot see (link reference
definitions, footnote definitions) affect only constructs M1 does not
decorate — the same documented assumption as the tail fast path. Measured
(native, release; research/08 "After" section): mid-document single-char
`apply_edit` ~13µs at 100KB / ~31µs at 300KB; apply+decorations combined
p95 ~56µs at 100KB; tail-path stream appends mean ~5-6µs per chunk.

One caveat on the tail fast path: per-append cost is O(open tail block),
so a stream that never closes its tail block (one long paragraph, or a
single list with no blank lines) is quadratic in total streamed bytes —
characterized by `tests/stream_perf.rs::
stream_append_into_never_closing_tail_block_grows_per_append`, rationale
in `reparse_tail`'s COST NOTE. A real fix needs incremental inline
parsing (deferred past M1).

## Known deviations

None — the boundary surface, semantics, complexity, and error behavior
follow `docs/boundary-v0.md` including the v0.2 clarifications (CoreChange
return shape for undo/redo, stream undo grouping, list-marker spans with
trailing whitespace, link delim/url/delim reveal pieces) and the v0.3
amendments (line-level marker reveal, `widget:ordered`, `enter`, the
undo-coalescing region rule, the surrogate payload rules).

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
