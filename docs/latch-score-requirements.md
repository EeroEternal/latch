# latch-score 质量评分能力需求文档

本文档用于向 `latch-score` 提出一项可复用的通用能力需求：
为模型服务端点提供基于运行时观测数据的质量评分、排名与反馈输出能力。

本文档中的 ParaRouter 只是第一方宿主（host）系统，不应成为 `latch-score` 的实现前提。
`latch-score` 应当作为独立评分引擎存在，可被任意 API 网关、路由器或代理层复用。

## 1. 背景

对于最终用户而言，真正重要的能力通常不是"接入多少个上游服务商"，而是：

- 通过一个统一 API 接入多个模型。
- 对 token 用量、成本和调用情况做精确统计。
- 在后台自动识别哪些模型端点更稳定、更快、更适合承载流量。

ParaRouter 会负责统一接入、鉴权、计量、账单与业务路由。
但"根据运行质量对模型端点进行持续评分与排名"这一能力，本质上更适合作为 `latch-score` 的职责，而不是直接写死在 ParaRouter 内部。

因此，希望 `latch-score` 提供一个通用质量评分引擎：

- 输入：宿主系统上报的请求观测数据。
- 输出：端点分数、池内排序、推荐结果、排除原因与可供路由层消费的反馈结构。

## 2. 目标

`latch-score` 需要满足以下目标：

- 支持对单个 endpoint 的运行质量做滚动评分。
- 支持按 pool 对 endpoint 进行排名与推荐。
- 支持根据最近窗口内的请求样本自动衰减历史影响。
- 支持解释性输出，而不仅是一个黑盒分数。
- 支持同步调用，不依赖特定 async runtime。
- 不依赖 ParaRouter、UniGateway 或任何具体网关实现。
- 能作为 `latch-router` 或宿主路由层的上游反馈来源。

## 3. 非目标

以下内容不应由 `latch-score` 直接承担：

- 不负责采集网关原始请求数据。
- 不负责理解宿主的用户体系、套餐、预算或权限模型。
- 不负责直接执行 HTTP 请求或流式转发。
- 不负责具体业务路由决策，只提供质量反馈与排序结果。
- 不负责展示层 UI，仅输出稳定、可序列化的数据结构。

换言之，`latch-score` 只处理"质量信号"，不处理"业务信号"。

## 4. 核心职责边界

建议职责划分如下。

### `latch-score` 负责

- 定义通用观测模型。
- 定义评分配置与权重。
- 累积观测样本并更新端点分数。
- 生成 endpoint、pool、全局维度的排名结果。
- 输出分数明细、分层（tier）、排除原因和反馈结构。
- 支持状态快照导出与恢复，便于宿主持久化。

### 宿主系统负责

- 采集请求级与流级观测数据。
- 将宿主数据转换为 `latch-score` 所需的观测结构。
- 把评分结果用于后台展示或路由决策。
- 将质量评分与价格、权限、预算、模型适配等业务规则结合。

## 5. 输入能力要求

`latch-score` 的输入应基于宿主上报的统一观测结构，而不是直接依赖某个 SDK 的原始类型。

### 5.1 请求观测模型

建议提供类似如下的核心结构：

```rust
pub struct RequestObservation {
    pub endpoint_id: String,
    pub pool_id: String,
    pub started_at: SystemTime,
    pub success: bool,
    pub error: Option<ObservationError>,
    pub was_retry: bool,
    pub latency: LatencyBreakdown,
    pub tokens: TokenStats,
    pub stream: Option<StreamMetrics>,
}
```

### 5.2 错误分类要求

`ObservationError` 需要支持清晰、可扩展的错误分类，至少包括：

- `Timeout`
- `RateLimited`
- `Upstream5xx`
- `Upstream4xx`
- `ConnectionFailure`
- `EmptyResponse`
- `TruncatedStream`
- `InvalidResponse`
- `Other { code, message }`

并需要满足以下原则：

- 客户端主动取消请求不应与上游故障等价处理。
- 鉴权失败、参数错误等非端点质量问题，不应对质量分造成同等级惩罚。
- 错误分类应支持宿主自定义映射。

### 5.3 延迟与 Token 信息

建议提供：

```rust
pub struct LatencyBreakdown {
    pub total_ms: u64,
    pub ttft_ms: Option<u64>,
}

pub struct TokenStats {
    pub input: u64,
    pub output: u64,
}
```

### 5.4 流式质量观测

对于流式响应，希望支持如下结构：

```rust
pub struct StreamMetrics {
    pub ttft_ms: u64,
    pub tokens_per_second: Option<f64>,
    pub max_inter_chunk_ms: Option<u64>,
    pub chunk_count: u64,
    pub completed_normally: bool,
    pub stream_broken: bool,
}
```

其中需要明确：

- `stream_broken = true` 仅表示上游流异常中断，不包含客户端主动断开。
- `tokens_per_second` 允许为空，因为有些宿主无法稳定估算。
- 非流式请求可以不传 `stream`。

## 6. 输出能力要求

`latch-score` 的输出需要同时满足后台展示和路由消费两类场景。

### 6.1 单 endpoint 评分结果

建议输出：

```rust
pub struct EndpointScore {
    pub endpoint_id: String,
    pub pool_id: String,
    pub score: f64,
    pub tier: ScoreTier,
    pub observation_count: usize,
    pub breakdown: ScoreBreakdown,
    pub excluded: bool,
    pub exclusion_reason: Option<String>,
}
```

`ScoreTier` 枚举值（与代码实现一致）：

```rust
pub enum ScoreTier {
    Poor,   // < 40
    Bronze, // 40–69
    Silver, // 70–89
    Gold,   // >= 90
}
```

### 6.2 pool 排名结果

建议输出：

```rust
pub struct PoolRanking {
    pub pool_id: String,
    pub ranked_endpoints: Vec<EndpointScore>,
    pub recommended: Option<EndpointScore>,
    pub recommended_fallback: Option<EndpointScore>,
    pub excluded_endpoints: Vec<EndpointScore>,
}
```

### 6.3 供路由层消费的反馈结构

建议输出：

```rust
pub struct PoolFeedback {
    pub endpoint_scores: Vec<(String, f64)>,
    pub recent_failures: Vec<(String, u32)>,
    pub penalty_per_failure: f64,
}
```

要求：

- 评分值既可读，也适合被路由层消费。
- 输出应稳定、可序列化。
- 支持按 endpoint 查询与按 pool 查询。

## 7. 评分维度要求

建议评分至少由以下四个维度构成。

### 7.1 可用性

衡量内容包括：

- 成功率
- 最近失败率
- 超时率
- 流断裂率
- 空响应率

这是最重要的评分维度之一，权重应最高。

### 7.2 延迟

衡量内容包括：

- 总延迟
- TTFT
- inter-chunk 最大间隔
- 请求尾部异常拖长情况

### 7.3 响应质量

衡量内容包括：

- 空响应
- 截断响应
- 明显异常输出
- 流是否正常收尾

### 7.4 成本

成本维度应为可选项。

原因是：

- 某些宿主只想做纯质量排名，不愿把价格引入评分。
- 某些宿主希望将价格作为轻权重，避免单纯选择最稳定但过贵的端点。

因此成本信号应设计为可配置的附加维度，而不是强制维度。

## 8. 算法与行为要求

### 8.1 滚动窗口

应支持仅基于最近 $N$ 条观测样本进行评分，避免过久历史污染当前判断。

### 8.2 时间衰减

应支持定期衰减旧观测的影响，避免历史故障永久压低分数。

### 8.3 冷启动处理

对于样本不足的 endpoint，应支持：

- baseline score
- 最小观测数门槛
- 不足样本时降低排名置信度

### 8.4 排除机制

当 endpoint 出现严重异常时，应允许短期排除：

- 连续超时
- 流连续断裂
- 近期错误率显著超阈值

并输出可解释的排除原因。

### 8.5 可解释性

最终得分必须支持分项拆解，至少应输出：

- availability
- latency
- quality
- cost
- penalty

宿主侧后台需要能直接展示这些明细。

## 9. 配置能力要求

`latch-score` 需要提供清晰的配置结构，例如：

```rust
pub struct ScoreConfig {
    pub window_size: usize,
    pub decay_period_secs: u64,
    pub baseline_score: f64,
    pub availability_weight: f64,
    pub latency_weight: f64,
    pub quality_weight: f64,
    pub cost_weight: f64,
    pub good_ttft_ms: u64,
    pub acceptable_ttft_ms: u64,
    pub good_tps: f64,
    pub max_error_rate: f64,
    pub max_truncation_rate: f64,
    pub max_empty_response_rate: f64,
}
```

要求：

- 所有关键阈值和权重都可配置。
- 提供 `default()` 以便宿主快速接入。
- 保持向后兼容，避免频繁破坏宿主配置。

## 10. API 设计要求

建议 `latch-score` 暴露如下最小 API：

```rust
impl ScoringEngine {
    pub fn new(config: ScoreConfig) -> Self;
    pub fn observe(&mut self, obs: RequestObservation);
    pub fn decay(&mut self);
    pub fn get_score(&self, endpoint_id: &str) -> Option<EndpointScore>;
    pub fn rank_pool(&self, pool_id: &str) -> PoolRanking;
    pub fn rank_all(&self) -> Vec<PoolRanking>;
    pub fn export_snapshot(&self) -> ScoreSnapshot;
    pub fn restore_snapshot(&mut self, snapshot: ScoreSnapshot);
}
```

补充要求：

- `observe()` 必须是轻量操作，适合在请求结束路径中调用。
- `rank_pool()` 和 `get_score()` 必须可直接用于 admin API。
- `export_snapshot()` / `restore_snapshot()` 便于宿主做持久化或跨实例同步。

## 11. 与宿主系统的集成要求

以 ParaRouter 作为第一方宿主时，推荐集成方式如下：

- ParaRouter 在 hooks 和流式 wrapper 中采集观测数据。
- ParaRouter 负责把宿主内部类型转换成 `RequestObservation`。
- ParaRouter 在后台展示 `EndpointScore`、`PoolRanking`。
- ParaRouter 或 `latch-router` 在路由时消费 `PoolFeedback`。

但 `latch-score` 本身不应依赖：

- ParaRouter 的用户 ID、API key、套餐、预算等字段。
- UniGateway 的原始 SDK 类型。
- 某个特定网关的运行时上下文。

## 12. 宿主业务信号与质量信号的分离

需要明确区分：

- 质量信号：快不快、稳不稳、是否易断流、是否经常超时。
- 业务信号：用户预算、用户套餐、模型授权、租户规则、地区隔离、请求类型偏好。

`latch-score` 只负责第一类。
第二类应交由宿主系统或 `latch-router` 做最终综合决策。

这条边界非常重要，否则评分引擎会被具体业务逻辑污染，失去复用价值。

## 13. 阶段性交付状态

### Phase 1 ✅ 已实现

- 定义通用观测结构（RequestObservation 等）。
- 实现 `ScoringEngine`、`ScoreConfig`、`observe()`、`get_score()`。
- 支持 availability 和 latency 两个核心维度。

### Phase 2 ✅ 已实现

- 增加 stream metrics 支持（StreamMetrics）。
- 支持 TTFT、TPS、stream broken、empty response、truncation 等质量指标。
- 支持分项 breakdown 与 tier。
- 排除机制已实现（连续超时 / 连续流断裂 → 短期剔除，含排除原因）。

### Phase 3 ✅ 已实现

- 增加 `rank_pool()`、`rank_all()` 与 `PoolFeedback`。
- 补齐排除机制与 fallback 推荐（`recommended` / `recommended_fallback`）。

### Phase 4 ✅ 已实现

- 增加 `export_snapshot()` / `restore_snapshot()`。
- 支持宿主持久化与多实例同步。

## 14. 预期结果

完成后，`latch-score` 应能成为一个独立、通用、可复用的质量评分引擎，为宿主系统提供以下能力：

- 对模型端点进行实时质量评分。
- 对同一 pool 内的多个端点进行质量排名。
- 为路由层提供稳定、可解释的反馈结构。
- 为后台提供可展示、可审计的评分明细。

其角色不是"业务路由器"，而是"质量判断器"。
宿主系统可以在此基础上再叠加成本、权限、预算和用户策略，形成完整路由闭环。
