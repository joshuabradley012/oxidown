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
