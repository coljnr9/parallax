use parallax::types::*;
use parallax::hardening::*;
use std::time::Duration;

#[tokio::test]
async fn test_retry_policy_success() {
    let policy = RetryPolicy::new(3, 1);
    let mut attempts = 0;
    
    let result: parallax::types::Result<i32> = policy.execute_with_retry(|| {
        attempts += 1;
        async move { Ok(42) }
    }).await;
    
    match result {
        Ok(val) => assert_eq!(val, 42),
        Err(e) => panic!("Expected Ok, got Err: {:?}", e),
    }
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn test_retry_policy_eventual_success() {
    let policy = RetryPolicy::new(3, 1);
    let mut attempts = 0;
    
    let result: parallax::types::Result<i32> = policy.execute_with_retry(|| {
        attempts += 1;
        let a = attempts;
        async move {
            if a < 3 {
                Err(ParallaxError::Internal("fail".to_string()))
            } else {
                Ok(42)
            }
        }
    }).await;
    
    match result {
        Ok(val) => assert_eq!(val, 42),
        Err(e) => panic!("Expected Ok, got Err: {:?}", e),
    }
    assert_eq!(attempts, 3);
}

#[tokio::test]
async fn test_circuit_breaker_trips() {
    let cb = CircuitBreaker::new(2, Duration::from_secs(1));
    
    // First failure
    cb.record_failure().await;
    assert!(cb.check().await.is_ok());
    
    // Second failure - should trip
    cb.record_failure().await;
    assert!(cb.check().await.is_err());
}

#[tokio::test]
async fn test_circuit_breaker_recovery() {
    let cb = CircuitBreaker::new(1, Duration::from_millis(50));
    
    cb.record_failure().await;
    assert!(cb.check().await.is_err());
    
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Should be half-open and allow one request
    assert!(cb.check().await.is_ok());
}
