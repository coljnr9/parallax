use crate::types::TurnRecord;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub created_at_ms: u64,
    pub last_updated_ms: u64,
    pub turns: Vec<TurnSummary>,
    pub issues: IssueCounts,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TurnSummary {
    pub turn_id: String,
    pub request_id: String,
    pub model_id: String,
    pub flavor: String,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub issues: IssueCounts,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct IssueCounts {
    pub tool_args_empty: u32,
    pub tool_args_repaired: u32,
    pub rescue_used: u32,
    pub tag_unregistered: u32,
    pub tag_leak_echo: u32,
    pub reasoning_leak: u32,
    #[serde(default)]
    pub tool_args_invalid: u32,
    #[serde(default)]
    pub tool_call_duplicate_id: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TurnDetail {
    pub turn_id: String,
    pub request_id: String,
    pub model_id: String,
    pub flavor: String,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub stages: Vec<StageIndex>,
    pub tool_calls: Vec<ToolCallIndex>,
    pub cursor_tags: TagSummary,
    pub issues: Vec<Issue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_summary: Option<Vec<SpanSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_query: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_query_tags: Option<Vec<crate::tag_extract::TagDelta>>,
    #[serde(default)]
    pub tool_results: Vec<ToolResultIndex>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolResultIndex {
    pub tool_call_id: String,
    pub name: Option<String>,
    pub snippet: String,
    pub is_error: bool,
    pub blob_ref: Option<BlobRef>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StageIndex {
    pub name: String,
    pub kind: StageKind,
    pub summary: serde_json::Value,
    pub blob_ref: Option<BlobRef>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    Snapshot,
    Event,
    Metric,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlobRef {
    pub blob_id: String,
    pub content_type: String,
    pub approx_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written_at_ms: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCallIndex {
    pub id: String,
    pub name: String,
    pub args_status: ToolArgsStatus,
    pub origin: ToolCallOrigin,
    pub evidence: ToolCallEvidence,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolArgsStatus {
    Ok,
    Repaired,
    Rescue,
    Empty,
    Invalid,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallOrigin {
    Ingress,
    UpstreamStream,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCallEvidence {
    pub stage: String,
    pub message_index: Option<usize>,
    pub snippet: Option<String>,
    pub blob_ref: Option<BlobRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offsets: Option<Offsets>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_tool_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_tool_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_arguments_snippet: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TagSummary {
    pub registered: Vec<TagOccurrence>,
    pub unregistered: Vec<TagOccurrence>,
    pub leaks: Vec<TagOccurrence>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TagOccurrence {
    pub tag: String,
    pub count: u32,
    pub locations: Vec<TagLocation>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TagLocation {
    pub stage: String,
    pub message_index: Option<usize>,
    pub offsets: Option<Offsets>,
    pub snippet: String,
    pub blob_ref: Option<BlobRef>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Offsets {
    pub start: usize,
    pub end: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Issue {
    pub kind: String,
    pub severity: String,
    pub message: String,
    pub context: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpanSummary {
    pub name: String,
    pub level: String,
    pub fields: serde_json::Value,
}

pub struct BundleManager {
    base_path: PathBuf,
    max_size_bytes: u64,
}

impl BundleManager {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            base_path: path.as_ref().to_path_buf(),
            max_size_bytes: 100 * 1024 * 1024, // 100MB default
        }
    }

    pub fn with_max_size<P: AsRef<Path>>(path: P, max_size_bytes: u64) -> Self {
        Self {
            base_path: path.as_ref().to_path_buf(),
            max_size_bytes,
        }
    }

    pub async fn ensure_conversation_dir(&self, cid: &str) -> std::io::Result<PathBuf> {
        let path = self.base_path.join("conversations").join(cid);
        tokio::fs::create_dir_all(&path).await?;
        Ok(path)
    }

    pub async fn ensure_turn_dir(&self, cid: &str, tid: &str) -> std::io::Result<PathBuf> {
        let path = self
            .base_path
            .join("conversations")
            .join(cid)
            .join("turns")
            .join(tid);
        tokio::fs::create_dir_all(&path).await?;
        tokio::fs::create_dir_all(path.join("blobs")).await?;
        Ok(path)
    }

    pub async fn write_conversation(
        &self,
        cid: &str,
        summary: &ConversationSummary,
    ) -> crate::types::Result<()> {
        let dir = self.ensure_conversation_dir(cid).await?;
        let path = dir.join("conversation.json");

        let mut to_write = summary.clone();

        // Try to merge with existing if it exists
        if let Ok(existing_content) = tokio::fs::read_to_string(&path).await {
            if let Ok(mut existing_summary) =
                serde_json::from_str::<ConversationSummary>(&existing_content)
            {
                // Keep the original created_at
                to_write.created_at_ms = existing_summary.created_at_ms;

                // Add or update turns
                for new_turn in summary.turns.iter() {
                    if let Some(pos) = existing_summary
                        .turns
                        .iter()
                        .position(|t| t.turn_id == new_turn.turn_id)
                    {
                        existing_summary.turns[pos] = new_turn.clone();
                    } else {
                        existing_summary.turns.push(new_turn.clone());
                    }
                }

                // Update timestamps and issues
                existing_summary.last_updated_ms = summary.last_updated_ms;
                existing_summary.issues = summary.issues.clone(); // In a real app we'd sum these, but for MVP this is fine

                to_write = existing_summary;
            }
        }

        let content = serde_json::to_string_pretty(&to_write)?;
        tokio::fs::write(path, content).await?;
        Ok(())
    }

    pub async fn write_turn(
        &self,
        cid: &str,
        tid: &str,
        detail: &TurnDetail,
    ) -> crate::types::Result<()> {
        let dir = self.ensure_turn_dir(cid, tid).await?;
        let path = dir.join("turn.json");
        let content = serde_json::to_string_pretty(detail)?;
        tokio::fs::write(path, content).await?;
        Ok(())
    }

    pub async fn read_turn(
        &self,
        cid: &str,
        tid: &str,
    ) -> crate::types::Result<Option<TurnDetail>> {
        let path = self
            .base_path
            .join("conversations")
            .join(cid)
            .join("turns")
            .join(tid)
            .join("turn.json");
        if !path.exists() {
            return Ok(None);
        }
        let content = tokio::fs::read_to_string(&path).await?;
        let detail = serde_json::from_str::<TurnDetail>(&content)?;
        Ok(Some(detail))
    }

    /// Merge two tag summaries by combining their registered/unregistered/leaks lists
    fn merge_tag_summaries(existing: &TagSummary, new: &TagSummary) -> TagSummary {
        let mut merged = TagSummary::default();

        // Merge registered tags
        let mut reg_map: std::collections::HashMap<String, TagOccurrence> = existing
            .registered
            .iter()
            .map(|t| (t.tag.clone(), t.clone()))
            .collect();
        for tag in &new.registered {
            reg_map.insert(tag.tag.clone(), tag.clone());
        }
        merged.registered = reg_map.into_values().collect();

        // Merge unregistered tags
        let mut unreg_map: std::collections::HashMap<String, TagOccurrence> = existing
            .unregistered
            .iter()
            .map(|t| (t.tag.clone(), t.clone()))
            .collect();
        for tag in &new.unregistered {
            unreg_map.insert(tag.tag.clone(), tag.clone());
        }
        merged.unregistered = unreg_map.into_values().collect();

        // Merge leaks
        let mut leak_map: std::collections::HashMap<String, TagOccurrence> = existing
            .leaks
            .iter()
            .map(|t| (t.tag.clone(), t.clone()))
            .collect();
        for tag in &new.leaks {
            leak_map.insert(tag.tag.clone(), tag.clone());
        }
        merged.leaks = leak_map.into_values().collect();

        merged
    }

    pub async fn merge_and_write_turn(
        &self,
        cid: &str,
        tid: &str,
        new_detail: &TurnDetail,
    ) -> crate::types::Result<()> {
        let mut detail = if let Some(existing) = self.read_turn(cid, tid).await? {
            existing
        } else {
            new_detail.clone()
        };

        // Merge stages: keep existing stages and add new ones that don't already exist
        for new_stage in &new_detail.stages {
            if !detail.stages.iter().any(|s| s.name == new_stage.name) {
                detail.stages.push(new_stage.clone());
            }
        }

        // Update other fields from new_detail
        detail.ended_at_ms = new_detail.ended_at_ms;
        detail.tool_calls = new_detail.tool_calls.clone();
        detail.tool_results = new_detail.tool_results.clone();
        // Merge cursor_tags instead of overwriting: combine registered/unregistered/leaks
        detail.cursor_tags =
            Self::merge_tag_summaries(&detail.cursor_tags, &new_detail.cursor_tags);
        detail.issues = new_detail.issues.clone();
        detail.trace_id = new_detail.trace_id.clone();
        detail.span_summary = new_detail.span_summary.clone();
        // Update role if provided (e.g. switching from User -> Assistant at end of stream)
        if new_detail.role.is_some() {
            detail.role = new_detail.role.clone();
        }
        // Preserve user_query from existing if new_detail doesn't have it
        if new_detail.user_query.is_some() {
            detail.user_query = new_detail.user_query.clone();
        }
        // Preserve user_query_tags from existing if new_detail doesn't have it
        if new_detail.user_query_tags.is_some() {
            detail.user_query_tags = new_detail.user_query_tags.clone();
        }

        self.write_turn(cid, tid, &detail).await
    }

    pub async fn add_stage(
        &self,
        cid: &str,
        tid: &str,
        stage_name: &str,
        blob_ref: BlobRef,
        summary: serde_json::Value,
    ) -> crate::types::Result<()> {
        let mut detail = if let Some(existing) = self.read_turn(cid, tid).await? {
            existing
        } else {
            // Turn detail doesn't exist yet, skip adding stage
            return Ok(());
        };

        // Only add if stage doesn't already exist
        if !detail.stages.iter().any(|s| s.name == stage_name) {
            detail.stages.push(StageIndex {
                name: stage_name.to_string(),
                kind: StageKind::Snapshot,
                summary,
                blob_ref: Some(blob_ref),
            });
            self.write_turn(cid, tid, &detail).await?;
        }

        Ok(())
    }

    pub async fn write_blob(
        &self,
        cid: &str,
        tid: &str,
        blob_id: &str,
        content: &[u8],
    ) -> crate::types::Result<BlobRef> {
        // Enforce size limit before writing
        let _ = self.enforce_size_limit().await;

        let dir = self.ensure_turn_dir(cid, tid).await?;

        // Determine file extension based on content type
        let (file_name, content_type) = if let Ok(s) = std::str::from_utf8(content) {
            // Try to detect JSON
            if s.trim().starts_with('{') || s.trim().starts_with('[') {
                (format!("{}.json", blob_id), "application/json".to_string())
            } else {
                (
                    format!("{}.txt", blob_id),
                    "text/plain; charset=utf-8".to_string(),
                )
            }
        } else {
            (
                format!("{}.bin", blob_id),
                "application/octet-stream".to_string(),
            )
        };

        let path = dir.join("blobs").join(&file_name);
        tokio::fs::write(&path, content).await?;

        // Compute SHA256
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(content);
        let sha256_hex = format!("{:x}", hasher.finalize());

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(BlobRef {
            blob_id: blob_id.to_string(),
            content_type,
            approx_bytes: content.len() as u64,
            file_name: Some(file_name),
            sha256: Some(sha256_hex),
            written_at_ms: Some(now),
        })
    }

    pub async fn update_summaries(
        &self,
        cid: &str,
        tid: &str,
        detail: &TurnDetail,
    ) -> crate::types::Result<()> {
        let summary_path = self
            .base_path
            .join("conversations")
            .join(cid)
            .join("conversation.json");
        let mut summary = if summary_path.exists() {
            let content = tokio::fs::read_to_string(&summary_path).await?;
            serde_json::from_str::<ConversationSummary>(&content)?
        } else {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            ConversationSummary {
                conversation_id: cid.to_string(),
                created_at_ms: now,
                last_updated_ms: now,
                turns: Vec::new(),
                issues: IssueCounts::default(),
            }
        };

        let turn_summary = TurnSummary {
            turn_id: tid.to_string(),
            request_id: detail.request_id.clone(),
            model_id: detail.model_id.clone(),
            flavor: detail.flavor.clone(),
            started_at_ms: detail.started_at_ms,
            ended_at_ms: detail.ended_at_ms,
            issues: Self::sum_issues(&detail.issues),
            role: detail.role.clone(),
        };

        // Update or add turn
        if let Some(pos) = summary.turns.iter().position(|t| t.turn_id == tid) {
            summary.turns[pos] = turn_summary;
        } else {
            summary.turns.push(turn_summary);
        }

        summary.last_updated_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Recalculate global issue counts
        summary.issues = summary
            .turns
            .iter()
            .fold(IssueCounts::default(), |mut acc, t| {
                acc.tool_args_empty += t.issues.tool_args_empty;
                acc.tool_args_repaired += t.issues.tool_args_repaired;
                acc.rescue_used += t.issues.rescue_used;
                acc.tag_unregistered += t.issues.tag_unregistered;
                acc.tag_leak_echo += t.issues.tag_leak_echo;
                acc.reasoning_leak += t.issues.reasoning_leak;
                acc.tool_args_invalid += t.issues.tool_args_invalid;
                acc.tool_call_duplicate_id += t.issues.tool_call_duplicate_id;
                acc
            });

        self.write_conversation(cid, &summary).await?;
        self.write_turn(cid, tid, detail).await?;
        Ok(())
    }

    fn sum_issues(issues: &[Issue]) -> IssueCounts {
        let mut counts = IssueCounts::default();
        for issue in issues {
            match issue.kind.as_str() {
                "ToolArgsEmptySuspicious" => counts.tool_args_empty += 1,
                "ToolArgsRepaired" => counts.tool_args_repaired += 1,
                "RescueUsed" => counts.rescue_used += 1,
                "CursorTagUnregistered" => counts.tag_unregistered += 1,
                "CursorTagLeakEcho" => counts.tag_leak_echo += 1,
                "ReasoningLeakSuspected" => counts.reasoning_leak += 1,
                "ToolArgsInvalid" => counts.tool_args_invalid += 1,
                "ToolCallDuplicateId" => counts.tool_call_duplicate_id += 1,
                _ => {}
            }
        }
        counts
    }

    /// Public version of sum_issues for use in streaming.rs
    pub fn sum_issues_public(issues: &[Issue]) -> IssueCounts {
        Self::sum_issues(issues)
    }

    /// Update only the conversation summary without rewriting turn.json
    pub async fn update_conversation_summary_only(
        &self,
        cid: &str,
        turn_summary: TurnSummary,
    ) -> crate::types::Result<()> {
        let summary_path = self
            .base_path
            .join("conversations")
            .join(cid)
            .join("conversation.json");
        let mut summary = if summary_path.exists() {
            let content = tokio::fs::read_to_string(&summary_path).await?;
            serde_json::from_str::<ConversationSummary>(&content)?
        } else {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            ConversationSummary {
                conversation_id: cid.to_string(),
                created_at_ms: now,
                last_updated_ms: now,
                turns: Vec::new(),
                issues: IssueCounts::default(),
            }
        };

        // Update or add turn summary
        if let Some(pos) = summary
            .turns
            .iter()
            .position(|t| t.turn_id == turn_summary.turn_id)
        {
            summary.turns[pos] = turn_summary;
        } else {
            summary.turns.push(turn_summary);
        }

        summary.last_updated_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Recalculate global issue counts
        summary.issues = summary
            .turns
            .iter()
            .fold(IssueCounts::default(), |mut acc, t| {
                acc.tool_args_empty += t.issues.tool_args_empty;
                acc.tool_args_repaired += t.issues.tool_args_repaired;
                acc.rescue_used += t.issues.rescue_used;
                acc.tag_unregistered += t.issues.tag_unregistered;
                acc.tag_leak_echo += t.issues.tag_leak_echo;
                acc.reasoning_leak += t.issues.reasoning_leak;
                acc.tool_args_invalid += t.issues.tool_args_invalid;
                acc.tool_call_duplicate_id += t.issues.tool_call_duplicate_id;
                acc
            });

        self.write_conversation(cid, &summary).await
    }

    pub fn detect_issues(
        &self,
        turn: &TurnRecord,
        _stages: &[StageIndex],
        cursor_tags: &TagSummary,
    ) -> Vec<Issue> {
        let mut issues = Vec::new();
        let mut tool_call_ids = std::collections::HashSet::new();

        // 1. ToolArgsEmptySuspicious & ToolCallDuplicateId
        for part in &turn.content {
            if let crate::types::MessagePart::ToolCall {
                id,
                name,
                arguments,
                ..
            } = part
            {
                // Check for duplicate IDs
                if !tool_call_ids.insert(id.clone()) {
                    issues.push(Issue {
                        kind: "ToolCallDuplicateId".to_string(),
                        severity: "error".to_string(),
                        message: format!("Duplicate tool call ID: '{}'", id),
                        context: serde_json::json!({ "tool_id": id, "tool_name": name }),
                    });
                }

                // Check for empty args on suspicious tools - ONLY FOR ASSISTANT ROLES
                if turn.role != crate::types::Role::User {
                    let is_empty = arguments.as_object().map(|m| m.is_empty()).unwrap_or(false);
                    if is_empty && self.is_suspicious_tool(name) {
                        issues.push(Issue {
                            kind: "ToolArgsEmptySuspicious".to_string(),
                            severity: "warning".to_string(),
                            message: format!("Tool '{}' called with empty arguments", name),
                            context: serde_json::json!({ "tool_id": id, "tool_name": name }),
                        });
                    }
                }
            }
        }

        // 2. ReasoningLeakSuspected (scan both Thought and Text parts)
        let mut all_content = String::new();
        for part in &turn.content {
            match part {
                crate::types::MessagePart::Thought { content } => {
                    all_content.push_str(content);
                    all_content.push('\n');
                }
                crate::types::MessagePart::Text { content, .. } => {
                    all_content.push_str(content);
                    all_content.push('\n');
                }
                _ => {}
            }
        }

        if !all_content.is_empty() {
            let leak_patterns = [
                ("<think", "XML think tag"),
                ("</think>", "XML think tag"),
                ("Reasoning:", "Reasoning prefix"),
                ("Thought:", "Thought prefix"),
                ("Chain-of-thought", "Chain-of-thought pattern"),
                ("<reasoning", "XML reasoning tag"),
                ("</reasoning>", "XML reasoning tag"),
            ];

            for (pattern, description) in &leak_patterns {
                if all_content.contains(pattern) {
                    issues.push(Issue {
                        kind: "ReasoningLeakSuspected".to_string(),
                        severity: "warning".to_string(),
                        message: format!("Suspected reasoning leak: found '{}' ({})", pattern, description),
                        context: serde_json::json!({ "pattern": pattern, "description": description }),
                    });
                    break; // Only report once per turn
                }
            }
        }

        // 3. CursorTagUnregistered - report unregistered tags found in ingress
        for tag_occ in &cursor_tags.unregistered {
            issues.push(Issue {
                kind: "CursorTagUnregistered".to_string(),
                severity: "info".to_string(),
                message: format!(
                    "Unregistered Cursor tag found: <{}> ({} occurrences)",
                    tag_occ.tag, tag_occ.count
                ),
                context: serde_json::json!({ "tag": tag_occ.tag, "count": tag_occ.count }),
            });
        }

        // 4. CursorTagLeakEcho - report registered tags that leaked to model output
        for tag_occ in &cursor_tags.leaks {
            issues.push(Issue {
                kind: "CursorTagLeakEcho".to_string(),
                severity: "warning".to_string(),
                message: format!(
                    "Cursor tag leaked to model output: <{}> ({} occurrences)",
                    tag_occ.tag, tag_occ.count
                ),
                context: serde_json::json!({ "tag": tag_occ.tag, "count": tag_occ.count }),
            });
        }

        issues
    }

    fn is_suspicious_tool(&self, name: &str) -> bool {
        matches!(
            name,
            "read_file"
                | "grep"
                | "glob_file_search"
                | "list_dir"
                | "codebase_search"
                | "run_terminal_cmd"
        )
    }

    /// Index tool calls and results from a finalized TurnRecord.
    pub fn index_tool_calls(
        turn: &TurnRecord,
        final_blob_ref: Option<BlobRef>,
    ) -> (Vec<ToolCallIndex>, Vec<ToolResultIndex>) {
        let mut tool_calls = Vec::new();
        let mut tool_results = Vec::new();

        for part in &turn.content {
            match part {
                crate::types::MessagePart::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    let args_status = if let Some(args_obj) = arguments.as_object() {
                        if args_obj.is_empty() {
                            ToolArgsStatus::Empty
                        } else {
                            ToolArgsStatus::Ok
                        }
                    } else {
                        ToolArgsStatus::Ok
                    };

                    // Create a short snippet from arguments for evidence
                    let snippet = if let Some(args_obj) = arguments.as_object() {
                        let keys: Vec<String> =
                            args_obj.keys().take(3).map(|k| k.to_string()).collect();
                        if keys.is_empty() {
                            "(empty args)".to_string()
                        } else {
                            format!("args: {}", keys.join(", "))
                        }
                    } else {
                        "(no args)".to_string()
                    };

                    // Store the full JSON arguments for the UI to parse
                    let raw_arguments_json = arguments.to_string();

                    let origin = match turn.role {
                        crate::types::Role::User => ToolCallOrigin::Ingress,
                        crate::types::Role::Assistant | crate::types::Role::Model => {
                            ToolCallOrigin::UpstreamStream
                        }
                        _ => ToolCallOrigin::Unknown,
                    };

                    tool_calls.push(ToolCallIndex {
                        id: id.clone(),
                        name: name.clone(),
                        args_status,
                        origin,
                        evidence: ToolCallEvidence {
                            stage: "final".to_string(),
                            message_index: None,
                            snippet: Some(snippet),
                            blob_ref: final_blob_ref.clone(),
                            offsets: None,
                            request_tool_index: None,
                            response_tool_index: None,
                            raw_arguments_snippet: Some(raw_arguments_json),
                        },
                    });
                }
                crate::types::MessagePart::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    name,
                    ..
                } => {
                    // Truncate content for snippet
                    let snippet = if content.len() > 100 {
                        format!("{}...", &content[..100])
                    } else {
                        content.clone()
                    };

                    tool_results.push(ToolResultIndex {
                        tool_call_id: tool_call_id.clone(),
                        name: name.clone(),
                        snippet,
                        is_error: *is_error,
                        blob_ref: final_blob_ref.clone(),
                    });
                }
                _ => {}
            }
        }

        (tool_calls, tool_results)
    }

    /// Extract cursor tags from ingress and final content with stage attribution.
    /// Populates registered, unregistered, and leaks categories with offsets.
    pub fn extract_cursor_tags(
        ingress_payload: &serde_json::Value,
        final_turn: Option<&TurnRecord>,
    ) -> TagSummary {
        let mut summary = TagSummary::default();
        let tag_registry = crate::tag_extract::TagRegistry::default();

        // Extract from ingress payload (as string)
        let ingress_str = ingress_payload.to_string();
        let ingress_tags = crate::tag_extract::extract_tags(&ingress_str);

        // Extract from final turn content (if available)
        let mut final_str = String::new();
        if let Some(turn) = final_turn {
            for part in &turn.content {
                match part {
                    crate::types::MessagePart::Text { content, .. } => {
                        final_str.push_str(content);
                        final_str.push('\n');
                    }
                    crate::types::MessagePart::Thought { content } => {
                        final_str.push_str(content);
                        final_str.push('\n');
                    }
                    _ => {}
                }
            }
        }
        let final_tags = crate::tag_extract::extract_tags(&final_str);

        // Track tags by stage and whether they're in model output
        let mut tag_map: std::collections::HashMap<String, Vec<(String, usize, usize, usize)>> =
            std::collections::HashMap::new();

        // Ingress tags (stage="ingress_raw")
        for extracted in ingress_tags {
            let entry = tag_map.entry(extracted.tag).or_default();
            entry.push((
                "ingress_raw".to_string(),
                extracted.start_offset,
                extracted.end_offset,
                0,
            ));
        }

        // Final tags (stage="final", these are model output)
        for extracted in final_tags {
            let entry = tag_map.entry(extracted.tag).or_default();
            entry.push((
                "final".to_string(),
                extracted.start_offset,
                extracted.end_offset,
                1,
            ));
        }

        // Categorize into registered/unregistered/leaks
        for (tag, occurrences) in tag_map {
            let is_registered = tag_registry.is_registered(&tag);
            let is_leak = is_registered
                && occurrences
                    .iter()
                    .any(|(stage, _, _, is_output)| stage == "final" && *is_output == 1);

            let count = occurrences.len() as u32;
            let locations: Vec<TagLocation> = occurrences
                .into_iter()
                .map(|(stage, start, end, _)| {
                    let snippet_start = start.saturating_sub(20);
                    let snippet_end = (end + 20).min(if stage == "final" {
                        final_str.len()
                    } else {
                        ingress_str.len()
                    });
                    let snippet = if stage == "final" {
                        final_str
                            .get(snippet_start..snippet_end)
                            .unwrap_or("")
                            .to_string()
                    } else {
                        ingress_str
                            .get(snippet_start..snippet_end)
                            .unwrap_or("")
                            .to_string()
                    };

                    TagLocation {
                        stage,
                        message_index: None,
                        offsets: Some(Offsets { start, end }),
                        snippet,
                        blob_ref: None,
                    }
                })
                .collect();

            let occurrence = TagOccurrence {
                tag: tag.clone(),
                count,
                locations,
            };

            if is_leak {
                summary.leaks.push(occurrence);
            } else if is_registered {
                summary.registered.push(occurrence);
            } else {
                summary.unregistered.push(occurrence);
            }
        }

        summary
    }

    /// Extract the user query from the ingress payload.
    /// Looks for the last user message with <user_query> tags or just the last user message.
    pub fn extract_user_query(ingress_payload: &serde_json::Value) -> Option<String> {
        let messages = ingress_payload.get("messages")?.as_array()?;

        // Find the last user message
        for msg in messages.iter().rev() {
            if msg.get("role")?.as_str()? == "user" {
                let content = msg.get("content")?;
                let content_str = if content.is_string() {
                    content.as_str()?.to_string()
                } else if content.is_array() {
                    // Handle array of content parts (some APIs use this)
                    content
                        .as_array()?
                        .iter()
                        .filter_map(|part| {
                            if part.get("type")?.as_str()? == "text" {
                                part.get("text")?.as_str()
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    continue;
                };

                // Try to extract from <user_query> tags
                if let Some(start) = content_str.find("<user_query>") {
                    if let Some(end) = content_str.find("</user_query>") {
                        let query = &content_str[start + 12..end];
                        return Some(query.trim().to_string());
                    }
                }

                // Otherwise return the whole content (truncated if too long)
                // We extended this limit to ensure tag extraction works correctly on larger contexts.
                // The UI should handle scrolling/truncation if needed.
                let truncated = if content_str.len() > 100_000 {
                    format!("{}... (truncated)", &content_str[..100_000])
                } else {
                    content_str
                };
                return Some(truncated);
            }
        }

        None
    }

    /// Compute tag deltas by comparing current user query with previous turn's user query
    pub async fn compute_user_query_tag_deltas(
        &self,
        cid: &str,
        current_user_query: &str,
    ) -> Option<Vec<crate::tag_extract::TagDelta>> {
        // Extract tags from current user query
        let current_tags = crate::tag_extract::extract_tags(current_user_query);

        // Find the previous turn with a user query
        let conversation_path = self.base_path.join("conversations").join(cid);
        let turns_dir = conversation_path.join("turns");

        if !turns_dir.exists() {
            // First turn, all tags are new
            return Some(crate::tag_extract::compute_tag_deltas(&current_tags, &[]));
        }

        // Read conversation.json to get turn order
        let conversation_json_path = conversation_path.join("conversation.json");
        if let Ok(conversation_json) = tokio::fs::read_to_string(&conversation_json_path).await {
            if let Ok(conversation) =
                serde_json::from_str::<ConversationSummary>(&conversation_json)
            {
                // Find the last user turn
                for turn in conversation.turns.iter().rev() {
                    if turn.role.as_deref() == Some("User") {
                        // Try to read this turn's detail
                        if let Ok(Some(turn_detail)) = self.read_turn(cid, &turn.turn_id).await {
                            if let Some(prev_query) = turn_detail.user_query {
                                let previous_tags = crate::tag_extract::extract_tags(&prev_query);
                                return Some(crate::tag_extract::compute_tag_deltas(
                                    &current_tags,
                                    &previous_tags,
                                ));
                            }
                        }
                    }
                }
            }
        }

        // No previous user turn found, all tags are new
        Some(crate::tag_extract::compute_tag_deltas(&current_tags, &[]))
    }

    /// Calculate total disk usage of debug_capture directory
    pub async fn calculate_total_size(&self) -> std::io::Result<u64> {
        let conversations_dir = self.base_path.join("conversations");
        if !conversations_dir.exists() {
            return Ok(0);
        }

        let mut total_size = 0u64;
        let mut stack = vec![conversations_dir];

        while let Some(dir) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let metadata = tokio::fs::metadata(&path).await?;

                if metadata.is_file() {
                    total_size += metadata.len();
                } else if metadata.is_dir() {
                    stack.push(path);
                }
            }
        }

        Ok(total_size)
    }

    /// List all conversations sorted by last_updated_ms (oldest first)
    pub async fn list_conversations_by_age(&self) -> crate::types::Result<Vec<(String, u64)>> {
        let conversations_dir = self.base_path.join("conversations");
        if !conversations_dir.exists() {
            return Ok(Vec::new());
        }

        let mut conversations = Vec::new();
        let mut entries = tokio::fs::read_dir(&conversations_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let conversation_json = path.join("conversation.json");
            if !conversation_json.exists() {
                continue;
            }

            let content = tokio::fs::read_to_string(&conversation_json).await?;
            if let Ok(summary) = serde_json::from_str::<ConversationSummary>(&content) {
                conversations.push((summary.conversation_id, summary.last_updated_ms));
            }
        }

        // Sort by last_updated_ms (oldest first)
        conversations.sort_by_key(|(_, updated)| *updated);
        Ok(conversations)
    }

    /// Delete a conversation directory
    pub async fn delete_conversation(&self, cid: &str) -> std::io::Result<()> {
        let conversation_dir = self.base_path.join("conversations").join(cid);
        if conversation_dir.exists() {
            tokio::fs::remove_dir_all(&conversation_dir).await?;
            tracing::info!(
                "Deleted conversation {} to free space",
                &cid[..8.min(cid.len())]
            );
        }
        Ok(())
    }

    /// Enforce size limit by deleting oldest conversations
    pub async fn enforce_size_limit(&self) -> crate::types::Result<()> {
        let total_size = self.calculate_total_size().await?;

        if total_size <= self.max_size_bytes {
            return Ok(());
        }

        tracing::warn!(
            "Debug capture size {}MB exceeds limit {}MB, cleaning up...",
            total_size / (1024 * 1024),
            self.max_size_bytes / (1024 * 1024)
        );

        let conversations = self.list_conversations_by_age().await?;
        let mut current_size = total_size;

        for (cid, _) in conversations {
            if current_size <= self.max_size_bytes {
                break;
            }

            // Calculate conversation size before deleting
            let conv_dir = self.base_path.join("conversations").join(&cid);
            let conv_size = Self::calculate_dir_size(&conv_dir).await.unwrap_or(0);

            self.delete_conversation(&cid).await?;
            current_size = current_size.saturating_sub(conv_size);

            tracing::info!(
                "Freed {}KB, remaining size: {}MB",
                conv_size / 1024,
                current_size / (1024 * 1024)
            );
        }

        Ok(())
    }

    /// Helper to calculate directory size
    async fn calculate_dir_size(path: &Path) -> std::io::Result<u64> {
        let mut total_size = 0u64;
        let mut stack = vec![path.to_path_buf()];

        while let Some(dir) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let metadata = tokio::fs::metadata(&path).await?;

                if metadata.is_file() {
                    total_size += metadata.len();
                } else if metadata.is_dir() {
                    stack.push(path);
                }
            }
        }

        Ok(total_size)
    }

    /// Query trace events for a specific turn and build a span summary
    pub async fn build_span_summary(
        &self,
        _cid: &str,
        tid: &str,
        rid: &str,
    ) -> Option<Vec<SpanSummary>> {
        // Try to read the trace log file (typically parallax.log.YYYY-MM-DD)
        let log_dir = std::path::Path::new(".");
        let mut trace_events = Vec::new();

        // Scan for log files and extract events matching this turn
        if let Ok(entries) = std::fs::read_dir(log_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("parallax.log") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            for line in content.lines() {
                                // Try to parse as JSON trace event
                                if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
                                    // Check if this event belongs to our turn
                                    if event.get("request_id").and_then(|v| v.as_str()) == Some(rid)
                                        && event.get("turn_id").and_then(|v| v.as_str())
                                            == Some(tid)
                                    {
                                        trace_events.push(event);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if trace_events.is_empty() {
            return None;
        }

        // Build span summaries from collected events
        let mut spans = Vec::new();

        for event in trace_events.iter().take(50) {
            // Extract key fields from each event
            if let Some(level) = event.get("level").and_then(|l| l.as_str()) {
                if let Some(target) = event.get("target").and_then(|t| t.as_str()) {
                    spans.push(SpanSummary {
                        name: target.to_string(),
                        level: level.to_string(),
                        fields: event
                            .get("fields")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    });
                }
            }
        }

        if spans.is_empty() {
            None
        } else {
            Some(spans)
        }
    }

    /// Compute a structural diff between two JSON values
    pub fn compute_json_diff(
        old: &serde_json::Value,
        new: &serde_json::Value,
    ) -> serde_json::Value {
        match (old, new) {
            (serde_json::Value::Object(old_map), serde_json::Value::Object(new_map)) => {
                let mut diff = serde_json::Map::new();
                let mut all_keys = std::collections::HashSet::new();

                for key in old_map.keys() {
                    all_keys.insert(key.clone());
                }
                for key in new_map.keys() {
                    all_keys.insert(key.clone());
                }

                for key in all_keys {
                    let old_val = old_map.get(&key);
                    let new_val = new_map.get(&key);

                    match (old_val, new_val) {
                        (None, Some(v)) => {
                            diff.insert(key, json!({ "added": v }));
                        }
                        (Some(_), None) => {
                            diff.insert(key, json!({ "removed": true }));
                        }
                        (Some(o), Some(n)) if o != n => {
                            // For nested objects, recurse; for primitives, show both
                            if o.is_object() && n.is_object() {
                                diff.insert(key, Self::compute_json_diff(o, n));
                            } else {
                                diff.insert(key, json!({ "old": o, "new": n }));
                            }
                        }
                        _ => {} // No change
                    }
                }

                serde_json::Value::Object(diff)
            }
            (old, new) if old != new => {
                json!({ "old": old, "new": new })
            }
            _ => serde_json::Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_tool_calls() {
        let turn = crate::types::TurnRecord {
            role: crate::types::Role::Assistant,
            content: vec![
                crate::types::MessagePart::Text {
                    content: "I'll help you".to_string(),
                    cache_control: None,
                },
                crate::types::MessagePart::ToolCall {
                    id: "call_123".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({ "path": "/src/main.rs" }),
                    signature: None,
                    metadata: serde_json::json!({}),
                    cache_control: None,
                },
            ],
            tool_call_id: None,
        };

        let blob_ref = BlobRef {
            blob_id: "final".to_string(),
            content_type: "application/json".to_string(),
            approx_bytes: 1024,
            file_name: Some("final.json".to_string()),
            sha256: Some("abc123".to_string()),
            written_at_ms: Some(1234567890),
        };

        let (tool_calls, _tool_results) = BundleManager::index_tool_calls(&turn, Some(blob_ref));

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "read_file");
        assert_eq!(tool_calls[0].id, "call_123");
        assert_eq!(tool_calls[0].args_status, ToolArgsStatus::Ok);
        assert_eq!(tool_calls[0].origin, ToolCallOrigin::UpstreamStream);
    }

    #[test]
    fn test_index_empty_tool_args() {
        let turn = crate::types::TurnRecord {
            role: crate::types::Role::Assistant,
            content: vec![crate::types::MessagePart::ToolCall {
                id: "call_456".to_string(),
                name: "grep".to_string(),
                arguments: serde_json::json!({}),
                signature: None,
                metadata: serde_json::json!({}),
                cache_control: None,
            }],
            tool_call_id: None,
        };

        let (tool_calls, _tool_results) = BundleManager::index_tool_calls(&turn, None);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].args_status, ToolArgsStatus::Empty);
    }
}
