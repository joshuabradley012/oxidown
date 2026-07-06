// NOTE: paragraphs in these documents are deliberately single source lines.
// In a source-truth editor a newline in the source IS a line break on screen
// (soft-wrapping is the view's job, via EditorView.lineWrapping) — hard-
// wrapping prose at ~72 columns would render those wraps literally.

/**
 * Every currently-supported mark, in one block (contract v0.2 + amendments):
 * headings h1–h6, bold/italic in both delimiter flavors, bold-italic, inline
 * code, strikethrough, links + autolinks, nested blockquotes, a fenced code
 * block with syntax highlighting, bullet/ordered/task lists incl. deep and
 * MIXED nesting, and a thematic break. Shared by the default document and
 * the streaming demo (the stream just repeats it).
 */
const ALL_MARKS = `## Inline marks

**Bold** and __also bold__, *italic* and _also italic_, ***bold italic***, \`inline code\`, ~~strikethrough~~, a [link](https://example.com), and an autolink <https://oxidown.dev>. Put the cursor inside any of them to reveal the raw syntax.

## Headings

# Heading 1
## Heading 2
### Heading 3
#### Heading 4
##### Heading 5
###### Heading 6

## Blockquotes

> Level one quote — the bar on the left marks the depth.
> > Level two nests with a second bar and more indent.
> > > Level three, same idea.

## Code

\`\`\`ts
function greet(name: string): string {
  // comments, keywords, strings, and numbers all highlight
  const excitement = 3;
  return \`hello, \${name}\` + "!".repeat(excitement);
}
\`\`\`

## Lists — plain, ordered, tasks, nested, mixed

- bullet level one
- another bullet
  - nested bullet — its dot starts where the parent's text starts
    - third level, one more step in
- [ ] a task item
- [x] a completed task
  - [ ] a task nested under a task
1. ordered one
2. ordered two
   1. nested ordered item
   - a bullet nested under an ordered item
   - [x] a task nested under an ordered item
3. ordered three

---
`;

export const SAMPLE_DOC = `# Oxidown demo

Everything below is plain markdown — the file is the document. Move the cursor into any formatted span to reveal its delimiters; move away to conceal them. Toggle source mode above to see the raw text.

${ALL_MARKS}
## 日本語の段落（IME テスト）

これは日本語の段落です。**太字**と*斜体*、\`コード\`も混ざっています。この行でかな漢字変換を試してください。変換中（未確定文字列がある間）は装飾の再計算が凍結され、確定した時点で再描画されます。

## Emoji width test

Emoji 😀 in plain text, **bold 🎉 emoji**, and *italic 🚀 emoji* — all positions cross the boundary as UTF-16 code units, so astral characters count as two.

## What to try

Type, undo (Cmd/Ctrl-Z), redo (Cmd/Ctrl-Shift-Z or Ctrl-Y), paste, drag-select across formatted spans, Tab/Shift-Tab to indent, click the checkboxes, and toggle source mode above.
`;

/**
 * Hardcoded "LLM answer" for the streaming demo (no network): a short intro,
 * then the full ALL_MARKS block — so the append fast-path is exercised across
 * every supported construct, delivered in randomly-sized chunks that do NOT
 * align to token or markdown boundaries, on purpose.
 */
export const STREAM_TEXT = `## Streaming into a live document

Watch this answer type itself in, then try editing the top of the document at the same time — your cursor never moves, because the stream's edits are mapped around it by the core.

Everything Oxidown can render will now stream in, chunk by unpredictable chunk. Mid-construct states (an unterminated fence, a half-typed \`**bold\`) render honestly for a moment and snap into shape when the closing syntax arrives.

${ALL_MARKS}
That's the whole vocabulary — streamed into a document you could keep editing the entire time. One undo (Cmd/Ctrl-Z) reverts this whole answer as a single unit without touching your own edits.
`;

export function largeDocFiller(paragraphs = 200): string {
  const parts: string[] = [];
  for (let i = 1; i <= paragraphs; i++) {
    parts.push(
      `Filler paragraph ${i}: **bold run ${i}** with *italic ${i}* and \`code_${i}\` — ` +
        `lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor ` +
        `incididunt ut labore et dolore magna aliqua.`,
    );
  }
  return "\n\n## Large document section\n\n" + parts.join("\n\n") + "\n";
}
