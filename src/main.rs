use parallax::constants::{
    AGENT_KEYWORDS, ASK_KEYWORDS, DEBUG_KEYWORDS, OPENROUTER_CHAT_COMPLETIONS, PLAN_KEYWORDS,
};
use parallax::db::*;
use parallax::engine::*;
use parallax::log_rotation::{LogRotationConfig, LogRotationManager};
use parallax::logging::turn_id_middleware;
use parallax::pricing::fetch_pricing;
use parallax::tui::{App, TuiEvent};
use parallax::*;

use parallax::ingress::RawTurn;
use parallax::projections::OpenRouterAdapter;
use parallax::projections::ProviderFlavor;
use parallax::streaming::StreamHandler;

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

    let intent = if PLAN_KEYWORDS.iter().any(|k| content.contains(k)) {
        Some(crate::tui::Intent::Plan)
    } else if AGENT_KEYWORDS.iter().any(|k| content.contains(k)) {
        Some(crate::tui::Intent::Agent)
    } else if DEBUG_KEYWORDS.iter().any(|k| content.contains(k)) {
        Some(crate::tui::Intent::Debug)
    } else if ASK_KEYWORDS.iter().any(|k| content.contains(k)) {
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
    )
)]
async fn chat_completions_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    let _start = std::time::Instant::now();
    let span = tracing::Span::current();

    if state.args.enable_debug_capture {
        crate::debug_utils::capture_debug_snapshot(
            "ingress_raw",
            "unknown",
            "unknown",
            "unknown",
            &payload,
        )
        .await;
    }

    if let Err(resp) = validate_payload(&payload) {
        span.record("shim.outcome", "client_error");
        return *resp;
    }

    let entry = match ParallaxEngine::lift(payload.clone(), &state.db).await {
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

    // Capture the lifted context for debugging
    let lifted_json = match serde_json::to_value(&context) {
        Ok(val) => val,
        Err(e) => {
            tracing::warn!("Failed to serialize context for debug: {}", e);
            serde_json::Value::Null
        }
    };
    if state.args.enable_debug_capture {
        crate::debug_utils::capture_debug_snapshot(
            "lifted",
            &rid,
            &context.conversation_id,
            &model_id,
            &lifted_json,
        )
        .await;
    }

    let cid = context.conversation_id.clone();
    let turn_id = crate::logging::get_turn_id();

    let intent = detect_intent(&model_id, &payload);

    let mut recorder =
        crate::debug_utils::FlightRecorder::new(&turn_id, &rid, &cid, &model_id, flavor.name());
    recorder.record_stage("ingress_raw", payload.clone());

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
    entry.into_parts()
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

    recorder.record_stage("upstream_request", outgoing_request_json);

    let result = execute_upstream_request(&state, &outgoing_request).await;

    match result {
        Ok(response) => {
            let _ = state
                .tx_kernel
                .send(crate::kernel::KernelCommand::UpdateHealth { success: true })
                .await;
            let _ = state
                .tx_kernel
                .send(crate::kernel::KernelCommand::RecordCircuitSuccess)
                .await;

            let is_streaming: bool = outgoing_request.stream.unwrap_or(false);
            let tools_were_advertised = outgoing_request
                .tools
                .as_ref()
                .map(|t| !t.is_empty())
                .unwrap_or(false);

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
                )
                .await
            } else {
                handle_non_streaming_response(response, recorder).await
            }
        }
        Err(e) => {
            let _ = state
                .tx_kernel
                .send(crate::kernel::KernelCommand::UpdateHealth { success: false })
                .await;
            let _ = state
                .tx_kernel
                .send(crate::kernel::KernelCommand::RecordCircuitFailure)
                .await;

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

    let (tx_resp, rx_resp) = tokio::sync::oneshot::channel();
    let _ = state
        .tx_kernel
        .send(crate::kernel::KernelCommand::CheckCircuit { resp: tx_resp })
        .await;
    match rx_resp.await {
        Ok(res) => res?,
        Err(_) => {
            return Err(ParallaxError::Internal(
                "Kernel disconnected".to_string(),
                tracing_error::SpanTrace::capture(),
            )
            .into())
        }
    }

    let state_clone = state.clone();
    let req_clone = outgoing_request.clone();

    retry_policy
        .execute_with_retry(move || {
            let state = state_clone.clone();
            let req = req_clone.clone();
            async move {
                let response = state
                    .client
                    .post(OPENROUTER_CHAT_COMPLETIONS)
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
) -> Response {
    let status = response.status();
    let mut body = match response.json::<serde_json::Value>().await {
        Ok(b) => b,
        Err(_) => serde_json::Value::Null,
    };

    recorder.record_stage("upstream_response", body.clone());
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
    let conversation_id = context.conversation_id.clone();
    let tx_tui = state.tx_tui.clone();
    let pricing = state.pricing.clone();
    let disable_rescue = state.disable_rescue;

    let current_span = tracing::Span::current();
    let state_clone = state.clone();
    tokio::spawn(async move {
        StreamHandler::handle_stream(
            lines_stream,
            db,
            conversation_id,
            request_id,
            tx,
            model_id,
            pricing,
            tx_tui,
            start_time,
            disable_rescue,
            tools_were_advertised,
            state_clone,
        )
        .instrument(current_span)
        .await;
    });

    Sse::new(ReceiverStream::new(rx)).into_response()
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    // Setup TUI channel
    let (tx_tui, rx_tui) = broadcast::channel(100);

    // Setup Custom Logger that pipes to TUI
    use tracing_subscriber::prelude::*;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "parallax=debug,parallax::tui=off,parallax::streaming=off".into());

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

    let (tx_kernel, rx_kernel) = mpsc::channel(100);
    let kernel = crate::kernel::Kernel::new(
        args.circuit_breaker_threshold,
        std::time::Duration::from_secs(30),
        tx_tui.clone(),
        rx_kernel,
    );
    tokio::spawn(async move {
        kernel.run().await;
    });

    let state = Arc::new(AppState {
        client,
        openrouter_key,
        db,
        tx_tui: tx_tui.clone(),
        pricing: Arc::new(pricing),
        disable_rescue: args.disable_rescue,
        args: args.clone(),
        tx_kernel,
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
