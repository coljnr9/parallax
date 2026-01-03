use crate::specs::openai::*;
use crate::types::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Google,
    Anthropic,
    OpenAi,
    Standard,
}

impl ProviderKind {
    /// Detect provider kind from a model name
    pub fn from_model_name(model_name: &str) -> Self {
        let lower = model_name.to_lowercase();
        match lower {
            m if m.contains("gpt") || m.contains("openai/") => ProviderKind::OpenAi,
            m if m.contains("claude") || m.contains("anthropic/") => ProviderKind::Anthropic,
            m if m.contains("gemini") || m.contains("google/") => ProviderKind::Google,
            _ => ProviderKind::Standard,
        }
    }
}

pub trait ProviderFlavor: Send + Sync {
    fn requires_thought_signatures(&self) -> bool;
    fn name(&self) -> &'static str;
    fn kind(&self) -> ProviderKind;
    #[allow(dead_code)]
    fn supports_system_role(&self) -> bool {
        true
    }
    #[allow(dead_code)]
    fn max_tokens_mandatory(&self) -> bool {
        false
    }
    fn stop_sequences(&self) -> Vec<String> {
        vec![
            "</tool_code>".to_string(),
            "User:".to_string(),
            "<|user|>".to_string(),
            "Observation:".to_string(),
        ]
    }
}

pub struct GeminiFlavor;
impl ProviderFlavor for GeminiFlavor {
    fn requires_thought_signatures(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "google"
    }
    fn kind(&self) -> ProviderKind {
        ProviderKind::Google
    }
}

pub struct AnthropicFlavor;
impl ProviderFlavor for AnthropicFlavor {
    fn requires_thought_signatures(&self) -> bool {
        false
    }
    fn name(&self) -> &'static str {
        "anthropic"
    }
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }
    fn supports_system_role(&self) -> bool {
        false
    } // System at root, not in messages
    fn max_tokens_mandatory(&self) -> bool {
        true
    }
}

pub struct OpenAiFlavor;
impl ProviderFlavor for OpenAiFlavor {
    fn requires_thought_signatures(&self) -> bool {
        false
    }
    fn name(&self) -> &'static str {
        "openai"
    }
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAi
    }
}

pub struct StandardFlavor;
impl ProviderFlavor for StandardFlavor {
    fn requires_thought_signatures(&self) -> bool {
        false
    }
    fn name(&self) -> &'static str {
        "standard"
    }
    fn kind(&self) -> ProviderKind {
        ProviderKind::Standard
    }
}

pub fn resolve_flavor_for_kind(kind: ProviderKind) -> Box<dyn ProviderFlavor> {
    match kind {
        ProviderKind::Google => Box::new(GeminiFlavor),
        ProviderKind::Anthropic => Box::new(AnthropicFlavor),
        ProviderKind::OpenAi => Box::new(OpenAiFlavor),
        ProviderKind::Standard => Box::new(StandardFlavor),
    }
}

pub struct OpenRouterAdapter;

impl OpenRouterAdapter {
    pub async fn project(
        context: &ConversationContext,
        model_id: &str,
        flavor: &dyn ProviderFlavor,
        db: &crate::db::DbPool,
        _intent: Option<crate::tui::Intent>,
    ) -> OpenAiRequest {
        tracing::info!("[⚙️  -> ⚙️ ] Projecting turn for model: {}", model_id);
        let is_thinking = model_id.contains("thinking") || model_id.contains("claude-3.7");

        // Extract and prune history if needed for Google models
        let pruned_context = Self::prune_history_if_needed(context, flavor);

        let messages = Self::transform_messages(&pruned_context, flavor, db).await;

        let (max_tokens, extra) = Self::extract_request_config(context, is_thinking);

        let stop = Some(flavor.stop_sequences());

        let tools = Self::extract_tools(&context.extra_body);
        let tool_choice = context
            .extra_body
            .get("tool_choice")
            .map(Self::project_tool_choice);

        OpenAiRequest {
            model: model_id.to_string(),
            messages,
            stream: Some(true),
            temperature: context
                .extra_body
                .get("temperature")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
            top_p: context
                .extra_body
                .get("top_p")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
            max_tokens,
            tools,
            tool_choice,
            stop,
            extra,
        }
    }

    fn prune_history_if_needed(
        context: &ConversationContext,
        flavor: &dyn ProviderFlavor,
    ) -> ConversationContext {
        let mut history = context.history.clone();

        if flavor.kind() == ProviderKind::Google {
            let analysis = crate::history_pruning::HistoryDepthAnalysis::analyze(&history);
            if analysis.exceeds_google_limits() {
                tracing::warn!(
                    "[HISTORY-PRUNE] Google model exceeds limits: depth={}, turns={}, pruning/flattening...",
                    analysis.estimated_json_depth,
                    analysis.total_turns
                );
                // Use Flattening strategy for Google to reduce recursion depth
                history = crate::history_pruning::prune_history(
                    history,
                    crate::history_pruning::PruningStrategy::Flattening,
                    50,
                );
            } else if analysis.approaching_google_limits() {
                tracing::info!(
                    "[HISTORY-PRUNE] Google model approaching limits: depth={}, turns={}",
                    analysis.estimated_json_depth,
                    analysis.total_turns
                );
            }
        }

        // Create a modified context with pruned history
        let mut pruned_context = context.clone();
        pruned_context.history = history;
        pruned_context
    }

    fn extract_tools(extra_body: &serde_json::Value) -> Option<Vec<OpenAiTool>> {
        let raw_tools = extra_body.get("tools")?.as_array()?;
        let mut projected_tools = Vec::new();

        for t in raw_tools {
            if let Some(obj) = t.as_object() {
                // Determine if it's already in OpenAI format or needs transformation
                let name = obj
                    .get("name")
                    .or_else(|| obj.get("function").and_then(|f| f.get("name")));
                let description = obj
                    .get("description")
                    .or_else(|| obj.get("function").and_then(|f| f.get("description")));
                let parameters = obj
                    .get("parameters")
                    .or_else(|| obj.get("input_schema"))
                    .or_else(|| obj.get("function").and_then(|f| f.get("parameters")));

                if let (Some(n), Some(p)) = (name.and_then(|v| v.as_str()), parameters) {
                    let mut final_params = p.clone();

                    // PATCH: Fix grep schema for models that get confused
                    if n == "grep" {
                        if let Some(props) = final_params
                            .get_mut("properties")
                            .and_then(|v| v.as_object_mut())
                        {
                            // Ripgrep treats -C as mutually exclusive with -A and -B.
                            // We remove -C from the schema to force the model to use -A/-B if it wants context,
                            // or we could rewrite the descriptions. Removing -C is the safest way to
                            // ensure the model doesn't send conflicting flags.
                            props.remove("-C");
                        }
                    }

                    projected_tools.push(OpenAiTool {
                        r#type: "function".to_string(),
                        function: OpenAiFunctionDefinition {
                            name: n.to_string(),
                            description: description
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            parameters: final_params,
                            extra: HashMap::new(),
                        },
                        extra: HashMap::new(),
                    });
                }
            }
        }

        if projected_tools.is_empty() {
            None
        } else {
            Some(projected_tools)
        }
    }

    async fn transform_messages(
        context: &ConversationContext,
        flavor: &dyn ProviderFlavor,
        db: &crate::db::DbPool,
    ) -> Vec<OpenAiMessage> {
        let mut messages = Vec::new();
        let history_len = context.history.len();

        for (i, record) in context.history.iter().enumerate() {
            let _is_last_turn = i == history_len - 1;
            let is_cache_breakpoint = flavor.kind() == ProviderKind::Anthropic
                && i > 0
                && (i == history_len - 3 || i == history_len - 5);

            let msg = match record.role {
                Role::System | Role::Developer => Self::transform_system_message(record, flavor),
                Role::User => Self::transform_user_message(record, flavor, is_cache_breakpoint),
                Role::Assistant | Role::Model => {
                    Self::transform_assistant_message(record, flavor, db).await
                }
                Role::Tool => Self::transform_tool_message(record, context, is_cache_breakpoint),
            };

            // Validation for Gemini: log if we're creating problematic messages
            if flavor.kind() == ProviderKind::Google {
                if let OpenAiMessage::Assistant {
                    content,
                    tool_calls,
                    ..
                } = &msg
                {
                    if !tool_calls.is_empty() && content.is_none() {
                        tracing::warn!(
                            "[GEMINI-COMPAT] Assistant message at index {} has tool_calls but no content. \
                             This should have been fixed by transform_assistant_message.",
                            i
                        );
                    }
                }
            }

            messages.push(msg);
        }
        messages
    }

    fn transform_system_message(record: &TurnRecord, flavor: &dyn ProviderFlavor) -> OpenAiMessage {
        OpenAiMessage::System {
            content: Self::content_to_text(&record.content),
            cache_control: if flavor.kind() == ProviderKind::Anthropic {
                Some(serde_json::json!({ "type": "ephemeral" }))
            } else {
                None
            },
        }
    }

    fn transform_user_message(
        record: &TurnRecord,
        _flavor: &dyn ProviderFlavor,
        is_cache_breakpoint: bool,
    ) -> OpenAiMessage {
        let cache_control = if is_cache_breakpoint {
            Some(serde_json::json!({ "type": "ephemeral" }))
        } else {
            None
        };

        OpenAiMessage::User {
            content: if cache_control.is_some() {
                OpenAiContent::Parts(vec![OpenAiContentPart::Text {
                    text: Self::content_to_text(&record.content),
                    cache_control,
                }])
            } else {
                OpenAiContent::String(Self::content_to_text(&record.content))
            },
        }
    }

    async fn transform_assistant_message(
        record: &TurnRecord,
        flavor: &dyn ProviderFlavor,
        db: &crate::db::DbPool,
    ) -> OpenAiMessage {
        let mut tool_calls = Vec::new();
        let mut thoughts = Vec::new();
        let mut text_parts = Vec::new();

        for part in &record.content {
            match part {
                MessagePart::Text { content, .. } => {
                    text_parts.push(content.clone());
                }
                MessagePart::Thought { content } => {
                    thoughts.push(content.clone());
                }
                MessagePart::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    let (thought_signature, extra_content) =
                        Self::handle_assistant_tool_calls(id, name, arguments, flavor, db).await;

                    tool_calls.push(OpenAiToolCall {
                        id: id.clone(),
                        r#type: "function".to_string(),
                        function: OpenAiFunctionCall {
                            name: name.clone(),
                            arguments: arguments.to_string(),
                        },
                        thought_signature,
                        extra_content,
                    });
                }
                _ => {}
            }
        }

        let text_content = text_parts.join("\n");

        if tool_calls.is_empty() {
            if let Some(rescue) = crate::rescue::detect_xml_invoke(&text_content) {
                tracing::info!(
                    "[RESCUE-PROJECT] Converted XML in history to ToolCall: {}",
                    rescue.name
                );
                tool_calls.push(OpenAiToolCall {
                    id: match rescue.tool_call["id"].as_str() {
                        Some(s) if !s.is_empty() => s.to_string(),
                        _ => "gen_id".to_string(),
                    },
                    r#type: "function".to_string(),
                    function: OpenAiFunctionCall {
                        name: rescue.name,
                        arguments: match rescue.tool_call["function"]["arguments"].as_str() {
                            Some(s) if !s.is_empty() => s.to_string(),
                            _ => "{}".to_string(),
                        },
                    },
                    thought_signature: None,
                    extra_content: None,
                });
            }
        }

        let final_content = Self::apply_gemini_fix(&text_content, flavor, &tool_calls);

        OpenAiMessage::Assistant {
            content: final_content,
            reasoning: if thoughts.is_empty() { None } else { Some(thoughts.join("\n")) },
            tool_calls,
        }
    }

    fn apply_gemini_fix(text_content: &str, flavor: &dyn ProviderFlavor, tool_calls: &[OpenAiToolCall]) -> Option<String> {
        // GEMINI FIX: Gemini requires every message to have at least one "parts" field.
        // When an assistant message has tool calls but no text content, we must provide
        // at least an empty string to ensure OpenRouter can transform it into a valid
        // Gemini message with a parts array.
        if text_content.is_empty() {
            if flavor.kind() == ProviderKind::Google && !tool_calls.is_empty() {
                tracing::debug!(
                    "[GEMINI-COMPAT] Assistant message has tool_calls but no text content. \
                     Providing empty string to ensure valid Gemini parts field."
                );
                Some(String::new())
            } else {
                None
            }
        } else {
            Some(text_content.to_string())
        }
    }

    fn transform_tool_message(
        record: &TurnRecord,
        context: &ConversationContext,
        is_cache_breakpoint: bool,
    ) -> OpenAiMessage {
        let (tool_call_id, name) = match record.tool_call_id.as_ref() {
            Some(id) => {
                let name = record.content.iter().find_map(|p| {
                    if let MessagePart::ToolResult { name, .. } = p {
                        name.clone()
                    } else {
                        None
                    }
                });
                (id.clone(), name)
            }
            None => {
                let found = record.content.iter().find_map(|p| {
                    if let MessagePart::ToolResult {
                        tool_call_id,
                        name,
                        ..
                    } = p
                    {
                        Some((tool_call_id.clone(), name.clone()))
                    } else {
                        None
                    }
                });
                match found {
                    Some(pair) => pair,
                    None => ("missing_id".to_string(), None),
                }
            }
        };

        let final_name = match name {
            Some(n) => n,
            None => {
                let found_in_history = context.history.iter().find_map(|r| {
                    r.content.iter().find_map(|p| {
                        if let MessagePart::ToolCall { id, name, .. } = p {
                            if id == &tool_call_id {
                                Some(name.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                });
                match found_in_history {
                    Some(n) => n,
                    None => "unknown_tool".to_string(),
                }
            }
        };

        OpenAiMessage::Tool {
            content: Self::content_to_text(&record.content),
            tool_call_id,
            name: final_name,
            cache_control: if is_cache_breakpoint {
                Some(serde_json::json!({ "type": "ephemeral" }))
            } else {
                None
            },
        }
    }

    async fn handle_assistant_tool_calls(
        id: &str,
        _name: &str,
        _arguments: &serde_json::Value,
        flavor: &dyn ProviderFlavor,
        db: &crate::db::DbPool,
    ) -> (Option<String>, Option<serde_json::Value>) {
        let mut thought_signature = None;
        let mut extra_content = None;

        if flavor.requires_thought_signatures() {
            if let Ok(Some(sig_json)) =
                crate::engine::ParallaxEngine::load_signature_from_db(id, db).await
            {
                if let Ok(hub_sig) = serde_json::from_str::<HubSignature>(&sig_json) {
                    thought_signature = hub_sig.thought_signature;
                    if let Some(details) = hub_sig.reasoning_details {
                        extra_content = Some(serde_json::json!({ "reasoning_details": details }));
                    }
                }
            }
        }

        (thought_signature, extra_content)
    }
    fn extract_request_config(
        context: &ConversationContext,
        is_thinking: bool,
    ) -> (Option<u32>, HashMap<String, serde_json::Value>) {
        let mut extra = HashMap::new();
        if let Some(obj) = context.extra_body.as_object() {
            for (k, v) in obj {
                if matches!(
                    k.as_str(),
                    "model"
                        | "messages"
                        | "stream"
                        | "temperature"
                        | "top_p"
                        | "tools"
                        | "tool_choice"
                        | "max_tokens"
                        | "system"
                        | "stream_options"
                        | "metadata"
                ) {
                    continue;
                }
                if k == "extra_body" {
                    if let Some(inner) = v.as_object() {
                        for (ik, iv) in inner {
                            extra.insert(ik.clone(), iv.clone());
                        }
                    }
                    continue;
                }
                extra.insert(k.clone(), v.clone());
            }
        }

        let mut max_tokens = context
            .extra_body
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        if is_thinking {
            // Force at least 64k tokens for thinking models to prevent cutoffs
            let max_tokens_val = max_tokens.unwrap_or_default();
            if max_tokens_val < 64000 {
                max_tokens = Some(64000);
            }
        }

        (max_tokens, extra)
    }

    fn project_tool_choice(raw_choice: &serde_json::Value) -> serde_json::Value {
        if let Some(obj) = raw_choice.as_object() {
            if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
                if t == "auto" || t == "any" || t == "required" {
                    return serde_json::Value::String(if t == "any" {
                        "required".to_string()
                    } else {
                        t.to_string()
                    });
                }
                if t == "tool" {
                    if let Some(name) = obj.get("name") {
                        return serde_json::json!({
                            "type": "function",
                            "function": { "name": name }
                        });
                    }
                }
            }
        }
        raw_choice.clone()
    }

    fn content_to_text(content: &[MessagePart]) -> String {
        content
            .iter()
            .filter_map(|p| match p {
                MessagePart::Text { content, .. } => Some(content.clone()),
                MessagePart::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_anthropic_cache_projection() {
        let history = vec![
            TurnRecord {
                role: Role::System,
                content: vec![MessagePart::Text {
                    content: "System prompt".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
            TurnRecord {
                role: Role::User,
                content: vec![MessagePart::Text {
                    content: "Message 1".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
            TurnRecord {
                role: Role::Assistant,
                content: vec![MessagePart::Text {
                    content: "Response 1".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
            TurnRecord {
                role: Role::User,
                content: vec![MessagePart::Text {
                    content: "Message 2".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
            TurnRecord {
                role: Role::Assistant,
                content: vec![MessagePart::Text {
                    content: "Response 2".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
            TurnRecord {
                role: Role::User,
                content: vec![MessagePart::Text {
                    content: "Message 3".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
        ];

        let _context = ConversationContext {
            history,
            conversation_id: "test".to_string(),
            extra_body: json!({}),
        };

        let _flavor = AnthropicFlavor;
    }
}
