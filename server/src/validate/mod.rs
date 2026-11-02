//! Cheap per-entity checks against the schema. Not a conformance checker: no
//! WHERE rules, no inverse cardinalities, no uniqueness beyond `#id`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::index::Document;
use crate::parser::{ParsedPart, Value, ValueNode, body_start, parse_record};
use crate::scanner::{EntityRecord, flags};
use crate::schema::{
    Attribute, EntityId, NamedRef, Schema, SchemaSet, Simple, TypeExpr, TypeId, TypeKind,
};
use crate::text::Offset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Information,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Information => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub start: Offset,
    pub end: Offset,
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
}

const UNRESOLVED: u32 = u32::MAX - 1;
const UNKNOWN: u32 = u32::MAX;

/// Validation state for one document snapshot.
pub struct Validator<'a> {
    pub doc: &'a Document,
    pub schema: Option<Arc<Schema>>,
    /// Interned name -> schema entity id, filled lazily.
    name_cache: Vec<u32>,
    /// Maximum list elements type-checked per aggregate.
    pub list_check_limit: usize,
}

impl<'a> Validator<'a> {
    pub fn new(doc: &'a Document, schemas: &SchemaSet) -> Self {
        let schema = doc
            .schema_name
            .as_deref()
            .and_then(|n| schemas.for_file_schema(n));
        Self {
            doc,
            schema,
            name_cache: vec![UNRESOLVED; doc.names.len()],
            list_check_limit: 64,
        }
    }

    pub fn with_schema(doc: &'a Document, schema: Option<Arc<Schema>>) -> Self {
        Self {
            doc,
            schema,
            name_cache: vec![UNRESOLVED; doc.names.len()],
            list_check_limit: 64,
        }
    }

    pub fn resolve_name(&mut self, name: u32) -> Option<EntityId> {
        let schema = self.schema.as_ref()?;
        if name as usize >= self.name_cache.len() {
            self.name_cache.resize(name as usize + 1, UNRESOLVED);
        }
        let cached = self.name_cache[name as usize];
        if cached == UNRESOLVED {
            let resolved = schema.entity_bytes(self.doc.names.get(name));
            self.name_cache[name as usize] = resolved.unwrap_or(UNKNOWN);
            return resolved;
        }
        (cached != UNKNOWN).then_some(cached)
    }

    /// Document-level checks: schema header and duplicate ids.
    pub fn document_checks(&mut self, out: &mut Vec<Diagnostic>) {
        let doc = self.doc;
        let file_schema = doc.names.lookup(b"FILE_SCHEMA");
        let header = file_schema.and_then(|n| {
            doc.entities
                .iter()
                .take(64)
                .find(|r| r.flags & flags::HEADER != 0 && r.name == n)
        });
        match (&doc.schema_name, &self.schema, header) {
            (None, _, _) => {
                let (s, e) = header.map(|r| (r.start, r.end)).unwrap_or((0, 0));
                out.push(Diagnostic {
                    start: s,
                    end: e,
                    severity: Severity::Warning,
                    code: "W001",
                    message: "no FILE_SCHEMA found; only syntax is checked".into(),
                });
            }
            (Some(name), None, h) => {
                let (s, e) = h.map(|r| (r.start, r.end)).unwrap_or((0, 0));
                out.push(Diagnostic {
                    start: s,
                    end: e,
                    severity: Severity::Warning,
                    code: "W001",
                    message: format!("unknown schema '{name}'; only syntax is checked"),
                });
            }
            _ => {}
        }
        for &(id, idx) in &doc.duplicates {
            let r = &doc.entities[idx as usize];
            let first = doc
                .lookup(id)
                .map(|i| doc.text.position(doc.entities[i].start).line + 1);
            let id_end = r.start + 1 + (id.checked_ilog10().unwrap_or(0) + 1);
            out.push(Diagnostic {
                start: r.start,
                end: id_end.min(r.end),
                severity: Severity::Error,
                code: "E005",
                message: match first {
                    Some(line) => format!("#{id} is already defined on line {line}"),
                    None => format!("#{id} is defined more than once"),
                },
            });
        }
    }

    pub fn entity(&mut self, idx: usize, out: &mut Vec<Diagnostic>) {
        let doc = self.doc;
        let rec = doc.entities[idx];
        let buf = doc.text.bytes();

        // An unterminated string is reported by the argument parser with its exact span.
        if rec.is_malformed() && rec.flags & flags::UNTERMINATED_STRING == 0 {
            self.malformed(&rec, out);
        }
        if rec.flags & (flags::GARBAGE | flags::NO_EQUALS | flags::NO_NAME | flags::BAD_ID) != 0 {
            return;
        }
        if rec.flags & flags::KEYWORD != 0 {
            return;
        }

        let body = if rec.flags & flags::HEADER != 0 {
            rec.start
        } else {
            body_start(buf, rec.start, rec.end)
        };
        let parsed = parse_record(buf, body, rec.end);
        for e in &parsed.errors {
            out.push(Diagnostic {
                start: e.start,
                end: e.end,
                severity: Severity::Error,
                code: "E006",
                message: e.message.clone(),
            });
        }

        if rec.flags & flags::HEADER != 0 {
            self.header_record(&rec, &parsed.parts, out);
            return;
        }
        let Some(schema) = self.schema.clone() else {
            return;
        };

        if rec.is_complex() {
            let part_ids: Vec<Option<EntityId>> = parsed
                .parts
                .iter()
                .map(|p| schema.entity_bytes(&buf[p.name_start as usize..p.name_end as usize]))
                .collect();
            for (part, eid) in parsed.parts.iter().zip(&part_ids) {
                let Some(eid) = *eid else {
                    let name = &buf[part.name_start as usize..part.name_end as usize];
                    out.push(Diagnostic {
                        start: part.name_start,
                        end: part.name_end,
                        severity: Severity::Error,
                        code: "E001",
                        message: format!(
                            "unknown entity {} in schema {}",
                            String::from_utf8_lossy(name),
                            schema.name
                        ),
                    });
                    continue;
                };
                // A derivation declared by another part of the complex instance (e.g.
                // IfcSIUnit deriving IfcNamedUnit.Dimensions) applies to this part.
                let mut own = schema.entity_def(eid).own_attrs.clone();
                for a in &mut own {
                    a.derived |= part_ids.iter().flatten().any(|p| {
                        schema
                            .attributes(*p)
                            .iter()
                            .any(|fa| fa.owner == eid && fa.derived && fa.name == a.name)
                    });
                }
                self.check_args(&schema, part, &own, &schema.entity_def(eid).name, out);
            }
            return;
        }

        let Some(part) = parsed.parts.first() else {
            return;
        };
        let Some(eid) = self.resolve_name(rec.name) else {
            out.push(Diagnostic {
                start: part.name_start,
                end: part.name_end,
                severity: Severity::Error,
                code: "E001",
                message: format!(
                    "unknown entity {} in schema {}",
                    doc.names.get_str(rec.name),
                    schema.name
                ),
            });
            return;
        };
        let def = schema.entity_def(eid);
        if def.is_abstract {
            out.push(Diagnostic {
                start: part.name_start,
                end: part.name_end,
                severity: Severity::Error,
                code: "E002",
                message: format!("{} is abstract and cannot be instantiated", def.name),
            });
        }
        self.check_args(&schema, part, &def.flat, &def.name, out);
    }

    fn malformed(&self, rec: &EntityRecord, out: &mut Vec<Diagnostic>) {
        let f = rec.flags;
        let (code, msg): (&str, &str) = if f & flags::GARBAGE != 0 {
            ("E006", "unexpected text; expected `#id = ENTITY(...);`")
        } else if f & flags::BAD_ID != 0 {
            ("E006", "invalid instance id")
        } else if f & flags::NO_EQUALS != 0 {
            ("E006", "expected '=' after the instance id")
        } else if f & flags::NO_NAME != 0 {
            ("E006", "expected an entity name after '='")
        } else if f & flags::UNTERMINATED_STRING != 0 {
            ("E006", "unterminated string")
        } else if f & flags::UNTERMINATED_COMMENT != 0 {
            ("E006", "unterminated comment")
        } else if f & flags::MISSING_SEMICOLON != 0 {
            ("E006", "missing ';' at the end of the record")
        } else {
            ("E006", "malformed record")
        };
        let (start, end) =
            if f & flags::MISSING_SEMICOLON != 0 && f & flags::UNTERMINATED_STRING == 0 {
                (rec.end.saturating_sub(1).max(rec.start), rec.end)
            } else {
                (rec.start, rec.end.min(rec.start + 200))
            };
        out.push(Diagnostic {
            start,
            end,
            severity: Severity::Error,
            code,
            message: msg.into(),
        });
    }

    fn header_record(&self, rec: &EntityRecord, parts: &[ParsedPart], out: &mut Vec<Diagnostic>) {
        let name = self.doc.names.get_str(rec.name);
        let expected = match name {
            "FILE_DESCRIPTION" => Some(2),
            "FILE_NAME" => Some(7),
            "FILE_SCHEMA" => Some(1),
            "FILE_POPULATION" | "SECTION_LANGUAGE" | "SECTION_CONTEXT" => None,
            _ => {
                out.push(Diagnostic {
                    start: rec.start,
                    end: rec.start + name.len() as Offset,
                    severity: Severity::Warning,
                    code: "W002",
                    message: format!("unexpected record {name} without an instance id"),
                });
                return;
            }
        };
        if let (Some(n), Some(part)) = (expected, parts.first())
            && part.args.len() != n
        {
            out.push(Diagnostic {
                start: part.args_start,
                end: part.args_end,
                severity: Severity::Warning,
                code: "W002",
                message: format!("{name} takes {n} arguments, found {}", part.args.len()),
            });
        }
    }

    fn check_args(
        &mut self,
        schema: &Arc<Schema>,
        part: &ParsedPart,
        attrs: &[Attribute],
        entity_name: &str,
        out: &mut Vec<Diagnostic>,
    ) {
        if part.args.len() != attrs.len() {
            out.push(Diagnostic {
                start: part.args_start,
                end: part.args_end,
                severity: Severity::Error,
                code: "E003",
                message: format!(
                    "{entity_name} has {} attributes, found {} arguments",
                    attrs.len(),
                    part.args.len()
                ),
            });
        }
        for (attr, node) in attrs.iter().zip(&part.args) {
            self.check_attribute(schema, attr, node, out);
        }
    }

    fn check_attribute(
        &mut self,
        schema: &Arc<Schema>,
        attr: &Attribute,
        node: &ValueNode,
        out: &mut Vec<Diagnostic>,
    ) {
        match &node.value {
            Value::Error => return,
            Value::Derived => {
                if !attr.derived {
                    out.push(Diagnostic {
                        start: node.start,
                        end: node.end,
                        severity: Severity::Error,
                        code: "E008",
                        message: format!(
                            "{} is not a derived attribute; '*' is not allowed here",
                            attr.name
                        ),
                    });
                }
                return;
            }
            Value::Null => {
                if attr.derived {
                    out.push(Diagnostic {
                        start: node.start,
                        end: node.end,
                        severity: Severity::Warning,
                        code: "E008",
                        message: format!("{} is derived in this entity; expected '*'", attr.name),
                    });
                } else if !attr.optional {
                    out.push(Diagnostic {
                        start: node.start,
                        end: node.end,
                        severity: Severity::Error,
                        code: "E008",
                        message: format!("{} is not optional", attr.name),
                    });
                }
                return;
            }
            _ => {}
        }
        if attr.derived {
            out.push(Diagnostic {
                start: node.start,
                end: node.end,
                severity: Severity::Warning,
                code: "E008",
                message: format!("{} is derived in this entity; expected '*'", attr.name),
            });
            return;
        }
        if let Some(msg) = self.check_value(schema, node, &attr.ty, out) {
            out.push(Diagnostic {
                start: node.start,
                end: node.end,
                severity: Severity::Error,
                code: "E008",
                message: format!("{}: {msg}", attr.name),
            });
        }
    }

    /// Returns a mismatch description, or `None` when the value fits. Reference
    /// and select errors are pushed directly with their own codes.
    fn check_value(
        &mut self,
        schema: &Arc<Schema>,
        node: &ValueNode,
        ty: &TypeExpr,
        out: &mut Vec<Diagnostic>,
    ) -> Option<String> {
        if matches!(node.value, Value::Error) {
            return None;
        }
        match ty {
            TypeExpr::Aggregate { kind, lo, hi, elem } => {
                let Value::List { items, total } = &node.value else {
                    if let Value::Typed { inner, .. } = &node.value
                        && matches!(inner.value, Value::List { .. })
                    {
                        return self.check_value(schema, inner, ty, out);
                    }
                    return Some(format!(
                        "expected {} but found {}",
                        schema.type_expr_to_string(ty),
                        node.kind_name()
                    ));
                };
                if let Some(lo) = lo
                    && *total < *lo as usize
                {
                    return Some(format!(
                        "{} needs at least {lo} elements, found {total}",
                        kind.as_str()
                    ));
                }
                if let Some(hi) = hi
                    && *total > *hi as usize
                {
                    return Some(format!(
                        "{} allows at most {hi} elements, found {total}",
                        kind.as_str()
                    ));
                }
                let limit = self.list_check_limit;
                for item in items.iter().take(limit) {
                    if let Some(msg) = self.check_value(schema, item, elem, out) {
                        return Some(format!("element: {msg}"));
                    }
                }
                None
            }
            TypeExpr::Simple(s) => self.check_simple(node, *s),
            TypeExpr::Named(NamedRef::Entity(e)) => {
                self.check_entity_ref(schema, node, *e, out);
                None
            }
            TypeExpr::Named(NamedRef::Type(t)) => self.check_defined(schema, node, *t, out),
            TypeExpr::Named(NamedRef::Unresolved(_)) => None,
        }
    }

    fn check_simple(&self, node: &ValueNode, s: Simple) -> Option<String> {
        let buf = self.doc.text.bytes();
        let ok = match (&node.value, s) {
            (Value::Real | Value::Int, Simple::Real | Simple::Number) => true,
            (Value::Int, Simple::Integer) => true,
            (Value::Str, Simple::String) => true,
            (Value::Binary, Simple::Binary) => true,
            (Value::Enum, Simple::Boolean) => matches!(
                &buf[node.start as usize..node.end as usize],
                b".T." | b".F."
            ),
            (Value::Enum, Simple::Logical) => matches!(
                &buf[node.start as usize..node.end as usize],
                b".T." | b".F." | b".U."
            ),
            (Value::Typed { inner, .. }, _) => return self.check_simple(inner, s),
            _ => false,
        };
        if ok {
            None
        } else {
            Some(format!(
                "expected {} but found {}",
                s.as_str(),
                node.kind_name()
            ))
        }
    }

    fn target_entity(&mut self, id: u32) -> Result<Option<Vec<EntityId>>, ()> {
        let doc = self.doc;
        let Some(idx) = doc.lookup(id) else {
            return Err(());
        };
        let rec = doc.entities[idx];
        if rec.is_complex() {
            let buf = doc.text.bytes();
            let parsed = parse_record(buf, body_start(buf, rec.start, rec.end), rec.end);
            let schema = self.schema.clone();
            let ids = parsed
                .parts
                .iter()
                .filter_map(|p| {
                    schema
                        .as_ref()?
                        .entity_bytes(&buf[p.name_start as usize..p.name_end as usize])
                })
                .collect();
            return Ok(Some(ids));
        }
        Ok(self.resolve_name(rec.name).map(|e| vec![e]))
    }

    fn check_entity_ref(
        &mut self,
        schema: &Arc<Schema>,
        node: &ValueNode,
        expected: EntityId,
        out: &mut Vec<Diagnostic>,
    ) {
        let Value::Ref(id) = node.value else {
            out.push(Diagnostic {
                start: node.start,
                end: node.end,
                severity: Severity::Error,
                code: "E008",
                message: format!(
                    "expected a reference to {} but found {}",
                    schema.entity_def(expected).name,
                    node.kind_name()
                ),
            });
            return;
        };
        match self.target_entity(id) {
            Err(()) => out.push(Diagnostic {
                start: node.start,
                end: node.end,
                severity: Severity::Error,
                code: "E004",
                message: format!("#{id} is not defined"),
            }),
            Ok(None) => {}
            Ok(Some(actual)) => {
                if !actual.iter().any(|a| schema.is_subtype_of(*a, expected)) {
                    let names: Vec<&str> = actual
                        .iter()
                        .map(|a| schema.entity_def(*a).name.as_str())
                        .collect();
                    out.push(Diagnostic {
                        start: node.start,
                        end: node.end,
                        severity: Severity::Error,
                        code: "E009",
                        message: format!(
                            "expected {} but #{id} is {}",
                            schema.entity_def(expected).name,
                            names.join("+")
                        ),
                    });
                }
            }
        }
    }

    fn check_defined(
        &mut self,
        schema: &Arc<Schema>,
        node: &ValueNode,
        t: TypeId,
        out: &mut Vec<Diagnostic>,
    ) -> Option<String> {
        let buf = self.doc.text.bytes();
        let def = schema.type_def(t);
        match &def.kind {
            TypeKind::Enum { values, .. } => {
                let inner = match &node.value {
                    Value::Typed {
                        name_start,
                        name_end,
                        inner,
                    } => {
                        let wrapper = &buf[*name_start as usize..*name_end as usize];
                        if !wrapper.eq_ignore_ascii_case(def.name.as_bytes()) {
                            return Some(format!(
                                "expected {} but found {}",
                                def.name,
                                String::from_utf8_lossy(wrapper)
                            ));
                        }
                        &**inner
                    }
                    _ => node,
                };
                if inner.value != Value::Enum {
                    return Some(format!(
                        "expected {} enumeration but found {}",
                        def.name,
                        inner.kind_name()
                    ));
                }
                let text = &buf[inner.start as usize + 1..inner.end as usize - 1];
                let upper = String::from_utf8_lossy(text).to_ascii_uppercase();
                if values.contains(&upper) {
                    None
                } else {
                    out.push(Diagnostic {
                        start: inner.start,
                        end: inner.end,
                        severity: Severity::Error,
                        code: "E007",
                        message: format!(".{upper}. is not a value of {}", def.name),
                    });
                    None
                }
            }
            TypeKind::Select { .. } => {
                let closure = schema.select_closure(t)?;
                match &node.value {
                    Value::Ref(id) => {
                        match self.target_entity(*id) {
                            Err(()) => out.push(Diagnostic {
                                start: node.start,
                                end: node.end,
                                severity: Severity::Error,
                                code: "E004",
                                message: format!("#{id} is not defined"),
                            }),
                            Ok(None) => {}
                            Ok(Some(actual)) => {
                                let fits = actual.iter().any(|a| {
                                    closure
                                        .entities
                                        .iter()
                                        .any(|m| schema.is_subtype_of(*a, *m))
                                });
                                if !fits {
                                    let names: Vec<&str> = actual
                                        .iter()
                                        .map(|a| schema.entity_def(*a).name.as_str())
                                        .collect();
                                    out.push(Diagnostic {
                                        start: node.start,
                                        end: node.end,
                                        severity: Severity::Error,
                                        code: "E009",
                                        message: format!(
                                            "#{id} is {} which is not a valid {}",
                                            names.join("+"),
                                            def.name
                                        ),
                                    });
                                }
                            }
                        }
                        None
                    }
                    Value::Typed {
                        name_start,
                        name_end,
                        inner,
                    } => {
                        let wrapper = &buf[*name_start as usize..*name_end as usize];
                        let Some(wt) = schema.type_id(std::str::from_utf8(wrapper).unwrap_or(""))
                        else {
                            let message = match schema.entity_bytes(wrapper) {
                                Some(e) => format!(
                                    "{} is an entity; use a reference such as #12 instead",
                                    schema.entity_def(e).name
                                ),
                                None => {
                                    format!("unknown type {}", String::from_utf8_lossy(wrapper))
                                }
                            };
                            out.push(Diagnostic {
                                start: *name_start,
                                end: *name_end,
                                severity: Severity::Error,
                                code: "E010",
                                message,
                            });
                            return None;
                        };
                        if !closure.types.contains(&wt) {
                            out.push(Diagnostic {
                                start: *name_start,
                                end: *name_end,
                                severity: Severity::Error,
                                code: "E010",
                                message: format!(
                                    "{} is not a member of {}",
                                    schema.type_def(wt).name,
                                    def.name
                                ),
                            });
                            return None;
                        }
                        self.check_defined(schema, inner, wt, out)
                    }
                    _ => Some(format!(
                        "{} needs a typed value such as IFCLABEL('..') or a reference, found {}",
                        def.name,
                        node.kind_name()
                    )),
                }
            }
            TypeKind::Defined(expr) => {
                if let Value::Typed {
                    name_start,
                    name_end,
                    inner,
                } = &node.value
                {
                    let wrapper = &buf[*name_start as usize..*name_end as usize];
                    let wt = schema.type_id(std::str::from_utf8(wrapper).unwrap_or(""));
                    let same_family = wt
                        .is_some_and(|w| w == t || schema.base_type(w).0 == schema.base_type(t).0);
                    if !same_family {
                        return Some(format!(
                            "expected {} but found {}",
                            def.name,
                            String::from_utf8_lossy(wrapper)
                        ));
                    }
                    return self.check_value(schema, inner, expr, out);
                }
                self.check_value(schema, node, expr, out)
                    .map(|m| format!("{} {m}", def.name))
            }
        }
    }
}

/// Validate a whole document. `cancel` is polled between entities.
pub fn validate_all(
    doc: &Document,
    schemas: &SchemaSet,
    cancel: Option<&AtomicBool>,
) -> Vec<Diagnostic> {
    let mut v = Validator::new(doc, schemas);
    let mut out = Vec::new();
    v.document_checks(&mut out);
    for idx in 0..doc.entities.len() {
        if idx % 256 == 0 && cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            break;
        }
        v.entity(idx, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [ReferenceView]'),'2;1');\nFILE_NAME('a.ifc','2024-01-01T00:00:00',(''),(''),'x','y','z');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n";
    const FOOTER: &str = "ENDSEC;\nEND-ISO-10303-21;\n";

    fn diags(data: &str) -> Vec<(String, String)> {
        let src = format!("{HEADER}{data}{FOOTER}");
        let doc = Document::new(src.as_bytes().to_vec(), 0);
        let out = validate_all(&doc, &SchemaSet::embedded(), None);
        out.iter()
            .map(|d| (d.code.to_string(), d.message.clone()))
            .collect()
    }

    const VALID: &str = "#1=IFCOWNERHISTORY(#2,#5,$,.ADDED.,1731578952,#2,#5,1731578952);\n#2=IFCPERSONANDORGANIZATION(#3,#4,$);\n#3=IFCPERSON('id','Jan',$,$,$,$,$,$);\n#4=IFCORGANIZATION($,'bSI','desc',$,$);\n#5=IFCAPPLICATION(#6,'5.3.3','app','su');\n#6=IFCORGANIZATION($,'BIM-Tools',$,$,$);\n#7=IFCAXIS2PLACEMENT3D(#8,#9,#10);\n#8=IFCCARTESIANPOINT((0.,0.,0.));\n#9=IFCDIRECTION((0.,0.,1.));\n#10=IFCDIRECTION((1.,0.,0.));\n#11=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,$,#7,$);\n#12=IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#11,$,.MODEL_VIEW.,$);\n#13=IFCPROJECT('2Ndyd$OSX7s9A04nc4lyye',#1,'p','d',$,$,$,(#11),#14);\n#14=IFCUNITASSIGNMENT((#15,#16));\n#15=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n#16=IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.);\n#31=IFCPROPERTYSINGLEVALUE('ConstructionMethod',$,IFCLABEL('new construction'),$);\n#32=IFCPROPERTYSET('3wBmY8vZz83vO$cDNkL7kY',#1,'Pset_BuildingCommon',$,(#31));\n#33=IFCRELDEFINESBYPROPERTIES('2olvRyf$j6TfWHgcbyQ6NC',#1,$,$,(#13),#32);\n#34=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,1.,1.)));\n#35=IFCPROPERTYSINGLEVALUE('B',$,IFCBOOLEAN(.T.),$);\n";

    #[test]
    fn valid_file_has_no_diagnostics() {
        let d = diags(VALID);
        assert!(d.is_empty(), "{d:#?}");
    }

    #[test]
    fn detects_each_rule() {
        let cases: Vec<(&str, &str, &str)> = vec![
            (
                "#99=IFCNOTATHING('x');",
                "E001",
                "unknown entity IFCNOTATHING",
            ),
            ("#99=IFCPRODUCT('x',$,$,$,$,$,$);", "E002", "abstract"),
            (
                "#99=IFCDIRECTION((1.,0.,0.),'extra');",
                "E003",
                "IfcDirection has 1 attributes, found 2 arguments",
            ),
            (
                "#99=IFCAXIS2PLACEMENT3D(#4242,$,$);",
                "E004",
                "#4242 is not defined",
            ),
            (
                "#8=IFCCARTESIANPOINT((1.,1.,1.));",
                "E005",
                "#8 is already defined",
            ),
            ("#99=IFCDIRECTION((1.,0.,0.)", "E006", "missing ';'"),
            ("#99=IFCDIRECTION((1.,0.,0.);", "E006", "expected ')'"),
            (
                "#99=IFCSIUNIT(*,.NOTAUNIT.,$,.METRE.);",
                "E007",
                ".NOTAUNIT. is not a value of IfcUnitEnum",
            ),
            (
                "#99=IFCDIRECTION('nope');",
                "E008",
                "DirectionRatios: expected LIST [2:3] OF IfcReal but found string",
            ),
            (
                "#99=IFCDIRECTION((1.));",
                "E008",
                "LIST needs at least 2 elements, found 1",
            ),
            (
                "#99=IFCDIRECTION($);",
                "E008",
                "DirectionRatios is not optional",
            ),
            (
                "#99=IFCPERSON('id','Jan',$,$,$,$,$,$);\n#98=IFCPERSON(*,$,$,$,$,$,$,$);",
                "E008",
                "not a derived attribute",
            ),
            (
                "#99=IFCSIUNIT($,.LENGTHUNIT.,$,.METRE.);",
                "E008",
                "Dimensions is derived in this entity; expected '*'",
            ),
            (
                "#99=IFCAXIS2PLACEMENT3D(#9,$,$);",
                "E009",
                "expected IfcCartesianPoint but #9 is IfcDirection",
            ),
            (
                "#99=IFCPROPERTYSINGLEVALUE('x',$,#8,$);",
                "E009",
                "#8 is IfcCartesianPoint which is not a valid IfcValue",
            ),
            (
                "#99=IFCPROPERTYSINGLEVALUE('x',$,IFCNOPE('v'),$);",
                "E010",
                "unknown type IFCNOPE",
            ),
            (
                "#99=IFCPROPERTYSINGLEVALUE('x',$,IFCDIRECTION('v'),$);",
                "E010",
                "IfcDirection is an entity",
            ),
            (
                "#99=IFCPROPERTYSINGLEVALUE('x',$,'plain',$);",
                "E008",
                "needs a typed value",
            ),
            (
                "#99=IFCPROPERTYSINGLEVALUE('x',$,IFCLABEL(1.),$);",
                "E008",
                "IfcLabel expected STRING but found real",
            ),
        ];
        for (snippet, code, needle) in cases {
            let d = diags(&format!("{VALID}{snippet}\n"));
            assert!(
                d.iter().any(|(c, m)| c == code && m.contains(needle)),
                "snippet {snippet:?}: expected {code} containing {needle:?}, got {d:#?}"
            );
        }
    }

    #[test]
    fn complex_instances_check_each_part() {
        let d = diags(&format!(
            "{VALID}#99=(IFCNAMEDUNIT(*,.LENGTHUNIT.)IFCSIUNIT(.MILLI.,.METRE.));\n"
        ));
        assert!(d.is_empty(), "{d:#?}");
        let d = diags(&format!(
            "{VALID}#99=(IFCNAMEDUNIT(*,.LENGTHUNIT.)IFCSIUNIT(.MILLI.));\n"
        ));
        assert!(
            d.iter()
                .any(|(c, m)| c == "E003" && m.contains("IfcSIUnit has 2 attributes, found 1")),
            "{d:#?}"
        );
    }

    #[test]
    fn unknown_schema_is_a_warning_only() {
        let src = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC9'));\nENDSEC;\nDATA;\n#1=IFCNOPE(1,2,3);\nENDSEC;\nEND-ISO-10303-21;\n";
        let doc = Document::new(src.as_bytes().to_vec(), 0);
        let out = validate_all(&doc, &SchemaSet::embedded(), None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, "W001");
    }

    #[test]
    fn header_arity() {
        let src = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'),'x');\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";
        let doc = Document::new(src.as_bytes().to_vec(), 0);
        let out = validate_all(&doc, &SchemaSet::embedded(), None);
        assert!(
            out.iter()
                .any(|d| d.code == "W002" && d.message.contains("FILE_SCHEMA takes 1 arguments")),
            "{out:#?}"
        );
    }
}
