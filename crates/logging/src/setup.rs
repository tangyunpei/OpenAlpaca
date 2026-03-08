use std::io;
use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::redact::redact_sensitive;

/// Configuration for logging initialization.
pub struct LogConfig {
    pub level: String,
    pub file: Option<String>,
    pub console_style: ConsoleStyle,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum ConsoleStyle {
    #[default]
    Pretty,
    Json,
    Compact,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            file: None,
            console_style: ConsoleStyle::default(),
        }
    }
}

fn build_console_layer<S>(style: ConsoleStyle) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let writer = RedactingWriter::new(io::stderr);
    match style {
        ConsoleStyle::Pretty => tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_target(true)
            .with_thread_ids(false)
            .with_ansi(true)
            .pretty()
            .boxed(),
        ConsoleStyle::Compact => tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_target(true)
            .with_thread_ids(false)
            .with_ansi(true)
            .compact()
            .boxed(),
        ConsoleStyle::Json => tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_target(true)
            .with_thread_ids(false)
            .with_ansi(false)
            .json()
            .boxed(),
    }
}

/// Initialize tracing with console output and optional file output.
/// Returns guards that must be held for the lifetime of the application
/// to ensure logs are flushed.
pub fn init_logging(config: &LogConfig) -> Vec<WorkerGuard> {
    let mut guards = Vec::new();

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    let console_layer = build_console_layer(config.console_style);

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer);

    if let Some(ref file_path) = config.file {
        let dir = Path::new(file_path).parent().unwrap_or(Path::new("."));
        let filename = Path::new(file_path)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("openalpaca.log");

        let file_appender = tracing_appender::rolling::never(dir, filename);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        guards.push(guard);

        let file_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(RedactingWriter::new(non_blocking))
            .with_target(true)
            .with_ansi(false);

        registry.with(file_layer).init();
    } else {
        registry.init();
    }

    guards
}

/// A `MakeWriter` wrapper that redacts sensitive content from log output.
#[derive(Clone)]
struct RedactingWriter<W> {
    inner: W,
}

impl<W> RedactingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<'a, W: MakeWriter<'a>> MakeWriter<'a> for RedactingWriter<W> {
    type Writer = RedactingWriterInstance<W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriterInstance {
            inner: self.inner.make_writer(),
        }
    }
}

struct RedactingWriterInstance<W> {
    inner: W,
}

impl<W: io::Write> io::Write for RedactingWriterInstance<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        let redacted = redact_sensitive(&s);
        self.inner.write_all(redacted.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
