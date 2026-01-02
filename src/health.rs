use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use std::sync::Arc;
use crate::AppState;
use crate::redaction::{redact_value, RedactionLevel};

#[derive(Serialize)]
pub struct LivenessResponse {
    pub status: &'static str,
}

#[derive(Serialize)]
pub struct ReadinessResponse {
    pub status: String,
    pub database: String,
    pub pricing: String,
}

pub async fn liveness() -> Json<LivenessResponse> {
    Json(LivenessResponse { status: "ok" })
}

pub async fn readiness(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ReadinessResponse>) {
    let mut db_ok = true;
    let mut pricing_ok = true;

    // Check DB
    if let Err(e) = sqlx::query("SELECT 1").fetch_one(&state.db).await {
        tracing::error!("Readiness check: DB error: {}", e);
        db_ok = false;
    }

    // Check Pricing
    if state.pricing.is_empty() {
        tracing::error!("Readiness check: Pricing empty");
        pricing_ok = false;
    }

    let status_code = if db_ok && pricing_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status_code,
        Json(ReadinessResponse {
            status: if db_ok && pricing_ok { "ready" } else { "unready" }.to_string(),
            database: if db_ok { "ok" } else { "error" }.to_string(),
            pricing: if pricing_ok { "ok" } else { "empty" }.to_string(),
        }),
    )
}

pub async fn admin_conversation(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    Path(cid): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    // 1. IP Whitelist Check (Local only)
    let ip = addr.ip();
    if !ip.is_loopback() {
        tracing::warn!("Blocked admin access attempt from {}", ip);
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "Unauthorized" })));
    }

    // 2. Query DB for conversation history
    let messages = match crate::db::get_conversation_history(&cid, &state.db).await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("DB Error: {}", e) })),
            );
        }
    };

    if messages.is_empty() {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "Conversation not found" })));
    }

    // 3. Prepare response and redact
    let mut response = serde_json::json!({
        "conversation_id": cid,
        "message_count": messages.len(),
        "messages": messages,
    });

    // Apply strict redaction for admin endpoint to be safe
    redact_value(&mut response, RedactionLevel::Strict);

    (StatusCode::OK, Json(response))
}


