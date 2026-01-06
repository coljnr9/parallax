//! Token Counting Utility
//!
//! Provides heuristic and potentially BPE-based token counting for messages and tools.

use crate::types::{MessagePart, TurnRecord};

/// Estimator for token counts to avoid expensive BPE for simple pruning decisions.
pub struct TokenEstimator;

impl TokenEstimator {
    /// Estimate token count for a list of turns.
    pub fn estimate_total_tokens(history: &[TurnRecord]) -> usize {
        history.iter().map(Self::estimate_turn_tokens).sum()
    }

    /// Estimate token count for a single turn.
    pub fn estimate_turn_tokens(turn: &TurnRecord) -> usize {
        let mut tokens = 4; // Turn overhead

        for part in &turn.content {
            tokens += match part {
                MessagePart::Text { content, .. } => Self::estimate_text_tokens(content),
                MessagePart::Thought { content } => Self::estimate_text_tokens(content) + 4,
                MessagePart::ToolCall {
                    name, arguments, ..
                } => {
                    let arg_str = arguments.to_string();
                    Self::estimate_text_tokens(name) + Self::estimate_text_tokens(&arg_str) + 10
                }
                MessagePart::ToolResult { content, .. } => Self::estimate_text_tokens(content) + 4,
                MessagePart::Image { .. } => 1000, // Heuristic for images
            }
        }

        tokens
    }

    /// Lightweight heuristic: ~4 chars per token for English text.
    pub fn estimate_text_tokens(text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        // Conservative heuristic: 3 chars per token for code/complex text,
        // 4 for normal text. We'll use 3 to be safe.
        (text.len() / 3).max(1)
    }
}

#[cfg(test)]
mod tests {
    use crate::types::Role;
    use super::*;
    use crate::types::{MessagePart, TurnRecord};
    use serde_json::json;

    #[test]
    fn test_token_estimation() {
        let history = vec![
            TurnRecord {
                role: Role::System,
                content: vec![MessagePart::Text {
                    content: "You are a helpful assistant.".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
            TurnRecord {
                role: Role::User,
                content: vec![MessagePart::Text {
                    content: "Hello, how are you?".into(),
                    cache_control: None,
                }],
                tool_call_id: None,
            },
        ];

        let est = TokenEstimator::estimate_total_tokens(&history);
        assert!(est > 0);
        assert!(est < 100);
    }

    #[test]
    fn test_tool_token_estimation() {
        let turn = TurnRecord {
            role: Role::Assistant,
            content: vec![MessagePart::ToolCall {
                id: "call_123".into(),
                name: "read_file".into(),
                arguments: json!({"path": "src/main.rs"}),
                signature: None,
                metadata: json!({}),
                cache_control: None,
            }],
            tool_call_id: None,
        };

        let est = TokenEstimator::estimate_turn_tokens(&turn);
        assert!(est > 10);
    }
}
