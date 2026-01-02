use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RedactionLevel {
    Strict,  // Production - redact all secrets + tool args
    Normal,  // Development - redact secrets + large blobs
    Minimal, // Debug - minimal redaction (only known secret keys)
}

impl Default for RedactionLevel {
    fn default() -> Self {
        match std::env::var("REDACTION_LEVEL")
            .unwrap_or_else(|_| "normal".to_string())
            .to_lowercase()
            .as_str()
        {
            "strict" => RedactionLevel::Strict,
            "minimal" => RedactionLevel::Minimal,
            _ => RedactionLevel::Normal,
        }
    }
}

pub fn redact_value(v: &mut Value, level: RedactionLevel) {
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                let k_lower = k.to_lowercase();

                // 1. Secret keys (Always redacted)
                if k_lower.contains("key")
                    || k_lower.contains("auth")
                    || k_lower.contains("token")
                    || k_lower.contains("secret")
                    || k_lower.contains("password")
                    || k_lower == "authorization"
                    || k_lower == "cookie"
                {
                    *val = Value::String("[REDACTED]".to_string());
                    continue;
                }

                // 2. Level-based redaction
                match level {
                    RedactionLevel::Strict => {
                        // In strict mode, we redact tool arguments and potential user content in certain fields
                        if k_lower == "arguments" || k_lower == "content" {
                            *val = Value::String("[REDACTED-STRICT]".to_string());
                        } else {
                            redact_value(val, level);
                        }
                    }
                    RedactionLevel::Normal => {
                        // Redact large data blobs
                        if k_lower == "data"
                            && val.is_string()
                            && val.as_str().unwrap_or("").len() > 100
                        {
                            *val = Value::String("[REDACTED-DATA]".to_string());
                        } else if k_lower == "arguments"
                            && val.is_string()
                            && val.as_str().unwrap_or("").len() > 500
                        {
                            *val = Value::String("[REDACTED-LARGE-ARGS]".to_string());
                        } else {
                            redact_value(val, level);
                        }
                    }
                    RedactionLevel::Minimal => {
                        redact_value(val, level);
                    }
                }
            }
        }
        Value::Array(arr) => {
            for val in arr {
                redact_value(val, level);
            }
        }
        _ => {}
    }
}
