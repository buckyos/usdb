# Balance-History Script Registry Snapshot 优化备忘

## 1. 目的

本文记录 `script_registry` 在 `balance-history` 和 USDB 整体设计中的实际定位、
当前 full snapshot 为什么携带该数据，以及后续降低快照体积和生成成本的可选方向。

本文是优化备忘，不改变当前 snapshot schema、安装规则或已经生成的 snapshot artifact。
正式调整前仍需单独完成设计评审、容量测量和恢复测试。

相关文档：

- [Balance-History Script Registry Plan](../balance-history-script-registry-plan.md)
- [Core Snapshot 与 Script Registry Sidecar 设计](./balance-history-core-snapshot-and-script-registry-sidecar-design.md)
- [Exact-Height Snapshot Tool Design](./balance-history-exact-height-snapshot-tool-design.md)
- [Mainnet Exact-Height Snapshot Operations](./balance-history-mainnet-exact-height-snapshot-operations.md)

## 2. 当前语义

`script_registry` 保存：

```text
canonical script_hash -> original BTC scriptPubKey
```

索引器从 BTC transaction output 中读取 `scriptPubKey`，计算 canonical `script_hash`，
并将映射作为 block batch 的辅助副产物写入 RocksDB。相同 hash 的相同脚本允许重复观察，
最终按 hash 去重。

该映射解决的是不可逆查询：

```text
scriptPubKey -> script_hash    可以直接计算
script_hash -> scriptPubKey    不能通过哈希反推
scriptPubKey -> BTC address    仅标准脚本可以按指定网络转换
```

registry 具有以下属性：

- 是 display/lookup cache，不属于 balance-history block commit；
- 不进入 USDB 共识状态或经济状态计算；
- 不要求所有脚本都能转换为 BTC address；
- 当前采用 append-like/best-effort 语义，不为 reorg 删除已观察到的映射；
- 当前 full snapshot 会完整导出和安装 registry，以保持历史地址解析能力。

## 3. 实际使用场景

### 3.1 需要反向查询的场景

- `resolve_script_hashes` RPC 将查询结果中的 owner/script hash 转为原始
  `scriptPubKey`，并尝试转换为用户可读的 BTC address；
- balance-history browser 展示余额、历史记录或 UTXO 对应的 BTC address；
- usdb-indexer browser 展示 Miner Pass owner 的 BTC address；
- 运维、审计和调试时检查某个 script hash 对应的原始锁定脚本；
- snapshot 安装节点在没有从创世块重新扫描的情况下解析 snapshot 高度之前出现过的
  历史 script hash。

### 3.2 不依赖反向查询的核心路径

以下路径内部使用 canonical script hash，不需要恢复原始 `scriptPubKey`：

- BTC balance history 和 UTXO 状态更新；
- spend 时按旧 UTXO 的 script hash 扣减余额；
- block commit、state ref 和 reorg rollback；
- `total_miner_btc_sats` 聚合；
- Miner Pass owner 比较和 `leader_btc_addr -> script hash` 正向解析；
- candidate set、raw/effective energy 和 USDB 共识验证。

因此 registry 缺失不应改变共识结果，但会使历史 script hash 的地址展示不可用或不完整。

## 4. 当前 Snapshot 契约

虽然 registry 不是共识状态，当前 full snapshot 仍把它作为完整 artifact 的组成部分：

- SQLite snapshot 包含 `script_registry` 表；
- manifest/meta 记录 `script_registry_count`；
- full verify 校验表计数与 metadata 一致；
- snapshot install 将 registry 恢复到目标 RocksDB；
- readiness 单独暴露 registry 的可用状态和记录数。

本地高度 `963800` 的主网生成结果可作为一次容量基线，不是协议常量：

| 指标 | 观测值 |
| --- | ---: |
| SQLite snapshot 文件大小 | `271354195968` bytes |
| balance history | `59356343` |
| live UTXO | `165748439` |
| block commit | `963800` |
| script registry | `1541365559` |

registry 的记录量远大于 live UTXO，已经明显影响 snapshot 导出、校验、分发和安装成本。
后续应通过分阶段耗时、SQLite page 使用量、压缩包体积和安装耗时进一步量化，而不能仅按
记录数推断实际占比。

## 5. 后续优化选项

### 5.1 保持当前 full snapshot

优点是 artifact 和恢复流程最简单，fresh joiner 安装后立即具备完整历史地址解析能力。
缺点是所有共识节点都要承担辅助数据的生成、下载、校验和安装成本。

### 5.2 核心快照与 registry sidecar 分离（已选方向）

将发布物拆分为：

```text
core snapshot
  balance history + live UTXO + block commits + consensus/state identity

optional script-registry artifact
  script_hash -> scriptPubKey
```

预期语义：

- core snapshot 独立完成共识安全校验和后续追块；
- registry artifact 独立分片、校验、签名、下载和安装；
- core-only 节点的 `consensus_ready` 不受 registry 缺失影响；
- `resolve_script_hashes` 明确区分完整覆盖下的 `not_found` 与 sidecar 缺失时的
  `unresolved`，不再用 `found=false` 混合两种语义；
- explorer/archive 节点安装完整 registry，普通共识节点可以不安装。

该方案保留完整审计能力，同时避免辅助索引阻塞核心 checkpoint 发布。后续实现进一步确定：
registry sidecar 保持为独立、不可变的只读 SQLite，snapshot 节点不再把历史 registry 导入
RocksDB；RocksDB 只保留从高度 `0` 重建所得的完整 registry，或 snapshot height 之后的 live
overlay。详细契约见 [Core Snapshot 与 Script Registry Sidecar 设计](./balance-history-core-snapshot-and-script-registry-sidecar-design.md)。

### 5.3 只保留活跃集合

可以只保存 live UTXO、当前 Miner Pass owner 或近期窗口涉及的脚本。该方案体积更小，但会
丢失任意历史 script hash 的反向解析能力，并引入集合边界、reorg 和 pruning 语义，不建议在
没有明确产品需求前直接采用。

### 5.4 按需从 Bitcoin 数据恢复

可在查询时通过 raw transaction、`txindex` 或区块扫描恢复 scriptPubKey。该方案会增加
Bitcoin Core 配置依赖、历史查询延迟和失败模式；对于已花费输出，没有 `txindex` 时通常需要
额外索引或重新扫描，因此不适合作为稳定的默认 RPC 后端。

## 6. 推荐实施顺序

1. 在 snapshot 工具中记录每张表的 SQLite page/byte 占用、导出时间、verify 时间和安装时间。
2. 冻结 core snapshot 的最小必需表、identity 和 fail-closed 校验规则。
3. 设计 registry sidecar 的 manifest、分片、签名、原子安装和 partial readiness 语义。
4. 增加 core-only、full、sidecar 损坏、安装中断、restart 和 fresh joiner 测试。
5. 对比 full 与 split artifact 在 100 GB 以上数据量下的生成、传输、磁盘峰值和恢复时间。
6. 设计通过评审后再修改 snapshot schema；开发阶段无需保留旧 schema 兼容双栈。

## 7. 当前结论

- 当前高度 `963800` 的 snapshot 包含完整 registry，符合现有 full snapshot 契约，无需重建。
- registry 当前只服务展示、解析和审计，不应成为 USDB 共识或经济状态的可用性前提。
- 已确定采用 core snapshot 与可选 registry sidecar 分离，本文仍作为问题和方案比较备忘；
  实施边界和任务拆分以独立设计文档为准。
