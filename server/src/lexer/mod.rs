//! Zero-allocation tokenizer for STEP Part 21 text.

use crate::text::Offset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Semicolon,
    Eq,
    LParen,
    RParen,
    Comma,
    Dollar,
    Star,
    /// `#123`
    Ref,
    /// Entity, type or keyword name.
    Ident,
    /// `'...'` including quotes.
    String,
    /// `.ENUM.` including dots.
    Enum,
    Integer,
    Real,
    /// `"0FA3"` including quotes.
    Binary,
    /// Any byte sequence that is not a token.
    Error,
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: Offset,
    pub end: Offset,
}

impl Token {
    pub fn text<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        &buf[self.start as usize..self.end as usize]
    }
}

pub struct Lexer<'a> {
    buf: &'a [u8],
    pos: usize,
    end: usize,
}

#[inline]
pub fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

#[inline]
pub fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Result of scanning a string body.
pub enum StringEnd {
    /// Offset one past the closing quote.
    Closed(usize),
    /// No closing quote before `limit` or a newline; offset where scanning stopped.
    Unterminated(usize),
}

/// Scan a string whose opening quote sits at `quote_pos`. Doubled quotes are an
/// escaped quote. A raw newline terminates the scan when the following line looks
/// like the start of another entity, so one missing quote cannot swallow the file.
pub fn skip_string(buf: &[u8], quote_pos: usize, limit: usize) -> StringEnd {
    let mut p = quote_pos + 1;
    loop {
        match memchr::memchr2(b'\'', b'\n', &buf[p..limit]) {
            None => return StringEnd::Unterminated(limit),
            Some(k) => {
                let at = p + k;
                if buf[at] == b'\n' {
                    if looks_like_entity_start(buf, at + 1, limit) {
                        return StringEnd::Unterminated(at);
                    }
                    p = at + 1;
                } else if at + 1 < limit && buf[at + 1] == b'\'' {
                    p = at + 2;
                } else {
                    return StringEnd::Closed(at + 1);
                }
            }
        }
    }
}

/// Offset one past `*/` for a comment opening at `pos` (pointing at `/`), or `limit`.
pub fn skip_comment(buf: &[u8], pos: usize, limit: usize) -> usize {
    let body = pos + 2;
    if body > limit {
        return limit;
    }
    match memchr::memmem::find(&buf[body..limit], b"*/") {
        Some(k) => body + k + 2,
        None => limit,
    }
}

/// True when the text at `p` (after optional blanks) is `#<digits> =` or an
/// upper-case keyword followed by `;` or `(`, i.e. the start of a new record.
pub fn looks_like_entity_start(buf: &[u8], mut p: usize, limit: usize) -> bool {
    while p < limit && matches!(buf[p], b' ' | b'\t' | b'\r') {
        p += 1;
    }
    if p >= limit {
        return true;
    }
    if buf[p] == b'#' {
        let mut q = p + 1;
        while q < limit && buf[q].is_ascii_digit() {
            q += 1;
        }
        if q == p + 1 {
            return false;
        }
        while q < limit && matches!(buf[q], b' ' | b'\t') {
            q += 1;
        }
        return q < limit && buf[q] == b'=';
    }
    if buf[p].is_ascii_uppercase() {
        let mut q = p;
        while q < limit
            && (buf[q].is_ascii_uppercase()
                || buf[q].is_ascii_digit()
                || matches!(buf[q], b'_' | b'-'))
        {
            q += 1;
        }
        return q < limit && matches!(buf[q], b';' | b'(');
    }
    false
}

impl<'a> Lexer<'a> {
    pub fn new(buf: &'a [u8], start: Offset, end: Offset) -> Self {
        Self {
            buf,
            pos: start as usize,
            end: end as usize,
        }
    }

    pub fn pos(&self) -> Offset {
        self.pos as Offset
    }

    fn skip_trivia(&mut self) {
        while self.pos < self.end {
            match self.buf[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                b'/' if self.pos + 1 < self.end && self.buf[self.pos + 1] == b'*' => {
                    self.pos = skip_comment(self.buf, self.pos, self.end);
                }
                _ => break,
            }
        }
    }

    pub fn peek(&mut self) -> Token {
        let save = self.pos;
        let t = self.next_token();
        self.pos = save;
        t
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_trivia();
        let start = self.pos;
        if start >= self.end {
            return Token {
                kind: TokenKind::Eof,
                start: start as Offset,
                end: start as Offset,
            };
        }
        let b = self.buf[start];
        let (kind, end) = match b {
            b';' => (TokenKind::Semicolon, start + 1),
            b'=' => (TokenKind::Eq, start + 1),
            b'(' => (TokenKind::LParen, start + 1),
            b')' => (TokenKind::RParen, start + 1),
            b',' => (TokenKind::Comma, start + 1),
            b'$' => (TokenKind::Dollar, start + 1),
            b'*' => (TokenKind::Star, start + 1),
            b'#' => {
                let mut p = start + 1;
                while p < self.end && self.buf[p].is_ascii_digit() {
                    p += 1;
                }
                if p == start + 1 {
                    (TokenKind::Error, p)
                } else {
                    (TokenKind::Ref, p)
                }
            }
            b'\'' => match skip_string(self.buf, start, self.end) {
                StringEnd::Closed(p) => (TokenKind::String, p),
                StringEnd::Unterminated(p) => (TokenKind::Error, p),
            },
            b'"' => {
                let mut p = start + 1;
                while p < self.end && self.buf[p].is_ascii_hexdigit() {
                    p += 1;
                }
                if p < self.end && self.buf[p] == b'"' {
                    (TokenKind::Binary, p + 1)
                } else {
                    (TokenKind::Error, p)
                }
            }
            b'.' => {
                if start + 1 < self.end && self.buf[start + 1].is_ascii_digit() {
                    self.number(start)
                } else {
                    let mut p = start + 1;
                    while p < self.end
                        && (self.buf[p].is_ascii_alphanumeric() || self.buf[p] == b'_')
                    {
                        p += 1;
                    }
                    if p > start + 1 && p < self.end && self.buf[p] == b'.' {
                        (TokenKind::Enum, p + 1)
                    } else {
                        (TokenKind::Error, p.max(start + 1))
                    }
                }
            }
            b'+' | b'-' => {
                if start + 1 < self.end
                    && (self.buf[start + 1].is_ascii_digit() || self.buf[start + 1] == b'.')
                {
                    self.number(start)
                } else {
                    (TokenKind::Error, start + 1)
                }
            }
            b'0'..=b'9' => self.number(start),
            _ if is_ident_start(b) => {
                let mut p = start + 1;
                while p < self.end && is_ident_continue(self.buf[p]) {
                    p += 1;
                }
                (TokenKind::Ident, p)
            }
            _ => {
                // Consume one UTF-8 scalar so error tokens never split a character.
                let mut p = start + 1;
                while p < self.end && (self.buf[p] & 0xC0) == 0x80 {
                    p += 1;
                }
                (TokenKind::Error, p)
            }
        };
        self.pos = end;
        Token {
            kind,
            start: start as Offset,
            end: end as Offset,
        }
    }

    fn number(&self, start: usize) -> (TokenKind, usize) {
        let buf = self.buf;
        let mut p = start;
        if matches!(buf[p], b'+' | b'-') {
            p += 1;
        }
        let mut real = false;
        while p < self.end && buf[p].is_ascii_digit() {
            p += 1;
        }
        if p < self.end && buf[p] == b'.' {
            real = true;
            p += 1;
            while p < self.end && buf[p].is_ascii_digit() {
                p += 1;
            }
        }
        if p < self.end && matches!(buf[p], b'E' | b'e') {
            let mut q = p + 1;
            if q < self.end && matches!(buf[q], b'+' | b'-') {
                q += 1;
            }
            if q < self.end && buf[q].is_ascii_digit() {
                real = true;
                while q < self.end && buf[q].is_ascii_digit() {
                    q += 1;
                }
                p = q;
            }
        }
        (
            if real {
                TokenKind::Real
            } else {
                TokenKind::Integer
            },
            p,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(TokenKind, &str)> {
        let buf = src.as_bytes();
        let mut lx = Lexer::new(buf, 0, buf.len() as Offset);
        let mut out = Vec::new();
        loop {
            let t = lx.next_token();
            if t.kind == TokenKind::Eof {
                break;
            }
            out.push((t.kind, std::str::from_utf8(t.text(buf)).unwrap()));
        }
        out
    }

    #[test]
    fn tokens() {
        use TokenKind::*;
        assert_eq!(
            kinds("#12=IFCSITE('a''b',#1,$,*,.ADDED.,1.5E-3,-7,\"0F\",/*c*/ .T.);"),
            vec![
                (Ref, "#12"),
                (Eq, "="),
                (Ident, "IFCSITE"),
                (LParen, "("),
                (String, "'a''b'"),
                (Comma, ","),
                (Ref, "#1"),
                (Comma, ","),
                (Dollar, "$"),
                (Comma, ","),
                (Star, "*"),
                (Comma, ","),
                (Enum, ".ADDED."),
                (Comma, ","),
                (Real, "1.5E-3"),
                (Comma, ","),
                (Integer, "-7"),
                (Comma, ","),
                (Binary, "\"0F\""),
                (Comma, ","),
                (Enum, ".T."),
                (RParen, ")"),
                (Semicolon, ";"),
            ]
        );
        assert_eq!(
            kinds("ISO-10303-21;"),
            vec![(Ident, "ISO-10303-21"), (Semicolon, ";")]
        );
        assert_eq!(
            kinds("1. .5 0.E+2"),
            vec![(Real, "1."), (Real, ".5"), (Real, "0.E+2")]
        );
        assert_eq!(kinds("é"), vec![(Error, "é")]);
    }

    #[test]
    fn unterminated_string_stops_at_next_entity() {
        let src = "#1=IFCX('oops);\n#2=IFCY();";
        let toks = kinds(src);
        assert_eq!(toks[3], (TokenKind::LParen, "("));
        assert_eq!(toks[4].0, TokenKind::Error);
        assert_eq!(toks[4].1, "'oops);");
        assert_eq!(toks[5], (TokenKind::Ref, "#2"));
    }

    #[test]
    fn string_may_span_lines_when_next_line_is_not_an_entity() {
        assert_eq!(kinds("'a\nb'"), vec![(TokenKind::String, "'a\nb'")]);
    }

    #[test]
    fn entity_start_detection() {
        assert!(looks_like_entity_start(b"  #12 = IFCX();", 0, 15));
        assert!(looks_like_entity_start(b"ENDSEC;", 0, 7));
        assert!(looks_like_entity_start(b"FILE_NAME(", 0, 10));
        assert!(!looks_like_entity_start(b"#12,#13", 0, 7));
        assert!(!looks_like_entity_start(b"  continued text", 0, 16));
    }
}
