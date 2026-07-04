//! Losslessness: load(text) → get_text() must be byte-identical for anything,
//! including text that stresses parser normalization behavior (which must
//! never leak into the document — decorations are derived, text is truth).

use oxidown_core::Editor;

const CORPUS: &[&str] = &[
    "",
    "\n",
    "plain paragraph\n",
    "no trailing newline",
    // Weird whitespace.
    "   leading spaces\n\ttab\tinside\t\ntrailing spaces   \n  \n\t\n",
    "line one\r\nline two\r\n", // CRLF preserved
    "mixed\nendings\r\nhere\r",
    // Delimiter flavor must be preserved as typed.
    "*star* _underscore_ **double-star** __double-underscore__\n",
    "***bold italic*** ___also___\n",
    // Setext-looking text.
    "Title\n===\n",
    "Title\n---\n",
    "not a setext\n\n===\n",
    // Unclosed / unbalanced delimiters.
    "**unclosed strong\n",
    "*unclosed em\n",
    "`unclosed code\n",
    "close without open** and *\n",
    "a ** b ** c (not strong per flanking rules)\n",
    // Headings, valid and almost-valid.
    "# h1\n## h2\n###### h6\n####### not-a-heading\n#nospace\n  ## indented\n",
    "#\n# \n#\t tab-after-hash\n",
    // Escapes.
    "\\*not em\\* and \\# not heading\n",
    // Code with backticks inside.
    "`` a ` b `` and ``` x ``` and ` padded `\n",
    // Non-ASCII: CJK, emoji, combining marks, RTL.
    "# 你好世界\n**粗体** *斜体* `代码`\n",
    "emoji **😀😀** pair\n",
    "combining e\u{301}\u{327} marks **e\u{301}**\n",
    "rtl **שלום** text\n",
    // Lists / quotes (parsed, but must round-trip untouched).
    "* item one\n* item two\n\n> quote **bold**\n",
    "1. ordered\n2) alt marker\n",
    // Fences and HTML (outside M0 decoration scope, still text).
    "```rust\nfn main() {}\n```\n",
    "<div>raw html</div>\n",
    // Reference definitions & footnote-ish syntax.
    "[ref]: https://example.com\ntext [ref]\n",
    // NBSP and zero-width characters.
    "nb\u{a0}sp and zero\u{200b}width\n",
];

#[test]
fn load_get_text_is_byte_identical() {
    for (i, doc) in CORPUS.iter().enumerate() {
        let mut ed = Editor::new(1);
        ed.load(doc);
        assert_eq!(
            ed.get_text().as_bytes(),
            doc.as_bytes(),
            "corpus entry {i} not byte-identical: {doc:?}"
        );
    }
}

#[test]
fn doc_len_utf16_matches_std() {
    for doc in CORPUS {
        let mut ed = Editor::new(1);
        ed.load(doc);
        let expected: usize = doc.chars().map(char::len_utf16).sum();
        assert_eq!(ed.doc_len_utf16(), expected, "utf16 length for {doc:?}");
    }
}

#[test]
fn decorations_never_error_on_corpus() {
    // Emission over the full corpus must be panic- and error-free.
    for doc in CORPUS {
        let mut ed = Editor::new(1);
        let rev = ed.load(doc);
        ed.decorations(rev, 0, ed.doc_len_utf16(), &[]).unwrap();
    }
}
