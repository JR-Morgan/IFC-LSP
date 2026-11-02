//! Background validation: debounced per document, cancellable, publishes
//! diagnostics with the version they were computed for.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, NumberOrString, PublishDiagnosticsParams, Uri,
    notification::Notification as _,
};

use crate::index::Document;
use crate::schema::SchemaSet;
use crate::text::Offset;
use crate::validate::{Severity, Validator};

pub struct DocState {
    pub doc: RwLock<Document>,
    /// Set by the main thread on every change so an in-flight validation stops.
    pub cancel: AtomicBool,
}

impl DocState {
    pub fn new(doc: Document) -> Arc<Self> {
        Arc::new(Self {
            doc: RwLock::new(doc),
            cancel: AtomicBool::new(false),
        })
    }
}

pub enum Job {
    Validate {
        uri: Uri,
        state: Arc<DocState>,
        due: Instant,
        focus: Offset,
    },
    Forget(Uri),
    Shutdown,
}

#[derive(Clone)]
pub struct Settings {
    pub schemas: Arc<SchemaSet>,
    pub max_problems: usize,
}

pub struct Analysis {
    tx: Sender<Job>,
    handle: Option<JoinHandle<()>>,
}

impl Analysis {
    pub fn start(out: Sender<lsp_server::Message>, settings: Arc<RwLock<Settings>>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let handle = std::thread::Builder::new()
            .name("ifc-analysis".into())
            .spawn(move || worker(rx, out, settings))
            .expect("spawn analysis thread");
        Self {
            tx,
            handle: Some(handle),
        }
    }

    pub fn schedule(&self, uri: Uri, state: Arc<DocState>, delay: Duration, focus: Offset) {
        let _ = self.tx.send(Job::Validate {
            uri,
            state,
            due: Instant::now() + delay,
            focus,
        });
    }

    pub fn forget(&self, uri: Uri) {
        let _ = self.tx.send(Job::Forget(uri));
    }

    pub fn shutdown(mut self) {
        let _ = self.tx.send(Job::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct Pending {
    state: Arc<DocState>,
    due: Instant,
    focus: Offset,
}

fn worker(rx: Receiver<Job>, out: Sender<lsp_server::Message>, settings: Arc<RwLock<Settings>>) {
    #[allow(clippy::mutable_key_type)]
    let mut pending: HashMap<Uri, Pending> = HashMap::new();
    loop {
        let next_due = pending.values().map(|p| p.due).min();
        let job = match next_due {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(due) => rx.recv_timeout(due.saturating_duration_since(Instant::now())),
        };
        match job {
            Ok(Job::Validate {
                uri,
                state,
                due,
                focus,
            }) => {
                pending.insert(uri, Pending { state, due, focus });
            }
            Ok(Job::Forget(uri)) => {
                pending.remove(&uri);
            }
            Ok(Job::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        let ready: Vec<Uri> = pending
            .iter()
            .filter(|(_, p)| p.due <= now)
            .map(|(u, _)| u.clone())
            .collect();
        for uri in ready {
            let Some(p) = pending.remove(&uri) else {
                continue;
            };
            let settings = settings.read().unwrap().clone();
            run_validation(&uri, &p.state, p.focus, &settings, &out);
        }
    }
}

const PARTIAL_PUBLISH_EVERY: Duration = Duration::from_millis(250);

fn run_validation(
    uri: &Uri,
    state: &DocState,
    focus: Offset,
    settings: &Settings,
    out: &Sender<lsp_server::Message>,
) {
    state.cancel.store(false, Ordering::Relaxed);
    let doc = state.doc.read().unwrap();
    let version = doc.version;
    let started = Instant::now();
    let mut validator = Validator::new(&doc, &settings.schemas);
    let mut found = Vec::new();
    validator.document_checks(&mut found);

    let n = doc.entities.len();
    let start = doc.first_entity_from(focus).min(n);
    let order = (start..n).chain(0..start);
    let mut last_publish = Instant::now();
    let mut published_partial = false;
    for (i, idx) in order.enumerate() {
        if i % 64 == 0 {
            if state.cancel.load(Ordering::Relaxed) {
                log::debug!(
                    "validation of {} v{version} cancelled after {i} entities",
                    uri.as_str()
                );
                return;
            }
            if i > 0 && last_publish.elapsed() >= PARTIAL_PUBLISH_EVERY {
                publish(uri, &doc, version, &found, settings, out, Some(n - i));
                last_publish = Instant::now();
                published_partial = true;
            }
        }
        validator.entity(idx, &mut found);
    }
    log::debug!(
        "validated {} v{version}: {} records, {} diagnostics in {:.1} ms{}",
        uri.as_str(),
        n,
        found.len(),
        started.elapsed().as_secs_f64() * 1e3,
        if published_partial {
            " (with partial publishes)"
        } else {
            ""
        }
    );
    publish(uri, &doc, version, &found, settings, out, None);
}

fn publish(
    uri: &Uri,
    doc: &Document,
    version: i32,
    found: &[crate::validate::Diagnostic],
    settings: &Settings,
    out: &Sender<lsp_server::Message>,
    remaining: Option<usize>,
) {
    let cap = settings.max_problems.max(1);
    let mut diagnostics: Vec<Diagnostic> = found.iter().take(cap).map(|d| to_lsp(doc, d)).collect();
    if found.len() > cap {
        diagnostics.push(Diagnostic {
            range: doc.range(0, 0),
            severity: Some(DiagnosticSeverity::INFORMATION),
            code: Some(NumberOrString::String("I001".into())),
            source: Some("ifc".into()),
            message: format!(
                "{} more problems not shown (ifc.diagnostics.maxProblems = {cap})",
                found.len() - cap
            ),
            ..Default::default()
        });
    }
    if let Some(left) = remaining {
        diagnostics.push(Diagnostic {
            range: doc.range(0, 0),
            severity: Some(DiagnosticSeverity::INFORMATION),
            code: Some(NumberOrString::String("I002".into())),
            source: Some("ifc".into()),
            message: format!("validation in progress, {left} records left"),
            ..Default::default()
        });
    }
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: Some(version),
    };
    let n = lsp_server::Notification::new(
        lsp_types::notification::PublishDiagnostics::METHOD.to_string(),
        params,
    );
    let _ = out.send(lsp_server::Message::Notification(n));
}

pub fn clear_diagnostics(uri: &Uri, out: &Sender<lsp_server::Message>) {
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: Vec::new(),
        version: None,
    };
    let n = lsp_server::Notification::new(
        lsp_types::notification::PublishDiagnostics::METHOD.to_string(),
        params,
    );
    let _ = out.send(lsp_server::Message::Notification(n));
}

fn to_lsp(doc: &Document, d: &crate::validate::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: doc.range(d.start, d.end),
        severity: Some(match d.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Information => DiagnosticSeverity::INFORMATION,
        }),
        code: Some(NumberOrString::String(d.code.to_string())),
        source: Some("ifc".into()),
        message: d.message.clone(),
        ..Default::default()
    }
}

impl Document {
    pub fn range(&self, start: Offset, end: Offset) -> lsp_types::Range {
        self.text.range(start, end)
    }
}
