# Obsidian Editing Mechanics: A Pattern Audit for the Command Layer

> Research compiled July 2026 for the Oxidown plan refresh, triggered by the Tab/marker-width-indent work in progress. Obsidian is closed-source; every claim below is triangulated from official help docs (obsidian.md/help/\*), official changelogs (obsidian.md/changelog/\*), forum.obsidian.md threads (including direct replies from Obsidian team member **WhiteNoise** and moderators, treated as authoritative on internal behavior), and the source/READMEs of open plugins that patch or replicate the gaps — chiefly [obsidian-outliner](https://github.com/vslinko/obsidian-outliner) and [Auto-List-Management-Obsidian](https://github.com/OmriLeviGit/Auto-List-Management-Obsidian). Anything not directly confirmable is marked **UNVERIFIED**. Cross-checked where useful against HyperMD, Milkdown, Typora, and GitHub's markdown editor.

---

## 0. The headline finding, up front

Obsidian's ordered-list auto-renumbering — shipped in **v1.8.3** (Jan 30, 2025), changelog: *"When modifying a numbered list, the numbers are now updated automatically"* ([changelog](https://obsidian.md/changelog/2025-01-30-desktop-v1.8.3/)) — works by **literally rewriting every sibling item's digit in the saved source text** on every insert/delete. That is exactly the class of unsolicited byte-level rewrite that both `plan.md` principle #1 (*"if you didn't edit a byte, we didn't change it. Clean git diffs are a feature"*) and `boundary-v0.md`'s model rule #2 (*"the core never mutates text on its own initiative"*) were designed to rule out.

It's also the direct cause of Obsidian's own multi-year renumbering bug tail: mass-renumbering regressions in [1.8.4](https://forum.obsidian.md/t/numbered-list-change-in-1-8-4/95960), cross-list-bleed the same week ([forum](https://forum.obsidian.md/t/once-changing-the-numbering-of-a-numbered-list-there-is-now-automatic-changing-of-the-numbering-of-all-descending-numbered-lists/95911)), a callout/Tab-interaction bug patched only in [1.8.10](https://forum.obsidian.md/t/numbered-lists-in-callouts-indenting-an-item-pressing-tab-does-not-renumber-the-item-correctly/99212), and a nested-list paste-renumbering bug still open as of [April 2025](https://forum.obsidian.md/t/nested-numbered-lists-not-numbered-correctly-after-pasting/99389) — because "rewrite the file" is a much bigger, riskier operation surface than "style the display."

CommonMark's own semantics only require an ordered list's **start** number to be meaningful; sibling item numbers in the source are cosmetic (a renderer is free to display 1, 2, 3… regardless of what's literally typed). Oxidown can get correct live numbering **for free at the view layer**: compute each item's display number from its position in the parsed list run — data the overlay already produces for `list-item` line decorations (`docs/boundary-v0.md` v0.2) — and render *that* instead of trusting the literal digits, leaving the source untouched until the user edits it directly. Today `mark:list-marker` is specified as a plain style-over-source span (`docs/boundary-v0.md`, "Expanded decoration vocabulary"); making ordered markers a **computed-value widget** (the same trick already used for `widget:bullet`) sidesteps the invariant-2 conflict entirely and is strictly more robust than what Obsidian shipped. This is elaborated in §5 and is the single P1 item in this report with the best cost/benefit ratio.

---

## 1. List editing mechanics

### 1.1 Tab / Shift-Tab — line vs. subtree, and the first-item edge case

**Core Obsidian indents/outdents only the current line, not the subtree.** A child item left behind at its original indent breaks the parent/child relationship and must be re-indented individually — confirmed by two multi-year-open forum threads: ["Indent outline carries children"](https://forum.obsidian.md/t/indent-outline-carries-children/2505) (*"if you indent(promote) a row that has children rows, the children rows remain at their original indent level"*) and ["Indenting the parent of folded content does not indent child items"](https://forum.obsidian.md/t/indenting-the-parent-of-folded-content-does-not-indent-child-items/6203). The only official text (obsidian.md/help/syntax) is *"Use Tab or Shift+Tab to indent or unindent selected list items"* — the plural implies a multi-line selection shifts each selected line one level, but **whether relative nesting survives a multi-line-selection Tab is UNVERIFIED** (no source confirms it either way).

```
Before (cursor in "Item A", no selection):    After Tab:
- Item A|                                     - Item A            (still top-level? no —)
  - Item A.1                                    - Item A           <- now indented...
  - Item A.2                                    - Item A.1         <- ...but children UNMOVED,
- Item B                                        - Item A.2             now visual siblings, not nested
                                               - Item B
```

**First item of a list, Tab does nothing useful**: Obsidian team member WhiteNoise, in [a 2021 thread](https://forum.obsidian.md/t/cant-use-tab-to-indent-the-first-list-item/30274): *"That's how markdown works. IF you [have] a blank line above, if you indent you are creating a code block."* Treated as correct CommonMark parsing, not a bug — there's no parent line for the first item to nest under.

**obsidian-outliner's "Enhance the Tab key"** (default on) explicitly fixes exactly the subtree gap: its command table describes Tab as *"Indent the list and sublists"* / Shift+Tab as *"Outdent the list and sublists"* ([README](https://github.com/vslinko/obsidian-outliner)) — i.e. it moves the whole subtree together, the fix core lacks. Multi-line selection is explicitly **not** supported even by the plugin — listed under its own `## Unsupported (yet) features`, linking to still-open [issue #3](https://github.com/vslinko/obsidian-outliner/issues/3).

**Fit for Oxidown — CORE, and this is the exact feature already in flight.** `plan.md` §5.8 already names `indent_list_item` as a core text-transform command. The marker-width-aware Tab work should implement **subtree-aware indent/outdent from day one** — i.e. do what the plugin does, not what core Obsidian does — since the overlay already computes per-line `depth` for `list-item` decorations (`boundary-v0.md` v0.2), so "find this item's contiguous run of deeper-indented descendant lines" is data the core already has for free. The first-item case should simply return `null` from the command (matches the existing `command(...): CoreChange | null` "no-op" convention already established for `toggleTask`/`setHeading`).
**Priority: P1** — directly blocks/refines the task at hand; strictly better than the reference implementation at near-zero extra cost since the depth data already exists.

### 1.2 Ordered-list auto-renumbering

Before v1.8.3 this was flatly broken in the live editor — WhiteNoise's own 2021 bug report: *"the numbering does NOT update in Edit mode when a middle item is deleted... It IS updated in Preview mode"* ([forum](https://forum.obsidian.md/t/automatically-keep-numbered-list-ordered-in-editor-adding-removing-swapping-pasting/28428)). Shipped in [v1.8.3](https://obsidian.md/changelog/2025-01-30-desktop-v1.8.3/): inserting a new "2." between existing "2."/"3." bumps everything below to "3."/"4." live, no extra trigger. It's bundled into the general **"Smart lists"** editor setting (obsidian.md/help/settings: *"Automatically set indentation and place list items correctly"*), not a dedicated renumber toggle — a user request for a separate opt-out was raised and not substantively engaged ([forum](https://forum.obsidian.md/t/option-to-turn-off-new-auto-renumber-feature-in-future-releases/96245)). Rollout was rocky: mass-renumbering regressions ([1.8.4](https://forum.obsidian.md/t/numbered-list-change-in-1-8-4/95960)), cross-list bleed the same week ([forum](https://forum.obsidian.md/t/once-changing-the-numbering-of-a-numbered-list-there-is-now-automatic-changing-of-the-numbering-of-all-descending-numbered-lists/95911)), and a callout/Tab interaction bug open through [1.8.10](https://forum.obsidian.md/t/numbered-lists-in-callouts-indenting-an-item-pressing-tab-does-not-renumber-the-item-correctly/99212). **Auto-List-Management-Obsidian**, the closest thing to a "smart lists" community plugin, documents a residual core bug even post-1.8.x: numbered checklists can start renumbering from "2." instead of "1." during checkbox reorder ([README](https://github.com/OmriLeviGit/Auto-List-Management-Obsidian)).

```
Before:                 Press Enter after "a":        Result (auto-renumbered live):
1. a|                   1. a                          1. a
2. b                    2. |                          2.
3. c                    2. b                           3. b
                         3. c                           4. c
```

**Fit for Oxidown — SPLIT: view-computed display, not core-rewritten source.** See §0/§5 — recommend a computed-number widget decoration over core-initiated renumbering splices.
**Priority: P1** as specified in §0, but explicitly **not** by copying Obsidian's implementation strategy.

### 1.3 Enter to continue a list

Confirmed default: Enter at the end of an item's text inserts the next marker. The only official evidence is a bug-fix note that frames it as pre-existing default behavior: *"Pressing Enter in a multi-line list item now continues the list properly"* ([v1.8.3 changelog](https://obsidian.md/changelog/2025-01-30-desktop-v1.8.3/)).

```
- Buy milk|            Enter →      - Buy milk
                                    - |
1. First|              Enter →      1. First
                                    2. |
```

Checkbox continuation (new item gets its own empty `- [ ]`) is extremely well-attested informally (third-party guides) but **not pinned to an official verbatim example** — flagging as core-but-only-medium-confidence-sourced.

**Fit for Oxidown — CORE.** `plan.md` §5.8 already names *"list-continuation-on-return"* as an anticipated command. This wants to be a core `continueListItem`-style transform triggered by the view's Enter-key handler (CM6's own history/markdown keymaps are disabled per `plan.md` §7.1, so Enter must be explicitly routed to the core — nothing does this for free). Should also cover task items (empty `- [ ] ` on continuation) since `toggleTask` already exists and the overlay already models task items distinctly.
**Priority: P1** — table-stakes list UX, cheap given the parser already distinguishes list-item boundaries and marker kind.

### 1.4 Enter on an EMPTY list item

Core Obsidian requires **two** Enter presses to fully exit a list from an empty item — confirmed directly by WhiteNoise in [a 2023 thread](https://forum.obsidian.md/t/cannot-exit-the-automatic-list-after-pressing-enter-twice/54358), who frames it as consistent-but-improvable markdown paragraph semantics (*"Enter=New Paragraph... could be improved by detecting that particular case"*); an older thread, ["Pushing enter after a checkbox/list to exit it"](https://forum.obsidian.md/t/pushing-enter-after-a-checkbox-list-to-exit-it-horrible-wording/150), documents the same two-step pattern from the user side.

```
- Item one              Enter once →   - Item one            Enter again →   - Item one
- |  (empty item)                                    <- blank line, still
                                                          list-adjacent                       <- now a plain paragraph
```

Exactly **how a nested empty item behaves — outdent-per-level before clearing, vs. the flat double-Enter pattern above — is UNVERIFIED**; no source spells out the step-by-step mechanic for a nested case. obsidian-outliner's "Enhance the Enter key" (default on) replaces this with a single, immediate one-level outdent per Enter on an empty item, explicitly framed as making Obsidian *"behave the same as other outliners"* (README) — i.e., a Workflowy/Roam-style mechanic, implying core's default really is the clunkier double-Enter/blank-line pattern the forum threads describe.

**Fit for Oxidown — CORE, and a clear opportunity to ship the better mechanic, not Obsidian's.** This is the same `continueListItem`/Enter-dispatch command as §1.3, branching on "is this item's text empty": outdent one level if nested, else clear the marker and drop to a plain paragraph — single Enter, no double-press quirk, no discoverability tax. It's a pure text transform over data the core already has (item depth, marker span).
**Priority: P1** — same command surface as §1.3; the single-outdent design is strictly better UX for near-zero extra design cost, and this is exactly the kind of deterministic edge case a fresh core, unconstrained by legacy default, should just get right.

### 1.5 Backspace at/inside a marker

No official documentation covers the exact single-keystroke scenario. Adjacent, sourced data points: Cmd/Ctrl+Backspace at item-start deletes through the bullet **and** all leading indentation in one stroke ([forum](https://forum.obsidian.md/t/command-backspace-in-a-list-should-stop-at-the-bullet/74850), no dev reply); plain Backspace in leading whitespace removes an entire indent level in one press, not gradually ([forum](https://forum.obsidian.md/t/backspace-deletes-all-indent-in-one-go-can-i-make-it-step-back-gradually/104451)). obsidian-outliner's "Stick the cursor to the content" feature (default on) — *"Don't let the cursor move to the bullet position. Affects cursor movement, text deletion, text selection"* — implies that **without** the plugin, the cursor *can* enter the marker region and Backspace deletes ordinary single characters there, but **no source states the literal keystroke-by-keystroke outcome for stock core** — treat as UNVERIFIED inference, not a confirmed fact. A related outliner issue, [#77](https://github.com/vslinko/obsidian-outliner/issues/77), only states the *desired* behavior (*"bullet mark should be removed converting the item to normal text"*), not what core actually does.

**Fit for Oxidown — CORE.** Given the "marker-width-aware" framing of the current Tab work, Backspace-at-marker-boundary should be handled the same way: delete the whole marker atomically in one keystroke (dash + required trailing space, or `N. `), converting the line to a plain paragraph — consistent with treating the marker as one indivisible unit everywhere else in this system (widget:bullet, list-marker reveal-in-lockstep). This avoids replicating Obsidian's apparently inconsistent, undocumented behavior.
**Priority: P2** — nice consistency win, lower urgency than Tab/Enter since it's a narrower edge case (single-character-at-a-time backspace into a marker is a rare gesture compared to Tab/Enter, which fire on every list edit).

### 1.6 Checkbox toggle shortcut

Core command **"Toggle checkbox status"**, default hotkey **Ctrl+L / Cmd+L** — not Ctrl/Cmd+Enter (that opens links). Official FAQ, [v1.0.0 changelog](https://obsidian.md/changelog/2022-10-13-desktop-v1.0.0/): *"The default hotkey for 'Toggle checkbox status' has been changed to Ctrl+L (or Cmd+L on MacOS). Ctrl+Enter is now the default hotkey for opening links under the cursor in a new tab."* Running the command on a plain bullet **converts it into a task** rather than no-op'ing ([forum](https://forum.obsidian.md/t/toggle-checkbox-status-removing-bullet-pointst/45383)). Click-to-toggle in Live Preview works (confirmed indirectly: Tasks-plugin bug reports contrast their *own* extra logic failing there against the base checkbox toggling fine — [obsidian-tasks#3148](https://github.com/obsidian-tasks-group/obsidian-tasks/issues/3148), [#455](https://github.com/obsidian-tasks-group/obsidian-tasks/issues/455)). **Whether the cursor must sit exactly on the brackets vs. anywhere on the line is UNVERIFIED.**

**Fit for Oxidown — already CORE, done.** `boundary-v0.md` already specifies `command(name: "toggleTask", pos: number)` with "pos anywhere in the list item" — this is *more* permissive than what's confirmed of Obsidian and needs no design change. The only open question is the view-side keybinding: bind Cmd/Ctrl+L to match muscle memory (and note the Cmd/Ctrl+Enter slot is "spoken for" by link-opening in Obsidian's convention, worth deciding deliberately rather than by accident).
**Priority: P1** — trivial, already-built, just wire the keymap.

### 1.7 Task-state cycling

**Not core beyond a fixed 3-state cycle.** Core's only relevant command, **"Cycle bullet/checkbox"**, shipped [v0.13.27](https://obsidian.md/changelog/2022-02-28-desktop-v0.13.27/): *"cycles between regular bullet, checkbox, and checked checkbox"* — bullet → `[ ]` → `[x]`, no default hotkey ([forum](https://forum.obsidian.md/t/add-toggle-bullet-checkbox-hotkey/35186)). Separately, core *renders* any single character inside `[ ]` as "done"-styled (obsidian.md/help/syntax: *"You can use any character inside the brackets to mark it as complete"* — `[x]`, `[?]`, `[-]` all render complete) but this is syntax tolerance, not a state machine: there is no core settings UI for an ordered custom-state sequence, and toggling a custom-marked checkbox back on always collapses to plain `[x]`, losing the custom marker — an open, unresolved gap ([forum](https://forum.obsidian.md/t/preserve-marker-type-when-toggling-custom-checkbox/93384)). True N-state cycling (in-progress/cancelled/etc.) lives entirely in the **Tasks plugin's** configurable status settings ([docs](https://publish.obsidian.md/tasks/Getting+Started/Statuses/Status+Settings)) plus theme CSS packages (e.g. ITS Theme's alternate checkboxes) — confirmed plugin territory, not core, after an exhaustive changelog/help search turned up nothing further.

**Fit for Oxidown — CORE for the 3-state cycle, explicitly PLUGIN/extension territory beyond that.** A `cycleTask`/`cycleBullet` command mirroring Obsidian's fixed 3-state cycle is cheap and matches the existing `toggleTask` shape. Open-ended custom task states are exactly the kind of "extension must be syntax, never schema" case `plan.md` §2 already reserves for the Phase-B parser-plugin mechanism — don't build a bespoke state-table in M1's core.
**Priority: P2** for the 3-state cycle (small, complements `toggleTask`, not urgent); **P3 (skip for now)** for custom N-state cycling — revisit only alongside the general extension-syntax mechanism.

---

## 2. Blockquote editing

### 2.1 Enter inside a quote — continue and exit

Enter at the end of a non-empty quoted line continues with `> ` — confirmed as intended default via a regression-fix note, [v1.8.4 changelog](https://obsidian.md/changelog/2025-01-31-desktop-v1.8.4/): *"Fixed bug where pressing Enter inside a blockquote would not continue the blockquote onto the next line."* This is gated by the same **"Smart lists"** setting as list continuation — when disabled, a still-open bug shows blockquote/checkbox continuation on Enter breaking entirely ([forum](https://forum.obsidian.md/t/disable-smart-lists-causing-blockquote-not-add-on-entering/97720)), meaning Obsidian bundles list-continuation, blockquote-continuation, and renumbering under **one internal toggle**, not three independent features.

Enter on an *empty* quote line (`> ` alone) exits the blockquote, dropping to a plain paragraph — a long-standing, still-unimplemented feature request describes the current behavior as a side effect users don't want in the other direction (they want to type a truly blank line *inside* a multi-paragraph quote): *"If I type in `>` and Enter, the `>` disappears"* ([forum](https://forum.obsidian.md/t/allow-typing-empty-line-in-block-quotes/29746)). The documented workaround for a blank line *within* one quote is **Shift+Enter** ([forum](https://forum.obsidian.md/t/command-to-insert-newline-without-changing-blockquote-level/45390)).

```
> some text|          Enter →      > some text        > first line          Enter →   > first line
                                    > |                > |  (empty)
                                                                                        |  (exited quote)
```

Two temporary regressions are worth flagging as a caution about how brittle text-level list/quote heuristics are: Enter inside a *nested* quote briefly closed **all** levels at once instead of stepping down one ([forum](https://forum.obsidian.md/t/blockquote-no-longer-maintains-lowers-level-after-normal-enter-but-closes-it/95767)), and — only reproducible when a list existed earlier in the same document — Enter after `>>a` converted the next line into a bulleted/numbered list instead of continuing the quote ([forum](https://forum.obsidian.md/t/nested-blockquotes-becoming-lists-on-newline/95842)). Both read as classic "regex/heuristic leaked state from an unrelated construct" bugs.

**Fit for Oxidown — CORE, unify with §1.3/§1.4 under one Enter-dispatch.** `boundary-v0.md`'s v0.2 decoration model already treats the blockquote run and the list-marker run on a mixed line (`> > - item`) as **independent, per-construct extents** (see the "piecewise, construct by construct" rule). The Enter-key command should follow the same discipline: resolve what construct(s) the cursor sits inside from the overlay, and dispatch continuation/exit logic per construct rather than by line-level regex — which is structurally why Obsidian's two regressions above happened (their heuristic conflated "is this a list line" and "is this a quote line" instead of asking the parse tree). Empty-line exit should be single-Enter (see §1.4's reasoning) rather than replicating the double-Enter quirk.
**Priority: P1** — same command family as §1.3/§1.4, and the "ask the overlay, not a regex" principle it validates is broadly important.

### 2.2 Removing quote markers (stepped outdent)

**No dedicated core shortcut** to step a nested quote down one level (`> > > x` → `> > x`). Core's only related affordance, **"Toggle Blockquote"**, is an on/off toggle for one level, not a stepped outdent for arbitrary depth — confirmed by a 2022 feature request calling it *"insufficient as it's designed for only one level of quoting"* ([forum](https://forum.obsidian.md/t/commands-for-increasing-and-decreasing-blockquote-level/42778)). The gap is filled entirely by the third-party **"Blockquote Levels"** plugin (czottmann), whose Increase/Decrease commands are explicitly modeled on list Tab/Shift-Tab ([obsidianstats](https://www.obsidianstats.com/plugins/blockquote-levels), [repo](https://github.com/czottmann/obsidian-blockquote-levels)) — meaning Shift-Tab does **not** natively decrease blockquote level in core (Shift-Tab is list-reserved). Whether plain Backspace at a quote-line start deletes one `>` at a time is **UNVERIFIED** — no source addresses it either way. A related, now-fixed bug: "Toggle blockquote" applied twice used to permanently corrupt list indentation instead of restoring it ([forum](https://forum.obsidian.md/t/toggle-blockquote-strips-indentation-destroying-list-structure/41431), fixed v0.16).

**Fit for Oxidown — CORE, and a place to simply be better than stock Obsidian.** Since `boundary-v0.md` already models blockquote `depth` as first-class per-line data (v0.2 `DecorationLineV2` with a `depth` field), a stepped `indentBlockquote`/`outdentBlockquote` command (Tab/Shift-Tab when the cursor is in the quote-marker run rather than a list marker) is nearly free to implement correctly — we don't have Obsidian's excuse of Shift-Tab being "already spoken for" by lists only, because our per-construct reveal model already disambiguates which run the cursor is touching.
**Priority: P2** — real quality-of-life gap Obsidian still hasn't closed natively, but blockquote nesting depth is a less common editing gesture than list Tab/Enter; land after §1's list commands ship.

### 2.3 Blockquote + list combinations

A confirmed, long-running (2023 → still open mid-2025) core bug: Tab on a quoted line nested under a list item inserts the tab **after** the `>` instead of before it, breaking nesting — reported [July 2023](https://forum.obsidian.md/t/incorrect-indentation-behavior-with-quotes-and-item-lists/62714), still present in [June 2025](https://forum.obsidian.md/t/incorrect-indentation-behavior-with-quotes-and-item-lists/62714). Toggling blockquote/callout on an ordered or mixed nested list breaks numbering and flattens nesting ([forum](https://forum.obsidian.md/t/ordered-and-mixed-list-broken-when-toggling-blockquote-and-callout/97500), reported v1.8.9). Pasting a list (or multiple items) directly into a callout box was, as of a 2023 report, simply unsupported — the documented workaround is paste-then-select-then-toggle-blockquote from the command palette ([forum](https://forum.obsidian.md/t/callout-pasting-multiple-items/51330)).

```
Expected (Tab on the quote line):        Actual bug (tab lands after ">"):
- list item 1                            - list item 1
    > this is the quote                  >     this is the quote
- list item 2                            - list item 2
```

**Fit for Oxidown — CORE; this is the strongest evidence in this whole report for "ask the overlay, not the raw text."** All three bugs above trace to the same root cause: naive text-level insertion logic (insert N spaces "at column X") without construct-aware knowledge of where the `>` run ends and the list-marker run begins on a mixed line. `boundary-v0.md`'s v0.2 spec was *already written* to treat these as separate, independently-revealed extents on the same line (the mixed-line rules under "revealed: true on blockquote/list-item lines") specifically to avoid this class of bug — this research is a direct, concrete validation that the spec's piecewise-construct model is the right call, not over-engineering.
**Priority: P1 (as validation, not new work)** — no new command needed; flag as a must-pass test case once Tab/indent commands exist: "Tab on a quoted line nested under a list item must indent inside the quote run, not push the `>` itself."

---

## 3. Paste behavior

### 3.1 Multi-line plain text pasted into a list item

Confirmed: neither clean outcome happens in core Obsidian. A [2025 forum report](https://forum.obsidian.md/t/pasting-multi-line-text-into-sub-bullet-breaks-out-of-bullets/105837) shows pasting multi-line plain text inside a list item does **not** turn each line into a new list item, and does **not** cleanly nest the extra lines as continuation text inside the one item either — the **first newline breaks out of the bullet entirely**, dumping every subsequent pasted line as unindented plain-paragraph text below/outside the list (the user explicitly contrasts this with Logseq, which preserves block depth on paste). The **obsidian-smarter-paste** plugin exists specifically to fix the analogous blockquote case (*"the appropriate syntax will be applied to all lines pasted"* — [repo](https://github.com/chrisgrieser/obsidian-smarter-paste)), confirming stock paste does not extend list/quote syntax across pasted lines. A 2020 feature request to auto-convert pasted plain-text lines into list items remains open, unimplemented ([forum](https://forum.obsidian.md/t/convert-lines-of-text-into-a-list/952)).

```
Cursor: - foo|                 Paste clipboard:            Result (broken):
                                bar                          - foo
                                baz                          bar          <- outside the list, unindented
                                                              baz          <- outside the list, unindented
```

**Fit for Oxidown — CORE, genuinely ownable improvement.** `boundary-v0.md` already distinguishes `EditOrigin: "paste"` from `"user"` (today used only to force an undo-group break). Extending that distinction so paste *inside* a list/quote context runs through a context-aware transform (continue the marker for each pasted line, matching the item's kind — bullet, ordered, task) is a natural `command`-style text transform, not a raw insert-splice — this is precisely the kind of overlay-aware operation the core is positioned to do well, and Obsidian's multi-year-unfixed gap here is a low bar to clear.
**Priority: P2** — clear win, but paste-into-list is a narrower gesture than Tab/Enter; sequence after the core Enter/Tab commands land since it likely shares the same "what construct/marker kind is this line" primitive.

### 3.2 Pasting a list into a list

Confirmed, long-standing (2021 → present), commonly reported core bug: pasting markdown-list text at the start of an existing (often freshly auto-created) list line produces a literal **duplicated marker** rather than merging into the destination context — `- ` + pasted `- Last met on Tuesday` → `- - Last met on Tuesday` (checkbox variant: `- [ ] - [ ] do chores`). First reported [April 2021](https://forum.obsidian.md/t/automatically-de-duplicate-pasted-bulleted-lists/17386), still reproducing per [2022 follow-ups](https://forum.obsidian.md/t/bullet-lists/65225); documented workarounds are move-lines-with-Ctrl+Up/Down instead of cut/paste, or a manual find/replace. A related, more specific bug: pasting a nested numbered list does not reset child numbering under the new parent — it continues sequentially from the previous parent's last child ([forum](https://forum.obsidian.md/t/nested-numbered-lists-not-numbered-correctly-after-pasting/99389), reported April 2025, acknowledged by WhiteNoise as "will be fixed 1.9" with no confirmed landing found in this research). Cross-hierarchy paste (e.g. from Word) can also sever parent/child relationships and mangle bullet styles ([forum](https://forum.obsidian.md/t/copy-paste-maintain-bullet-list-hierarchy/94763)).

```
Cursor on fresh line: - |          Clipboard: - Last met on Tuesday      Result: - - Last met on Tuesday
```

**Fit for Oxidown — CORE, same root fix as §3.1.** A paste-aware insertion path that recognizes "the pasted text is itself a list" and re-anchors it against the destination's existing marker/depth (strip a redundant leading marker rather than concatenating two) is the same primitive as §3.1 — the core already owns marker-kind detection for decorations, so reusing that for paste normalization is incremental, not new machinery.
**Priority: P1** — this specific bug (literal doubled markers) is embarrassing, four-plus years old, and trivially avoidable if paste is routed through the overlay instead of treated as a blind clipboard-verbatim splice; cheap structural win to bake in from the start rather than retrofit.

### 3.3 Pasting a URL over selected text — auto-link creation

**Confirmed core feature, shipped recently**: Obsidian [v1.11.4](https://obsidian.md/changelog/2026-01-12-desktop-v1.11.4/) (Jan 2026): *"When text is selected, pasting a URL into the editor will convert the selection into a Markdown link using the URL (e.g. `[selected text](pasted URL)`)."* Extended to multi-cursor in [v1.12.5](https://obsidian.md/changelog/2026-03-05-desktop-v1.12.5/) (March 2026). This sat as an unimplemented, frequently-bumped forum request from **July 2020 through at least May 2024** ([forum](https://forum.obsidian.md/t/automatically-insert-markdown-formatted-link-if-text-is-selected-when-pasting/3646): *"such a basic feature [that is] not yet available... after almost 4 years"*) — before shipping, it was community-plugin territory (`obsidian-url-into-selection`). **Whether a dedicated settings toggle exists to disable it is UNVERIFIED** — no help-doc entry found for one.

```
Select "my website" (clipboard = https://example.com)     Paste     →     [my website](https://example.com)
```

**Fit for Oxidown — CORE, cheap, and self-contained.** This is a small, well-scoped transform: on a paste whose payload is a bare URL and whose target is a non-empty selection, emit a link-wrapping splice instead of an overwrite — essentially a variant of the existing `toggleStrong`/`toggleEm`-style command shape (wrap selection in delimiters), just triggered by paste instead of a keybinding, and gated on "does the clipboard payload parse as a bare URL."
**Priority: P1** — small, isolated, no architectural tension, good user-visible payoff; safe to build alongside the other paste-normalization work in §3.1/3.2 since all three touch the same paste-interception point.

---

## 4. Other notable patterns

### 4.1 Smart Home key

Confirmed core, list/task-only: since [v0.16.0](https://obsidian.md/changelog/2022-08-30-desktop-v0.16.0/), Home moves the cursor to the start of a list item's *content*, past the marker (`- aa bb|` → Home → `- |aa bb`); task-line Home-past-checkbox regressed and was re-fixed in [v1.1.0](https://obsidian.md/changelog/2022-12-05-desktop-v1.1.0/). This is layered *on top of* CM6's own default keymap, which already binds Home to a whitespace-only smart-home (`moveByLineBoundary`: *"If the line is indented... this will move to the end of the indentation instead of the start of the line"* — [CM6 commands source](https://github.com/codemirror/commands/blob/main/src/commands.ts)) — Obsidian's marker-skip is custom engineering beyond stock CM6, evidenced by the dedicated changelog entries. **Two things are UNVERIFIED**: whether a second Home press on a list line falls through to true column 0 (the classic two-stage pattern), and whether blockquote `> ` markers get any Home handling at all — no source addresses either, and the existence of a third-party plugin (`obsidian-homekey-plugin`) specifically extending smart-Home to quotes/headings/footnotes is circumstantial evidence that core's native scope really is list/task lines only.

**Fit for Oxidown — VIEW-side, no core round-trip needed.** The view already receives everything required to compute "where does content start on this line" from decorations already specified: `widget:bullet`'s `to` offset, `mark:list-marker`'s span (which per `boundary-v0.md` clarification #3 already includes the required trailing whitespace), and the blockquote line decoration's presence/depth. A CM6-side Home-key handler can walk the already-fetched decoration set for the current line with zero extra core calls. This is also a chance to close Obsidian's own gap: since we already decorate blockquote runs as a distinct, addressable extent (unlike Obsidian), smart-Home can correctly skip `> ` prefixes too, for free.
**Priority: P2** — good polish, purely view-side, no new core surface, but not blocking anything else; sequence whenever the view team has a slow week.

### 4.2 Fold indicators / folding

Confirmed base-editor CORE feature (not a plugin) since [v0.5.0](https://obsidian.md/changelog/2020-05-10-desktop-v0.5.0/); official page [obsidian.md/help/folding](https://obsidian.md/help/folding) describes the gutter arrow gesture; two independent settings gate it (**Fold heading**, **Fold indent** — obsidian.md/help/settings). Notably, **fold scope differs by view mode**: in Live Preview, folding a list item hides everything under its first line — nested sub-items *and* any trailing indented paragraph text belonging to that item — while Reading view only folds the sub-list, leaving trailing paragraph text visible ([forum](https://forum.obsidian.md/t/59227) — direct quote: *"Live Preview folds everything under the first line of the list item, while Reading View folds only the sub-lists"*). Heading fold extends until the next heading of same-or-higher level, including any sub-headings' own content (confirmed via a staff-acknowledged regression, [forum](https://forum.obsidian.md/t/only-first-paragraph-bullet-point-is-folded-when-switching-from-reader-editor/95988)); a horizontal rule does **not** break a heading fold, a still-open request ([forum](https://forum.obsidian.md/t/dont-fold-over-horizontal-rule-line-break/16079)). Fold *state* persistence is real but flaky/undocumented in exact mechanism (assumed workspace-local, not portable, complaints span [2018](https://forum.obsidian.md/t/persist-list-and-header-folding-after-leaving-page/2754)–[2023](https://forum.obsidian.md/t/remember-fold-collapse-status-of-headings-and-lists/32821)).

**Fit for Oxidown — SPLIT, and worth calling out explicitly since the task asked for view-side flags.** *Fold-range computation* — "given this line, what's the foldable extent" — is a CORE query, because it needs the same block-index/list-item-depth/heading-level data the overlay already computes for decorations (a `foldRange(pos) -> {from, to}`-style query is a thin wrapper over existing structures, not new parsing work). *Fold state* (which ranges are currently collapsed, and rendering the collapsed placeholder) is inherently **VIEW-side** UI state — CM6 ships a folding gutter/extension for exactly this, and fold state is ephemeral/session-local by design in every editor surveyed (nothing here should live in the op log or the document). Persisting fold state across sessions, if desired, is a sidecar-store concern (`plan.md` §5.5), not core-document state.
**Priority: P2** — nice, cheap to spec now (the query shape is obvious given existing overlay data) but not required for the current Tab/list milestone; land the `foldRange` query opportunistically alongside other overlay-extent queries (see §4.5).

### 4.3 Drag handles for list reordering

**Core Obsidian has no mouse drag-to-reorder or drag-to-reparent for list items** — confirmed by absence: unimplemented feature requests spanning 2020–2025 ([2020](https://forum.obsidian.md/t/reorder-bullet-points-items-in-lists-with-mouse-drag-and-drop/1307), [2025](https://forum.obsidian.md/t/add-native-drag-and-drop-sorting-across-obsidian/106149)) and a direct "no native drag" answer on the forum ([thread](https://forum.obsidian.md/t/how-can-i-reorder-list-items/86981)). Core's only native drag-reorder is the **Outline** panel (sidebar TOC), and only for whole document *sections* by heading, not bullets ([obsidian.md/help/Plugins/Outline](https://obsidian.md/help/Plugins/Outline)). The real, mouse-driven, mid-drag reparent-by-horizontal-movement experience is entirely **obsidian-outliner** territory (drag-and-drop shipped experimental v4.5.0, default-on v4.7.0 — [releases](https://github.com/vslinko/obsidian-outliner/releases)), and it's fragile: recurring version-specific breakage through 2026 (issues [#444](https://github.com/vslinko/obsidian-outliner/issues/444), [#480](https://github.com/vslinko/obsidian-outliner/issues/480), [#588](https://github.com/vslinko/obsidian-outliner/issues/588)), desktop-only. GitHub's own task-list editor, by contrast, ships a polished **native six-dot drag handle** for flat task-list reordering ([docs](https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/about-tasklists)), and Milkdown's `@milkdown/plugin-block` unifies drag-handle *and* whole-block selection into one gesture (mousedown on the handle both selects the block and starts the drag — [docs](https://milkdown.dev/docs/api/plugin-block)).

**Fit for Oxidown — SPLIT, and cheap if built on top of §1.1's commands.** The drag *gesture* (pointer tracking, drop-target highlighting, reparent-by-horizontal-offset) is inherently view-side UI. But the actual *mutation* it produces — "move this item (and its subtree) to after/before/inside that item" — should be expressed as calls into the same `indentListItem`/`outdentListItem`/a new `moveListItem` core command family from §1.1, never as ad hoc view-side text surgery. Framed this way, a drag handle is "just" a mouse front-end for commands we need to build anyway.
**Priority: P3** for now (Obsidian itself only gets this via a fragile plugin; not core-Obsidian table stakes) but **worth designing the core `moveListItem` command's signature generously enough now** that a future drag-handle view feature is additive, not a rework.

### 4.4 Auto-pairing of markdown delimiters

**Both halves confirmed core, one setting** (Settings → Editor → "Auto-pair Markdown syntax": *"Pair symbols automatically for bold, italic, code, and more"*). **Wrap-selection-by-typing** (select `word`, press `*` once → `*word*`) is confirmed by developer **Licat** in a forum reply noting CodeMirror *"exposes this functionality as a single configuration"* ([forum](https://forum.obsidian.md/t/rename-or-preferably-split-auto-pair-markdown-syntax/5581)) — i.e. one toggle controls both wrap-on-type-over-selection and auto-close-on-empty-selection, and users who only want one behavior (a repeatedly requested split) have been declined. **Auto-close with nothing selected** (`*` → `*|*`, cursor between) is separately confirmed ([forum](https://forum.obsidian.md/t/auto-pair-markdown-syntax-malfunction/20811)); backtick auto-pair specifically requires adjacent whitespace, a staff-confirmed constraint not a bug (*"Yes you need a space"* — [forum](https://forum.obsidian.md/t/inline-code-backtick-not-auto-pairing/111943)). Separately, **Cmd/Ctrl+B / Cmd/Ctrl+I always wrap the selection regardless of the setting**, and since v0.14.4 a repeat-press "smartly skips over" an existing closing marker instead of adding a second one ([forum](https://forum.obsidian.md/t/make-the-hotkey-for-bold-and-italic-behave-like-in-document-processors-word-google-docs/32239)). Typora ships the split Obsidian declined: separate preferences for bracket-pairing vs. markdown-syntax-pairing, and for `~`/`=`/`^` specifically *only* wrap-on-selection fires, no lone-character auto-close ([Typora docs](https://support.typora.io/Auto-Pair/)).

```
Select "hello", type *  →  *hello*          Type * with nothing selected  →  *|*  (cursor between)
```

**Fit for Oxidown — mostly CORE, and mostly already built.** The wrap-selection half is **not new work**: it's the existing `toggleStrong`/`toggleEm`/`toggleCode`/`toggleStrike` commands (`boundary-v0.md` command list), just dispatched from a different input event — the view's keydown handler intercepts `*`/`_`/`` ` ``/`~` when the current selection is non-empty and calls the matching `command(...)` instead of inserting the literal character. The empty-selection auto-close half (bare `*` → `*|*`) has no markdown semantics yet (it's just a mirrored-character insert until a matching close appears) and is naturally **VIEW-side**, using CM6's own `closeBrackets`-style extension configured for markdown delimiter characters — no core call needed for that half.
**Priority: P1** for wrap-selection (it's free — reuse existing commands, just add the keymap layer) and **P2** for empty-selection auto-close (view-only, small, no dependency on anything else).

### 4.5 Selecting a whole list item / line-select gestures

**No gutter/margin click-to-select exists in core** — confirmed by absence via two unimplemented, unanswered feature requests ([2021](https://forum.obsidian.md/t/select-by-dragging-clicking-in-the-gutter/15628), [2023](https://forum.obsidian.md/t/add-ability-to-select-entire-list-item-by-clicking-to-left-of-item-number/52014)). Triple-click does select the paragraph/line under the cursor, including its marker text ([forum](https://forum.obsidian.md/t/hotkey-for-select-curent-line/7009)); whether double-click still grabs the whole line-plus-marker (an old, disputed-as-a-bug report — [forum](https://forum.obsidian.md/t/double-clicking-sentence-in-a-list-grabs-entire-line-including-list-item-marker/39983)) is **UNVERIFIED for current versions**. obsidian-outliner's enhanced Cmd/Ctrl+A cycles current-item → whole-list on repeated presses (README) — again plugin territory filling a native gap. Milkdown's block-handle mousedown (§4.3) is the standout alternative: one gesture produces both a full-node selection and a drag-start.

**Fit for Oxidown — mostly VIEW, with one small CORE dependency.** Selection mechanics themselves are CM6/view concerns. But "select this whole list item including nested children" needs to know the item's full extent (start of marker through the end of its deepest descendant) — the same data `foldRange`/subtree-Tab (§1.1, §4.2) already need. Recommend exposing that as one shared overlay query (`itemExtent(pos)`) rather than three separate ad hoc computations for indent, fold, and select.
**Priority: P3** — genuinely nice-to-have, no user-facing urgency, but cheap to fold into the same query as §1.1/§4.2 if those are being built anyway.

### 4.6 Mod-click (Cmd/Ctrl-click)

Confirmed core, deliberately non-conflicting with multi-cursor: Mod-click follows a link, opening it in a **new tab** (obsidian.md/help/tabs); multi-cursor is a **separate** modifier, Alt/Option-click (obsidian.md/help/multiple-cursors) — an explicit design choice per WhiteNoise: *"alt-click can't be used because it's for multiple cursors"* ([forum](https://forum.obsidian.md/t/a-key-command-to-select-a-link-without-opening-it-in-live-preview/33628)). A community plugin exists specifically to *invert* the default (require Mod-click to follow instead of plain click), confirming plain-click-follows-link is core's actual default and Mod-click's role is "open in new tab," not "follow at all" ([obsidian-ctrl-click-links](https://github.com/eikowagenknecht/obsidian-ctrl-click-links)).

**Fit for Oxidown — VIEW-side entirely, no new core surface.** This is pure CM6 event-handling (which modifier does what gesture) over data the core already exposes via `mark:link`/`mark:url` decorations — the core doesn't need to know about mouse modifiers at all.
**Priority: P3 / not really "borrowing" anything** — straightforward to replicate whenever link-opening is wired up; not a pattern that needs core design attention.

---

## 5. Architecture implications (cutting across the above)

1. **Extend the ordered-marker decoration to carry a computed sequence number, and make it a widget rather than a plain style-over-source mark.** Today's spec (`boundary-v0.md` v0.2) has `mark:list-marker` styling the literal source digits; nothing computes "what number should this display as." Recommend the overlay emit each ordered item's position-in-run alongside the marker span, and the view render *that* (like `widget:bullet` already does for unordered markers) rather than trusting the literal typed digits. This turns Obsidian's riskiest new feature (renumber-by-rewriting-the-file) into a free, byte-preserving view computation — directly serving `plan.md`'s "clean git diffs are a feature" principle instead of fighting it.
2. **Route Enter/Tab/Backspace list-and-quote handling through one "what construct is the cursor touching" primitive**, not per-key regex heuristics. §2.3's Tab-after-`>` bug and §2.1's two Enter regressions are exactly the failure mode `boundary-v0.md`'s v0.2 piecewise-construct reveal model was written to avoid for decorations — the same discipline needs to extend to editing commands, which currently aren't specified at all beyond `toggleTask`/`toggleStrong`-family. Concretely: `indentListItem`, `outdentListItem`, `continueListItem` (Enter), `outdentBlockquote`, and paste-normalization should all share one "resolve constructs at position" core query rather than reimplementing line-parsing five times.
3. **Give `EditOrigin: "paste"` real behavioral teeth, not just undo-group semantics.** It exists today only to force an undo break; §3.1–3.3 all want a paste-time hook that can rewrite the inbound splice contextually (continue list markers across pasted lines, strip a redundant marker when pasting a list into a list, wrap a bare-URL paste over a selection as a link). This is a natural, bounded extension of the existing origin-tagging design, not a new subsystem.
4. **A handful of overlay queries (`foldRange`, `itemExtent`) are cheap to add once, since they all read the same per-line depth/block data the decoration emitter already computes** — worth specifying their shapes now even though the consuming features (folding, drag-select) are P2/P3, so the underlying overlay data model doesn't need revisiting later.

---

## 6. Priority roadmap

| # | Pattern | Obsidian status | Fit | Priority | Why |
|---|---|---|---|---|---|
| 1.1 | Tab/Shift-Tab: subtree-aware indent | Core: line-only (bug); plugin: subtree | CORE | **P1** | Directly in flight; overlay already has depth data; strictly better than the reference |
| 1.2 | Ordered-list renumbering | Core (v1.8.3), rewrites source, buggy | VIEW (computed number) | **P1** | Avoids invariant-2 conflict entirely; better than Obsidian's own approach |
| 1.3 | Enter continues list marker | Core | CORE | **P1** | Table stakes; already named in plan.md §5.8 |
| 1.4 | Enter on empty item: outdent-then-clear | Core (worse: double-Enter) | CORE | **P1** | Same command as 1.3; ship the better single-Enter mechanic |
| 1.5 | Backspace deletes marker atomically | Core (unverified/inconsistent) | CORE | **P2** | Consistency win, narrower gesture than Tab/Enter |
| 1.6 | Checkbox toggle keybinding (Cmd/Ctrl+L) | Core | CORE (already built) | **P1** | `toggleTask` exists; just wire the keymap |
| 1.7 | 3-state bullet/checkbox cycle | Core | CORE | **P2** | Cheap, complements `toggleTask` |
| 1.7b | N-state custom task cycling | Plugin (Tasks) | extension mechanism | **P3** | Explicitly plugin territory; revisit with Phase-B extension syntax |
| 2.1 | Blockquote Enter continue/exit | Core | CORE | **P1** | Same Enter-dispatch family as 1.3/1.4 |
| 2.2 | Stepped blockquote outdent | Plugin only (core lacks it) | CORE | **P2** | Real gap Obsidian hasn't closed; land after list commands |
| 2.3 | Quote+list Tab must respect construct boundary | Core bug (unfixed 2+ yrs) | CORE (validation) | **P1** | Proof the piecewise-construct model in boundary-v0 is correct; add as a test case |
| 3.1 | Paste multi-line text continues list markers | Core: broken | CORE | **P2** | Clear win; sequence after Enter/Tab share the construct-detection primitive |
| 3.2 | Paste list into list: no duplicated markers | Core bug (4+ yrs open) | CORE | **P1** | Cheap, embarrassing bug to avoid; same primitive as 3.1 |
| 3.3 | Paste URL over selection → auto-link | Core (shipped 2026) | CORE | **P1** | Small, isolated, good payoff |
| 4.1 | Smart Home (skip marker/quote prefix) | Core (list/task only) | VIEW | **P2** | Decoration spans already carry what's needed; no core call |
| 4.2 | Fold range computation / fold state | Core (base editor) | SPLIT (core: range: query; view: state+render) | **P2** | Cheap once overlay depth data exists; not urgent |
| 4.3 | Drag handles for reorder/reparent | Plugin only, fragile | SPLIT (view: gesture; core: reuse move/indent commands) | **P3** | Design `moveListItem` signature generously now; build the handle later |
| 4.4a | Wrap-selection auto-pair (`*sel*` on typing `*`) | Core | CORE (reuse existing toggle commands) | **P1** | Free — just a new keymap layer over commands that already exist |
| 4.4b | Empty-selection auto-close (`*` → `*\|*`) | Core | VIEW (CM6 closeBrackets-style) | **P2** | No markdown semantics involved; purely mirrored-character insertion |
| 4.5 | Select whole item (gutter click / repeated Mod+A) | Not core (plugin only) | VIEW + shared `itemExtent` query | **P3** | Nice-to-have; piggyback on fold/indent's extent data |
| 4.6 | Mod-click follows link in new tab | Core | VIEW | **P3** | Straightforward, no core design needed |

---

## Sources consulted (representative, not exhaustive — full citations inline above)

Official: [obsidian.md/help/syntax](https://obsidian.md/help/syntax), [/help/settings](https://obsidian.md/help/settings), [/help/folding](https://obsidian.md/help/folding), [/help/tabs](https://obsidian.md/help/tabs), [/help/multiple-cursors](https://obsidian.md/help/multiple-cursors), [/help/Plugins/Outline](https://obsidian.md/help/Plugins/Outline), and changelogs from v0.5.0 through v1.12.5 (2020–2026). Forum: dozens of threads on forum.obsidian.md, prioritizing those with direct replies from team member **WhiteNoise** or moderators. Plugins: [obsidian-outliner](https://github.com/vslinko/obsidian-outliner), [Auto-List-Management-Obsidian](https://github.com/OmriLeviGit/Auto-List-Management-Obsidian), [obsidian-blockquote-levels](https://github.com/czottmann/obsidian-blockquote-levels), [obsidian-smarter-paste](https://github.com/chrisgrieser/obsidian-smarter-paste), [obsidian-url-into-selection](https://github.com/denolehov/obsidian-url-into-selection). Cross-editor comparison: [HyperMD](https://github.com/laobubu/HyperMD), [Milkdown plugin-block](https://milkdown.dev/docs/api/plugin-block), [Typora Auto-Pair docs](https://support.typora.io/Auto-Pair/), [GitHub markdown-toolbar-element](https://github.com/github/markdown-toolbar-element) and [tasklists docs](https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/about-tasklists).
