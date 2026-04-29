use latch_core::{
    config::{PoolFeedback, PoolRoute, PoolTier},
    message::Message,
};
use super::analyzer::ContentProfile;

/// Calculate score for a pool
pub fn calculate_pool_score(
    pool: &PoolRoute,
    total_tokens: usize,
    messages: &[Message],
    profile: &ContentProfile,
    feedback: Option<&PoolFeedback>,
    long_request_tokens: usize,
) -> (f32, String) {
    let mut score = 0.0f32;
    let mut reasons: Vec<String> = Vec::new();

    // Base score by tier (before weight)
    // Note: weight is applied to the entire score, not just tier
    let tier_score = match pool.tier {
        PoolTier::Fast => 0.5,
        PoolTier::Standard => 0.6,
        PoolTier::Premium => 0.7,
        PoolTier::Backup => 0.1,
    };
    score += tier_score;
    reasons.push(format!("tier={:?}({})", pool.tier, tier_score));

    // Weight
    score *= pool.weight;
    reasons.push(format!("weight={}", pool.weight));

    // Keyword matching
    let matched_keywords: Vec<&String> = pool
        .match_keywords
        .iter()
        .filter(|kw| {
            // Check if keyword appears in any recent message
            messages
                .iter()
                .rev()
                .take(3)
                .any(|m| m.content.to_lowercase().contains(&kw.to_lowercase()))
        })
        .collect();

    let keyword_bonus = matched_keywords.len() as f32 * pool.keyword_score;
    if keyword_bonus > 0.0 {
        score += keyword_bonus;
        reasons.push(format!("keywords={} (matched: {})", keyword_bonus, matched_keywords.len()));
    }

    // Image compatibility
    if let Some(images) = pool.images {
        if images == profile.has_images {
            score += 0.1;
            reasons.push("image_match=0.1".to_string());
        }
    }

    // Long request handling
    if total_tokens > long_request_tokens {
        match pool.tier {
            PoolTier::Premium => {
                score += 0.2;
                reasons.push("long_request_premium=+0.2".to_string());
            }
            PoolTier::Fast => {
                score -= 0.2;
                reasons.push("long_request_fast=-0.2".to_string());
            }
            _ => {}
        }
    }

    // Content feature bonuses
    if profile.has_code && matches!(pool.tier, PoolTier::Premium | PoolTier::Standard) {
        score += 0.1;
        reasons.push("code=+0.1".to_string());
    }
    if profile.has_architecture && matches!(pool.tier, PoolTier::Premium) {
        score += 0.15;
        reasons.push("architecture=+0.15".to_string());
    }
    if profile.is_simple && matches!(pool.tier, PoolTier::Fast) {
        score += 0.2;
        reasons.push("simple=+0.2".to_string());
    }
    if profile.has_failure_retry && matches!(pool.tier, PoolTier::Premium) {
        score += 0.1;
        reasons.push("failure_retry=+0.1".to_string());
    }

    // Feedback penalty
    if let Some(fb) = feedback {
        if let Some(&failures) = fb.recent_failures.get(&pool.pool_id) {
            let penalty = fb.penalty_per_failure.powi(failures as i32);
            score *= penalty;
            reasons.push(format!("feedback_penalty={:.2}", penalty));
        }
    }

    // Ensure score is in valid range
    score = score.max(0.0).min(1.0);

    (score, reasons.join(", "))
}
