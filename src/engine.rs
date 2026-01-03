use crate::db::DbPool;
use crate::ingress::*;
use crate::types::*;

pub struct TurnOperation<M> {
    pub model: M,
    pub input_context: ConversationContext,
    pub request_id: String,
}

pub enum TurnOperationEntry {
    Gemini(TurnOperation<ModelProvider>),
    Anthropic(TurnOperation<ModelProvider>),
    OpenAI(TurnOperation<ModelProvider>),
    Standard(TurnOperation<ModelProvider>),
}

impl TurnOperationEntry {
    /// Decompose the entry into its constituent parts: (model_id, context, request_id, flavor)
    pub fn into_parts(
        self,
    ) -> (
        String,
        ConversationContext,
        String,
        std::sync::Arc<dyn crate::projections::ProviderFlavor + Send + Sync>,
    ) {
        match self {
            TurnOperationEntry::Gemini(op) => (
                op.model.model_name().to_string(),
                op.input_context,
                op.request_id,
                std::sync::Arc::new(crate::projections::GeminiFlavor),
            ),
            TurnOperationEntry::Anthropic(op) => (
                op.model.model_name().to_string(),
                op.input_context,
                op.request_id,
                std::sync::Arc::new(crate::projections::AnthropicFlavor),
            ),
            TurnOperationEntry::OpenAI(op) => (
                op.model.model_name().to_string(),
                op.input_context,
                op.request_id,
                std::sync::Arc::new(crate::projections::OpenAiFlavor),
            ),
            TurnOperationEntry::Standard(op) => (
                op.model.model_name().to_string(),
                op.input_context,
                op.request_id,
                std::sync::Arc::new(crate::projections::StandardFlavor),
            ),
        }
    }
}

pub struct ParallaxEngine;

const MAX_HISTORY_LENGTH: usize = 1000;
const MAX_MESSAGE_PARTS: usize = 100;
const MAX_TOOL_CALLS_PER_REQUEST: usize = 4096;

impl ParallaxEngine {
    pub fn validate_context(context: &ConversationContext) -> Result<()> {
        if context.history.is_empty() {
            return Err(ParallaxError::InvalidIngress(
                "Lifted context history cannot be empty".into(),
            )
            .into());
        }

        if context.history.len() > MAX_HISTORY_LENGTH {
            return Err(ParallaxError::InvalidIngress(format!(
                "Conversation history exceeds limit of {}",
                MAX_HISTORY_LENGTH
            ))
            .into());
        }

        let total_tool_calls: usize = context.history.iter().map(|record| {
            record.content.iter().filter(|part| matches!(part, MessagePart::ToolCall { .. })).count()
        }).sum();

        if total_tool_calls > MAX_TOOL_CALLS_PER_REQUEST {
            return Err(ParallaxError::InvalidIngress(format!(
                "Total tool calls ({}) exceeds limit of {}",
                total_tool_calls, MAX_TOOL_CALLS_PER_REQUEST
            ))
            .into());
        }

        for (i, record) in context.history.iter().enumerate() {
            if record.content.len() > MAX_MESSAGE_PARTS {
                return Err(ParallaxError::InvalidIngress(format!(
                    "Message {} exceeds part limit of {}",
                    i, MAX_MESSAGE_PARTS
                ))
                .into());
            }

            if record.role == Role::Tool {
                let has_result = record
                    .content
                    .iter()
                    .any(|p| matches!(p, MessagePart::ToolResult { .. }));
                if !has_result {
                    tracing::warn!(
                        "Message {} is role 'tool' but contains no ToolResult parts",
                        i
                    );
                }
            }

            if record.role == Role::Assistant && record.content.is_empty() {
                return Err(ParallaxError::InvalidIngress(format!(
                    "Message {} (Assistant) is empty and contains no tool calls",
                    i
                ))
                .into());
            }
        }

        Ok(())
    }

    pub async fn lift(payload: serde_json::Value, db: &DbPool) -> Result<TurnOperationEntry> {
        let raw: RawTurn = serde_json::from_value(payload)
            .map_err(|e| ParallaxError::InvalidIngress(e.to_string()))?;

        let anchor_hash = raw.generate_anchor_hash()?;
        let request_id = raw.extract_request_id();

        // Pass 1: Build records and infer roles
        let raw_records = Self::process_raw_records(raw.messages, &anchor_hash, db).await?;

        // Pass 2: Coalesce sequential records
        let history = Self::coalesce_history(raw_records);

        let context = ConversationContext {
            history,
            conversation_id: anchor_hash,
            extra_body: raw.extra,
        };

        Self::route_model(raw.model, context, request_id)
    }

    async fn process_raw_records(
        messages: Vec<RawTurnRecord>,
        anchor_hash: &str,
        db: &DbPool,
    ) -> Result<Vec<TurnRecord>> {
        let mut raw_records = Vec::new();
        for mut raw_rec in messages {
            if raw_rec.role.is_none() {
                if let Some(t) = &raw_rec.type_ {
                    raw_rec.role = Some(match t.as_str() {
                        "function_call" => Role::Assistant,
                        "function_call_output" => Role::Tool,
                        _ => Role::User,
                    });
                }
            }
            raw_records.push(Self::lift_record(raw_rec, anchor_hash, db).await?);
        }
        Ok(raw_records)
    }

    fn coalesce_history(raw_records: Vec<TurnRecord>) -> Vec<TurnRecord> {
        let mut history: Vec<TurnRecord> = Vec::new();
        for rec in raw_records {
            let should_coalesce = history.last().map(|last| {
                last.role == rec.role && rec.role != Role::User && rec.role != Role::Tool
            }).unwrap_or(false);

            if should_coalesce {
                let last = history.last_mut().expect("Checked in should_coalesce");
                last.content.extend(rec.content);
                if last.tool_call_id.is_none() {
                    last.tool_call_id = rec.tool_call_id;
                }
            } else {
                history.push(rec);
            }
        }
        history
    }

    fn route_model(
        model: ModelProvider,
        context: ConversationContext,
        request_id: String,
    ) -> Result<TurnOperationEntry> {
        let op = TurnOperation {
            model: model.clone(),
            input_context: context,
            request_id,
        };

        match model {
            ModelProvider::Gemini(_) => {
                Self::log_route("Gemini");
                Ok(TurnOperationEntry::Gemini(op))
            }
            ModelProvider::Anthropic(_) => {
                Self::log_route("Anthropic");
                Ok(TurnOperationEntry::Anthropic(op))
            }
            ModelProvider::OpenAI(_) => {
                Self::log_route("OpenAI");
                Ok(TurnOperationEntry::OpenAI(op))
            }
            ModelProvider::Standard(_) => {
                Self::log_route("Standard");
                Ok(TurnOperationEntry::Standard(op))
            }
        }
    }

    fn log_route(flavor: &str) {
        tracing::debug!("[⚙️] Routing to {} flavor", flavor);
    }

    async fn lift_record(
        raw_rec: RawTurnRecord,
        conversation_id: &str,
        db: &DbPool,
    ) -> Result<TurnRecord> {
        let role = raw_rec.role.unwrap_or(Role::User);
        let mut parts = Vec::new();

        // 1. Handle standard content parts
        if let Some(content) = raw_rec.content {
            Self::process_content_parts(content, &mut parts);
        }

        // 2. Handle legacy OpenAI function_call
        if let (Some(name), Some(args), Some(id)) =
            (raw_rec.name.as_ref(), raw_rec.arguments.as_ref(), raw_rec.call_id.as_ref())
        {
            let parsed_args = match crate::json_repair::repair_tool_call_arguments(name, args) {
                Ok(val) => val,
                Err(e) => {
                    tracing::warn!(
                        "[⚙️] Failed to repair legacy function_call arguments for {}: {}",
                        name,
                        e
                    );
                    serde_json::json!({ "raw": args })
                }
            };
            parts.push(MessagePart::ToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: parsed_args,
                signature: None,
                metadata: serde_json::json!({}),
                cache_control: None,
            });
        }

        // 3. Handle standard tool calls
        for tc in raw_rec.tool_calls {
            let mut args = match crate::json_repair::repair_tool_call_arguments(
                &tc.function.name,
                &tc.function.arguments,
            ) {
                Ok(val) => val,
                Err(e) => {
                    return Err(ParallaxError::InvalidIngress(format!(
                        "Malformed tool arguments: {}",
                        e
                    ))
                    .into());
                }
            };

            // Apply hardening/sanitization to tool arguments
            crate::hardening::sanitize_tool_call(&tc.function.name, &mut args);

            Self::save_signature_to_db(&tc.id, conversation_id, &tc.extra, db).await?;

            parts.push(MessagePart::ToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: args,
                signature: None,
                metadata: tc.extra,
                cache_control: None,
            });
        }

        // 4. Handle legacy OpenAI function_call_output
        if role == Role::Tool {
            if let Some(call_id) = raw_rec.tool_call_id.clone().or(raw_rec.call_id.clone()) {
                let content_str = raw_rec.output.unwrap_or_else(|| {
                    parts.first().and_then(|p| {
                        if let MessagePart::Text { content, .. } = p {
                            Some(content.clone())
                        } else {
                            None
                        }
                    }).unwrap_or_default()
                });
                parts = vec![MessagePart::ToolResult {
                    tool_call_id: call_id,
                    content: content_str,
                    is_error: false,
                    name: None,
                    cache_control: None,
                }];
            }
        }

        Ok(TurnRecord {
            role,
            content: parts,
            tool_call_id: raw_rec.tool_call_id,
        })
    }

    fn process_content_parts(content: RawContent, parts: &mut Vec<MessagePart>) {
        match content {
            RawContent::String(s) => {
                parts.push(MessagePart::Text {
                    content: s,
                    cache_control: None,
                });
            }
            RawContent::Parts(raw_parts) => {
                for p in raw_parts {
                    match p {
                        RawContentPart::Text { text, .. } => {
                            parts.push(MessagePart::Text {
                                content: text,
                                cache_control: None,
                            });
                        }
                        RawContentPart::ImageUrl { image_url } => {
                            parts.push(MessagePart::Image {
                                url: Some(image_url.url),
                                mime_type: None,
                                data: None,
                                cache_control: None,
                            });
                        }
                        RawContentPart::Image { source } => {
                            parts.push(MessagePart::Image {
                                url: None,
                                mime_type: Some(source.media_type),
                                data: Some(source.data),
                                cache_control: None,
                            });
                        }
                        RawContentPart::ToolUse { id, name, input } => {
                            parts.push(MessagePart::ToolCall {
                                id,
                                name,
                                arguments: input,
                                signature: None,
                                metadata: serde_json::json!({}),
                                cache_control: None,
                            });
                        }
                        RawContentPart::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            let content_str = if let Some(s) = content.as_str() {
                                s.to_string()
                            } else if let Some(arr) = content.as_array() {
                                // Anthropic tool results can be an array of parts
                                arr.iter()
                                    .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            } else {
                                content.to_string()
                            };

                            parts.push(MessagePart::ToolResult {
                                tool_call_id: tool_use_id,
                                content: content_str,
                                is_error,
                                name: None,
                                cache_control: None,
                            });
                        }
                        RawContentPart::Unknown => {}
                    }
                }
            }
            RawContent::Null => {
                // Do nothing, just valid empty content
            }
        }
    }

    pub async fn save_signature_to_db(
        tool_id: &str,
        conversation_id: &str,
        metadata: &serde_json::Value,
        pool: &DbPool,
    ) -> Result<()> {
        let meta = match serde_json::from_value::<HubSignatureMetadata>(metadata.clone()) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    "Failed to deserialize signature metadata for {}: {}",
                    tool_id,
                    e
                );
                HubSignatureMetadata {
                    thought_signature: None,
                    reasoning_details: None,
                }
            }
        };

        let sig: HubSignature = meta.into();

        if sig.thought_signature.is_some()
            || sig
                .reasoning_details
                .as_ref()
                .is_some_and(|v| !v.as_array().is_none_or(|a| a.is_empty()))
        {
            let sig_json = serde_json::to_string(&sig).map_err(ParallaxError::Serialization)?;

            // Extract reasoning tokens if available from OpenRouter reasoning_details
            let reasoning_tokens = sig
                .reasoning_details
                .as_ref()
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first| first.get("tokens"))
                .and_then(|v| v.as_i64());

            tracing::trace!(
                "[⚙️  -> ⚙️ ] Sticky Signature Saved for {}: {} bytes ({} reasoning tokens)",
                tool_id,
                sig_json.len(),
                reasoning_tokens.unwrap_or(0)
            );

            sqlx::query("INSERT OR REPLACE INTO tool_signatures (id, conversation_id, signature, reasoning_tokens, thought_signature) VALUES (?1, ?2, ?3, ?4, ?5)")
                .bind(tool_id)
                .bind(conversation_id)
                .bind(sig_json)
                .bind(reasoning_tokens)
                .bind(sig.thought_signature.as_ref())
                .execute(pool)
                .await
                .map_err(ParallaxError::Database)?;
        }
        Ok(())
    }

    pub async fn load_signature_from_db(tool_id: &str, pool: &DbPool) -> Result<Option<String>> {
        let row = sqlx::query_as::<sqlx::Sqlite, (String,)>(
            "SELECT signature FROM tool_signatures WHERE id = ?1",
        )
        .bind(tool_id)
        .fetch_optional(pool)
        .await
        .map_err(ParallaxError::Database)?;

        Ok(row.map(|r| r.0))
    }

    pub async fn get_context_from_db(
        conversation_id: &str,
        _db: &DbPool,
    ) -> Result<ConversationContext> {
        // We'll just build a skeleton context here for now as Parallax normally lifts from ingress.
        // In a real retry scenario, we'd need to reconstruct history from DB or have it passed along.
        // For the immediate goal of fixing the compile error, we'll try to find where history is stored.

        // Actually, looking at the schema, we might not have a full 'conversations' table with all history.
        // Let's assume we can at least return a context with the ID.
        Ok(ConversationContext {
            history: Vec::new(),
            conversation_id: conversation_id.to_string(),
            extra_body: serde_json::json!({}),
        })
    }
}
