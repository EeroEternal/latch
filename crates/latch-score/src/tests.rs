use super::*;
use latch_core::score::{LatencyBreakdown, StreamMetrics, TokenStats};
use std::time::{Duration, SystemTime};

struct FakeClock {
    now: std::sync::Arc<std::sync::Mutex<SystemTime>>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            now: std::sync::Arc::new(std::sync::Mutex::new(SystemTime::now())),
        }
    }

    fn advance(&self, dur: Duration) {
        let mut now = self.now.lock().unwrap();
        *now += dur;
    }

    fn clone_ref(&self) -> Self {
        Self {
            now: self.now.clone(),
        }
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        *self.now.lock().unwrap()
    }
}

fn default_config() -> ScoreConfig {
    ScoreConfig {
        window_size: 100,
        decay_period_secs: 300,
        baseline_score: 60.0,
        availability_weight: 0.35,
        latency_weight: 0.25,
        quality_weight: 0.25,
        cost_weight: 0.15,
        good_ttft_ms: 500,
        acceptable_ttft_ms: 2000,
        good_tps: 50.0,
    }
}

fn make_obs(endpoint_id: &str, pool_id: &str, success: bool) -> RequestObservation {
    RequestObservation {
        endpoint_id: endpoint_id.to_string(),
        pool_id: pool_id.to_string(),
        started_at: SystemTime::now(),
        success,
        error: if success {
            None
        } else {
            Some(ObservationError::Timeout)
        },
        was_retry: false,
        latency: LatencyBreakdown {
            total_ms: 300,
            ttft_ms: Some(200),
        },
        tokens: TokenStats {
            input: 100,
            output: 50,
        },
        stream: None,
    }
}

#[test]
fn new_engine_has_no_scores() {
    let engine = ScoringEngine::new(default_config());
    assert!(engine.get_score("ep-1").is_none());
}

#[test]
fn observe_success_raises_score() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    engine.observe(make_obs("ep-1", "pool-1", true));

    let score = engine.get_score("ep-1").unwrap();
    assert!(score.score > 50.0, "success should give decent score, got {}", score.score);
}

#[test]
fn observe_failure_lowers_score() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    engine.observe(make_obs("ep-1", "pool-1", false));

    let score = engine.get_score("ep-1").unwrap();
    assert!(score.score < 50.0, "failure should lower score, got {}", score.score);
}

#[test]
fn score_is_clamped() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    for _ in 0..10 {
        let mut obs = make_obs("ep-1", "pool-1", false);
        obs.latency = LatencyBreakdown {
            total_ms: 5000,
            ttft_ms: Some(3000),
        };
        engine.observe(obs);
    }

    let score = engine.get_score("ep-1").unwrap();
    assert!(
        score.score >= 0.0 && score.score <= 100.0,
        "score {} out of range",
        score.score
    );
}

#[test]
fn tier_calculation_is_correct() {
    assert_eq!(ScoringEngine::<FakeClock>::score_to_tier(95.0), ScoreTier::Gold);
    assert_eq!(ScoringEngine::<FakeClock>::score_to_tier(80.0), ScoreTier::Silver);
    assert_eq!(ScoringEngine::<FakeClock>::score_to_tier(50.0), ScoreTier::Bronze);
    assert_eq!(ScoringEngine::<FakeClock>::score_to_tier(30.0), ScoreTier::Poor);
}

#[test]
fn decay_does_not_panic() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    engine.observe(make_obs("ep-1", "pool-1", true));
    clock.advance(Duration::from_secs(600));
    engine.decay();

    let score = engine.get_score("ep-1").unwrap();
    assert!(score.score >= 0.0 && score.score <= 100.0);
}

#[test]
fn window_size_is_respected() {
    let clock = FakeClock::new();
    let mut config = default_config();
    config.window_size = 5;
    let mut engine = ScoringEngine::with_clock(config, clock.clone_ref());

    for i in 0..10 {
        let mut obs = make_obs("ep-1", "pool-1", true);
        obs.latency.ttft_ms = Some(i * 100);
        engine.observe(obs);
    }

    let state = engine.endpoints.get("ep-1").unwrap();
    assert_eq!(state.observations.len(), 5, "window should be capped at 5");
}

#[test]
fn stream_broken_lowers_quality() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    let mut obs = make_obs("ep-1", "pool-1", true);
    obs.stream = Some(StreamMetrics {
        ttft_ms: 200,
        tokens_per_second: Some(60.0),
        max_inter_chunk_ms: Some(100),
        chunk_count: 10,
        completed_normally: false,
        stream_broken: true,
    });
    engine.observe(obs);

    let breakdown = engine.get_breakdown("ep-1").unwrap();
    assert!(
        breakdown.quality < 1.0,
        "stream_broken should lower quality, got {}",
        breakdown.quality
    );

    let mut obs2 = make_obs("ep-1", "pool-1", false);
    obs2.error = Some(ObservationError::Timeout);
    engine.observe(obs2);

    let score = engine.get_score("ep-1").unwrap();
    assert!(
        score.score < 90.0,
        "quality penalty + failure should lower overall score, got {}",
        score.score
    );
}

#[test]
fn consecutive_timeouts_excludes_endpoint() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    for _ in 0..3 {
        let mut obs = make_obs("ep-1", "pool-1", false);
        obs.error = Some(ObservationError::Timeout);
        engine.observe(obs);
    }

    let score = engine.get_score("ep-1").unwrap();
    assert!(score.excluded, "endpoint should be excluded after 3 consecutive timeouts");
    assert_eq!(score.exclusion_reason.as_deref(), Some("consecutive timeouts"));
}

#[test]
fn success_resets_consecutive_counters() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    for _ in 0..2 {
        let mut obs = make_obs("ep-1", "pool-1", false);
        obs.error = Some(ObservationError::Timeout);
        engine.observe(obs);
    }

    engine.observe(make_obs("ep-1", "pool-1", true));

    let state = engine.endpoints.get("ep-1").unwrap();
    assert_eq!(
        state.consecutive_timeouts, 0,
        "success should reset consecutive_timeouts"
    );
    assert!(!state.last_score.excluded, "endpoint should not be excluded after success");
}

#[test]
fn quality_dimension_affects_score() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    let mut obs = make_obs("ep-1", "pool-1", true);
    obs.stream = Some(StreamMetrics {
        ttft_ms: 200,
        tokens_per_second: Some(80.0),
        max_inter_chunk_ms: Some(50),
        chunk_count: 20,
        completed_normally: true,
        stream_broken: false,
    });
    engine.observe(obs);

    let breakdown = engine.get_breakdown("ep-1").unwrap();
    assert!(
        breakdown.quality > 0.8,
        "good stream should give high quality, got {}",
        breakdown.quality
    );
}

#[test]
fn rank_pool_returns_ranking_with_recommended() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    let obs1 = make_obs("ep-1", "pool-1", true);
    let mut obs2 = make_obs("ep-2", "pool-1", true);
    obs2.latency.ttft_ms = Some(100);
    engine.observe(obs1);
    engine.observe(obs2);

    let ranking = engine.rank_pool("pool-1").unwrap();
    assert_eq!(ranking.pool_id, "pool-1");
    assert!(ranking.recommended.is_some(), "should have a recommended endpoint");
    assert!(ranking.ranked_endpoints.len() >= 2);
    if let Some(ref recommended) = ranking.recommended {
        assert_eq!(recommended.endpoint_id, ranking.ranked_endpoints[0].endpoint_id);
    }
}

#[test]
fn rank_pool_excludes_excluded_endpoints() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    for _ in 0..3 {
        let mut obs = make_obs("ep-1", "pool-1", false);
        obs.error = Some(ObservationError::Timeout);
        engine.observe(obs);
    }

    engine.observe(make_obs("ep-2", "pool-1", true));

    let ranking = engine.rank_pool("pool-1").unwrap();
    assert!(!ranking.excluded_endpoints.is_empty(), "should have excluded endpoints");
    assert_eq!(ranking.excluded_endpoints[0].endpoint_id, "ep-1");
    if let Some(ref recommended) = ranking.recommended {
        assert_eq!(recommended.endpoint_id, "ep-2");
    }
}

#[test]
fn rank_all_returns_all_pools() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    engine.observe(make_obs("ep-1", "pool-1", true));
    engine.observe(make_obs("ep-2", "pool-2", true));

    let all = engine.rank_all();
    assert!(all.len() >= 2);
    let pool_ids: Vec<&str> = all.iter().map(|ranking| ranking.pool_id.as_str()).collect();
    assert!(pool_ids.contains(&"pool-1"));
    assert!(pool_ids.contains(&"pool-2"));
}

#[test]
fn get_pool_feedback_returns_scores() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    engine.observe(make_obs("ep-1", "pool-1", true));
    engine.observe(make_obs("ep-2", "pool-1", true));

    let feedback = engine.get_pool_feedback("pool-1");
    assert!(feedback.endpoint_scores.contains_key("ep-1"));
    assert!(feedback.endpoint_scores.contains_key("ep-2"));
}

#[test]
fn export_and_restore_snapshot() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    engine.observe(make_obs("ep-1", "pool-1", true));
    engine.observe(make_obs("ep-2", "pool-1", false));

    let snapshot = engine.export_snapshot();
    assert!(snapshot.endpoints.len() >= 2);
    assert!(snapshot.last_decay_at_secs > 0);

    let mut engine2 = ScoringEngine::with_clock(default_config(), clock.clone_ref());
    engine2.restore_snapshot(snapshot);

    let s1 = engine.get_score("ep-1").unwrap();
    let s2 = engine2.get_score("ep-1").unwrap();
    assert_eq!(s1.score, s2.score);
}

#[test]
fn restored_engine_can_continue_observing() {
    let clock = FakeClock::new();
    let mut engine = ScoringEngine::with_clock(default_config(), clock.clone_ref());

    engine.observe(make_obs("ep-1", "pool-1", true));
    let snapshot = engine.export_snapshot();

    let mut engine2 = ScoringEngine::with_clock(default_config(), clock);
    engine2.restore_snapshot(snapshot);

    engine2.observe(make_obs("ep-1", "pool-1", true));
    let score = engine2.get_score("ep-1").unwrap();
    assert!(score.score > 0.0);
}

#[test]
fn decay_lowers_scores() {
    let clock = FakeClock::new();
    let mut config = default_config();
    config.decay_period_secs = 60;
    let mut engine = ScoringEngine::with_clock(config, clock.clone_ref());

    let mut obs = make_obs("ep-1", "pool-1", true);
    obs.started_at = clock.now() - Duration::from_secs(120);
    engine.observe(obs);

    clock.advance(Duration::from_secs(120));
    engine.observe(make_obs("ep-1", "pool-1", true));

    let score = engine.get_score("ep-1").unwrap().score;
    assert!(score >= 0.0 && score <= 100.0);
}

#[test]
fn decay_moves_score_toward_baseline() {
    let clock = FakeClock::new();
    let mut config = default_config();
    config.decay_period_secs = 60;
    config.baseline_score = 60.0;
    let mut engine = ScoringEngine::with_clock(config, clock.clone_ref());

    for _ in 0..5 {
        let mut obs = make_obs("ep-1", "pool-1", false);
        obs.started_at = clock.now() - Duration::from_secs(300);
        engine.observe(obs);
    }
    let score_before = engine.get_score("ep-1").unwrap().score;

    clock.advance(Duration::from_secs(300));
    engine.decay();

    let score_after = engine.get_score("ep-1").unwrap().score;
    assert_ne!(score_before, score_after);
}