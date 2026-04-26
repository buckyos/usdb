Title: UIP-0008 Activation Registry Implementation Notes
Status: Working Notes
Related: UIP-0008
Created: 2026-04-26

# 摘要

本文是 UIP-0008 的实现阶段备忘，用于记录 activation registry 在 USDB 多服务架构中的落地方式。

核心建议：

```text
集中定义，分散校验和使用
```

也就是：

- activation matrix / registry 作为同一份共识配置来源集中定义。
- 各服务在本地加载同一份 registry，并在自己的职责边界内校验和使用。
- 不引入运行时中心化 activation service，避免新增单点依赖。

本文不是正式协议正文。若后续实现稳定，应把稳定部分回写到 UIP-0008，或新增专门 UIP 固定 machine-readable registry schema、canonical encoding 和跨仓库发布机制。

# 背景

USDB 当前横跨多个组件：

- `balance-history`
- `usdb-indexer`
- `usdb-control-plane`
- ETHW / go-ethereum 扩展
- 浏览器、脚本和审计工具

这些组件都会依赖协议版本：

- BTC-side pass / balance / energy 版本。
- USDB state view / query semantics 版本。
- ETHW header payload / difficulty / reward 版本。
- local state commit / system state id 版本。

如果每个组件各自维护 activation matrix，就会出现同一高度下版本解释不一致的问题，进一步导致：

- `snapshot_id` 不一致。
- `system_state_id` 不一致。
- validator replay 失败。
- ETHW difficulty / reward 校验分叉。
- 审计工具无法复现节点状态。

# 设计原则

## 1. 单一配置来源

activation registry 应该只有一个 canonical source。

推荐路径：

```text
doc/UIP/activation-registry.json
```

或在实现阶段先放到更适合构建系统的位置，例如：

```text
src/btc/usdb-util/activation-registry.json
```

无论最终路径在哪里，所有服务都应该从同一份 registry 生成或读取版本信息。

## 2. 共享库优先

Rust 侧应在 `usdb-util` 中集中实现：

- `ActivationRecord`
- `ActivationRegistry`
- `ChainContext`
- `ActiveVersionSet`
- `activation_registry_id`
- `active_version_set_id`
- `lookup_active_version_set(context)`

`balance-history`、`usdb-indexer` 和 `usdb-control-plane` 不应各自实现 activation lookup 逻辑。

## 3. 跨语言生成或校验

go-ethereum 不能直接依赖 Rust crate，因此需要一个跨语言策略：

- 方案 A：go-ethereum 读取同一份 JSON registry，并实现同一套 canonical 校验。
- 方案 B：由 registry 生成 Go 侧常量和测试向量。
- 方案 C：先在早期实现中手写 Go chain config，但必须通过 `activation_registry_id` 和测试向量与 Rust 侧校验一致。

长期更推荐方案 A 或 B。

## 4. 本地校验，不依赖中心服务

不要把 activation lookup 做成一个运行时中心 RPC 服务。

原因：

- validator / miner 不能依赖额外中心服务决定共识规则。
- 中心服务不可用会影响出块和验块。
- 网络分区时容易产生不一致行为。

正确方式是每个节点本地加载同一份 registry，并在启动和运行时做一致性检查。

# 推荐架构

```text
activation-registry.json
        |
        | parse / canonical encode
        v
usdb-util::ActivationRegistry
        |
        +--> balance-history
        |       - expose balance_history_semantics_version
        |       - include upstream version fields in snapshot identity
        |
        +--> usdb-indexer
        |       - lookup BTC-side active_version_set by btc_height
        |       - compute active_version_set_id
        |       - include active_version_set_id in local_state_commit
        |       - expose versions in UIP-0006 state view
        |
        +--> usdb-control-plane
        |       - expose registry metadata
        |       - reject incompatible service combinations
        |
        +--> scripts / tests
                - activation boundary tests
                - version mismatch tests

activation-registry.json
        |
        | read or generate
        v
go-ethereum / ETHW
        - expected payload_version by ethw_block
        - expected difficulty_policy_version by ethw_block
        - expected reward_rule_version by ethw_block
        - compare USDB state view registry id / active version id
```

# 服务职责

## balance-history

职责：

- 暴露自身 API version 和 query semantics version。
- 在 `ConsensusSnapshotIdentity` 中保留 balance-history 相关版本字段。
- 不负责选择 USDB energy / level / reward 公式。

不应做：

- 不应自己维护 USDB activation matrix。
- 不应解释 ETHW reward 或 difficulty 版本。

## usdb-indexer

职责：

- 按 `btc_height` 查询 BTC-side `active_version_set`。
- 使用对应版本解析 pass、状态机、energy、effective energy、level。
- 将 `active_version_set_id` 绑定进 `local_state_commit`。
- 在 UIP-0006 state view 中返回 registry / version 相关审计字段。

关键点：

- 历史查询必须按目标高度 lookup，不能用当前 head 的版本。
- 如果本地不支持目标版本，必须 fail closed。
- 如果 registry 与 upstream / validator 期望不一致，必须返回明确 mismatch。

## usdb-control-plane

职责：

- 汇总服务版本、registry id、active version set 信息。
- 对外展示当前部署是否满足目标网络配置。
- 在启动或健康检查中发现不兼容服务组合。

不应做：

- 不应成为共识路径上的唯一 activation lookup 服务。

## ETHW / go-ethereum

职责：

- 按 ETHW block number / chain config 查询 expected `payload_version`。
- 按 ETHW block number / chain config 查询 expected `difficulty_policy_version`。
- 按 ETHW block number / chain config 查询 expected `reward_rule_version`。
- 验证 block header 中声明的 `payload_version` 和 `difficulty_policy_version`。
- 查询 USDB state view 时校验 registry id / active version set id。

关键点：

- payload 中的 `difficulty_policy_version` 是声明，不是 miner 自由选择。
- expected version 来自 ETHW chain config / activation registry。
- 不一致时拒绝区块或停止出块。

# Registry ID 与 Version Set ID

推荐语义：

```text
activation_registry_id = hash(canonical_activation_registry)
active_version_set_id  = hash(canonical_active_version_set)
```

`local_state_commit` 只需要承诺 `active_version_set_id`，不需要内联完整 `active_version_set`。

前提是：

- `active_version_set_id -> active_version_set` 可通过稳定 registry 重建。
- registry schema 和 canonical encoding 已固定。
- 节点启动时可以检查 registry id 是否符合目标网络。

在 schema 未固定前，先不要把 `activation_registry_id` 作为强共识字段。可以先作为 RPC / status 的审计字段暴露。

# 机器可读 Registry

机器可读 registry 是指 JSON / YAML / TOML 这类可以被程序解析的 activation matrix。

它解决的是：

- 避免手工同步 Markdown 表格。
- 给 Rust / Go / 测试脚本提供同一份输入。
- 为 `activation_registry_id` 提供 canonical 输入。
- 让 CI 能检查冲突记录和缺失网络。

落地顺序建议：

1. 先保留 Markdown 里的初始激活矩阵。
2. 实现 `usdb-util::ActivationRegistry` 时定义 JSON schema。
3. 增加测试向量，固定排序、字段类型、未知字段策略。
4. 生成 `activation_registry_id`。
5. go-ethereum 侧读取同一 JSON 或使用生成代码。

# 启动校验

每个关键服务启动时应该记录并校验：

- registry source。
- `activation_registry_id`。
- network id。
- supported version families。
- target network required version families。

公开网络上，如果 registry 缺失、冲突或版本不支持，服务必须 fail closed。

开发网络可以允许显式 override，但日志必须清晰标出：

```text
activation_registry_override=true
network_type=local
```

# RPC / State View 暴露

UIP-0006 state view 后续应该暴露：

```json
{
  "activation_registry_id": "...",
  "active_version_set_id": "...",
  "active_version_set": {
    "energy_formula_version": "...",
    "effective_energy_formula_version": "...",
    "level_formula_version": "...",
    "state_view_version": "..."
  }
}
```

可以先返回完整 `active_version_set` 方便调试；进入共识 commit 时只承诺 `active_version_set_id`。

# 测试建议

至少需要：

- registry parse test。
- canonical encoding test。
- duplicate active record conflict test。
- unknown public network fail closed test。
- regtest genesis activation test。
- height boundary lookup test。
- reorg across activation height test。
- local override is rejected on public networks。
- Rust / Go activation test vectors match。
- `local_state_commit` changes when `active_version_set_id` changes。

# 分阶段实现建议

## Phase 1: Rust-only Registry

- 在 `usdb-util` 中定义结构和 lookup。
- 先支持当前 v1 / genesis activation。
- `usdb-indexer` 使用 lookup 替代全局公式常量。
- UIP-0006 state view 返回 active version fields。

## Phase 2: Commit Binding

- 定义 canonical `active_version_set_id`。
- 将 `active_version_set_id` 纳入 `local_state_commit`。
- 增加 mismatch 错误和测试。

## Phase 3: Machine-readable Registry

- 增加 `activation-registry.json`。
- 固定 schema、排序和未知字段策略。
- 生成 `activation_registry_id`。

## Phase 4: ETHW Integration

- go-ethereum 侧读取或生成 registry。
- ETHW chain config 固定 expected versions。
- block validation 校验 `payload_version` 和 `difficulty_policy_version`。
- 与 USDB state view 交叉校验 registry id / active version set id。

# 待定事项

1. machine-readable registry 的最终路径。
2. canonical encoding 使用 JSON canonicalization 还是自定义二进制编码。
3. registry 是否进入 release artifact。
4. go-ethereum 使用读取 JSON 还是生成 Go 代码。
5. `activation_registry_id` 何时从审计字段升级为强共识字段。
