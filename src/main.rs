use parallax::engine::ParallaxEngine;
use parallax::ingress::RawTurn;
use parallax::str_utils;
use parallax::types::*;
use parallax::AppState;
use axum::{
    extract::State,
    response::sse::Sse,
    routing::post,
    Json, Router,
};
use futures_util::stream::Stream;
use futures_util::StreamExt;
use std::sync::Arc;
use std::net::SocketAddr;
use clap::Parser;

pub fn clean_content_for_intent(raw_content: &str) -> Option<parallax::tui::Intent> {
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

    let search_window = str_utils::suffix_chars(&clean_content, 500);
    detect_intent_keywords(search_window)
}

fn detect_intent_tag(clean_content: &str) -> Option<parallax::tui::Intent> {
    if let Some(start) = clean_content.find("<system_reminder>") {
        let after_start_idx = start + "<system_reminder>".len();
        if let Some(after_start) =
            str_utils::slice_bytes_safe(clean_content, after_start_idx, clean_content.len())
            && let Some(end_offset) = after_start.find("</system_reminder>")
            && let Some(reminder_content) = str_utils::slice_bytes_safe(after_start, 0, end_offset)
        {
            let upper_reminder = reminder_content.to_uppercase();
            let intent = if upper_reminder.contains("AGENT")
                || upper_reminder.contains("COMPOSER")
                || upper_reminder.contains("BUILD")
            {
                parallax::tui::Intent::Agent
            } else if upper_reminder.contains("PLAN") {
                parallax::tui::Intent::Plan
            } else if upper_reminder.contains("DEBUG") {
                parallax::tui::Intent::Debug
            } else {
                parallax::tui::Intent::Ask
            };

            tracing::debug!(
                "Intent detected via <system_reminder>: {:?} (snippet: {:?})",
                intent,
                str_utils::prefix_chars(reminder_content, 50)
            );
            return Some(intent);
        }
    }
    None
}

fn detect_intent_keywords(search_window: &str) -> Option<parallax::tui::Intent> {
    let content = search_window.to_uppercase();

    let intent = if content.contains(" PLAN MODE") || content.contains(" PLANNING MODE") {
        Some(parallax::tui::Intent::Plan)
    } else if content.contains(" AGENT MODE")
        || content.contains(" COMPOSER MODE")
        || content.contains(" BUILD MODE")
    {
        Some(parallax::tui::Intent::Agent)
    } else if content.contains(" DEBUG MODE") {
        Some(parallax::tui::Intent::Debug)
    } else if content.contains(" ASK MODE") || content.contains(" CHAT MODE") {
        Some(parallax::tui::Intent::Ask)
    } else {
        None
    };

    if let Some(i) = intent {
        tracing::debug!(
            "Intent detected via keywords: {:?} (window: {:?})",
            i,
            str_utils::prefix_chars(search_window, 50)
        );
    }

    intent
}

#[tracing::instrument(
    name = "shim.request",
    skip_all,
    fields(
        conversation_id = %ingress.generate_anchor_hash().unwrap_or_default(),
        request_id = %ingress.extract_request_id(),
        model = %ingress.model.model_name()
    )
)]
pub async fn handle_parallax_request(
    State(state): State<Arc<AppState>>,
    Json(ingress): Json<RawTurn>,
) -> parallax::types::Result<Sse<impl Stream<Item = std::result::Result<axum::response::sse::Event, ParallaxError>>>> {
    let start_time = std::time::Instant::now();
    let conversation_id = ingress.generate_anchor_hash().map_err(|e| ParallaxError::InvalidIngress(e.to_string()))?;
    let request_id = ingress.extract_request_id();
    let model_id = ingress.model.model_name().to_string();

    ingress.validate().map_err(|e| {
        tracing::error!("Ingress validation failed: {}", e);
        e
    })?;

    let intent = ingress
        .messages
        .last()
        .and_then(|h| match &h.content {
            Some(parallax::ingress::RawContent::String(s)) => clean_content_for_intent(s),
            _ => None,
        })
        .unwrap_or(parallax::tui::Intent::Ask);

    let entry = ParallaxEngine::lift(serde_json::to_value(&ingress).map_err(ParallaxError::Serialization)?, &state.db).await?;
    
    let (flavor, op) = match entry {
        parallax::engine::TurnOperationEntry::Gemini(op) => (parallax::projections::resolve_flavor_for_kind(parallax::projections::ProviderKind::Google), op),
        parallax::engine::TurnOperationEntry::Anthropic(op) => (parallax::projections::resolve_flavor_for_kind(parallax::projections::ProviderKind::Anthropic), op),
        parallax::engine::TurnOperationEntry::OpenAI(op) => (parallax::projections::resolve_flavor_for_kind(parallax::projections::ProviderKind::OpenAi), op),
        parallax::engine::TurnOperationEntry::Standard(op) => (parallax::projections::resolve_flavor_for_kind(parallax::projections::ProviderKind::Standard), op),
    };

    let projected_request = parallax::projections::OpenRouterAdapter::project(
        &op.input_context,
        &model_id,
        flavor.as_ref(),
        &state.db,
        Some(intent),
    ).await;

    parallax::debug_utils::capture_debug_snapshot(
        "ingress_raw",
        &model_id,
        &conversation_id,
        &request_id,
        &serde_json::to_value(&ingress).unwrap_or(serde_json::Value::Null),
    )
    .await;

    let _ = state.tx_tui.send(parallax::tui::TuiEvent::RequestStarted {
        id: request_id.clone(),
        cid: conversation_id.clone(),
        method: format!("{:?}", intent),
        intent: Some(intent),
        model: model_id.clone(),
    });

    let (tx, rx) = tokio::sync::mpsc::channel(100);
    let db = state.db.clone();
    let pricing = state.pricing.clone();
    let tx_tui = state.tx_tui.clone();
    let disable_rescue = false; // Default
    let tools_were_advertised = projected_request.tools.as_ref().is_some_and(|t| !t.is_empty());
    let state_clone = Arc::clone(&state);

    tokio::spawn(async move {
        let response = state_clone
            .client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", state_clone.openrouter_key))
            .json(&projected_request)
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                let bytes_stream = resp.bytes_stream().map(|r| r.map_err(std::io::Error::other));
                let lines_stream = tokio_util::codec::FramedRead::new(
                    tokio_util::io::StreamReader::new(bytes_stream),
                    tokio_util::codec::LinesCodec::new_with_max_length(1024 * 1024),
                );

                parallax::streaming::StreamHandler::handle_stream(
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
                .await;
            }
            Ok(resp) => {
                let status = resp.status();
                let err_text = resp.text().await.unwrap_or_else(|_| "Unknown error".to_string());
                tracing::error!("Upstream error: {} - {}", status, err_text);
                let _ = tx.send(Err(ParallaxError::Upstream(status, err_text))).await;
            }
            Err(e) => {
                tracing::error!("Network error: {}", e);
                let _ = tx.send(Err(ParallaxError::Network(e))).await;
            }
        }
    });

    Ok(Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    
    let args = parallax::main_helper::Args::parse();
    let app_state = Arc::new(parallax::main_helper::AppState::new(args).await?);

    let app = Router::new()
        .route("/request", post(handle_parallax_request))
        .with_state(app_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tracing::info!("Listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}
