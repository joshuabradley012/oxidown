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
 */
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
    fontFamily:
      "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace",
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
});
