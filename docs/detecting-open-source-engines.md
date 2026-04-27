# Detecting Directly Exposed Open-Source Inference Engines

> **Goal**: Determine whether a remote API endpoint is backed by a directly exposed open-source engine (vLLM, SGLang, TensorRT-LLM, TGI) or by a proprietary cloud provider/aggregator with opaque infrastructure.

---

## 1. Why Detection Matters for Latch

| Backend Type | Recommended Latch Strategy |
|-------------|---------------------------|
| **Direct vLLM/SGLang** | Full power: Affinity Routing + Ghost Keep-Alive + Stateful Context |
| **Cloud Provider (OpenAI, Anthropic)** | Cloud-Native optimizations only: compression, model routing, caching injection |
| **Aggregator (OpenRouter, Together)** | Hybrid: keep-alive may work if they expose session stickiness; usually treat as cloud |
| **Proxied vLLM/SGLang (via Nginx/Cloudflare)** | Depends on proxy config; sticky sessions + health endpoints often leak through |

If we can reliably fingerprint the engine, Latch can **auto-configure** its optimization tier instead of forcing users to guess.

---

## 2. Fingerprinting Methodology

We use a **multi-signal approach**:

1. **Passive Probes**: Non-destructive endpoint enumeration (`GET` requests).
2. **Active Probes**: Send minimal test requests to trigger engine-specific behavior.
3. **Response Semantics**: Parse error formats, header leaks, and non-standard fields.

---

## 3. Passive Probes (Zero Risk)

### 3.1 Endpoint Enumeration

Try `GET` on these paths. A `200` or `404` with a specific body reveals the engine:

| Endpoint | vLLM | SGLang | TensorRT-LLM (Triton) | OpenAI |
|----------|------|--------|----------------------|--------|
| `/v1/models` | ✅ | ✅ | ❌ (uses `/v2/models`) | ✅ |
| `/metrics` | ✅ **Prometheus** | ❌ | ✅ **Prometheus** | ❌ |
| `/health` | ✅ | ✅ `/health_generate` | ✅ `/v2/health/ready` | ❌ |
| `/` (root) | FastAPI docs page | FastAPI docs page | Triton splash page | 302/404 |
| `/tokenize` | ✅ **Native** | ❌ | ❌ | ❌ |
| `/v1/completions` | ✅ Legacy | ✅ Legacy | ❌ | ⚠️ Deprecated |
| `/get_model_info` | ❌ | ✅ **Native** | ❌ | ❌ |
| `/version` | ❌ | ❌ | ✅ | ❌ |

**Key Finding**: If `/metrics` returns Prometheus metrics containing `vllm:` prefixed names, you are talking to a **directly exposed vLLM** instance.

### 3.2 Response Header Leaks

Check `Server` and `X-*` headers:

```bash
curl -sI https://api.target.com/v1/chat/completions
```

| Header Signature | Likely Engine |
|-----------------|---------------|
| `Server: uvicorn` | FastAPI-based (vLLM, SGLang, many proxies) |
| `Server: nginx` + no `X-` headers | Likely behind reverse proxy; engine hidden |
| `X-SGLang-Server: ...` | **Direct SGLang** |
| `X-Request-Id: req_...` | OpenAI-style proxy layer |
| `X-Ratelimit-*` | OpenAI or copycat |
| No `Server` header at all | Cloudflare/AWS ALB in front |

> **Note**: Headers can be sanitized by proxies. Absence of leaks does not prove it's a cloud provider; presence of leaks confirms direct exposure.

### 3.3 Model List (`/v1/models`) Semantics

```bash
curl -s https://api.target.com/v1/models | jq '.data[] | {id, owned_by}'
```

| Pattern | Interpretation |
|---------|---------------|
| `owned_by: "openai"` or `owned_by: "anthropic"` | Official cloud API or high-fidelity proxy |
| `owned_by: ""` or `owned_by: "vllm"` | Direct vLLM deployment |
| `id` contains local path like `/data/models/...` | **Direct exposure** with no sanitization |
| `root` field present with local filenames | **Direct exposure** |
| Many models (100+) from disparate sources | Aggregator (OpenRouter, Together) |

---

## 4. Active Probes (Low Cost)

### 4.1 The Invalid Parameter Test

Send a request with a parameter that OpenAI rejects but open-source engines accept:

**Test A: `best_of` (vLLM native)**

```bash
curl -s https://api.target.com/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "default",
    "messages": [{"role": "user", "content": "hi"}],
    "max_tokens": 1,
    "best_of": 2
  }'
```

| Response | Engine |
|----------|--------|
| Success (`200`) with multiple completions | **Direct vLLM** or compatible |
| `400` `"best_of" is not a valid parameter` | **OpenAI / strict proxy** |

**Test B: `ignore_eos` (vLLM native)**

```bash
curl -s https://api.target.com/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "default",
    "messages": [{"role": "user", "content": "hi"}],
    "max_tokens": 1,
    "ignore_eos": true
  }'
```

| Response | Engine |
|----------|--------|
| Success | **Direct vLLM / SGLang** |
| `400` invalid parameter | **Cloud provider** |

**Test C: `regex` constraint (SGLang native)**

```bash
curl -s https://api.target.com/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "default",
    "messages": [{"role": "user", "content": "hi"}],
    "max_tokens": 10,
    "regex": "[0-9]+"
  }'
```

| Response | Engine |
|----------|--------|
| Success, output matches regex | **Direct SGLang** |
| `400` unknown field | **vLLM / Cloud** |

### 4.2 Error Format Fingerprint

Trigger an obvious error (e.g., `max_tokens: -1`) and inspect the JSON:

```bash
curl -s https://api.target.com/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "default", "messages": [], "max_tokens": -1}'
```

| Error Body Pattern | Engine |
|-------------------|--------|
| `{"error": {"message": "...", "type": "invalid_request_error", "param": null, "code": null}}` | OpenAI-style |
| `{"error": {"message": "...", "type": "", "param": "", "code": ...}}` + Python traceback hints | **Direct vLLM** |
| `{"error": {"message": "...", "code": ...}}` + mentions of `srt` or `scheduler` | **Direct SGLang** |
| HTML error page (nginx/Cloudflare) | Behind proxy / aggregator |

### 4.3 Tokenization Probe

vLLM exposes a non-standard `/tokenize` endpoint:

```bash
curl -s https://api.target.com/tokenize \
  -H "Content-Type: application/json" \
  -d '{"model": "default", "prompt": "hello world"}'
```

- `200` with token array → **Direct vLLM**
- `404` → Could be anything else

---

## 5. Engine-Specific Deep Dives

### 5.1 vLLM Fingerprints

**Positive signals:**
- `/metrics` exposes `vllm:gpu_cache_usage_perc`, `vllm:num_requests_running`, `vllm:generation_tokens_total`
- Accepts `best_of`, `use_beam_search`, `ignore_eos`, `spaces_between_special_tokens`
- `usage` block includes exact token counts derived from the model's tokenizer
- Streaming SSE uses `"data: {"choices":[{"delta":{"content":"..."}}]}` without OpenAI's extra `system_fingerprint` field

**Negative signals:**
- Absence of `/tokenize` does not rule out vLLM (can be disabled).

### 5.2 SGLang Fingerprints

**Positive signals:**
- `/get_model_info` returns `{"model_path": "...", "tokenizer_path": "..."}`
- `/health_generate` accepts a dummy generation request for health checking
- Supports `regex` and `json_schema` in the chat completion body (structured generation)
- SSE stream ends with a `meta_info` chunk: `{"meta_info": {"prompt_tokens": ..., "completion_tokens": ...}}`
- May expose `X-SGLang-Server` header

**Negative signals:**
- SGLang is also FastAPI-based, so root `/` returning a docs page is not unique.

### 5.3 TensorRT-LLM (via Triton) Fingerprints

**Positive signals:**
- Endpoints follow `/v2/models/{model_name}/infer` or `/v2/models/{model_name}/generate`
- `/v2/health/ready` and `/v2/health/live` return `200` with empty body
- Root `/` returns NVIDIA Triton server splash page
- May expose `Server: NVIDIA-Triton` header

### 5.4 Text Generation Inference (HuggingFace TGI) Fingerprints

**Positive signals:**
- `/info` endpoint returns model metadata, including `model_id`, `model_sha`, `model_pipeline_tag`
- `/metrics` returns Prometheus metrics prefixed with `tgi_`
- Supports `grammar` parameter for constrained generation (JSON schema via GBNF)

---

## 6. Aggregator & Proxy Detection

Some platforms (OpenRouter, Together, Fireworks, Groq) run open-source engines internally but expose a unified API.

**Detection heuristics:**

| Signal | Meaning |
|--------|---------|
| Model IDs from many providers (e.g., `openai/gpt-4o`, `anthropic/claude-3-opus`, `meta-llama/...`) | **Aggregator** (OpenRouter-style) |
| `X-OpenRouter-...` headers | **OpenRouter** specifically |
| Requires custom auth headers (e.g., `x-api-key` instead of `Authorization`) | Likely aggregator or private deployment |
| Response contains `usage.total_cost` field | **OpenRouter** billing extension |
| Rate limit headers are extremely precise (`X-Ratelimit-Limit-Requests`, `X-Ratelimit-Remaining-Tokens`) | **OpenAI** or high-fidelity proxy |

**Important**: If an aggregator does **not** provide session stickiness, Ghost Keep-Alive is useless because the aggregator's internal scheduler will redistribute requests. However, some aggregators allow `X-Session-ID` or similar for sticky routing—check their docs.

---

## 7. Putting It All Together: Decision Tree

```
1. GET /metrics
   ├── Returns Prometheus + vllm:* metrics ──► DIRECT VLLM
   ├── Returns Prometheus + tgi:* metrics ───► DIRECT TGI
   └── 404 or non-Prometheus ───────────────► Go to 2

2. GET /tokenize
   ├── 200 with token array ────────────────► DIRECT VLLM (high confidence)
   └── 404 ─────────────────────────────────► Go to 3

3. GET /get_model_info
   ├── 200 with model_path ─────────────────► DIRECT SGLANG
   └── 404 ─────────────────────────────────► Go to 4

4. GET /v1/models, inspect owned_by & id
   ├── owned_by empty / local paths in id ──► DIRECT EXPOSURE (engine TBD)
   ├── owned_by = "openai" / "anthropic" ───► CLOUD PROVIDER
   └── 100+ models from mixed sources ──────► AGGREGATOR

5. POST /v1/chat/completions with best_of=2
   ├── 200 success ─────────────────────────► DIRECT VLLM or COMPATIBLE
   └── 400 unknown param ───────────────────► STRICT PROXY / CLOUD

6. POST /v1/chat/completions with regex="[0-9]+"
   ├── 200 & output matches regex ──────────► DIRECT SGLANG
   └── 400 unknown param ───────────────────► VLLM / CLOUD / OTHER

7. POST with max_tokens=-1, inspect error format
   ├── Contains "srt" or "scheduler" ───────► DIRECT SGLANG
   ├── Contains Python traceback hints ─────► DIRECT VLLM
   └── Clean OpenAI-style JSON error ───────► CLOUD / WELL-PROXIED
```

---

## 8. Practical Recommendation for Latch

Instead of manual detection, Latch should implement an **auto-discovery probe** on startup:

```rust
// Pseudo-code for latch bootstrap
async fn detect_engine(url: &str) -> EngineType {
    if probe_metrics(url).await.is_vllm() {
        return EngineType::DirectVllm;
    }
    if probe_get_model_info(url).await.is_ok() {
        return EngineType::DirectSglang;
    }
    if probe_tokenize(url).await.is_ok() {
        return EngineType::DirectVllm; // secondary signal
    }
    if probe_regex_constraint(url).await.matches() {
        return EngineType::DirectSglang;
    }
    EngineType::UnknownCloud
}
```

**Auto-configuration mapping:**

| Detected Engine | Latch Mode | Keep-Alive | Affinity | Compression |
|----------------|------------|------------|----------|-------------|
| `DirectVllm` | Full optimization | ✅ Enabled | ✅ Enabled | ⚠️ Optional |
| `DirectSglang` | Full optimization | ✅ Enabled | ✅ Enabled | ⚠️ Optional |
| `Aggregator` | Hybrid | ❌ Disabled | ⚠️ If sticky header available | ✅ Enabled |
| `UnknownCloud` | Cloud-native only | ❌ Disabled | ❌ Disabled | ✅ Enabled |

---

## 9. References

- [vLLM OpenAI-Compatible Server Docs](https://docs.vllm.ai/en/latest/serving/openai_compatible_server.html)
- [SGLang Backend API](https://docs.sglang.ai/backend/backend.html)
- [NVIDIA Triton Inference Server](https://docs.nvidia.com/deeplearning/triton-inference-server/)
- [HuggingFace TGI](https://huggingface.co/docs/text-generation-inference/)
- [OpenRouter API Docs](https://openrouter.ai/docs)
