//! Hover targets and markdown rendering, for IFC documents and schema documents.

use std::fmt::Write as _;

use crate::index::Document;
use crate::lexer::{Lexer, TokenKind};
use crate::parser::{ParsedPart, Value, ValueNode, body_start, parse_record};
use crate::schema::{EntityId, NamedRef, Schema, TypeExpr, TypeId, TypeKind};
use crate::text::Offset;

pub enum HoverTarget {
    /// `#id` reference or definition.
    Ref { id: u32, start: Offset, end: Offset },
    /// Constructor name of the record at `record`.
    Entity {
        record: usize,
        start: Offset,
        end: Offset,
    },
    /// Typed-value wrapper such as `IFCLABEL` inside an argument list.
    TypeName { start: Offset, end: Offset },
    /// `.VALUE.` inside the record at `record`.
    EnumValue {
        record: usize,
        start: Offset,
        end: Offset,
    },
}

pub fn target_at(doc: &Document, offset: Offset) -> Option<HoverTarget> {
    if let Some((id, start, end)) = doc.ref_at(offset) {
        return Some(HoverTarget::Ref { id, start, end });
    }
    let idx = doc.entity_at(offset)?;
    let rec = doc.entities[idx];
    if rec.is_header() {
        return None;
    }
    let buf = doc.text.bytes();
    let body = body_start(buf, rec.start, rec.end);
    let mut lx = Lexer::new(buf, body, rec.end);
    let mut first = true;
    loop {
        let t = lx.next_token();
        match t.kind {
            TokenKind::Eof => return None,
            TokenKind::Ident if t.start <= offset && offset <= t.end => {
                let after = lx.peek();
                if after.kind != TokenKind::LParen {
                    return None;
                }
                return Some(
                    if first || rec.is_complex() && is_part_name(buf, &rec, t.start) {
                        HoverTarget::Entity {
                            record: idx,
                            start: t.start,
                            end: t.end,
                        }
                    } else {
                        HoverTarget::TypeName {
                            start: t.start,
                            end: t.end,
                        }
                    },
                );
            }
            TokenKind::Enum if t.start <= offset && offset <= t.end => {
                return Some(HoverTarget::EnumValue {
                    record: idx,
                    start: t.start,
                    end: t.end,
                });
            }
            TokenKind::Ident if first => first = false,
            _ if t.start > offset => return None,
            _ => {}
        }
    }
}

fn is_part_name(buf: &[u8], rec: &crate::scanner::EntityRecord, name_start: Offset) -> bool {
    let parsed = parse_record(buf, body_start(buf, rec.start, rec.end), rec.end);
    parsed.parts.iter().any(|p| p.name_start == name_start)
}

/// Upper-cased value of the `.VALUE.` token at `[start, end)`.
pub fn enum_value_text(doc: &Document, start: Offset, end: Offset) -> String {
    let raw = doc.text.slice(start, end);
    let inner = raw
        .strip_prefix(b".")
        .and_then(|r| r.strip_suffix(b"."))
        .unwrap_or(raw);
    String::from_utf8_lossy(inner).to_ascii_uppercase()
}

/// The enumeration type governing the `.VALUE.` token at `offset` in record `idx`:
/// first through the attribute the value sits in, then by uniqueness of the value
/// across the schema's enumerations.
pub fn enum_type_at(doc: &Document, idx: usize, offset: Offset, schema: &Schema) -> Option<TypeId> {
    let rec = doc.entities[idx];
    let buf = doc.text.bytes();
    let parsed = parse_record(buf, body_start(buf, rec.start, rec.end), rec.end);
    let from_attr = parsed.parts.iter().find_map(|part| {
        let name = &buf[part.name_start as usize..part.name_end as usize];
        let e = schema.entity_bytes(name)?;
        let def = schema.entity_def(e);
        let attrs = if rec.is_complex() {
            &def.own_attrs
        } else {
            &def.flat
        };
        let (i, node) = part
            .args
            .iter()
            .enumerate()
            .find(|(_, n)| n.start <= offset && offset <= n.end)?;
        enum_type_in(schema, node, &attrs.get(i)?.ty, offset, buf)
    });
    if from_attr.is_some() {
        return from_attr;
    }
    let (start, end) = token_span_at(&parsed, offset)?;
    let value = enum_value_text(doc, start, end);
    let candidates = schema.enums_with_value(&value);
    (candidates.len() == 1).then(|| candidates[0])
}

fn token_span_at(parsed: &crate::parser::ParsedRecord, offset: Offset) -> Option<(Offset, Offset)> {
    fn walk(n: &ValueNode, offset: Offset) -> Option<(Offset, Offset)> {
        if !(n.start <= offset && offset <= n.end) {
            return None;
        }
        match &n.value {
            Value::Enum => Some((n.start, n.end)),
            Value::Typed { inner, .. } => walk(inner, offset),
            Value::List { items, .. } => items.iter().find_map(|i| walk(i, offset)),
            _ => None,
        }
    }
    parsed
        .parts
        .iter()
        .flat_map(|p| p.args.iter())
        .find_map(|n| walk(n, offset))
}

fn enum_type_in(
    schema: &Schema,
    node: &ValueNode,
    ty: &TypeExpr,
    offset: Offset,
    buf: &[u8],
) -> Option<TypeId> {
    match &node.value {
        Value::Enum => match ty {
            TypeExpr::Named(NamedRef::Type(t)) => match &schema.type_def(*t).kind {
                TypeKind::Enum { .. } => Some(*t),
                TypeKind::Defined(expr) => enum_type_in(schema, node, expr, offset, buf),
                TypeKind::Select { .. } => None,
            },
            _ => None,
        },
        Value::Typed {
            name_start,
            name_end,
            inner,
        } => {
            let wrapper =
                std::str::from_utf8(&buf[*name_start as usize..*name_end as usize]).ok()?;
            let w = schema.type_id(wrapper)?;
            enum_type_in(
                schema,
                inner,
                &TypeExpr::Named(NamedRef::Type(w)),
                offset,
                buf,
            )
        }
        Value::List { items, .. } => {
            let item = items
                .iter()
                .find(|i| i.start <= offset && offset <= i.end)?;
            let elem = match ty {
                TypeExpr::Aggregate { elem, .. } => &**elem,
                TypeExpr::Named(NamedRef::Type(t)) => match &schema.type_def(*t).kind {
                    TypeKind::Defined(TypeExpr::Aggregate { elem, .. }) => &**elem,
                    _ => return None,
                },
                _ => return None,
            };
            enum_type_in(schema, item, elem, offset, buf)
        }
        _ => None,
    }
}

const MAX_VALUE_CHARS: usize = 60;
const MAX_ENTITY_TEXT: usize = 600;

pub fn render_ref(doc: &Document, id: u32, schema: Option<&Schema>) -> String {
    let mut md = String::new();
    match doc.lookup(id) {
        None => {
            let _ = writeln!(md, "**#{id}** is not defined in this file");
        }
        Some(idx) => {
            let rec = doc.entities[idx];
            let line = doc.text.position(rec.start).line + 1;
            let name = if rec.is_complex() {
                "complex instance"
            } else {
                doc.names.get_str(rec.name)
            };
            let _ = writeln!(md, "**#{id}** = {name} · line {line}");
            if let Some(s) = schema
                && let Some(e) = s.entity_bytes(name.as_bytes())
            {
                let _ = writeln!(md, "\n{}", supertype_chain(s, e));
            }
            let text = String::from_utf8_lossy(doc.entity_text(idx));
            let shown: String = text.chars().take(MAX_ENTITY_TEXT).collect();
            let _ = write!(
                md,
                "\n```ifc\n{shown}{}\n```",
                if text.len() > shown.len() { "…" } else { "" }
            );
        }
    }
    md
}

fn supertype_chain(schema: &Schema, e: EntityId) -> String {
    let mut parts = vec![schema.entity_def(e).name.clone()];
    let mut cur = e;
    for _ in 0..32 {
        let def = schema.entity_def(cur);
        let Some(&p) = def.supertypes.first() else {
            break;
        };
        parts.push(schema.entity_def(p).name.clone());
        cur = p;
    }
    parts.join(" ← ")
}

fn attribute_type(s: &Schema, a: &crate::schema::Attribute) -> String {
    let mut t = s.type_expr_to_string(&a.ty);
    if a.optional {
        t.insert_str(0, "OPTIONAL ");
    }
    if a.derived {
        t.push_str(" (derived)");
    }
    t
}

fn write_entity_header(md: &mut String, s: &Schema, e: EntityId) {
    let def = s.entity_def(e);
    let _ = writeln!(
        md,
        "### {}{}",
        def.name,
        if def.is_abstract { " (abstract)" } else { "" }
    );
    let _ = writeln!(md, "{}\n", supertype_chain(s, e));
}

fn render_part(
    md: &mut String,
    doc: &Document,
    part: &ParsedPart,
    complex: bool,
    schema: Option<&Schema>,
) {
    let buf = doc.text.bytes();
    let name =
        String::from_utf8_lossy(&buf[part.name_start as usize..part.name_end as usize]).to_string();
    let entity = schema.and_then(|s| s.entity_bytes(name.as_bytes()).map(|e| (s, e)));
    match entity {
        Some((s, e)) => {
            write_entity_header(md, s, e);
            let def = s.entity_def(e);
            let attrs = if complex { &def.own_attrs } else { &def.flat };
            if attrs.len() != part.args.len() {
                let _ = writeln!(
                    md,
                    "⚠ {} attributes expected, {} arguments given\n",
                    attrs.len(),
                    part.args.len()
                );
            }
            let _ = writeln!(md, "| # | Attribute | Type | Value |");
            let _ = writeln!(md, "|--:|---|---|---|");
            let rows = attrs.len().max(part.args.len());
            for i in 0..rows {
                let attr = attrs.get(i);
                let arg = part.args.get(i);
                let attr_name = match attr {
                    Some(a) if a.owner != e => {
                        format!("{} _({})_", a.name, s.entity_def(a.owner).name)
                    }
                    Some(a) => a.name.clone(),
                    None => "_(extra)_".into(),
                };
                let ty = attr.map(|a| attribute_type(s, a)).unwrap_or_default();
                let value = arg
                    .map(|v| render_value(doc, v, schema))
                    .unwrap_or_else(|| "_(missing)_".into());
                let _ = writeln!(
                    md,
                    "| {} | {} | {} | {} |",
                    i + 1,
                    attr_name,
                    escape(&ty),
                    value
                );
            }
        }
        None => {
            let _ = writeln!(md, "### {name}");
            match schema {
                Some(s) => {
                    let _ = writeln!(md, "Not an entity of schema {}\n", s.name);
                }
                None => {
                    let _ = writeln!(md, "No schema loaded for this file\n");
                }
            }
            let _ = writeln!(md, "| # | Value |");
            let _ = writeln!(md, "|--:|---|");
            for (i, v) in part.args.iter().enumerate() {
                let _ = writeln!(md, "| {} | {} |", i + 1, render_value(doc, v, schema));
            }
        }
    }
}

pub fn render_entity(doc: &Document, idx: usize, schema: Option<&Schema>) -> String {
    let rec = doc.entities[idx];
    let buf = doc.text.bytes();
    let parsed = parse_record(buf, body_start(buf, rec.start, rec.end), rec.end);
    let mut md = String::new();
    for part in &parsed.parts {
        render_part(&mut md, doc, part, rec.is_complex(), schema);
        md.push('\n');
    }
    md
}

/// Entity declaration without instance values, for hovers inside a schema.
pub fn render_entity_decl(s: &Schema, e: EntityId) -> String {
    let mut md = String::new();
    write_entity_header(&mut md, s, e);
    let def = s.entity_def(e);
    let _ = writeln!(md, "| # | Attribute | Type |");
    let _ = writeln!(md, "|--:|---|---|");
    for (i, a) in def.flat.iter().enumerate() {
        let attr_name = if a.owner != e {
            format!("{} _({})_", a.name, s.entity_def(a.owner).name)
        } else {
            a.name.clone()
        };
        let _ = writeln!(
            md,
            "| {} | {} | {} |",
            i + 1,
            attr_name,
            escape(&attribute_type(s, a))
        );
    }
    let subtypes: Vec<&str> = s
        .entities
        .iter()
        .filter(|d| d.supertypes.contains(&e))
        .map(|d| d.name.as_str())
        .collect();
    if !subtypes.is_empty() {
        let _ = writeln!(md, "\nsubtypes: {}", subtypes.join(", "));
    }
    md
}

pub fn render_type_decl(s: &Schema, t: TypeId) -> String {
    let def = s.type_def(t);
    let mut md = format!("### {}\n", def.name);
    match &def.kind {
        TypeKind::Defined(expr) => {
            let _ = writeln!(md, "`TYPE {} = {}`", def.name, s.type_expr_to_string(expr));
            let (base, base_expr) = s.base_type(t);
            if base != t
                && let Some(be) = base_expr
            {
                let _ = writeln!(
                    md,
                    "\nunderlying: `{}` = `{}`",
                    s.type_def(base).name,
                    s.type_expr_to_string(be)
                );
            }
        }
        TypeKind::Enum { values, .. } => {
            let _ = writeln!(
                md,
                "enumeration: {}",
                values
                    .iter()
                    .map(|v| format!("`.{v}.`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        TypeKind::Select { members, .. } => {
            let names: Vec<String> = members
                .iter()
                .map(|m| match m {
                    NamedRef::Entity(e) => s.entity_def(*e).name.clone(),
                    NamedRef::Type(tt) => s.type_def(*tt).name.clone(),
                    NamedRef::Unresolved(n) => n.clone(),
                })
                .collect();
            let _ = writeln!(md, "select of: {}", names.join(", "));
        }
    }
    md
}

pub fn render_type(
    doc: &Document,
    start: Offset,
    end: Offset,
    schema: Option<&Schema>,
) -> Option<String> {
    let s = schema?;
    let name = std::str::from_utf8(doc.text.slice(start, end)).ok()?;
    Some(render_type_decl(s, s.type_id(name)?))
}

pub fn render_enum_value(
    doc: &Document,
    idx: usize,
    start: Offset,
    end: Offset,
    schema: Option<&Schema>,
) -> Option<String> {
    let s = schema?;
    let value = enum_value_text(doc, start, end);
    let mut md = String::new();
    match enum_type_at(doc, idx, start, s) {
        Some(t) => {
            let def = s.type_def(t);
            let known =
                matches!(&def.kind, TypeKind::Enum { values, .. } if values.contains(&value));
            let _ = writeln!(
                md,
                "`.{value}.` {} **{}**\n",
                if known {
                    "is a value of"
                } else {
                    "is not a value of"
                },
                def.name
            );
            md.push_str(&render_type_decl(s, t));
        }
        None => {
            let candidates = s.enums_with_value(&value);
            if candidates.is_empty() {
                let _ = writeln!(
                    md,
                    "`.{value}.` is not a value of any enumeration in {}",
                    s.name
                );
            } else {
                let names: Vec<&str> = candidates
                    .iter()
                    .map(|t| s.type_def(*t).name.as_str())
                    .collect();
                let _ = writeln!(md, "`.{value}.` is a value of: {}", names.join(", "));
            }
        }
    }
    Some(md)
}

/// Hover for an identifier inside a schema document.
pub fn render_schema_word(s: &Schema, word: &str) -> Option<String> {
    if let Some(e) = s.entity(word) {
        return Some(render_entity_decl(s, e));
    }
    if let Some(t) = s.type_id(word) {
        return Some(render_type_decl(s, t));
    }
    None
}

fn render_value(doc: &Document, v: &ValueNode, schema: Option<&Schema>) -> String {
    let buf = doc.text.bytes();
    let raw = || String::from_utf8_lossy(&buf[v.start as usize..v.end as usize]).to_string();
    match &v.value {
        Value::Null => "`$`".into(),
        Value::Derived => "`*`".into(),
        Value::Ref(id) => match doc.record(*id) {
            Some(r) if r.is_complex() => format!("`#{id}` (complex)"),
            Some(r) => {
                let name = doc.names.get_str(r.name);
                let display = schema.and_then(|s| {
                    s.entity_bytes(name.as_bytes())
                        .map(|e| s.entity_def(e).name.clone())
                });
                format!("`#{id}` {}", display.unwrap_or_else(|| name.to_string()))
            }
            None => format!("`#{id}` ⚠ undefined"),
        },
        Value::List { items, total } => {
            let text = raw();
            if text.chars().count() <= MAX_VALUE_CHARS {
                format!("`{}`", escape(&text))
            } else {
                let elem = items.first().map(describe_shape).unwrap_or_default();
                format!("list of {total}{elem}")
            }
        }
        _ => {
            let text = raw();
            if text.chars().count() <= MAX_VALUE_CHARS {
                format!("`{}`", escape(&text))
            } else {
                let shown: String = text.chars().take(MAX_VALUE_CHARS).collect();
                format!("`{}…`", escape(&shown))
            }
        }
    }
}

fn describe_shape(v: &ValueNode) -> String {
    match &v.value {
        Value::List { items, total } => {
            let inner = items.first().map(describe_shape).unwrap_or_default();
            format!(" × list of {total}{inner}")
        }
        _ => format!(" {}", v.kind_name()),
    }
}

fn escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}
