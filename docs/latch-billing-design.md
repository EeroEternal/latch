# latch-billing 实现计划

基于 xrouter 代码审查修正后的通用 Token 计费库设计。

---

## 1. 架构总览

一个 crate，纯同步核心：

```
latch-billing          ← 纯同步，零 I/O，运行时无关
├── observation.rs      ← UsageObservation + MeterSet + UsageSource
├── pricing.rs          ← ModelRef + PriceSnapshot + RatingEngine trait
├── rating.rs           ← RatedUsageRecord + RatingResult + RatedLineItem
├── identity.rs         ← BillingSubject + CorrelationIds + UsageEventId
├── quota.rs            ← QuotaAuthorizer / QuotaReservator trait
├── storage.rs          ← ObservationStore / RatedRecordStore trait（同步，内存/文件）
└── export.rs           ← RatedRecordExporter trait

下游应用（如 xrouter）自行实现：
├── 异步缓冲层（BufferedMeteringSink 等）
├── 异步存储实现（Postgres, Redis 等）
└── Quota 实现（Phase 2+）
```

| 原则 | 实现 |
|------|------|
| 同步核心 | latch-billing 所有 trait 和类型不依赖 tokio/async |
| 中性类型 | 无 UniGateway SDK、Axum、Tower、provider SDK 依赖 |
| 追加事件流 | UsageObservation 是 immutable fact，不改写 |
| 事实与派生分离 | observation 不含 cost，rating 生成新对象 |
| Meter 可扩展 | MeterKind 枚举替代硬编码字段 |
| 定价键多维 | ModelRef = billable_model + vendor + region + tier |
| 幂等键含 attempt | UsageEventId 支持 (request_id, attempt_index, provider_id) |
| 操作语义分离 | metering fail-open / quota authorization fail-closed |

---

## 2. 核心领域模型

### 2.1 UsageObservation（原始观测，不可变）

```rust
pub struct UsageObservation {
    pub event_id: UsageEventId,
    pub subject: BillingSubject,
    pub meter_set: MeterSet,
    pub model_ref: ModelRef,
    pub provider_ref: Option<ProviderRef>,
    pub source: UsageSource,
    pub outcome: UsageOutcome,
    pub timing: UsageTiming,
    pub correlation: CorrelationIds,
    /// 可扩展属性：is_fallback, step_type, estimated_reason 等
    pub attributes: Attributes,
}

/// 受约束的属性集合，newtype 封装以强制文档中的约束
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Attributes {
    inner: HashMap<String, String>,
}

impl Attributes {
    pub const MAX_KEY_LEN: usize = 64;
    pub const MAX_VALUE_LEN: usize = 256;

    pub fn new() -> Self {
        Self { inner: HashMap::new() }
    }

    /// 插入属性。key 不能以 `sys.` 开头（library 预留前缀），长度不能超限。
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Result<(), AttributeError> {
        let key = key.into();
        let value = value.into();
        if key.starts_with("sys.") {
            return Err(AttributeError::ReservedPrefix(key));
        }
        if key.len() > Self::MAX_KEY_LEN {
            return Err(AttributeError::KeyTooLong { key, len: key.len() });
        }
        if value.len() > Self::MAX_VALUE_LEN {
            return Err(AttributeError::ValueTooLong { key, len: value.len() });
        }
        self.inner.insert(key, value);
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).map(|s| s.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.inner.iter()
    }
}

pub enum AttributeError {
    ReservedPrefix(String),
    KeyTooLong { key: String, len: usize },
    ValueTooLong { key: String, len: usize },
}
```

/// 用 HashMap 保证同一种 MeterKind 不会出现两次。
/// 构造时通过 `MeterSet::new()` 做去重：相同 key 的 quantity 累加。
pub struct MeterSet {
    pub meters: HashMap<MeterKind, u64>,
}

impl MeterSet {
    pub fn new() -> Self {
        Self { meters: HashMap::new() }
    }

    /// 插入或累加。如果 key 已存在，quantity 会叠加而非覆盖。
    /// 使用 checked_add 防止 u64 溢出，溢出时返回 Overflow 错误。
    pub fn accumulate(&mut self, kind: MeterKind, quantity: u64) -> Result<(), MeterSetError> {
        use std::collections::hash_map::Entry;
        match self.meters.entry(kind) {
            Entry::Occupied(mut e) => {
                let new_val = e.get().checked_add(quantity)
                    .ok_or_else(|| MeterSetError::Overflow(e.key().clone()))?;
                e.insert(new_val);
            }
            Entry::Vacant(e) => {
                e.insert(quantity);
            }
        }
        Ok(())
    }

    pub fn get(&self, kind: &MeterKind) -> u64 {
        self.meters.get(kind).copied().unwrap_or(0)
    }
}

pub enum MeterSetError {
    /// 插入导致 u64 溢出
    Overflow(MeterKind),
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum MeterKind {
    InputTokens,
    OutputTokens,
    CachedInputTokens,
    CachedWriteTokens,
    ReasoningTokens,
    AudioInputTokens,
    AudioOutputTokens,
    ImageCount,
    Custom(String),
}

pub enum UsageSource {
    ProviderReported,
    StreamAccumulated,
    Estimated,
    /// 修正事件。必须携带 `correction_of` 属性指向被修正的原始 UsageEventId。
    Corrected { correction_of: UsageEventId },
}

/// 请求的最终状态
pub enum UsageOutcome {
    Success,
    Error { code: String },
    Timeout,
    Unknown,
}

/// 观测时间信息
pub struct UsageTiming {
    /// 观测产生的时间
    pub observed_at: DateTime<Utc>,
    /// 请求完成时间（流式结束时填充）
    pub completed_at: Option<DateTime<Utc>>,
}

/// 货币代码，用 newtype 包装以保证类型安全
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct CurrencyCode(String);

impl CurrencyCode {
    pub fn usd() -> Self { CurrencyCode("USD".into()) }
    pub fn cny() -> Self { CurrencyCode("CNY".into()) }
    pub fn eur() -> Self { CurrencyCode("EUR".into()) }

    pub fn as_str(&self) -> &str { &self.0 }
}

impl std::fmt::Display for CurrencyCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for CurrencyCode {
    fn as_ref(&self) -> &str { &self.0 }
}

impl std::str::FromStr for CurrencyCode {
    type Err = CurrencyCodeError;

    /// 仅接受 3 位 ASCII 大写字母（ISO 4217 格式）。
    /// 不做完整的 ISO 4217 枚举校验，避免引入 weight 级依赖。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 3 && s.chars().all(|c| c.is_ascii_uppercase()) {
            Ok(CurrencyCode(s.to_string()))
        } else {
            Err(CurrencyCodeError::Invalid(s.to_string()))
        }
    }
}

pub enum CurrencyCodeError {
    Invalid(String),
}
```

### 2.2 BillingSubject（计费主体）

```rust
pub struct BillingSubject {
    pub tenant_id: Option<String>,
    pub org_id: Option<String>,
    pub project_id: Option<String>,
    pub api_key_id: Option<String>,
    pub end_user_id: Option<String>,
    pub feature: Option<String>,
}
```

### 2.3 关联与幂等

```rust
/// 幂等键。由 `UsageEventId::from_attempt()` 构造，保证生成规则集中在一处。
pub struct UsageEventId {
    pub idempotency_key: String,
}

impl UsageEventId {
    /// 从 request_id、attempt_index、provider_id 拼接幂等键。
    /// 对于需要更精确幂等语义的场景（如 step 级计费），使用 `UsageEventIdBuilder`。
    pub fn from_attempt(
        request_id: &str,
        attempt_index: i32,
        provider_id: &str,
    ) -> Result<Self, UsageEventIdError> {
        if attempt_index < 0 {
            return Err(UsageEventIdError::InvalidAttemptIndex(attempt_index));
        }
        Ok(Self {
            idempotency_key: format!(
                "{}:{}:{}",
                request_id, attempt_index, provider_id
            ),
        })
    }

    /// 用于非 provider 场景（如 client-side estimation），
    /// 调用方需自行保证唯一性。
    pub fn from_raw(key: impl Into<String>) -> Self {
        Self {
            idempotency_key: key.into(),
        }
    }
}

/// 需要更精确幂等语义时使用（如 step 级计费、多阶段 attempt）
pub struct UsageEventIdBuilder {
    request_id: String,
    attempt_index: i32,
    provider_id: String,
    step_id: Option<String>,
    phase: Option<String>,
}

impl UsageEventIdBuilder {
    pub fn new(request_id: &str, attempt_index: i32, provider_id: &str) -> Self {
        Self {
            request_id: request_id.into(),
            attempt_index,
            provider_id: provider_id.into(),
            step_id: None,
            phase: None,
        }
    }

    pub fn step_id(mut self, id: impl Into<String>) -> Self {
        self.step_id = Some(id.into());
        self
    }

    pub fn phase(mut self, p: impl Into<String>) -> Self {
        self.phase = Some(p.into());
        self
    }

    pub fn build(self) -> Result<UsageEventId, UsageEventIdError> {
        if self.attempt_index < 0 {
            return Err(UsageEventIdError::InvalidAttemptIndex(self.attempt_index));
        }
        let mut key = format!(
            "{}:{}:{}",
            self.request_id, self.attempt_index, self.provider_id
        );
        if let Some(ref s) = self.step_id {
            key.push(':');
            key.push_str(s);
        }
        if let Some(ref p) = self.phase {
            key.push(':');
            key.push_str(p);
        }
        Ok(UsageEventId { idempotency_key: key })
    }
}

pub enum UsageEventIdError {
    InvalidAttemptIndex(i32),
}

pub struct CorrelationIds {
    pub request_id: Option<String>,
    pub trace_id: Option<String>,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub run_id: Option<String>,
    pub step_id: Option<String>,
    pub attempt_index: Option<i32>,
}
```

**IdempotencyKey 生成规则**：`from_attempt(request_id, attempt_index, provider_id)` 为基础构造，返回 `Result`，不 panic。需要 step 级或 phase 级精确幂等时使用 `UsageEventIdBuilder`。单独 `request_id` 不够——回退链和多次 attempt 会被错误折叠。

### 2.4 ModelRef（定价键）

```rust
pub struct ModelRef {
    pub billable_model: String,
    pub vendor: Option<String>,
    pub region: Option<String>,
    pub tier: Option<String>,
}

pub struct ProviderRef {
    pub provider_id: String,
}
```

### 2.5 RatedUsageRecord（评分结果）

```rust
pub struct RatedUsageRecord {
    /// 本条 rated record 的唯一标识（独立于 observation.event_id）。
    pub rated_record_id: String,
    /// 关联的原始观测
    pub observation: UsageObservation,
    pub rating: RatingResult,
    /// 当这是对之前 rated record 的修正时，指向被取代的 rated record 的 rated_record_id。
    pub supersedes: Option<String>,
}

pub struct RatingResult {
    pub line_items: Vec<RatedLineItem>,
    pub total_cost: Decimal,
    pub currency: CurrencyCode,
    pub price_snapshot_id: String,
    pub rated_at: DateTime<Utc>,
}

pub struct RatedLineItem {
    pub meter_kind: MeterKind,
    pub quantity: u64,
    pub unit_price: Decimal,
    pub subtotal: Decimal,
}
```

---

## 3. 定价与评分 Trait

### 3.1 定价快照（Push 模式）

`PricingSource` trait 保留在 latch-billing 中，但仅用于纯内存/文件场景（如 `TomlPricingSource`）。

**当定价数据在 DB 或远程服务时，推荐 push 模式**：调用方（如 xrouter adapter）先用 sqlx 等异步工具查好 `PriceSnapshot`，再传给同步的 `RatingEngine::rate()`。这样 PricingSource 不需要 async，xrouter 也不因为 latch-billing 引入额外的 DB 依赖。

```rust
/// 纯同步定价源（仅用于文件/内存场景）
pub trait PricingSource: Send + Sync {
    fn resolve_snapshot(
        &self,
        model_ref: &ModelRef,
        provider_ref: Option<&ProviderRef>,
    ) -> Result<PriceSnapshot, PricingError>;
}

pub struct PriceSnapshot {
    pub snapshot_id: String,
    pub model_ref: ModelRef,
    pub currency: CurrencyCode,
    pub prices: HashMap<MeterKind, MeterPrice>,
    pub tiers: Option<TierConfig>,
    pub effective_from: DateTime<Utc>,
    pub effective_until: Option<DateTime<Utc>>,
}

/// 阶梯定价配置（Phase 3 实现）
///
/// 按指定基线 meter 的累计使用量自动切换价格乘数或绝对价格。
/// 累计范围由 `accumulation_scope` 决定（per-tenant 生命周期 / per-billing-period）。
/// 累计值由调用方在调用 `RatingEngine::rate()` 时传入，或由 PricingSource 维护。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// 按哪个 meter 累计（如 InputTokens、OutputTokens 或总量）
    pub baseline_meter: TierBaseline,
    /// 累计量由谁提供
    pub accumulation_scope: AccumulationScope,
    pub boundaries: Vec<TierBoundary>,
}

pub enum TierBaseline {
    /// 按单个 meter 累计
    Meter(MeterKind),
    /// 按所有 meter 的 token 总量累计
    TotalTokens,
}

pub enum AccumulationScope {
    /// 调用方在 rate() 时通过 RatingContext 传入累计值（当前唯一可执行路径）
    CallerProvided,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierBoundary {
    /// 阈值（含），单位 MTok。0 表示从 0 开始的第一档。
    pub up_to_mtok: u64,
    /// 价格乘数（相对于 base_price）
    pub price_multiplier: Option<Decimal>,
    /// 或直接指定绝对价格（与 multiplier 二选一）
    pub absolute_price_per_mtok: Option<Decimal>,
}

pub struct MeterPrice {
    pub unit_price: Decimal,      // per 1 unit of meter
    pub unit_display: String,     // "1M tokens"
}
```

### 3.2 RatingEngine

`rate()` 需要 `RatingContext` 来承载累计使用量等信息，否则 `TierConfig` 的阶梯定价无法获取阈值判断所需的累计值。

```rust
pub trait RatingEngine: Send + Sync {
    fn rate(
        &self,
        observation: &UsageObservation,
        snapshot: &PriceSnapshot,
        context: &RatingContext,
    ) -> Result<RatingResult, RatingError>;
}

/// 评分上下文：承载累计使用量、账期等定价判定所需的外部信息
pub struct RatingContext {
    /// 按 `TierConfig.baseline_meter` 维度的累计使用量（MTok）。
    pub cumulative_baseline_usage_mtok: u64,
    /// 计费周期标识（如 "2025-05"），用于账期级 tier 重置判定
    pub billing_period: Option<String>,
    /// 租户 scope（用于 tenant 级累计区分）
    pub tenant_scope: Option<String>,
}
```

`PricingSource` 负责查找价格快照，`RatingEngine` 负责用快照 + 上下文计算 observation 的成本。分离后方便缓存和单测。

### 3.3 计费 Pipeline 闭环

xrouter Phase 1 主路径：**事实优先落盘，定价独立完成**。

```
xrouter adapter:
  1. 构建 UsageObservation
  2. MeteringSink::try_record(observation)  ← observation 优先落盘，fail-open
  3. 异步查 PriceSnapshot（sqlx，xrouter 已有数据源）
  4. 同步调 RatingEngine::rate(observation, snapshot, context)
  5. 评分成功 → 合并为 RatedUsageRecord（含 rated_record_id）
              → RatedRecordSink::try_record_rated(record)
  6. 评分失败（pricing 不可用）→ log + metric
              → observation 已落盘，可后续回补 billing
```

**关键规则**：pricing 失败不吞事实。即使 PriceSnapshot 查询失败或 rate() 返回 error，
step 2 已将 observation 持久化，后续可通过异步 worker 回补 rated record。

```rust
/// observation 入队（事实优先路径）
pub trait MeteringSink: Send + Sync {
    fn try_record(&self, observation: UsageObservation) -> Result<RecordOutcome, RecordError>;
}

/// 已评分记录入队（定价成功后续路径）
pub trait RatedRecordSink: Send + Sync {
    fn try_record_rated(&self, record: RatedUsageRecord) -> Result<RecordOutcome, RecordError>;
}
```

两条路径共存，xrouter 两种都用：observation 总是优先写，rated record 定价成功后写。

### 3.4 Pipeline 一致性模型

| 规则 | 说明 |
|------|------|
| raw observation 是一级事实源 | observation 必须先于 rated record 持久化；pricing 失败不吞事实 |
| rated record 总是基于某个 observation | `RatedUsageRecord.observation.event_id` 指向原始观测；`rated_record_id` 独立标识 |
| observation 写入失败不回退 | observation 成功后，后续 rating 失败只记录 log，observation 不掉 |
| 一条 observation 只允许一条 active rated record | `supersedes` 链末端的 rated record 为 active |
| correction 到达必须生成新 rated record | 新 rated record 的 `supersedes` 指向旧 rated record 的 `rated_record_id` |

### 3.5 rated_record_id 生成契约

`rated_record_id` 由接入侧（xrouter adapter）生成，格式固定为：

```
rated_record_id = "{observation.event_id}:v{rating_revision}"
```

- `rating_revision` 从 1 开始，每次生成新的 active rated record 时递增
- 同一 observation 首次评分为 `:v1`，correction 到来时生成 `:v2` 并 supersede `:v1`
- 幂等重试：相同 observation + 相同 RatingContext 的重复 rate() 结果复用同一 rated_record_id
- `supersedes` 只能指向同一 observation 链内的 rated_record_id
- 此格式让存储层既可以用 rated_record_id 做主键去重，也可以解析 event_id 做关联查询

---

## 4. 存储 Trait

### 4.1 同步存储（latch-billing，仅内存/文件）

```rust
/// 追加原始观测（不可变事实）—— 同步版本
///
/// 幂等约定：同一个 `UsageEventId` 重复 append 时，统一外部语义：
/// - 视为成功，不返回错误
/// - 副作用只生效一次（不重复计数）
/// - 返回 `StoreResult::AlreadyExists` 供调用方观测
pub trait ObservationStore: Send + Sync {
    fn append_observation(&self, observation: UsageObservation) -> Result<StoreResult, StoreError>;
}

/// 追加评分后的记录 —— 同步版本
pub trait RatedRecordStore: Send + Sync {
    fn append_rated_record(&self, record: RatedUsageRecord) -> Result<StoreResult, StoreError>;
}

pub enum StoreResult {
    /// 首次写入
    Appended,
    /// 幂等重复：已存在，未重复写入
    AlreadyExists,
}
```

### 4.2 异步存储（由下游应用实现）

下游应用（如 xrouter）应自行实现异步存储层，基于 latch-billing 的同步 trait。
这样可以干净地实现 `PgObservationStore` / `PgRatedRecordStore`，无需 `block_on`。

```rust
/// 异步 observation 存储（下游应用实现）
#[async_trait]
pub trait AsyncObservationStore: Send + Sync {
    async fn append_observation(&self, observation: UsageObservation) -> Result<StoreResult, StoreError>;
}

/// 异步 rated record 存储（下游应用实现）
#[async_trait]
pub trait AsyncRatedRecordStore: Send + Sync {
    async fn append_rated_record(&self, record: RatedUsageRecord) -> Result<StoreResult, StoreError>;
}
```

### 4.3 存储幂等约定

**统一外部语义**：同一个 `UsageEventId` 重复写入时，无论底层实现如何，外部行为必须统一为：
- 视为成功（不返回错误）
- 副作用只生效一次（不重复计数、不重复入队）
- 返回 `StoreResult::AlreadyExists` 供调用方观测，但不改变幂等语义

### 4.4 接入模式

**三种接入模式**：

| 模式 | 写入内容 | 适用场景 |
|------|---------|---------|
| 1 | 只写 observation | 离线异步计价，事实不丢（需要 AsyncPricingSource，暂未实现） |
| 2 | 只写 rated record | 内联计价（不需要原始数据） |
| 3 | observation + rated record | xrouter Phase 1 主路径：事实优先 + 定价独立 |

> xrouter Phase 1 走模式 3：adapter 先 try_record(observation)，再 inline rate + try_record_rated(record)。
> pricing 失败时 observation 已在模式 3 step 1 落盘，不对事实层产生破窗。

---

## 5. 异步缓冲（由下游应用实现）

下游应用需要自行实现异步缓冲层。以下是参考设计：

两个 sink 实现，对应两条路径：

```rust
/// observation 入队（辅助路径）
pub trait MeteringSink: Send + Sync {
    fn try_record(&self, observation: UsageObservation) -> Result<RecordOutcome, RecordError>;
}

/// 已评分记录入队（xrouter 主路径）
pub trait RatedRecordSink: Send + Sync {
    fn try_record_rated(&self, record: RatedUsageRecord) -> Result<RecordOutcome, RecordError>;
}

pub enum RecordOutcome {
    /// 成功入队，将被异步持久化
    Enqueued,
    /// fail-open：channel 满，已触发 on_drop 回调，记录被丢弃
    DroppedFailOpen,
}
```

### BufferedRatedRecordSink（xrouter 主路径）

```
try_record_rated(RatedUsageRecord)
    │
    ▼
bounded mpsc channel (capacity N)
    │
    ├── 成功 enqueue → 返回 Ok(Enqueued)
    └── channel 满 → 触发 on_drop callback（tracing::warn! + metrics counter）
                     → 返回 Ok(DroppedFailOpen)（fail-open，不阻塞调用方）
    │
    ▼
flush worker
    ├── batch dequeue（最多 batch_size 条或 interval 到期）
    ├── 调用 AsyncRatedRecordStore::append_rated_record() → PgRatedRecordStore
    └── 失败时 retry（指数退避，最多 retry_max 次）
                     → 超过重试次数后 on_drop callback
```

### BufferedMeteringSink（辅助路径）

```
try_record(UsageObservation)
    │
    ▼
bounded mpsc channel (capacity N)
    │
    ├── 成功 enqueue → 返回 Ok(Enqueued)
    └── channel 满 → 返回 Ok(DroppedFailOpen)
    │
    ▼
flush worker
    ├── batch dequeue
    ├── 调用 AsyncObservationStore::append_observation() → PgObservationStore
    └── retry（同上）
```

- **fail-open**：channel 满或 flush 最终失败时丢弃记录，通过 `on_drop` callback 记录
- **on_drop callback**：`Box<dyn Fn(&DropContext) + Send + Sync>`，默认实现为 `tracing::warn!` + 内部 counter。生产环境必须用 structured logging 记录丢弃事件
- **DropContext**：
  ```rust
  pub struct DropContext {
      /// 丢弃的记录类型
      pub kind: DropKind,
      /// observation 的 event_id 或 rated record 的 rated_record_id
      pub record_id: String,
      /// 丢弃原因
      pub reason: DropReason,
      /// 租户/主体提示（可选）
      pub subject_hint: Option<String>,
  }

  pub enum DropKind {
      Observation,
      RatedRecord,
  }

  pub enum DropReason {
      /// channel 满
      ChannelFull,
      /// flush 重试耗尽
      FlushFailed { attempts: u32 },
  }
  ```
- **graceful shutdown**：tokio graceful shutdown 时 drain 剩余 buffer
- 配置项：`capacity`, `batch_size`, `flush_interval_ms`, `retry_max`, `retry_backoff_ms`

---

## 6. Quota 子系统（独立 seam）

> **注意**：以下请求/响应类型为 placeholder skeleton，完整定义属于 Phase 2。
> Phase 1 只要求 trait 签名存在，具体结构体可在 Phase 2 展开。

```rust
// Phase 2 展开的 placeholder 类型：
// struct QuotaRequest { pub subject: BillingSubject, pub requested: UsageAmount, ... }
// enum QuotaDecision { Allowed { remaining: u64 }, Denied { reason: String }, ... }
// struct ReservationRequest { ... }
// struct Reservation { pub id: String, ... }
// struct UsageAmount { pub meters: HashMap<MeterKind, u64> }

/// 同步判定：此请求可否放行？（fail-closed）
pub trait QuotaAuthorizer: Send + Sync {
    fn authorize(&self, request: &QuotaRequest) -> Result<QuotaDecision, QuotaError>;
}

/// 预留/提交/退还（Phase 2）
pub trait QuotaReservator: Send + Sync {
    fn reserve(&self, reservation: &ReservationRequest) -> Result<Reservation, QuotaError>;
    fn commit(&self, reservation_id: &str, amount: &UsageAmount) -> Result<(), QuotaError>;
    fn refund(&self, reservation_id: &str, unused: &UsageAmount) -> Result<(), QuotaError>;
}
```

**关键区别**：

| 操作 | 语义 | 失败策略 | 实现阶段 |
|------|------|---------|---------|
| `MeteringSink::try_record` | 追加事件 | fail-open | Phase 2 |
| `RatedRecordSink::try_record_rated` | 追加评分记录 | fail-open | Phase 2 |
| `AsyncObservationStore::append_observation` | 持久化 | fail-open + retry | Phase 2 |
| `AsyncRatedRecordStore::append_rated_record` | 持久化 | fail-open + retry | Phase 2 |
| `QuotaAuthorizer::authorize` | 授权判定 | fail-closed | Phase 2 |
| `QuotaReservator::reserve/commit` | 预留结算 | fail-closed | Phase 2 |

---

## 7. Export Hook

```rust
pub trait RatedRecordExporter: Send + Sync {
    fn export(&self, record: &RatedUsageRecord) -> Result<(), ExportError>;
}
```

消费已评分的记录（不是裸 observation），可接入 Stripe、OpenMeter、Kafka 等。

---

## 8. xrouter 接入点

不在 latch-billing 里写 provider adapter。xrouter 现有代码加一个薄映射层。

**xrouter 主路径**：Phase 1 起 adapter 先写 observation → 再内联评分 → 再写 rated record。pricing 失败不吞事实。

**关键约束**：
- xrouter package 依赖 `latch-billing`、`sqlx`
- xrouter 业务模块和 adapter 只依赖 `latch-billing`
- xrouter 自行实现异步缓冲层（BufferedMeteringSink 等）

```
xrouter/Cargo.toml
  ├── latch-billing              ← 类型 + trait（业务模块 + adapter 依赖）
  └── sqlx                       ← 查定价 + 写 DB（已有依赖）

xrouter 启动时（main.rs / app setup）
  ├── 构造自有的 BufferedMeteringSink<PgObservationStore>
  ├── 构造自有的 BufferedRatedRecordSink<PgRatedRecordStore>
  └── 以 Arc<dyn MeteringSink> + Arc<dyn RatedRecordSink> 注入到 gateway state
```

```
xrouter/src/adapter/generic/base.rs  ← 已有 Usage 提取 + stream 兜底
xrouter/src/db/models.rs             ← RequestLog + with_context
xrouter/src/request_context.rs       ← request_id / attempt_index / provider_id
        │
        ▼
xrouter/src/tokenbill_adapter.rs     ← 新增，薄映射（只依赖 latch-billing）
        │
        ▼
Arc<dyn RatedRecordSink>              ← trait object，由 xrouter 自行实现
```

adapter 职责（纯类型映射，不涉及 I/O，不持有 DB 连接）：

1. `RequestExecutionContext` + `Usage` → `UsageObservation`（含 `MeterSet` 构建）
2. 调用 `UsageEventId::from_attempt(request_id, attempt_index, provider_id)` 生成幂等键
3. 通过 `Arc<dyn MeteringSink>::try_record()` 优先持久化 observation（fact first）
4. 由 adapter 外层的 async 上下文查好 `PriceSnapshot`（xrouter DB 已有的 `provider_prices`）
5. 同步 `RatingEngine::rate(observation, snapshot, context)`
6. 评分成功 → 生成 `rated_record_id`（`{event_id}:v{revision}`）→ `RatedRecordSink::try_record_rated()`
7. 评分失败 → `tracing::warn!` + metric counter，observation 已在 step 3 落盘

**依赖方向总结**：

```
xrouter ──→ latch-billing （类型 + trait）
xrouter ──→ sqlx （已有依赖，非通过 latch-billing 引入）
xrouter 自行实现 ──→ tokio / postgres / redis （异步基础设施）
```

---

## 9. 实施阶段

### Phase 1 — 最小正确内核（latch-billing）

| 任务 | 内容 |
|------|------|
| 1.1 | `observation.rs`：UsageObservation, MeterSet, MeterKind, UsageSource |
| 1.2 | `identity.rs`：BillingSubject, UsageEventId, CorrelationIds |
| 1.3 | `pricing.rs`：ModelRef, ProviderRef, PriceSnapshot, MeterPrice, PricingSource trait |
| 1.4 | `rating.rs`：RatedUsageRecord, RatingResult, RatedLineItem, RatingEngine trait |
| 1.5 | `storage.rs`：ObservationStore trait, RatedRecordStore trait, StoreError |
| 1.6 | `quota.rs`：QuotaAuthorizer trait, QuotaReservator trait, QuotaError（seam only） |
| 1.7 | `export.rs`：RatedRecordExporter trait |
| 1.8 | `lib.rs`：re-export + 文档 |
| 1.9 | 单元测试：meter 序列化、Decimal 精度、幂等键生成 |

**依赖**：`rust_decimal`, `chrono`, `serde`, `serde_json`

### Phase 2 — 异步基础设施（由下游应用实现）

| 任务 | 内容 | 实现位置 |
|------|------|----------|
| 2.1 | `MeteringSink` trait + `RatedRecordSink` trait（latch-billing 中已定义） | latch-billing |
| 2.2 | `BufferedMeteringSink` 实现 + `BufferedRatedRecordSink` 实现 | 下游应用（如 xrouter） |
| 2.3 | `AsyncObservationStore` / `AsyncRatedRecordStore` trait 定义 | 下游应用 |
| 2.4 | flush worker：batch dequeue + retry + graceful shutdown | 下游应用 |
| 2.5 | `PgObservationStore`：Postgres 实现 AsyncObservationStore | 下游应用 |
| 2.6 | `PgRatedRecordStore`：Postgres 实现 AsyncRatedRecordStore | 下游应用 |
| 2.7 | 配置 struct：BufferConfig（capacity, batch_size, flush_interval, retry） | 下游应用 |
| 2.8 | 集成测试：buffer overflow fail-open、shutdown drain、retry recovery | 下游应用 |

### Phase 3 — Pricing 实现

| 任务 | 内容 |
|------|------|
| 3.1 | `TomlPricingSource`：从 TOML 文件加载 PriceSnapshot |
| 3.2 | `DefaultRatingEngine`：meter × unit_price + tier 折扣 |
| 3.3 | TierConfig：按累计 MTok 阈值切换 multiplier |

### Phase 4 — Quota 实现（由下游应用实现）

> **注意**：Quota 实现涉及 Redis I/O，应由下游应用（如 xrouter）自行实现。
> `latch-billing` 只定义 `QuotaAuthorizer` 和 `QuotaReservator` trait。

| 任务 | 内容 | 实现位置 |
|------|------|----------|
| 4.1 | `RedisQuotaAuthorizer`：基于 Redis 的限额同步判定 | 下游应用 |
| 4.2 | `RedisQuotaReservator`：reserve/commit/refund 实现 | 下游应用 |

### Phase 5 — Export 实现（由下游应用实现）

> **注意**：Export 实现涉及网络 I/O，应由下游应用自行实现。
> `latch-billing` 只定义 `RatedRecordExporter` trait。

| 任务 | 内容 | 实现位置 |
|------|------|----------|
| 5.1 | `OpenMeterExporter` | 下游应用 |
| 5.2 | Stripe Exporter | 下游应用 |
| 5.3 | Kafka Exporter | 下游应用 |

### Phase 6 — xrouter 适配

| 任务 | 内容 |
|------|------|
| 6.1 | `xrouter/src/tokenbill_adapter.rs`：纯类型映射层，仅依赖 latch-billing |
| 6.2 | xrouter `Cargo.toml` 添加 `latch-billing` + `sqlx` 依赖 |
| 6.3 | app setup 层构造 `BufferedMeteringSink` + `BufferedRatedRecordSink`，注入两路 trait object |
| 6.4 | adapter 流程：`try_record(observation)` → 查 `PriceSnapshot` → `rate()` → `try_record_rated(record)` |
| 6.5 | pricing 失败路径：log + metric + 保留 observation（已落盘，可回补） |

---

## 10. 设计决策记录

### MeterSet 用 HashMap 而非 Vec

`HashMap<MeterKind, u64>` 从结构上杜绝重复 MeterKind。`MeterSet::insert()` 对相同 key 做 quantity 累加，调用方无需手动去重。`MeterKind` 需要 derive `Hash + Eq + PartialEq`。

### UsageEventId 构造方法集中

`UsageEventId::from_attempt()` 是推荐构造方式，拼接逻辑不散落到 adapter 层。保留 `from_raw()` 用于非 provider 场景。

### 定价快照 Push 模式

`PricingSource` trait 保留用于纯内存/文件场景。DB 或远程服务的定价数据由调用方先异步查好 `PriceSnapshot`，再传给同步 `RatingEngine::rate()`。这样 latch-billing 保持 sync，调用方也不因此引入额外 DB 依赖。

### fail-open 必须有可观测性

`BufferedMeteringSink` 和 `BufferedRatedRecordSink` 的 `on_drop` callback 接收 `&DropContext`（含 `DropKind`、`record_id`、`DropReason`、`subject_hint`）。默认实现为 `tracing::warn!` + 内部 counter。生产环境必须用 structured logging 感知丢弃事件。

### Pipeline 与定价失败契约

xrouter Phase 1：observation 优先落盘（`MeteringSink`）→ inline rate → rated record（`RatedRecordSink`）。pricing 失败不吞事实：即使 `PriceSnapshot` 查询失败或 `rate()` 返回 error，observation 已持久化，rated record 可延后回补。

### UsageObservation 含 outcome + attributes + Corrected 语义

`UsageObservation` 新增 `outcome: UsageOutcome`（Success/Error/Timeout/Unknown）和 `attributes: HashMap<String, String>`（is_fallback, step_type, estimated_reason 等）。`UsageSource::Corrected` 携带 `correction_of: UsageEventId`，修正事件有明确的修正目标。

### Correction 覆盖规则（含确定性排序）

raw observation 永远 append-only；correction 是独立 observation。最新版本判定：主排序键 `observed_at`，次排序键 `event_id`（字典序），确保时钟偏差或同时间戳写入时结果确定且可复现。`RatedUsageRecord.supersedes` 指向旧 rated record 的 `rated_record_id`，correction 到达必须生成新 rated record。

### Rated record 独立身份（rated_record_id）

一条 observation 只允许一条 active rated record，但历史上可存在多条 superseded rated record。`rated_record_id` 让每条 rated record 在持久化层和审计回放中都有独立稳定身份，`supersedes` 链清晰可追溯。

### 存储幂等统一语义

重复 `UsageEventId` 写入时外部行为统一：视为成功 + 副作用只生效一次 + 返回 `StoreResult::AlreadyExists` 供观测。store trait 签名统一为 `Result<StoreResult, StoreError>`，彻底移除旧的 `DuplicateEvent`/ignore 二选一表述。

### Attributes 从文档约束升级为类型约束

`Attributes` newtype 封装 `HashMap`，`insert()` 强制校验 key 前缀（`sys.` 预留）、长度上限（64B/256B）。"禁止 prompt/response 原文"目前为文档约束（运行时检测全文内容在 API 层代价过高），后续可扩展 content-hash 校验。`#[serde(transparent)]` 确保序列化为纯 map 形态而非 `{"inner": ...}`。

### HashMap vs BTreeMap 取舍

`MeterSet` 内部用 `HashMap`——`accumulate()` 的 O(1) 更新直觉更符合 hot path，确定性输出不是 core 的首要目标。如果需要确定性序列化，下游可自行转换为 `BTreeMap`。

### try_record 返回 RecordOutcome

`MeteringSink::try_record()` 返回 `Result<RecordOutcome, RecordError>`，其中 `RecordOutcome` 区分 `Enqueued` 和 `DroppedFailOpen`。调用方拿到 `Ok` 后能知道是否真的入队，既保持 fail-open 语义，也保留测试和观测能力。

### xrouter 依赖边界

xrouter 业务模块和 adapter 只依赖 `latch-billing`。xrouter 自行实现异步基础设施（tokio、缓冲层、存储层）。xrouter 继续直接依赖 sqlx（已有依赖，不通过 latch-billing 间接引入）。

### ProviderRef 职责边界

`ProviderRef` 当前职责限于幂等键构造和定价快照选择辅助，不承诺承载完整 reconciliation 元数据。后续如需 provider kind/region/name 等字段，通过 `attributes` 或提升为 typed field。

### MeterSet accumulate 用 checked_add 防溢出

`MeterSet::accumulate()`（原 `insert`，更名以反映累加语义）返回 `Result<(), MeterSetError>`，内部用 `checked_add` 替代裸 `+=`。u64 溢出在 debug 会 panic、release 会 wrap，使用 `checked_add` 后统一返回错误，行为确定。

### UsageEventId::from_attempt 返回 Result 而非 panic

`from_attempt()` 对非法 `attempt_index` 返回 `Err(UsageEventIdError)` 而非 panic。库的核心 API 不应在边界输入上 panic。同时提供 `UsageEventIdBuilder` 支持 step_id/phase 等更精确的幂等维度。

### CurrencyCode 用 newtype + fn 构造

`CurrencyCode(String)` newtype 包装，用 `pub fn usd()` 等关联函数而非 `pub const`（`String::from` 不能用于 const 初始化）。提供 `FromStr` 实现，仅接受 3 位 ASCII 大写。不在 latch-billing 中引入 `iso_currency` 等重量级依赖。

### 币种约束在 PriceSnapshot 层

`PriceSnapshot.currency` 强制一次 rating 单币种。`MeterPrice` 不再各自携带币种，避免同一 snapshot 内不同 meter 不同币种导致 `RatingResult.currency` 无从选择的矛盾。

### MeterKind Custom(String) 的 Hash/Eq 正确性

Rust 的 `#[derive(Hash, Eq, PartialEq)]` 对 enum 内 `String` 字段调用 `String::hash()`/`String::eq()`，行为正确。`Custom(String)` 作为 HashMap key 不会与枚举变体冲突——`derive` 生成时会自动插入 enum discriminant。无需额外实现。

### 异步存储 trait 由下游应用定义

latch-billing 的 `ObservationStore` / `RatedRecordStore` 为同步 trait，仅用于内存/文件场景。下游应用（如 xrouter）应自行定义 `AsyncObservationStore` / `AsyncRatedRecordStore`（`#[async_trait]`）并实现异步缓冲层。这样 `PgObservationStore` 可以干净地用 `sqlx` 实现 async I/O，无需 `block_on`，同时保持 latch-billing 的运行时无关性。

### MVP 不做的

- proc macro（如 `#[track_tokens]`）
- provider SDK wrapper（latch-billing-openai 等不存在）
- Tower / Axum middleware
- 发票/税率/statement 模板
- 支付集成
- 多币种汇率转换
