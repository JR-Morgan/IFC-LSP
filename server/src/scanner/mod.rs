//! Entity boundary scanner: the hot path of the initial index build.
//!
//! It records only where each entity starts and ends, its `#id` and its interned
//! constructor name; argument lists are parsed later, on demand.

use rustc_hash::FxHashMap;

use crate::lexer::{
    StringEnd, is_ident_continue, is_ident_start, looks_like_entity_start, skip_comment,
    skip_string,
};
use crate::text::Offset;

pub mod flags {
    /// Something about the record is wrong; see the other bits for what.
    pub const MALFORMED: u16 = 1 << 0;
    /// `#1=(IFCA(...)IFCB(...));`
    pub const COMPLEX: u16 = 1 << 1;
    /// Record without a `#id` (header entities such as `FILE_SCHEMA(...)`).
    pub const HEADER: u16 = 1 << 2;
    /// Bare keyword line: `ISO-10303-21;`, `HEADER;`, `DATA;`, `ENDSEC;`, ...
    pub const KEYWORD: u16 = 1 << 3;
    pub const UNTERMINATED_STRING: u16 = 1 << 4;
    pub const MISSING_SEMICOLON: u16 = 1 << 5;
    pub const NO_EQUALS: u16 = 1 << 6;
    pub const NO_NAME: u16 = 1 << 7;
    pub const BAD_ID: u16 = 1 << 8;
    pub const MULTILINE: u16 = 1 << 9;
    pub const UNTERMINATED_COMMENT: u16 = 1 << 10;
    /// Unrecognised text between records.
    pub const GARBAGE: u16 = 1 << 11;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityRecord {
    /// Offset of the `#` (or of the first byte of a header record).
    pub start: Offset,
    /// Offset one past the terminating `;` (or where recovery stopped).
    pub end: Offset,
    /// Instance id; 0 for header records and unparsable ids.
    pub id: u32,
    /// Interned constructor name, see [`NameInterner`].
    pub name: u32,
    pub flags: u16,
}

impl EntityRecord {
    pub fn is_malformed(&self) -> bool {
        self.flags & flags::MALFORMED != 0
    }
    pub fn is_complex(&self) -> bool {
        self.flags & flags::COMPLEX != 0
    }
    pub fn is_header(&self) -> bool {
        self.flags & (flags::HEADER | flags::KEYWORD) != 0
    }
    pub fn has_id(&self) -> bool {
        self.flags & (flags::HEADER | flags::KEYWORD | flags::GARBAGE) == 0 && self.id != 0
    }
    pub fn shift(&mut self, delta: i64) {
        self.start = (self.start as i64 + delta) as Offset;
        self.end = (self.end as i64 + delta) as Offset;
    }
}

pub const NAME_NONE: u32 = 0;
pub const NAME_COMPLEX: u32 = 1;

/// Upper-cased constructor names interned per document.
#[derive(Debug, Clone)]
pub struct NameInterner {
    names: Vec<Box<[u8]>>,
    map: FxHashMap<Box<[u8]>, u32>,
}

impl Default for NameInterner {
    fn default() -> Self {
        let mut s = Self {
            names: Vec::new(),
            map: FxHashMap::default(),
        };
        s.intern(b"");
        s.intern(b"(COMPLEX)");
        s
    }
}

impl NameInterner {
    pub fn intern(&mut self, raw: &[u8]) -> u32 {
        let mut stack = [0u8; 96];
        let upper: &[u8] = if raw.len() <= stack.len() {
            for (d, s) in stack.iter_mut().zip(raw) {
                *d = s.to_ascii_uppercase();
            }
            &stack[..raw.len()]
        } else {
            return self.intern_owned(raw.to_ascii_uppercase().into_boxed_slice());
        };
        if let Some(&id) = self.map.get(upper) {
            return id;
        }
        self.intern_owned(upper.to_vec().into_boxed_slice())
    }

    fn intern_owned(&mut self, upper: Box<[u8]>) -> u32 {
        if let Some(&id) = self.map.get(&*upper) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.push(upper.clone());
        self.map.insert(upper, id);
        id
    }

    pub fn lookup(&self, raw: &[u8]) -> Option<u32> {
        let upper = raw.to_ascii_uppercase();
        self.map.get(&*upper).copied()
    }

    pub fn get(&self, id: u32) -> &[u8] {
        &self.names[id as usize]
    }

    pub fn get_str(&self, id: u32) -> &str {
        std::str::from_utf8(self.get(id)).unwrap_or("?")
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

pub struct Scanner<'a> {
    buf: &'a [u8],
    pos: usize,
    limit: usize,
}

impl<'a> Scanner<'a> {
    pub fn new(buf: &'a [u8], start: Offset) -> Self {
        Self {
            buf,
            pos: start as usize,
            limit: buf.len(),
        }
    }

    pub fn pos(&self) -> Offset {
        self.pos as Offset
    }

    fn skip_trivia(&mut self) -> u16 {
        let mut fl = 0;
        while self.pos < self.limit {
            match self.buf[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' | 0 => self.pos += 1,
                0xEF if self.pos == 0 && self.buf.starts_with(b"\xEF\xBB\xBF") => self.pos = 3,
                b'/' if self.pos + 1 < self.limit && self.buf[self.pos + 1] == b'*' => {
                    let e = skip_comment(self.buf, self.pos, self.limit);
                    if e == self.limit && !self.buf[..e].ends_with(b"*/") {
                        fl |= flags::UNTERMINATED_COMMENT;
                    }
                    self.pos = e;
                }
                _ => break,
            }
        }
        fl
    }

    fn skip_blanks(&mut self) {
        while self.pos < self.limit {
            match self.buf[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                b'/' if self.pos + 1 < self.limit && self.buf[self.pos + 1] == b'*' => {
                    self.pos = skip_comment(self.buf, self.pos, self.limit);
                }
                _ => break,
            }
        }
    }

    fn end_of_line(&self, from: usize) -> usize {
        match memchr::memchr(b'\n', &self.buf[from..self.limit]) {
            Some(k) => from + k,
            None => self.limit,
        }
    }

    /// Scan forward from `self.pos` to the terminating `;`, skipping strings and
    /// comments. Returns the exclusive end offset and recovery flags.
    fn terminator(&mut self, start: usize) -> (usize, u16) {
        let buf = self.buf;
        let mut p = self.pos;
        let mut fl = 0u16;
        loop {
            let cand = match memchr::memchr3(b';', b'\'', b'\n', &buf[p..self.limit]) {
                Some(k) => p + k,
                None => self.limit,
            };
            if let Some(k) = memchr::memchr(b'/', &buf[p..cand]) {
                let slash = p + k;
                if slash + 1 < self.limit && buf[slash + 1] == b'*' {
                    let e = skip_comment(buf, slash, self.limit);
                    if e == self.limit && !buf[..e].ends_with(b"*/") {
                        fl |= flags::UNTERMINATED_COMMENT | flags::MALFORMED;
                        return (self.limit, fl);
                    }
                    p = e;
                } else {
                    p = slash + 1;
                }
                continue;
            }
            if cand >= self.limit {
                return (self.limit, fl | flags::MISSING_SEMICOLON | flags::MALFORMED);
            }
            match buf[cand] {
                b';' => return (cand + 1, fl),
                b'\'' => match skip_string(buf, cand, self.limit) {
                    StringEnd::Closed(e) => {
                        if memchr::memchr(b'\n', &buf[cand..e]).is_some() {
                            fl |= flags::MULTILINE;
                        }
                        p = e;
                    }
                    StringEnd::Unterminated(e) => {
                        return (e, fl | flags::UNTERMINATED_STRING | flags::MALFORMED);
                    }
                },
                _ => {
                    // Newline: a new record on the next line means this one lost its `;`.
                    if looks_like_entity_start(buf, cand + 1, self.limit) {
                        let _ = start;
                        return (cand, fl | flags::MISSING_SEMICOLON | flags::MALFORMED);
                    }
                    fl |= flags::MULTILINE;
                    p = cand + 1;
                }
            }
        }
    }

    /// Offset where the next record would start, without consuming it.
    pub fn peek_start(&mut self) -> Option<Offset> {
        self.skip_trivia();
        (self.pos < self.limit).then_some(self.pos as Offset)
    }

    pub fn next_record(&mut self, names: &mut NameInterner) -> Option<EntityRecord> {
        let trivia_flags = self.skip_trivia();
        if self.pos >= self.limit {
            return None;
        }
        let buf = self.buf;
        let start = self.pos;
        let mut fl = trivia_flags;
        let b = buf[start];
        if b == b'#' {
            let mut p = start + 1;
            let mut id: u64 = 0;
            while p < self.limit && buf[p].is_ascii_digit() {
                id = id.saturating_mul(10).saturating_add((buf[p] - b'0') as u64);
                p += 1;
            }
            if p == start + 1 || id == 0 || id > u32::MAX as u64 {
                fl |= flags::BAD_ID | flags::MALFORMED;
                id = 0;
            }
            self.pos = p;
            self.skip_blanks();
            if self.pos >= self.limit || buf[self.pos] != b'=' {
                let end = self.end_of_line(start);
                self.pos = end;
                return Some(EntityRecord {
                    start: start as Offset,
                    end: end as Offset,
                    id: id as u32,
                    name: NAME_NONE,
                    flags: fl | flags::NO_EQUALS | flags::MALFORMED,
                });
            }
            self.pos += 1;
            self.skip_blanks();
            let name = if self.pos < self.limit && buf[self.pos] == b'(' {
                fl |= flags::COMPLEX;
                NAME_COMPLEX
            } else if self.pos < self.limit && is_ident_start(buf[self.pos]) {
                let ns = self.pos;
                while self.pos < self.limit && is_ident_continue(buf[self.pos]) {
                    self.pos += 1;
                }
                names.intern(&buf[ns..self.pos])
            } else {
                fl |= flags::NO_NAME | flags::MALFORMED;
                NAME_NONE
            };
            let (end, tf) = self.terminator(start);
            self.pos = end;
            if memchr::memchr(b'\n', &buf[start..end]).is_some() {
                fl |= flags::MULTILINE;
            }
            return Some(EntityRecord {
                start: start as Offset,
                end: end as Offset,
                id: id as u32,
                name,
                flags: fl | tf,
            });
        }
        if is_ident_start(b) {
            let mut p = start + 1;
            while p < self.limit && is_ident_continue(buf[p]) {
                p += 1;
            }
            let name = names.intern(&buf[start..p]);
            self.pos = p;
            self.skip_blanks();
            if self.pos < self.limit && buf[self.pos] == b';' {
                self.pos += 1;
                return Some(EntityRecord {
                    start: start as Offset,
                    end: self.pos as Offset,
                    id: 0,
                    name,
                    flags: fl | flags::KEYWORD,
                });
            }
            let (end, tf) = self.terminator(start);
            self.pos = end;
            return Some(EntityRecord {
                start: start as Offset,
                end: end as Offset,
                id: 0,
                name,
                flags: fl | tf | flags::HEADER,
            });
        }
        // Garbage: consume to the end of the line (at least one byte).
        let end = self.end_of_line(start + 1).max(start + 1).min(self.limit);
        self.pos = end;
        Some(EntityRecord {
            start: start as Offset,
            end: end as Offset,
            id: 0,
            name: NAME_NONE,
            flags: fl | flags::GARBAGE | flags::MALFORMED,
        })
    }
}

pub fn scan_all(buf: &[u8], names: &mut NameInterner) -> Vec<EntityRecord> {
    let mut out = Vec::with_capacity(buf.len() / 96 + 16);
    let mut sc = Scanner::new(buf, 0);
    while let Some(r) = sc.next_record(names) {
        out.push(r);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(src: &str) -> (Vec<EntityRecord>, NameInterner) {
        let mut names = NameInterner::default();
        let recs = scan_all(src.as_bytes(), &mut names);
        (recs, names)
    }

    fn text<'a>(src: &'a str, r: &EntityRecord) -> &'a str {
        &src[r.start as usize..r.end as usize]
    }

    #[test]
    fn header_and_data() {
        let src = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1=IFCWALL('a',#2);\n#2 = IFCOWNERHISTORY($);\nENDSEC;\nEND-ISO-10303-21;\n";
        let (recs, names) = scan(src);
        let kinds: Vec<(&str, u32, u16)> = recs
            .iter()
            .map(|r| (names.get_str(r.name), r.id, r.flags))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("ISO-10303-21", 0, flags::KEYWORD),
                ("HEADER", 0, flags::KEYWORD),
                ("FILE_SCHEMA", 0, flags::HEADER),
                ("ENDSEC", 0, flags::KEYWORD),
                ("DATA", 0, flags::KEYWORD),
                ("IFCWALL", 1, 0),
                ("IFCOWNERHISTORY", 2, 0),
                ("ENDSEC", 0, flags::KEYWORD),
                ("END-ISO-10303-21", 0, flags::KEYWORD),
            ]
        );
        assert_eq!(text(src, &recs[6]), "#2 = IFCOWNERHISTORY($);");
    }

    #[test]
    fn semicolons_inside_strings_and_comments() {
        let src = "#1=IFCX('a;b''c;',/* ; */ #2);\n#2=IFCY();";
        let (recs, _) = scan(src);
        assert_eq!(recs.len(), 2);
        assert_eq!(text(src, &recs[0]), "#1=IFCX('a;b''c;',/* ; */ #2);");
        assert_eq!(recs[0].flags, 0);
    }

    #[test]
    fn multiline_entity() {
        let src = "#1=IFCX(\n  #2,\n  'x'\n);\n#2=IFCY();";
        let (recs, _) = scan(src);
        assert_eq!(recs.len(), 2);
        assert_eq!(text(src, &recs[0]), "#1=IFCX(\n  #2,\n  'x'\n);");
        assert_eq!(recs[0].flags, flags::MULTILINE);
    }

    #[test]
    fn missing_semicolon_recovers_at_next_entity() {
        let src = "#1=IFCX('a')\n#2=IFCY();";
        let (recs, _) = scan(src);
        assert_eq!(recs.len(), 2);
        assert_eq!(text(src, &recs[0]), "#1=IFCX('a')");
        assert!(recs[0].flags & flags::MISSING_SEMICOLON != 0);
        assert_eq!(recs[1].id, 2);
        assert_eq!(recs[1].flags, 0);
    }

    #[test]
    fn unterminated_string_recovers() {
        let src = "#1=IFCX('a);\n#2=IFCY();\n";
        let (recs, _) = scan(src);
        assert_eq!(recs.len(), 2);
        assert!(recs[0].flags & flags::UNTERMINATED_STRING != 0);
        assert_eq!(recs[1].id, 2);
    }

    #[test]
    fn complex_and_malformed() {
        let src = "#1=(IFCA(1)IFCB(2));\n#2 IFCX();\n#=IFCX();\n#3=;\nhello world\n#4=IFCZ();";
        let (recs, names) = scan(src);
        assert_eq!(recs.len(), 6);
        assert!(recs[0].is_complex());
        assert_eq!(recs[0].name, NAME_COMPLEX);
        assert!(recs[1].flags & flags::NO_EQUALS != 0);
        assert_eq!(text(src, &recs[1]), "#2 IFCX();");
        assert!(recs[2].flags & flags::BAD_ID != 0);
        assert!(recs[3].flags & flags::NO_NAME != 0);
        assert_eq!(text(src, &recs[3]), "#3=;");
        assert!(recs[4].flags & flags::MISSING_SEMICOLON != 0);
        assert_eq!(text(src, &recs[4]), "hello world");
        assert_eq!(names.get_str(recs[5].name), "IFCZ");
        assert_eq!(recs[5].id, 4);
    }

    #[test]
    fn interner_is_case_insensitive() {
        let mut n = NameInterner::default();
        let a = n.intern(b"IfcWall");
        let b = n.intern(b"IFCWALL");
        assert_eq!(a, b);
        assert_eq!(n.get_str(a), "IFCWALL");
        assert_eq!(n.lookup(b"ifcwall"), Some(a));
    }
}
