use latch_core::{Message, RouterConfig, RoutingDecision};

pub fn route_model(messages: &[Message], config: &RouterConfig) -> RoutingDecision {
    let total_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
    let estimated_tokens = total_chars / 4;

    let last_user = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.to_lowercase())
        .unwrap_or_default();

    let mentions_code = contains_any(&last_user, &["code", "rust", "python", "debug", "stack trace"]);
    let mentions_analysis = contains_any(
        &last_user,
        &["design", "architecture", "reason", "compare", "tradeoff"],
    );

    let (provider, reason, confidence) = if estimated_tokens < 4_000 && !mentions_code && !mentions_analysis {
        (
            config.fast_pool.clone(),
            "short non-code request -> fast pool".to_string(),
            0.84,
        )
    } else {
        (
            config.strong_pool.clone(),
            "complex or long request -> strong pool".to_string(),
            0.78,
        )
    };

    RoutingDecision {
        provider,
        reason,
        confidence,
    }
}

fn contains_any(text: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|k| text.contains(k))
}

#[cfg(test)]
mod tests {
    use super::route_model;
    use latch_core::{Message, RouterConfig};

    fn msg(role: &str, content: &str) -> Message {
        Message::new(role, content)
    }

    fn cfg() -> RouterConfig {
        RouterConfig {
            fast_pool: "fast".to_string(),
            strong_pool: "strong".to_string(),
            confidence_threshold: 0.7,
        }
    }

    #[test]
    fn routes_short_general_requests_to_fast_pool() {
        let messages = vec![msg("user", "Summarize this meeting in three bullets.")];
        let decision = route_model(&messages, &cfg());
        assert_eq!(decision.provider, "fast");
        assert!(decision.reason.contains("fast pool"));
    }

    #[test]
    fn routes_code_requests_to_strong_pool() {
        let messages = vec![msg("user", "Please debug this Rust code and explain the stack trace.")];
        let decision = route_model(&messages, &cfg());
        assert_eq!(decision.provider, "strong");
        assert!(decision.reason.contains("strong pool"));
    }

    #[test]
    fn routes_long_requests_to_strong_pool() {
        let long = "x".repeat(20_000);
        let messages = vec![msg("user", &long)];
        let decision = route_model(&messages, &cfg());
        assert_eq!(decision.provider, "strong");
    }
}
