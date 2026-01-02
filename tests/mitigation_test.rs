use parallax::types::*;
use parallax::debug_utils::FlightRecorder;
use axum::http::StatusCode;

#[tokio::test]
async fn test_upstream_error_classification() {
    let mut recorder = FlightRecorder::new("turn_1", "req_1", "conv_1", "model_1", "flavor_1");
    
    // Test HTML/Cloudflare
    let html_body = "<!DOCTYPE html><html><body>CF-RAY: 1234567890abcdef</body></html>";
    recorder.record_upstream_error(StatusCode::SERVICE_UNAVAILABLE, html_body);
    
    let stage = match recorder.stages.get("upstream_error") {
        Some(s) => s,
        None => panic!("Stage upstream_error not found"),
    };
    assert_eq!(stage["classification"], "HTML/Cloudflare");
    assert_eq!(stage["cf_ray"], "1234567890abcdef");
    
    // Test JSON
    let json_body = r#"{"error": {"message": "Rate limit exceeded", "code": 429}}"#;
    recorder.record_upstream_error(StatusCode::TOO_MANY_REQUESTS, json_body);
    
    let stage = match recorder.stages.get("upstream_error") {
        Some(s) => s,
        None => panic!("Stage upstream_error not found"),
    };
    assert_eq!(stage["classification"], "JSON");
    assert_eq!(stage["json"]["error"]["code"], 429);
}

#[test]
fn test_parse_provider_error() {
    let json = r#"{"error":{"message":"Overloaded","code":503}}"#;
    let event = parse_provider_line(json);
    match event {
        LineEvent::Error(e) => {
            assert_eq!(e.error.code, Some(503));
            assert!(e.error.message.contains("Overloaded"));
        }
        LineEvent::Pulse(_) => panic!("Got Pulse instead of Error"),
        LineEvent::Unknown(s) => panic!("Got Unknown: {}", s),
    }
}

