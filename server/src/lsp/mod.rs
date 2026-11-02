//! LSP front end over stdio: one main loop thread, one analysis thread.

mod analysis;
mod hover;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::Notification as _;
use lsp_types::request::Request as _;
use lsp_types::{
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
    HoverParams, HoverProviderCapability, InitializeParams, InitializeResult, Location,
    MarkupContent, MarkupKind, OneOf, Position, Range, ReferenceParams, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use rustc_hash::FxHashMap;

use crate::index::Document;
use crate::schema::{Schema, SchemaSet};
use crate::text::Offset;
use analysis::{Analysis, DocState, Settings};

const DEBOUNCE: Duration = Duration::from_millis(300);

/// URI scheme under which embedded schemas are served as read-only documents.
pub const SCHEMA_SCHEME: &str = "ifc-schema";
/// Custom request: `{ "uri": "ifc-schema:/IFC4_ADD2_TC1.exp" }` -> `{ "text": "..." }`.
pub const SCHEMA_SOURCE_REQUEST: &str = "ifc/schemaSource";

#[allow(clippy::mutable_key_type)]
struct Server {
    sender: crossbeam_channel::Sender<Message>,
    docs: FxHashMap<Uri, Arc<DocState>>,
    settings: Arc<RwLock<Settings>>,
    analysis: Option<Analysis>,
    extra_schema_dirs: Vec<PathBuf>,
    /// Serve embedded schemas as `ifc-schema:` documents (VS Code) instead of cached files.
    virtual_schema_docs: bool,
}

pub fn run_stdio() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        ..Default::default()
    };
    let (id, params) = connection.initialize_start()?;
    let init: InitializeParams = serde_json::from_value(params).context("initialize params")?;
    let result = InitializeResult {
        capabilities,
        server_info: Some(ServerInfo {
            name: "ifc-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;

    let settings = Arc::new(RwLock::new(Settings {
        schemas: Arc::new(build_schema_set(&[], false)),
        max_problems: 1000,
    }));
    let mut server = Server {
        sender: connection.sender.clone(),
        docs: FxHashMap::default(),
        settings: settings.clone(),
        analysis: Some(Analysis::start(connection.sender.clone(), settings)),
        extra_schema_dirs: Vec::new(),
        virtual_schema_docs: false,
    };
    if let Some(opts) = init.initialization_options {
        server.apply_settings(&opts);
    }
    log::info!("ifc-lsp {} ready", env!("CARGO_PKG_VERSION"));

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    break;
                }
                server.handle_request(req);
            }
            Message::Notification(n) => server.handle_notification(n),
            Message::Response(_) => {}
        }
    }
    if let Some(a) = server.analysis.take() {
        a.shutdown();
    }
    // The writer thread ends only once every sender clone is gone.
    drop(server);
    drop(connection);
    io_threads.join()?;
    Ok(())
}

/// Document URI for a schema: the file it was loaded from, or a virtual document
/// served through [`SCHEMA_SOURCE_REQUEST`] for embedded schemas.
pub fn schema_uri(schema: &Schema) -> Option<Uri> {
    let s = match &schema.source_path {
        Some(path) => file_uri(path)?,
        None => format!("{SCHEMA_SCHEME}:/{}.exp", schema.key),
    };
    Uri::from_str(&s).ok()
}

fn file_uri(path: &Path) -> Option<String> {
    let abs = std::path::absolute(path).ok()?;
    let text = abs.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !text.starts_with('/') {
        out.push('/');
    }
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("%{b:02X}"));
            }
        }
    }
    Some(out)
}

fn is_schema_uri(uri: &Uri) -> bool {
    uri.as_str().starts_with(&format!("{SCHEMA_SCHEME}:"))
}

fn build_schema_set(dirs: &[PathBuf], virtual_docs: bool) -> SchemaSet {
    let cache = if virtual_docs {
        None
    } else {
        SchemaSet::default_cache_dir()
    };
    let mut set = SchemaSet::embedded().with_cache_dir(cache);
    for d in dirs {
        match set.add_directory(d) {
            Ok(names) => log::info!("loaded schemas {names:?} from {}", d.display()),
            Err(e) => log::warn!("cannot read schema directory {}: {e}", d.display()),
        }
    }
    set
}

/// Local path of a `file:` URI, percent-decoded.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.split(['?', '#']).next()?;
    let raw = rest.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let decoded = (raw[i] == b'%' && i + 2 < raw.len())
            .then(|| std::str::from_utf8(&raw[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match decoded {
            Some(v) => {
                bytes.push(v);
                i += 3;
            }
            None => {
                bytes.push(raw[i]);
                i += 1;
            }
        }
    }
    let mut text = String::from_utf8(bytes).ok()?;
    if cfg!(windows) && text.len() >= 3 && text.as_bytes()[0] == b'/' && text.as_bytes()[2] == b':'
    {
        text.remove(0);
    }
    Some(PathBuf::from(text))
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => {
            let norm = |p: &Path| {
                let s = p.to_string_lossy().replace('\\', "/");
                if cfg!(windows) {
                    s.to_ascii_lowercase()
                } else {
                    s
                }
            };
            norm(a) == norm(b)
        }
    }
}

impl Server {
    fn analysis(&self) -> &Analysis {
        self.analysis.as_ref().expect("analysis thread")
    }

    fn apply_settings(&mut self, value: &serde_json::Value) {
        // Accept both `{ ifc: {...} }` (didChangeConfiguration) and the bare object.
        let root = value.get("ifc").unwrap_or(value);
        let dirs: Vec<PathBuf> = root
            .get("extraSchemaDirectories")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        let max_problems = root
            .get("diagnostics")
            .and_then(|d| d.get("maxProblems"))
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(1, 100_000) as usize);
        let virtual_docs = root
            .get("schemaDocuments")
            .and_then(|v| v.as_str())
            .map(|v| v.eq_ignore_ascii_case("virtual"))
            .unwrap_or(self.virtual_schema_docs);

        let mut s = self.settings.write().unwrap();
        if let Some(m) = max_problems {
            s.max_problems = m;
        }
        if dirs != self.extra_schema_dirs || virtual_docs != self.virtual_schema_docs {
            s.schemas = Arc::new(build_schema_set(&dirs, virtual_docs));
            self.extra_schema_dirs = dirs;
            self.virtual_schema_docs = virtual_docs;
        }
        drop(s);
        for (uri, state) in &self.docs {
            state
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            self.analysis()
                .schedule(uri.clone(), state.clone(), Duration::ZERO, 0);
        }
    }

    fn schemas(&self) -> Arc<SchemaSet> {
        self.settings.read().unwrap().schemas.clone()
    }

    /// The schema a schema-document URI refers to (virtual or on-disk).
    fn schema_for_uri(&self, uri: &Uri) -> Option<Arc<Schema>> {
        let schemas = self.schemas();
        let text = uri.as_str();
        if let Some(rest) = text.strip_prefix(&format!("{SCHEMA_SCHEME}:")) {
            let key = rest.trim_start_matches('/').trim_end_matches(".exp");
            return schemas.by_name(key);
        }
        let path = uri_to_path(text)?;
        schemas
            .names()
            .into_iter()
            .filter_map(|n| schemas.by_name(&n))
            .find(|s| {
                s.source_path
                    .as_deref()
                    .is_some_and(|p| same_path(p, &path))
            })
    }

    fn handle_notification(&mut self, n: Notification) {
        match n.method.as_str() {
            lsp_types::notification::DidOpenTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(n.params) else {
                    return;
                };
                if is_schema_uri(&p.text_document.uri)
                    || p.text_document.language_id.eq_ignore_ascii_case("express")
                    || p.text_document
                        .uri
                        .as_str()
                        .to_ascii_lowercase()
                        .ends_with(".exp")
                {
                    return;
                }
                let t0 = Instant::now();
                let doc = Document::new(p.text_document.text.into_bytes(), p.text_document.version);
                log::debug!(
                    "opened {} ({} bytes, {} records) in {:.1} ms",
                    p.text_document.uri.as_str(),
                    doc.text.len(),
                    doc.entities.len(),
                    t0.elapsed().as_secs_f64() * 1e3
                );
                let state = DocState::new(doc);
                self.docs.insert(p.text_document.uri.clone(), state.clone());
                self.analysis()
                    .schedule(p.text_document.uri, state, Duration::ZERO, 0);
            }
            lsp_types::notification::DidChangeTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(n.params) else {
                    return;
                };
                let Some(state) = self.docs.get(&p.text_document.uri).cloned() else {
                    return;
                };
                state
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let mut focus = 0;
                {
                    let mut doc = state.doc.write().unwrap();
                    let t0 = Instant::now();
                    for change in p.content_changes {
                        match change.range {
                            Some(range) => {
                                let start = doc.text.offset(range.start);
                                let end = doc.text.offset(range.end).max(start);
                                doc.apply_edit(start, end, change.text.as_bytes());
                                focus = start;
                            }
                            None => {
                                *doc = Document::new(
                                    change.text.into_bytes(),
                                    p.text_document.version,
                                );
                                focus = 0;
                            }
                        }
                    }
                    doc.version = p.text_document.version;
                    log::trace!(
                        "change applied in {:.2} ms",
                        t0.elapsed().as_secs_f64() * 1e3
                    );
                }
                self.analysis()
                    .schedule(p.text_document.uri, state, DEBOUNCE, focus);
            }
            lsp_types::notification::DidCloseTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(n.params) else {
                    return;
                };
                let Some(state) = self.docs.remove(&p.text_document.uri) else {
                    return;
                };
                state
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                self.analysis().forget(p.text_document.uri.clone());
                analysis::clear_diagnostics(&p.text_document.uri, &self.sender);
            }
            lsp_types::notification::DidChangeConfiguration::METHOD => {
                let Ok(p) = serde_json::from_value::<DidChangeConfigurationParams>(n.params) else {
                    return;
                };
                self.apply_settings(&p.settings);
            }
            _ => {}
        }
    }

    fn handle_request(&mut self, req: Request) {
        let id = req.id.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.dispatch(req)));
        let response = match result {
            Ok(Ok(value)) => Response::new_ok(id, value),
            Ok(Err(e)) => Response::new_err(id, ErrorCode::InternalError as i32, e.to_string()),
            Err(_) => Response::new_err(
                id,
                ErrorCode::InternalError as i32,
                "ifc-lsp: handler panicked".into(),
            ),
        };
        let _ = self.sender.send(Message::Response(response));
    }

    fn dispatch(&self, req: Request) -> anyhow::Result<serde_json::Value> {
        match req.method.as_str() {
            lsp_types::request::HoverRequest::METHOD => {
                let p: HoverParams = serde_json::from_value(req.params)?;
                let uri = &p.text_document_position_params.text_document.uri;
                let pos = p.text_document_position_params.position;
                let hover = if let Some(schema) = self.schema_for_uri(uri) {
                    schema_hover(&schema, pos)
                } else {
                    self.hover(uri, pos)
                };
                Ok(serde_json::to_value(hover)?)
            }
            lsp_types::request::GotoDefinition::METHOD => {
                let p: GotoDefinitionParams = serde_json::from_value(req.params)?;
                let uri = &p.text_document_position_params.text_document.uri;
                let pos = p.text_document_position_params.position;
                let location = if let Some(schema) = self.schema_for_uri(uri) {
                    schema_definition(&schema, uri, pos)
                } else {
                    self.definition(uri, pos)
                };
                Ok(serde_json::to_value(
                    location.map(GotoDefinitionResponse::Scalar),
                )?)
            }
            lsp_types::request::References::METHOD => {
                let p: ReferenceParams = serde_json::from_value(req.params)?;
                Ok(serde_json::to_value(self.references(p))?)
            }
            SCHEMA_SOURCE_REQUEST => {
                let uri_text = req
                    .params
                    .get("uri")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("ifc/schemaSource needs a uri"))?;
                let uri = Uri::from_str(uri_text).map_err(|e| anyhow::anyhow!("bad uri: {e}"))?;
                let text = self.schema_for_uri(&uri).map(|s| s.source().to_string());
                Ok(serde_json::json!({ "text": text }))
            }
            _ => Ok(serde_json::Value::Null),
        }
    }

    fn with_doc<T>(&self, uri: &Uri, f: impl FnOnce(&Document) -> T) -> Option<T> {
        let state = self.docs.get(uri)?;
        let doc = state.doc.read().unwrap();
        Some(f(&doc))
    }

    fn hover(&self, uri: &Uri, pos: Position) -> Option<Hover> {
        let schemas = self.schemas();
        self.with_doc(uri, |doc| {
            let offset = doc.text.offset(pos);
            let schema = doc
                .schema_name
                .as_deref()
                .and_then(|n| schemas.for_file_schema(n));
            let schema = schema.as_deref();
            let target = hover::target_at(doc, offset)?;
            let (value, start, end) = match target {
                hover::HoverTarget::Ref { id, start, end } => {
                    (hover::render_ref(doc, id, schema), start, end)
                }
                hover::HoverTarget::Entity { record, start, end } => {
                    (hover::render_entity(doc, record, schema), start, end)
                }
                hover::HoverTarget::TypeName { start, end } => {
                    (hover::render_type(doc, start, end, schema)?, start, end)
                }
                hover::HoverTarget::EnumValue { record, start, end } => (
                    hover::render_enum_value(doc, record, start, end, schema)?,
                    start,
                    end,
                ),
            };
            Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: Some(doc.range(start, end)),
            })
        })?
    }

    fn definition(&self, uri: &Uri, pos: Position) -> Option<Location> {
        let schemas = self.schemas();
        self.with_doc(uri, |doc| {
            let offset = doc.text.offset(pos);
            let target = hover::target_at(doc, offset)?;
            if let hover::HoverTarget::Ref { id, .. } = target {
                let rec = doc.record(id)?;
                let id_len = 1 + (id.checked_ilog10().unwrap_or(0) + 1);
                return Some(Location {
                    uri: uri.clone(),
                    range: doc.range(rec.start, rec.start + id_len),
                });
            }
            let schema = schemas.for_file_schema(doc.schema_name.as_deref()?)?;
            let range = match target {
                hover::HoverTarget::Entity { start, end, .. } => {
                    let e = schema.entity_bytes(doc.text.slice(start, end))?;
                    schema.entity_decl_range(e)
                }
                hover::HoverTarget::TypeName { start, end } => {
                    let name = std::str::from_utf8(doc.text.slice(start, end)).ok()?;
                    schema.type_decl_range(schema.type_id(name)?)
                }
                hover::HoverTarget::EnumValue { record, start, end } => {
                    let t = hover::enum_type_at(doc, record, start, &schema)?;
                    schema.enum_value_range(t, &hover::enum_value_text(doc, start, end))
                }
                hover::HoverTarget::Ref { .. } => return None,
            };
            Some(Location {
                uri: schema_uri(&schema)?,
                range,
            })
        })?
    }

    fn references(&self, p: ReferenceParams) -> Option<Vec<Location>> {
        let uri = &p.text_document_position.text_document.uri;
        let pos = p.text_document_position.position;
        self.with_doc(uri, |doc| {
            let offset = doc.text.offset(pos);
            let (id, _, _) = doc.ref_at(offset)?;
            let def_start = doc.record(id).map(|r| r.start);
            let t0 = Instant::now();
            let mut out = Vec::new();
            for (start, end) in find_refs(doc, id) {
                let is_def = def_start == Some(start);
                if is_def && !p.context.include_declaration {
                    continue;
                }
                out.push(Location {
                    uri: uri.clone(),
                    range: doc.range(start, end),
                });
            }
            log::debug!(
                "references for #{id}: {} in {:.1} ms",
                out.len(),
                t0.elapsed().as_secs_f64() * 1e3
            );
            Some(out)
        })?
    }
}

fn schema_word(schema: &Schema, pos: Position) -> Option<(&str, Range)> {
    let (s, e) = schema.word_at(pos)?;
    Some((&schema.source()[s..e], schema.span_range(s, e - s)))
}

fn schema_hover(schema: &Schema, pos: Position) -> Option<Hover> {
    let (word, range) = schema_word(schema, pos)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover::render_schema_word(schema, word)?,
        }),
        range: Some(range),
    })
}

fn schema_definition(schema: &Schema, uri: &Uri, pos: Position) -> Option<Location> {
    let (word, _) = schema_word(schema, pos)?;
    let range = if let Some(e) = schema.entity(word) {
        schema.entity_decl_range(e)
    } else {
        schema.type_decl_range(schema.type_id(word)?)
    };
    Some(Location {
        uri: uri.clone(),
        range,
    })
}

/// Every `#id` token in the document, as byte spans.
fn find_refs(doc: &Document, id: u32) -> Vec<(Offset, Offset)> {
    let bytes = doc.text.bytes();
    let needle = format!("#{id}");
    let nb = needle.as_bytes();
    let mut out = Vec::new();
    for pos in memchr::memmem::find_iter(bytes, nb) {
        let end = pos + nb.len();
        if bytes.get(end).is_some_and(|b| b.is_ascii_digit()) {
            continue;
        }
        out.push((pos as Offset, end as Offset));
    }
    out
}
