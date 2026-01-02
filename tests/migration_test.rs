use tempfile::tempdir;
use parallax::db::{init_db, cleanup_old_data};

#[tokio::test]
async fn test_migrations_and_schema() {
    let dir = match tempdir() {
        Ok(d) => d,
        Err(e) => panic!("Failed to create temp dir: {:?}", e),
    };
    let db_path = dir.path().join("test_parallax.db");
    
    // 1. Initialize DB (runs migrations)
    let pool = match init_db(&db_path).await {
        Ok(p) => p,
        Err(e) => panic!("Failed to init DB: {:?}", e),
    };

    // 2. Verify WAL mode
    let journal_mode: (String,) = match sqlx::query_as("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await {
            Ok(jm) => jm,
            Err(e) => panic!("Failed to query journal_mode: {:?}", e),
        };
    assert_eq!(journal_mode.0.to_uppercase(), "WAL");

    // 3. Verify Tables exist
    let tables: Vec<(String,)> = match sqlx::query_as("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(&pool)
        .await {
            Ok(t) => t,
            Err(e) => panic!("Failed to query tables: {:?}", e),
        };
    
    let table_names: Vec<String> = tables.into_iter().map(|t| t.0).collect();
    assert!(table_names.contains(&"conversation_states".to_string()));
    assert!(table_names.contains(&"tool_signatures".to_string()));
    assert!(table_names.contains(&"schema_metadata".to_string()));
    assert!(!table_names.contains(&"usage_stats".to_string()), "usage_stats should have been dropped");

    // 4. Verify Indexes exist
    let indexes: Vec<(String,)> = match sqlx::query_as("SELECT name FROM sqlite_master WHERE type='index'")
        .fetch_all(&pool)
        .await {
            Ok(i) => i,
            Err(e) => panic!("Failed to query indexes: {:?}", e),
        };
    
    let index_names: Vec<String> = indexes.into_iter().map(|i| i.0).collect();
    assert!(index_names.contains(&"idx_tool_signatures_conversation_id".to_string()));
    assert!(index_names.contains(&"idx_tool_signatures_created_at".to_string()));

    // 5. Verify Columns in tool_signatures
    let columns: Vec<(i64, String, String, i64, Option<String>, i64)> = match sqlx::query_as("PRAGMA table_info(tool_signatures)")
        .fetch_all(&pool)
        .await {
            Ok(c) => c,
            Err(e) => panic!("Failed to query table_info: {:?}", e),
        };
    
    let col_names: Vec<String> = columns.into_iter().map(|c| c.1).collect();
    assert!(col_names.contains(&"reasoning_tokens".to_string()));
    assert!(col_names.contains(&"thought_signature".to_string()));

    pool.close().await;
}

#[tokio::test]
async fn test_retention_cleanup() {
    let dir = match tempdir() {
        Ok(d) => d,
        Err(e) => panic!("Failed to create temp dir: {:?}", e),
    };
    let db_path = dir.path().join("test_cleanup.db");
    let pool = match init_db(&db_path).await {
        Ok(p) => p,
        Err(e) => panic!("Failed to init DB: {:?}", e),
    };

    // Insert old data (8 days ago)
    match sqlx::query("INSERT INTO tool_signatures (id, conversation_id, signature, created_at) VALUES (?, ?, ?, datetime('now', '-8 days'))")
        .bind("old_id")
        .bind("conv_1")
        .bind("{}")
        .execute(&pool)
        .await {
            Ok(_) => (),
            Err(e) => panic!("Failed to insert old data: {:?}", e),
        };

    // Insert new data (now)
    match sqlx::query("INSERT INTO tool_signatures (id, conversation_id, signature, created_at) VALUES (?, ?, ?, datetime('now'))")
        .bind("new_id")
        .bind("conv_1")
        .bind("{}")
        .execute(&pool)
        .await {
            Ok(_) => (),
            Err(e) => panic!("Failed to insert new data: {:?}", e),
        };

    // Run cleanup for 7 days
    match cleanup_old_data(&pool, 7).await {
        Ok(_) => (),
        Err(e) => panic!("Cleanup failed: {:?}", e),
    };

    // Verify
    let count: (i64,) = match sqlx::query_as("SELECT COUNT(*) FROM tool_signatures")
        .fetch_one(&pool)
        .await {
            Ok(c) => c,
            Err(e) => panic!("Failed to count signatures: {:?}", e),
        };
    
    assert_eq!(count.0, 1);

    pool.close().await;
}