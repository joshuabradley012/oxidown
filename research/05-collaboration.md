# Collaborative Editing Tech for a Markdown-Native Rust Editor

> Research compiled July 2026 for the Oxidown plan refresh. Collaboration is a FUTURE feature; the v1 document model must not preclude it. Claims verified against primary sources (npm/crates.io/GitHub/official blogs); unverifiable items flagged ⚠.

## TL;DR

1. **The field converged (2024-2026) on the event-graph / eg-walker model** for character-fidelity text merging: store an append-only log of *original, untransformed* operations with unique IDs and causal parents; build CRDT state transiently only when concurrency exists. Loro is built on it, **Figma adopted it for Code Layers (2025)**, Zed's DeltaDB (2026) is the same family. Excellent news: *a single-user editor with a well-designed operation log is already 80% of an eg-walker document.*
2. **For rich text semantics, Peritext won the argument**: formatting as marks anchored to stable character IDs with before/after anchor sides and per-mark expand rules. Automerge 2.2+ and Loro both implement Peritext-derived designs; Yjs did not (it kept Delta-style attributes; v14 adds attributed changesets/track-changes instead).
3. **Production reality 2026 is hybrid**: server-ordered logs + rebasing (Google Docs, ProseMirror/CodeMirror, Replit, tldraw, Linear) still dominate shipped products; CRDTs own local-first/offline (Zed, Anytype, Obsidian Relay, Notion's offline pages). You don't need to pick a merge algorithm now — you need a data model that doesn't foreclose any of them.
4. **Markdown-specific:** syncing raw markdown source through a text CRDT ships (Obsidian Relay, HedgeDoc 2) but has documented intent failures (`**The **fox** jumped.**`, `#`+`#`→`##`). CRDT-over-rich-AST (blocks + marks, markdown as serialization) is what every polished product chose.
5. **Minimal v1 door-openers:** stable IDs (at least at block granularity), opaque anchor types instead of raw offsets in the public API, invertible/serializable operations with origin tags, undo as inverted-ops (never snapshots), no wall-clock or index-based invariants in persistence.

## 1. Peritext (Ink & Switch)

**Source:** https://www.inkandswitch.com/peritext/ (Litt, Lim, Kleppmann, van Hardenberg); peer-reviewed CSCW 2022.

- **Stable character IDs.** Every inserted character gets an immutable opId (`counter@nodeId`). Concurrent ops reference characters by identity, never by index. Deleted characters persist as tombstones for anchoring.
- **Formatting = marks stored alongside the text sequence.** `addMark`/`removeMark` with `start`/`end` **anchors**: a character opId plus a `"before"`/`"after"` side — the boundary attaches to the *gap* next to a character.
- **Expand semantics via anchor side.** Bold ends "before the character after the span," so text typed at the end of a bold run stays bold; links end "after the last character," so appended text does not extend the link.
- **Deterministic derivation**; same-type conflicts resolve LWW by opId; different mark types coexist; comments get set semantics.
- Intent-preservation showcase: Alice bolds a sentence while Bob inserts a word inside it → the merged doc bolds the inserted word too. The essay documents a Yjs anomaly where bold could bleed into never-bold text.
- Limitations: inline formatting only (no block structure in the original); text move/splice explicitly unsolved.

**Adoption:** Automerge (2.2 marks API "originally described in Peritext", https://automerge.org/blog/rich-text/); Loro (Peritext-inspired, redesigned with paired style anchors for event-graph replay, https://loro.dev/blog/loro-richtext); Yjs did not adopt.

## 2. Yjs / yrs — status July 2026

- Latest stable v13.6.31 (May 2026). **v14 in late RC** — adds **changesets ("delta") and attributions** (attributing changes to a user/AI agent/timestamp), enabling versioned history and Track Changes; built with the BlockNote team.
- **Rich text model:** `Y.Text` = Quill-Delta-style inline attributes; `Y.XmlFragment` for tree docs. **Not Peritext** — the formatting-intent anomalies remain.
- **Bindings:** y-prosemirror (known bug class: [#258 concurrent-schema-fixup divergence](https://github.com/yjs/y-prosemirror/issues/258)); Lexical ships first-party Yjs collab; Tiptap Collaboration = Yjs + Hocuspocus; BlockNote on ProseMirror+Yjs.
- **Positions:** the sanctioned stable anchor is **`Y.RelativePosition`**, serializable (https://docs.yjs.dev/api/relative-positions).
- **Undo:** `Y.UndoManager` — selective undo **by transaction origin** (`trackedOrigins`), 500ms capture merging.
- **Awareness protocol:** presence/cursors live *outside* the document history — an ephemeral schemaless CRDT with 30s timeout. Key architectural lesson.
- **yrs** 0.27.2 (June 2026), binary-compatible with Yjs. ⚠ No evidence yet that yrs supports v14 changesets/attributions.
- Production users (self-reported): Evernote, Proton Docs, GitBook, JupyterLab, Linear, AFFiNE, +~60. The most battle-tested ecosystem, with the weakest rich-text semantics of the three Rust options.

## 3. Automerge 2.x → 3.x

- **Automerge 3 shipped July 2025** (https://automerge.org/blog/automerge-3/): compressed representation at runtime; memory cut ">10x" ("pasting Moby Dick… 700Mb in Automerge 2… only 1.3Mb" in 3). Rust crate `automerge` 0.10.0 (Jun 2026), active.
- **Rich text = productionized Peritext** (since 2.2): `mark(doc, path, {start, end, expand}, name, value)` with `expand: before|after|both|none`; **block markers** for paragraphs/headings/list items; `spans`/`updateSpans` APIs (https://automerge.org/docs/reference/documents/rich-text/).
- **Positions:** Automerge **cursors** — "a relative position, 'before character X'", serializable.
- Editor binding automerge-prosemirror still "beta quality." Incoming: Keyhive (capability-based access control), Sedimentree, Subduction.

## 4. Loro

- 1.0 October 2024; current 1.13.6 (June 2026), steady cadence. Rust core; JS/WASM, Swift, Python, C-FFI, React Native bindings.
- **Architecture — the eg-walker adaptation in production:** split **OpLog** (immutable history) / **DocState** (materialized state), stored in ~4KB lazily-loaded blocks; "The Event Graph Walker algorithm… has been adapted" (https://loro.dev/docs/advanced/doc_state_and_oplog). Fugue for text ordering.
- **Rich text:** Peritext-compliant via paired style anchors; passes all Peritext paper scenarios.
- **Beyond text:** MovableList, movable Tree (Kleppmann 2021 + fractional index), Counter, time travel/checkout, shallow snapshots.
- **Cursors:** op ID + container ID + Side; queries on deleted positions return a refreshed nearby cursor.
- **Undo:** purpose-built for collab — "undo/redo affects only local operations from the bound peer"; stores + transforms cursors per undo step.
- ⚠ Benchmarks unverified (site Cloudflare-blocked at research time); youngest ecosystem, small bus factor. Feature-wise the best on-paper fit for a markdown editor.

## 5. The event-graph turn (eg-walker)

- Seph Gentle's arc: ["I was wrong. CRDTs are the future"](https://josephg.com/blog/crdts-are-the-future/) (2020) → ["5000x faster CRDTs"](https://josephg.com/blog/crdts-go-brrr/) (2021: Automerge v1 ~291s/880MB → diamond-types 0.056s/1.1MB on a real 260k-keystroke trace; flat run-length-encoded spans, range B-tree, positional caching).
- **The eg-walker paper:** "Collaborative Text Editing with Eg-walker: Better, Faster, Smaller" — Gentle & Kleppmann, [arXiv:2409.14252](https://arxiv.org/abs/2409.14252), **EuroSys 2025**, Best Artifact award.
  - Editing history = **event graph**: a git-like DAG where each event is one op + unique ID + parent event IDs. **Events store positions as original indices at generation time** and are never transformed in storage.
  - **Merge = replay**: topologically sort, transform each event — like OT but no central server; long-diverged trace: OT ~1 hour vs eg-walker ~24ms.
  - **Transient CRDT state, built only under concurrency**, discarded at every "critical version." Sequential editing — the common case — never accumulates CRDT state.
  - **Document at rest = plain text + compact columnar event log.** No permanent tombstones; ~10x less steady-state memory than CRDTs.
  - ⚠ The paper contains **no user-facing selective-undo treatment** — retaining the event graph *enables* history features, but that design is on you. Adjacent: Stewen & Kleppmann, "Undo and Redo Support for Replicated Registers" (PaPoC '24).
- **Strategic implication:** you don't need CRDT-native document *storage*. An append-only event log (IDs + causal parents) plus a cached text snapshot is sufficient; CRDT machinery becomes a transient computation at merge time. This is what makes a single-user v1 upgradeable.
- **diamond-types**: the reference implementation; plain text only; not production-ready as a dependency. **cola**: plain-text Rust CRDT, maintenance mode; worth reading for position encoding ([design post](https://nomad.foo/blog/cola)).

## 6. OT vs CRDT in production, 2026

| Product | Approach |
|---|---|
| Google Docs | Central-server OT over a revision log (unchanged since 2010) |
| Notion | Per-block server reconciliation (LWW-ish); **Dec 2025: offline pages migrated to a new CRDT data model** — now hybrid |
| Figma canvas | Server-authoritative per-property LWW + fractional indexing ("isn't using true CRDTs") |
| **Figma Code Layers (2025)** | Evaluated LWW/OT/CRDT, **chose eg-walker**: "as fast as CRDTs at merging, but has minimal memory overhead like OTs" ([post](https://www.figma.com/blog/building-figmas-code-layers/)) |
| Linear | Server-authoritative transaction log, property-level LWW |
| Zed | CRDT-native buffers from day one; **DeltaDB (2026)** — CRDT "version control between commits," git-interoperable |
| Replit, tldraw sync, Etherpad, Overleaf | OT / server-authoritative rebasing |
| Obsidian Relay, Anytype, HedgeDoc 2 | Yjs / CRDT, local-first |

**Haverbeke's argument** ([ProseMirror collab](https://marijnhaverbeke.nl/blog/collaborative-editing.html), [CodeMirror collab](https://marijnhaverbeke.nl/blog/collaborative-editing-cm.html)): with a central authority ordering changes, clients rebase and retry; he rejects CRDTs on resource grounds (note: eg-walker substantially answers this 2020 objection) and concedes the model "fails for offline work or branching workflows." Matthew Weidner's ["Collaborative Text Editing without CRDTs or OT"](https://mattweidner.com/2025/05/21/text-without-crdts.html) names server reconciliation as a third way.

**2026 consensus:** (1) with a server, a server-ordered log + rebasing is the simple proven default; (2) CRDTs won local-first/offline; (3) the hottest pattern is hybrid: server-authoritative product with event-graph text merge where character fidelity matters. The OT-vs-CRDT war is effectively over — eg-walker-style designs are OT-cheap in the common case and CRDT-correct under conflict.

## 7. Designing a collab-ready single-user v1

**Collaboration-readiness is a property of the data model and API surface, not of shipping a merge algorithm.**

### 7.1 Identity: stable IDs, not offsets
- Give blocks stable unique IDs at creation (`(replica_id, counter)` — Zed's scheme, collision-free without coordination). Impossible to retrofit onto persisted documents later.
- **Character-level IDs: do NOT eagerly materialize.** Eg-walker's insight: per-character identity derives from the event log on demand (insertion event ID + offset). Storing per-char metadata eagerly is the Automerge-1 mistake.
- **Never let byte offsets escape the core.** Offsets are valid against exactly one document version; "byte offset" is UTF-8-specific — a portability bug even single-user (Swift/Kotlin/JS disagree on units).

### 7.2 Anchors as the public position type
Expose an opaque `Anchor` (internally: op/insertion ID + offset + **bias/side**) with resolve functions — exactly like Zed anchors, Yjs RelativePosition, Automerge cursors, Loro Cursor. Use for selections, comment ranges, decoration spans, scroll positions. The bias parameter is Peritext's before/after distinction and is what makes bold-vs-link expansion work later.

### 7.3 Operations: an event log from day one
- Ops carry: unique ID, lamport counter + replica id, **origin tag** (local-user / paste / plugin / AI / remote-peer), positions as plain indices valid at generation time, and the **parent version** they were generated against (single-user: always "previous op", costs nothing — and turns the log into an event DAG the day a second writer appears).
- Required algebra: `invert` (undo), `serialize`, and ideally `map`/`compose` à la [CodeMirror ChangeSet](https://codemirror.net/docs/ref/#state.ChangeSet) — its documented law `A.compose(B.map(A)) == B.compose(A.map(B, true))` is the minimal contract for server-rebased collab, the cheapest collab you might ship first.
- Express edits as intent-preserving splices, not snapshot diffs.
- Persist the log (columnar/run-length encoded — ~1 byte/keystroke in practice) alongside a cached snapshot. History truncation can be a feature (Loro shallow snapshots).
- **Upgradeable vs not:** a sequential log of invertible ops + parent refs *literally becomes* an eg-walker event graph, and also supports ProseMirror-style server rebasing. "Mutate buffer in place + keep snapshots for undo" forecloses both.

### 7.4 Undo: inverted ops with origin filtering, never snapshots
Haverbeke: collaborative undo "definitely should not use a single, shared history. If you undo, the last edit that *you* made should be undone" — and the post-undo state "is a *new* one, not seen before," which snapshot-restore cannot produce. Implement v1 undo as a stack of inverted operations, grouped by time window (~500ms), tagged by origin, with stored inverses mapped/rebased across subsequent changes. Collab-era selective undo becomes a filter (`origin == me`). Figma's spec: "if you undo a lot, copy something, and redo back to the present, the document should not change." Store cursor state per undo step.

### 7.5 Document shape for structural collab (if/when needed)
Blocks with stable IDs; block *move* as a first-class operation (not delete+insert); inline formatting as mark spans with expand semantics per mark type; schema normalization expressed as deterministic ops (y-prosemirror #258 shows what happens otherwise).

### 7.6 Safe to defer
Merge algorithm choice; network protocol; awareness/presence (deliberately outside document history); tombstone storage; per-character metadata; access control.

### 7.7 Minimal v1 checklist
1. Stable IDs persisted at block granularity, survive moves.
2. Public positions: opaque `Anchor {op_id, offset, bias}`; offsets only as transient render-layer values.
3. All mutation via transactions producing serialized, invertible, origin-tagged ops with lamport stamp + parent version; append-only log persisted with snapshot.
4. Undo = inverted-op stack filtered by origin, mapped across later ops.
5. Move as a real op. No wall-clock in conflict-relevant data.
6. A `replica_id` concept from day one (single-user: one replica; multi-device sync reuses it).

## 8. Markdown-specific collaboration

**Approach 1 — sync the markdown source (plain-text CRDT/OT + reparse).** Shipped: Etherpad, HedgeDoc, Overleaf (LaTeX), **Obsidian Relay** (Yjs over CM6), Peerdraft. Pros: trivial infrastructure, files stay plain markdown, same-paragraph character merges work. Cons — documented in the Peritext essay itself, which explicitly analyzes markdown:
- *Marker interleaving:* Alice bolds "The fox", Bob bolds "fox jumped." → merged source `**The **fox** jumped.**` renders with the one word both users bolded left unbolded.
- *Reparse ambiguity:* two users each prepend `#` → `##`, an H2 neither user ever saw. Merged bytes are valid markdown that parses to a third structure.
- *Bold-while-typing:* text typed at the edge of a concurrently-bolded range lands outside the `**` markers.
- *Block moves:* delete+insert in source strands concurrent edits inside the moved region.

**Approach 2 — CRDT over rich AST/blocks, markdown as serialization.** What every polished product chose: BlockNote, Tiptap Collaboration, Milkdown, AFFiNE/BlockSuite, Automerge rich text. Cost: markdown becomes a (lossy) import/export boundary; schema + normalization + heavier infrastructure.

**Middle grounds:** Etherpad's attribute-pool design (formatting as attributes over plain text) historically dodged marker interleaving while staying text-oriented; Overleaf shows source-level merging is acceptable when users are source-literate — arguably true for markdown power users.

**For a markdown-native product:** fine to treat the rope of markdown source as the document for v1; make the op log per §7.3 so both futures stay open. Source-level text-CRDT sync (Obsidian-Relay-grade) is shippable for multi-device sync and casual collab; Google-Docs-grade concurrent formatting would require the structural model — an explicit, deferred decision.

**Key primary sources:** [Peritext](https://www.inkandswitch.com/peritext/) · [eg-walker paper](https://arxiv.org/abs/2409.14252) · [Automerge 3](https://automerge.org/blog/automerge-3/) · [Loro 1.0](https://loro.dev/blog/v1.0) · [Zed CRDTs](https://zed.dev/blog/crdts) · [Figma Code Layers](https://www.figma.com/blog/building-figmas-code-layers/) · [Notion offline](https://www.notion.com/blog/how-we-made-notion-available-offline) · [Haverbeke on collab](https://marijnhaverbeke.nl/blog/collaborative-editing.html) · [Weidner 2025](https://mattweidner.com/2025/05/21/text-without-crdts.html)
