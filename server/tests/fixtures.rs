use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ifc_lsp::index::Document;
use ifc_lsp::schema::SchemaSet;
use ifc_lsp::validate::{Diagnostic, validate_all};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn lint(path: &Path) -> (Document, Vec<Diagnostic>) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let doc = Document::new(bytes, 0);
    let diags = validate_all(&doc, &SchemaSet::embedded(), None);
    (doc, diags)
}

fn describe(doc: &Document, diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| {
            let p = doc.text.position(d.start);
            format!(
                "{}:{} {} {}",
                p.line + 1,
                p.character + 1,
                d.code,
                d.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn valid_fixtures_have_no_diagnostics() {
    for name in ["valid_ifc4.ifc", "valid_ifc2x3.ifc"] {
        let (doc, diags) = lint(&fixture(name));
        assert!(diags.is_empty(), "{name}:\n{}", describe(&doc, &diags));
    }
}

#[test]
fn invalid_fixture_reports_expected_codes() {
    let (doc, diags) = lint(&fixture("invalid_ifc4.ifc"));
    let codes: BTreeSet<&str> = diags.iter().map(|d| d.code).collect();
    let expected: BTreeSet<&str> = [
        "E001", "E002", "E003", "E004", "E005", "E006", "E007", "E008", "E009", "E010",
    ]
    .into();
    assert_eq!(codes, expected, "{}", describe(&doc, &diags));
    // The unterminated string on line 22 must not hide the record after it.
    assert!(doc.record(15).is_some());
    let line_of = |id: u32| doc.text.position(doc.record(id).unwrap().start).line + 1;
    assert_eq!(line_of(15), 23);
}

/// Lints every `.ifc` under `IFC_LSP_SAMPLE_DIR` (not part of the repository) and
/// expects them to be clean. Skipped when the variable is unset.
#[test]
fn sample_directory_is_clean() {
    let Ok(dir) = std::env::var("IFC_LSP_SAMPLE_DIR") else {
        eprintln!("IFC_LSP_SAMPLE_DIR not set; skipping");
        return;
    };
    let mut failures = Vec::new();
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("sample dir") {
        let path = entry.unwrap().path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ifc"))
        {
            continue;
        }
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 200 * 1024 * 1024 {
            continue;
        }
        count += 1;
        let (doc, diags) = lint(&path);
        if !diags.is_empty() {
            failures.push(format!(
                "{}:\n{}",
                path.display(),
                describe(&doc, &diags[..diags.len().min(20)])
            ));
        }
    }
    assert!(count > 0, "no .ifc files in {dir}");
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
