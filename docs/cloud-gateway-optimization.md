# Latch Cloud-Native AI Gateway Optimization

> **TL;DR**: When targeting self-hosted GPU clusters (SGLang/vLLM), Latch acts as a **KV Cache keeper**. When targeting third-party cloud providers (OpenAI, Anthropic, Gemini, DeepSeek), Latch should evolve into a **Token compressor and request intelligent scheduler**.

---

## 1. Background & Strategy Shift

### Self-Hosted Scenario (Original Design)

| Mechanism | Value |
|-----------|-------|
| Stateful Context | Saves upstream bandwidth |
| Affinity Routing | Pins session to the same GPU node |
| Ghost Keep-Alive | Prevents KV Cache eviction via dummy requests |

**Core assumption**: We control the backend topology and can guarantee which GPU handles which session.

### Third-Party Cloud Scenario

Cloud providers expose standardized APIs (OpenAI-compatible or proprietary). Their infrastructure is a black box behind load balancers:

- **Affinity Routing fails**: LB distributes requests unpredictably across nodes.
- **Ghost Keep-Alive fails**: Heartbeat requests hit random nodes and burn money for no benefit.
- **Stateful Context partially works**: Saves Agent→Gateway bandwidth, but Gateway→Cloud still sends full context every time.

**The new optimization target shifts from "keeping KV hot" to "sending fewer tokens, to the right model, with resilience".**

---

## 2. Optimization Opportunities

### 2.1 Prompt Compression & History Pruning (Highest ROI)

Cloud providers charge by input tokens. A 200k-token Agent history often contains obsolete early messages that contribute little to the current turn.

**Strategies Latch can implement:**

| Strategy | Mechanism | Impact |
|----------|-----------|--------|
| **Sliding Window** | Retain only the most recent N turns; discard older ones. | Reduce 200k → 50k input tokens (75% savings). |
| **Summarization Swap** | When history exceeds threshold, send early messages to a cheap model (e.g., Claude Haiku, GPT-4o-mini) for summarization; replace raw messages with the summary. | Preserves semantic context while cutting token count significantly. |
| **Deduplication & Merge** | Merge adjacent same-role messages (e.g., multiple tool results). | Saves 10–30% formatting overhead. |
| **Static Prompt Deduplication** | If multiple sessions share an identical system prompt, manage it centrally instead of repeating per request. | Effective in multi-Agent deployments. |

> **Why the cloud provider won't do this**: They have no context of your Agent's workflow. Truncation is a product-level decision that only the gateway can make safely.

---

### 2.2 Automatic Prompt Caching Injection

Some cloud providers require **explicit markers** to enable KV Cache reuse. Latch can automatically rewrite request payloads.

#### Anthropic Claude

Requires `cache_control: {type: "ephemeral"}` on blocks intended for caching:

```json
// Original request from Agent
{"messages": [{"role": "system", "content": "<200k system prompt>"}]}

// Latch-rewritten request to Anthropic
{"messages": [
  {"role": "system", "content": "<200k system prompt>", "cache_control": {"type": "ephemeral"}}
]}
```

**Benefit**: Cache hits reduce input token cost by **~90%**.

#### Google Gemini

Requires explicit Context Caching API calls:

1. Latch creates a `cached_content` object for the static prefix.
2. Subsequent requests reference the cache ID instead of resending the full prefix.

**Benefit**: Avoid repeated billing for static long-context prefixes.

#### OpenAI / DeepSeek

Prompt Caching is **fully automatic**; no request modification needed.

**Latch's role**: Detect provider type from configuration and inject the appropriate caching metadata transparently.

---

### 2.3 Intelligent Model Routing

Not every request requires the most expensive frontier model.

**Simple heuristic router:**

```rust
fn route_model(history: &[Message], user_intent: &str) -> &str {
    if total_tokens < 4_000 && !user_intent.contains("code") {
        "gpt-4o-mini"        // ~20x cheaper
    } else if contains_image(history) {
        "gpt-4o"
    } else {
        "claude-opus"
    }
}
```

**Advanced router:**

- Run a local lightweight classifier (e.g., Llama 3.1 8B) to categorize intent complexity.
- Maintain a feedback loop: track routing decisions vs. downstream task success rates.
- Route retries to stronger models if the cheap model fails validation.

**Benefit**: Simple requests become nearly free; complex requests still get top-tier quality.

---

### 2.4 Retry, Fallback & Resilience

Cloud APIs are subject to transient failures that Agent code should not handle:

| Failure Mode | Latch Behavior |
|--------------|----------------|
| **429 Rate Limit** | Exponential backoff with jitter; queue request internally. |
| **503 Overloaded** | Retry up to N times; if persistent, fallback to alternate region or provider. |
| **SSE Stream Break** | Track received tokens; issue a continuation request with `max_tokens` adjusted. |
| **Timeout** | Return a graceful degradation response or retry with a faster model. |

**Benefit**: Agent developers write linear code; Latch absorbs cloud turbulence.

---

### 2.5 Result-Level Exact Cache

While conversations are generally non-deterministic, Agent workflows contain many **deterministic calls**:

- Repeated tool schema validation
- Identical code review requests
- Temperature=0 factual lookups
- Multi-Agent shared knowledge base queries

**Implementation:**

```rust
// Key: hash(messages, model, temperature, top_p)
// Value: cached response
static RESPONSE_CACHE: DashMap<u64, CachedResponse> = DashMap::new();
```

For exact key hits, return immediately without touching the cloud provider.

**Benefit**: Zero latency and zero cost for repetitive deterministic workloads.

---

### 2.6 Multi-Tenant Quota & Cost Attribution

If Latch serves multiple Agents, teams, or users:

| Feature | Implementation |
|---------|----------------|
| **Token Accounting** | Per-`session_id` / per-`api_key` input/output token counters. |
| **Hard Quotas** | Reject requests once a budget ceiling is reached (e.g., "$50/day per team"). |
| **Rate Limiting** | Per-tenant RPM/TPM throttling to prevent one rogue Agent from exhausting global capacity. |
| **Cost Attribution** | Export metrics to Prometheus / CloudWatch for cross-model, cross-provider unified billing. |

**Benefit**: Cloud providers bill the aggregate; Latch provides the per-tenant microscope.

---

### 2.7 Speculative Pre-loading (Advanced)

For Agents with highly predictable workflows (e.g., fixed multi-step pipeline):

1. Latch predicts the next likely user message or tool output.
2. Issues a **shadow request** to the cloud provider before the Agent officially asks.
3. If prediction is correct, serves the pre-fetched result instantly.
4. If wrong, silently discards the shadow response.

**Trade-off**: Burns tokens for speculative accuracy gains. Recommended only for latency-critical, deterministic pipelines.

---

## 3. Implementation Roadmap

| Phase | Feature | Files to Touch | Complexity |
|-------|---------|----------------|------------|
| **P0** | Sliding window history truncation | `handlers.rs` | Low |
| **P0** | Anthropic `cache_control` auto-injection | `handlers.rs` | Low |
| **P1** | Multi-model router + fallback | `router.rs`, `state.rs` | Medium |
| **P1** | Exponential backoff retry wrapper | `handlers.rs` | Medium |
| **P2** | Async summarization compression | New `src/compress.rs` | High |
| **P2** | Exact-key response cache | `state.rs`, `handlers.rs` | Medium |
| **P3** | Per-session quota & metering | `state.rs`, `ghost.rs` | Medium |
| **P3** | Speculative pre-loading | New `src/speculate.rs` | High |

---

## 4. Key Insight

> **Self-hosted GPU optimization is about physics (keeping KV Cache in VRAM).**
> **Cloud provider optimization is about economics (sending fewer tokens to cheaper endpoints).**

Latch's architecture—stateful session management, modular handlers, and background daemons—makes it an ideal chassis for both worlds. The plugin is the policy; the engine remains the same.

---

## 5. References

- [OpenAI Prompt Caching](https://platform.openai.com/docs/guides/prompt-caching)
- [Anthropic Prompt Caching](https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching)
- [Google Gemini Context Caching](https://ai.google.dev/gemini-api/docs/caching)
- [DeepSeek Hit Cache](https://platform.deepseek.com/api-docs/pricing/)
