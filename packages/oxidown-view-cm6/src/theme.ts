import { EditorView } from "@codemirror/view";

/**
 * Oxidown base theme.
 *
 * Concealment is a CM6 REPLACE decoration (see extension.ts) — characters
 * stay in the document; only their rendering collapses. Because heading
 * sizes are set on the LINE element (`.ox-h1`…`.ox-h6`), revealed delimiters
 * inherit the line's font-size: line height does not change between
 * concealed and revealed states.
 *
 * `min-height: <line-height>em` on every line guards against collapse when a
 * line consists solely of concealed characters (em resolves against the
 * line's own font-size, so heading lines keep heading height).
 *
 * v0.2 (M1) additions follow the same discipline: blockquote/code-block/
 * code-fence/hr are LINE decorations (no font-size changes tied to reveal
 * state, so nothing shifts when the `> ` marker or checkbox reveals/conceals);
 * the code-fence and code-block lines deliberately share one font-family and
 * background treatment so a fenced block reads as one consistent unit
 * regardless of which lines happen to show revealed delimiters.
 */
const MONO_FONT =
  "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace";

export const oxidownTheme = EditorView.baseTheme({
  ".cm-content": {
    lineHeight: "1.5",
  },
  ".cm-line": {
    minHeight: "1.5em",
  },

  // Inline marks
  ".ox-strong": { fontWeight: "700" },
  ".ox-em": { fontStyle: "italic" },
  ".ox-code": {
    fontFamily: MONO_FONT,
    borderRadius: "3px",
  },
  "&light .ox-code": { backgroundColor: "rgba(0, 0, 0, 0.06)" },
  "&dark .ox-code": { backgroundColor: "rgba(255, 255, 255, 0.12)" },

  // Revealed delimiters: full size (inherit the line's font-size), dimmed.
  ".ox-delim": { opacity: "0.45" },

  // (Concealment is a CM6 replace decoration — see extension.ts. No CSS.)

  // Heading line decorations (font-size on the line, so delimiters inherit it)
  ".ox-h1": { fontSize: "1.6em", fontWeight: "700" },
  ".ox-h2": { fontSize: "1.45em", fontWeight: "700" },
  ".ox-h3": { fontSize: "1.3em", fontWeight: "650" },
  ".ox-h4": { fontSize: "1.15em", fontWeight: "650" },
  ".ox-h5": { fontSize: "1.05em", fontWeight: "600" },
  ".ox-h6": { fontSize: "1em", fontWeight: "600" },

  // --- v0.2 (M1) additions ---------------------------------------------

  // Strikethrough
  ".ox-strike": { textDecoration: "line-through" },

  // Links: text is always visible (mark:link); destination only appears
  // when revealed (mark:url). Neither changes font-size — no layout shift.
  ".ox-link": { textDecoration: "underline" },
  "&light .ox-link": { color: "#1a56b0" },
  "&dark .ox-link": { color: "#7db2ff" },
  ".ox-url": { opacity: "0.6", fontStyle: "italic" },

  // List markers. Ordered markers stay text ("1. ") and get a fixed-width,
  // right-aligned box with tabular numerals so single/double-digit items
  // align; the bullet widget (below) shares the same box metrics so
  // conceal↔reveal never shifts the item text.
  ".ox-list-marker": {
    opacity: "0.7",
    fontVariantNumeric: "tabular-nums",
    display: "inline-block",
    minWidth: "1.5em",
    textAlign: "right",
  },
  // Unordered bullet widget (replaces the raw "- " span when concealed).
  // The dot is a pure-CSS circle (`::before`), NOT a text glyph — so its
  // size, optical centering (vertical-align: middle), and the gap to the
  // item text are exact and font-independent. The box stays compact so
  // first-level items hug the margin.
  // The box matches `.ox-list-marker` (1.5em, right-aligned) so bullet,
  // number, and checkbox columns share one alignment, at every nesting level
  // (nested indent comes from the source's leading spaces, identically).
  ".ox-bullet": {
    display: "inline-block",
    minWidth: "1.5em",
    textAlign: "right",
    opacity: "0.85",
    // The caret adjacent to a widget inherits the widget box's rect height;
    // an uncapped inline-block strut is the full line height (24px), so the
    // caret rendered enlarged next to bullets. Cap the box near text height;
    // text-bottom keeps the dot optically centered (~0.6px off).
    height: "1.2em",
    verticalAlign: "text-bottom",
  },
  ".ox-bullet::before": {
    content: '""',
    display: "inline-block",
    width: "0.34em",
    height: "0.34em",
    borderRadius: "50%",
    backgroundColor: "currentColor",
    verticalAlign: "middle",
    marginRight: "0.5em",
  },

  // Blockquotes: one vertical bar PER nesting level, drawn as layered
  // background gradients at increasing offsets (a single border-left cannot
  // render nested bars). Never a font-size change, so the `> ` marker's
  // reveal/conceal cannot shift height.
  "&light .cm-content": {
    "--ox-bq-bar": "linear-gradient(rgba(0,0,0,0.28), rgba(0,0,0,0.28))",
    "--ox-hr-line": "linear-gradient(rgba(0,0,0,0.25), rgba(0,0,0,0.25))",
  },
  "&dark .cm-content": {
    "--ox-bq-bar": "linear-gradient(rgba(255,255,255,0.32), rgba(255,255,255,0.32))",
    "--ox-hr-line": "linear-gradient(rgba(255,255,255,0.28), rgba(255,255,255,0.28))",
  },
  ".ox-blockquote": {
    opacity: "0.85",
    backgroundRepeat: "no-repeat",
    backgroundSize: "3px 100%",
  },
  ".ox-bq-1": {
    backgroundImage: "var(--ox-bq-bar)",
    backgroundPosition: "0 0",
    paddingLeft: "0.9em",
  },
  ".ox-bq-2": {
    backgroundImage: "var(--ox-bq-bar), var(--ox-bq-bar)",
    backgroundPosition: "0 0, 0.8em 0",
    paddingLeft: "1.7em",
  },
  ".ox-bq-3": {
    backgroundImage: "var(--ox-bq-bar), var(--ox-bq-bar), var(--ox-bq-bar)",
    backgroundPosition: "0 0, 0.8em 0, 1.6em 0",
    paddingLeft: "2.5em",
  },

  // Fenced code blocks: fence + body share one font-family/background so the
  // whole block reads as one unit. No line-level opacity — it would wash out
  // the line BACKGROUND too; revealed fence text is dimmed by its delim mark.
  ".ox-code-fence": {
    fontFamily: MONO_FONT,
  },
  ".ox-code-block": {
    fontFamily: MONO_FONT,
  },
  "&light .ox-code-fence, &light .ox-code-block": {
    backgroundColor: "rgba(0, 0, 0, 0.045)",
  },
  "&dark .ox-code-fence, &dark .ox-code-block": {
    backgroundColor: "rgba(255, 255, 255, 0.08)",
  },
  // Inside fenced blocks the LINE carries the background — the inline code
  // mark must be transparent or every text run gets a double-shaded band.
  "&light .ox-code-block .ox-code, &light .ox-code-fence .ox-code": {
    backgroundColor: "transparent",
  },
  "&dark .ox-code-block .ox-code, &dark .ox-code-fence .ox-code": {
    backgroundColor: "transparent",
  },

  // List items (every depth): hanging indent — padding reserves the full
  // marker column for the depth and the negative text-indent pulls only the
  // FIRST line (the marker) back into it, so wrapped text aligns with the
  // first line's text. Nested raw indent whitespace conceals (depth >= 2)
  // and each nested marker starts at its parent's text column.
  // calc: our padding REPLACES the line's default (CM6 base: 6px left).
  ".ox-list-item": { textIndent: "-1.5em" },
  ".ox-li-1": { paddingLeft: "calc(6px + 1.5em)" },
  ".ox-li-2": { paddingLeft: "calc(6px + 3em)" },
  ".ox-li-3": { paddingLeft: "calc(6px + 4.5em)" },
  ".ox-li-4": { paddingLeft: "calc(6px + 6em)" },

  // Breathing room before a nested quote block (set on the parent line).
  ".ox-bq-gap": { paddingBottom: "4px" },

  // Source-mode line (marker region being edited): neutralize the marker
  // box so raw `- `/`1. ` render at their NATURAL width — any visible gap
  // is a real whitespace character, never phantom box padding.
  ".ox-src .ox-list-marker": {
    display: "inline",
    minWidth: "0",
    textAlign: "left",
    opacity: "1",
    fontVariantNumeric: "normal",
  },

  // Thematic break: the raw `---` is concealed (per the amended contract) and
  // the line draws an actual centered 1px rule; revealing the line shows the
  // dashes (as dimmed delim marks) on top of the rule. (Light/dark
  // `--ox-hr-line` values are defined alongside `--ox-bq-bar` above.)
  ".ox-hr": {
    backgroundImage: "var(--ox-hr-line)",
    backgroundSize: "100% 1px",
    backgroundPosition: "0 50%",
    backgroundRepeat: "no-repeat",
  },
  // While the hr line is being edited (dashes revealed), hide the rule so
  // the raw `---` isn't overstruck by it.
  ".ox-hr-revealed": {
    backgroundImage: "none",
  },

  // Syntax highlighting tokens (fenced code): @lezer/highlight's
  // classHighlighter emits `tok-*` classes; colors here, parsing in
  // highlight.ts. Muted, theme-aware palette.
  "&light .tok-keyword": { color: "#9333ea" },
  "&dark .tok-keyword": { color: "#c678dd" },
  "&light .tok-string, &light .tok-string2": { color: "#16a34a" },
  "&dark .tok-string, &dark .tok-string2": { color: "#98c379" },
  ".tok-comment": { fontStyle: "italic" },
  "&light .tok-comment": { color: "#9ca3af" },
  "&dark .tok-comment": { color: "#7f848e" },
  "&light .tok-number, &light .tok-bool, &light .tok-literal": { color: "#ea580c" },
  "&dark .tok-number, &dark .tok-bool, &dark .tok-literal": { color: "#d19a66" },
  "&light .tok-typeName, &light .tok-className": { color: "#0d9488" },
  "&dark .tok-typeName, &dark .tok-className": { color: "#e5c07b" },
  "&light .tok-propertyName": { color: "#2563eb" },
  "&dark .tok-propertyName": { color: "#61afef" },
  "&light .tok-operator, &light .tok-punctuation": { color: "#6b7280" },
  "&dark .tok-operator, &dark .tok-punctuation": { color: "#abb2bf" },
  "&light .tok-meta": { color: "#78716c" },
  "&dark .tok-meta": { color: "#8b949e" },

  // Task checkbox widget (the first widget island): pure-CSS custom checkbox
  // (Tailwind-forms style — `appearance: none` + inline SVG check), aligned
  // to the text baseline so it sits inline without nudging line height (the
  // CM6 replace decoration keeps the line box's own metrics).
  ".ox-task-checkbox": {
    appearance: "none",
    WebkitAppearance: "none",
    // Inputs do NOT inherit font-size; without this, every em unit below
    // resolves against the UA's ~13px default instead of the line's font.
    fontSize: "inherit",
    width: "1.05em",
    height: "1.05em",
    borderRadius: "0.28em",
    border: "1.5px solid rgba(120, 120, 128, 0.55)",
    backgroundColor: "transparent",
    display: "inline-block",
    // Optically centered against the text: vertical-align middle plus an
    // empirically measured nudge (box center was ~1.8px below line center).
    verticalAlign: "middle",
    position: "relative",
    top: "-0.13em",
    // The task item's "- " marker is concealed (~0 width), so the checkbox
    // provides its own lead-in. Margins center the checkbox on the SAME
    // column center as the bullet dot (0.83em: dot spans [0.66em, 1.0em] in
    // its 1.5em box) while keeping item text at the shared 1.5em column:
    // 0.3em + 1.05em box + 0.15em = 1.5em.
    margin: "0 0.15em 0 0.3em",
    cursor: "pointer",
    transition: "background-color 80ms ease, border-color 80ms ease",
  },
  ".ox-task-checkbox:hover": {
    borderColor: "rgba(59, 130, 246, 0.8)",
  },
  ".ox-task-checkbox:checked": {
    backgroundColor: "#3b82f6",
    borderColor: "#3b82f6",
    backgroundImage:
      "url(\"data:image/svg+xml,%3csvg viewBox='0 0 16 16' fill='white' xmlns='http://www.w3.org/2000/svg'%3e%3cpath d='M12.207 4.793a1 1 0 010 1.414l-5 5a1 1 0 01-1.414 0l-2-2a1 1 0 011.414-1.414L6.5 9.086l4.293-4.293a1 1 0 011.414 0z'/%3e%3c/svg%3e\")",
    // >100% zooms the check glyph itself (the SVG carries internal padding);
    // center keeps it symmetric inside the blue square.
    backgroundSize: "135% 135%",
    backgroundPosition: "center",
    backgroundRepeat: "no-repeat",
  },
  "&dark .ox-task-checkbox": {
    border: "1.5px solid rgba(180, 180, 190, 0.5)",
  },
  "&dark .ox-task-checkbox:checked": {
    backgroundColor: "#3b82f6",
    borderColor: "#3b82f6",
  },
});
