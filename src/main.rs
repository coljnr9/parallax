#![allow(clippy::manual_unwrap_or_default)]
#![allow(clippy::manual_unwrap_or)]
use parallax::db::*;
use parallax::engine::*;
use parallax::log_rotation::{LogRotationConfig, LogRotationManager};
use parallax::logging::turn_id_middleware;
use parallax::pricing::fetch_pricing;
use parallax::tui::{App, TuiEvent};
use parallax::*;

use parallax::ingress::RawTurn;
use parallax::projections::AnthropicFlavor;
use parallax::projections::GeminiFlavor;
use parallax::projections::OpenAiFlavor;
use parallax::projections::OpenRouterAdapter;
use parallax::projections::ProviderFlavor;
use parallax::projections::StandardFlavor;
use parallax::streaming::StreamHandler;

use axum::response::sse::KeepAlive;
use axum::{
    extract::State,
    http as ax_http, middleware,
    response::{IntoResponse, Response, Sse},
    routing::post,
    Json, Router,
};
use clap::Parser;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::Instrument;
use tracing_subscriber::Layer;

struct TuiLayer {
    tx: broadcast::Sender<TuiEvent>,
}

impl<S> Layer<S> for TuiLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut message = String::new();
        let mut visitor = LogVisitor {
            message: &mut message,
        };
        event.record(&mut visitor);

        let metadata = event.metadata();
        let level = metadata.level().to_string();
        let target = metadata.target().to_string();
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();

        let _ = self.tx.send(TuiEvent::LogMessage {
            level,
            target,
            message,
            timestamp,
        });
    }
}

struct LogVisitor<'a> {
    message: &'a mut String,
}

impl<'a> tracing::field::Visit for LogVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message.push_str(&format!("{:?}", value));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        }
    }
}

// --- SERVER ---

fn detect_intent(_model: &str, payload: &serde_json::Value) -> Option<crate::tui::Intent> {
    let raw_content = payload
        .get("messages")
        .or_else(|| payload.get("input"))
        .and_then(|m| m.as_array())
        .and_then(|a| a.last())
        .and_then(|l| l.get("content"))
        .and_then(|c| c.as_str())?;

    // 1. Explicit Tag Parsing (Prioritized Source of Truth)
    // We only look for tags that are NOT inside triple-backtick code blocks.
    let mut clean_content = String::new();
    let mut in_code_block = false;
    for line in raw_content.lines() {
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
        }
        if !in_code_block {
            clean_content.push_str(line);
            clean_content.push('\n');
        }
    }

    if let Some(intent) = detect_intent_tag(&clean_content) {
        return Some(intent);
    }

    // 2. Keyword Fallback (Only if no explicit tag found outside code blocks)
    // We look at the last 500 chars to avoid catching keywords in long system prompts or scaffold text.
    let search_window = if clean_content.len() > 500 {
        &clean_content[clean_content.len() - 500..]
    } else {
        &clean_content
    };

    detect_intent_keywords(search_window)
}

fn detect_intent_tag(clean_content: &str) -> Option<crate::tui::Intent> {
    if let Some(start) = clean_content.find("<system_reminder>") {
        let after_start = &clean_content[start + "<system_reminder>".len()..];
        if let Some(end_offset) = after_start.find("</system_reminder>") {
            let reminder_content = &after_start[..end_offset];
            let upper_reminder = reminder_content.to_uppercase();

            let intent = if upper_reminder.contains("AGENT")
                || upper_reminder.contains("COMPOSER")
                || upper_reminder.contains("BUILD")
            {
                crate::tui::Intent::Agent
            } else if upper_reminder.contains("PLAN") {
                crate::tui::Intent::Plan
            } else if upper_reminder.contains("DEBUG") {
                crate::tui::Intent::Debug
            } else {
                crate::tui::Intent::Ask
            };

            tracing::debug!(
                "Intent detected via <system_reminder>: {:?} (snippet: {:?})",
                intent,
                reminder_content.chars().take(50).collect::<String>()
            );
            return Some(intent);
        }
    }
    None
}

fn detect_intent_keywords(search_window: &str) -> Option<crate::tui::Intent> {
    let content = search_window.to_uppercase();

    let intent = if content.contains(" PLAN MODE") || content.contains(" PLANNING MODE") {
        Some(crate::tui::Intent::Plan)
    } else if content.contains(" AGENT MODE")
        || content.contains(" COMPOSER MODE")
        || content.contains(" BUILD MODE")
    {
        Some(crate::tui::Intent::Agent)
    } else if content.contains(" DEBUG MODE") {
        Some(crate::tui::Intent::Debug)
    } else if content.contains(" ASK MODE") || content.contains(" CHAT MODE") {
        Some(crate::tui::Intent::Ask)
    } else {
        None
    };

    if let Some(i) = intent {
        tracing::debug!(
            "Intent detected via keywords: {:?} (window: {:?})",
            i,
            search_window.chars().take(50).collect::<String>()
        );
    }

    intent
}

#[allow(dead_code)]
async fn replay_artifact(path: &str) -> Result<()> {
    println!("--- REPLAYING ARTIFACT: {} ---", path);
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(ParallaxError::Io)?;
    let recorder: crate::debug_utils::FlightRecorder = serde_json::from_str(&content)
        .map_err(|e| ParallaxError::InvalidIngress(format!("Failed to parse artifact: {}", e)))?;

    if let Some(ingress_raw) = recorder.stages.get("ingress_raw") {
        println!("Stage: Ingress Raw found.");
        let raw: RawTurn = serde_json::from_value(ingress_raw.clone()).map_err(|e| {
            ParallaxError::InvalidIngress(format!("Invalid ingress in artifact: {}", e))
        })?;

        println!("Validating...");
        raw.validate()?;
        println!("Validation OK.");

        // For replay to work fully we'd need a DB pool.
        // We'll skip the bits that need DB for now or just print what we have.
        println!(
            "Replay of lifting/projection requires a DB pool. Artifact contains {} decisions.",
            recorder.decisions.len()
        );
        for (i, d) in recorder.decisions.iter().enumerate() {
            println!("Decision {}: {}", i, d);
        }
    }

    Ok(())
}

#[tracing::instrument(
    name = "shim.request",
    skip_all,
    fields(
        request_id = tracing::field::Empty,
        model.target = tracing::field::Empty,
        tokens.prompt = tracing::field::Empty,
        tokens.completion = tracing::field::Empty,
        http.status = tracing::field::Empty,
        shim.outcome = tracing::field::Empty,
        cf.ray = tracing::field::Empty,
        cf.ip = tracing::field::Empty,
    )
)]
async fn chat_completions_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    let _start = std::time::Instant::now();
    let span = tracing::Span::current();

    // Capture Cloudflare/Tunnel headers if present
    if let Some(ray_id) = headers.get("cf-ray").and_then(|h| h.to_str().ok()) {
        span.record("cf.ray", ray_id);
        tracing::debug!("Request received via Cloudflare Ray: {}", ray_id);
    }
    if let Some(forwarded) = headers.get("x-forwarded-for").and_then(|h| h.to_str().ok()) {
        span.record("cf.ip", forwarded);
    }

    // Extract Cursor conversation ID from headers if present
    // Try multiple possible header names that Cursor might use
    let cursor_conversation_id = headers
        .get("x-cursor-conversation-id")
        .or_else(|| headers.get("x-conversation-id"))
        .or_else(|| headers.get("cursor-conversation-id"))
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    if let Some(ref cid) = cursor_conversation_id {
        tracing::info!(
            "[🖱️  -> ⚙️ ] Found Cursor conversation ID in header: [{}...]",
            crate::str_utils::prefix_chars(cid, 8)
        );
    }

    if let Err(resp) = validate_payload(&payload) {
        span.record("shim.outcome", "client_error");
        return *resp;
    }

    let entry = match ParallaxEngine::lift(payload.clone(), &state.db, cursor_conversation_id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[🖱️  -> ⚙️ ] Lift Failed: {}", e);
            span.record("shim.outcome", "internal_error");
            return (
                ax_http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let (model_id, context, rid, flavor) = resolve_flavor_context(entry);
    span.record("request_id", &rid);
    span.record("model.target", &model_id);

    let cid = context.conversation_id.clone();
    let cid_source = context.conversation_id_source.clone();
    let turn_id_uuid = uuid::Uuid::new_v4().to_string();
    let tid = turn_id_uuid.clone();

    // Phase 2: Initialize bundle
    let bundle_manager = crate::debug_bundle::BundleManager::new("debug_capture");
    let _ = bundle_manager.ensure_turn_dir(&cid, &tid).await;

    tracing::info!(
        "[⚙️  -> ⚙️ ] Turn Context: CID: [{}...] (Source: {}) TID: [{}...] RID: [{}...]",
        crate::str_utils::prefix_chars(&cid, 8),
        cid_source,
        crate::str_utils::prefix_chars(&tid, 8),
        crate::str_utils::prefix_chars(&rid, 8)
    );

    // Always write ingress_raw blob so users can see their messages
    let _ = bundle_manager
        .write_blob(&cid, &tid, "ingress_raw", payload.to_string().as_bytes())
        .await;

    // Capture the lifted context for debugging
    let lifted_json = match serde_json::to_value(&context) {
        Ok(val) => val,
        Err(e) => {
            tracing::warn!("Failed to serialize context for debug: {}", e);
            serde_json::Value::Null
        }
    };
    if state.args.enable_debug_capture {
        let _ = bundle_manager
            .write_blob(&cid, &tid, "lifted", lifted_json.to_string().as_bytes())
            .await;
        crate::debug_utils::capture_debug_snapshot("lifted", &rid, &cid, &model_id, &lifted_json)
            .await;
    }

    let intent = detect_intent(&model_id, &payload);

    let mut recorder =
        crate::debug_utils::FlightRecorder::new(&tid, &rid, &cid, &model_id, flavor.name());
    recorder.record_stage("ingress_raw", payload.clone());

    // Phase 2: Start TurnDetail
    let start_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut stages_vec: Vec<crate::debug_bundle::StageIndex> = Vec::new();

    // Extract useful metadata from ingress payload for summary
    let messages_len = payload
        .get("messages")
        .and_then(|m| m.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let tools_len = payload
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    stages_vec.push(crate::debug_bundle::StageIndex {
        name: "ingress_raw".to_string(),
        kind: crate::debug_bundle::StageKind::Snapshot,
        summary: serde_json::json!({
            "len": payload.to_string().len(),
            "messages_len": messages_len,
            "tools_len": tools_len,
        }),
        blob_ref: Some(crate::debug_bundle::BlobRef {
            blob_id: "ingress_raw".to_string(),
            content_type: "application/json".to_string(),
            approx_bytes: payload.to_string().len() as u64,
            file_name: Some("ingress_raw.json".to_string()),
            sha256: None,
            written_at_ms: None,
        }),
    });

    if state.args.enable_debug_capture {
        stages_vec.push(crate::debug_bundle::StageIndex {
            name: "lifted".to_string(),
            kind: crate::debug_bundle::StageKind::Snapshot,
            summary: serde_json::json!({
                "history_len": context.history.len(),
            }),
            blob_ref: Some(crate::debug_bundle::BlobRef {
                blob_id: "lifted".to_string(),
                content_type: "application/json".to_string(),
                approx_bytes: lifted_json.to_string().len() as u64,
                file_name: Some("lifted.json".to_string()),
                sha256: None,
                written_at_ms: None,
            }),
        });
    }

    let user_query_opt = crate::debug_bundle::BundleManager::extract_user_query(&payload);

    // Compute tag deltas if user_query exists
    let user_query_tags = if let Some(ref user_query) = user_query_opt {
        bundle_manager
            .compute_user_query_tag_deltas(&cid, user_query)
            .await
    } else {
        None
    };

    // Extract cursor tags from ingress payload (final will be added during finalization)
    let cursor_tags = crate::debug_bundle::BundleManager::extract_cursor_tags(&payload, None);

    let turn_detail = crate::debug_bundle::TurnDetail {
        turn_id: tid.clone(),
        request_id: rid.clone(),
        model_id: model_id.clone(),
        flavor: flavor.name().to_string(),
        started_at_ms: start_ms,
        ended_at_ms: None,
        stages: stages_vec,
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
        cursor_tags,
        issues: Vec::new(),
        trace_id: None,
        span_summary: None,
        user_query: user_query_opt,
        role: Some("User".to_string()),
        conversation_id_source: context.conversation_id_source.clone(),
        user_query_tags,
    };

    // Initial write
    let _ = bundle_manager
        .update_summaries(&cid, &tid, &turn_detail)
        .await;

    if let Err(e) = ParallaxEngine::validate_context(&context) {
        return handle_validation_error(e, &mut recorder).await;
    }

    let _ = state.tx_tui.send(TuiEvent::RequestStarted {
        id: rid.clone(),
        cid: cid.clone(),
        method: "Chat".to_string(),
        model: model_id.clone(),
        intent,
    });

    // Delegated to reduce complexity
    handle_turn_processing(
        state,
        context,
        model_id,
        flavor,
        rid,
        &mut recorder,
        &payload,
        intent,
        tid,
    )
    .await
}

async fn handle_validation_error(
    e: ObservedError,
    recorder: &mut crate::debug_utils::FlightRecorder,
) -> Response {
    tracing::error!("[⚙️  -> ⚙️ ] Context Validation Failed: {}", e);
    recorder.record_decision(format!("Validation Failed: {}", e));
    recorder.save().await;
    e.into_response()
}

#[allow(clippy::too_many_arguments)]
async fn handle_turn_processing(
    state: Arc<AppState>,
    context: ConversationContext,
    model_id: String,
    flavor: Arc<dyn ProviderFlavor + Send + Sync>,
    rid: String,
    recorder: &mut crate::debug_utils::FlightRecorder,
    payload: &serde_json::Value,
    intent: Option<crate::tui::Intent>,
    tid: String,
) -> Response {
    let start_time = std::time::Instant::now();
    let cid = context.conversation_id.clone();

    let span = tracing::info_span!(
        "turn",
        cid = &cid[..8.min(cid.len())],
        rid = &rid[..8.min(rid.len())]
    );

    crate::logging::log_request_summary(payload);

    let response = process_turn(
        state.clone(),
        context,
        model_id,
        flavor,
        rid.clone(),
        start_time,
        recorder,
        intent,
        tid,
    )
    .instrument(span)
    .await;

    recorder.save().await;

    let latency = start_time.elapsed().as_millis();

    let status = match response.status() {
        s if s.is_success() => 200,
        s => s.as_u16(),
    };

    let _ = state.tx_tui.send(TuiEvent::RequestFinished {
        id: rid,
        status,
        latency_ms: latency,
    });

    response
}

fn validate_payload(payload: &serde_json::Value) -> std::result::Result<(), Box<Response>> {
    let raw: RawTurn = match serde_json::from_value(payload.clone()) {
        Ok(r) => r,
        Err(e) => {
            return Err(Box::new((
                ax_http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Payload deserialization failed: {}", e) })),
            ).into_response()));
        }
    };

    if let Err(e) = raw.validate() {
        tracing::error!("[🖱️  -> ⚙️ ] Validation Failed: {}", e);
        return Err(Box::new(
            (
                ax_http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string(), "code": "VALIDATION_ERROR" })),
            )
                .into_response(),
        ));
    }

    Ok(())
}

fn resolve_flavor_context(
    entry: TurnOperationEntry,
) -> (
    String,
    ConversationContext,
    String,
    Arc<dyn ProviderFlavor + Send + Sync>,
) {
    match entry {
        TurnOperationEntry::Gemini(op) => (
            op.model.model_name().to_string(),
            op.input_context,
            op.request_id,
            Arc::new(GeminiFlavor),
        ),
        TurnOperationEntry::Anthropic(op) => (
            op.model.model_name().to_string(),
            op.input_context,
            op.request_id,
            Arc::new(AnthropicFlavor),
        ),
        TurnOperationEntry::OpenAI(op) => (
            op.model.model_name().to_string(),
            op.input_context,
            op.request_id,
            Arc::new(OpenAiFlavor),
        ),
        TurnOperationEntry::Standard(op) => (
            op.model.model_name().to_string(),
            op.input_context,
            op.request_id,
            Arc::new(StandardFlavor),
        ),
    }
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
async fn process_turn(
    state: Arc<AppState>,
    context: ConversationContext,
    model_id: String,
    flavor: Arc<dyn ProviderFlavor + Send + Sync>,
    request_id: String,
    start_time: std::time::Instant,
    recorder: &mut crate::debug_utils::FlightRecorder,
    intent: Option<crate::tui::Intent>,
    tid: String,
) -> Response {
    tracing::info!(
        "[🖱️  -> ⚙️ ] Received Turn [History: {}]",
        context.history.len()
    );

    let outgoing_request = match project_request(&state, &context, &model_id, flavor, intent).await
    {
        Ok(val) => val,
        Err(e) => return e,
    };

    let outgoing_request_json = match serde_json::to_value(&outgoing_request) {
        Ok(val) => val,
        Err(e) => {
            tracing::error!("Failed to serialize request for logging: {}", e);
            return (
                ax_http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Serialization failed"})),
            )
                .into_response();
        }
    };

    crate::debug_utils::log_traffic_summary(
        &format!("Shim -> OpenRouter ({})", model_id),
        &outgoing_request_json,
    );

    recorder.record_stage("upstream_request", outgoing_request_json.clone());

    // Phase 2: Write projected request
    let bundle_manager = crate::debug_bundle::BundleManager::new("debug_capture");
    if let Ok(blob_ref) = bundle_manager
        .write_blob(
            &context.conversation_id,
            &tid,
            "projected",
            outgoing_request_json.to_string().as_bytes(),
        )
        .await
    {
        let _ = bundle_manager
            .add_stage(
                &context.conversation_id,
                &tid,
                "projected",
                blob_ref,
                serde_json::json!({
                    "len": outgoing_request_json.to_string().len(),
                }),
            )
            .await;
    }

    let result = execute_upstream_request(&state, &outgoing_request).await;

    match result {
        Ok(response) => {
            state.health.record_success();
            state.circuit_breaker.record_success().await;

            // Emit health update
            let _ = state.tx_tui.send(TuiEvent::UpstreamHealthUpdate {
                consecutive_failures: state
                    .health
                    .consecutive_failures
                    .load(std::sync::atomic::Ordering::Relaxed),
                total_requests: state
                    .health
                    .total_requests
                    .load(std::sync::atomic::Ordering::Relaxed),
                failed_requests: state
                    .health
                    .failed_requests
                    .load(std::sync::atomic::Ordering::Relaxed),
                degraded: false,
            });

            let is_streaming = match outgoing_request.stream {
                Some(s) => s,
                None => false,
            };
            let tools_were_advertised = match outgoing_request.tools.as_ref() {
                Some(t) => !t.is_empty(),
                None => false,
            };

            if is_streaming {
                handle_upstream_response(
                    response,
                    state.clone(),
                    context,
                    model_id,
                    request_id,
                    start_time,
                    recorder,
                    tools_were_advertised,
                    tid,
                )
                .await
            } else {
                handle_non_streaming_response(response, recorder, &context.conversation_id, &tid)
                    .await
            }
        }
        Err(e) => {
            state.health.record_failure();
            state.circuit_breaker.record_failure().await;

            // Emit health update
            let _ = state.tx_tui.send(TuiEvent::UpstreamHealthUpdate {
                consecutive_failures: state
                    .health
                    .consecutive_failures
                    .load(std::sync::atomic::Ordering::Relaxed),
                total_requests: state
                    .health
                    .total_requests
                    .load(std::sync::atomic::Ordering::Relaxed),
                failed_requests: state
                    .health
                    .failed_requests
                    .load(std::sync::atomic::Ordering::Relaxed),
                degraded: true,
            });

            tracing::error!("[☁️  -> ⚙️ ] Request Error: {}", e);

            match &e.inner {
                ParallaxError::Upstream(status, body) => {
                    recorder.record_upstream_error(*status, body);
                }
                _ => {
                    recorder.record_decision(format!("Request Error: {}", e));
                }
            }

            e.into_response()
        }
    }
}

async fn execute_upstream_request(
    state: &Arc<AppState>,
    outgoing_request: &crate::specs::openai::OpenAiRequest,
) -> Result<reqwest::Response> {
    let retry_policy = crate::hardening::RetryPolicy::new(state.args.max_retries, 100);

    state.circuit_breaker.check().await?;

    let state_clone = state.clone();
    let req_clone = outgoing_request.clone();

    retry_policy
        .execute_with_retry(move || {
            let state = state_clone.clone();
            let req = req_clone.clone();
            async move {
                let response = state
                    .client
                    .post("https://openrouter.ai/api/v1/chat/completions")
                    .header("Authorization", format!("Bearer {}", state.openrouter_key))
                    .json(&req)
                    .send()
                    .await
                    .map_err(|e| ObservedError::from(ParallaxError::Network(e)))?;

                let status = response.status();
                if status.is_success() {
                    Ok(response)
                } else {
                    let error_body = match response.text().await {
                        Ok(text) => text,
                        Err(_) => "Unknown error".to_string(),
                    };
                    Err(ObservedError::from(ParallaxError::Upstream(
                        status, error_body,
                    )))
                }
            }
        })
        .await
}

async fn handle_non_streaming_response(
    response: reqwest::Response,
    recorder: &mut crate::debug_utils::FlightRecorder,
    cid: &str,
    tid: &str,
) -> Response {
    let status = response.status();
    let mut body = match response.json::<serde_json::Value>().await {
        Ok(b) => b,
        Err(_) => serde_json::Value::Null,
    };

    recorder.record_stage("upstream_response", body.clone());

    // Phase 2: Write upstream response
    let bundle_manager = crate::debug_bundle::BundleManager::new("debug_capture");
    if let Ok(blob_ref) = bundle_manager
        .write_blob(cid, tid, "upstream_response", body.to_string().as_bytes())
        .await
    {
        let _ = bundle_manager
            .add_stage(
                cid,
                tid,
                "upstream_response",
                blob_ref,
                serde_json::json!({
                    "len": body.to_string().len(),
                    "status": status.as_u16(),
                }),
            )
            .await;
    }

    crate::logging::sanitize_response_body(&mut body);
    recorder.record_stage("sanitized_response", body.clone());
    crate::logging::log_response_summary(&body);

    (status, Json(body)).into_response()
}

async fn project_request(
    state: &Arc<AppState>,
    context: &ConversationContext,
    model_id: &str,
    flavor: Arc<dyn ProviderFlavor + Send + Sync>,
    intent: Option<crate::tui::Intent>,
) -> std::result::Result<crate::specs::openai::OpenAiRequest, Response> {
    Ok(OpenRouterAdapter::project(context, model_id, flavor.as_ref(), &state.db, intent).await)
}

#[allow(clippy::too_many_arguments)]
async fn handle_upstream_response(
    response: reqwest::Response,
    state: Arc<AppState>,
    context: ConversationContext,
    model_id: String,
    request_id: String,
    start_time: std::time::Instant,
    recorder: &mut crate::debug_utils::FlightRecorder,
    tools_were_advertised: bool,
    tid: String,
) -> Response {
    let status = response.status();
    tracing::info!("[☁️  -> ⚙️ ] Status: {}", status);

    if !status.is_success() {
        let error_body = match response.text().await {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("Failed to read error body: {}", e);
                format!("Upstream error (body unreadable): {}", e)
            }
        };
        tracing::error!("[☁️  -> ⚙️ ] Upstream Error: {}", error_body);
        recorder.record_stage("upstream_error", serde_json::json!({ "body": error_body }));
        return (status, error_body).into_response();
    }

    let bytes_stream = response
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let lines_stream = FramedRead::new(
        tokio_util::io::StreamReader::new(bytes_stream),
        LinesCodec::new_with_max_length(1024 * 1024), // 1MB per line
    );

    let (tx, rx) = mpsc::channel(100);
    let db = state.db.clone();
    let _conversation_id = context.conversation_id.clone();
    let tx_tui = state.tx_tui.clone();
    let pricing = state.pricing.clone();
    let disable_rescue = state.disable_rescue;

    let current_span = tracing::Span::current();
    let _ = current_span;
    let state_clone = state.clone();
    let rid_clone = request_id.clone();
    let cid_clone = context.conversation_id.clone();
    let model_clone = model_id.clone();

    tokio::spawn(async move {
        let stream_id = uuid::Uuid::new_v4().to_string();
        let stream_span = tracing::info_span!(
            "stream",
            rid = %rid_clone,
            cid = %crate::str_utils::prefix_chars(&cid_clone, 6),
            model = %model_clone,
            stream_id = %stream_id
        );

        StreamHandler::handle_stream(
            lines_stream,
            db,
            cid_clone,
            request_id,
            tx,
            model_id,
            pricing,
            tx_tui,
            start_time,
            disable_rescue,
            tools_were_advertised,
            state_clone,
            tid,
        )
        .instrument(stream_span)
        .await;
    });

    Sse::new(ReceiverStream::new(rx))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text(": keepalive"),
        )
        .into_response()
}

/// Build the debug UI on startup
#[allow(clippy::cognitive_complexity)]
fn build_debug_ui() {
    use std::process::Command;

    tracing::info!("Building debug UI...");

    // Check if debug_ui directory exists
    if !std::path::Path::new("debug_ui").exists() {
        tracing::warn!("debug_ui directory not found, skipping UI build");
        return;
    }

    // Check if node_modules exists, if not run npm install
    if !std::path::Path::new("debug_ui/node_modules").exists() {
        tracing::info!("Installing UI dependencies...");
        let install_result = Command::new("npm")
            .args(["install"])
            .current_dir("debug_ui")
            .status();

        match install_result {
            Ok(status) if status.success() => {
                tracing::info!("UI dependencies installed successfully");
            }
            Ok(status) => {
                tracing::warn!("npm install failed with status: {}", status);
                return;
            }
            Err(e) => {
                tracing::warn!("Failed to run npm install: {}", e);
                return;
            }
        }
    }

    // Build the UI
    let build_result = Command::new("npm")
        .args(["run", "build"])
        .current_dir("debug_ui")
        .status();

    match build_result {
        Ok(status) if status.success() => {
            tracing::info!("Debug UI built successfully");
        }
        Ok(status) => {
            tracing::warn!("UI build failed with status: {}", status);
        }
        Err(e) => {
            tracing::warn!("Failed to run UI build: {}", e);
        }
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    // Setup TUI channel
    let (tx_tui, rx_tui) = broadcast::channel(100);

    // Setup Custom Logger that pipes to TUI
    use tracing_subscriber::prelude::*;

    let filter = match tracing_subscriber::EnvFilter::try_from_default_env() {
        Ok(f) => f,
        Err(_) => "parallax=debug,parallax::tui=off,parallax::streaming=off".into(),
    };

    // Setup file logging
    let file_appender = tracing_appender::rolling::daily(".", "parallax.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Setup Agent NDJSON tracing (logs/trace_buffer.json)
    let _ = std::fs::create_dir_all("logs");
    let agent_appender = tracing_appender::rolling::daily("logs", "trace_buffer.json");
    let (agent_non_blocking, _agent_guard) = tracing_appender::non_blocking(agent_appender);

    // Combine everything
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false),
        )
        .with(parallax::agent_layer::AgentNdjsonLayer::new(
            parallax::redaction_layer::RedactingWriter::new(agent_non_blocking),
        ))
        .with(TuiLayer { tx: tx_tui.clone() })
        .with(tracing_error::ErrorLayer::default())
        .init();

    // Initialize global panic hook
    parallax::logging::setup_panic_hook();

    // Setup log rotation manager
    let log_rotation_config = LogRotationConfig::default();
    let log_rotation_manager = LogRotationManager::new(log_rotation_config);

    // Check and rotate logs on startup
    let _ = log_rotation_manager.check_and_rotate(std::path::Path::new("."), "parallax.log");
    let _ =
        log_rotation_manager.check_and_rotate(std::path::Path::new("logs"), "trace_buffer.json");

    let args = Arc::new(Args::parse());

    // Build debug UI on startup
    build_debug_ui();

    let db = match init_db(&args.database).await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("Failed to initialize database: {}", e);
            std::process::exit(1);
        }
    };
    let openrouter_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("Error: OPENROUTER_API_KEY environment variable is missing or empty.");
            eprintln!("Please set it in your .env file or environment.");
            std::process::exit(1);
        }
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(args.request_timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(args.connect_timeout_secs))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(Some(std::time::Duration::from_secs(60)))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to build HTTP client: {}", e);
            std::process::exit(1);
        }
    };

    let pricing = fetch_pricing(&client).await;
    if pricing.is_empty() {
        tracing::warn!(
            "Warning: Could not fetch pricing from OpenRouter. Cost tracking will be unavailable."
        );
    } else {
        tracing::info!("Fetched pricing for {} models", pricing.len());
    }

    let health = Arc::new(UpstreamHealth::default());
    let circuit_breaker = Arc::new(crate::hardening::CircuitBreaker::new(
        args.circuit_breaker_threshold,
        std::time::Duration::from_secs(30),
    ));

    let state = Arc::new(AppState {
        client,
        openrouter_key,
        db,
        tx_tui: tx_tui.clone(),
        pricing: Arc::new(pricing),
        disable_rescue: args.disable_rescue,
        args: args.clone(),
        tx_kernel: mpsc::channel(1).0, // Placeholder for now, check if needed
        health,
        circuit_breaker,
    });

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/chat/completions", post(chat_completions_handler))
        .route("/health", axum::routing::get(health::liveness))
        .route("/readyz", axum::routing::get(health::readiness))
        .route(
            "/admin/conversation/:cid",
            axum::routing::get(health::admin_conversation),
        )
        // Debug API
        .route(
            "/debug/conversations",
            axum::routing::get(list_conversations),
        )
        .route(
            "/debug/conversation/:cid",
            axum::routing::get(get_conversation),
        )
        .route(
            "/debug/conversation/:cid/turn/:tid",
            axum::routing::get(get_turn),
        )
        .route("/debug/blob/:cid/:tid/:bid", axum::routing::get(get_blob))
        .route(
            "/debug/diff/:cid/:tid",
            axum::routing::post(compute_stage_diff),
        )
        .route(
            "/debug/export/conversation/:cid",
            axum::routing::get(export_conversation),
        )
        .route(
            "/debug/export/turn/:cid/:tid",
            axum::routing::get(export_turn),
        )
        .route("/debug/replay/:cid/:tid", axum::routing::post(replay_turn))
        // Serve static UI with SPA fallback
        .route("/debug/ui", axum::routing::get(debug_ui_root))
        .route("/debug/ui/*path", axum::routing::get(debug_ui_handler))
        .layer(axum::extract::DefaultBodyLimit::max(args.max_body_size))
        .layer(middleware::from_fn(turn_id_middleware))
        .with_state(state.clone());

    let addr = format!("{}:{}", args.host, args.port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind to {}: {}", addr, e);
            std::process::exit(1);
        }
    };

    // Spawn Server
    let _server_handle = tokio::spawn(async move {
        tracing::info!("Parallax listening on {}", addr);
        use futures_util::FutureExt;

        let server_future = async move { axum::serve(listener, app).await };

        match std::panic::AssertUnwindSafe(server_future)
            .catch_unwind()
            .await
        {
            Ok(result) => {
                if let Err(e) = result {
                    tracing::error!("Server error: {}", e);
                }
            }
            Err(panic_payload) => {
                let message = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    *s
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.as_str()
                } else {
                    "Unknown panic"
                };
                tracing::error!(target: "panic", "CRITICAL: Server task panicked: {}", message);
            }
        }
    });

    // Run TUI on main thread
    let app_tui = App::new(rx_tui);

    if let Err(e) = app_tui.run().await {
        eprintln!("TUI Error: {}", e);
    }
}

// --- DEBUG API HANDLERS ---

async fn list_conversations() -> impl IntoResponse {
    let mut conversations = Vec::new();
    let mut read_dir = match tokio::fs::read_dir("debug_capture/conversations").await {
        Ok(rd) => rd,
        Err(_) => return Json(conversations).into_response(),
    };

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            let _cid = entry.file_name().to_string_lossy().to_string();
            let summary_path = entry.path().join("conversation.json");
            if let Ok(content) = tokio::fs::read_to_string(summary_path).await {
                if let Ok(summary) =
                    serde_json::from_str::<crate::debug_bundle::ConversationSummary>(&content)
                {
                    conversations.push(summary);
                }
            }
        }
    }

    conversations.sort_by_key(|c| std::cmp::Reverse(c.last_updated_ms));
    Json(conversations).into_response()
}

async fn get_conversation(
    axum::extract::Path(cid): axum::extract::Path<String>,
) -> impl IntoResponse {
    let path = format!("debug_capture/conversations/{}/conversation.json", cid);
    match tokio::fs::read_to_string(path).await {
        Ok(content) => (ax_http::StatusCode::OK, content).into_response(),
        Err(_) => (ax_http::StatusCode::NOT_FOUND, "Conversation not found").into_response(),
    }
}

async fn get_turn(
    axum::extract::Path((cid, tid)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    let path = format!(
        "debug_capture/conversations/{}/turns/{}/turn.json",
        cid, tid
    );
    match tokio::fs::read_to_string(path).await {
        Ok(content) => (ax_http::StatusCode::OK, content).into_response(),
        Err(_) => (ax_http::StatusCode::NOT_FOUND, "Turn not found").into_response(),
    }
}

async fn get_blob(
    axum::extract::Path((cid, tid, bid)): axum::extract::Path<(String, String, String)>,
) -> impl IntoResponse {
    let blobs_dir = format!("debug_capture/conversations/{}/turns/{}/blobs", cid, tid);

    // Try to find the blob with any extension (.json, .txt, .bin)
    let extensions = vec![".json", ".txt", ".bin"];
    for ext in extensions {
        let path = format!("{}/{}{}", blobs_dir, bid, ext);
        if let Ok(bytes) = tokio::fs::read(&path).await {
            let content_type = match ext {
                ".json" => "application/json",
                ".txt" => "text/plain; charset=utf-8",
                ".bin" => "application/octet-stream",
                _ => "application/octet-stream",
            };
            return (
                ax_http::StatusCode::OK,
                [(ax_http::header::CONTENT_TYPE, content_type)],
                bytes,
            )
                .into_response();
        }
    }

    (ax_http::StatusCode::NOT_FOUND, "Blob not found").into_response()
}

async fn compute_stage_diff(
    axum::extract::Path((cid, tid)): axum::extract::Path<(String, String)>,
    axum::extract::Json(payload): axum::extract::Json<serde_json::Value>,
) -> impl IntoResponse {
    let stage1 = payload.get("stage1").and_then(|s| s.as_str());
    let stage2 = payload.get("stage2").and_then(|s| s.as_str());

    if stage1.is_none() || stage2.is_none() {
        return (
            ax_http::StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": "Missing stage1 or stage2" })),
        )
            .into_response();
    }

    // Read both stage blobs
    let blob1_path = format!(
        "debug_capture/conversations/{}/turns/{}/blobs/{}.json",
        cid,
        tid,
        stage1.unwrap()
    );
    let blob2_path = format!(
        "debug_capture/conversations/{}/turns/{}/blobs/{}.json",
        cid,
        tid,
        stage2.unwrap()
    );

    let blob1_content = match tokio::fs::read_to_string(&blob1_path).await {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(val) => val,
            Err(_) => {
                return (
                    ax_http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({ "error": "Failed to parse stage1 JSON" })),
                )
                    .into_response();
            }
        },
        Err(_) => {
            return (
                ax_http::StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({ "error": "stage1 blob not found" })),
            )
                .into_response();
        }
    };

    let blob2_content = match tokio::fs::read_to_string(&blob2_path).await {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(val) => val,
            Err(_) => {
                return (
                    ax_http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({ "error": "Failed to parse stage2 JSON" })),
                )
                    .into_response();
            }
        },
        Err(_) => {
            return (
                ax_http::StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({ "error": "stage2 blob not found" })),
            )
                .into_response();
        }
    };

    // Compute diff
    let diff =
        crate::debug_bundle::BundleManager::compute_json_diff(&blob1_content, &blob2_content);

    (
        ax_http::StatusCode::OK,
        axum::Json(serde_json::json!({
            "stage1": stage1,
            "stage2": stage2,
            "diff": diff,
        })),
    )
        .into_response()
}

async fn export_conversation(
    axum::extract::Path(cid): axum::extract::Path<String>,
) -> impl IntoResponse {
    use std::io::Write;

    // Read conversation summary
    let conv_path = format!("debug_capture/conversations/{}/conversation.json", cid);
    let conv_content = match tokio::fs::read_to_string(&conv_path).await {
        Ok(content) => content,
        Err(_) => {
            return (ax_http::StatusCode::NOT_FOUND, "Conversation not found").into_response();
        }
    };

    // Create a zip file in memory
    let mut zip_buffer = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_buffer));
        let options = zip::write::FileOptions::default();

        // Add conversation.json
        let _ = zip.start_file("conversation.json", options);
        let _ = zip.write_all(conv_content.as_bytes());

        // Add all turn directories
        let turns_dir = format!("debug_capture/conversations/{}/turns", cid);
        if let Ok(mut entries) = tokio::fs::read_dir(&turns_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(tid) = path.file_name().and_then(|n| n.to_str()) {
                        // Add turn.json
                        let turn_path = format!("{}/{}/turn.json", turns_dir, tid);
                        if let Ok(turn_content) = tokio::fs::read_to_string(&turn_path).await {
                            let _ = zip.start_file(format!("turns/{}/turn.json", tid), options);
                            let _ = zip.write_all(turn_content.as_bytes());
                        }

                        // Add blobs
                        let blobs_dir = format!("{}/{}/blobs", turns_dir, tid);
                        if let Ok(mut blob_entries) = tokio::fs::read_dir(&blobs_dir).await {
                            while let Ok(Some(blob_entry)) = blob_entries.next_entry().await {
                                let blob_path = blob_entry.path();
                                if blob_path.is_file() {
                                    if let Some(blob_name) =
                                        blob_path.file_name().and_then(|n| n.to_str())
                                    {
                                        if let Ok(blob_content) = tokio::fs::read(&blob_path).await
                                        {
                                            let _ = zip.start_file(
                                                format!("turns/{}/blobs/{}", tid, blob_name),
                                                options,
                                            );
                                            let _ = zip.write_all(&blob_content);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let _ = zip.finish();
    }

    (
        ax_http::StatusCode::OK,
        [
            (ax_http::header::CONTENT_TYPE, "application/zip"),
            (
                ax_http::header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"conversation-{}.zip\"", cid),
            ),
        ],
        zip_buffer,
    )
        .into_response()
}

async fn export_turn(
    axum::extract::Path((cid, tid)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    use std::io::Write;

    // Read turn.json
    let turn_path = format!(
        "debug_capture/conversations/{}/turns/{}/turn.json",
        cid, tid
    );
    let turn_content = match tokio::fs::read_to_string(&turn_path).await {
        Ok(content) => content,
        Err(_) => {
            return (ax_http::StatusCode::NOT_FOUND, "Turn not found").into_response();
        }
    };

    // Create a zip file in memory
    let mut zip_buffer = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_buffer));
        let options = zip::write::FileOptions::default();

        // Add turn.json
        let _ = zip.start_file("turn.json", options);
        let _ = zip.write_all(turn_content.as_bytes());

        // Add blobs
        let blobs_dir = format!("debug_capture/conversations/{}/turns/{}/blobs", cid, tid);
        if let Ok(mut blob_entries) = tokio::fs::read_dir(&blobs_dir).await {
            while let Ok(Some(blob_entry)) = blob_entries.next_entry().await {
                let blob_path = blob_entry.path();
                if blob_path.is_file() {
                    if let Some(blob_name) = blob_path.file_name().and_then(|n| n.to_str()) {
                        if let Ok(blob_content) = tokio::fs::read(&blob_path).await {
                            let _ = zip.start_file(format!("blobs/{}", blob_name), options);
                            let _ = zip.write_all(&blob_content);
                        }
                    }
                }
            }
        }

        let _ = zip.finish();
    }

    (
        ax_http::StatusCode::OK,
        [
            (ax_http::header::CONTENT_TYPE, "application/zip"),
            (
                ax_http::header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"turn-{}.zip\"", tid),
            ),
        ],
        zip_buffer,
    )
        .into_response()
}

async fn replay_turn(
    axum::extract::Path((cid, tid)): axum::extract::Path<(String, String)>,
    axum::extract::Json(payload): axum::extract::Json<serde_json::Value>,
) -> impl IntoResponse {
    // Read the ingress_raw blob to get the original payload
    let ingress_path = format!(
        "debug_capture/conversations/{}/turns/{}/blobs/ingress_raw.json",
        cid, tid
    );
    let ingress_content = match tokio::fs::read_to_string(&ingress_path).await {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(val) => val,
            Err(_) => {
                return (
                    ax_http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({ "error": "Failed to parse ingress_raw JSON" })),
                )
                    .into_response();
            }
        },
        Err(_) => {
            return (
                ax_http::StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({ "error": "ingress_raw blob not found" })),
            )
                .into_response();
        }
    };

    // Extract options from payload
    let stages_to_run = payload
        .get("stages")
        .and_then(|s| s.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["projected", "final"]);

    // For now, return a simple response indicating replay capability
    // Full replay would require re-running the engine with the stored payload
    (
        ax_http::StatusCode::OK,
        axum::Json(serde_json::json!({
            "status": "replay_capability_available",
            "message": "Replay endpoint is available for future implementation",
            "ingress_payload_size": ingress_content.to_string().len(),
            "requested_stages": stages_to_run,
            "note": "Full replay requires re-running the engine with stored payload and current code"
        })),
    )
        .into_response()
}

#[allow(clippy::cognitive_complexity)]
async fn debug_ui_handler(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    // Strip leading slash if present and construct path
    let clean_path = path.trim_start_matches('/');
    let file_path = format!("debug_ui/dist/{}", clean_path);

    tracing::debug!("Serving UI asset: {} (from path: {})", file_path, path);

    match tokio::fs::read(&file_path).await {
        Ok(content) => {
            let content_type = if file_path.ends_with(".js") {
                "application/javascript; charset=utf-8"
            } else if file_path.ends_with(".css") {
                "text/css; charset=utf-8"
            } else if file_path.ends_with(".json") {
                "application/json"
            } else if file_path.ends_with(".svg") {
                "image/svg+xml"
            } else if file_path.ends_with(".html") {
                "text/html; charset=utf-8"
            } else {
                "application/octet-stream"
            };

            ([(ax_http::header::CONTENT_TYPE, content_type)], content).into_response()
        }
        Err(e) => {
            tracing::warn!("Failed to serve asset {}: {}", file_path, e);
            // Fallback to index.html for SPA routing
            match tokio::fs::read("debug_ui/dist/index.html").await {
                Ok(content) => (
                    [(ax_http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    content,
                )
                    .into_response(),
                Err(e) => {
                    tracing::error!("Failed to read index.html: {}", e);
                    (
                        ax_http::StatusCode::NOT_FOUND,
                        "Debug UI not found. Run: cd debug_ui && npm run build",
                    )
                        .into_response()
                }
            }
        }
    }
}

async fn debug_ui_root() -> impl IntoResponse {
    // Serve index.html at /debug/ui root
    match tokio::fs::read("debug_ui/dist/index.html").await {
        Ok(content) => (
            [(ax_http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            content,
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Failed to read index.html: {}", e);
            (
                ax_http::StatusCode::NOT_FOUND,
                "Debug UI not found. Run: cd debug_ui && npm run build",
            )
                .into_response()
        }
    }
}
