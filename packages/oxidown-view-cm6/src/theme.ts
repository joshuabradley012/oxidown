import { EditorView } from "@codemirror/view";

/**
 * Oxidown base theme.
 *
 * The conceal trick: delimiters are never removed from the DOM (that would
 * break IME and cursor math) — they are visually collapsed with a tiny
 * font-size + negative letter-spacing. Because heading sizes are set on the
 * LINE element (`.ox-h1`…`.ox-h6`), revealed delimiters inherit the line's
 * font-size, and concealed delimiters (0.01em relative to the line) are far
 * too small to influence the line box: line height does not change between
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

  // Concealed delimiters: visually collapsed, characters stay in the DOM.
  ".ox-conceal": {
    fontSize: "0.01em",
    letterSpacing: "-0.01em",
  },

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
  ".ox-bullet": {
    display: "inline-block",
    minWidth: "1em",
    opacity: "0.85",
  },
  ".ox-bullet::before": {
    content: '""',
    display: "inline-block",
    width: "0.34em",
    height: "0.34em",
    borderRadius: "50%",
    backgroundColor: "currentColor",
    verticalAlign: "middle",
    marginLeft: "0.08em",
    marginRight: "0.55em",
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
  // whole block reads as one unit; the fence line is dimmed relative to body.
  ".ox-code-fence": {
    fontFamily: MONO_FONT,
    opacity: "0.55",
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

  // Task checkbox widget (the first widget island): pure-CSS custom checkbox
  // (Tailwind-forms style — `appearance: none` + inline SVG check), aligned
  // to the text baseline so it sits inline without nudging line height (the
  // CM6 replace decoration keeps the line box's own metrics).
  ".ox-task-checkbox": {
    appearance: "none",
    WebkitAppearance: "none",
    width: "1.05em",
    height: "1.05em",
    borderRadius: "0.28em",
    border: "1.5px solid rgba(120, 120, 128, 0.55)",
    backgroundColor: "transparent",
    display: "inline-block",
    verticalAlign: "text-bottom",
    margin: "0 0.4em 0 0",
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
    backgroundSize: "100% 100%",
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
