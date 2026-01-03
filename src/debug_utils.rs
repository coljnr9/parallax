use crate::redaction::{redact_value, RedactionLevel};
use crate::str_utils;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct FlightRecorder {
    pub turn_id: String,
    pub request_id: String,
    pub conversation_id: String,
    pub model_id: String,
    pub flavor: String,
    pub decisions: Vec<String>,
    pub stages: std::collections::HashMap<String, Value>,
    #[serde(skip)]
    pub redaction_level: RedactionLevel,
}

impl FlightRecorder {
    pub fn new(turn_id: &str, rid: &str, cid: &str, model: &str, flavor: &str) -> Self {
        Self {
            turn_id: turn_id.to_string(),
            request_id: rid.to_string(),
            conversation_id: cid.to_string(),
            model_id: model.to_string(),
            flavor: flavor.to_string(),
            decisions: Vec::new(),
            stages: std::collections::HashMap::new(),
            redaction_level: RedactionLevel::default(),
        }
    }

    pub fn record_decision(&mut self, decision: String) {
        self.decisions.push(decision);
    }

    pub fn record_stage(&mut self, label: &str, payload: Value) {
        let mut sanitized = payload.clone();
        redact_value(&mut sanitized, self.redaction_level);
        self.stages.insert(label.to_string(), sanitized);
    }

    /// Classifies an upstream error body (HTML vs JSON) and records it.
    pub fn record_upstream_error(&mut self, status: reqwest::StatusCode, body: &str) {
        let mut error_info = serde_json::json!({
            "status": status.as_u16(),
            "classification": "unknown",
            "body_snippet": str_utils::prefix_chars(body, 500),
        });

        if body.trim_start().starts_with("<!DOCTYPE html") || body.trim_start().starts_with("<html")
        {
            error_info["classification"] = serde_json::json!("HTML/Cloudflare");
            if let Some(ray_id_idx) = body.find("CF-RAY:") {
                let ray_id = match body[ray_id_idx..].split_whitespace().nth(1) {
                    Some(id) => {
                        // Further split by common HTML tags or punctuation
                        match id.split('<').next() {
                            Some(prefix) => prefix,
                            None => id,
                        }
                    }
                    None => "unknown",
                };
                error_info["cf_ray"] = serde_json::json!(ray_id);
            }
        } else if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
            error_info["classification"] = serde_json::json!("JSON");
            error_info["json"] = json;
        }

        self.record_stage("upstream_error", error_info);
    }

    fn get_capture_path(&self) -> (String, String) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let safe_model = self.model_id.replace("/", "_").replace(":", "_");
        let safe_cid = str_utils::prefix_chars(&self.conversation_id, 8);
        let filename = format!(
            "debug_capture/{}_flight_{}_{}.json",
            timestamp, safe_cid, safe_model
        );
        ("debug_capture".to_string(), filename)
    }

    pub async fn save(&self) {
        let (dir, filename) = self.get_capture_path();

        if let Err(e) = tokio::fs::create_dir_all(dir).await {
            tracing::error!("Failed to create debug_capture directory: {}", e);
            return;
        }

        self.save_to_disk(&filename).await;
    }

    async fn save_to_disk(&self, filename: &str) {
        let content = self.serialize_flight();
        if let Some(content) = content {
            self.write_flight_to_disk(filename, content).await;
        }
    }

    fn serialize_flight(&self) -> Option<String> {
        match serde_json::to_string_pretty(self) {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::error!("Failed to serialize flight recorder: {}", e);
                None
            }
        }
    }

    async fn write_flight_to_disk(&self, filename: &str, content: String) {
        if let Err(e) = tokio::fs::write(filename, content).await {
            tracing::error!("Failed to save flight recorder artifact: {}", e);
        } else {
            tracing::info!("Saved flight recorder artifact to {}", filename);
            Self::cleanup_old_reports().await;
        }
    }

    async fn cleanup_old_reports() {
        let dir = "debug_capture";
        let max_files = 1024;
        let max_size_bytes = 100 * 1024 * 1024; // 100MB

        let mut entries = Vec::new();
        let mut read_dir = match tokio::fs::read_dir(dir).await {
            Ok(rd) => rd,
            Err(_) => return,
        };

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let metadata = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.is_file() {
                let modified = metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                entries.push((entry.path(), modified, metadata.len()));
            }
        }

        // Sort by modification time (oldest first)
        entries.sort_by_key(|e| e.1);

        let mut total_size: u64 = entries.iter().map(|e| e.2).sum();
        let mut file_count = entries.len();

        for (path, _, size) in entries {
            if file_count <= max_files && total_size <= max_size_bytes {
                break;
            }

            match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    file_count -= 1;
                    total_size -= size;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // File was already deleted, just update counts
                    file_count -= 1;
                    total_size -= size;
                }
                Err(e) => {
                    tracing::error!("Failed to delete old debug report {:?}: {}", path, e);
                }
            }
        }
    }
}

#[allow(dead_code)]
pub fn summarize_json(value: &Value) -> String {
    summarize_json_inner(value, 0)
}

pub fn log_traffic_summary(direction: &str, body: &Value) {
    tracing::info!("--- [TRACE: {}] ---", direction);

    log_request_messages(body);
    log_tool_definitions(body);
    log_response_choices(body);

    tracing::info!("------------------------");
}

fn log_request_messages(body: &Value) {
    if let Some(msgs) = body.get("messages").and_then(|v| v.as_array()) {
        let count = msgs.len();
        let last_role = msgs
            .last()
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            .unwrap_or("unknown");

        // Check for prefill (Assistant as last message)
        let is_prefill = last_role == "assistant";
        tracing::info!(
            "📝 History: {} msgs | Last: {} | Prefill: {}",
            count,
            last_role,
            is_prefill
        );
    }
}

fn log_tool_definitions(body: &Value) {
    if let Some(tools) = body.get("tools").and_then(|v| v.as_array()) {
        let tool_names: Vec<_> = tools
            .iter()
            .filter_map(|t| t.get("function")?.get("name")?.as_str())
            .collect();
        tracing::info!("🛠️  Tools definitions: {:?}", tool_names);

        // Check for 'strict' mode (The killer bug)
        let has_strict = tools.iter().any(|t| {
            t.get("function")
                .is_some_and(|f| f.get("strict").unwrap_or(&Value::Bool(false)) == true)
        });
        if has_strict {
            tracing::warn!("⚠️  WARNING: 'strict: true' detected! (Must strip for OpenRouter)");
        }
    }
}

pub fn log_response_choices(body: &Value) {
    if let Some(choices) = body.get("choices").and_then(|v| v.as_array()) {
        for (i, choice) in choices.iter().enumerate() {
            let finish = choice
                .get("finish_reason")
                .and_then(|s| s.as_str())
                .unwrap_or("NONE");
            let msg = choice.get("message");
            let content = msg.and_then(|m| m.get("content"));
            let tools = msg
                .and_then(|m| m.get("tool_calls"))
                .and_then(|t| t.as_array());

            // Check Content Status
            let content_status = match content {
                Some(Value::Null) => "NULL (Good)",
                Some(Value::String(s)) if s.is_empty() => "EMPTY (Good)",
                Some(Value::String(s)) => &format!("TEXT ({} chars)", s.len()), // Log length, not text
                _ => "MISSING",
            };

            // Check Tool Status
            let tool_status = if let Some(t) = tools {
                let ids: Vec<_> = t
                    .iter()
                    .filter_map(|call| call.get("id").and_then(|id| id.as_str()))
                    .collect();
                format!("{} calls -> IDs: {:?}", t.len(), ids)
            } else {
                "None".to_string()
            };

            tracing::info!(
                "🤖 Choice #{}: Finish='{}' | Content={} | Tools={}",
                i,
                finish,
                content_status,
                tool_status
            );
        }
    }
}

pub async fn capture_debug_snapshot(
    label: &str,
    model_id: &str,
    conversation_id: &str,
    request_id: &str,
    payload: &Value,
) {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let safe_model = model_id.replace("/", "_").replace(":", "_");
    let safe_cid = str_utils::prefix_chars(conversation_id, 8);
    let safe_rid = str_utils::prefix_chars(request_id, 8);
    let filename = format!(
        "debug_capture/{}_{}_{}_{}_{}.json",
        timestamp, label, safe_cid, safe_rid, safe_model
    );

    save_snapshot_to_disk(&filename, payload).await;
}

async fn save_snapshot_to_disk(filename: &str, payload: &Value) {
    if let Err(e) = tokio::fs::create_dir_all("debug_capture").await {
        tracing::error!("Failed to create debug_capture directory: {}", e);
        return;
    }

    write_snapshot_to_disk(filename, payload).await;
}

async fn write_snapshot_to_disk(filename: &str, payload: &Value) {
    if let Ok(content) = serde_json::to_string_pretty(payload) {
        let _ = tokio::fs::write(filename, content).await;
    }
}

#[allow(dead_code)]
fn summarize_json_inner(value: &Value, indent: usize) -> String {
    let space = "  ".repeat(indent);
    let next_space = "  ".repeat(indent + 1);

    match value {
        Value::Object(map) => {
            if map.is_empty() {
                "{}".to_string()
            } else {
                let mut summarized_fields = Vec::new();
                for (k, v) in map {
                    // Special handling for the "messages" array to show first and last
                    if k == "messages" {
                        if let Value::Array(arr) = v {
                            if arr.len() > 2 {
                                let mut items = Vec::new();
                                items.push(format!(
                                    "{}{}",
                                    next_space,
                                    summarize_json_inner(&arr[0], indent + 1)
                                ));
                                items.push(format!(
                                    "{}(...{} more items skipped)",
                                    next_space,
                                    arr.len() - 2
                                ));
                                items.push(format!(
                                    "{}{}",
                                    next_space,
                                    summarize_json_inner(&arr[arr.len() - 1], indent + 1)
                                ));
                                summarized_fields.push(format!(
                                    "{}{}: [\n{}\n{}]",
                                    next_space,
                                    k,
                                    items.join(",\n"),
                                    next_space
                                ));
                                continue;
                            }
                        }
                    }
                    summarized_fields.push(format!(
                        "{}{}: {}",
                        next_space,
                        k,
                        summarize_json_inner(v, indent + 1)
                    ));
                }
                format!("{{\n{}\n{}}}", summarized_fields.join(",\n"), space)
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                "[]".to_string()
            } else {
                let limit = 2;
                let mut summarized_items = Vec::new();
                for v in arr.iter().take(limit) {
                    summarized_items.push(format!(
                        "{}{}",
                        next_space,
                        summarize_json_inner(v, indent + 1)
                    ));
                }

                if arr.len() > limit {
                    summarized_items.push(format!(
                        "{}(...{} more items)",
                        next_space,
                        arr.len() - limit
                    ));
                }

                format!("[\n{}\n{}]", summarized_items.join(",\n"), space)
            }
        }
        Value::String(s) => {
            if s.len() > 60 {
                format!(
                    "\"{}...{}\" ({} chars)",
                    str_utils::prefix_chars(s, 20).replace("\n", "\\n"),
                    str_utils::suffix_chars(s, 20).replace("\n", "\\n"),
                    s.len()
                )
            } else {
                format!("\"{}\"", s.replace("\n", "\\n"))
            }
        }
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_summarize_complex() {
        let val = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "This is a very long prompt that should be truncated eventually if it was even longer than this."},
                {"role": "assistant", "content": "Short"}
            ],
            "config": {
                "temp": 0.7,
                "nested": "A very long string that will definitely be truncated by our logic"
            }
        });
        let summary = summarize_json(&val);
        println!("{}", summary);
        assert!(summary.contains("model"));
        assert!(summary.contains("config"));
    }
}
