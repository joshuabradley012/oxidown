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

  // List markers: always visible, never concealed.
  ".ox-list-marker": { opacity: "0.7" },

  // Blockquotes: left border + muted text, depth-dependent indent. Never a
  // font-size change, so the `> ` marker's reveal/conceal cannot shift height.
  ".ox-blockquote": {
    borderLeft: "3px solid currentColor",
    opacity: "0.8",
    paddingLeft: "0.75em",
  },
  ".ox-bq-1": { borderLeftColor: "rgba(100, 100, 100, 0.5)" },
  ".ox-bq-2": { paddingLeft: "1.5em", borderLeftColor: "rgba(100, 100, 100, 0.35)" },
  ".ox-bq-3": { paddingLeft: "2.25em", borderLeftColor: "rgba(100, 100, 100, 0.2)" },

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

  // Thematic break: styled only (M1) — dimmed, spaced-out text, no conceal.
  ".ox-hr": { opacity: "0.4", letterSpacing: "0.2em" },

  // Task checkbox widget (the first widget island): aligned to the text
  // baseline so it sits inline with surrounding text without nudging line
  // height (the CM6 replace decoration keeps the line box's own metrics).
  ".ox-task-checkbox": {
    verticalAlign: "text-bottom",
    margin: "0 0.35em 0 0",
    cursor: "pointer",
  },
});
