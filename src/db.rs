use crate::types::{ParallaxError, Result};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use std::path::Path;

pub type DbPool = SqlitePool;

pub async fn init_db<P: AsRef<Path>>(path: P) -> Result<DbPool> {
    let path_str = match path.as_ref().to_str() {
        Some(s) => s,
        None => {
            return Err(ParallaxError::Internal(
                "Invalid database path: Path contains non-UTF8 characters".to_string(),
                tracing_error::SpanTrace::capture(),
            )
            .into())
        }
    };
    let url = format!("sqlite:{}?mode=rwc", path_str);

    let pool = match SqlitePool::connect(&url).await {
        Ok(p) => p,
        Err(e) => return Err(ParallaxError::Database(e).into()),
    };

    configure_db(&pool).await?;

    // Run Migrations
    if let Err(e) = sqlx::migrate!("./migrations").run(&pool).await {
        return Err(ParallaxError::Internal(
            format!("Migration failed: {}", e),
            tracing_error::SpanTrace::capture(),
        )
        .into());
    }

    verify_schema_version(&pool).await;

    // Run cleanup
    if let Err(e) = cleanup_old_data(&pool, 7).await {
        tracing::warn!("Database cleanup failed: {}", e);
    }

    Ok(pool)
}

async fn configure_db(pool: &DbPool) -> Result<()> {
    // Configure WAL mode and performance pragmas
    let pragmas = [
        "PRAGMA journal_mode = WAL",
        "PRAGMA synchronous = NORMAL",
        "PRAGMA busy_timeout = 5000",
    ];

    for pragma in pragmas {
        if let Err(e) = sqlx::query(pragma).execute(pool).await {
            return Err(ParallaxError::Database(e).into());
        }
    }
    Ok(())
}

async fn verify_schema_version(pool: &DbPool) {
    // Verify Schema Version
    let version_row: std::result::Result<(String,), sqlx::Error> =
        sqlx::query_as("SELECT value FROM schema_metadata WHERE key = 'schema_version'")
            .fetch_one(pool)
            .await;

    match version_row {
        Ok((version,)) => {
            tracing::info!("Database initialized. Schema version: {}", version);
        }
        Err(e) => {
            tracing::warn!("Could not verify schema version: {}", e);
        }
    }
}

pub async fn cleanup_old_data(
    pool: &DbPool,
    retention_days: i64,
) -> std::result::Result<(), sqlx::Error> {
    let threshold = format!("-{} days", retention_days);

    let deleted_sigs =
        sqlx::query("DELETE FROM tool_signatures WHERE created_at < datetime('now', ?)")
            .bind(&threshold)
            .execute(pool)
            .await?;

    let deleted_states =
        sqlx::query("DELETE FROM conversation_states WHERE updated_at < datetime('now', ?)")
            .bind(&threshold)
            .execute(pool)
            .await?;

    if deleted_sigs.rows_affected() > 0 || deleted_states.rows_affected() > 0 {
        println!(
            "Cleanup complete: removed {} signatures and {} conversation states older than {} days.",
            deleted_sigs.rows_affected(),
            deleted_states.rows_affected(),
            retention_days
        );
    }

    Ok(())
}

pub async fn get_conversation_history(
    cid: &str,
    pool: &DbPool,
) -> crate::types::Result<Vec<crate::types::TurnRecord>> {
    let row = sqlx::query("SELECT state_json FROM conversation_states WHERE id = ?")
        .bind(cid)
        .fetch_optional(pool)
        .await?;

    match row {
        Some(r) => {
            let json_str: String = r.get(0);
            let context: crate::types::ConversationContext = serde_json::from_str(&json_str)?;
            Ok(context.history)
        }
        None => Ok(Vec::new()),
    }
}
