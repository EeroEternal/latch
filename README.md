# latch
Feature-gated SDK crates for Latch gateway capabilities

## Workspace Notes

- This repository is now a pure Cargo workspace focused on SDK crates.
- `crates/latch-core` is an extracted library crate for neutral shared types and configs.
- `crates/latch-compress` provides stateless compression primitives for transparent proxy mode.
- `crates/latch-cache` provides prompt-cache metadata planning/injection helpers.
- `crates/latch-router` provides synchronous heuristic model/pool routing decisions.
- `crates/latch-retry` provides retry/fallback/circuit-breaker policy primitives.
- `crates/latch-detect` provides backend engine auto-detection with mockable probes.
- `crates/latch-meter` provides per-session usage metering, quota checks, and cost estimation.
- `crates/latch-sdk` is a feature-gated umbrella crate for downstream consumers.
- `latch-core` intentionally has no UniGateway SDK dependency, so upstream gateways can adapt their own protocol types at the boundary.

## `latch-compress` MVP API

```rust
use latch_compress::{sliding_window, sliding_window_with_meta};

let trimmed = sliding_window(&messages, 8);
let meta = sliding_window_with_meta(&messages, 8);
```

- `sliding_window` keeps all `system` messages and the last `max_turns * 2` non-system messages.
- `sliding_window_with_meta` returns `CompressionResult` for observability.

## `latch-cache` MVP API

```rust
use latch_cache::{apply_prompt_cache_plan, plan_prompt_cache};
use latch_core::PromptCacheProvider;

let plan = plan_prompt_cache(&messages, PromptCacheProvider::Anthropic);
let tagged = apply_prompt_cache_plan(&messages, &plan);
```

- Anthropic mode: marks `system` messages with `cache_control: {"type":"ephemeral"}`.
- OpenAI-compatible mode: no request-body rewrite.

## `latch-router` MVP API

```rust
use latch_core::RouterConfig;
use latch_router::route_model;

let cfg = RouterConfig {
    fast_pool: "fast".into(),
    strong_pool: "strong".into(),
    confidence_threshold: 0.7,
};
let decision = route_model(&messages, &cfg);
```

- Pure synchronous heuristic decision, async runtime-agnostic.
- Returns `RoutingDecision { provider, reason, confidence }` for observability hooks.

## `latch-retry` MVP API

```rust
use latch_retry::{next_decision, AttemptDecision, RetryState};

let mut state = RetryState::default();
let decision = next_decision(&mut state, &retry_config, attempt_index);
match decision {
    AttemptDecision::RetryAfter(d) => { /* sleep + retry */ }
    AttemptDecision::Fallback { provider } => { /* switch provider */ }
    AttemptDecision::Stop => { /* bubble error */ }
}
```

- Sync-first policy engine (runtime-agnostic).
- Optional `tokio` feature provides `sleep_for(...)` async helper.

## `latch-detect` MVP API

```rust
use latch_detect::{detect_backend, ReqwestProbe};

let probe = ReqwestProbe::new(base_url, reqwest_client);
let report = detect_backend(&probe).await?;
```

- Uses `HttpProbe` trait abstraction for testability.
- Ships direct `ReqwestProbe` implementation for production use.
- Assumes direct network probing (no proxy-specific branch logic).

## `latch-meter` MVP API

```rust
use latch_meter::UsageMeter;

let mut meter = UsageMeter::new();
let verdict = meter.preview_request("session-1", &meter_cfg, predicted_in, predicted_out);
if matches!(verdict, latch_core::MeterVerdict::Allow) {
    let usage = meter.record_request("session-1", &meter_cfg, actual_in, actual_out);
}
```

- Sync-only, runtime-agnostic usage accounting.
- Supports per-session token/request limits.
- Tracks estimated spend using separate input/output token prices.
