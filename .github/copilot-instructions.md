# Copilot Instructions for `latch`

## Build, test, and lint commands

This repository is a pure Cargo workspace (no Makefile/justfile/task runner). Use workspace-level Cargo commands:

```bash
# Build all crates
cargo build --workspace

# Run all tests
cargo test --workspace

# Run one specific test (example)
cargo test -p latch-compress keeps_system_and_last_turns

# Run one async detector test (example)
cargo test -p latch-detect detects_vllm_from_metrics

# Lint all crates/targets
cargo clippy --workspace --all-targets

# Formatting check
cargo fmt --all --check
```

## High-level architecture

- The workspace is split into small SDK crates under `crates/`, with `latch-core` as the neutral shared-types/config crate used by all others.
- `latch-sdk` is the umbrella crate: it always re-exports `latch-core` and re-exports other crates behind feature flags (`compress`, `cache`, `router`, `retry`, `detect`, `meter`, or `full`).
- Most crates are policy/primitives libraries (not a running gateway service):  
  - `latch-compress`: history compression (`sliding_window` + metadata variant).  
  - `latch-cache`: provider-specific prompt-cache tagging plan + application.  
  - `latch-router`: synchronous heuristic routing to fast/strong pools.  
  - `latch-retry`: retry/fallback/circuit-breaker decision engine.  
  - `latch-meter`: per-session quota/cost metering.  
  - `latch-detect`: async backend fingerprinting via probe traits and HTTP probing.
- `latch-detect` is the only crate centered on async I/O; it uses `HttpProbe` for mockable detection logic and `ReqwestProbe` as the production implementation.

## Key conventions in this codebase

- Keep `latch-core` dependency-light and protocol-neutral; backend/gateway-specific protocol adaptation is expected at integration boundaries, not in core types.
- Prefer sync-first, runtime-agnostic APIs for policy crates (`compress`, `cache`, `router`, `retry`, `meter`); async helpers are optional features (for example, `latch-retry`’s `tokio` sleep helper).
- Model/provider behavior is encoded explicitly in enums/config instead of stringly-typed branching spread across files (for example `PromptCacheProvider`, `BackendKind`, `MeterVerdict`, `AttemptDecision`).
- Prompt-cache rewriting is intentionally provider-specific: Anthropic mode tags `system` messages with `cache_control: { "type": "ephemeral" }`; OpenAI-compatible mode does not rewrite messages.
- Keep heuristics lightweight and deterministic in-library (for example token estimation via `chars / 4` in compression/routing paths) so crates stay dependency-light.
- Unit tests are colocated in each crate’s `src/lib.rs`; when changing behavior, update crate-local tests first rather than adding a separate integration test harness.
