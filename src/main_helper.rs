use crate::types::*;
use std::sync::Arc;
use tokio::sync::broadcast;
use crate::db::DbPool;
use crate::tui::TuiEvent;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, default_value_t = 8080)]
    pub port: u16,
    #[arg(short, long, default_value = "127.0.0.1")]
    pub host: String,
    #[arg(short, long, default_value = "parallax.db")]
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
    pub health: Arc<UpstreamHealth>,
    pub circuit_breaker: Arc<crate::hardening::CircuitBreaker>,
}

impl AppState {
    pub async fn new(args: Args) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(args.request_timeout_secs))
            .connect_timeout(std::time::Duration::from_secs(args.connect_timeout_secs))
            .build()
            .map_err(ParallaxError::Network)?;

        let openrouter_key = std::env::var("OPENROUTER_API_KEY")
            .map_err(|_| ParallaxError::Internal("OPENROUTER_API_KEY not set".to_string(), tracing_error::SpanTrace::capture()))?;

        let db = crate::db::init_db(&args.database).await?;
        let (tx_tui, _) = tokio::sync::broadcast::channel(100);
        
        let pricing = Arc::new(std::collections::HashMap::new()); // Placeholder
        let health = Arc::new(UpstreamHealth { 
            consecutive_failures: 0.into(),
            total_requests: 0.into(),
            failed_requests: 0.into(),
            last_success: None.into(),
            last_failure: None.into(),
        });
        let circuit_breaker = Arc::new(crate::hardening::CircuitBreaker::new(args.circuit_breaker_threshold, std::time::Duration::from_secs(30)));

        Ok(Self {
            client,
            openrouter_key,
            db,
            tx_tui,
            pricing,
            disable_rescue: args.disable_rescue,
            args: Arc::new(args),
            health,
            circuit_breaker,
        })
    }
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
            return Err(ParallaxError::Internal(format!(
                "unknown model name: {}",
                model_id
            ), tracing_error::SpanTrace::capture()).into())
        }
    };

    let cached_tokens = match usage.prompt_tokens_details.as_ref() {
        Some(details) => details.cached_tokens.unwrap_or_default(),
        None => 0,
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
        ).into());
    }

    Ok(CostBreakdown {
        actual_cost: total,
        potential_cost_no_cache: potential_total,
    })
}
