//! Rope wrapper. Internal positions are UTF-8 byte offsets; the public core
//! API speaks UTF-16 code units (per docs/boundary-v0.md) and converts here
//! using ropey's utf16 metrics.

use std::cell::Cell;
use std::ops::Range;

use ropey::Rope;

use crate::error::CoreError;

/// A single text replacement in UTF-8 **byte** coordinates. This is the
/// core-internal splice; the boundary type ([`crate::Splice`]) is UTF-16.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteSplice {
    pub at: usize,
    pub delete: usize,
    pub insert: String,
}

impl ByteSplice {
    pub fn end(&self) -> usize {
        self.at + self.delete
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextBuffer {
    rope: Rope,
}

impl TextBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_text(s: &str) -> Self {
        Self {
            rope: Rope::from_str(s),
        }
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn len_utf16(&self) -> usize {
        self.rope.len_utf16_cu()
    }

    /// Convert a UTF-16 code-unit offset into a byte offset.
    ///
    /// Errors: `OutOfBounds` past the end of the document; `SurrogateSplit`
    /// when the offset lands between the two code units of a surrogate pair
    /// (detected by round-tripping through the char index).
    pub fn utf16_to_byte(&self, cu: usize) -> Result<usize, CoreError> {
        let len = self.rope.len_utf16_cu();
        if cu > len {
            return Err(CoreError::OutOfBounds { pos: cu, len });
        }
        let ch = self.rope.utf16_cu_to_char(cu);
        if self.rope.char_to_utf16_cu(ch) != cu {
            return Err(CoreError::SurrogateSplit { pos: cu });
        }
        Ok(self.rope.char_to_byte(ch))
    }

    /// Like [`utf16_to_byte`](Self::utf16_to_byte), but snaps an offset that
    /// falls inside a surrogate pair DOWN to the containing code point's
    /// start. For query positions (viewport edges, selections, composition
    /// ranges) snapping is correct; only splices demand exact boundaries.
    pub fn utf16_to_byte_floor(&self, cu: usize) -> Result<usize, CoreError> {
        let len = self.rope.len_utf16_cu();
        if cu > len {
            return Err(CoreError::OutOfBounds { pos: cu, len });
        }
        Ok(self.rope.char_to_byte(self.rope.utf16_cu_to_char(cu)))
    }

    /// Like [`utf16_to_byte_floor`](Self::utf16_to_byte_floor), but snaps UP
    /// to the following code-point boundary instead.
    pub fn utf16_to_byte_ceil(&self, cu: usize) -> Result<usize, CoreError> {
        let len = self.rope.len_utf16_cu();
        if cu > len {
            return Err(CoreError::OutOfBounds { pos: cu, len });
        }
        let ch = self.rope.utf16_cu_to_char(cu);
        if self.rope.char_to_utf16_cu(ch) == cu {
            Ok(self.rope.char_to_byte(ch))
        } else {
            Ok(self.rope.char_to_byte(ch + 1))
        }
    }

    /// Convert a byte offset into a UTF-16 code-unit offset.
    ///
    /// The byte offset must lie on a char boundary within the document — an
    /// internal invariant: every internal byte position is derived either
    /// from a validated UTF-16 conversion or from parser spans (which are
    /// char-aligned by construction).
    pub fn byte_to_utf16(&self, byte: usize) -> usize {
        self.rope.char_to_utf16_cu(self.rope.byte_to_char(byte))
    }

    pub fn byte_slice_to_string(&self, range: Range<usize>) -> String {
        self.rope.byte_slice(range).to_string()
    }

    pub fn replace_bytes(&mut self, range: Range<usize>, insert: &str) {
        let cs = self.rope.byte_to_char(range.start);
        let ce = self.rope.byte_to_char(range.end);
        if ce > cs {
            self.rope.remove(cs..ce);
        }
        if !insert.is_empty() {
            self.rope.insert(cs, insert);
        }
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// The raw byte at `idx`, or `None` past the end. Used for cheap
    /// single-byte probes (e.g. "is the previous byte a newline"); safe on
    /// any index — no char-boundary requirement.
    pub fn byte_at(&self, idx: usize) -> Option<u8> {
        (idx < self.rope.len_bytes()).then(|| self.rope.byte(idx))
    }

    /// Byte range of the line containing `byte` (char-boundary aligned),
    /// excluding the trailing line terminator (`\n` or `\r\n`).
    pub fn line_range_at(&self, byte: usize) -> Range<usize> {
        let line = self.rope.byte_to_line(byte.min(self.rope.len_bytes()));
        let start = self.rope.line_to_byte(line);
        let mut end = if line + 1 < self.rope.len_lines() {
            self.rope.line_to_byte(line + 1)
        } else {
            self.rope.len_bytes()
        };
        if end > start && self.rope.byte(end - 1) == b'\n' {
            end -= 1;
        }
        if end > start && self.rope.byte(end - 1) == b'\r' {
            end -= 1;
        }
        start..end
    }
}

/// Byte-oriented random-access reader over a [`TextBuffer`], for code that
/// scans/compares small document regions with **doc-absolute byte indices**
/// (the command planners). Replaces the old "materialize the entire rope
/// into one `String` per command" pattern — an O(doc) allocation+copy of
/// which the planners only ever read a handful of local lines
/// (research/08-perf-baseline.md §10 item 4).
///
/// Access goes through ropey's contiguous chunks with a one-chunk cache
/// (`Cell` — chunk refs borrow the rope, so they're `Copy`): sequential
/// scans cost O(1) per byte with one O(log doc) chunk lookup per ~1KB
/// crossed. Nothing is copied, ever; correctness never depends on any
/// window/cache sizing.
pub struct SrcBytes<'a> {
    text: &'a TextBuffer,
    len: usize,
    /// `(chunk_start_byte, chunk)` of the most recently accessed chunk.
    chunk: Cell<(usize, &'a str)>,
}

impl<'a> SrcBytes<'a> {
    pub fn new(text: &'a TextBuffer) -> Self {
        Self {
            text,
            len: text.len_bytes(),
            chunk: Cell::new((0, "")),
        }
    }

    /// Document length in bytes (NOT a window length — indices are always
    /// doc-absolute).
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The byte at `i`, or `None` past the document's end. No char-boundary
    /// requirement.
    #[inline]
    pub fn get(&self, i: usize) -> Option<u8> {
        if i >= self.len {
            return None;
        }
        let (start, chunk) = self.chunk.get();
        if let Some(&b) = chunk.as_bytes().get(i.wrapping_sub(start)) {
            return Some(b);
        }
        let (chunk, chunk_start, _, _) = self.text.rope.chunk_at_byte(i);
        self.chunk.set((chunk_start, chunk));
        Some(chunk.as_bytes()[i - chunk_start])
    }

    /// The byte at `i`; panics past the document's end (mirrors `bytes[i]`).
    #[inline]
    pub fn byte(&self, i: usize) -> u8 {
        self.get(i).expect("SrcBytes::byte out of bounds")
    }

    /// Append the bytes of `range` (which must be char-boundary aligned,
    /// like every parser-derived span) to `out`.
    pub fn push_slice_to(&self, out: &mut String, range: Range<usize>) {
        for chunk in self.text.rope.byte_slice(range).chunks() {
            out.push_str(chunk);
        }
    }

    /// Whether `range`'s bytes (char-boundary aligned) equal `s`.
    pub fn slice_eq(&self, range: Range<usize>, s: &str) -> bool {
        self.text.rope.byte_slice(range) == s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_identity() {
        let t = TextBuffer::from_text("hello");
        assert_eq!(t.len_utf16(), 5);
        assert_eq!(t.len_bytes(), 5);
        for i in 0..=5 {
            assert_eq!(t.utf16_to_byte(i).unwrap(), i);
            assert_eq!(t.byte_to_utf16(i), i);
        }
    }

    #[test]
    fn emoji_surrogate_pairs() {
        // 😀 U+1F600: 4 UTF-8 bytes, 2 UTF-16 code units (surrogate pair).
        let t = TextBuffer::from_text("a😀b");
        assert_eq!(t.len_bytes(), 6);
        assert_eq!(t.len_utf16(), 4);
        assert_eq!(t.utf16_to_byte(0).unwrap(), 0);
        assert_eq!(t.utf16_to_byte(1).unwrap(), 1);
        // Position 2 splits the surrogate pair.
        assert_eq!(
            t.utf16_to_byte(2),
            Err(CoreError::SurrogateSplit { pos: 2 })
        );
        assert_eq!(t.utf16_to_byte(3).unwrap(), 5);
        assert_eq!(t.utf16_to_byte(4).unwrap(), 6);
        assert_eq!(t.byte_to_utf16(1), 1);
        assert_eq!(t.byte_to_utf16(5), 3);
        assert_eq!(t.byte_to_utf16(6), 4);
    }

    #[test]
    fn cjk() {
        // 你/好: 3 UTF-8 bytes each, 1 UTF-16 code unit each (BMP).
        let t = TextBuffer::from_text("你好x");
        assert_eq!(t.len_bytes(), 7);
        assert_eq!(t.len_utf16(), 3);
        assert_eq!(t.utf16_to_byte(0).unwrap(), 0);
        assert_eq!(t.utf16_to_byte(1).unwrap(), 3);
        assert_eq!(t.utf16_to_byte(2).unwrap(), 6);
        assert_eq!(t.utf16_to_byte(3).unwrap(), 7);
        assert_eq!(t.byte_to_utf16(3), 1);
        assert_eq!(t.byte_to_utf16(6), 2);
    }

    #[test]
    fn combining_marks() {
        // "e" + U+0301 COMBINING ACUTE ACCENT: separate scalar values.
        // U+0301 is 2 UTF-8 bytes, 1 UTF-16 code unit.
        let t = TextBuffer::from_text("e\u{301}z");
        assert_eq!(t.len_bytes(), 4);
        assert_eq!(t.len_utf16(), 3);
        assert_eq!(t.utf16_to_byte(1).unwrap(), 1); // between base and mark: valid
        assert_eq!(t.utf16_to_byte(2).unwrap(), 3);
        assert_eq!(t.byte_to_utf16(3), 2);
    }

    #[test]
    fn out_of_bounds() {
        let t = TextBuffer::from_text("ab");
        assert_eq!(
            t.utf16_to_byte(3),
            Err(CoreError::OutOfBounds { pos: 3, len: 2 })
        );
    }

    #[test]
    fn replace_bytes_roundtrip() {
        let mut t = TextBuffer::from_text("a😀b");
        t.replace_bytes(1..5, "X");
        assert_eq!(t.text(), "aXb");
        t.replace_bytes(1..2, "");
        assert_eq!(t.text(), "ab");
        t.replace_bytes(2..2, "😀");
        assert_eq!(t.text(), "ab😀");
    }
}
