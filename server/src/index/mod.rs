//! Per-document entity index with incremental updates.

use rustc_hash::FxHashMap;

use crate::lexer::{Lexer, TokenKind};
use crate::scanner::{EntityRecord, NameInterner, Scanner, flags, scan_all};
use crate::text::{Offset, TextBuffer};

#[derive(Debug, Clone)]
enum IdMap {
    /// `id -> entity index + 1`; 0 means absent.
    Dense(Vec<u32>),
    Sparse(FxHashMap<u32, u32>),
}

impl IdMap {
    fn get(&self, id: u32) -> Option<usize> {
        match self {
            IdMap::Dense(v) => v
                .get(id as usize)
                .copied()
                .filter(|&x| x != 0)
                .map(|x| x as usize - 1),
            IdMap::Sparse(m) => m.get(&id).map(|&x| x as usize),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Document {
    pub text: TextBuffer,
    pub names: NameInterner,
    /// Records sorted by `start`, non-overlapping.
    pub entities: Vec<EntityRecord>,
    ids: IdMap,
    /// `(id, entity index)` for every definition of an id after its first.
    pub duplicates: Vec<(u32, u32)>,
    /// Value of the first string in `FILE_SCHEMA`, upper-cased, if present.
    pub schema_name: Option<String>,
    pub version: i32,
}

impl Document {
    pub fn new(text: impl Into<Vec<u8>>, version: i32) -> Self {
        let text = TextBuffer::new(text);
        let mut names = NameInterner::default();
        let entities = scan_all(text.bytes(), &mut names);
        let mut doc = Self {
            text,
            names,
            entities,
            ids: IdMap::Dense(Vec::new()),
            duplicates: Vec::new(),
            schema_name: None,
            version,
        };
        doc.rebuild_ids();
        doc.schema_name = doc.find_schema_name();
        doc
    }

    pub fn entity_count(&self) -> usize {
        self.entities.iter().filter(|r| r.has_id()).count()
    }

    /// Approximate heap footprint of the index excluding the text itself.
    pub fn index_bytes(&self) -> usize {
        let ids = match &self.ids {
            IdMap::Dense(v) => v.capacity() * 4,
            IdMap::Sparse(m) => m.capacity() * 12,
        };
        self.entities.capacity() * std::mem::size_of::<EntityRecord>()
            + ids
            + self.duplicates.capacity() * 8
    }

    /// Index of the entity record defining `id`.
    pub fn lookup(&self, id: u32) -> Option<usize> {
        self.ids.get(id)
    }

    pub fn record(&self, id: u32) -> Option<&EntityRecord> {
        self.lookup(id).map(|i| &self.entities[i])
    }

    /// Index of the record whose span contains `offset`.
    pub fn entity_at(&self, offset: Offset) -> Option<usize> {
        let i = self.entities.partition_point(|r| r.start <= offset);
        if i == 0 {
            return None;
        }
        let r = &self.entities[i - 1];
        (offset < r.end).then_some(i - 1)
    }

    /// Index of the first record ending at or after `offset`.
    pub fn first_entity_from(&self, offset: Offset) -> usize {
        self.entities.partition_point(|r| r.end < offset)
    }

    pub fn entity_text(&self, idx: usize) -> &[u8] {
        let r = &self.entities[idx];
        self.text.slice(r.start, r.end)
    }

    /// The `#id` token under `offset`, as `(id, start, end)`.
    pub fn ref_at(&self, offset: Offset) -> Option<(u32, Offset, Offset)> {
        let bytes = self.text.bytes();
        let mut s = offset as usize;
        if s >= bytes.len() {
            return None;
        }
        // Allow the cursor to sit just after the last digit.
        if !bytes[s].is_ascii_digit() && bytes[s] != b'#' && s > 0 && bytes[s - 1].is_ascii_digit()
        {
            s -= 1;
        }
        while s > 0 && bytes[s].is_ascii_digit() {
            s -= 1;
        }
        if bytes[s] != b'#' {
            return None;
        }
        let mut e = s + 1;
        let mut id: u64 = 0;
        while e < bytes.len() && bytes[e].is_ascii_digit() {
            id = id
                .saturating_mul(10)
                .saturating_add((bytes[e] - b'0') as u64);
            e += 1;
        }
        if e == s + 1 || id == 0 || id > u32::MAX as u64 {
            return None;
        }
        Some((id as u32, s as Offset, e as Offset))
    }

    /// Replace `[start, end)` with `inserted` and bring the index up to date by
    /// rescanning only the affected records.
    pub fn apply_edit(&mut self, start: Offset, end: Offset, inserted: &[u8]) {
        let delta = inserted.len() as i64 - (end - start) as i64;
        // Rescan from one record before the first touched one: a record ending at a
        // newline because the *next* line looked like an entity may grow back.
        let first_touched = self.entities.partition_point(|r| r.end < start);
        let i0 = first_touched.saturating_sub(1);
        let rescan_from = if i0 == 0 {
            0
        } else {
            self.entities[i0 - 1].end
        };

        self.text.splice(start, end, inserted);
        let new_end = (end as i64 + delta) as Offset;

        let mut new_recs: Vec<EntityRecord> = Vec::new();
        let mut scanner = Scanner::new(self.text.bytes(), rescan_from);
        let k = loop {
            match scanner.peek_start() {
                None => break self.entities.len(),
                Some(s) => {
                    if s >= new_end {
                        let old_s = s as i64 - delta;
                        if old_s >= 0
                            && let Ok(j) = self.entities[i0..]
                                .binary_search_by_key(&(old_s as Offset), |r| r.start)
                        {
                            break i0 + j;
                        }
                    }
                    match scanner.next_record(&mut self.names) {
                        Some(r) => new_recs.push(r),
                        None => break self.entities.len(),
                    }
                }
            }
        };

        let removed = &self.entities[i0..k];
        let ids_unchanged = removed.len() == new_recs.len()
            && removed
                .iter()
                .zip(&new_recs)
                .all(|(a, b)| a.id == b.id && a.has_id() == b.has_id());
        let touches_header = removed.iter().chain(&new_recs).any(|r| r.is_header());

        if delta != 0 {
            for r in &mut self.entities[k..] {
                r.shift(delta);
            }
        }
        self.entities.splice(i0..k, new_recs);

        if !ids_unchanged {
            self.rebuild_ids();
        }
        if touches_header {
            self.schema_name = self.find_schema_name();
        }
    }

    fn rebuild_ids(&mut self) {
        let max_id = self
            .entities
            .iter()
            .filter(|r| r.has_id())
            .map(|r| r.id)
            .max()
            .unwrap_or(0) as usize;
        let count = self.entities.len();
        self.duplicates.clear();
        if max_id <= 4 * count + 1024 {
            let mut v = vec![0u32; max_id + 1];
            for (idx, r) in self.entities.iter().enumerate() {
                if r.has_id() {
                    let slot = &mut v[r.id as usize];
                    if *slot == 0 {
                        *slot = idx as u32 + 1;
                    } else {
                        self.duplicates.push((r.id, idx as u32));
                    }
                }
            }
            self.ids = IdMap::Dense(v);
        } else {
            let mut m = FxHashMap::with_capacity_and_hasher(count, Default::default());
            for (idx, r) in self.entities.iter().enumerate() {
                if r.has_id() && m.insert(r.id, idx as u32).is_some() {
                    m.insert(r.id, m[&r.id]);
                    self.duplicates.push((r.id, idx as u32));
                }
            }
            // Keep the first definition as the canonical one.
            for &(id, _) in &self.duplicates {
                if let Some(first) = self.entities.iter().position(|r| r.has_id() && r.id == id) {
                    m.insert(id, first as u32);
                }
            }
            self.ids = IdMap::Sparse(m);
        }
    }

    fn find_schema_name(&self) -> Option<String> {
        let file_schema = self.names.lookup(b"FILE_SCHEMA")?;
        let rec = self
            .entities
            .iter()
            .take(64)
            .find(|r| r.flags & flags::HEADER != 0 && r.name == file_schema)?;
        let bytes = self.text.bytes();
        let mut lx = Lexer::new(bytes, rec.start, rec.end);
        loop {
            let t = lx.next_token();
            match t.kind {
                TokenKind::String => {
                    let s = t.text(bytes);
                    let inner = &s[1..s.len() - 1];
                    return Some(String::from_utf8_lossy(inner).trim().to_ascii_uppercase());
                }
                TokenKind::Eof | TokenKind::Semicolon => return None,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('x'),'2;1');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1=IFCOWNERHISTORY(#2,#5,$,.ADDED.,1731578952,#2,#5,1731578952);\n#2=IFCPERSONANDORGANIZATION(#3,#4,$);\n#3=IFCPERSON('id','Jan',$,$,$,$,$,$);\n#4=IFCORGANIZATION($,'bSI','desc',$,$);\n#5=IFCAPPLICATION(#6,'5.3.3','app','su');\n#6=IFCORGANIZATION($,'BIM-Tools',$,$,$);\n#7=IFCAXIS2PLACEMENT3D(#8,#9,#10);\n#8=IFCCARTESIANPOINT((0.,0.,0.));\n#9=IFCDIRECTION((0.,0.,1.));\n#10=IFCDIRECTION((1.,0.,0.));\nENDSEC;\nEND-ISO-10303-21;\n";

    #[test]
    fn builds_index() {
        let doc = Document::new(SAMPLE, 0);
        assert_eq!(doc.schema_name.as_deref(), Some("IFC4"));
        assert_eq!(doc.entity_count(), 10);
        let r = doc.record(8).unwrap();
        assert_eq!(doc.names.get_str(r.name), "IFCCARTESIANPOINT");
        assert!(doc.record(11).is_none());
        let off = SAMPLE.find("#9,#10").unwrap() as Offset;
        assert_eq!(doc.ref_at(off), Some((9, off, off + 2)));
        assert_eq!(doc.ref_at(off + 1), Some((9, off, off + 2)));
        assert_eq!(doc.ref_at(off + 2), Some((9, off, off + 2)));
        assert_eq!(doc.ref_at(off + 3), Some((10, off + 3, off + 6)));
        assert_eq!(doc.entity_at(off), Some(doc.lookup(7).unwrap()));
    }

    #[test]
    fn duplicates_tracked() {
        let doc = Document::new("#1=A();\n#2=B();\n#1=C();\n", 0);
        assert_eq!(doc.duplicates, vec![(1, 2)]);
        assert_eq!(doc.lookup(1), Some(0));
    }

    #[test]
    fn sparse_ids() {
        let doc = Document::new("#1=A();\n#4000000000=B(#1);\n", 0);
        assert!(matches!(doc.ids, IdMap::Sparse(_)));
        assert_eq!(doc.lookup(4000000000), Some(1));
    }

    fn check_consistent(doc: &Document) {
        let mut names = NameInterner::default();
        let expected = scan_all(doc.text.bytes(), &mut names);
        let got: Vec<(Offset, Offset, u32, &str, u16)> = doc
            .entities
            .iter()
            .map(|r| (r.start, r.end, r.id, doc.names.get_str(r.name), r.flags))
            .collect();
        let exp: Vec<(Offset, Offset, u32, &str, u16)> = expected
            .iter()
            .map(|r| (r.start, r.end, r.id, names.get_str(r.name), r.flags))
            .collect();
        assert_eq!(
            got,
            exp,
            "text:\n{}",
            String::from_utf8_lossy(doc.text.bytes())
        );
        let fresh = Document::new(doc.text.bytes().to_vec(), 0);
        for r in &doc.entities {
            if r.has_id() {
                assert_eq!(doc.lookup(r.id), fresh.lookup(r.id), "id {}", r.id);
            }
        }
        assert_eq!(doc.duplicates, fresh.duplicates);
        assert_eq!(doc.schema_name, fresh.schema_name);
    }

    #[test]
    fn incremental_edits_match_full_rescan() {
        let mut doc = Document::new(SAMPLE, 0);
        let edits: Vec<(&str, &str)> = vec![
            ("#3=IFCPERSON(", "#3=IFCPERSON('new',"),
            ("#4=IFCORGANIZATION($,'bSI'", "#4=IFCORGANIZATION($,'bSI"),
            ("#4=IFCORGANIZATION($,'bSI", "#4=IFCORGANIZATION($,'bSI'"),
            (
                "#9=IFCDIRECTION((0.,0.,1.));",
                "#9=IFCDIRECTION((0.,0.,1.))",
            ),
            (
                "#9=IFCDIRECTION((0.,0.,1.))",
                "#9=IFCDIRECTION((0.,0.,1.));\n#11=IFCWALL();",
            ),
            ("#11=IFCWALL();", ""),
            ("FILE_SCHEMA(('IFC4'))", "FILE_SCHEMA(('IFC2X3'))"),
            ("#2=IFCPERSONANDORGANIZATION(#3,#4,$);\n", ""),
            ("#5=IFCAPPLICATION", "#5=\nIFCAPPLICATION"),
        ];
        for (from, to) in edits {
            let text = String::from_utf8(doc.text.bytes().to_vec()).unwrap();
            let a = text
                .find(from)
                .unwrap_or_else(|| panic!("missing {from:?}")) as Offset;
            doc.apply_edit(a, a + from.len() as Offset, to.as_bytes());
            check_consistent(&doc);
        }
        assert_eq!(doc.schema_name.as_deref(), Some("IFC2X3"));
    }

    #[test]
    fn random_edits_match_full_rescan() {
        let mut seed = 0x1234_5678_9ABC_DEF1u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let alphabet = b"ab\n\n;;#=(),'x1 /*";
        let mut doc = Document::new(SAMPLE, 0);
        for _ in 0..3000 {
            let len = doc.text.len();
            let a = (next() % (len as u64 + 1)) as Offset;
            let b = (a + (next() % 12) as Offset).min(len);
            let n = (next() % 10) as usize;
            let ins: Vec<u8> = (0..n)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            doc.apply_edit(a, b, &ins);
            check_consistent(&doc);
        }
    }
}
