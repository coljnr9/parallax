#![allow(clippy::manual_unwrap_or_default)]
#![allow(clippy::manual_unwrap_or)]
use crate::constants::{
    GEMINI_FLASH_FALLBACK, OPENROUTER_CHAT_COMPLETIONS, RETRYABLE_STATUS_CODES,
    TOOLS_REQUIRING_ARGS,
};
use crate::db::DbPool;
use crate::engine::ParallaxEngine;
use crate::types::LineEvent;
use crate::types::ProviderPulse;
use crate::types::*;
use crate::AppState;
use bytes::Bytes;
use futures_util::Stream;
use futures_util::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_util::codec::{FramedRead, LinesCodec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Initial,
    BufferingUntilToolOrDone,
    StreamingToClient,
    Finished,
}

pub struct StreamHandler;

const MAX_STREAM_LINES: usize = 100_000;

impl StreamHandler {
    #[allow(clippy::too_many_arguments)]
    async fn finalize_and_log_turn(
        finalized_turn: &TurnRecord,
        usage: Option<&Usage>,
        model_id: &str,
        conversation_id: &str,
        request_id: &str,
        start_time: std::time::Instant,
    ) {
        let finalized_turn_val = if let Ok(v) = serde_json::to_value(finalized_turn) {
            v
        } else {
            serde_json::Value::Null
        };
        crate::debug_utils::capture_debug_snapshot(
            "final",
            model_id,
            conversation_id,
            request_id,
            &finalized_turn_val,
        )
        .await;
        let usage_owned = usage.cloned();
        Self::log_finalized_turn(finalized_turn, &usage_owned, start_time.elapsed());
    }

    async fn persist_signatures(accumulator: &TurnAccumulator, conversation_id: &str, db: &DbPool) {
        for (tool_id, metadata_map) in &accumulator.signatures {
            if let Err(e) = ParallaxEngine::save_signature_to_db(
                tool_id,
                conversation_id,
                &serde_json::Value::Object(metadata_map.clone()),
                db,
            )
            .await
            {
                tracing::error!("Failed to persist signature for tool {}: {}", tool_id, e);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn handle_stream<R>(
        mut lines_stream: FramedRead<tokio_util::io::StreamReader<R, Bytes>, LinesCodec>,
        db: DbPool,
        conversation_id: String,
        request_id: String,
        tx: mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        model_id: String,
        pricing: std::sync::Arc<std::collections::HashMap<String, CostModel>>,
        tx_tui: tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
        start_time: std::time::Instant,
        _disable_rescue: bool,
        tools_were_advertised: bool,
        state: std::sync::Arc<AppState>,
    ) where
        R: Stream<Item = std::result::Result<Bytes, std::io::Error>> + Unpin + Send,
    {
        let mut accumulator = TurnAccumulator::new();
        let mut tool_index_map = HashMap::<u32, String>::new();
        let mut metrics = crate::logging::StreamMetric::new();
        let mut line_count = 0;
        let mut stream_state = StreamState::Initial;
        let mut buffered_pulses = Vec::new();

        while let Some(line_result) = lines_stream.next().await {
            line_count += 1;
            if line_count > MAX_STREAM_LINES {
                tracing::error!(
                    "[☁️  -> ⚙️ ] Stream exceeded max line limit ({})",
                    MAX_STREAM_LINES
                );
                let _ = tx
                    .send(Err(ParallaxError::Internal(
                        "Stream exceeded max line limit".to_string(),
                        tracing_error::SpanTrace::capture(),
                    )))
                    .await;
                break;
            }

            let should_break = Self::process_stream_line(
                line_result,
                &mut metrics,
                &mut stream_state,
                &mut accumulator,
                &mut tool_index_map,
                &conversation_id,
                &tx,
                &request_id,
                &tx_tui,
                tools_were_advertised,
                &mut buffered_pulses,
                state.clone(),
                model_id.clone(),
            )
            .await;

            if let Some(true) = should_break {
                break;
            }
        }

        Self::finish_stream(
            &accumulator,
            &model_id,
            &conversation_id,
            &request_id,
            &db,
            &pricing,
            &tx_tui,
            &tx,
            &metrics,
            start_time,
            tools_were_advertised,
            stream_state,
            state,
            &buffered_pulses,
        )
        .await;
    }

    #[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
    async fn finish_stream(
        accumulator: &TurnAccumulator,
        model_id: &str,
        conversation_id: &str,
        request_id: &str,
        db: &DbPool,
        pricing: &std::collections::HashMap<String, CostModel>,
        tx_tui: &tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        metrics: &crate::logging::StreamMetric,
        start_time: std::time::Instant,
        tools_were_advertised: bool,
        stream_state: StreamState,
        state: std::sync::Arc<AppState>,
        buffered_pulses: &[ProviderPulse],
    ) {
        if let Some(usage) = &accumulator.usage {
            Self::compute_and_send_cost(model_id, request_id, usage, pricing, tx_tui);
        }

        if !accumulator.signatures.is_empty() {
            Self::persist_signatures(accumulator, conversation_id, db).await;
        }

        let finalized_turn = accumulator.clone().finalize();

        // Detect tool calls that ended up with empty arguments.
        let mut empty_arg_tools: Vec<(String, String)> = Vec::new();
        for part in &finalized_turn.content {
            if let crate::types::MessagePart::ToolCall {
                id,
                name,
                arguments,
                ..
            } = part
            {
                let is_empty_object = match arguments.as_object() {
                    Some(m) => m.is_empty(),
                    None => false,
                };
                if is_empty_object {
                    let suspicious = TOOLS_REQUIRING_ARGS.contains(&name.as_str());
                    if suspicious {
                        empty_arg_tools.push((id.clone(), name.clone()));
                    }
                }
            }
        }

        if !empty_arg_tools.is_empty() {
            let summary = empty_arg_tools
                .iter()
                .map(|(id, name)| format!("{}:{}", name, id))
                .collect::<Vec<_>>()
                .join(", ");

            tracing::warn!(
                "[⚙️ ] Finalized turn has tool calls with empty args: {}",
                summary
            );

            let _ = tx_tui.send(crate::tui::TuiEvent::LogMessage {
                level: "WARN".to_string(),
                target: "parallax::streaming".to_string(),
                message: format!(
                    "Finalized turn has tool calls with empty args (likely streaming/provider issue): {}",
                    summary
                ),
                timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            });
        }

        // Check for diff-only response if tools were advertised but we're still buffering
        if tools_were_advertised
            && (stream_state == StreamState::BufferingUntilToolOrDone
                || stream_state == StreamState::Initial)
        {
            let mut text_content = String::new();
            for part in &finalized_turn.content {
                if let MessagePart::Text { content, .. } = part {
                    text_content.push_str(content);
                }
            }

            if crate::hardening::is_diff_like(&text_content) {
                tracing::warn!(
                    "[⚙️ ] Model {} returned diff-like response without tool calls for request {}. Retrying with enforcement...",
                    model_id,
                    request_id
                );

                let _ = tx_tui.send(crate::tui::TuiEvent::LogMessage {
                    level: "WARN".to_string(),
                    target: "parallax::streaming".to_string(),
                    message: format!(
                        "Diff-like response without tool calls detected for {}; retrying once.",
                        request_id
                    ),
                    timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                });

                // Attempt retry
                if let Err(e) = Self::retry_with_diff_enforcement(
                    state,
                    conversation_id,
                    model_id,
                    request_id,
                    tx,
                    tx_tui,
                )
                .await
                {
                    tracing::error!("[⚙️ ] Retry failed: {}", e);
                    let _ = tx.send(Err(e.inner)).await;
                    // CRITICAL FIX: Send [DONE] even when retry fails
                    let _ = tx
                        .send(Ok(axum::response::sse::Event::default().data("[DONE]")))
                        .await;
                }
                return;
            } else {
                // If it's not diff-like, we flush the buffered pulses to the client now
                for pulse in buffered_pulses {
                    if let Ok(json) = serde_json::to_string(pulse) {
                        if tx
                            .send(Ok(axum::response::sse::Event::default().data(json)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        }

        // Check for empty response (common with Gemini Pro on large contexts)
        if finalized_turn.content.is_empty() {
            tracing::error!(
                "[⚙️ ] Model {} returned completely empty stream for request {}",
                model_id,
                request_id
            );

            if state.args.gemini_fallback && model_id.contains("gemini-3-pro") {
                tracing::warn!("[⚙️ ] Gemini Pro empty stream detected; falling back to Flash...");
                let _ = tx_tui.send(crate::tui::TuiEvent::LogMessage {
                    level: "WARN".to_string(),
                    target: "parallax::streaming".to_string(),
                    message: format!(
                        "Gemini Pro empty response for {}; falling back to Flash.",
                        request_id
                    ),
                    timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                });

                if let Err(e) =
                    Self::fallback_to_flash(state, conversation_id.to_string(), tx, tx_tui).await
                {
                    tracing::error!("[⚙️ ] Fallback failed: {}", e);
                    let _ = tx.send(Err(e.inner)).await;
                    // CRITICAL FIX: Send [DONE] even when fallback fails
                    let _ = tx
                        .send(Ok(axum::response::sse::Event::default().data("[DONE]")))
                        .await;
                }
                return;
            }

            let error_msg = format!(
                "Model {} returned an empty response. This often happens with Gemini Pro when the context window is large. Try using Gemini Flash or starting a new chat.",
                model_id
            );
            let _ = tx
                .send(Err(ParallaxError::Upstream(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    error_msg,
                )))
                .await;
            // CRITICAL FIX: Send [DONE] after error
            let _ = tx
                .send(Ok(axum::response::sse::Event::default().data("[DONE]")))
                .await;
            return;
        }

        Self::finalize_and_log_turn(
            &finalized_turn,
            accumulator.usage.as_ref(),
            model_id,
            conversation_id,
            request_id,
            start_time,
        )
        .await;

        metrics.log_summary();
        if tx
            .send(Ok(axum::response::sse::Event::default().data("[DONE]")))
            .await
            .is_err()
        {
            tracing::trace!("Client disconnected, stopping stream");
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_stream_line(
        line_result: std::result::Result<String, tokio_util::codec::LinesCodecError>,
        metrics: &mut crate::logging::StreamMetric,
        stream_state: &mut StreamState,
        accumulator: &mut TurnAccumulator,
        tool_index_map: &mut HashMap<u32, String>,
        conversation_id: &str,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        request_id: &str,
        tx_tui: &tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
        tools_were_advertised: bool,
        buffered_pulses: &mut Vec<ProviderPulse>,
        state: std::sync::Arc<AppState>,
        model_id: String,
    ) -> Option<bool> {
        match line_result {
            Ok(line) => {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        tracing::debug!("[☁️  -> ⚙️ ] Stream end marker [DONE] received");
                        *stream_state = StreamState::Finished;
                        return Some(true);
                    }

                    return Self::handle_provider_line(
                        data,
                        metrics,
                        stream_state,
                        accumulator,
                        tool_index_map,
                        conversation_id,
                        tx,
                        request_id,
                        tx_tui,
                        tools_were_advertised,
                        buffered_pulses,
                        state,
                        model_id,
                    )
                    .await;
                }
            }
            Err(e) => {
                Self::handle_line_error(e, tx).await;
                *stream_state = StreamState::Finished;
                return Some(true);
            }
        }
        None
    }

    async fn handle_line_error(
        e: tokio_util::codec::LinesCodecError,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
    ) {
        tracing::error!("[☁️  -> ⚙️ ] Line Parse Error: {}", e);
        let io_err = match e {
            tokio_util::codec::LinesCodecError::Io(io) => io,
            tokio_util::codec::LinesCodecError::MaxLineLengthExceeded => {
                std::io::Error::other("Max line length exceeded")
            }
        };
        if tx.send(Err(ParallaxError::Io(io_err))).await.is_err() {
            tracing::trace!("Client disconnected, stopping stream");
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_provider_line(
        data: &str,
        metrics: &mut crate::logging::StreamMetric,
        stream_state: &mut StreamState,
        accumulator: &mut TurnAccumulator,
        tool_index_map: &mut HashMap<u32, String>,
        conversation_id: &str,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        request_id: &str,
        tx_tui: &tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
        tools_were_advertised: bool,
        buffered_pulses: &mut Vec<ProviderPulse>,
        state: std::sync::Arc<AppState>,
        model_id: String,
    ) -> Option<bool> {
        match crate::types::parse_provider_line(data) {
            LineEvent::Pulse(pulse) => {
                Self::handle_pulse_event(
                    pulse,
                    metrics,
                    stream_state,
                    accumulator,
                    tool_index_map,
                    conversation_id,
                    tx,
                    request_id,
                    tx_tui,
                    tools_were_advertised,
                    buffered_pulses,
                )
                .await;
                None
            }
            crate::types::LineEvent::Error(err) => {
                let _ = state
                    .tx_kernel
                    .send(crate::kernel::KernelCommand::UpdateHealth { success: false })
                    .await;
                let _ = state
                    .tx_kernel
                    .send(crate::kernel::KernelCommand::RecordCircuitFailure)
                    .await;
                Self::handle_provider_error(
                    data,
                    &err,
                    tx,
                    *stream_state == StreamState::StreamingToClient,
                    state,
                    conversation_id.to_string(),
                    model_id,
                    request_id.to_string(),
                    tx_tui.clone(),
                )
                .await;
                *stream_state = StreamState::Finished;
                Some(true)
            }
            crate::types::LineEvent::Unknown(_) => {
                Self::handle_unknown_event(data, tx).await;
                None
            }
        }
    }

    fn detect_520_error(err: &crate::types::ProviderError) -> bool {
        err.error.code == Some(520)
    }

    #[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
    async fn handle_provider_error(
        data: &str,
        err: &crate::types::ProviderError,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        has_seen_tool_call: bool,
        state: std::sync::Arc<AppState>,
        conversation_id: String,
        model_id: String,
        request_id: String,
        tx_tui: tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
    ) {
        let err_str = if let Ok(s) = serde_json::to_string(err) {
            s
        } else {
            String::new()
        };
        
        // Detect 520 errors specifically
        if Self::detect_520_error(err) {
            tracing::error!(
                "[☁️  -> ⚙️ ] Stream Error 520 (Provider Web Server Error) [request_id: {}]: {}",
                request_id,
                err_str
            );
        } else {
            tracing::error!("[☁️  -> ⚙️ ] Stream Error: {}", err_str);
        }

        // Classification & Retry Logic
        let is_retryable = Self::is_retryable_error(err);

        if is_retryable && !has_seen_tool_call {
            if Self::handle_gemini_fallback(
                &err_str,
                &err.error.message,
                &model_id,
                &state,
                &conversation_id,
                tx,
                &tx_tui,
            )
            .await
            {
                return;
            }

            Self::handle_standard_retry(
                &err.error.message,
                &state,
                &conversation_id,
                &model_id,
                tx,
                tx_tui,
            )
            .await;
            return;
        }

        if tx
            .send(Ok(axum::response::sse::Event::default().data(data)))
            .await
            .is_err()
        {
            tracing::trace!("Client disconnected, stopping stream");
        }
    }

    fn is_retryable_error(err: &crate::types::ProviderError) -> bool {
        match err.error.code {
            Some(code) if RETRYABLE_STATUS_CODES.contains(&code) => true,
            _ => {
                err.error.message.to_lowercase().contains("overloaded")
                    || err.error.message.to_lowercase().contains("rate limit")
                    || err.error.message.to_lowercase().contains("timeout")
            }
        }
    }

    async fn handle_gemini_fallback(
        _err_str: &str,
        error_message: &str,
        model_id: &str,
        state: &std::sync::Arc<AppState>,
        conversation_id: &str,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        tx_tui: &tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
    ) -> bool {
        if state.args.gemini_fallback && model_id.contains("gemini-3-pro") {
            tracing::warn!("[⚙️ ] Gemini Pro error detected; falling back to Flash...");
            let _ = tx_tui.send(crate::tui::TuiEvent::LogMessage {
                level: "WARN".to_string(),
                target: "parallax::streaming".to_string(),
                message: format!(
                    "Gemini Pro error: {}; falling back to Flash.",
                    error_message
                ),
                timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            });

            if let Err(e) =
                Self::fallback_to_flash(state.clone(), conversation_id.to_string(), tx, tx_tui)
                    .await
            {
                tracing::error!("[⚙️ ] Fallback failed: {}", e);
                let _ = tx.send(Err(e.inner)).await;
            }
            return true;
        }
        false
    }

    async fn handle_standard_retry(
        error_message: &str,
        state: &std::sync::Arc<AppState>,
        conversation_id: &str,
        model_id: &str,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        tx_tui: tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
    ) {
        let _ = tx_tui.send(crate::tui::TuiEvent::LogMessage {
            level: "WARN".to_string(),
            target: "parallax::streaming".to_string(),
            message: format!("Retryable stream error: {}; retrying once.", error_message),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        });

        if let Err(e) = Self::retry_stream(
            state.clone(),
            conversation_id.to_string(),
            model_id.to_string(),
            tx,
            tx_tui,
        )
        .await
        {
            tracing::error!("[⚙️ ] Stream retry failed: {}", e);
            let _ = tx.send(Err(e.inner)).await;
        }
    }

    async fn retry_stream(
        state: std::sync::Arc<AppState>,
        conversation_id: String,
        model_id: String,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        tx_tui: tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
    ) -> Result<()> {
        Self::execute_retry_or_fallback(state, conversation_id, model_id, tx, tx_tui).await
    }

    async fn fallback_to_flash(
        state: std::sync::Arc<AppState>,
        conversation_id: String,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        tx_tui: &tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
    ) -> Result<()> {
        // Find a suitable Flash model. We'll use a heuristic or common name.
        let fallback_model = GEMINI_FLASH_FALLBACK.to_string();
        Self::execute_retry_or_fallback(state, conversation_id, fallback_model, tx, tx_tui.clone())
            .await
    }

    async fn execute_retry_or_fallback(
        state: std::sync::Arc<AppState>,
        conversation_id: String,
        model_id: String,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        _tx_tui: tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
    ) -> Result<()> {
        // Reconstruct context and execute
        let retry_context = ConversationContext {
            history: Vec::new(),
            conversation_id: conversation_id.clone(),
            extra_body: serde_json::json!({}),
        };

        let kind = crate::projections::ProviderKind::from_model_name(&model_id);
        let flavor: std::sync::Arc<dyn crate::projections::ProviderFlavor + Send + Sync> =
            match kind {
                crate::projections::ProviderKind::Google => {
                    std::sync::Arc::new(crate::projections::GeminiFlavor)
                }
                crate::projections::ProviderKind::Anthropic => {
                    std::sync::Arc::new(crate::projections::AnthropicFlavor)
                }
                crate::projections::ProviderKind::OpenAi => {
                    std::sync::Arc::new(crate::projections::OpenAiFlavor)
                }
                crate::projections::ProviderKind::Standard => {
                    std::sync::Arc::new(crate::projections::StandardFlavor)
                }
            };

        let mut outgoing_request = crate::projections::OpenRouterAdapter::project(
            &retry_context,
            &model_id,
            flavor.as_ref(),
            &state.db,
            None,
        )
        .await;

        outgoing_request.stream = Some(true);

        let response = state
            .client
            .post(OPENROUTER_CHAT_COMPLETIONS)
            .header("Authorization", format!("Bearer {}", state.openrouter_key))
            .json(&outgoing_request)
            .send()
            .await
            .map_err(ParallaxError::Network)?;

        if !response.status().is_success() {
            let err_body = match response.text().await {
                Ok(t) => t,
                Err(_) => "Unknown error (failed to read response text)".to_string(),
            };
            return Err(ParallaxError::Upstream(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Retry/Fallback failed: {}", err_body),
            )
            .into());
        }

        let bytes_stream = response
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
        let mut lines_stream = FramedRead::new(
            tokio_util::io::StreamReader::new(bytes_stream),
            LinesCodec::new_with_max_length(1024 * 1024),
        );

        while let Some(line_result) = lines_stream.next().await {
            match line_result {
                Ok(line) => {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if tx
                            .send(Ok(axum::response::sse::Event::default().data(data)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }

        Ok(())
    }

    async fn handle_unknown_event(
        data: &str,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
    ) {
        tracing::warn!("[☁️  -> ⚙️ ] Unknown Line Event: {}", data);
        if tx
            .send(Ok(axum::response::sse::Event::default().data(data)))
            .await
            .is_err()
        {
            tracing::trace!("Client disconnected, stopping stream");
        }
    }

    #[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
    async fn handle_pulse_event(
        mut pulse: ProviderPulse,
        metrics: &mut crate::logging::StreamMetric,
        stream_state: &mut StreamState,
        accumulator: &mut TurnAccumulator,
        tool_index_map: &mut HashMap<u32, String>,
        conversation_id: &str,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        request_id: &str,
        tx_tui: &tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
        tools_were_advertised: bool,
        buffered_pulses: &mut Vec<ProviderPulse>,
    ) {
        metrics.record_chunk(&pulse);

        let has_tool_calls = pulse.choices.iter().any(|c| c.delta.tool_calls.is_some());

        match stream_state {
            StreamState::Initial => {
                if has_tool_calls {
                    *stream_state = StreamState::StreamingToClient;
                } else if tools_were_advertised {
                    *stream_state = StreamState::BufferingUntilToolOrDone;
                } else {
                    *stream_state = StreamState::StreamingToClient;
                }
            }
            StreamState::BufferingUntilToolOrDone => {
                if has_tool_calls {
                    *stream_state = StreamState::StreamingToClient;
                }
            }
            _ => {}
        }

        if *stream_state == StreamState::StreamingToClient {
            Self::sanitize_tool_calls_in_place(&mut pulse);
        }

        tracing::trace!("[☁️  -> ⚙️ ] Pulse: {:?}", pulse);
        Self::process_pulse(&pulse, conversation_id, tool_index_map, accumulator).await;

        if *stream_state == StreamState::BufferingUntilToolOrDone {
            buffered_pulses.push(pulse);
            return;
        }

        // Send buffered pulses if we just transitioned to streaming
        if *stream_state == StreamState::StreamingToClient && !buffered_pulses.is_empty() {
            for p in buffered_pulses.drain(..) {
                if let Ok(json) = serde_json::to_string(&p) {
                    if tx
                        .send(Ok(axum::response::sse::Event::default().data(json)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }

        // Re-serialize the sanitized pulse
        if let Ok(sanitized_json) = serde_json::to_string(&pulse) {
            if tx
                .send(Ok(
                    axum::response::sse::Event::default().data(sanitized_json)
                ))
                .await
                .is_err()
            {
                tracing::trace!("Client disconnected, stopping stream");
            }
        }
        // Emit TUI StreamUpdate
        for choice in &pulse.choices {
            let tool_call_desc = match choice.delta.tool_calls.as_ref() {
                Some(tcs) => tcs.iter().find_map(|tc| {
                    tc.function.as_ref().map(|f| {
                        format!(
                            "{}({})",
                            if let Some(n) = f.name.as_deref() {
                                n
                            } else {
                                ""
                            },
                            if let Some(a) = f.arguments.as_deref() {
                                a
                            } else {
                                ""
                            }
                        )
                    })
                }),
                None => None,
            };

            if let Some(content) = &choice.delta.content {
                if !content.is_empty() {
                    let _ = tx_tui.send(crate::tui::TuiEvent::StreamUpdate {
                        id: request_id.to_string(),
                        content_delta: content.clone(),
                        tool_call: tool_call_desc.clone(),
                    });
                }
            } else if let Some(tc) = tool_call_desc {
                let _ = tx_tui.send(crate::tui::TuiEvent::StreamUpdate {
                    id: request_id.to_string(),
                    content_delta: String::new(),
                    tool_call: Some(tc),
                });
            }

            if let Some(thought) = choice.delta.extract_reasoning() {
                if !thought.is_empty() {
                    let _ = tx_tui.send(crate::tui::TuiEvent::StreamUpdate {
                        id: request_id.to_string(),
                        content_delta: thought,
                        tool_call: None,
                    });
                }
            }
        }
    }

    fn sanitize_tool_calls_in_place(pulse: &mut ProviderPulse) {
        for choice in &mut pulse.choices {
            if choice.delta.content.is_some() {
                choice.delta.content = None;
            }
            if let Some(reason) = &choice.finish_reason {
                if reason == "stop" {
                    choice.finish_reason = Some("tool_calls".to_string());
                }
            }
        }
    }

    async fn retry_with_diff_enforcement(
        state: std::sync::Arc<AppState>,
        conversation_id: &str,
        model_id: &str,
        _request_id: &str,
        tx: &mpsc::Sender<std::result::Result<axum::response::sse::Event, ParallaxError>>,
        _tx_tui: &tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
    ) -> Result<()> {
        // Fetch original conversation context to retry
        // In a real scenario, we'd need to reconstruct history.
        // For now, we'll try to find a way to get the context or at least the last turn.

        let mut retry_context = ConversationContext {
            history: Vec::new(),
            conversation_id: conversation_id.to_string(),
            extra_body: serde_json::json!({}),
        };

        retry_context.history.push(TurnRecord {
            role: Role::User,
            content: vec![MessagePart::Text {
                content: "CRITICAL: Do not output diffs/patches. Use the provided tools to apply changes. If you cannot use tools, say so explicitly.".to_string(),
                cache_control: None,
            }],
            tool_call_id: None,
        });

        // Determine flavor
        let kind = crate::projections::ProviderKind::from_model_name(model_id);
        let flavor: std::sync::Arc<dyn crate::projections::ProviderFlavor + Send + Sync> =
            match kind {
                crate::projections::ProviderKind::Google => {
                    std::sync::Arc::new(crate::projections::GeminiFlavor)
                }
                crate::projections::ProviderKind::Anthropic => {
                    std::sync::Arc::new(crate::projections::AnthropicFlavor)
                }
                crate::projections::ProviderKind::OpenAi => {
                    std::sync::Arc::new(crate::projections::OpenAiFlavor)
                }
                crate::projections::ProviderKind::Standard => {
                    std::sync::Arc::new(crate::projections::StandardFlavor)
                }
            };

        let intent = None;

        let mut outgoing_request = crate::projections::OpenRouterAdapter::project(
            &retry_context,
            model_id,
            flavor.as_ref(),
            &state.db,
            intent,
        )
        .await;

        outgoing_request.stream = Some(true);

        // Record retry attempt in flight recorder if possible
        // (We'd need the recorder passed in, but we have the request_id)
        tracing::info!(
            "[⚙️ ] Executing retry for {} with enforcement",
            conversation_id
        );

        // Execute retry
        let response = state
            .client
            .post(OPENROUTER_CHAT_COMPLETIONS)
            .header("Authorization", format!("Bearer {}", state.openrouter_key))
            .json(&outgoing_request)
            .send()
            .await
            .map_err(ParallaxError::Network)?;

        if !response.status().is_success() {
            let err_body = match response.text().await {
                Ok(t) => t,
                Err(_) => "Unknown error (failed to read response text)".to_string(),
            };
            return Err(ParallaxError::Upstream(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Retry failed: {}", err_body),
            )
            .into());
        }

        let bytes_stream = response
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
        let mut lines_stream = FramedRead::new(
            tokio_util::io::StreamReader::new(bytes_stream),
            LinesCodec::new_with_max_length(1024 * 1024),
        );

        while let Some(line_result) = lines_stream.next().await {
            match line_result {
                Ok(line) => {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if tx
                            .send(Ok(axum::response::sse::Event::default().data(data)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }

        Ok(())
    }

    fn compute_and_send_cost(
        model_id: &str,
        request_id: &str,
        usage: &Usage,
        pricing: &HashMap<String, CostModel>,
        tx_tui: &tokio::sync::broadcast::Sender<crate::tui::TuiEvent>,
    ) {
        match crate::main_helper::calculate_cost(model_id, usage, pricing) {
            Ok(breakdown) => {
                let _ = tx_tui.send(crate::tui::TuiEvent::CostUpdate {
                    id: request_id.to_string(),
                    model: model_id.to_string(),
                    usage: usage.clone(),
                    actual_cost: breakdown.actual_cost,
                    potential_cost_no_cache: breakdown.potential_cost_no_cache,
                });
            }
            Err(reason) => {
                tracing::warn!("[⚙️ ] Unable to compute cost because: {}", reason);
                let _ = tx_tui.send(crate::tui::TuiEvent::CostUpdate {
                    id: request_id.to_string(),
                    model: model_id.to_string(),
                    usage: usage.clone(),
                    actual_cost: 0.0,
                    potential_cost_no_cache: 0.0,
                });
            }
        }
    }

    fn push_tool_call_pulse_parts(
        content: &mut Vec<PulsePart>,
        tool_deltas: &[ProviderToolCallDelta],
        tool_index_map: &HashMap<u32, String>,
    ) {
        // Some providers (notably Minimax via OpenRouter) omit tool_call `id` on follow-up chunks and
        // only provide it on the first chunk. We map via `index` to keep deltas associated.
        // If we never receive an id, fall back to a stable synthetic id per index.
        #[derive(Debug, Clone, Copy)]
        enum ToolIdSource {
            Provided,
            MappedFromIndex,
            SyntheticIndex,
        }

        for td in tool_deltas {
            let (id, id_source) = if let Some(id) = &td.id {
                (Some(id.clone()), ToolIdSource::Provided)
            } else if let Some(id) = tool_index_map.get(&td.index) {
                (Some(id.clone()), ToolIdSource::MappedFromIndex)
            } else {
                (
                    Some(format!("tool_index_{}", td.index)),
                    ToolIdSource::SyntheticIndex,
                )
            };

            if td.id.is_none() {
                let name_present = match td.function.as_ref() {
                    Some(f) => match &f.name {
                        Some(n) => !n.is_empty(),
                        None => false,
                    },
                    None => false,
                };
                let args_delta_len = match td.function.as_ref().and_then(|f| f.arguments.as_ref()) {
                    Some(a) => a.len(),
                    None => 0,
                };

                tracing::warn!(
                    "[STREAM] tool_call id missing; using {:?} (index={}, name_present={}, args_delta_len={})",
                    id_source,
                    td.index,
                    name_present,
                    args_delta_len,
                );
            }

            content.push(PulsePart::ToolCall {
                id,
                name: match td.function.as_ref() {
                    Some(f) => f.name.clone(),
                    None => None,
                },
                arguments_delta: match td.function.as_ref().and_then(|f| f.arguments.as_ref()) {
                    Some(a) => a.clone(),
                    None => String::new(),
                },
                metadata: Some(serde_json::Value::Object(td.extra.clone())),
            });
        }
    }

    async fn process_pulse(
        pulse: &ProviderPulse,
        _conversation_id: &str,
        tool_index_map: &mut HashMap<u32, String>,
        accumulator: &mut TurnAccumulator,
    ) {
        let mut part_types = Vec::new();
        for choice in &pulse.choices {
            if let Some(ref content) = choice.delta.content {
                part_types.push(format!("text({})", content.len()));
            }

            if let Some(reasoning_str) = choice.delta.extract_reasoning() {
                part_types.push(format!("thought({})", reasoning_str.len()));
            }

            if let Some(ref tool_deltas) = choice.delta.tool_calls {
                for td in tool_deltas {
                    part_types.push(format!("tool_call({})", td.index));
                    if let Some(id) = &td.id {
                        tool_index_map.insert(td.index, id.clone());
                    }
                    if let Some(id) = tool_index_map.get(&td.index) {
                        if !td.extra.is_empty() {
                            accumulator
                                .signatures
                                .entry(id.clone())
                                .or_default()
                                .extend(td.extra.clone());
                        }
                    }
                }
            }

            // Turn-level metadata
            if !choice.delta.extra.is_empty() {
                if let Some(id) = tool_index_map.values().last() {
                    accumulator
                        .signatures
                        .entry(id.clone())
                        .or_default()
                        .extend(choice.delta.extra.clone());
                } else {
                    accumulator
                        .signatures
                        .entry("turn_level".to_string())
                        .or_default()
                        .extend(choice.delta.extra.clone());
                }
            }
        }

        if !part_types.is_empty() {
            tracing::debug!("[☁️  -> ⚙️ ] Pulse Parts: {:?}", part_types);
        }

        let mut content = Vec::new();
        let choice = &pulse.choices[0];

        // 1. Tool Calls
        if let Some(ref tool_deltas) = choice.delta.tool_calls {
            Self::push_tool_call_pulse_parts(&mut content, tool_deltas, tool_index_map);
        }

        // 2. Text Content
        if let Some(ref text) = choice.delta.content {
            if !text.is_empty() {
                content.push(PulsePart::Text {
                    delta: text.clone(),
                });
            }
        }

        // 3. Reasoning / Thought (from extra)
        if let Some(reasoning_str) = choice.delta.extract_reasoning() {
            content.push(PulsePart::Thought {
                delta: reasoning_str,
            });
        }

        if !content.is_empty() || pulse.usage.is_some() {
            let internal_pulse = InternalPulse {
                content,
                finish_reason: choice.finish_reason.clone(),
                usage: pulse.usage.clone(),
            };
            accumulator.push(internal_pulse);
        }
    }

    fn log_finalized_turn(turn: &TurnRecord, usage: &Option<Usage>, latency: std::time::Duration) {
        let mut parts_summary = Vec::new();
        let mut total_text_len = 0;
        let mut total_thought_len = 0;
        let mut tools_called = Vec::new();

        for p in &turn.content {
            match p {
                MessagePart::Text { content, .. } => {
                    total_text_len += content.len();
                    parts_summary.push(format!("text({} chars)", content.len()));
                }
                MessagePart::ToolCall { name, .. } => {
                    tools_called.push(name.clone());
                    parts_summary.push(format!("tool_call({})", name));
                }
                MessagePart::Thought { content } => {
                    total_thought_len += content.len();
                    parts_summary.push(format!("thought({} chars)", content.len()));
                }
                _ => {}
            }
        }

        let usage_str = match usage {
            Some(u) => {
                let cache_str = match u
                    .prompt_tokens_details
                    .as_ref()
                    .and_then(|d| d.cached_tokens)
                {
                    Some(c) => format!(" ({} cached)", c),
                    None => String::new(),
                };
                format!(
                    "Prompt: {}{}, Completion: {}, Total: {}",
                    u.prompt_tokens, cache_str, u.completion_tokens, u.total_tokens
                )
            }
            None => "Usage unavailable".to_string(),
        };

        tracing::info!(
            "Stream Finished. Latency: {:?}\n\
             Summary: {}\n\
             Stats: {} chars text, {} chars thought, {} tools\n\
             Tokens: {}",
            latency,
            parts_summary.join(", "),
            total_text_len,
            total_thought_len,
            tools_called.len(),
            usage_str
        );
    }
}
