#![allow(clippy::manual_unwrap_or_default)]
#![allow(clippy::manual_unwrap_or)]
use crate::db::DbPool;
use crate::tui::TuiEvent;
use crate::types::*;
use clap::Parser;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long, default_value = "parallax.db")]
    pub database: String,
    #[arg(long, default_value_t = false)]
    pub disable_rescue: bool,
    #[arg(long, default_value_t = 120)]
    pub request_timeout_secs: u64,
    #[arg(long, default_value_t = 10)]
    pub connect_timeout_secs: u64,
    #[arg(long, default_value_t = 50 * 1024 * 1024)]
    pub max_body_size: usize,
    #[arg(long, default_value_t = 3)]
    pub max_retries: u32,
    #[arg(long, default_value_t = 5)]
    pub circuit_breaker_threshold: u32,
    #[arg(long, default_value_t = false)]
    pub gemini_fallback: bool,
    #[arg(long, default_value_t = false)]
    pub enable_debug_capture: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub client: reqwest::Client,
    pub openrouter_key: String,
    pub db: DbPool,
    pub tx_tui: broadcast::Sender<TuiEvent>,
    pub pricing: Arc<std::collections::HashMap<String, CostModel>>,
    pub disable_rescue: bool,
    pub args: Arc<Args>,
    pub tx_kernel: tokio::sync::mpsc::Sender<crate::kernel::KernelCommand>,
    pub health: Arc<crate::types::UpstreamHealth>,
    pub circuit_breaker: Arc<crate::hardening::CircuitBreaker>,
}

pub struct CostBreakdown {
    pub actual_cost: f64,
    pub potential_cost_no_cache: f64,
}

pub fn calculate_cost(
    model_id: &str,
    usage: &Usage,
    pricing: &std::collections::HashMap<String, CostModel>,
) -> Result<CostBreakdown> {
    let price = match pricing.get(model_id) {
        Some(p) => p,
        None => {
            return Err(ParallaxError::Internal(
                format!("unknown model name: {}", model_id),
                tracing_error::SpanTrace::capture(),
            )
            .into())
        }
    };

    let cached_tokens = if let Some(c) = usage
        .prompt_tokens_details
        .as_ref()
        .and_then(|details| details.cached_tokens)
    {
        c
    } else {
        0
    };
    let uncached_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);

    let prompt_cost = (uncached_prompt_tokens as f64) * price.prompt;
    let cache_cost = (cached_tokens as f64) * price.prompt_cache_read;
    let completion_cost = (usage.completion_tokens as f64) * price.completion;

    let total = prompt_cost + cache_cost + completion_cost + price.request;

    // Calculate potential cost without cache savings
    let potential_prompt_cost = (usage.prompt_tokens as f64) * price.prompt;
    let potential_total = potential_prompt_cost + completion_cost + price.request;

    if total == 0.0 && (usage.total_tokens > 0) {
        return Err(ParallaxError::Internal(
            "cost not available (pricing returned 0.0 for tokens)".to_string(),
            tracing_error::SpanTrace::capture(),
        )
        .into());
    }

    Ok(CostBreakdown {
        actual_cost: total,
        potential_cost_no_cache: potential_total,
    })
}
