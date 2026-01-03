use crate::constants::{DIFF_MARKERS, FORBIDDEN_PLAN_TERMS};
use crate::tag_extract::TagRegistry;
use crate::types::{ParallaxError, Result};
use axum::http as ax_http;
use serde_json::{Map, Value};
use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::RwLock;

pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
}

impl RetryPolicy {
    pub fn new(max_attempts: u32, base_delay_ms: u64) -> Self {
        Self {
            max_attempts,
            base_delay_ms,
        }
    }

    pub async fn execute_with_retry<F, Fut, T>(&self, mut operation: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut attempts = 0;
        loop {
            attempts += 1;
            match operation().await {
                Ok(val) => return Ok(val),
                Err(e) if attempts < self.max_attempts && self.is_retryable(&e) => {
                    let base_delay = self.base_delay_ms * 2u64.pow(attempts - 1);
                    // Add jitter: ±25% of the base delay
                    let jitter_range = base_delay / 4;
                    let jitter = if jitter_range > 0 {
                        fastrand::i64(-(jitter_range as i64)..jitter_range as i64)
                    } else {
                        0
                    };
                    let final_delay_ms = (base_delay as i64 + jitter).max(1) as u64;
                    let delay = Duration::from_millis(final_delay_ms);

                    tracing::warn!(
                        "Request failed (attempt {}): {}. Retrying in {:?} (jittered)...",
                        attempts,
                        e,
                        delay
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn is_retryable(&self, err: &crate::types::ObservedError) -> bool {
        match &err.inner {
            ParallaxError::Network(_) | ParallaxError::Io(_) | ParallaxError::Internal(_, _) => {
                true
            }
            ParallaxError::Upstream(status, _) => {
                status.is_server_error() || *status == axum::http::StatusCode::TOO_MANY_REQUESTS
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

pub struct CircuitBreaker {
    state: Arc<RwLock<CircuitState>>,
    failure_threshold: u32,
    recovery_timeout: Duration,
    consecutive_failures: Arc<AtomicU32>,
    last_failure_time: Arc<RwLock<Option<Instant>>>,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, recovery_timeout: Duration) -> Self {
        Self {
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            failure_threshold,
            recovery_timeout,
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            last_failure_time: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn check(&self) -> Result<()> {
        let mut state = self.state.write().await;
        self.check_locked(&mut state).await
    }

    pub async fn check_locked(&self, state: &mut CircuitState) -> Result<()> {
        if *state == CircuitState::Open {
            let last_failure = match self.last_failure_time.try_read() {
                Ok(last) => last,
                Err(_) => {
                    tracing::warn!(
                        "Could not read last failure time for circuit breaker, assuming still OPEN"
                    );
                    return Err(ParallaxError::Upstream(
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        "Circuit breaker is OPEN (failure time locked)".to_string(),
                    )
                    .into());
                }
            };

            if let Some(last) = *last_failure {
                if last.elapsed() > self.recovery_timeout {
                    tracing::info!("Circuit breaker transitioning to Half-Open");
                    *state = CircuitState::HalfOpen;
                    return Ok(());
                }
            }

            return Err(ParallaxError::Upstream(
                ax_http::StatusCode::SERVICE_UNAVAILABLE,
                "Circuit breaker is OPEN".to_string(),
            )
            .into());
        }
        Ok(())
    }

    pub async fn state_raw_lock(&self) -> tokio::sync::RwLockReadGuard<'_, CircuitState> {
        self.state.read().await
    }

    pub async fn record_success(&self) {
        let mut state = self.state.write().await;
        self.consecutive_failures.store(0, Ordering::Relaxed);
        if *state != CircuitState::Closed {
            tracing::info!("Circuit breaker transitioning to CLOSED");
            *state = CircuitState::Closed;
        }
    }

    pub async fn record_failure(&self) {
        let mut state = self.state.write().await;
        let failures = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        *self.last_failure_time.write().await = Some(Instant::now());

        if failures >= self.failure_threshold && *state != CircuitState::Open {
            tracing::error!(
                "Circuit breaker transitioning to OPEN ({} consecutive failures)",
                failures
            );
            *state = CircuitState::Open;
        }
    }
}

pub fn sanitize_tool_call(name: &str, args: &mut serde_json::Value) {
    match name {
        "grep" => sanitize_grep_args(args),
        "create_plan" => sanitize_plan_args(args),
        _ => {}
    }
}

fn sanitize_grep_args(args: &mut Value) {
    if let Value::Object(map) = args {
        // Fix mutual exclusivity: If -A or -B is set (and > 0), remove -C if it is 0
        let has_a = has_positive_value(map, "-A");
        let has_b = has_positive_value(map, "-B");
        let c_is_zero = is_zero(map, "-C");

        if (has_a || has_b) && c_is_zero {
            map.remove("-C");
        }
    }
}

fn sanitize_plan_args(args: &mut Value) {
    if let Value::Object(map) = args {
        // Extract title first before any mutable borrows
        let title = match map.get("name").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "Implementation Plan".to_string(),
        };

        // Ensure plan has a proper H1 title if missing
        if let Some(Value::String(plan)) = map.get_mut("plan") {
            let trimmed_plan = plan.trim();
            if !trimmed_plan.starts_with("# ") {
                // Prepend the title as H1
                *plan = format!("# {}\n\n{}", title, plan);
            }
        }

        // Clean up any forbidden terms that might cause execution failures
        if let Some(Value::String(plan)) = map.get_mut("plan") {
            for (forbidden, replacement) in FORBIDDEN_PLAN_TERMS {
                if plan.contains(forbidden) {
                    *plan = plan.replace(forbidden, replacement);
                }
            }
        }
    }
}

fn has_positive_value(map: &Map<String, Value>, key: &str) -> bool {
    match map.get(key).and_then(|v| v.as_u64()) {
        Some(v) => v > 0,
        None => false,
    }
}

fn is_zero(map: &Map<String, Value>, key: &str) -> bool {
    match map.get(key).and_then(|v| v.as_u64()) {
        Some(v) => v == 0,
        None => false,
    }
}

pub fn is_diff_like(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }

    // Check for fenced diff/patch blocks
    if text.contains("```diff") || text.contains("```patch") {
        return true;
    }

    // Heuristic: Check for common unified diff markers at the start of lines
    for line in text.lines() {
        let trimmed = line.trim_start();
        for marker in DIFF_MARKERS {
            if trimmed.starts_with(marker) {
                return true;
            }
        }
    }

    false
}

/// Scrub known tool-protocol leakage patterns from assistant text/thought.
/// Specifically targets xAI's <xai:function_call> markup and "Assistant:" boilerplate.
pub fn scrub_tool_protocol_leaks(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let mut out = String::new();

    for line in text.lines() {
        let trimmed = line.trim();

        // xAI tool-call markup leaking into model text/thought
        if trimmed.starts_with("<xai:function_call") {
            continue;
        }

        // Common boilerplate prefix that some models emit
        if trimmed.starts_with("Assistant:") {
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    // Avoid changing semantics beyond removing those specific lines
    out.trim_end().to_string()
}

#[derive(Default, Debug)]
pub struct CursorTagScrubber {
    state: ScrubberState,
}

#[derive(Debug, Default, PartialEq, Eq, Clone)]
enum ScrubberState {
    #[default]
    Normal,
    InTagOpening(String), // The partial tag being read: e.g. "user_qu"
    InTagClosing(String, String), // (tag_to_match, partial_closing_tag) e.g. ("user_query", "/user_qu")
    Redacting(String),    // tag_name being redacted
    Unrolling(String),    // tag_name whose content we are keeping, but we want to drop its closing tag
}

impl CursorTagScrubber {
    pub fn new() -> Self {
        Self::default()
    }

    /// Processes a chunk of text and returns the scrubbed version.
    /// Maintains state between calls to handle tags spanning chunks.
    pub fn scrub_chunk(&mut self, chunk: &str) -> String {
        let registry = TagRegistry::default();
        let mut output = String::new();

        for c in chunk.chars() {
            match self.state.clone() {
                ScrubberState::Normal => {
                    if c == '<' {
                        self.state = ScrubberState::InTagOpening(String::new());
                    } else {
                        output.push(c);
                    }
                }
                ScrubberState::InTagOpening(mut partial) => {
                    if c == '>' {
                        let tag_name = partial.clone();
                        let is_scaffolding = registry.tags.iter().any(|t| t.tag == tag_name && t.is_scaffolding);
                        
                        if is_scaffolding {
                            if tag_name == "user_query" {
                                // For user_query, we drop the tags but keep the content
                                self.state = ScrubberState::Unrolling(tag_name);
                            } else {
                                // For other scaffolding, we redact everything
                                self.state = ScrubberState::Redacting(tag_name);
                            }
                        } else {
                            // Not a scaffolding tag, emit the original sequence
                            output.push('<');
                            output.push_str(&partial);
                            output.push('>');
                            self.state = ScrubberState::Normal;
                        }
                    } else if c.is_alphanumeric() || c == '_' || c == '-' {
                        partial.push(c);
                        self.state = ScrubberState::InTagOpening(partial);
                    } else if c == '/' && partial.is_empty() {
                         self.state = ScrubberState::InTagClosing(String::new(), "/".to_string());
                    } else {
                        // Not a valid tag character
                        output.push('<');
                        output.push_str(&partial);
                        output.push(c);
                        self.state = ScrubberState::Normal;
                    }
                }
                ScrubberState::Unrolling(tag_name) => {
                    if c == '<' {
                        self.state = ScrubberState::InTagClosing(tag_name, String::new());
                    } else {
                        output.push(c);
                    }
                }
                ScrubberState::InTagClosing(tag_to_match, mut partial_close) => {
                    if c == '>' {
                        let close_name = if let Some(stripped) = partial_close.strip_prefix('/') {
                            stripped
                        } else {
                            partial_close.as_str()
                        };

                        if !tag_to_match.is_empty() && close_name == tag_to_match {
                            // Successfully closed a tag we were unrolling or redacting
                            self.state = ScrubberState::Normal;
                        } else {
                            // Doesn't match
                            if tag_to_match.is_empty() {
                                // We were in Normal state and saw </...>
                                output.push('<');
                                output.push_str(&partial_close);
                                output.push('>');
                                self.state = ScrubberState::Normal;
                            } else {
                                // We are in Redacting/Unrolling mode but this wasn't our closing tag
                                // If we were unrolling, we must emit the '<', the partial, and the '>' because it might be a nested tag or just text
                                // Wait, if we are in Unrolling, and we see <other_tag>, we should just emit it.
                                let is_unrolling = registry.tags.iter().any(|t| t.tag == tag_to_match && t.tag == "user_query");
                                
                                if is_unrolling {
                                    output.push('<');
                                    output.push_str(&partial_close);
                                    output.push('>');
                                    self.state = ScrubberState::Unrolling(tag_to_match);
                                } else {
                                    // Stay redacting
                                    self.state = ScrubberState::Redacting(tag_to_match);
                                }
                            }
                        }
                    } else if c.is_alphanumeric() || c == '_' || c == '-' || c == '/' {
                        partial_close.push(c);
                        self.state = ScrubberState::InTagClosing(tag_to_match, partial_close);
                    } else if tag_to_match.is_empty() {
                        output.push('<');
                        output.push_str(&partial_close);
                        output.push(c);
                        self.state = ScrubberState::Normal;
                    } else {
                        let is_unrolling = registry.tags.iter().any(|t| t.tag == tag_to_match && t.tag == "user_query");
                        if is_unrolling {
                            output.push('<');
                            output.push_str(&partial_close);
                            output.push(c);
                            self.state = ScrubberState::Unrolling(tag_to_match);
                        } else {
                            self.state = ScrubberState::Redacting(tag_to_match);
                        }
                    }
                }
                ScrubberState::Redacting(tag_name) => {
                    if c == '<' {
                        self.state = ScrubberState::InTagClosing(tag_name, String::new());
                    } else {
                        // Drop content
                    }
                }
            }
        }
        output
    }

    /// Finalize scrubbing, returning any leftovers (e.g. if a tag was left open).
    pub fn finalize(self) -> String {
        match self.state {
            ScrubberState::Normal => String::new(),
            ScrubberState::InTagOpening(partial) => format!("<{}", partial),
            ScrubberState::InTagClosing(tag_to_match, partial_close) => {
                if tag_to_match.is_empty() {
                    format!("<{}", partial_close)
                } else {
                    let is_unrolling = tag_to_match == "user_query";
                    if is_unrolling {
                         format!("<{}", partial_close)
                    } else {
                        String::new()
                    }
                }
            }
            ScrubberState::Redacting(_) => String::new(),
            ScrubberState::Unrolling(_) => String::new(),
        }
    }
}

pub fn scrub_cursor_tags(text: &str) -> String {
    let mut scrubber = CursorTagScrubber::new();
    let mut out = scrubber.scrub_chunk(text);
    out.push_str(&scrubber.finalize());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_scrub_cursor_tags_simple() {
        let input = "Hello <user_query>What is up?</user_query> more <system_reminder>secret</system_reminder> end";
        let got = scrub_cursor_tags(input);
        assert_eq!(got, "Hello What is up? more  end");
    }

    #[test]
    fn test_scrub_cursor_tags_streaming() {
        let mut scrubber = CursorTagScrubber::new();
        let chunk1 = "Hello <user_qu";
        let chunk2 = "ery>What is ";
        let chunk3 = "up?</user_query> more <system_remin";
        let chunk4 = "der>secret</system_reminder> end";

        let mut got = String::new();
        got.push_str(&scrubber.scrub_chunk(chunk1));
        got.push_str(&scrubber.scrub_chunk(chunk2));
        got.push_str(&scrubber.scrub_chunk(chunk3));
        got.push_str(&scrubber.scrub_chunk(chunk4));
        got.push_str(&scrubber.finalize());

        assert_eq!(got, "Hello What is up? more  end");
    }

    #[test]
    fn test_scrub_cursor_tags_non_scaffolding() {
        let input = "Keep <unknown_tag>content</unknown_tag> please";
        let got = scrub_cursor_tags(input);
        assert_eq!(got, "Keep <unknown_tag>content</unknown_tag> please");
    }

    #[test]
    fn test_scrub_cursor_tags_malformed() {
        let input = "Partial <system_reminder without end";
        let got = scrub_cursor_tags(input);
        assert_eq!(got, "Partial <system_reminder without end");
    }

    #[test]
    fn test_scrub_tool_protocol_leaks() {
        let input = "Assistant: \n<xai:function_call name=\"read_file\">\nKeep this line.\n";
        let got = scrub_tool_protocol_leaks(input);
        assert_eq!(got, "Keep this line.");

        let input_no_leaks = "This is a normal line.";
        let got_no_leaks = scrub_tool_protocol_leaks(input_no_leaks);
        assert_eq!(got_no_leaks, "This is a normal line.");

        // Relaxed matching for lines starting with "Assistant:"
        let input_relaxed =
            "Assistant: 1|# Terminus System Architecture\n<xai:function_call name=\"read_file\">";
        let got_relaxed = scrub_tool_protocol_leaks(input_relaxed);
        assert_eq!(got_relaxed, "");
    }

    #[test]
    fn test_is_diff_like() {
        assert!(is_diff_like("```diff\n+ added\n- removed\n```"));
        assert!(is_diff_like("```patch\n+ added\n- removed\n```"));
        assert!(is_diff_like("diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1,1 +1,1 @@\n-old\n+new"));
        assert!(is_diff_like("--- a/file.rs\n+++ b/file.rs"));
        assert!(is_diff_like("@@ -1,5 +1,6 @@"));

        // Negative cases
        assert!(!is_diff_like("Just some normal text."));
        assert!(!is_diff_like(
            "fn main() {\n    println!(\"Hello, world!\");\n}"
        ));
        assert!(!is_diff_like("The value is --- unknown ---."));
    }

    #[test]
    fn test_sanitize_grep_mutual_exclusivity() {
        let mut args = json!({
            "-A": 2,
            "-B": 2,
            "-C": 0,
            "pattern": "test"
        });

        sanitize_tool_call("grep", &mut args);

        if let Some(map) = args.as_object() {
            assert!(map.contains_key("-A"));
            assert!(map.contains_key("-B"));
            assert!(!map.contains_key("-C")); // Should be removed
        } else {
            panic!("Expected args to be an object");
        }
    }

    #[test]
    fn test_sanitize_grep_keeps_c_if_others_missing() {
        let mut args = json!({
            "-C": 2,
            "pattern": "test"
        });

        sanitize_tool_call("grep", &mut args);

        if let Some(map) = args.as_object() {
            assert!(map.contains_key("-C"));
        } else {
            panic!("Expected args to be an object");
        }
    }

    #[test]
    fn test_sanitize_grep_keeps_c_if_others_zero() {
        let mut args = json!({
            "-A": 0,
            "-B": 0,
            "-C": 2,
            "pattern": "test"
        });

        sanitize_tool_call("grep", &mut args);

        if let Some(map) = args.as_object() {
            assert!(map.contains_key("-C"));
            assert!(map.contains_key("-A"));
            assert!(map.contains_key("-B"));
        } else {
            panic!("Expected args to be an object");
        }
    }

    #[test]
    fn test_sanitize_plan_adds_missing_title() {
        let mut args = json!({
            "plan": "This is a plan without a title.\n\n## Implementation\n\nStep 1: Do something"
        });

        sanitize_tool_call("create_plan", &mut args);

        if let Some(map) = args.as_object() {
            let plan_val = match map.get("plan") {
                Some(p) => p,
                None => panic!("Missing 'plan' field"),
            };
            let plan = match plan_val.as_str() {
                Some(s) => s,
                None => panic!("'plan' is not a string"),
            };
            assert!(plan.starts_with("# Implementation Plan"));
            assert!(plan.contains("This is a plan without a title"));
        } else {
            panic!("Expected args to be an object");
        }
    }

    #[test]
    fn test_sanitize_plan_uses_name_field() {
        let mut args = json!({
            "name": "My Custom Plan",
            "plan": "This plan should use the name field as title."
        });

        sanitize_tool_call("create_plan", &mut args);

        if let Some(map) = args.as_object() {
            let plan_val = match map.get("plan") {
                Some(p) => p,
                None => panic!("Missing 'plan' field"),
            };
            let plan = match plan_val.as_str() {
                Some(s) => s,
                None => panic!("'plan' is not a string"),
            };
            assert!(plan.starts_with("# My Custom Plan"));
            assert!(plan.contains("This plan should use the name field as title"));
        } else {
            panic!("Expected args to be an object");
        }
    }

    #[test]
    fn test_sanitize_plan_keeps_existing_title() {
        let mut args = json!({
            "plan": "# Existing Title\n\nThis plan already has a title."
        });

        sanitize_tool_call("create_plan", &mut args);

        if let Some(map) = args.as_object() {
            let plan_val = match map.get("plan") {
                Some(p) => p,
                None => panic!("Missing 'plan' field"),
            };
            let plan = match plan_val.as_str() {
                Some(s) => s,
                None => panic!("'plan' is not a string"),
            };
            assert!(plan.starts_with("# Existing Title"));
            assert!(plan.contains("This plan already has a title"));
        } else {
            panic!("Expected args to be an object");
        }
    }

    #[test]
    fn test_sanitize_plan_cleans_forbidden_terms() {
        let mut args = json!({
            "plan": "# Plan with forbidden terms\n\nUse npm install and cargo build commands.\nAlso use grep for searching."
        });

        sanitize_tool_call("create_plan", &mut args);

        if let Some(map) = args.as_object() {
            let plan_val = match map.get("plan") {
                Some(p) => p,
                None => panic!("Missing 'plan' field"),
            };
            let plan = match plan_val.as_str() {
                Some(s) => s,
                None => panic!("'plan' is not a string"),
            };
            assert!(
                !plan.contains("npm install"),
                "Should not contain 'npm install'"
            );
            assert!(
                !plan.contains("cargo build"),
                "Should not contain 'cargo build'"
            );
            // Check that grep was replaced with ripgrep (not just removed)
            assert!(
                plan.contains("ripgrep "),
                "Should contain 'ripgrep ' replacement"
            );
            assert!(plan.contains("package manager install"));
            assert!(plan.contains("rust build"));
            // Verify the original problematic terms are gone
            assert!(!plan.contains("npm install"));
            assert!(!plan.contains("cargo build"));
        } else {
            panic!("Expected args to be an object");
        }
    }
}
