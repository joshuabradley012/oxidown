export const SAMPLE_DOC = `# Oxidown M0 demo

## Hybrid live preview

Some **bold text**, some *italic text*, and \`inline code\`. Move the cursor
into a formatted span to reveal its delimiters; move away to conceal them.

### Nesting

Here is **bold with *italic* inside** and ***both at once***.
Alternate delimiters work too: __strong__ and _emphasis_.

## 日本語の段落（IME テスト）

これは日本語の段落です。**太字**と*斜体*、\`コード\`も混ざっています。
この行でかな漢字変換を試してください。変換中（未確定文字列がある間）は
装飾の再計算が凍結され、確定した時点で再描画されます。

## Emoji width test

Emoji 😀 in plain text, **bold 🎉 emoji**, and *italic 🚀 emoji* — all
positions cross the boundary as UTF-16 code units, so astral characters
count as two.

## What to try

Type, undo (Cmd/Ctrl-Z), redo (Cmd/Ctrl-Shift-Z or Ctrl-Y), paste,
drag-select across formatted spans, and toggle source mode above.
`;

/**
 * Hardcoded "LLM answer" for the streaming demo (no network). Deliberately a
 * mix of M1 markdown constructs — headings, bold/italic, a fenced code
 * block, a task list, a blockquote — so the demo exercises the append
 * fast-path across all of them. main.ts delivers this via streamOpen/
 * streamAppend/streamClose in randomly-sized chunks that do NOT align to
 * token or markdown boundaries, on purpose.
 */
export const STREAM_TEXT = `## Streaming into a live document

Here's what's happening while this text appears.

### What's new in M1

The core now understands more markdown, and the view renders it without
ever losing the source text underneath: **bold**, *italic*, and even
~~struck-through~~ runs stay concealed until your cursor visits them.

> Concealment never removes characters from the DOM — it only collapses
> them visually. That discipline is exactly what keeps this stream from
> corrupting anything while it types itself in.

### Try this right now

1. Leave this answer streaming.
2. Click into the **top** of the document and keep typing.
3. Notice your own edits are never interrupted — the stream keeps
   appending exactly where it left off, underneath your cursor.

### A tiny code sample

\`\`\`ts
function toggle(doc: string, from: number, to: number): string {
  return doc.slice(0, from) + "**" + doc.slice(from, to) + "**" + doc.slice(to);
}
\`\`\`

### Progress checklist

- [x] Protocol extended additively: marks, lines, widgets, anchors
- [x] MockCore implements commands, anchors, and streaming
- [x] The view renders the new vocabulary with no layout shift
- [ ] Wire up the real Rust/wasm core once it lands
- [ ] Bring the same widget pattern to tables and images

### Why streaming is the headline feature

Most editors treat AI output as a read-only bubble bolted onto the side of
the real document. Oxidown treats it as *first-class input*: the same
splice-based pipe that carries your keystrokes carries the model's tokens,
chunk by chunk, arriving at unpredictable byte boundaries.

That means the parser has to stay honest about partial input — an
unterminated fence or a half-typed \`**bold\` never gets to corrupt
anything, because the document is never anything other than plain text.

> "The file is the document." Everything else — decorations, widgets,
> reveal state — is disposable and re-derivable. If it's wrong, it just
> repaints; it can never corrupt what you actually wrote.

That's the whole pitch — thanks for reading. Now go try editing while
this finishes streaming in.
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
