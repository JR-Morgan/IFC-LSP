//! Schema model built from EXPRESS declarations, plus the embedded IFC releases.

pub mod express;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use lsp_types::{Position, Range};

use rustc_hash::FxHashMap;

pub type EntityId = u32;
pub type TypeId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Simple {
    Real,
    Integer,
    Number,
    Boolean,
    Logical,
    String,
    Binary,
}

impl Simple {
    pub fn as_str(self) -> &'static str {
        match self {
            Simple::Real => "REAL",
            Simple::Integer => "INTEGER",
            Simple::Number => "NUMBER",
            Simple::Boolean => "BOOLEAN",
            Simple::Logical => "LOGICAL",
            Simple::String => "STRING",
            Simple::Binary => "BINARY",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggKind {
    List,
    Set,
    Array,
    Bag,
}

impl AggKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AggKind::List => "LIST",
            AggKind::Set => "SET",
            AggKind::Array => "ARRAY",
            AggKind::Bag => "BAG",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum NamedRef {
    Entity(EntityId),
    Type(TypeId),
    Unresolved(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Simple(Simple),
    Named(NamedRef),
    Aggregate {
        kind: AggKind,
        lo: Option<u32>,
        hi: Option<u32>,
        elem: Box<TypeExpr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    Defined(TypeExpr),
    Enum {
        /// Upper-cased values.
        values: Vec<String>,
        /// Source offset of each value's declaration, parallel to `values`.
        value_offsets: Vec<usize>,
        based_on: Option<String>,
    },
    Select {
        members: Vec<NamedRef>,
        based_on: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: String,
    pub kind: TypeKind,
    /// Source offset of the name in the declaration.
    pub decl: usize,
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: String,
    /// Entity that declares the attribute.
    pub owner: EntityId,
    pub optional: bool,
    pub ty: TypeExpr,
    /// Redeclared as DERIVE somewhere in the chain, so instances carry `*`.
    pub derived: bool,
}

#[derive(Debug, Clone, Default)]
pub struct EntityDef {
    pub name: String,
    /// Source offset of the name in the declaration.
    pub decl: usize,
    pub is_abstract: bool,
    pub supertype_names: Vec<String>,
    pub supertypes: Vec<EntityId>,
    pub own_attrs: Vec<Attribute>,
    pub derived_redeclarations: Vec<(String, String)>,
    pub explicit_redeclarations: Vec<(String, String, bool, TypeExpr)>,
    /// All explicit attributes in Part 21 order (inherited first).
    pub flat: Vec<Attribute>,
}

/// Transitive members of a SELECT type.
#[derive(Debug, Clone, Default)]
pub struct SelectClosure {
    pub entities: Vec<EntityId>,
    pub types: Vec<TypeId>,
}

#[derive(Debug, Clone, Default)]
pub struct Schema {
    pub name: String,
    /// Name the schema is registered under in the [`SchemaSet`] (embedded release
    /// key or upper-cased declared name).
    pub key: String,
    /// Where the source came from, when it is a file on disk.
    pub source_path: Option<PathBuf>,
    pub entities: Vec<EntityDef>,
    pub types: Vec<TypeDef>,
    pub errors: Vec<String>,
    source: String,
    line_starts: Vec<u32>,
    entity_by_name: FxHashMap<String, EntityId>,
    type_by_name: FxHashMap<String, TypeId>,
    select_closures: Vec<Option<SelectClosure>>,
}

impl Schema {
    pub fn parse(src: &str) -> Schema {
        express::parse(src)
    }

    pub(crate) fn set_source(&mut self, src: &str) {
        self.source = src.to_string();
        self.line_starts = std::iter::once(0)
            .chain(memchr::memchr_iter(b'\n', src.as_bytes()).map(|i| i as u32 + 1))
            .collect();
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// LSP position of a byte offset in the schema source (ASCII, so columns are bytes).
    pub fn position(&self, offset: usize) -> Position {
        let line = match self.line_starts.binary_search(&(offset as u32)) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        Position::new(line as u32, offset as u32 - self.line_starts[line])
    }

    pub fn span_range(&self, offset: usize, len: usize) -> Range {
        Range::new(self.position(offset), self.position(offset + len))
    }

    pub fn entity_decl_range(&self, e: EntityId) -> Range {
        let d = &self.entities[e as usize];
        self.span_range(d.decl, d.name.len())
    }

    pub fn type_decl_range(&self, t: TypeId) -> Range {
        let d = &self.types[t as usize];
        self.span_range(d.decl, d.name.len())
    }

    /// Range of an enumeration value's declaration, or the type's own declaration
    /// when the value is unknown.
    pub fn enum_value_range(&self, t: TypeId, value_upper: &str) -> Range {
        if let TypeKind::Enum {
            values,
            value_offsets,
            ..
        } = &self.types[t as usize].kind
            && let Some(i) = values.iter().position(|v| v == value_upper)
            && let Some(&off) = value_offsets.get(i)
        {
            return self.span_range(off, value_upper.len());
        }
        self.type_decl_range(t)
    }

    /// Enumeration types that contain `value_upper`.
    pub fn enums_with_value(&self, value_upper: &str) -> Vec<TypeId> {
        self.types
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                matches!(&t.kind, TypeKind::Enum { values, .. } if values.iter().any(|v| v == value_upper))
            })
            .map(|(i, _)| i as TypeId)
            .collect()
    }

    /// Identifier under `pos` in the schema source, as a byte span.
    pub fn word_at(&self, pos: Position) -> Option<(usize, usize)> {
        let line = pos.line as usize;
        let start = *self.line_starts.get(line)? as usize;
        let end = self
            .line_starts
            .get(line + 1)
            .map(|e| *e as usize)
            .unwrap_or(self.source.len());
        let bytes = self.source.as_bytes();
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let mut p = (start + pos.character as usize).min(end);
        if p >= end || !is_word(bytes[p]) {
            if p > start && is_word(bytes[p - 1]) {
                p -= 1;
            } else {
                return None;
            }
        }
        let mut s = p;
        while s > start && is_word(bytes[s - 1]) {
            s -= 1;
        }
        let mut e = p;
        while e < end && is_word(bytes[e]) {
            e += 1;
        }
        Some((s, e))
    }

    pub fn entity(&self, name: &str) -> Option<EntityId> {
        self.entity_by_name.get(&name.to_ascii_uppercase()).copied()
    }

    pub fn entity_bytes(&self, name: &[u8]) -> Option<EntityId> {
        self.entity(std::str::from_utf8(name).ok()?)
    }

    pub fn entity_def(&self, id: EntityId) -> &EntityDef {
        &self.entities[id as usize]
    }

    pub fn type_id(&self, name: &str) -> Option<TypeId> {
        self.type_by_name.get(&name.to_ascii_uppercase()).copied()
    }

    pub fn type_def(&self, id: TypeId) -> &TypeDef {
        &self.types[id as usize]
    }

    pub fn attributes(&self, id: EntityId) -> &[Attribute] {
        &self.entities[id as usize].flat
    }

    pub fn is_subtype_of(&self, sub: EntityId, sup: EntityId) -> bool {
        if sub == sup {
            return true;
        }
        let mut stack = vec![sub];
        let mut guard = 0;
        while let Some(e) = stack.pop() {
            guard += 1;
            if guard > 64 {
                return false;
            }
            for &p in &self.entities[e as usize].supertypes {
                if p == sup {
                    return true;
                }
                stack.push(p);
            }
        }
        false
    }

    pub fn select_closure(&self, id: TypeId) -> Option<&SelectClosure> {
        self.select_closures
            .get(id as usize)
            .and_then(|c| c.as_ref())
    }

    /// Follow defined-type aliases (`IfcPositiveInteger = IfcInteger = INTEGER`) to
    /// the underlying type expression. Stops at enums, selects and aggregates.
    pub fn base_type(&self, mut id: TypeId) -> (TypeId, Option<&TypeExpr>) {
        for _ in 0..32 {
            match &self.types[id as usize].kind {
                TypeKind::Defined(TypeExpr::Named(NamedRef::Type(next))) => id = *next,
                TypeKind::Defined(expr) => return (id, Some(expr)),
                _ => return (id, None),
            }
        }
        (id, None)
    }

    pub fn type_expr_to_string(&self, ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Simple(s) => s.as_str().to_string(),
            TypeExpr::Named(NamedRef::Entity(e)) => self.entities[*e as usize].name.clone(),
            TypeExpr::Named(NamedRef::Type(t)) => self.types[*t as usize].name.clone(),
            TypeExpr::Named(NamedRef::Unresolved(n)) => n.clone(),
            TypeExpr::Aggregate { kind, lo, hi, elem } => {
                let b = |v: &Option<u32>| v.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
                format!(
                    "{} [{}:{}] OF {}",
                    kind.as_str(),
                    b(lo),
                    b(hi),
                    self.type_expr_to_string(elem)
                )
            }
        }
    }

    fn resolve_ref(&self, r: &mut NamedRef) {
        if let NamedRef::Unresolved(n) = r {
            if let Some(e) = self.entity(n) {
                *r = NamedRef::Entity(e);
            } else if let Some(t) = self.type_id(n) {
                *r = NamedRef::Type(t);
            }
        }
    }

    fn resolve_expr(&self, ty: &mut TypeExpr) {
        match ty {
            TypeExpr::Named(r) => self.resolve_ref(r),
            TypeExpr::Aggregate { elem, .. } => self.resolve_expr(elem),
            TypeExpr::Simple(_) => {}
        }
    }

    pub(crate) fn resolve(&mut self) {
        self.entity_by_name = self
            .entities
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.to_ascii_uppercase(), i as EntityId))
            .collect();
        self.type_by_name = self
            .types
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.to_ascii_uppercase(), i as TypeId))
            .collect();

        // Resolve names inside types.
        let mut types = std::mem::take(&mut self.types);
        for td in &mut types {
            match &mut td.kind {
                TypeKind::Defined(expr) => self.resolve_expr(expr),
                TypeKind::Select { members, .. } => {
                    for m in members.iter_mut() {
                        self.resolve_ref(m);
                    }
                }
                TypeKind::Enum { .. } => {}
            }
        }
        self.types = types;

        // Merge BASED_ON extensions (IFC4 extensible enums/selects).
        for i in 0..self.types.len() {
            let base = match &self.types[i].kind {
                TypeKind::Enum {
                    based_on: Some(b), ..
                }
                | TypeKind::Select {
                    based_on: Some(b), ..
                } => self.type_id(b),
                _ => None,
            };
            if let Some(b) = base {
                let base_kind = self.types[b as usize].kind.clone();
                match (&mut self.types[i].kind, base_kind) {
                    (
                        TypeKind::Enum {
                            values,
                            value_offsets,
                            ..
                        },
                        TypeKind::Enum {
                            values: bv,
                            value_offsets: bo,
                            ..
                        },
                    ) => {
                        for (v, o) in bv.into_iter().zip(bo) {
                            if !values.contains(&v) {
                                values.push(v);
                                value_offsets.push(o);
                            }
                        }
                    }
                    (TypeKind::Select { members, .. }, TypeKind::Select { members: bm, .. }) => {
                        for m in bm {
                            if !members.contains(&m) {
                                members.push(m);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Resolve entity supertypes and attribute types.
        let mut entities = std::mem::take(&mut self.entities);
        for (i, ed) in entities.iter_mut().enumerate() {
            ed.supertypes = ed
                .supertype_names
                .iter()
                .filter_map(|n| self.entity(n))
                .collect();
            for a in &mut ed.own_attrs {
                a.owner = i as EntityId;
                self.resolve_expr(&mut a.ty);
            }
            for (_, _, _, ty) in &mut ed.explicit_redeclarations {
                self.resolve_expr(ty);
            }
        }
        self.entities = entities;

        // Flatten attributes, inherited first.
        let n = self.entities.len();
        let mut flat: Vec<Option<Vec<Attribute>>> = vec![None; n];
        for i in 0..n {
            self.flatten(i as EntityId, &mut flat, 0);
        }
        for (i, f) in flat.into_iter().enumerate() {
            self.entities[i].flat = f.unwrap_or_default();
        }

        // Select closures.
        self.select_closures = (0..self.types.len() as TypeId)
            .map(|t| self.compute_closure(t))
            .collect();
    }

    fn flatten(
        &self,
        id: EntityId,
        memo: &mut Vec<Option<Vec<Attribute>>>,
        depth: usize,
    ) -> Vec<Attribute> {
        if let Some(f) = &memo[id as usize] {
            return f.clone();
        }
        let ed = &self.entities[id as usize];
        let mut out = Vec::new();
        if depth < 64 {
            for &p in &ed.supertypes {
                out.extend(self.flatten(p, memo, depth + 1));
            }
        }
        for (sup, attr) in &ed.derived_redeclarations {
            let sup_id = self.entity(sup);
            for a in out.iter_mut() {
                if a.name.eq_ignore_ascii_case(attr)
                    && (sup_id.is_none_or(|s| self.is_subtype_of(s, a.owner) || a.owner == s))
                {
                    a.derived = true;
                }
            }
        }
        for (sup, attr, optional, ty) in &ed.explicit_redeclarations {
            let sup_id = self.entity(sup);
            for a in out.iter_mut() {
                if a.name.eq_ignore_ascii_case(attr)
                    && (sup_id.is_none_or(|s| self.is_subtype_of(s, a.owner) || a.owner == s))
                {
                    a.optional = *optional;
                    a.ty = ty.clone();
                }
            }
        }
        out.extend(ed.own_attrs.iter().cloned());
        memo[id as usize] = Some(out.clone());
        out
    }

    fn compute_closure(&self, id: TypeId) -> Option<SelectClosure> {
        let TypeKind::Select { .. } = &self.types[id as usize].kind else {
            return None;
        };
        let mut closure = SelectClosure::default();
        let mut seen = vec![false; self.types.len()];
        let mut stack = vec![id];
        while let Some(t) = stack.pop() {
            if seen[t as usize] {
                continue;
            }
            seen[t as usize] = true;
            if let TypeKind::Select { members, .. } = &self.types[t as usize].kind {
                for m in members {
                    match m {
                        NamedRef::Entity(e) => closure.entities.push(*e),
                        NamedRef::Type(tt) => match &self.types[*tt as usize].kind {
                            TypeKind::Select { .. } => stack.push(*tt),
                            _ => closure.types.push(*tt),
                        },
                        NamedRef::Unresolved(_) => {}
                    }
                }
            }
        }
        closure.entities.sort_unstable();
        closure.entities.dedup();
        closure.types.sort_unstable();
        closure.types.dedup();
        Some(closure)
    }
}

struct Embedded {
    name: &'static str,
    source: &'static str,
    parsed: OnceLock<Arc<Schema>>,
}

/// The schemas the server knows about: the embedded IFC releases plus any `.exp`
/// files loaded from user-configured directories.
pub struct SchemaSet {
    embedded: Vec<Embedded>,
    extra: Vec<(String, Arc<Schema>)>,
    /// When set, embedded schemas are written here on first use so that clients can
    /// open them as ordinary files.
    cache_dir: Option<PathBuf>,
}

const EMBEDDED: &[(&str, &str)] = &[
    (
        "IFC2X3_TC1",
        include_str!("../../../schemas/IFC2X3_TC1.exp"),
    ),
    (
        "IFC4_ADD2_TC1",
        include_str!("../../../schemas/IFC4_ADD2_TC1.exp"),
    ),
    ("IFC4X1", include_str!("../../../schemas/IFC4X1.exp")),
    ("IFC4X2", include_str!("../../../schemas/IFC4X2.exp")),
    (
        "IFC4X3_ADD1",
        include_str!("../../../schemas/IFC4X3_ADD1.exp"),
    ),
    (
        "IFC4X3_ADD2",
        include_str!("../../../schemas/IFC4X3_ADD2.exp"),
    ),
];

impl Default for SchemaSet {
    fn default() -> Self {
        Self::embedded()
    }
}

impl SchemaSet {
    pub fn embedded() -> Self {
        Self {
            embedded: EMBEDDED
                .iter()
                .map(|(name, source)| Embedded {
                    name,
                    source,
                    parsed: OnceLock::new(),
                })
                .collect(),
            extra: Vec::new(),
            cache_dir: None,
        }
    }

    pub fn with_cache_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.cache_dir = dir;
        self
    }

    /// Per-user cache directory for materialised schemas.
    pub fn default_cache_dir() -> Option<PathBuf> {
        let base = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches"))
        } else {
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        }?;
        Some(base.join("ifc-lsp").join("schemas"))
    }

    fn materialize(dir: &Path, name: &str, source: &str) -> Option<PathBuf> {
        let path = dir.join(format!("{name}.exp"));
        let current = std::fs::metadata(&path).map(|m| m.len()).ok();
        if current != Some(source.len() as u64)
            && let Err(e) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, source))
        {
            log::warn!("cannot write schema cache {}: {e}", path.display());
            return None;
        }
        Some(path)
    }

    pub fn names(&self) -> Vec<String> {
        self.embedded
            .iter()
            .map(|e| e.name.to_string())
            .chain(self.extra.iter().map(|(n, _)| n.clone()))
            .collect()
    }

    /// Load every `*.exp` in `dir`; returns the schema names loaded.
    pub fn add_directory(&mut self, dir: &Path) -> std::io::Result<Vec<String>> {
        let mut loaded = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exp"))
            {
                let src = std::fs::read_to_string(&path)?;
                let mut schema = Schema::parse(&src);
                if !schema.name.is_empty() {
                    let name = schema.name.to_ascii_uppercase();
                    schema.key = name.clone();
                    schema.source_path = Some(path.clone());
                    self.extra.retain(|(n, _)| n != &name);
                    loaded.push(name.clone());
                    self.extra.push((name, Arc::new(schema)));
                }
            }
        }
        Ok(loaded)
    }

    /// Canonical embedded schema for a `FILE_SCHEMA` value.
    pub fn canonical_name(file_schema: &str) -> Option<&'static str> {
        let s: String = file_schema
            .trim()
            .to_ascii_uppercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let s = s.as_str();
        Some(match s {
            _ if s.starts_with("IFC2X3") => "IFC2X3_TC1",
            "IFC4" | "IFC4_ADD1" | "IFC4_ADD2" | "IFC4_ADD2_TC1" | "IFC4X0" => "IFC4_ADD2_TC1",
            _ if s.starts_with("IFC4X1") => "IFC4X1",
            _ if s.starts_with("IFC4X2") => "IFC4X2",
            "IFC4X3_ADD1" => "IFC4X3_ADD1",
            _ if s.starts_with("IFC4X3") => "IFC4X3_ADD2",
            _ => return None,
        })
    }

    pub fn by_name(&self, name: &str) -> Option<Arc<Schema>> {
        let upper = name.to_ascii_uppercase();
        if let Some((_, s)) = self.extra.iter().find(|(n, _)| *n == upper) {
            return Some(s.clone());
        }
        let e = self.embedded.iter().find(|e| e.name == upper)?;
        Some(
            e.parsed
                .get_or_init(|| {
                    let mut s = Schema::parse(e.source);
                    s.key = e.name.to_string();
                    if let Some(dir) = &self.cache_dir {
                        s.source_path = Self::materialize(dir, e.name, e.source);
                    }
                    Arc::new(s)
                })
                .clone(),
        )
    }

    /// Schema for a `FILE_SCHEMA` value: an exact extra/embedded name first, then
    /// the release aliases.
    pub fn for_file_schema(&self, file_schema: &str) -> Option<Arc<Schema>> {
        if let Some(s) = self.by_name(file_schema) {
            return Some(s);
        }
        let canonical = Self::canonical_name(file_schema)?;
        self.by_name(canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_small_schema() {
        let src = r#"
SCHEMA TEST;
TYPE IfcLabel = STRING(255); END_TYPE;
TYPE IfcLengthMeasure = REAL; END_TYPE;
TYPE IfcPositiveLengthMeasure = IfcLengthMeasure; WHERE WR1 : SELF > 0.; END_TYPE;
TYPE IfcRoleEnum = ENUMERATION OF (SUPPLIER, USERDEFINED); END_TYPE;
TYPE IfcValue = SELECT (IfcMeasureValue, IfcLabel); END_TYPE;
TYPE IfcMeasureValue = SELECT (IfcLengthMeasure); END_TYPE;
TYPE IfcCompound = LIST [3:4] OF INTEGER; END_TYPE;
ENTITY IfcRoot ABSTRACT SUPERTYPE OF (ONEOF (IfcObject));
  GlobalId : IfcLabel;
  Name : OPTIONAL IfcLabel;
 UNIQUE UR1 : GlobalId;
END_ENTITY;
ENTITY IfcObject SUBTYPE OF (IfcRoot);
  Items : SET [1:?] OF IfcRoot;
  Value : OPTIONAL IfcValue;
 WHERE WR1 : SIZEOF(QUERY(t <* Items | 'X' IN TYPEOF(t))) = 0;
END_ENTITY;
ENTITY IfcNamedUnit;
  Dimensions : IfcCompound;
  Role : IfcRoleEnum;
END_ENTITY;
ENTITY IfcSIUnit SUBTYPE OF (IfcNamedUnit);
  Prefix : OPTIONAL IfcLabel;
 DERIVE
  SELF\IfcNamedUnit.Dimensions : IfcCompound := Dims(SELF);
END_ENTITY;
FUNCTION Dims(x : IfcSIUnit) : IfcCompound; RETURN([1,2,3]); END_FUNCTION;
RULE R FOR (IfcRoot); WHERE WR1 : TRUE; END_RULE;
END_SCHEMA;
"#;
        let s = Schema::parse(src);
        assert_eq!(s.name, "TEST");
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.entities.len(), 4);
        let obj = s.entity("IFCOBJECT").unwrap();
        let attrs: Vec<&str> = s.attributes(obj).iter().map(|a| a.name.as_str()).collect();
        assert_eq!(attrs, vec!["GlobalId", "Name", "Items", "Value"]);
        assert!(s.entity_def(s.entity("ifcroot").unwrap()).is_abstract);
        assert!(s.is_subtype_of(obj, s.entity("IfcRoot").unwrap()));
        let si = s.entity("IfcSIUnit").unwrap();
        let si_attrs = s.attributes(si);
        assert_eq!(si_attrs.len(), 3);
        assert!(
            si_attrs[0].derived,
            "Dimensions must be derived in IfcSIUnit"
        );
        assert!(!s.attributes(s.entity("IfcNamedUnit").unwrap())[0].derived);
        let value = s.type_id("IfcValue").unwrap();
        let closure = s.select_closure(value).unwrap();
        let names: Vec<&str> = closure
            .types
            .iter()
            .map(|t| s.type_def(*t).name.as_str())
            .collect();
        assert_eq!(names, vec!["IfcLabel", "IfcLengthMeasure"]);
        let (base, expr) = s.base_type(s.type_id("IfcPositiveLengthMeasure").unwrap());
        assert_eq!(s.type_def(base).name, "IfcLengthMeasure");
        assert_eq!(expr, Some(&TypeExpr::Simple(Simple::Real)));
        assert_eq!(
            s.type_expr_to_string(&s.attributes(obj)[2].ty),
            "SET [1:?] OF IfcRoot"
        );
    }

    #[test]
    fn embedded_schemas_parse() {
        let set = SchemaSet::embedded();
        let expected = [
            ("IFC2X3_TC1", 653),
            ("IFC4_ADD2_TC1", 776),
            ("IFC4X1", 801),
            ("IFC4X2", 816),
            ("IFC4X3_ADD1", 876),
            ("IFC4X3_ADD2", 876),
        ];
        for (name, count) in expected {
            let s = set.by_name(name).unwrap_or_else(|| panic!("{name}"));
            assert!(
                name.starts_with(&s.name.to_ascii_uppercase()),
                "{name} declares {}",
                s.name
            );
            assert_eq!(s.entities.len(), count, "{name}");
            assert!(s.errors.is_empty(), "{name}: {:?}", s.errors);
            let unresolved: Vec<String> = s
                .entities
                .iter()
                .flat_map(|e| e.flat.iter())
                .filter_map(|a| match &a.ty {
                    TypeExpr::Named(NamedRef::Unresolved(n)) => Some(n.clone()),
                    TypeExpr::Aggregate { elem, .. } => match &**elem {
                        TypeExpr::Named(NamedRef::Unresolved(n)) => Some(n.clone()),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            assert!(unresolved.is_empty(), "{name}: unresolved {unresolved:?}");
            for e in &s.entities {
                assert_eq!(
                    e.supertypes.len(),
                    e.supertype_names.len(),
                    "{name}: {}",
                    e.name
                );
            }
        }
    }

    #[test]
    fn declaration_positions() {
        let set = SchemaSet::embedded();
        let s = set.by_name("IFC4_ADD2_TC1").unwrap();
        assert_eq!(s.key, "IFC4_ADD2_TC1");
        let src = s.source();
        let site = s.entity("IfcSite").unwrap();
        let r = s.entity_decl_range(site);
        let line = src.lines().nth(r.start.line as usize).unwrap();
        assert_eq!(line.trim_end(), "ENTITY IfcSite");
        assert_eq!(
            &line[r.start.character as usize..r.end.character as usize],
            "IfcSite"
        );
        let unit = s.type_id("IfcUnitEnum").unwrap();
        let r = s.enum_value_range(unit, "LENGTHUNIT");
        let line = src.lines().nth(r.start.line as usize).unwrap();
        assert_eq!(
            &line[r.start.character as usize..r.end.character as usize],
            "LENGTHUNIT"
        );
        assert!(s.enums_with_value("LENGTHUNIT").contains(&unit));
        let r = s.type_decl_range(unit);
        assert_eq!(
            src.lines().nth(r.start.line as usize).unwrap().trim_end(),
            "TYPE IfcUnitEnum = ENUMERATION OF"
        );
        let w = s
            .word_at(Position::new(r.start.line, r.start.character + 3))
            .unwrap();
        assert_eq!(&src[w.0..w.1], "IfcUnitEnum");
    }

    #[test]
    fn ifc_site_attributes() {
        let set = SchemaSet::embedded();
        let s4 = set.for_file_schema("IFC4").unwrap();
        let site = s4.entity("IFCSITE").unwrap();
        let names: Vec<&str> = s4
            .attributes(site)
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "GlobalId",
                "OwnerHistory",
                "Name",
                "Description",
                "ObjectType",
                "ObjectPlacement",
                "Representation",
                "LongName",
                "CompositionType",
                "RefLatitude",
                "RefLongitude",
                "RefElevation",
                "LandTitleNumber",
                "SiteAddress"
            ]
        );
        let s23 = set.for_file_schema("IFC2X3").unwrap();
        assert_eq!(s23.attributes(s23.entity("IfcSite").unwrap()).len(), 14);
        let unit = s23.entity("IfcSIUnit").unwrap();
        let a = s23.attributes(unit);
        assert_eq!(a[0].name, "Dimensions");
        assert!(a[0].derived);
        let value = s4.type_id("IfcValue").unwrap();
        let label = s4.type_id("IfcLabel").unwrap();
        assert!(s4.select_closure(value).unwrap().types.contains(&label));
        assert_eq!(
            SchemaSet::canonical_name("IFC4X3_ADD2"),
            Some("IFC4X3_ADD2")
        );
        assert_eq!(SchemaSet::canonical_name("ifc2x3"), Some("IFC2X3_TC1"));
        assert_eq!(SchemaSet::canonical_name("IFC4X3_RC4"), Some("IFC4X3_ADD2"));
        assert_eq!(SchemaSet::canonical_name("IFC5"), None);
    }
}
