use crate::types::CostModel;

pub async fn fetch_pricing(
    client: &reqwest::Client,
) -> std::collections::HashMap<String, CostModel> {
    let mut attempts = 0;
    let max_attempts = 3;

    loop {
        attempts += 1;
        match client
            .get("https://openrouter.ai/api/v1/models")
            .send()
            .await
        {
            Ok(resp) => {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    let pricing = parse_pricing_json(&json);
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
                    return std::collections::HashMap::new();
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

fn parse_pricing_json(json: &serde_json::Value) -> std::collections::HashMap<String, CostModel> {
    let mut pricing = std::collections::HashMap::new();
    if let Some(models) = json.get("data").and_then(|d| d.as_array()) {
        for m in models {
            if let (Some(id), Some(p)) = (m.get("id").and_then(|v| v.as_str()), m.get("pricing")) {
                let context_length = m
                    .get("context_length")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                let mut model = parse_single_model_pricing(id, p);
                model.context_length = context_length;
                pricing.insert(id.to_string(), model);
            }
        }
    }
    pricing
}

fn parse_single_model_pricing(_id: &str, p: &serde_json::Value) -> CostModel {
    let parse_f64 = |val: &serde_json::Value| {
        if let Some(f) = val.as_f64() {
            return f;
        }
        if let Some(s) = val.as_str() {
            if let Ok(f) = s.parse::<f64>() {
                return f;
            }
        }
        0.0
    };

    let prompt = match p.get("prompt") {
        Some(v) => parse_f64(v),
        None => 0.0,
    };
    let completion = match p.get("completion") {
        Some(v) => parse_f64(v),
        None => 0.0,
    };
    let image = match p.get("image") {
        Some(v) => parse_f64(v),
        None => 0.0,
    };
    let request = match p.get("request") {
        Some(v) => parse_f64(v),
        None => 0.0,
    };
    let prompt_cache_read = match p.get("input_cache_read") {
        Some(v) => parse_f64(v),
        None => 0.0,
    };
    let prompt_cache_write = match p.get("input_cache_write") {
        Some(v) => parse_f64(v),
        None => 0.0,
    };

    CostModel {
        prompt,
        completion,
        image,
        request,
        prompt_cache_read,
        prompt_cache_write,
        context_length: None,
    }
}
