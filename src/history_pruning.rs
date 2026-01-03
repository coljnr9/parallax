//! Conversation History Pruning Module
//!
//! Implements strategies to reduce conversation history depth and size to avoid
//! hitting provider limits (especially Google's recursion depth limits).

use crate::str_utils;
use crate::types::{MessagePart, Role, TurnRecord};

/// Analyzes the depth of a conversation history
#[derive(Debug, Clone)]
pub struct HistoryDepthAnalysis {
    pub max_nesting_depth: usize,
    pub total_turns: usize,
    pub tool_result_turns: usize,
    pub estimated_json_depth: usize,
}

impl HistoryDepthAnalysis {
    /// Analyze conversation history for depth issues
    pub fn analyze(history: &[TurnRecord]) -> Self {
        let mut tool_result_turns = 0;

        for turn in history {
            for part in &turn.content {
                if let MessagePart::ToolResult { .. } = part {
                    tool_result_turns += 1;
                }
            }
        }

        // Note: Google's "max recursion depth" error is NOT about message array length.
        // The JSON structure is typically: {"messages": [msg1, msg2, ...]}
        // This is only 2-3 levels deep regardless of message count.
        // The actual limit is about total conversation length (~100 turns) and
        // deeply nested content within tool results, not the message array itself.
        Self {
            max_nesting_depth: 0,
            total_turns: history.len(),
            tool_result_turns,
            estimated_json_depth: 0,
        }
    }

    /// Check if history exceeds Google's limits
    pub fn exceeds_google_limits(&self) -> bool {
        // Google's practical limit is around 100 turns before context issues.
        // The "max recursion depth" error was about deeply nested tool results,
        // not the message array length. Being conservative with 100 turn limit.
        self.total_turns > 100
    }

    /// Check if history is approaching limits
    pub fn approaching_google_limits(&self) -> bool {
        // Warn when approaching 80% of the limit
        self.total_turns > 80
    }
}

/// Pruning strategy enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruningStrategy {
    /// Keep first N and last M messages, drop middle
    Windowing,
    /// Summarize old tool results into text
    Summarization,
    /// Flatten nested tool results
    Flattening,
    /// Remove least important turns
    SelectiveDeletion,
}

/// Prunes conversation history using the specified strategy
pub fn prune_history(
    history: Vec<TurnRecord>,
    strategy: PruningStrategy,
    target_turns: usize,
) -> Vec<TurnRecord> {
    match strategy {
        PruningStrategy::Windowing => prune_windowing(history, target_turns),
        PruningStrategy::Summarization => prune_summarization(history, target_turns),
        PruningStrategy::Flattening => prune_flattening(history, target_turns),
        PruningStrategy::SelectiveDeletion => prune_selective_deletion(history, target_turns),
    }
}

/// Windowing strategy: keep first N and last M messages
fn prune_windowing(history: Vec<TurnRecord>, target_turns: usize) -> Vec<TurnRecord> {
    if history.len() <= target_turns {
        return history;
    }

    let keep_first = target_turns / 3;
    let keep_last = target_turns - keep_first;

    let mut result = Vec::new();

    // Keep first messages
    result.extend(history.iter().take(keep_first).cloned());

    // Add separator message
    result.push(TurnRecord {
        role: Role::System,
        content: vec![MessagePart::Text {
            content: "[... conversation history pruned ...]".to_string(),
            cache_control: None,
        }],
        tool_call_id: None,
    });

    // Keep last messages
    let skip_count = history.len().saturating_sub(keep_last);
    result.extend(history.iter().skip(skip_count).cloned());

    result
}

/// Summarization strategy: collapse old tool results into text
fn prune_summarization(history: Vec<TurnRecord>, target_turns: usize) -> Vec<TurnRecord> {
    if history.len() <= target_turns {
        return history;
    }

    let mut result = Vec::new();
    let cutoff = history.len().saturating_sub(target_turns);

    for (i, turn) in history.into_iter().enumerate() {
        if i < cutoff {
            // Summarize old turns
            if let Some(summary) = summarize_turn(&turn) {
                result.push(summary);
            }
        } else {
            // Keep recent turns as-is
            result.push(turn);
        }
    }

    result
}

/// Flattening strategy: extract nested content from tool results
fn prune_flattening(history: Vec<TurnRecord>, target_turns: usize) -> Vec<TurnRecord> {
    if history.len() <= target_turns {
        return history;
    }

    let mut result = Vec::new();

    for turn in history {
        let mut flattened_content = Vec::new();

        for part in turn.content {
            match part {
                MessagePart::ToolResult { content, .. } => {
                    // Extract text from tool result, avoiding deep nesting
                    if let Some(text) = extract_text_from_tool_result(&content) {
                        flattened_content.push(MessagePart::Text {
                            content: text,
                            cache_control: None,
                        });
                    }
                }
                other => flattened_content.push(other),
            }
        }

        result.push(TurnRecord {
            role: turn.role,
            content: flattened_content,
            tool_call_id: turn.tool_call_id,
        });
    }

    // If still too long, apply windowing
    if result.len() > target_turns {
        prune_windowing(result, target_turns)
    } else {
        result
    }
}

/// Selective deletion strategy: remove least important turns
fn prune_selective_deletion(history: Vec<TurnRecord>, target_turns: usize) -> Vec<TurnRecord> {
    if history.len() <= target_turns {
        return history;
    }

    let mut scored_turns: Vec<(usize, TurnRecord, u32)> = history
        .into_iter()
        .enumerate()
        .map(|(i, turn)| (i, turn.clone(), score_turn_importance(&turn)))
        .collect();

    // Sort by importance (ascending), keeping high-importance turns
    scored_turns.sort_by_key(|(_i, _turn, score)| *score);

    // Keep the most important turns
    let mut kept: Vec<(usize, TurnRecord)> = scored_turns
        .into_iter()
        .rev()
        .take(target_turns)
        .map(|(i, turn, _)| (i, turn))
        .collect();

    // Sort back to original order
    kept.sort_by_key(|(i, _)| *i);

    kept.into_iter().map(|(_, turn)| turn).collect()
}

/// Summarize a turn by extracting key information
fn summarize_turn(turn: &TurnRecord) -> Option<TurnRecord> {
    let mut summary_parts = Vec::new();

    for part in &turn.content {
        match part {
            MessagePart::Text { content, .. } => {
                // Keep text but truncate if very long
                let truncated = str_utils::prefix_chars(content, 200).to_string();
                let truncated = if content.len() > 200 {
                    format!("{}...", truncated)
                } else {
                    truncated
                };
                summary_parts.push(MessagePart::Text {
                    content: truncated,
                    cache_control: None,
                });
            }
            MessagePart::ToolCall { name, .. } => {
                // Replace tool calls with summary
                summary_parts.push(MessagePart::Text {
                    content: format!("[Tool call: {}]", name),
                    cache_control: None,
                });
            }
            MessagePart::ToolResult { name, .. } => {
                // Replace tool results with summary
                summary_parts.push(MessagePart::Text {
                    content: format!(
                        "[Tool result: {}]",
                        match name.as_ref() {
                            Some(n) => n,
                            None => "unknown",
                        }
                    ),
                    cache_control: None,
                });
            }
            other => summary_parts.push(other.clone()),
        }
    }

    if summary_parts.is_empty() {
        None
    } else {
        Some(TurnRecord {
            role: turn.role.clone(),
            content: summary_parts,
            tool_call_id: turn.tool_call_id.clone(),
        })
    }
}

/// Extract text content from a tool result
fn extract_text_from_tool_result(content: &str) -> Option<String> {
    if content.is_empty() {
        None
    } else {
        Some(content.to_string())
    }
}

/// Score turn importance (higher = more important)
fn score_turn_importance(turn: &TurnRecord) -> u32 {
    let mut score = 0u32;

    // System messages are important
    if turn.role == Role::System {
        score += 100;
    }

    // User messages are important
    if turn.role == Role::User {
        score += 80;
    }

    // Tool calls are moderately important
    for part in &turn.content {
        if matches!(part, MessagePart::ToolCall { .. }) {
            score += 50;
        }
    }

    // Longer content is slightly more important
    let content_len: usize = turn
        .content
        .iter()
        .map(|p| match p {
            MessagePart::Text { content, .. } => content.len(),
            MessagePart::ToolCall { name, .. } => name.len(),
            _ => 0,
        })
        .sum();

    score += (content_len / 100).min(20) as u32;

    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_turn(role: Role, text: &str) -> TurnRecord {
        TurnRecord {
            role,
            content: vec![MessagePart::Text {
                content: text.to_string(),
                cache_control: None,
            }],
            tool_call_id: None,
        }
    }

    #[test]
    fn test_history_depth_analysis() {
        let history = vec![
            create_test_turn(Role::User, "Hello"),
            create_test_turn(Role::Assistant, "Hi"),
        ];

        let analysis = HistoryDepthAnalysis::analyze(&history);
        assert_eq!(analysis.total_turns, 2);
        assert!(!analysis.exceeds_google_limits());
    }

    #[test]
    fn test_prune_windowing() {
        let history: Vec<_> = (0..10)
            .map(|i| create_test_turn(Role::User, &format!("Message {}", i)))
            .collect();

        let pruned = prune_windowing(history, 4);
        assert!(pruned.len() <= 6); // 4 kept + 1 separator + buffer
    }

    #[test]
    fn test_prune_selective_deletion() {
        let history = vec![
            create_test_turn(Role::System, "System prompt"),
            create_test_turn(Role::User, "User message"),
            create_test_turn(Role::Assistant, "Assistant response"),
            create_test_turn(Role::User, "Another user message"),
        ];

        let pruned = prune_selective_deletion(history, 2);
        assert_eq!(pruned.len(), 2);
        // System and User messages should be kept
        assert!(pruned.iter().any(|t| t.role == Role::System));
    }
}
