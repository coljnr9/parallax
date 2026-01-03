use lazy_static::lazy_static;
use regex::Regex;
use std::io::Write;
use tracing::Subscriber;
use tracing_subscriber::layer::Layer;
use tracing_subscriber::registry::LookupSpan;

lazy_static! {
    static ref REDACTION_REGEX: Regex =
        match Regex::new(r"(?i)(sk-[A-Za-z0-9]{20,}|Bearer\s+[^\s]+|x-api-key:\s*[^\s]+)") {
            Ok(r) => r,
            Err(e) => panic!("Invalid redaction regex: {}", e),
        };
}

pub struct RedactingWriter<W: Write> {
    inner: W,
}

impl<W: Write> RedactingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let input = String::from_utf8_lossy(buf);
        let redacted = REDACTION_REGEX.replace_all(&input, "[REDACTED]");
        self.inner.write_all(redacted.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub struct RedactionLayer;

impl<S> Layer<S> for RedactionLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    // This layer is primarily a marker or for future structured redaction.
    // The actual redaction happens in RedactingWriter for raw text outputs.
}
