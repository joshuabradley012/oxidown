//! Conformance corpus (M1): ~80 hand-picked markdown snippets across every
//! decorated construct plus nasty nesting combinations. Not a test file
//! itself (no `#[test]`s here) — included by `../corpus_conformance.rs` via
//! `#[path]`, per docs/boundary-v0.md's v0.2 scope and plan.md §9
//! ("conformance... comrak as the oracle for divergence triage").
//!
//! Network access is unavailable in this environment, so this is a vendored,
//! hand-picked stand-in for the full CommonMark+GFM spec suites (per the
//! task brief), not a download of the official suite.

#[rustfmt::skip]
pub const CASES: &[&str] = &[
    // ---------------------------------------------------------- baseline --
    "",
    "\n",
    "plain paragraph\n",
    "no trailing newline",
    "   \n\t\n  \n",

    // ------------------------------------------------------------ headings --
    "# h1\n",
    "## h2\n",
    "### h3\n",
    "#### h4\n",
    "##### h5\n",
    "###### h6\n",
    "####### not a heading (7 hashes)\n",
    "#no-space-not-a-heading\n",
    "  ## indented heading\n",
    "# \n",
    "#\n",
    "Setext H1\n=========\n",
    "Setext H2\n---------\n",
    "# heading with **bold** and *em* and `code`\n",

    // ------------------------------------------------------- strong/emphasis --
    "*em*\n",
    "_em_\n",
    "**strong**\n",
    "__strong__\n",
    "***strong em***\n",
    "___strong em___\n",
    "**bold *em inside* bold**\n",
    "*em with **bold inside** em*\n",
    "a ** b ** c (not strong, flanking rules)\n",
    "**unclosed strong\n",
    "*unclosed em\n",

    // ------------------------------------------------------------ inline code --
    "`code`\n",
    "``code with ` backtick``\n",
    "` padded `\n",
    "`` `starts and ends with backtick` ``\n",
    "`code containing *asterisks* and _underscores_ and ~~tildes~~ and [brackets](x)`\n",
    "text `unterminated code\n",

    // ------------------------------------------------------------ strikethrough --
    "~~strike~~\n",
    "~~strike with **bold** inside~~\n",
    "**bold with ~~strike~~ inside**\n",
    "~~unclosed strike\n",

    // ------------------------------------------------------------------ links --
    "[text](http://example.com)\n",
    "[text](http://example.com \"a title\")\n",
    "[]()\n",
    "[text]()\n",
    "[text](path/with/(parens)/in/it)\n",
    "<http://example.com>\n",
    "<foo@example.com>\n",
    "text with <http://example.com> inline\n",
    "*[link in emphasis](url)*\n",
    "**[link in strong](url)**\n",
    "[**bold link text**](url)\n",
    "[link with `code` inside](url)\n",
    "[ref link][ref]\n\n[ref]: http://example.com\n",
    "![image](img.png)\n",

    // -------------------------------------------------------------- blockquotes --
    "> single line quote\n",
    "> line one\n> line two\n",
    "> outer\n> > nested\n> > > deeply nested\n",
    "> outer\n> > inner\n> outer again (lazy-ish)\n",
    "> line one\nlazy continuation line two\n",
    "> quote with **bold** and *em*\n",
    "> - item one\n> - item two\n",
    "> ```\n> code in quote\n> ```\n",
    "> first paragraph\n>\n> second paragraph\n",

    // ------------------------------------------------------------------- lists --
    "- one\n- two\n- three\n",
    "* one\n* two\n",
    "+ one\n+ two\n",
    "1. one\n2. two\n3. three\n",
    "1) one\n2) two\n",
    "5. starts at five\n6. six\n",
    "- loose one\n\n- loose two\n",
    "- tight one\n- tight two\n",
    "- top\n  - nested\n    - deeply nested\n",
    "- item\n  continuation text\n",
    "- [ ] unchecked task\n- [x] checked task\n",
    "- [ ] task one\n  - [x] nested task two\n",
    "> - [ ] task in blockquote\n",
    "1. [ ] ordered task\n2. [x] ordered task done\n",
    "- item with **bold** and [a link](url)\n",

    // -------------------------------------------------------------- code fences --
    "```\nplain fence\n```\n",
    "```rust\nfn main() {}\n```\n",
    "~~~\ntilde fence\n~~~\n",
    "~~~python\nprint('hi')\n~~~\n",
    "```\nunterminated fence\n",
    "```\n\n```\n",
    "    indented code block\n    second line\n",

    // -------------------------------------------------------------- thematic breaks --
    "---\n",
    "***\n",
    "___\n",
    "a\n\n---\n\nb\n",
    "- - -\n",

    // ------------------------------------------------------- tables/footnotes (parsed, no decor) --
    "| a | b |\n| - | - |\n| 1 | 2 |\n",
    "text with a footnote[^1]\n\n[^1]: the footnote text\n",

    // --------------------------------------------------------------- raw HTML --
    "<div>raw html block</div>\n",
    "text with <em>raw inline html</em>\n",

    // ------------------------------------------------------------------ escapes --
    "\\*not em\\* and \\# not heading and \\[not a link\\]\n",
    "1974\\. not a list\n",

    // --------------------------------------------------------- non-ASCII stress --
    "# 你好世界\n**粗体** *斜体* `代码` ~~删除线~~\n",
    "emoji **😀😀** pair and [链接](url) 😀\n",
    "combining e\u{301}\u{327} marks **e\u{301}** strike~~e\u{301}~~\n",
    "rtl **שלום** text and [קישור](url)\n",
    "- 你好\n- [ ] 任务 😀\n",
    "> 引用 **粗体**\n",

    // -------------------------------------------------------------- line endings --
    "line one\r\nline two\r\n",
    "> quote\r\n> continued\r\n",
    "- one\r\n- two\r\n",

    // ------------------------------------------------- review-fix regressions --
    "~x~\n",
    "~one~ and ~~two~~\n",
    "-\n  foo\n",
    "- - a\n",
    "- - - a\n",
    "- a\n - b\n",
    "1. a\n 2. b\n",
    "> - a\n>   - b\n",
    "> - a\n>   - b\n>     - c\n",
    "> ```rust\n> let x = 1;\n> ```\n",
    "> ```\n> > literal quote in code\n> ```\n",
    "- item\n  ```\n  fenced in list\n  ```\n",
    "- a\n  - b\n    ```\n    deep fence\n    ```\n",
    "1. item\n   ```\n   code\n   ```\n",
    "[t](u \"a)b\")\n",
    "[t](u \"(a\")\n",
    "[t](u '(a')\n",
    "[t](<u v>)\n",
    "[t](<>)\n",
    "[t](<u v> \"ti\")\n",
    "```\naaa\n\nbbb\n```\n",
    "```\n\n\n```\n",
    "- - - - - - - - a\n",
    "1. [ ] a\n1. [x] b\n",
    "# Title ##\n",
    "## t ## \n",
    "# Title #########\n",
    "## ##\n",
    "# foo#\n",

    // ------------------------------------------------------------- nasty nesting --
    "> - [ ] task in list in blockquote with **bold**\n",
    "- > blockquote inside a list item\n",
    "- item one\n\n  > nested blockquote inside loose item\n",
    "*emphasis with [link containing `code`](url) inside*\n",
    "[link text with **bold *and em* text**](url)\n",
    "~~strike [link](url) strike~~\n",
    "> ```\n> fenced code inside blockquote\n> with multiple lines\n> ```\n",
    "1. one\n   - nested unordered inside ordered\n2. two\n",
    "- [ ] a\n- [ ] b\n- [x] c\n- [ ] d\n",
    "***[bold-em link](url)***\n",
];
