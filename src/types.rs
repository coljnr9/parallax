use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use tracing_error::SpanTrace;
use uuid::Uuid;

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ToolCallId(pub String);

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct UserId(pub String);

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ConversationId(pub String);

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TurnId(pub Uuid);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct LatencyMs(pub u128);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, PartialOrd)]
pub struct CostUsd(pub f64);

impl fmt::Display for CostUsd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.4}", self.0)
    }
}

impl fmt::Display for LatencyMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ConversationId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl ConversationId {
    pub fn short(&self) -> &str {
        crate::str_utils::prefix_chars(&self.0, 6)
    }
}

impl RequestId {
    pub fn short(&self) -> &str {
        crate::str_utils::prefix_chars(&self.0, 8)
    }
}

use std::sync::atomic::{AtomicU32, AtomicU64};
use std::time::Instant;

pub struct UpstreamHealth {
    pub consecutive_failures: AtomicU32,
    pub total_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub last_success: std::sync::RwLock<Option<Instant>>,
    pub last_failure: std::sync::RwLock<Option<Instant>>,
}

impl Default for UpstreamHealth {
    fn default() -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            total_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            last_success: std::sync::RwLock::new(None),
            last_failure: std::sync::RwLock::new(None),
        }
    }
}

impl UpstreamHealth {
    pub fn record_success(&self) {
        self.total_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.consecutive_failures
            .store(0, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut last) = self.last_success.write() {
            *last = Some(Instant::now());
        }
    }

    pub fn record_failure(&self) {
        self.total_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.failed_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.consecutive_failures
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut last) = self.last_failure.write() {
            *last = Some(Instant::now());
        }
    }
}

#[derive(Error, Debug)]
pub enum ParallaxError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid ingress payload: {0}")]
    InvalidIngress(String),

    #[error("Identification failed: {0}")]
    Identification(String),

    #[error("Upstream error (status {0}): {1}")]
    Upstream(axum::http::StatusCode, String),

    #[error("Internal error: {0}")]
    Internal(String, SpanTrace),

    #[allow(dead_code)]
    #[error("Protocol error: {0}")]
    Protocol(String),
}

impl axum::response::IntoResponse for ObservedError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg, code) = match &self.inner {
            ParallaxError::Upstream(s, m) => (*s, m.clone(), "UPSTREAM_ERROR"),
            ParallaxError::InvalidIngress(m) => (
                axum::http::StatusCode::BAD_REQUEST,
                m.clone(),
                "INVALID_INGRESS",
            ),
            ParallaxError::Identification(m) => (
                axum::http::StatusCode::BAD_REQUEST,
                m.clone(),
                "IDENTIFICATION_ERROR",
            ),
            ParallaxError::Network(e) => (
                axum::http::StatusCode::BAD_GATEWAY,
                e.to_string(),
                "NETWORK_ERROR",
            ),
            ParallaxError::Database(e) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
                "DATABASE_ERROR",
            ),
            ParallaxError::Serialization(e) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
                "SERIALIZATION_ERROR",
            ),
            ParallaxError::Io(e) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
                "IO_ERROR",
            ),
            ParallaxError::Internal(m, _) => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                m.clone(),
                "INTERNAL_ERROR",
            ),
            ParallaxError::Protocol(m) => (
                axum::http::StatusCode::BAD_REQUEST,
                m.clone(),
                "PROTOCOL_ERROR",
            ),
        };
        (
            status,
            axum::Json(serde_json::json!({
                "error": msg,
                "code": code,
                "span_trace": self.span_trace.to_string(),
            })),
        )
            .into_response()
    }
}

#[derive(Debug)]
pub struct ObservedError {
    pub inner: ParallaxError,
    pub span_trace: SpanTrace,
}

impl std::fmt::Display for ObservedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\n\nSpan Trace:\n{}", self.inner, self.span_trace)
    }
}

impl std::error::Error for ObservedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.inner)
    }
}

impl<E> From<E> for ObservedError
where
    E: Into<ParallaxError>,
{
    fn from(error: E) -> Self {
        Self {
            inner: error.into(),
            span_trace: SpanTrace::capture(),
        }
    }
}

pub type Result<T> = std::result::Result<T, ObservedError>;

impl ToolCallId {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self(format!("call_{}", Uuid::new_v4().simple()))
    }
}

impl Default for ToolCallId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<String> for ToolCallId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<String> for UserId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<Uuid> for TurnId {
    fn from(u: Uuid) -> Self {
        Self(u)
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatTurn {
    User(String),
    AssistantThinking(String),
    AssistantToolCall(Vec<ToolCallInfo>),
    ToolResult { id: ToolCallId, content: String },
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: String,
}

/// Valid role transitions for conversation history
/// Format: (previous_role, current_role)
const VALID_ROLE_TRANSITIONS: &[(Role, Role)] = &[
    // Initial messages can be System, Developer, or User
    // (None is handled separately)
    // System/Developer can transition to User or System/Developer
    (Role::System, Role::User),
    (Role::System, Role::System),
    (Role::System, Role::Developer),
    (Role::Developer, Role::User),
    (Role::Developer, Role::System),
    (Role::Developer, Role::Developer),
    // User transitions to Assistant
    (Role::User, Role::Assistant),
    // Assistant transitions to User or Tool
    (Role::Assistant, Role::User),
    (Role::Assistant, Role::Tool),
    // Tool transitions to Tool or Assistant
    (Role::Tool, Role::Tool),
    (Role::Tool, Role::Assistant),
];

#[allow(dead_code)]
pub fn validate_history(history: &[TurnRecord]) -> Result<()> {
    if history.is_empty() {
        return Ok(());
    }

    let mut last_role: Option<Role> = None;

    for (i, turn) in history.iter().enumerate() {
        // Check if this is a valid transition
        let is_valid = match &last_role {
            None => {
                // First message must be System, Developer, or User
                matches!(turn.role, Role::User | Role::System | Role::Developer)
            }
            Some(prev) => {
                // Check if transition is in the valid list
                VALID_ROLE_TRANSITIONS
                    .iter()
                    .any(|(p, c)| p == prev && c == &turn.role)
            }
        };

        if !is_valid {
            let prev_display = match &last_role {
                Some(r) => format!("{:?}", r),
                None => "None".to_string(),
            };
            tracing::warn!(
                "Invalid role transition detected: {} -> {:?}",
                prev_display,
                turn.role
            );
            return Err(ParallaxError::Protocol(format!(
                "Invalid role transition at message {}: {} -> {:?}",
                i, prev_display, turn.role
            ))
            .into());
        }
        last_role = Some(turn.role.clone());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broken_history_alternation() {
        let history = vec![
            TurnRecord {
                role: Role::User,
                content: vec![MessagePart::Text {
                    content: "Hi".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
            TurnRecord {
                role: Role::User,
                content: vec![MessagePart::Text {
                    content: "Still there?".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
        ];

        let result = validate_history(&history);
        assert!(result.is_err());
        if let Err(err) = result {
            if let ParallaxError::Protocol(msg) = err.inner {
                assert!(msg.contains("Invalid role transition"));
            } else {
                panic!("Expected Protocol error, got {:?}", err.inner);
            }
        } else {
            panic!("Expected error result");
        }
    }

    #[test]
    fn test_valid_history() {
        let history = vec![
            TurnRecord {
                role: Role::System,
                content: vec![MessagePart::Text {
                    content: "Sys".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
            TurnRecord {
                role: Role::User,
                content: vec![MessagePart::Text {
                    content: "User".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
            TurnRecord {
                role: Role::Assistant,
                content: vec![MessagePart::Text {
                    content: "Asst".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
        ];

        assert!(validate_history(&history).is_ok());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostModel {
    pub prompt: f64,
    pub completion: f64,
    pub image: f64,
    pub request: f64,
    pub prompt_cache_read: f64,
    pub prompt_cache_write: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTokensDetails {
    pub cached_tokens: Option<u32>,
}

/// --- CORE ROLES ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
    Developer,
    Model,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConversationIdSource {
    #[serde(rename = "cursor_header")]
    CursorHeader,
    #[serde(rename = "cursor_metadata")]
    CursorMetadata,
    #[serde(rename = "anchor_hash")]
    AnchorHash,
    #[serde(rename = "unknown")]
    Unknown,
}

impl fmt::Display for ConversationIdSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CursorHeader => write!(f, "header"),
            Self::CursorMetadata => write!(f, "metadata"),
            Self::AnchorHash => write!(f, "hash"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// --- THE RICH HUB (Internal Representation) ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationContext {
    pub history: Vec<TurnRecord>,
    pub conversation_id: String,
    #[serde(default = "default_cid_source")]
    pub conversation_id_source: ConversationIdSource,
    #[serde(default)]
    pub extra_body: serde_json::Value,
}

fn default_cid_source() -> ConversationIdSource {
    ConversationIdSource::Unknown
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnRecord {
    pub role: Role,
    pub content: Vec<MessagePart>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum MessagePart {
    Text {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<serde_json::Value>,
    },
    Image {
        url: Option<String>,
        mime_type: Option<String>,
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<serde_json::Value>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        /// Methodical: A first-class signature structure
        signature: Option<HubSignature>,
        #[serde(default)]
        metadata: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<serde_json::Value>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<serde_json::Value>,
    },
    Thought {
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubSignature {
    /// The primary token string (Google's native format)
    pub thought_signature: Option<String>,
    /// The complex reasoning details (OpenRouter's aggregate format)
    pub reasoning_details: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct HubSignatureMetadata {
    pub thought_signature: Option<String>,
    pub reasoning_details: Option<serde_json::Value>,
}

impl From<HubSignatureMetadata> for HubSignature {
    fn from(meta: HubSignatureMetadata) -> Self {
        let mut thought_signature = meta.thought_signature;

        // Deep nesting fallback logic moved here
        if thought_signature.is_none() {
            if let Some(details) = &meta.reasoning_details {
                if let Some(arr) = details.as_array() {
                    if let Some(first) = arr.first() {
                        if let Some(data) = first.get("data").and_then(|v| v.as_str()) {
                            thought_signature = Some(data.to_string());
                        }
                    }
                }
            }
        }

        Self {
            thought_signature,
            reasoning_details: meta.reasoning_details,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum PulsePart {
    Text {
        delta: String,
    },
    ToolCall {
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
        metadata: Option<serde_json::Value>,
    },
    Thought {
        delta: String,
    },
}

/// --- STREAMING HUB (Micro Representation) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalPulse {
    pub content: Vec<PulsePart>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct PulseDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub role: Option<Role>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ProviderToolCallDelta>>,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl PulseDelta {
    pub fn extract_reasoning(&self) -> Option<String> {
        let val = self
            .extra
            .get("reasoning")
            .or_else(|| self.extra.get("thought"))?;
        match val.as_str() {
            Some(s) if !s.is_empty() => Some(s.to_string()),
            _ => None,
        }
    }

    pub fn extract_reasoning_mut(&mut self) -> Option<&mut String> {
        let key = if self.extra.contains_key("reasoning") {
            Some("reasoning")
        } else if self.extra.contains_key("thought") {
            Some("thought")
        } else {
            None
        };

        if let Some(k) = key {
            match self.extra.get_mut(k) {
                Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s),
                _ => None,
            }
        } else {
            None
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct ProviderPulseChoice {
    pub delta: PulseDelta,
    pub finish_reason: Option<String>,
}

#[derive(Default, Clone)]
pub struct TurnAccumulator {
    pub role: Option<Role>,
    pub text_buffer: String,
    pub thought_buffer: String,
    pub tool_calls: std::collections::HashMap<String, ToolCallBuffer>,
    pub signatures: std::collections::HashMap<String, serde_json::Map<String, serde_json::Value>>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Clone)]
pub struct ToolCallBuffer {
    pub name: String,
    pub arguments: String,
    pub metadata: serde_json::Value,
    /// Track if arguments JSON is complete (for streaming detection)
    pub arguments_complete: bool,
}

impl TurnAccumulator {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, pulse: InternalPulse) {
        if self.finish_reason.is_none() {
            self.finish_reason = pulse.finish_reason;
        }
        if let Some(usage) = pulse.usage {
            self.usage = Some(usage);
        }
        for part in pulse.content {
            match part {
                PulsePart::Text { delta } => self.text_buffer.push_str(&delta),
                PulsePart::Thought { delta } => self.thought_buffer.push_str(&delta),
                PulsePart::ToolCall {
                    id,
                    name,
                    arguments_delta,
                    metadata,
                } => {
                    let id_str = match id {
                        Some(s) => s.clone(),
                        None => "default".to_string(),
                    };
                    let entry = self
                        .tool_calls
                        .entry(id_str.clone())
                        .or_insert(ToolCallBuffer {
                            name: String::new(),
                            arguments: String::new(),
                            metadata: serde_json::Value::Null,
                            arguments_complete: false,
                        });
                    if let Some(n) = name {
                        tracing::debug!("[ACCUMULATOR] Tool call {} name: {}", id_str, n);
                        entry.name = n;
                    }
                    if let Some(m) = metadata {
                        entry.metadata = m.clone();
                        if let Some(obj) = m.as_object() {
                            self.signatures
                                .entry(id_str.clone())
                                .or_default()
                                .extend(obj.clone());
                        }
                    }
                    if !arguments_delta.is_empty() {
                        tracing::debug!(
                            "[ACCUMULATOR] Tool call {} arguments delta: {} chars (total: {} -> {})",
                            id_str,
                            arguments_delta.len(),
                            entry.arguments.len(),
                            entry.arguments.len() + arguments_delta.len()
                        );
                    }
                    entry.arguments.push_str(&arguments_delta);
                }
            }
        }
    }
    pub fn finalize(self) -> TurnRecord {
        let mut content = Vec::new();
        if !self.text_buffer.is_empty() {
            content.push(MessagePart::Text {
                content: self.text_buffer,
                cache_control: None,
            });
        }
        if !self.thought_buffer.is_empty() {
            content.push(MessagePart::Thought {
                content: self.thought_buffer,
            });
        }

        for (id, buf) in self.tool_calls {
            let finalized_tool_call = Self::finalize_tool_call(id, buf);
            content.push(finalized_tool_call);
        }

        let role = match self.role {
            Some(r) => r,
            None => Role::Assistant,
        };
        TurnRecord {
            role,
            content,
            tool_call_id: None,
        }
    }

    fn finalize_tool_call(id: String, buf: ToolCallBuffer) -> MessagePart {
        let mut args_json =
            match crate::json_repair::repair_tool_call_arguments(&buf.name, &buf.arguments) {
                Ok(v) => v,
                Err(repair_err) => {
                    tracing::warn!(
                    "[FINALIZE] Tool call '{}' (id={}) has invalid arguments even after repair: {}",
                    buf.name,
                    id,
                    repair_err
                );
                    // Return empty object as fallback rather than failing the entire operation
                    serde_json::json!({})
                }
            };

        // Hardening Hook: Sanitize tool arguments (e.g. fix mutually exclusive flags)
        crate::hardening::sanitize_tool_call(&buf.name, &mut args_json);

        MessagePart::ToolCall {
            id,
            name: buf.name,
            arguments: args_json,
            signature: None,
            metadata: buf.metadata,
            cache_control: None,
        }
    }
}

/// --- PROVIDER WIRE TYPES ---

#[derive(serde::Deserialize, Debug)]
pub enum LineEvent {
    Pulse(ProviderPulse),
    Error(ProviderError),
    #[allow(dead_code)]
    Unknown(String),
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct ProviderPulse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub choices: Vec<ProviderPulseChoice>,
    pub usage: Option<Usage>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct ProviderToolCallDelta {
    pub index: u32,
    pub id: Option<String>,
    pub function: Option<RawFunction>,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct RawFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct ProviderError {
    pub error: ProviderErrorDetails,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct ProviderErrorDetails {
    pub message: String,
    pub code: Option<u16>,
    pub metadata: Option<serde_json::Value>,

    /// Catch-all for extra provider fields like `retryable`, `reason`, `provider: { status, body }`
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn parse_provider_line(data: &str) -> LineEvent {
    if data.len() > 10 * 1024 * 1024 {
        return LineEvent::Error(ProviderError {
            error: ProviderErrorDetails {
                message: format!("JSON chunk too large: {} bytes", data.len()),
                code: Some(413),
                metadata: None,
                extra: serde_json::Map::new(),
            },
        });
    }
    // Try Error first as it's more specific (requires "error" key)
    if let Ok(err) = serde_json::from_str::<ProviderError>(data) {
        return LineEvent::Error(err);
    }
    if let Ok(pulse) = serde_json::from_str::<ProviderPulse>(data) {
        // Validation: A pulse should either have choices or usage to be considered a pulse
        if !pulse.choices.is_empty() || pulse.usage.is_some() {
            return LineEvent::Pulse(pulse);
        }
    }
    let snippet = if data.len() > 200 {
        format!("{}...", &data[..200])
    } else {
        data.to_string()
    };
    tracing::debug!("[STREAM] Unknown line format: {}", snippet);
    LineEvent::Unknown(data.to_string())
}

#[cfg(test)]
mod parsing_tests {
    use super::*;

    #[test]
    fn test_parse_provider_pulse_full() {
        let json = r#"{"id":"123","model":"gpt-4","choices":[{"index":0,"delta":{"content":"Hello"}}],"usage":null}"#;
        let event = parse_provider_line(json);
        match event {
            LineEvent::Pulse(p) => assert_eq!(p.id, "123"),
            _ => panic!("Expected Pulse"),
        }
    }

    #[test]
    fn test_parse_provider_pulse_partial_gemini() {
        // Gemini often sends chunks without ID or Model in final usage messages
        let json = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let event = parse_provider_line(json);
        match event {
            LineEvent::Pulse(p) => {
                assert!(p.id.is_empty()); // Default
                assert!(p.usage.is_some());
            }
            _ => panic!("Expected Pulse"),
        }
    }
}
