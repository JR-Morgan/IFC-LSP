//! On-demand parser for a single record's argument list.

use crate::lexer::{Lexer, Token, TokenKind};
use crate::text::Offset;

/// Lists longer than this keep only a prefix of their elements; `total` still
/// reports the full count.
pub const LIST_KEEP: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Derived,
    Ref(u32),
    Str,
    Enum,
    Int,
    Real,
    Binary,
    Typed {
        name_start: Offset,
        name_end: Offset,
        inner: Box<ValueNode>,
    },
    List {
        items: Vec<ValueNode>,
        total: usize,
    },
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValueNode {
    pub value: Value,
    pub start: Offset,
    pub end: Offset,
}

impl ValueNode {
    pub fn kind_name(&self) -> &'static str {
        match &self.value {
            Value::Null => "$",
            Value::Derived => "*",
            Value::Ref(_) => "reference",
            Value::Str => "string",
            Value::Enum => "enumeration",
            Value::Int => "integer",
            Value::Real => "real",
            Value::Binary => "binary",
            Value::Typed { .. } => "typed value",
            Value::List { .. } => "list",
            Value::Error => "invalid value",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyntaxError {
    pub start: Offset,
    pub end: Offset,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPart {
    pub name_start: Offset,
    pub name_end: Offset,
    pub args: Vec<ValueNode>,
    pub args_start: Offset,
    pub args_end: Offset,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedRecord {
    /// One part for a simple instance, several for a complex one.
    pub parts: Vec<ParsedPart>,
    pub errors: Vec<SyntaxError>,
}

struct Parser<'a> {
    buf: &'a [u8],
    lx: Lexer<'a>,
    la: Token,
    errors: Vec<SyntaxError>,
}

impl<'a> Parser<'a> {
    fn new(buf: &'a [u8], start: Offset, end: Offset) -> Self {
        let mut lx = Lexer::new(buf, start, end);
        let la = lx.next_token();
        Self {
            buf,
            lx,
            la,
            errors: Vec::new(),
        }
    }

    fn bump(&mut self) -> Token {
        let t = self.la;
        self.la = self.lx.next_token();
        t
    }

    fn error(&mut self, start: Offset, end: Offset, message: impl Into<String>) {
        if self.errors.len() < 16 {
            self.errors.push(SyntaxError {
                start,
                end: end.max(start + 1),
                message: message.into(),
            });
        }
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> bool {
        if self.la.kind == kind {
            self.bump();
            true
        } else {
            let t = self.la;
            self.error(t.start, t.end, format!("expected {what}"));
            false
        }
    }

    /// `( args )` with the opening paren as the lookahead. Returns `(args, start, end)`.
    fn arg_list(&mut self) -> (Vec<ValueNode>, usize, Offset, Offset) {
        let open = self.bump();
        let mut items = Vec::new();
        let mut total = 0usize;
        if self.la.kind == TokenKind::RParen {
            let close = self.bump();
            return (items, 0, open.start, close.end);
        }
        loop {
            let v = self.value();
            total += 1;
            if items.len() < LIST_KEEP {
                items.push(v);
            }
            match self.la.kind {
                TokenKind::Comma => {
                    self.bump();
                    if self.la.kind == TokenKind::RParen {
                        let t = self.la;
                        self.error(t.start, t.end, "trailing comma");
                        let close = self.bump();
                        return (items, total, open.start, close.end);
                    }
                }
                TokenKind::RParen => {
                    let close = self.bump();
                    return (items, total, open.start, close.end);
                }
                TokenKind::Eof | TokenKind::Semicolon => {
                    let t = self.la;
                    self.error(t.start, t.end, "expected ')'");
                    return (items, total, open.start, t.start);
                }
                _ => {
                    let t = self.la;
                    self.error(t.start, t.end, "expected ',' or ')'");
                    self.recover();
                    if self.la.kind == TokenKind::RParen {
                        let close = self.bump();
                        return (items, total, open.start, close.end);
                    }
                    if matches!(self.la.kind, TokenKind::Eof | TokenKind::Semicolon) {
                        return (items, total, open.start, self.la.start);
                    }
                    if self.la.kind == TokenKind::Comma {
                        self.bump();
                    }
                }
            }
        }
    }

    /// Skip to the next `,` or `)` at the current nesting level.
    fn recover(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.la.kind {
                TokenKind::Eof | TokenKind::Semicolon => return,
                TokenKind::Comma if depth == 0 => return,
                TokenKind::RParen => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                TokenKind::LParen => depth += 1,
                _ => {}
            }
            self.bump();
        }
    }

    fn value(&mut self) -> ValueNode {
        let t = self.la;
        let simple = |value| ValueNode {
            value,
            start: t.start,
            end: t.end,
        };
        match t.kind {
            TokenKind::Dollar => {
                self.bump();
                simple(Value::Null)
            }
            TokenKind::Star => {
                self.bump();
                simple(Value::Derived)
            }
            TokenKind::Ref => {
                self.bump();
                let id = std::str::from_utf8(&self.buf[t.start as usize + 1..t.end as usize])
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                if id == 0 {
                    self.error(t.start, t.end, "invalid instance reference");
                    return simple(Value::Error);
                }
                simple(Value::Ref(id))
            }
            TokenKind::String => {
                self.bump();
                simple(Value::Str)
            }
            TokenKind::Enum => {
                self.bump();
                simple(Value::Enum)
            }
            TokenKind::Integer => {
                self.bump();
                simple(Value::Int)
            }
            TokenKind::Real => {
                self.bump();
                simple(Value::Real)
            }
            TokenKind::Binary => {
                self.bump();
                simple(Value::Binary)
            }
            TokenKind::LParen => {
                let (items, total, start, end) = self.arg_list();
                ValueNode {
                    value: Value::List { items, total },
                    start,
                    end,
                }
            }
            TokenKind::Ident => {
                self.bump();
                if self.la.kind != TokenKind::LParen {
                    let la = self.la;
                    self.error(la.start, la.end, "expected '(' after type name");
                    return ValueNode {
                        value: Value::Error,
                        start: t.start,
                        end: t.end,
                    };
                }
                let (mut items, total, _, end) = self.arg_list();
                if total != 1 {
                    self.error(t.start, end, "a typed value takes exactly one argument");
                    return ValueNode {
                        value: Value::Error,
                        start: t.start,
                        end,
                    };
                }
                let inner = Box::new(items.pop().unwrap());
                ValueNode {
                    value: Value::Typed {
                        name_start: t.start,
                        name_end: t.end,
                        inner,
                    },
                    start: t.start,
                    end,
                }
            }
            TokenKind::Comma | TokenKind::RParen => {
                self.error(t.start, t.end, "missing value");
                ValueNode {
                    value: Value::Error,
                    start: t.start,
                    end: t.start,
                }
            }
            _ => {
                self.bump();
                let msg = match t.kind {
                    TokenKind::Error if self.buf.get(t.start as usize) == Some(&b'\'') => {
                        "unterminated string"
                    }
                    TokenKind::Error if self.buf.get(t.start as usize) == Some(&b'.') => {
                        "malformed enumeration value"
                    }
                    _ => "unexpected token",
                };
                self.error(t.start, t.end, msg);
                self.recover();
                ValueNode {
                    value: Value::Error,
                    start: t.start,
                    end: t.end,
                }
            }
        }
    }

    fn part(&mut self) -> Option<ParsedPart> {
        let name = self.la;
        if name.kind != TokenKind::Ident {
            self.error(name.start, name.end, "expected entity name");
            return None;
        }
        self.bump();
        if self.la.kind != TokenKind::LParen {
            let la = self.la;
            self.error(la.start, la.end, "expected '('");
            return Some(ParsedPart {
                name_start: name.start,
                name_end: name.end,
                args: Vec::new(),
                args_start: la.start,
                args_end: la.start,
            });
        }
        let (args, _, args_start, args_end) = self.arg_list();
        Some(ParsedPart {
            name_start: name.start,
            name_end: name.end,
            args,
            args_start,
            args_end,
        })
    }
}

/// Parse a record body. `body_start` must point at the constructor name (after
/// `#id=`) or at the `(` of a complex instance; for header records it is the
/// record start.
pub fn parse_record(buf: &[u8], body_start: Offset, end: Offset) -> ParsedRecord {
    let mut p = Parser::new(buf, body_start, end);
    let mut parts = Vec::new();
    if p.la.kind == TokenKind::LParen {
        let open = p.bump();
        while p.la.kind == TokenKind::Ident {
            if let Some(part) = p.part() {
                parts.push(part);
            } else {
                break;
            }
        }
        if parts.is_empty() {
            p.error(open.start, open.end, "complex instance has no parts");
        }
        p.expect(TokenKind::RParen, "')' closing the complex instance");
    } else if let Some(part) = p.part() {
        parts.push(part);
    }
    if !matches!(p.la.kind, TokenKind::Semicolon | TokenKind::Eof) {
        let t = p.la;
        p.error(t.start, t.end, "unexpected text after the argument list");
    }
    ParsedRecord {
        parts,
        errors: p.errors,
    }
}

/// Offset of the constructor name for a `#id=NAME(...)` record, i.e. the byte
/// after `=` and any blanks.
pub fn body_start(buf: &[u8], start: Offset, end: Offset) -> Offset {
    let s = start as usize;
    let e = end as usize;
    match memchr::memchr(b'=', &buf[s..e]) {
        Some(k) => {
            let mut p = s + k + 1;
            while p < e && matches!(buf[p], b' ' | b'\t' | b'\r' | b'\n') {
                p += 1;
            }
            p as Offset
        }
        None => start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> ParsedRecord {
        let buf = src.as_bytes();
        let bs = body_start(buf, 0, buf.len() as Offset);
        parse_record(buf, bs, buf.len() as Offset)
    }

    fn kinds(args: &[ValueNode]) -> Vec<&'static str> {
        args.iter().map(|a| a.kind_name()).collect()
    }

    #[test]
    fn simple_entity() {
        let r = parse("#1=IFCSITE('x',#2,$,*,.ADDED.,1,2.,\"0F\",IFCLABEL('t'),(1.,2.),());");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.parts.len(), 1);
        let p = &r.parts[0];
        assert_eq!(
            &"#1=IFCSITE("[p.name_start as usize..p.name_end as usize],
            "IFCSITE"
        );
        assert_eq!(
            kinds(&p.args),
            vec![
                "string",
                "reference",
                "$",
                "*",
                "enumeration",
                "integer",
                "real",
                "binary",
                "typed value",
                "list",
                "list"
            ]
        );
        assert_eq!(p.args[1].value, Value::Ref(2));
        match &p.args[9].value {
            Value::List { items, total } => {
                assert_eq!(*total, 2);
                assert_eq!(items[1].value, Value::Real);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn complex_entity() {
        let r = parse("#1=(IFCA(1)IFCB('x',#2));");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.parts.len(), 2);
        assert_eq!(kinds(&r.parts[1].args), vec!["string", "reference"]);
    }

    #[test]
    fn header_record() {
        let r = parse("FILE_SCHEMA(('IFC4'));");
        assert!(r.errors.is_empty());
        assert_eq!(kinds(&r.parts[0].args), vec!["list"]);
    }

    #[test]
    fn long_lists_are_truncated_but_counted() {
        let body: Vec<String> = (0..1000).map(|i| format!("({i}.,0.,0.)")).collect();
        let src = format!("#1=IFCCARTESIANPOINTLIST3D(({}));", body.join(","));
        let r = parse(&src);
        assert!(r.errors.is_empty());
        match &r.parts[0].args[0].value {
            Value::List { items, total } => {
                assert_eq!(*total, 1000);
                assert_eq!(items.len(), LIST_KEEP);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn errors_are_reported_and_recovered() {
        let r = parse("#1=IFCX('a',,#2);");
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].message, "missing value");
        assert_eq!(
            kinds(&r.parts[0].args),
            vec!["string", "invalid value", "reference"]
        );

        let r = parse("#1=IFCX('a',#2;");
        assert!(r.errors.iter().any(|e| e.message == "expected ')'"));

        let r = parse("#1=IFCX(IFCLABEL('a','b'));");
        assert!(
            r.errors
                .iter()
                .any(|e| e.message.contains("exactly one argument"))
        );

        let r = parse("#1=IFCX('a',);");
        assert!(
            r.errors.iter().any(|e| e.message == "trailing comma"),
            "{:?}",
            r.errors
        );

        let r = parse("#1=IFCX;");
        assert!(r.errors.iter().any(|e| e.message == "expected '('"));

        let r = parse("#1=IFCX('abc);");
        assert!(r.errors.iter().any(|e| e.message == "unterminated string"));
    }
}
