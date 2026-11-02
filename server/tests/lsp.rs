//! Drives the real binary over stdio: open, hover, definition, references,
//! incremental change, diagnostics, shutdown.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Lsp {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    next_id: i64,
}

impl Lsp {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ifc-lsp"))
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("IFC_LSP_LOG", "warn")
            .spawn()
            .expect("spawn ifc-lsp");
        let reader = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            reader,
            next_id: 1,
        }
    }

    fn send(&mut self, msg: Value) {
        if std::env::var_os("IFC_LSP_TEST_TRACE").is_some() {
            eprintln!("-> {msg}");
        }
        let body = serde_json::to_vec(&msg).unwrap();
        let stdin = self.child.stdin.as_mut().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        stdin.write_all(&body).unwrap();
        stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.wait_for(|m| m.get("id") == Some(&json!(id)))
    }

    fn read(&mut self) -> Value {
        let mut len = None;
        loop {
            let mut line = String::new();
            assert!(
                self.reader.read_line(&mut line).unwrap() > 0,
                "server closed stdout"
            );
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = Some(v.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0u8; len.expect("Content-Length")];
        self.reader.read_exact(&mut body).unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        if std::env::var_os("IFC_LSP_TEST_TRACE").is_some() {
            eprintln!("<- {v}");
        }
        v
    }

    fn wait_for(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(Instant::now() < deadline, "timed out waiting for a message");
            let m = self.read();
            if pred(&m) {
                return m;
            }
        }
    }

    fn wait_for_diagnostics(&mut self, version: i64) -> Vec<Value> {
        loop {
            let m = self
                .wait_for(|m| m.get("method") == Some(&json!("textDocument/publishDiagnostics")));
            let params = &m["params"];
            if params["version"] == json!(version) {
                let diags = params["diagnostics"].as_array().unwrap().clone();
                // Partial publishes carry an I002 progress marker; wait for the final one.
                if !diags.iter().any(|d| d["code"] == json!("I002")) {
                    return diags;
                }
            }
        }
    }
}

const URI: &str = "file:///tmp/test.ifc";
const TEXT: &str = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1=IFCCARTESIANPOINT((0.,0.,0.));\n#2=IFCDIRECTION((0.,0.,1.));\n#3=IFCAXIS2PLACEMENT3D(#1,#2,#99);\n#4=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\nENDSEC;\nEND-ISO-10303-21;\n";

#[test]
fn end_to_end_over_stdio() {
    let mut lsp = Lsp::start();
    let init = lsp.request(
        "initialize",
        json!({"processId": null, "rootUri": null, "capabilities": {}, "initializationOptions": {"schemaDocuments": "virtual"}}),
    );
    assert_eq!(init["result"]["capabilities"]["textDocumentSync"], json!(2));
    assert_eq!(init["result"]["capabilities"]["hoverProvider"], json!(true));
    lsp.notify("initialized", json!({}));

    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": URI, "languageId": "ifc", "version": 1, "text": TEXT}}),
    );
    let diags = lsp.wait_for_diagnostics(1);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0]["code"], json!("E004"));
    assert_eq!(
        diags[0]["range"]["start"],
        json!({"line": 7, "character": 29})
    );

    // Hover over `IFCAXIS2PLACEMENT3D` on line 7.
    let hover = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": URI}, "position": {"line": 7, "character": 8}}),
    );
    let md = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(md.contains("### IfcAxis2Placement3D"), "{md}");
    assert!(
        md.contains(
            "| 1 | Location _(IfcPlacement)_ | IfcCartesianPoint | `#1` IfcCartesianPoint |"
        ),
        "{md}"
    );
    assert!(md.contains("`#99` ⚠ undefined"), "{md}");

    // Hover over `#2` shows the target record.
    let hover = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": URI}, "position": {"line": 7, "character": 27}}),
    );
    let md = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(md.contains("**#2** = IFCDIRECTION · line 7"), "{md}");

    // Definition of `#1` jumps to line 5.
    let def = lsp.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": URI}, "position": {"line": 7, "character": 24}}),
    );
    assert_eq!(
        def["result"]["range"]["start"],
        json!({"line": 5, "character": 0})
    );
    assert_eq!(
        def["result"]["range"]["end"],
        json!({"line": 5, "character": 2})
    );

    // References to `#1`: the definition and the use.
    let refs = lsp.request(
        "textDocument/references",
        json!({"textDocument": {"uri": URI}, "position": {"line": 5, "character": 1}, "context": {"includeDeclaration": true}}),
    );
    assert_eq!(refs["result"].as_array().unwrap().len(), 2);

    // Definition of the constructor `IFCAXIS2PLACEMENT3D` opens the schema document.
    let def = lsp.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": URI}, "position": {"line": 7, "character": 8}}),
    );
    let schema_uri = def["result"]["uri"].as_str().unwrap().to_string();
    assert_eq!(schema_uri, "ifc-schema:/IFC4_ADD2_TC1.exp");
    let source = lsp.request("ifc/schemaSource", json!({"uri": schema_uri}));
    let text = source["result"]["text"].as_str().unwrap().to_string();
    let line = def["result"]["range"]["start"]["line"].as_u64().unwrap() as usize;
    assert_eq!(
        text.lines().nth(line).unwrap().trim_end(),
        "ENTITY IfcAxis2Placement3D"
    );

    // Definition of the enum value `.LENGTHUNIT.` lands on the value inside IfcUnitEnum.
    let def = lsp.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": URI}, "position": {"line": 8, "character": 18}}),
    );
    assert_eq!(def["result"]["uri"].as_str().unwrap(), schema_uri);
    let range = &def["result"]["range"];
    let line = range["start"]["line"].as_u64().unwrap() as usize;
    let (s, e) = (
        range["start"]["character"].as_u64().unwrap() as usize,
        range["end"]["character"].as_u64().unwrap() as usize,
    );
    assert_eq!(&text.lines().nth(line).unwrap()[s..e], "LENGTHUNIT");
    let enum_decl = text
        .lines()
        .position(|l| l.starts_with("TYPE IfcUnitEnum"))
        .unwrap();
    assert!(
        line > enum_decl && line < enum_decl + 40,
        "{line} vs {enum_decl}"
    );

    // Hover on the enum value names its enumeration.
    let hover = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": URI}, "position": {"line": 8, "character": 18}}),
    );
    let md = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(
        md.contains("`.LENGTHUNIT.` is a value of **IfcUnitEnum**"),
        "{md}"
    );

    // Inside the schema document: hover and definition on `IfcDirection`.
    let decl_line = text
        .lines()
        .position(|l| l.trim_end() == "ENTITY IfcAxis2Placement3D")
        .unwrap();
    let attr_line = (decl_line + 1..decl_line + 6)
        .find(|&i| {
            text.lines()
                .nth(i)
                .unwrap()
                .contains("Axis : OPTIONAL IfcDirection")
        })
        .unwrap();
    let col = text
        .lines()
        .nth(attr_line)
        .unwrap()
        .find("IfcDirection")
        .unwrap()
        + 3;
    let hover = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": schema_uri}, "position": {"line": attr_line, "character": col}}),
    );
    let md = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(md.contains("### IfcDirection"), "{md}");
    assert!(
        md.contains("| 1 | DirectionRatios | LIST [2:3] OF IfcReal |"),
        "{md}"
    );
    let def = lsp.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": schema_uri}, "position": {"line": attr_line, "character": col}}),
    );
    let line = def["result"]["range"]["start"]["line"].as_u64().unwrap() as usize;
    assert_eq!(
        text.lines().nth(line).unwrap().trim_end(),
        "ENTITY IfcDirection"
    );

    // Fix the dangling reference with an incremental edit: `#99` -> `$`.
    lsp.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": URI, "version": 2},
            "contentChanges": [{"range": {"start": {"line": 7, "character": 29}, "end": {"line": 7, "character": 32}}, "text": "$"}]
        }),
    );
    let diags = lsp.wait_for_diagnostics(2);
    assert!(diags.is_empty(), "{diags:#?}");

    // Break a string: the next record must still resolve.
    lsp.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": URI, "version": 3},
            "contentChanges": [{"range": {"start": {"line": 5, "character": 21}, "end": {"line": 5, "character": 21}}, "text": "'"}]
        }),
    );
    let diags = lsp.wait_for_diagnostics(3);
    assert!(
        diags.iter().any(|d| d["code"] == json!("E006")),
        "{diags:#?}"
    );
    let def = lsp.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": URI}, "position": {"line": 7, "character": 27}}),
    );
    assert_eq!(
        def["result"]["range"]["start"],
        json!({"line": 6, "character": 0})
    );

    let shutdown = lsp.request("shutdown", Value::Null);
    assert!(shutdown.get("error").is_none());
    lsp.notify("exit", Value::Null);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = lsp.child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = lsp.child.kill();
            panic!("server did not exit after `exit`");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status}");
}

/// Without the VS Code-only `schemaDocuments: "virtual"` option, embedded schemas are
/// written to the cache directory and definitions point at those files.
#[test]
fn schema_definitions_use_cached_files_by_default() {
    let mut lsp = Lsp::start();
    lsp.request(
        "initialize",
        json!({"processId": null, "rootUri": null, "capabilities": {}}),
    );
    lsp.notify("initialized", json!({}));
    lsp.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": URI, "languageId": "ifc", "version": 1, "text": TEXT}}),
    );
    let def = lsp.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": URI}, "position": {"line": 7, "character": 8}}),
    );
    let uri = def["result"]["uri"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(uri.starts_with("file://"), "{def}");
    assert!(uri.ends_with("/IFC4_ADD2_TC1.exp"), "{uri}");
    let path = uri.trim_start_matches("file://");
    let path = path.replace("%20", " ");
    let path = if cfg!(windows) {
        path.trim_start_matches('/').replace("%3A", ":")
    } else {
        path
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let line = def["result"]["range"]["start"]["line"].as_u64().unwrap() as usize;
    assert_eq!(
        text.lines().nth(line).unwrap().trim_end(),
        "ENTITY IfcAxis2Placement3D"
    );

    // The cached file is itself a schema document: hover works inside it.
    let hover = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": 10}}),
    );
    let md = hover["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(md.contains("### IfcAxis2Placement3D"), "{hover}");

    lsp.request("shutdown", Value::Null);
    lsp.notify("exit", Value::Null);
    let _ = lsp.child.wait();
}
