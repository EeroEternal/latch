# latch

**Latch = 状态层。为大模型的无状态交互提供有状态管理能力。**

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-blue.svg)](https://www.rust-lang.org)

## 🎯 项目定位

**Latch 是一个纯基础库（SDK）集合，专注于提供运行时无关的状态管理能力。**

### 核心设计原则

- **运行时无关（Runtime-Agnostic）**：所有核心 crate 都是纯同步的，不绑定特定异步运行时（tokio、async-std 等）
- **零 I/O 依赖**：不直接操作数据库、网络或文件系统，只提供计算逻辑和类型定义
- **可插拔（Pluggable）**：通过 trait 抽象，下游应用可以自行实现具体的 I/O 层
- **中立性（Neutral）**：不依赖具体的 gateway 实现或 provider SDK，保持通用性

### 边界说明

**✅ Latch 包含的：**
- 类型定义和数据结构
- 同步计算逻辑（评分、路由决策、重试策略等）
- Trait 定义（供下游应用实现）
- 配置结构和验证

**❌ Latch 不包含的：**
- 异步运行时基础设施（如 tokio 缓冲层、连接池）
- 数据库实现（PostgreSQL、Redis 等）
- 具体的 gateway 业务逻辑
- Provider SDK wrapper（OpenAI、Anthropic 等）
- HTTP 服务器框架（Axum、Actix 等）
- 发票、支付、税率等财务系统

> **下游应用（如 xrouter）需要自行实现：** 异步缓冲层、数据库存储、网络 I/O、运行时集成等。

---

## 📦 Crates 概览

Latch 采用 Cargo workspace 结构，每个 crate 都有明确的职责边界：

### 核心基础

| Crate | 版本 | 描述 | 同步/异步 |
|-------|------|------|-----------|
| **[latch-core](crates/latch-core/)** | 0.2.0 | 共享类型、配置和枚举定义 | 纯同步 |
| **[latch-sdk](crates/latch-sdk/)** | - | Feature-gated 伞形 crate，方便下游引用 | - |

### 状态管理能力

| Crate | 版本 | 描述 | 同步/异步 |
|-------|------|------|-----------|
| **[latch-compress](crates/latch-compress/)** | - | 无状态压缩原语（滑动窗口消息截断） | 纯同步 |
| **[latch-cache](crates/latch-cache/)** | - | Prompt 缓存元数据规划和注入辅助 | 纯同步 |
| **[latch-router](crates/latch-router/)** | - | 基于启发式的模型/池路由决策 | 纯同步 |
| **[latch-retry](crates/latch-retry/)** | 0.1.1 | 重试/回退/熔断策略原语（可选 tokio 辅助） | 纯同步 + 可选异步 |
| **[latch-detect](crates/latch-detect/)** | 0.2.0 | 后端引擎自动检测（网络探测） | 异步（需网络 I/O） |
| **[latch-meter](crates/latch-meter/)** | 0.2.0 | 会话级用量计量、配额检查和成本估算 | 纯同步 |
| **[latch-billing](crates/latch-billing/)** | 0.1.0 | 精确的 token 计费核心（定价、评分、配额） | 纯同步 |
| **[latch-score](crates/latch-score/)** | 0.1.0 | 端点质量评分引擎（延迟、TTFT、流健康度等） | 纯同步 |

---

## 🚀 快速开始

## 📚 Crate 详细说明

### `latch-core` - 共享类型和配置

所有其他 crate 的基础，定义了通用的类型、配置结构和枚举。

```rust
use latch_core::{MeterConfig, RouterConfig, EndpointScore};
```

- 不包含任何具体 gateway 或 provider 的依赖
- 上游 gateway 可以在边界处适配自己的协议类型

---

### `latch-compress` - 消息压缩原语

```rust
use latch_compress::{sliding_window, sliding_window_with_meta};

let trimmed = sliding_window(&messages, 8);
let meta = sliding_window_with_meta(&messages, 8);
```

- `sliding_window` 保留所有 `system` 消息和最后 `max_turns * 2` 条非 system 消息
- `sliding_window_with_meta` 返回 `CompressionResult` 用于可观测性
- **适用场景**：透明代理模式下的上下文窗口管理

---

### `latch-cache` - Prompt 缓存管理

```rust
use latch_cache::{apply_prompt_cache_plan, plan_prompt_cache};
use latch_core::PromptCacheProvider;

let plan = plan_prompt_cache(&messages, PromptCacheProvider::Anthropic);
let tagged = apply_prompt_cache_plan(&messages, &plan);
```

- Anthropic 模式：标记 `system` 消息为 `cache_control: {"type":"ephemeral"}`
- OpenAI 兼容模式：不重写请求体
- **适用场景**：优化 prompt 缓存命中率，降低 token 成本

---

### `latch-router` - 模型路由决策

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

- 纯同步启发式决策，运行时无关
- 返回 `RoutingDecision { provider, reason, confidence }` 用于可观测性
- **适用场景**：根据消息特征选择 fast/strong 模型池

---

### `latch-retry` - 重试和熔断策略

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

- 同步优先的策略引擎（运行时无关）
- 可选 `tokio` feature 提供 `sleep_for(...)` 异步辅助函数
- **适用场景**：请求失败时的重试、回退到备用 provider、熔断保护

---

### `latch-detect` - 后端引擎检测

```rust
use latch_detect::{detect_backend, ReqwestProbe};

let probe = ReqwestProbe::new(base_url, reqwest_client);
let report = detect_backend(&probe).await?;
```

- 使用 `HttpProbe` trait 抽象，可测试性强
- 内置 `ReqwestProbe` 实现用于生产环境
- 直接网络探测（无 proxy 特定逻辑）
- **注意**：这是唯一需要异步的 crate，因为必须进行网络 I/O

---

### `latch-meter` - 会话用量计量

```rust
use latch_meter::UsageMeter;

let mut meter = UsageMeter::new();
let verdict = meter.preview_request("session-1", &meter_cfg, predicted_in, predicted_out);
if matches!(verdict, latch_core::MeterVerdict::Allow) {
    let usage = meter.record_request("session-1", &meter_cfg, actual_in, actual_out);
}
```

- 纯同步，运行时无关的用量会计
- 支持 per-session token/请求数限制
- 使用独立的 input/output token 价格跟踪预估成本
- **适用场景**：gateway 层面的请求准入控制、配额检查

---

### `latch-billing` - 精确计费核心

提供完整的 token 计费核心逻辑，包括观测、定价、评分和配额管理。

```rust
use latch_billing::{MeterSet, MeterKind, UsageObservation, RatingEngine};

// 1. 创建观测
let mut meter_set = MeterSet::new();
meter_set.accumulate(MeterKind::InputTokens, 1000)?;
meter_set.accumulate(MeterKind::OutputTokens, 500)?;

// 2. 获取价格快照（由调用方从 DB 查询）
let snapshot = /* ... */;

// 3. 评分
let engine = /* RatingEngine 实现 */;
let result = engine.rate(&observation, &snapshot, &context)?;
println!("Cost: {} {}", result.total_cost, result.currency);
```

- 纯同步，零 I/O 依赖
- 定义核心类型：`UsageObservation`, `MeterSet`, `PriceSnapshot`, `RatedUsageRecord`
- 定义 trait：`PricingSource`, `RatingEngine`, `ObservationStore`, `QuotaAuthorizer`
- **适用场景**：精确的 token 计费、阶梯定价、配额授权
- **注意**：异步存储和缓冲层由下游应用自行实现

---

### `latch-score` - 端点质量评分

基于请求观测数据的质量评分引擎，为路由决策提供反馈。

```rust
use latch_score::{EndpointScorer, ScorerConfig};
use latch_core::{EndpointObservation, EndpointScore};

let config = ScorerConfig {
    latency_weight: 0.3,
    ttft_weight: 0.2,
    tps_weight: 0.2,
    // ...
};
let scorer = EndpointScorer::new(config);
let score = scorer.compute_score(&observation);
```

- 纯同步，运行时无关
- 评分维度：延迟、TTFT、TPS、流健康度、错误率等
- 输出：端点分数、质量等级、排除原因
- **适用场景**：为路由层提供端点质量反馈，优化路由决策

---

## 🏗️ 架构设计

### 分层架构

```
┌─────────────────────────────────────────────┐
│          下游应用（如 xrouter）              │
│  ┌─────────────────────────────────────┐   │
│  │  异步基础设施层                      │   │
│  │  - tokio 运行时                     │   │
│  │  - 缓冲层（BufferedMeteringSink）   │   │
│  │  - 数据库存储（PostgreSQL/Redis）   │   │
│  │  - HTTP 服务器（Axum/Actix）       │   │
│  └─────────────────────────────────────┘   │
│                      ↓                      │
│  ┌─────────────────────────────────────┐   │
│  │  Latch SDK（纯同步核心）             │   │
│  │  - latch-core（共享类型）           │   │
│  │  - latch-billing（计费核心）        │   │
│  │  - latch-score（质量评分）          │   │
│  │  - latch-router（路由决策）         │   │
│  │  - latch-meter（用量计量）          │   │
│  │  - latch-retry（重试策略）          │   │
│  │  - latch-compress（消息压缩）       │   │
│  │  - latch-cache（Prompt 缓存）       │   │
│  └─────────────────────────────────────┘   │
└─────────────────────────────────────────────┘
```

### 依赖关系

```
latch-sdk
├── latch-core（所有 crate 的基础）
├── latch-billing（依赖 latch-core）
├── latch-score（依赖 latch-core）
├── latch-meter（依赖 latch-core）
├── latch-router（依赖 latch-core）
├── latch-retry（依赖 latch-core，可选 tokio）
├── latch-compress（依赖 latch-core）
├── latch-cache（依赖 latch-core）
└── latch-detect（依赖 latch-core，需要 reqwest）
```

---

## 🤝 下游应用集成指南

### 1. 添加依赖

在你的 gateway 项目（如 xrouter）中：

```toml
# Cargo.toml
[dependencies]
latch-core = { version = "0.2.0", git = "https://github.com/EroeEternal/latch" }
latch-billing = { version = "0.1.0", git = "https://github.com/EroeEternal/latch" }
latch-score = { version = "0.1.0", git = "https://github.com/EroeEternal/latch" }
# ... 其他需要的 crate
```

### 2. 实现异步基础设施

Latch 提供 trait，你需要实现具体的 I/O 层：

```rust
use latch_billing::{ObservationStore, StoreResult, StoreError};
use latch_billing::observation::UsageObservation;

// 实现你自己的异步存储
struct PgObservationStore {
    pool: sqlx::PgPool,
}

impl ObservationStore for PgObservationStore {
    fn append_observation(&self, obs: UsageObservation) -> Result<StoreResult, StoreError> {
        // 委托给异步缓冲层或直接写入
        // 或使用 block_on（不推荐）
        todo!()
    }
}
```

### 3. 在应用启动时构造

```rust
use latch_billing::RatingEngine;
use latch_score::EndpointScorer;

// 在 composition root 层构造
let billing_engine = DefaultRatingEngine::new();
let scorer = EndpointScorer::new(config);

// 注入到 gateway state
let state = GatewayState {
    billing_engine: Arc::new(billing_engine),
    scorer: Arc::new(scorer),
    // ...
};
```

### 4. 在请求处理中使用

```rust
// 请求完成后，记录用量
let observation = build_observation(&request_context, &usage);
let result = state.billing_engine.rate(&observation, &price_snapshot, &context)?;

// 更新端点分数
let score = state.scorer.compute_score(&endpoint_observation);
```

---

## 📖 设计文档

- [latch-billing 设计文档](docs/latch-billing-design.md) - 计费系统详细设计
- [latch-score 需求文档](docs/latch-score-requirements.md) - 质量评分系统需求
- [检测开源引擎](docs/detecting-open-source-engines.md) - 后端检测策略

---

## 🛠️ 开发指南

### 构建项目

```bash
# 检查所有 crate 编译
cargo check --workspace

# 运行测试
cargo test --workspace

# 排除 latch-score（如有已知问题）
cargo check --workspace --exclude latch-score
```

### Crate 特性开关

部分 crate 提供可选 feature：

```toml
# latch-retry: 启用 tokio 辅助函数
latch-retry = { version = "0.1.1", features = ["tokio"] }

# latch-billing: 启用 TOML 定价源
latch-billing = { version = "0.1.0", features = ["toml"] }
```

---

## 📄 License

Apache-2.0。详见 [LICENSE](LICENSE) 文件。

---

## 🙋 常见问题

**Q: 为什么 latch 不使用异步？**  
A: 保持运行时无关性。核心计算逻辑是纯同步的，下游应用可以根据自己的需求选择异步运行时（tokio、async-std 等）。

**Q: latch 可以直接用于生产环境吗？**  
A: latch 提供的是核心计算逻辑，你需要自行实现异步基础设施（数据库、缓冲层等）才能用于生产。

**Q: 如何在 latch 中接入我的 gateway？**  
A: 在你的 gateway 中添加 latch crate 依赖，在边界处将 gateway 的类型转换为 latch 的类型，然后使用 latch 的计算逻辑。

**Q: latch-billing-tokio 去哪里了？**  
A: 已移除。异步基础设施应由下游应用（如 xrouter）自行实现，以保持 latch 的纯基础库定位。
