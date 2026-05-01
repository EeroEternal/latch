# Latch - AI Agent Guidelines

## 🎯 项目定位

**Latch 是一个纯基础库（SDK）集合，专注于提供运行时无关的状态管理能力。**

### 核心设计原则

1. **运行时无关（Runtime-Agnostic）**
   - 所有核心 crate 都是纯同步的
   - 不绑定特定异步运行时（tokio、async-std 等）
   - 下游应用可以自由选择运行时

2. **零 I/O 依赖**
   - 不直接操作数据库、网络或文件系统
   - 只提供计算逻辑和类型定义
   - 通过 trait 抽象让下游实现 I/O 层

3. **可插拔（Pluggable）**
   - 通过 trait 定义接口
   - 下游应用自行实现具体逻辑
   - 保持核心库的纯净性

4. **中立性（Neutral）**
   - 不依赖具体的 gateway 实现
   - 不依赖 provider SDK（OpenAI、Anthropic 等）
   - 不理解具体用户业务字段

---

## 🚧 边界规范

### ✅ Latch 应该包含的

- **类型定义**: 数据结构、枚举、配置结构
- **同步计算逻辑**: 评分、路由决策、重试策略、计费等
- **Trait 定义**: 供下游应用实现的接口
- **配置验证**: 参数校验和默认值
- **纯函数**: 无副作用的计算

### ❌ Latch 不应该包含的

- **异步运行时基础设施**
  - ❌ tokio 缓冲层（如 BufferedMeteringSink）
  - ❌ 连接池管理
  - ❌ 异步 worker
  
- **数据库实现**
  - ❌ PostgreSQL 存储（PgObservationStore）
  - ❌ Redis 缓存（RedisQuotaAuthorizer）
  - ❌ 任何具体的数据库驱动
  
- **网络 I/O**
  - ❌ HTTP 服务器（Axum、Actix）
  - ❌ HTTP 客户端（除了 latch-detect 的探测需求）
  - ❌ WebSocket 连接
  
- **Provider SDK Wrapper**
  - ❌ OpenAI SDK 封装
  - ❌ Anthropic SDK 封装
  - ❌ 任何 provider 特定的适配
  
- **业务逻辑**
  - ❌ 发票生成
  - ❌ 支付集成
  - ❌ 税率计算
  - ❌ 用户管理

---

## 📦 Crate 职责

### 核心基础

- **latch-core**: 共享类型、配置、枚举（所有 crate 的基础）
- **latch-sdk**: Feature-gated 伞形 crate

### 状态管理（纯同步）

- **latch-compress**: 消息压缩（滑动窗口）
- **latch-cache**: Prompt 缓存元数据规划
- **latch-router**: 启发式路由决策
- **latch-retry**: 重试/熔断策略（可选 tokio 辅助）
- **latch-meter**: 会话用量计量
- **latch-billing**: 精确计费核心
- **latch-score**: 端点质量评分

### 特殊场景

- **latch-detect**: 后端引擎检测（唯一需要异步的 crate，因为必须网络 I/O）

---

## 🏗️ 架构分层

```
┌─────────────────────────────────────────────┐
│          下游应用（如 xrouter）              │
│  ┌─────────────────────────────────────┐   │
│  │  异步基础设施层（应用负责）          │   │
│  │  - tokio 运行时                     │   │
│  │  - 缓冲层                           │   │
│  │  - 数据库存储                       │   │
│  │  - HTTP 服务器                      │   │
│  └─────────────────────────────────────┘   │
│                      ↓                      │
│  ┌─────────────────────────────────────┐   │
│  │  Latch SDK（纯同步核心）             │   │
│  │  - 类型定义                         │   │
│  │  - 计算逻辑                         │   │
│  │  - Trait 接口                       │   │
│  └─────────────────────────────────────┘   │
└─────────────────────────────────────────────┘
```

**关键规则**: Latch 只负责下半部分，上半部分由下游应用实现。

---

## 💡 设计决策指南

### 添加新功能时的检查清单

在添加新代码前，问自己：

1. **是否需要运行时？**
   - ❌ 如果需要 tokio/async → 不应该加入 latch
   - ✅ 如果纯同步计算 → 可以加入

2. **是否涉及 I/O？**
   - ❌ 如果需要读写数据库/网络 → 定义 trait，让下游实现
   - ✅ 如果只是内存计算 → 可以加入

3. **是否依赖具体实现？**
   - ❌ 如果依赖特定 gateway/provider → 不应该加入
   - ✅ 如果是通用逻辑 → 可以加入

4. **是否理解业务字段？**
   - ❌ 如果理解用户业务语义 → 不应该加入
   - ✅ 如果只处理通用信号 → 可以加入

### 正确的做法

```rust
// ✅ 定义 trait（在 latch 中）
pub trait ObservationStore: Send + Sync {
    fn append_observation(&self, obs: UsageObservation) -> Result<StoreResult, StoreError>;
}

// ❌ 实现具体的数据库存储（在下游应用中）
struct PgObservationStore {
    pool: sqlx::PgPool,
}

impl ObservationStore for PgObservationStore {
    fn append_observation(&self, obs: UsageObservation) -> Result<StoreResult, StoreError> {
        // 下游应用自己实现
    }
}
```

---

## 🚫 历史教训

### latch-billing-tokio 的移除

**错误**: 曾经在 latch 中包含 `latch-billing-tokio`，提供异步缓冲和数据库存储实现。

**问题**:
- 违反了"运行时无关"原则
- 混入了应用层基础设施
- 增加了不必要的依赖

**修正**: 已移除。异步基础设施应由下游应用（如 xrouter）自行实现。

**教训**: 
- Latch 只提供 trait 和同步核心
- 异步实现永远在应用层
- 保持 SDK 的纯净性

---

## 📝 代码规范

### 依赖管理

- **latch-core**: 无外部依赖（除了 serde、chrono 等基础库）
- **其他 crate**: 只依赖 latch-core 和必要的基础库
- **禁止**: 引入 tokio、sqlx、redis 等运行时/数据库依赖（latch-detect 除外）

### Feature 开关

可以使用 optional feature，但必须保持向后兼容：

```toml
[dependencies]
tokio = { version = "1", features = ["time"], optional = true }

[features]
default = []
tokio = ["dep:tokio"]  # 可选的异步辅助，不影响核心同步逻辑
```

### 文档要求

- 所有公共 API 必须有文档注释
- 明确标注同步/异步属性
- 说明适用场景和限制
- 提供使用示例

---

## 🔍 代码审查要点

审查 PR 时，检查：

1. ✅ 是否引入了运行时依赖？
2. ✅ 是否实现了具体的 I/O 操作？
3. ✅ 是否依赖了特定的 gateway/provider？
4. ✅ 是否理解了业务字段？
5. ✅ 是否应该定义为 trait 让下游实现？
6. ✅ 是否符合"纯同步核心"原则？

如果任何一项回答是"不应该"，则要求重构。

---

## 📚 参考文档

- [README.md](README.md) - 项目概述和使用指南
- [latch-billing 设计文档](docs/latch-billing-design.md) - 计费系统设计
- [latch-score 需求文档](docs/latch-score-requirements.md) - 质量评分需求

---

## 🤖 Agent 工作指南

当 AI agent 处理 latch 项目时：

1. **始终检查依赖**: 不添加 tokio、sqlx 等运行时依赖
2. **优先定义 trait**: 需要 I/O 时，先定义 trait，不实现
3. **保持同步**: 核心逻辑必须是纯同步的
4. **理解边界**: 不清楚是否应该加入时，参考本文件
5. **中文注释**: 所有代码注释使用中文

**记住**: Latch 是基础库，不是完整的应用框架！
