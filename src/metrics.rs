//! Metrics and Observability Module
//! 
//! Tracks aggregated metrics for tool arguments, JSON parsing, and provider-specific issues
//! to reduce log noise while maintaining observability.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Metrics for a specific provider and model combination
#[derive(Debug, Clone, Default)]
pub struct ProviderMetrics {
    pub empty_args_count: u64,
    pub invalid_json_count: u64,
    pub total_tool_calls: u64,
    pub tools_with_empty_args: HashMap<String, u64>,
}

/// Global metrics aggregator
pub struct MetricsAggregator {
    metrics: Arc<RwLock<HashMap<String, ProviderMetrics>>>,
}

impl MetricsAggregator {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record an empty arguments event
    pub async fn record_empty_args(&self, provider: &str, model: &str, tool_name: &str) {
        let key = format!("{}:{}", provider, model);
        let mut metrics = self.metrics.write().await;
        let entry = metrics.entry(key).or_insert_with(ProviderMetrics::default);
        entry.empty_args_count += 1;
        *entry
            .tools_with_empty_args
            .entry(tool_name.to_string())
            .or_insert(0) += 1;
    }

    /// Record an invalid JSON event
    pub async fn record_invalid_json(&self, provider: &str, model: &str) {
        let key = format!("{}:{}", provider, model);
        let mut metrics = self.metrics.write().await;
        let entry = metrics.entry(key).or_insert_with(ProviderMetrics::default);
        entry.invalid_json_count += 1;
    }

    /// Record a tool call
    pub async fn record_tool_call(&self, provider: &str, model: &str) {
        let key = format!("{}:{}", provider, model);
        let mut metrics = self.metrics.write().await;
        let entry = metrics.entry(key).or_insert_with(ProviderMetrics::default);
        entry.total_tool_calls += 1;
    }

    /// Get metrics for a provider:model combination
    pub async fn get_metrics(&self, provider: &str, model: &str) -> Option<ProviderMetrics> {
        let key = format!("{}:{}", provider, model);
        self.metrics.read().await.get(&key).cloned()
    }

    /// Get all metrics
    pub async fn get_all_metrics(&self) -> HashMap<String, ProviderMetrics> {
        self.metrics.read().await.clone()
    }

    /// Log aggregated metrics summary
    pub async fn log_summary(&self) {
        let metrics = self.metrics.read().await;
        if metrics.is_empty() {
            return;
        }

        tracing::info!("=== METRICS SUMMARY ===");
        for (key, m) in metrics.iter() {
            Self::log_provider_metric(key, m);
        }
        tracing::info!("======================");
    }

    fn log_provider_metric(key: &str, m: &ProviderMetrics) {
        let empty_args_rate = if m.total_tool_calls > 0 {
            (m.empty_args_count as f64 / m.total_tool_calls as f64) * 100.0
        } else {
            0.0
        };

        tracing::info!(
            "Provider {}: {} tool calls | {} empty args ({:.1}%) | {} invalid JSON",
            key,
            m.total_tool_calls,
            m.empty_args_count,
            empty_args_rate,
            m.invalid_json_count
        );

        if !m.tools_with_empty_args.is_empty() {
            let top_tools: Vec<_> = m
                .tools_with_empty_args
                .iter()
                .map(|(name, count)| format!("{}({})", name, count))
                .collect();
            tracing::info!("  Top tools with empty args: {}", top_tools.join(", "));
        }
    }

    /// Reset all metrics
    pub async fn reset(&self) {
        self.metrics.write().await.clear();
    }
}

impl Default for MetricsAggregator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_record_empty_args() {
        let agg = MetricsAggregator::new();
        agg.record_empty_args("openai", "gpt-4", "grep").await;
        agg.record_empty_args("openai", "gpt-4", "grep").await;

        let metrics = agg.get_metrics("openai", "gpt-4").await.unwrap();
        assert_eq!(metrics.empty_args_count, 2);
        assert_eq!(metrics.tools_with_empty_args.get("grep"), Some(&2));
    }

    #[tokio::test]
    async fn test_record_invalid_json() {
        let agg = MetricsAggregator::new();
        agg.record_invalid_json("google", "gemini").await;
        agg.record_invalid_json("google", "gemini").await;

        let metrics = agg.get_metrics("google", "gemini").await.unwrap();
        assert_eq!(metrics.invalid_json_count, 2);
    }

    #[tokio::test]
    async fn test_multiple_providers() {
        let agg = MetricsAggregator::new();
        agg.record_empty_args("openai", "gpt-4", "grep").await;
        agg.record_empty_args("google", "gemini", "grep").await;

        let all = agg.get_all_metrics().await;
        assert_eq!(all.len(), 2);
    }
}

