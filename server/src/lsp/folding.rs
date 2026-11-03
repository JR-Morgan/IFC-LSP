//! Folding ranges: section keywords in IFC files, block declarations in EXPRESS schemas.
//! The closing keyword line stays visible, like bracket folding in most editors.

use lsp_types::{FoldingRange, FoldingRangeKind};

use crate::index::Document;
use crate::scanner::flags;
use crate::schema::{Schema, express};

fn region(start_line: u32, end_line: u32) -> FoldingRange {
    FoldingRange {
        start_line,
        start_character: None,
        end_line,
        end_character: None,
        kind: Some(FoldingRangeKind::Region),
        collapsed_text: None,
    }
}

/// `HEADER;`/`DATA;` through the line before the matching `ENDSEC;`.
pub fn ifc_sections(doc: &Document) -> Vec<FoldingRange> {
    let mut open: Vec<u32> = Vec::new();
    let mut out = Vec::new();
    for r in &doc.entities {
        if r.flags & flags::KEYWORD == 0 {
            continue;
        }
        match doc.names.get_str(r.name) {
            "HEADER" | "DATA" => open.push(doc.text.position(r.start).line),
            "ENDSEC" => {
                if let Some(start) = open.pop() {
                    let end = doc.text.position(r.start).line;
                    if end > start + 1 {
                        out.push(region(start, end - 1));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Each ENTITY/TYPE/FUNCTION/RULE/... declaration through the line before its `END_*`.
pub fn express_blocks(schema: &Schema) -> Vec<FoldingRange> {
    express::block_spans(schema.source())
        .into_iter()
        .filter_map(|(s, e)| {
            let start = schema.position(s).line;
            let end = schema.position(e).line;
            (end > start + 1).then(|| region(start, end - 1))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(ranges: &[FoldingRange]) -> Vec<(u32, u32)> {
        ranges.iter().map(|r| (r.start_line, r.end_line)).collect()
    }

    #[test]
    fn ifc_header_and_data_sections() {
        let text = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1=IFCDIRECTION((0.,0.,1.));\n#2=IFCDIRECTION((1.,0.,0.));\nENDSEC;\nEND-ISO-10303-21;\n";
        let doc = Document::new(text, 0);
        assert_eq!(lines(&ifc_sections(&doc)), vec![(1, 3), (5, 7)]);
    }

    #[test]
    fn ifc_empty_or_missing_sections_do_not_fold() {
        let doc = Document::new(
            "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1=IFCDIRECTION((0.,0.,1.));\n",
            0,
        );
        assert!(ifc_sections(&doc).is_empty());
    }

    #[test]
    fn express_declarations() {
        let schema = Schema::parse(
            "SCHEMA S;\nTYPE A = ENUMERATION OF\n  (X,\n   Y);\nEND_TYPE;\nTYPE B = REAL;\nEND_TYPE;\nENTITY C;\n  a : A;\nEND_ENTITY;\nEND_SCHEMA;\n",
        );
        assert_eq!(lines(&express_blocks(&schema)), vec![(1, 3), (7, 8)]);
    }
}
