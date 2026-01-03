use crate::types::CostModel;

pub async fn fetch_pricing(
    client: &reqwest::Client,
) -> std::collections::HashMap<String, CostModel> {
    let mut attempts = 0;
    let max_attempts = 3;

    loop {
        attempts += 1;
        let mut pricing = std::collections::HashMap::new();
        match client
            .get("https://openrouter.ai/api/v1/models")
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(json) = resp.json::<serde_json::Value>().await
                    && let Some(models) = json.get("data").and_then(|d| d.as_array()) {
                        for m in models {
                            if let (Some(id), Some(p)) =
                                (m.get("id").and_then(|v| v.as_str()), m.get("pricing"))
                            {
                                let parse_f64 = |val: &serde_json::Value| {
                                    val.as_f64().or_else(|| {
                                        val.as_str().and_then(|s| s.parse::<f64>().ok())
                                    }).unwrap_or(0.0)
                                };

                                let prompt = p.get("prompt").map(parse_f64).unwrap_or(0.0);
                                let completion = p.get("completion").map(parse_f64).unwrap_or(0.0);
                                let image = p.get("image").map(parse_f64).unwrap_or(0.0);
                                let request = p.get("request").map(parse_f64).unwrap_or(0.0);
                                let prompt_cache_read = p.get("input_cache_read").map(parse_f64).unwrap_or(0.0);
                                let prompt_cache_write = p.get("input_cache_write").map(parse_f64).unwrap_or(0.0);

                                pricing.insert(
                                    id.to_string(),
                                    CostModel {
                                        prompt,
                                        completion,
                                        image,
                                        request,
                                        prompt_cache_read,
                                        prompt_cache_write,
                                    },
                                );
                            }
                        }
                        if !pricing.is_empty() {
                            return pricing;
                        }
                    }
                }
            Err(e) => {
                if attempts >= max_attempts {
                    tracing::error!(
                        "Failed to fetch pricing after {} attempts: {}",
                        max_attempts,
                        e
                    );
                    return pricing;
                }
                tracing::warn!(
                    "Failed to fetch pricing (attempt {}/{}): {}. Retrying in 2s...",
                    attempts,
                    max_attempts,
                    e
                );
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}
