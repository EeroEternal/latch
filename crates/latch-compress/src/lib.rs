use latch_core::{CompressionResult, CompressionStrategy, Message};

pub fn sliding_window(messages: &[Message], max_turns: usize) -> Vec<Message> {
    if max_turns == 0 || messages.is_empty() {
        return Vec::new();
    }

    let mut system_messages = Vec::new();
    let mut conversation_messages = Vec::new();

    for msg in messages.iter().cloned() {
        if msg.role == "system" {
            system_messages.push(msg);
        } else {
            conversation_messages.push(msg);
        }
    }

    let keep_count = max_turns.saturating_mul(2);
    let start = conversation_messages.len().saturating_sub(keep_count);

    let mut out = system_messages;
    out.extend(conversation_messages.into_iter().skip(start));
    out
}

pub fn sliding_window_with_meta(messages: &[Message], max_turns: usize) -> CompressionResult {
    let messages_out = sliding_window(messages, max_turns);
    let tokens_before = estimate_tokens(messages);
    let tokens_after = estimate_tokens(&messages_out);

    CompressionResult {
        messages: messages_out,
        tokens_before,
        tokens_after,
        strategy_used: CompressionStrategy::SlidingWindow,
    }
}

fn estimate_tokens(messages: &[Message]) -> usize {
    // Lightweight fallback estimator; upstream can replace with model-specific tokenizers.
    let chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
    chars / 4
}

#[cfg(test)]
mod tests {
    use super::{sliding_window, sliding_window_with_meta};
    use latch_core::Message;

    fn msg(role: &str, content: &str) -> Message {
        Message::new(role, content)
    }

    #[test]
    fn keeps_system_and_last_turns() {
        let messages = vec![
            msg("system", "you are helpful"),
            msg("user", "u1"),
            msg("assistant", "a1"),
            msg("user", "u2"),
            msg("assistant", "a2"),
            msg("user", "u3"),
            msg("assistant", "a3"),
        ];

        let out = sliding_window(&messages, 2);
        let got: Vec<&str> = out.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(got, vec!["you are helpful", "u2", "a2", "u3", "a3"]);
    }

    #[test]
    fn zero_turns_returns_empty() {
        let messages = vec![msg("system", "s"), msg("user", "u1"), msg("assistant", "a1")];
        assert!(sliding_window(&messages, 0).is_empty());
    }

    #[test]
    fn returns_metadata() {
        let messages = vec![msg("system", "s"), msg("user", "hello"), msg("assistant", "world")];
        let out = sliding_window_with_meta(&messages, 1);
        assert_eq!(out.messages.len(), 3);
        assert!(out.tokens_before >= out.tokens_after);
    }
}
