//! Parser for the subset of EXPRESS (ISO 10303-11) needed to describe entity
//! instances in a Part 21 file: schema name, TYPE and ENTITY declarations.
//! FUNCTION, RULE, WHERE, INVERSE and UNIQUE blocks are skipped.

use super::{AggKind, Attribute, EntityDef, NamedRef, Schema, Simple, TypeDef, TypeExpr, TypeKind};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Int(u64),
    Other(String),
    Punct(char),
    Eof,
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    /// Start offset of the token most recently returned by `next`.
    tok_start: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
            tok_start: 0,
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            if self.src[self.pos..].starts_with(b"(*") {
                let mut depth = 1;
                self.pos += 2;
                while self.pos < self.src.len() && depth > 0 {
                    if self.src[self.pos..].starts_with(b"(*") {
                        depth += 1;
                        self.pos += 2;
                    } else if self.src[self.pos..].starts_with(b"*)") {
                        depth -= 1;
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
            } else if self.src[self.pos..].starts_with(b"--") {
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
            } else {
                return;
            }
        }
    }

    fn next(&mut self) -> Tok {
        self.skip_trivia();
        self.tok_start = self.pos;
        if self.pos >= self.src.len() {
            return Tok::Eof;
        }
        let b = self.src[self.pos];
        if b.is_ascii_alphabetic() || b == b'_' {
            let s = self.pos;
            while self.pos < self.src.len()
                && (self.src[self.pos].is_ascii_alphanumeric() || self.src[self.pos] == b'_')
            {
                self.pos += 1;
            }
            return Tok::Ident(String::from_utf8_lossy(&self.src[s..self.pos]).into_owned());
        }
        if b.is_ascii_digit() {
            let s = self.pos;
            while self.pos < self.src.len()
                && (self.src[self.pos].is_ascii_alphanumeric() || self.src[self.pos] == b'.')
            {
                self.pos += 1;
            }
            let text = std::str::from_utf8(&self.src[s..self.pos]).unwrap_or("");
            return match text.parse::<u64>() {
                Ok(n) => Tok::Int(n),
                Err(_) => Tok::Other(text.to_string()),
            };
        }
        if b == b'\'' {
            let s = self.pos;
            self.pos += 1;
            while self.pos < self.src.len() {
                if self.src[self.pos] == b'\'' {
                    if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'\'' {
                        self.pos += 2;
                        continue;
                    }
                    self.pos += 1;
                    break;
                }
                self.pos += 1;
            }
            return Tok::Other(String::from_utf8_lossy(&self.src[s..self.pos]).into_owned());
        }
        self.pos += 1;
        Tok::Punct(b as char)
    }
}

fn ident_eq(t: &Tok, kw: &str) -> bool {
    matches!(t, Tok::Ident(s) if s.eq_ignore_ascii_case(kw))
}

pub struct Parser<'a> {
    lx: Lexer<'a>,
    peeked: Option<(Tok, usize)>,
    /// Start offset of the token most recently returned by `next`.
    last_start: usize,
    pub errors: Vec<String>,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            lx: Lexer::new(src),
            peeked: None,
            last_start: 0,
            errors: Vec::new(),
        }
    }

    fn peek(&mut self) -> &Tok {
        if self.peeked.is_none() {
            let t = self.lx.next();
            self.peeked = Some((t, self.lx.tok_start));
        }
        &self.peeked.as_ref().unwrap().0
    }

    fn next(&mut self) -> Tok {
        match self.peeked.take() {
            Some((t, p)) => {
                self.last_start = p;
                t
            }
            None => {
                let t = self.lx.next();
                self.last_start = self.lx.tok_start;
                t
            }
        }
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if ident_eq(self.peek(), kw) {
            self.next();
            true
        } else {
            false
        }
    }

    fn eat_punct(&mut self, c: char) -> bool {
        if *self.peek() == Tok::Punct(c) {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect_ident(&mut self) -> Option<String> {
        match self.next() {
            Tok::Ident(s) => Some(s),
            other => {
                self.errors
                    .push(format!("expected identifier, found {other:?}"));
                None
            }
        }
    }

    /// Skip tokens up to and including the next `;`.
    fn skip_statement(&mut self) {
        loop {
            match self.next() {
                Tok::Punct(';') | Tok::Eof => return,
                _ => {}
            }
        }
    }

    /// Skip tokens up to and including `<end_kw> ;`.
    fn skip_until_kw(&mut self, end_kw: &str) {
        loop {
            let t = self.next();
            if t == Tok::Eof {
                return;
            }
            if ident_eq(&t, end_kw) {
                self.eat_punct(';');
                return;
            }
        }
    }

    pub fn parse(mut self) -> Schema {
        let mut schema = Schema::default();
        loop {
            let t = self.next();
            match t {
                Tok::Eof => break,
                Tok::Ident(kw) => match kw.to_ascii_uppercase().as_str() {
                    "SCHEMA" => {
                        if let Some(n) = self.expect_ident() {
                            schema.name = n;
                        }
                        self.skip_statement();
                    }
                    "END_SCHEMA" => {
                        self.skip_statement();
                    }
                    "TYPE" => {
                        if let Some(td) = self.parse_type() {
                            schema.types.push(td);
                        }
                    }
                    "ENTITY" => {
                        if let Some(ed) = self.parse_entity() {
                            schema.entities.push(ed);
                        }
                    }
                    "FUNCTION" => self.skip_until_kw("END_FUNCTION"),
                    "RULE" => self.skip_until_kw("END_RULE"),
                    "PROCEDURE" => self.skip_until_kw("END_PROCEDURE"),
                    "CONSTANT" => self.skip_until_kw("END_CONSTANT"),
                    "REFERENCE" | "USE" => self.skip_statement(),
                    _ => self.skip_statement(),
                },
                _ => {}
            }
        }
        schema.errors = self.errors;
        schema
    }

    fn parse_type(&mut self) -> Option<TypeDef> {
        let name = self.expect_ident()?;
        let decl = self.last_start;
        if !self.eat_punct('=') {
            self.errors.push(format!("TYPE {name}: expected '='"));
            self.skip_until_kw("END_TYPE");
            return None;
        }
        let kind = self.parse_underlying();
        self.skip_until_kw("END_TYPE");
        Some(TypeDef { name, kind, decl })
    }

    fn parse_underlying(&mut self) -> TypeKind {
        self.eat_kw("EXTENSIBLE");
        if self.eat_kw("ENUMERATION") {
            let mut based_on = None;
            if self.eat_kw("BASED_ON") {
                based_on = self.expect_ident();
                self.eat_kw("WITH");
            }
            let mut values = Vec::new();
            let mut value_offsets = Vec::new();
            if (self.eat_kw("OF") || based_on.is_some()) && self.eat_punct('(') {
                loop {
                    match self.next() {
                        Tok::Ident(v) => {
                            values.push(v.to_ascii_uppercase());
                            value_offsets.push(self.last_start);
                        }
                        Tok::Punct(')') | Tok::Eof => break,
                        _ => {}
                    }
                }
            }
            return TypeKind::Enum {
                values,
                value_offsets,
                based_on,
            };
        }
        if self.eat_kw("SELECT") {
            let mut based_on = None;
            if self.eat_kw("BASED_ON") {
                based_on = self.expect_ident();
                self.eat_kw("WITH");
            }
            let mut members = Vec::new();
            if self.eat_punct('(') {
                loop {
                    match self.next() {
                        Tok::Ident(v) => members.push(NamedRef::Unresolved(v)),
                        Tok::Punct(')') | Tok::Eof => break,
                        _ => {}
                    }
                }
            }
            return TypeKind::Select { members, based_on };
        }
        TypeKind::Defined(self.parse_type_expr())
    }

    fn parse_bound(&mut self) -> Option<u32> {
        // A bound is an integer, `?`, or an expression we do not evaluate.
        let mut value = None;
        let mut depth = 0i32;
        loop {
            match self.peek() {
                Tok::Punct(':') | Tok::Punct(']') if depth == 0 => return value,
                Tok::Eof => return value,
                _ => {}
            }
            match self.next() {
                Tok::Int(n) if depth == 0 && value.is_none() => value = u32::try_from(n).ok(),
                Tok::Punct('(') | Tok::Punct('[') => depth += 1,
                Tok::Punct(')') | Tok::Punct(']') => depth -= 1,
                Tok::Punct('?') => return None,
                _ => value = None,
            }
        }
    }

    fn parse_type_expr(&mut self) -> TypeExpr {
        let t = self.next();
        let Tok::Ident(word) = t else {
            self.errors.push(format!("expected type, found {t:?}"));
            return TypeExpr::Named(NamedRef::Unresolved(String::from("?")));
        };
        let upper = word.to_ascii_uppercase();
        let agg = match upper.as_str() {
            "LIST" => Some(AggKind::List),
            "SET" => Some(AggKind::Set),
            "ARRAY" => Some(AggKind::Array),
            "BAG" => Some(AggKind::Bag),
            _ => None,
        };
        if let Some(kind) = agg {
            let (mut lo, mut hi) = (None, None);
            if self.eat_punct('[') {
                lo = self.parse_bound();
                self.eat_punct(':');
                hi = self.parse_bound();
                self.eat_punct(']');
            }
            self.eat_kw("OF");
            self.eat_kw("UNIQUE");
            self.eat_kw("OPTIONAL");
            let elem = Box::new(self.parse_type_expr());
            return TypeExpr::Aggregate { kind, lo, hi, elem };
        }
        let simple = match upper.as_str() {
            "REAL" => Some(Simple::Real),
            "INTEGER" => Some(Simple::Integer),
            "NUMBER" => Some(Simple::Number),
            "BOOLEAN" => Some(Simple::Boolean),
            "LOGICAL" => Some(Simple::Logical),
            "STRING" => Some(Simple::String),
            "BINARY" => Some(Simple::Binary),
            _ => None,
        };
        if let Some(s) = simple {
            if self.eat_punct('(') {
                while !matches!(self.next(), Tok::Punct(')') | Tok::Eof) {}
            }
            self.eat_kw("FIXED");
            return TypeExpr::Simple(s);
        }
        TypeExpr::Named(NamedRef::Unresolved(word))
    }

    fn parse_entity(&mut self) -> Option<EntityDef> {
        let name = self.expect_ident()?;
        let mut def = EntityDef {
            name,
            decl: self.last_start,
            ..Default::default()
        };
        // Header up to the first ';': ABSTRACT, SUPERTYPE OF (...), SUBTYPE OF (...).
        loop {
            match self.next() {
                Tok::Punct(';') | Tok::Eof => break,
                Tok::Ident(w) if w.eq_ignore_ascii_case("ABSTRACT") => def.is_abstract = true,
                Tok::Ident(w) if w.eq_ignore_ascii_case("SUBTYPE") => {
                    self.eat_kw("OF");
                    if self.eat_punct('(') {
                        loop {
                            match self.next() {
                                Tok::Ident(s) => def.supertype_names.push(s),
                                Tok::Punct(')') | Tok::Eof => break,
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        #[derive(PartialEq)]
        enum Section {
            Explicit,
            Derive,
            Other,
        }
        let mut section = Section::Explicit;
        loop {
            let t = self.next();
            match &t {
                Tok::Eof => break,
                Tok::Ident(w) if w.eq_ignore_ascii_case("END_ENTITY") => {
                    self.eat_punct(';');
                    break;
                }
                Tok::Ident(w) if w.eq_ignore_ascii_case("DERIVE") => {
                    section = Section::Derive;
                    continue;
                }
                Tok::Ident(w)
                    if w.eq_ignore_ascii_case("INVERSE")
                        || w.eq_ignore_ascii_case("UNIQUE")
                        || w.eq_ignore_ascii_case("WHERE") =>
                {
                    section = Section::Other;
                    continue;
                }
                _ => {}
            }
            match section {
                Section::Other => {
                    // `label : expression ;`
                    self.skip_statement();
                }
                Section::Explicit | Section::Derive => {
                    // Either `Name : ...` or `SELF \ Super . Name : ...`.
                    let mut redeclared_super = None;
                    let attr_name = match t {
                        Tok::Ident(w) if w.eq_ignore_ascii_case("SELF") => {
                            self.eat_punct('\\');
                            let sup = self.expect_ident();
                            self.eat_punct('.');
                            redeclared_super = sup;
                            match self.expect_ident() {
                                Some(n) => n,
                                None => {
                                    self.skip_statement();
                                    continue;
                                }
                            }
                        }
                        Tok::Ident(w) => w,
                        _ => {
                            self.skip_statement();
                            continue;
                        }
                    };
                    if !self.eat_punct(':') {
                        self.skip_statement();
                        continue;
                    }
                    if section == Section::Derive {
                        if let Some(sup) = redeclared_super {
                            def.derived_redeclarations.push((sup, attr_name));
                        }
                        self.skip_statement();
                        continue;
                    }
                    let optional = self.eat_kw("OPTIONAL");
                    let ty = self.parse_type_expr();
                    self.skip_statement();
                    if let Some(sup) = redeclared_super {
                        // A redeclared explicit attribute keeps its inherited position; the
                        // refined type is applied during resolution.
                        def.explicit_redeclarations
                            .push((sup, attr_name, optional, ty));
                        continue;
                    }
                    def.own_attrs.push(Attribute {
                        name: attr_name,
                        owner: 0,
                        optional,
                        ty,
                        derived: false,
                    });
                }
            }
        }
        Some(def)
    }
}

pub fn parse(src: &str) -> Schema {
    let mut schema = Parser::new(src).parse();
    schema.set_source(src);
    schema.resolve();
    schema
}
