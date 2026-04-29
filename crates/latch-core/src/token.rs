use std::sync::Arc;

/// Token estimator function type.
/// Callers can inject precise implementations (e.g., using tiktoken) if available.
pub type TokenEstimator = Arc<dyn Fn(&str) -> usize + Send + Sync>;

/// Wrapper for TokenEstimator that implements Debug
#[derive(Clone)]
pub struct TokenEstimatorWrapper(pub TokenEstimator);

impl std::fmt::Debug for TokenEstimatorWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TokenEstimatorWrapper")
    }
}

/// Default lightweight token estimator.
/// Uses heuristic based on character types:
/// - English characters: ~4 chars/token
/// - CJK characters (inc. Extension A): ~1.5 chars/token
/// - Other characters: ~4 chars/token
/// Returns 0 for empty string.
pub fn default_token_estimator() -> TokenEstimator {
    Arc::new(|text: &str| {
        let total: usize = text.chars().count();
        if total == 0 {
            return 0;
        }
        let english = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
        let cjk = text
            .chars()
            .filter(|c| matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}'))
            .count();
        let other = total - english - cjk;
(english / 4).max(1) + (cjk * 2 / 3).max(1) + (other / 4).max(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_estimator_handles_empty_string() {
        let est = default_token_estimator();
        assert_eq!((est)(""), 0); // Spec: empty string returns 0
    }

    #[test]
    fn default_estimator_english_text() {
        let est = default_token_estimator();
        let text = "This is a test of English text";
        let tokens = (est)(text);
        // ~31 chars / 4 = ~7-8 tokens
        assert!(tokens > 0);
        assert!(tokens < 15);
    }

    #[test]
    fn default_estimator_cjk_text() {
        let est = default_token_estimator();
        let text = "这是一个测试"; // 6 CJK chars
        let tokens = (est)(text);
        // 6 * 2 / 3 = 4 tokens
        assert!(tokens > 0);
    }
}
