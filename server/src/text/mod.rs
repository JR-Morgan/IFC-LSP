//! Flat UTF-8 text buffer with a line-start table and LSP position mapping.

use lsp_types::Position;

/// Byte offset into a document. Files handled through the LSP are capped by the
/// editor at 50 MB, so `u32` is ample.
pub type Offset = u32;

#[derive(Debug, Clone, Default)]
pub struct LineTable {
    /// Byte offset of the first byte of every line. `line_starts[0] == 0`.
    line_starts: Vec<Offset>,
}

impl LineTable {
    pub fn new(bytes: &[u8]) -> Self {
        let mut line_starts = Vec::with_capacity(bytes.len() / 64 + 1);
        line_starts.push(0);
        line_starts.extend(memchr::memchr_iter(b'\n', bytes).map(|i| (i + 1) as Offset));
        Self { line_starts }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, line: usize) -> Offset {
        self.line_starts[line.min(self.line_starts.len() - 1)]
    }

    /// Zero-based line containing `offset`.
    pub fn line_of(&self, offset: Offset) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// Replace the newline information for the byte range `[start, end)` after the
    /// text in that range has been replaced by `inserted`.
    pub fn splice(&mut self, start: Offset, end: Offset, inserted: &[u8]) {
        let la = self.line_of(start);
        let lb = self.line_of(end);
        let delta = inserted.len() as i64 - (end - start) as i64;
        let new_lines: Vec<Offset> = memchr::memchr_iter(b'\n', inserted)
            .map(|i| start + i as Offset + 1)
            .collect();
        let tail_from = lb + 1;
        if delta != 0 {
            for ls in &mut self.line_starts[tail_from..] {
                *ls = (*ls as i64 + delta) as Offset;
            }
        }
        self.line_starts.splice(la + 1..tail_from, new_lines);
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextBuffer {
    bytes: Vec<u8>,
    lines: LineTable,
}

impl TextBuffer {
    pub fn new(text: impl Into<Vec<u8>>) -> Self {
        let bytes = text.into();
        let lines = LineTable::new(&bytes);
        Self { bytes, lines }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> Offset {
        self.bytes.len() as Offset
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn lines(&self) -> &LineTable {
        &self.lines
    }

    pub fn slice(&self, start: Offset, end: Offset) -> &[u8] {
        &self.bytes[start as usize..end as usize]
    }

    /// Replace `[start, end)` with `inserted`, keeping the line table in step.
    pub fn splice(&mut self, start: Offset, end: Offset, inserted: &[u8]) {
        self.lines.splice(start, end, inserted);
        self.bytes
            .splice(start as usize..end as usize, inserted.iter().copied());
    }

    pub fn position(&self, offset: Offset) -> Position {
        let offset = offset.min(self.len());
        let line = self.lines.line_of(offset);
        let line_start = self.lines.line_start(line) as usize;
        let prefix = &self.bytes[line_start..offset as usize];
        let character = if prefix.is_ascii() {
            prefix.len() as u32
        } else {
            utf16_len(prefix)
        };
        Position::new(line as u32, character)
    }

    /// Byte offset of an LSP (line, UTF-16 column) position, clamped to the line end
    /// (excluding the newline) and to the end of the buffer.
    pub fn offset(&self, pos: Position) -> Offset {
        let line = pos.line as usize;
        if line >= self.lines.line_count() {
            return self.len();
        }
        let line_start = self.lines.line_start(line) as usize;
        let line_end = if line + 1 < self.lines.line_count() {
            self.lines.line_start(line + 1) as usize - 1
        } else {
            self.bytes.len()
        };
        let line_bytes = &self.bytes[line_start..line_end];
        let col = pos.character as usize;
        if line_bytes.is_ascii() {
            return (line_start + col.min(line_bytes.len())) as Offset;
        }
        let mut units = 0usize;
        for (i, ch) in std::str::from_utf8(line_bytes).unwrap_or("").char_indices() {
            if units >= col {
                return (line_start + i) as Offset;
            }
            units += ch.len_utf16();
        }
        line_end as Offset
    }

    pub fn range(&self, start: Offset, end: Offset) -> lsp_types::Range {
        lsp_types::Range::new(self.position(start), self.position(end))
    }
}

fn utf16_len(bytes: &[u8]) -> u32 {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.chars().map(|c| c.len_utf16() as u32).sum(),
        // Mid-character offsets only arise from malformed input; count bytes.
        Err(_) => bytes.len() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_table_basic() {
        let t = LineTable::new(b"ab\ncd\n\nef");
        assert_eq!(t.line_count(), 4);
        assert_eq!(t.line_of(0), 0);
        assert_eq!(t.line_of(2), 0);
        assert_eq!(t.line_of(3), 1);
        assert_eq!(t.line_of(6), 2);
        assert_eq!(t.line_of(7), 3);
        assert_eq!(t.line_of(8), 3);
    }

    #[test]
    fn splice_matches_rebuild() {
        let mut buf = TextBuffer::new("line one\nline two\nline three\n");
        buf.splice(5, 8, b"1\n2");
        assert_eq!(buf.bytes(), b"line 1\n2\nline two\nline three\n");
        assert_eq!(
            buf.lines().line_starts,
            LineTable::new(buf.bytes()).line_starts
        );
        buf.splice(0, 9, b"");
        assert_eq!(buf.bytes(), b"line two\nline three\n");
        assert_eq!(
            buf.lines().line_starts,
            LineTable::new(buf.bytes()).line_starts
        );
        buf.splice(buf.len(), buf.len(), b"tail\n\n");
        assert_eq!(
            buf.lines().line_starts,
            LineTable::new(buf.bytes()).line_starts
        );
    }

    #[test]
    fn positions_utf16() {
        let buf = TextBuffer::new("a😀b\n日本\n");
        assert_eq!(buf.position(0), Position::new(0, 0));
        assert_eq!(buf.position(1), Position::new(0, 1));
        assert_eq!(buf.position(5), Position::new(0, 3));
        assert_eq!(buf.position(6), Position::new(0, 4));
        assert_eq!(buf.position(7), Position::new(1, 0));
        assert_eq!(buf.position(10), Position::new(1, 1));
        assert_eq!(buf.offset(Position::new(0, 3)), 5);
        assert_eq!(buf.offset(Position::new(0, 99)), 6);
        assert_eq!(buf.offset(Position::new(1, 1)), 10);
        assert_eq!(buf.offset(Position::new(5, 0)), buf.len());
        for off in 0..=buf.len() {
            if buf
                .bytes()
                .get(off as usize)
                .is_none_or(|b| (*b as i8) >= -0x40)
            {
                assert_eq!(buf.offset(buf.position(off)), off, "offset {off}");
            }
        }
    }

    #[test]
    fn random_splices_keep_line_table_consistent() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let alphabet = b"ab\n\n;#=(),'x";
        let mut buf = TextBuffer::new("#1=A();\n#2=B(#1);\n");
        for _ in 0..2000 {
            let len = buf.len();
            let a = (next() % (len as u64 + 1)) as Offset;
            let b = (a + (next() % 6) as Offset).min(len);
            let n = (next() % 8) as usize;
            let ins: Vec<u8> = (0..n)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            buf.splice(a, b, &ins);
            assert_eq!(
                buf.lines().line_starts,
                LineTable::new(buf.bytes()).line_starts
            );
        }
    }
}
