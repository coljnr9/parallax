use serde_json::{json, Value};
use std::io::Write;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

pub struct AgentNdjsonLayer<W: Write + Send + Sync + 'static> {
    writer: std::sync::Mutex<W>,
}

impl<W: Write + Send + Sync + 'static> AgentNdjsonLayer<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: std::sync::Mutex::new(writer),
        }
    }
}

impl<S, W> Layer<S> for AgentNdjsonLayer<W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: Write + Send + Sync + 'static,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let timestamp = chrono::Utc::now().to_rfc3339();
        let level = event.metadata().level().to_string();
        let target = event.metadata().target().to_string();

        let mut fields = json!({});
        let mut visitor = JsonVisitor(&mut fields);
        event.record(&mut visitor);

        let mut span_list = Vec::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                // Simplified: capture span name and IDs for now
                span_list.push(json!({
                    "name": span.name(),
                    "id": span.id().into_u64(),
                }));
            }
        }

        let trace_id = match span_list.first().and_then(|s| s.get("id")) {
            Some(id) => id.to_string(),
            None => "none".to_string(),
        };

        let output = json!({
            "timestamp": timestamp,
            "level": level,
            "target": target,
            "trace_id": trace_id,
            "span_list": span_list,
            "fields": fields,
        });

        if let Ok(mut w) = self.writer.lock() {
            let _ = writeln!(w, "{}", output);
        }
    }
}

struct JsonVisitor<'a>(&'a mut Value);

impl<'a> tracing::field::Visit for JsonVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0[field.name()] = json!(format!("{:?}", value));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0[field.name()] = json!(value);
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0[field.name()] = json!(value);
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0[field.name()] = json!(value);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0[field.name()] = json!(value);
    }
}
