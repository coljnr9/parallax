#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use parallax::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::util::ServiceExt;

    // Helper to setup a test app
    async fn setup_test_app() -> axum::Router {
        let (tx_tui, _) = broadcast::channel(100);
        let db = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE signatures (id TEXT PRIMARY KEY, conversation_id TEXT, metadata TEXT)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE turns (id TEXT PRIMARY KEY, conversation_id TEXT, role TEXT, content TEXT, tool_call_id TEXT)").execute(&db).await.unwrap();

        let state = Arc::new(parallax::AppState {
            client: reqwest::Client::new(),
            openrouter_key: "test".to_string(),
            db,
            tx_tui,
            pricing: Arc::new(HashMap::new()),
            disable_rescue: false,
            args: Arc::new(parallax::Args {
                port: 8080,
                host: "127.0.0.1".to_string(),
                database: "test.db".to_string(),
                disable_rescue: false,
                request_timeout_secs: 120,
                connect_timeout_secs: 10,
                max_body_size: 1024,
                max_retries: 3,
                circuit_breaker_threshold: 5,
                gemini_fallback: false,
                enable_debug_capture: true,
            }),
            health: Arc::new(parallax::types::UpstreamHealth::default()),
            circuit_breaker: Arc::new(parallax::hardening::CircuitBreaker::new(
                5,
                std::time::Duration::from_secs(30),
            )),
        });

        axum::Router::new()
            .route("/health", axum::routing::get(parallax::health::liveness))
            .route("/readyz", axum::routing::get(parallax::health::readiness))
            .route(
                "/admin/conversation/:cid",
                axum::routing::get(parallax::health::admin_conversation),
            )
            .with_state(state)
    }

    #[tokio::test]
    async fn test_health_liveness() {
        let app = setup_test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn test_health_readiness_error() {
        let app = setup_test_app().await;
        // Our setup has empty pricing, so it should fail readiness
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "unready");
        assert_eq!(json["pricing"], "empty");
    }

    #[tokio::test]
    async fn test_admin_unauthorized() {
        let app = setup_test_app().await;
        // ConnectInfo is not present in oneshot usually unless we use a Mock connector
        // For this test we just want to see it fail or handle the missing connect info
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/conversation/test-cid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Axum returns 500 if ConnectInfo is missing and the handler requires it
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
