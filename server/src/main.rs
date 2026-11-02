// The CLI subcommands are the only code allowed to write to stdout.
#![allow(clippy::print_stdout)]

use std::io::Write;
use std::time::Instant;

use ifc_lsp::index::Document;

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(
                std::io::stderr().lock(),
                "[ifc-lsp {}] {}",
                record.level(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

fn init_logging() {
    let level = match std::env::var("IFC_LSP_LOG").as_deref() {
        Ok("trace") => log::LevelFilter::Trace,
        Ok("debug") => log::LevelFilter::Debug,
        Ok("warn") => log::LevelFilter::Warn,
        Ok("error") => log::LevelFilter::Error,
        Ok("off") => log::LevelFilter::Off,
        _ => log::LevelFilter::Info,
    };
    let _ = log::set_logger(&StderrLogger);
    log::set_max_level(level);
}

fn main() -> anyhow::Result<()> {
    init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("ifc-lsp {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help" | "-h") => {
            println!("usage: ifc-lsp                 run the language server on stdio");
            println!("       ifc-lsp index-bench <f> time the entity index build for a file");
            println!("       ifc-lsp lint <f>        print diagnostics for a file");
            Ok(())
        }
        Some("index-bench") => index_bench(
            args.get(1)
                .ok_or_else(|| anyhow::anyhow!("index-bench needs a file"))?,
        ),
        Some("lint") => lint(
            args.get(1)
                .ok_or_else(|| anyhow::anyhow!("lint needs a file"))?,
        ),
        // vscode-languageclient passes `--stdio` for the stdio transport.
        None | Some("--stdio") => ifc_lsp::lsp::run_stdio(),
        Some(other) => anyhow::bail!("unknown argument {other:?}"),
    }
}

fn index_bench(path: &str) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let bytes = std::fs::read(path)?;
    let read_ms = t0.elapsed().as_secs_f64() * 1e3;
    let t1 = Instant::now();
    let doc = Document::new(bytes, 0);
    let index_ms = t1.elapsed().as_secs_f64() * 1e3;
    let mb = doc.text.len() as f64 / (1024.0 * 1024.0);
    println!("file:        {path}");
    println!(
        "size:        {mb:.1} MB, {} lines",
        doc.text.lines().line_count()
    );
    println!("schema:      {}", doc.schema_name.as_deref().unwrap_or("?"));
    println!(
        "records:     {} ({} entities, {} duplicate ids)",
        doc.entities.len(),
        doc.entity_count(),
        doc.duplicates.len()
    );
    println!("read:        {read_ms:.1} ms");
    println!(
        "index:       {index_ms:.1} ms ({:.0} MB/s)",
        mb / (index_ms / 1e3)
    );
    println!(
        "index heap:  {:.1} MB (text excluded)",
        doc.index_bytes() as f64 / (1024.0 * 1024.0)
    );
    let malformed = doc.entities.iter().filter(|r| r.is_malformed()).count();
    println!("malformed:   {malformed}");

    let t2 = Instant::now();
    let mut found = 0usize;
    let mut probes = 0usize;
    for r in doc.entities.iter().step_by(97) {
        if r.has_id() {
            probes += 1;
            if doc.lookup(r.id).is_some() {
                found += 1;
            }
        }
    }
    let lookup_ns = t2.elapsed().as_nanos() as f64 / probes.max(1) as f64;
    println!("lookup:      {probes} probes, {found} hits, {lookup_ns:.0} ns each");

    let t3 = Instant::now();
    let diags =
        ifc_lsp::validate::validate_all(&doc, &ifc_lsp::schema::SchemaSet::embedded(), None);
    let validate_ms = t3.elapsed().as_secs_f64() * 1e3;
    println!(
        "validate:    {validate_ms:.1} ms, {} diagnostics",
        diags.len()
    );
    Ok(())
}

fn lint(path: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    let doc = Document::new(bytes, 0);
    let schemas = ifc_lsp::schema::SchemaSet::embedded();
    let diags = ifc_lsp::validate::validate_all(&doc, &schemas, None);
    let out = std::io::stdout();
    let mut out = out.lock();
    for d in &diags {
        let p = doc.text.position(d.start);
        writeln!(
            out,
            "{}:{}:{}: {} {}: {}",
            path,
            p.line + 1,
            p.character + 1,
            d.severity.as_str(),
            d.code,
            d.message
        )?;
    }
    writeln!(out, "{} diagnostics", diags.len())?;
    Ok(())
}
