use crate::ingress::RawTurn;
use axum::{
    body::Body,
    http::{Request, Response},
    middleware::Next,
};
use colored::*;
use std::panic;
use tracing::{error, info, warn, Span};
use tracing::{info_span, Instrument};
use uuid::Uuid;

pub const SHIM_TURN_ID_HEADER: &str = "x-shim-turn-id";

/// Sets up a global panic hook that logs panics using tracing and restores TUI.
pub fn setup_panic_hook() {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        // Capture backtrace if possible
        let backtrace = std::backtrace::Backtrace::capture();

        let payload = panic_info.payload();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "Unknown panic payload"
        };

        let location = panic_info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        error!(
            target: "panic",
            message = %message,
            location = %location,
            backtrace = %backtrace,
            "FATAL: Application panicked"
        );

        // If TUI is active, we should ideally restore the terminal.
        // Since we don't have direct access to the terminal here without more global state,
        // we rely on the TUI's own panic hook if it set one up, or we just let the original hook run.
        original_hook(panic_info);
    }));
}

pub async fn turn_id_middleware(mut req: Request<Body>, next: Next) -> Response<Body> {
    let turn_id = Uuid::new_v4().to_string();
    if let Ok(val) = turn_id.parse() {
        req.headers_mut().insert(SHIM_TURN_ID_HEADER, val);
    }

    let span = info_span!("request", turn_id = %turn_id);
    next.run(req).instrument(span).await
}

pub fn sanitize_response_body(body: &mut serde_json::Value) {
    let choices = match body.get_mut("choices").and_then(|c| c.as_array_mut()) {
        Some(c) => c,
        None => return,
    };

    for choice in choices {
        let _is_stop = choice.get("finish_reason").and_then(|f| f.as_str()) == Some("stop");

        let message = match choice.get_mut("message") {
            Some(m) => m,
            None => continue,
        };

        let tool_calls = message.get("tool_calls").and_then(|tc| tc.as_array());
        let has_tool_calls: bool = tool_calls.map(|tc| !tc.is_empty()).unwrap_or_default();

        if has_tool_calls {
            // Rule 1: Shut Up and Run
            message["content"] = serde_json::Value::Null;
            choice["finish_reason"] = serde_json::json!("tool_calls");
        }
    }
}

pub fn log_request_summary(payload: &serde_json::Value) {
    if let Ok(raw_turn) = serde_json::from_value::<RawTurn>(payload.clone()) {
        let msg_count = raw_turn.messages.len();
        let last_role = match raw_turn.messages.last().map(|m| format!("{:?}", m.role)) {
            Some(role) => role,
            None => "NONE".into(),
        };
        let is_prefill: bool = raw_turn
            .messages
            .last()
            .map(|m| m.role == Some(crate::types::Role::Assistant))
            .unwrap_or_default();

        info!(
            target: "flight_recorder",
            "[REQ] Messages: {} | Last Role: {} | Prefill: {}",
            msg_count, last_role, is_prefill
        );
    }
}

pub fn log_response_summary(response_body: &serde_json::Value) {
    let choices = response_body.get("choices").and_then(|c| c.as_array());
    if let Some(first_choice) = choices.and_then(|c| c.first()) {
        let finish_reason = first_choice
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN");
        let tool_calls = first_choice
            .get("message")
            .and_then(|m| m.get("tool_calls"))
            .and_then(|tc| tc.as_array());
        let tool_count = match tool_calls {
            Some(tc) => tc.len(),
            None => 0,
        };

        let content = first_choice
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str());
        let content_status = match content {
            None => "NULL",
            Some("") => "EMPTY",
            Some(s) => &format!("TEXT[{}]", s.len()),
        };

        if finish_reason == "stop" && tool_count > 0 {
            warn!(
                target: "flight_recorder",
                "{}", "[PROTOCOL MISMATCH] finish_reason='stop' but tool_calls present".bold().red()
            );
        }

        info!(
            target: "flight_recorder",
            "[RES] Finish: {} | Tools: {} | Content: {}",
            finish_reason, tool_count, content_status
        );
    }
}

#[derive(Default)]
pub struct StreamMetric {
    pub chunks: usize,
    pub tokens: usize,
    pub tool_parts: usize,
    pub text_chars: usize,
    pub tool_names: Vec<String>,
}

impl StreamMetric {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_chunk(&mut self, pulse: &crate::types::ProviderPulse) {
        self.chunks += 1;
        if let Some(usage) = &pulse.usage {
            self.tokens = usage.total_tokens as usize;
        }
        for choice in &pulse.choices {
            if let Some(content) = &choice.delta.content {
                self.text_chars += content.len();
            }
            if let Some(tools) = &choice.delta.tool_calls {
                self.tool_parts += tools.len();
                for t in tools {
                    if let Some(f) = &t.function {
                        if let Some(name) = &f.name {
                            if !name.is_empty() {
                                self.tool_names.push(name.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn log_summary(&self) {
        let turn_id = get_turn_id();
        let tools_str = if self.tool_names.is_empty() {
            format!("{}", self.tool_parts)
        } else {
            format!("{} ({})", self.tool_parts, self.tool_names.join(", "))
        };

        info!(
            target: "flight_recorder",
            "[STREAM END] TurnID: {} | Chunks: {} | Tools: {} | Text: {} chars",
            turn_id, self.chunks, tools_str, self.text_chars
        );
    }
}

pub fn get_turn_id() -> String {
    match Span::current().field("turn_id").map(|v| v.to_string()) {
        Some(id) => id,
        None => "unknown".to_string(),
    }
}
