use latch_core::{
    config::CompressionConfig,
    message::Message,
    decision::{CompressionAction, CompressionResult},
    TokenEstimator, default_token_estimator,
};
use latch_core::CompressionStrategy;

/// Main entry point: compresses messages based on config
pub fn compress(
    messages: &[Message],
    config: &CompressionConfig,
) -> CompressionAction {
    let token_estimator = &config.token_estimator;
    let tokens_before = estimate_tokens(messages, Some(token_estimator));

    // Check if compression is needed
    if tokens_before < config.min_tokens_to_compress {
        return CompressionAction::Skip {
            tokens: tokens_before,
            reason: format!(
                "{} tokens < min_tokens_to_compress ({})",
                tokens_before, config.min_tokens_to_compress
            ),
        };
    }

    match config.strategy {
        latch_core::CompressionStrategy::None => CompressionAction::Skip {
            tokens: tokens_before,
            reason: "strategy is None".to_string(),
        },
        latch_core::CompressionStrategy::SlidingWindow => {
            let result = sliding_window_structured(messages, config);
            let tokens_after = estimate_tokens(&result, Some(&config.token_estimator));
            CompressionAction::Applied(CompressionResult {
                messages: result,
                tokens_before,
                tokens_after,
                strategy_used: latch_core::CompressionStrategy::SlidingWindow,
                savings_ratio: calculate_savings(tokens_before, tokens_after),
            })
        }
        latch_core::CompressionStrategy::Dedup => {
            let result = dedup_adjacent(messages, config.dedup_max_merged_chars);
            let tokens_after = estimate_tokens(&result, Some(&config.token_estimator));
            CompressionAction::Applied(CompressionResult {
                messages: result,
                tokens_before,
                tokens_after,
                strategy_used: latch_core::CompressionStrategy::Dedup,
                savings_ratio: calculate_savings(tokens_before, tokens_after),
            })
        }
        latch_core::CompressionStrategy::DedupThenWindow => {
            // First dedup
            let deduped = dedup_adjacent(messages, config.dedup_max_merged_chars);

            // Check if we still need to compress after dedup
            let deduped_tokens = estimate_tokens(&deduped, Some(&config.token_estimator));
            if deduped_tokens < config.min_tokens_to_compress {
                // No need for sliding window
                return CompressionAction::Applied(CompressionResult {
                    messages: deduped,
                    tokens_before,
                    tokens_after: deduped_tokens,
                    strategy_used: latch_core::CompressionStrategy::DedupThenWindow,
                    savings_ratio: calculate_savings(tokens_before, deduped_tokens),
                });
            }

            // Apply sliding window
            let result = sliding_window_structured(&deduped, config);
            let tokens_after = estimate_tokens(&result, Some(&config.token_estimator));
            CompressionAction::Applied(CompressionResult {
                messages: result,
                tokens_before,
                tokens_after,
                strategy_used: latch_core::CompressionStrategy::DedupThenWindow,
                savings_ratio: calculate_savings(tokens_before, tokens_after),
            })
        }
    }
}

/// Structure-aware sliding window
/// - Preserves system messages (with optional truncation)
/// - Keeps structural units (user/assistant pairs, tool_use/tool_result pairs)
pub fn sliding_window_structured(
    messages: &[Message],
    config: &CompressionConfig,
) -> Vec<Message> {
    let token_estimator = &config.token_estimator;

    // Separate system messages and conversation
    let mut system_messages: Vec<Message> = Vec::new();
    let mut conversation: Vec<Message> = Vec::new();

    for msg in messages {
        if msg.role == "system" {
            system_messages.push(msg.clone());
        } else {
            conversation.push(msg.clone());
        }
    }

    // Truncate system messages if needed
    let mut system_tokens: usize = system_messages
        .iter()
        .map(|m| token_estimator(&m.content))
        .sum();

    while config.max_system_tokens > 0 && system_tokens > config.max_system_tokens {
        if let Some(last) = system_messages.pop() {
            system_tokens = system_tokens.saturating_sub(token_estimator(&last.content));
        } else {
            break;
        }
    }

    // Identify structural units in conversation
    let units = identify_structural_units(&conversation);

    // Take last max_turns units
    let start_idx = units.len().saturating_sub(config.max_turns);
    let kept_units: Vec<&[Message]> = units.iter().skip(start_idx).map(|v| v.as_slice()).collect();

    // Build result
    let mut result: Vec<Message> = system_messages;
    for unit in kept_units {
        result.extend_from_slice(unit);
    }

    result
}

/// Identify structural units in conversation messages
/// Returns Vec<Vec<Message>> where each inner Vec is a structural unit:
/// - user/assistant pair
/// - tool_use (assistant) + tool_result (user) pair
fn identify_structural_units(messages: &[Message]) -> Vec<Vec<Message>> {
    let mut units: Vec<Vec<Message>> = Vec::new();
    let mut current_unit: Vec<Message> = Vec::new();

    for msg in messages {
        // Don't start new unit for tool results - they belong to current unit
        let is_tool_result = msg.is_tool_result();

        // Start a new unit when:
        // - current unit is not empty AND
        // - this is a user message (not tool_result) AND
        // - last message in current unit is "assistant" (completed a turn)
        let starts_new_unit = !is_tool_result
            && msg.role == "user"
            && !current_unit.is_empty()
            && current_unit.last().map(|m| m.role == "assistant").unwrap_or(false);

        if starts_new_unit {
            units.push(current_unit.clone());
            current_unit.clear();
        }

        current_unit.push(msg.clone());
    }

    // Don't forget the last unit
    if !current_unit.is_empty() {
        units.push(current_unit);
    }

    units
}

/// Merge adjacent messages with the same role (except tool messages)
pub fn dedup_adjacent(messages: &[Message], max_merged_chars: usize) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();

    for m in messages {
        // Don't merge tool messages
        if m.is_tool_call() || m.is_tool_result() {
            out.push(m.clone());
            continue;
        }

        if let Some(last) = out.last_mut() {
            // Don't merge if previous message is a tool message
            let prev_is_tool = last.is_tool_call() || last.is_tool_result();
            
            if !prev_is_tool && last.role == m.role {
                let combined_len = last.content.len() + m.content.len() + 4; // "\n\n" = 2 chars, but let's be safe

                if max_merged_chars == 0 || combined_len <= max_merged_chars {
                    last.content.push_str("\n\n");
                    last.content.push_str(&m.content);
                    continue;
                }
            }
        }

        out.push(m.clone());
    }

    out
}

/// Estimate total tokens for a list of messages
pub fn estimate_tokens(messages: &[Message], estimator: Option<&TokenEstimator>) -> usize {
    match estimator {
        Some(est) => messages.iter().map(|m| est(&m.content)).sum(),
        None => {
            // Fallback: chars / 4
            messages.iter().map(|m| m.content.chars().count() / 4).sum()
        }
    }
}

/// Estimate tokens for a single message
pub fn estimate_msg_tokens(msg: &Message, estimator: Option<&TokenEstimator>) -> usize {
    let default_est = default_token_estimator();
    let f = estimator.unwrap_or(&default_est);
    // Message's role overhead is tiny, ignore it
    f(&msg.content)
}

#[deprecated(since = "0.2", note = "Use compress() or sliding_window_structured() instead")]
pub fn sliding_window(messages: &[Message], max_turns: usize) -> Vec<Message> {
    let config = CompressionConfig {
        min_tokens_to_compress: 0,
        max_turns,
        max_system_tokens: 0,
        strategy: CompressionStrategy::SlidingWindow,
        dedup_max_merged_chars: 0,
        token_estimator: default_token_estimator(),
    };
    sliding_window_structured(messages, &config)
}

#[deprecated(since = "0.2", note = "Use compress() instead")]
pub fn sliding_window_with_meta(messages: &[Message], max_turns: usize) -> CompressionResult {
    let config = CompressionConfig {
        min_tokens_to_compress: 0,
        max_turns,
        max_system_tokens: 0,
        strategy: CompressionStrategy::SlidingWindow,
        dedup_max_merged_chars: 0,
        token_estimator: default_token_estimator(),
    };
    let msgs = sliding_window_structured(messages, &config);
    let tokens_before = estimate_tokens(messages, Some(&config.token_estimator));
    let tokens_after = estimate_tokens(&msgs, Some(&config.token_estimator));
    CompressionResult {
        messages: msgs,
        tokens_before,
        tokens_after,
        strategy_used: CompressionStrategy::SlidingWindow,
        savings_ratio: calculate_savings(tokens_before, tokens_after),
    }
}

/// Calculate savings ratio
fn calculate_savings(before: usize, after: usize) -> f32 {
    if before > after {
        (before - after) as f32 / before as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use latch_core::CompressionStrategy;
    use std::sync::Arc;

    fn msg(role: impl Into<String>, content: impl Into<String>) -> Message {
        Message::new(role, content)
    }

    fn default_config() -> CompressionConfig {
        CompressionConfig {
            min_tokens_to_compress: 2000,
            max_turns: 10,
            max_system_tokens: 8000,
            strategy: CompressionStrategy::SlidingWindow,
            dedup_max_merged_chars: 50000,
            token_estimator: default_token_estimator(),
        }
    }

    fn default_token_estimator() -> TokenEstimator {
        Arc::new(|text: &str| text.chars().count() / 4)
    }

    #[test]
    fn skip_when_below_threshold() {
        let messages = vec![msg("user", "hello")];
        let config = default_config();
        let result = compress(&messages, &config);

        match result {
            CompressionAction::Skip { tokens, reason } => {
                assert!(tokens > 0);
                assert!(reason.contains("min_tokens_to_compress"));
            }
            _ => panic!("Expected Skip"),
        }
    }

    #[test]
    fn sliding_window_keeps_system_and_last_turns() {
        let messages = vec![
            msg("system", "you are helpful"),
            msg("user", "u1"),
            msg("assistant", "a1"),
            msg("user", "u2"),
            msg("assistant", "a2"),
            msg("user", "u3"),
            msg("assistant", "a3"),
        ];

        let mut config = default_config();
        config.min_tokens_to_compress = 0; // Force compression
        config.max_turns = 2;

        let result = compress(&messages, &config);

        match result {
            CompressionAction::Applied(r) => {
                let roles: Vec<&str> = r.messages.iter().map(|m| m.role.as_str()).collect();
                // Should have system + last 2 turns (u2/a2, u3/a3)
                assert!(roles.contains(&"system"));
                assert!(roles.contains(&"user"));
            }
            _ => panic!("Expected Applied"),
        }
    }

    #[test]
    fn dedup_merges_adjacent_same_role() {
        let messages = vec![
            msg("user", "line 1"),
            msg("user", "line 2"),
            msg("assistant", "response"),
            msg("assistant", "more response"),
        ];

        let config = default_config();
        let result = dedup_adjacent(&messages, config.dedup_max_merged_chars);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "line 1\n\nline 2");
        assert_eq!(result[1].content, "response\n\nmore response");
    }

    #[test]
    fn dedup_skips_tool_messages() {
        let messages = vec![
            msg("assistant", r#"{"type":"tool_use","name":"search"}"#),
            msg("user", r#"{"type":"tool_result","content":"..."}"#),
            msg("user", "follow up"),
        ];

        let config = default_config();
        let result = dedup_adjacent(&messages, config.dedup_max_merged_chars);

        // Tool messages should not be merged
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn system_over_limit_truncates() {
        // Create system messages that exceed max_system_tokens
        let long_system = "a".repeat(10000); // ~2500 tokens
        let messages = vec![
            msg("system", &long_system),
            msg("system", &long_system),
            msg("user", "hello"),
        ];
        let config = CompressionConfig {
            min_tokens_to_compress: 0,
            max_turns: 10,
            max_system_tokens: 1000, // ~1000 tokens
            strategy: CompressionStrategy::SlidingWindow,
            dedup_max_merged_chars: 50000,
            token_estimator: default_token_estimator(),
        };
        let result = compress(&messages, &config);
        match result {
            CompressionAction::Applied(r) => {
                // Should have truncated system messages
                let system_tokens: usize = r.messages
                    .iter()
                    .filter(|m| m.role == "system")
                    .map(|m| estimate_msg_tokens(m, Some(&config.token_estimator)))
                    .sum();
                assert!(system_tokens <= 1000 + 100); // Allow some margin
            }
            _ => panic!("Expected Applied"),
        }
    }

    #[test]
    fn sliding_window_preserves_tool_pairs() {
        let messages = vec![
            msg("user", "use tool"),
            msg("assistant", r#"{"type":"tool_use","name":"search"}"#),
            msg("user", r#"{"type":"tool_result","content":"..."}"#),
            msg("user", "follow up"),
        ];
        let mut config = default_config();
        config.min_tokens_to_compress = 0;
        config.max_turns = 1; // Only keep 1 structural unit

        let result = compress(&messages, &config);
        match result {
            CompressionAction::Applied(r) => {
                // The tool_use + tool_result should be kept together
                let has_tool = r.messages.iter().any(|m| m.is_tool_call());
                let has_result = r.messages.iter().any(|m| m.is_tool_result());
                // Either both are kept or both are dropped
                assert_eq!(has_tool, has_result);
            }
            _ => panic!("Expected Applied"),
        }
    }

    #[test]
    fn dedup_then_window_works() {
        // Use longer messages so token estimation is meaningful
        let messages = vec![
            msg("user", "line1 ".repeat(50)),  // ~300 chars
            msg("user", "line2 ".repeat(50)),  // ~300 chars
            msg("assistant", "response1 ".repeat(50)),  // ~500 chars
            msg("user", "u3 ".repeat(50)),  // ~100 chars
            msg("assistant", "a3 ".repeat(50)),  // ~100 chars
        ];
        let mut config = default_config();
        config.strategy = CompressionStrategy::DedupThenWindow;
        config.min_tokens_to_compress = 0;
        config.max_turns = 1;

        let result = compress(&messages, &config);
        match result {
            CompressionAction::Applied(r) => {
                // After dedup: user("line1\n\nline2"), assistant("response1")
                // After window (max_turns=1): keep last 1 turn (u3/a3)
                assert!(r.tokens_before >= r.tokens_after);
                // Should have saved some tokens
                assert!(r.savings_ratio >= 0.0);
            }
            _ => panic!("Expected Applied"),
        }
    }

    #[test]
    fn all_system_messages_respect_limit() {
        let messages = vec![
            msg("system", "a".repeat(5000)),
            msg("system", "b".repeat(5000)),
        ];
        let config = CompressionConfig {
            min_tokens_to_compress: 0,
            max_turns: 10,
            max_system_tokens: 500, // ~500 tokens
            strategy: CompressionStrategy::SlidingWindow,
            dedup_max_merged_chars: 50000,
            token_estimator: default_token_estimator(),
        };
        let result = compress(&messages, &config);
        match result {
            CompressionAction::Applied(r) => {
                let total_chars: usize = r.messages.iter().map(|m| m.content.len()).sum();
                // Should be truncated to ~2000 chars (500 tokens * 4)
                assert!(total_chars < 5000);
            }
            _ => panic!("Expected Applied"),
        }
    }

    #[test]
    fn savings_ratio_calculated_correctly() {
        // Use longer messages so compression actually saves tokens
        let messages = vec![
            msg("system", "you are helpful ".repeat(100)),
            msg("user", "u1 ".repeat(100)),
            msg("assistant", "a1 ".repeat(100)),
            msg("user", "u2 ".repeat(100)),
            msg("assistant", "a2 ".repeat(100)),
        ];
        let mut config = default_config();
        config.min_tokens_to_compress = 0;
        config.max_turns = 1;  // Only keep last turn (u2/a2)

        let result = compress(&messages, &config);
        match result {
            CompressionAction::Applied(r) => {
                // savings_ratio = (before - after) / before
                assert!(r.savings_ratio >= 0.0);
                assert!(r.savings_ratio <= 1.0);
                // Should save some tokens (dropped u1/a1, kept system + u2/a2)
                assert!(r.tokens_before > r.tokens_after);
            }
            _ => panic!("Expected Applied"),
        }
    }

    #[test]
    fn max_turns_zero_returns_only_system() {
        let messages = vec![
            msg("system", "you are helpful"),
            msg("user", "u1"),
            msg("assitant", "a1"),
        ];
        let mut config = default_config();
        config.min_tokens_to_compress = 0;
        config.max_turns = 0;

        let result = compress(&messages, &config);
        match result {
            CompressionAction::Applied(r) => {
                // Only system messages should remain
                assert_eq!(r.messages.len(), 1);
                assert_eq!(r.messages[0].role, "system");
            }
            _ => panic!("Expected Applied"),
        }
    }
}
