# Balance-History Core Snapshot 与 Script Registry Sidecar 设计

## 1. 文档状态

- 状态：Draft，等待实现评审。
- 适用阶段：USDB 开发期，不保留旧 snapshot schema 或安装流程的兼容双栈。
- 已确认方向：将 `script_registry` 从 core snapshot 剥离为独立、只读的 SQLite
  sidecar；snapshot 安装节点不再把历史 registry 导入 RocksDB。
- 批次 1 进度：core/registry v1 schema、manifest、artifact ID、签名域以及 registry
  readiness/resolution 类型已实现，等待代码评审；现有生成器、安装器和 RPC 尚未切换。
- 本文冻结目标语义、存储边界和实施顺序；批次 1 只增加可复用契约，不切换现有运行路径。

相关文档：

- [Script Registry Snapshot 优化备忘](./balance-history-script-registry-snapshot-optimization-memo.md)
- [Exact-Height Snapshot Tool Design](./balance-history-exact-height-snapshot-tool-design.md)
- [Snapshot Signing](./balance-history-snapshot-signing.md)
- [Readiness Design](./balance-history-readiness-design.md)
- [Release Node Kit and Deployment](../publish/usdb-release-node-kit-and-deployment.md)

## 2. 背景与问题

`script_registry` 保存：

```text
canonical script_hash -> original BTC scriptPubKey
```

它用于把内部 canonical script hash 解析为原始 `scriptPubKey`，再在标准脚本可转换时生成
指定 BTC network 下的地址。它是展示、审计和诊断所需的不可逆映射，不是余额或共识状态。

高度 `963800` 的主网 snapshot 提供了当前容量基线：

| 数据 | 记录数 |
| --- | ---: |
| balance history | `59,356,343` |
| live UTXO | `165,748,439` |
| block commit | `963,800` |
| script registry | `1,541,365,559` |

registry 约占四类记录总数的 `87%`。当前 full snapshot 安装会先校验约 253 GiB 的 SQLite
文件，再把全部 registry 从 SQLite 解码并通过普通 RocksDB batch 路径重新写入。测试节点上，
core 三表约数小时完成，而 registry 按约 12K entries/s 需要一天以上。这样会出现以下倒置：

- snapshot 原本用于缩短 fresh joiner 启动时间；
- 辅助 registry 的重复导入反而可能使安装慢于从零扫描 BTC blk；
- balance-history、usdb-indexer 和 USDB chain 被一个非共识数据集阻塞；
- SQLite artifact 和 RocksDB registry 同时保留同一份历史映射，产生额外磁盘占用和写放大。

## 3. 设计目标

本次重构必须满足：

1. core snapshot 安装完成后即可启动 balance-history 和后续服务。
2. registry 缺失、下载中或损坏不得改变 balance、UTXO、block commit、state-ref 或
   `consensus_ready`。
3. snapshot 节点可以组合 snapshot height 及之前的只读 base registry 与之后的 live
   registry。
4. 从高度 `0` 重建的节点继续使用 RocksDB registry，不强制生成本地 SQLite sidecar。
5. 不把十亿级 base registry 再导入 RocksDB，也不在启动路径制造第二份历史数据。
6. registry 查询必须明确区分“找到”“完整集合中未找到”和“覆盖不完整，暂时无法判断”。
7. core 与 registry 各自具备独立 hash、manifest、签名、状态、失败恢复和发布生命周期。
8. 当前处于开发阶段，新 schema 直接替换旧 schema，不引入长期迁移或兼容分支。

## 4. 非目标

本设计不做以下事情：

- 不让 registry 参与 USDB 共识或 balance-history block commit。
- 不承诺每个 script hash 都能转换为 BTC address。
- 不通过 Bitcoin RPC、txindex 或临时重扫区块弥补 sidecar 缺失。
- 不删除 reorg 分支曾观察到的 registry 映射；registry 继续采用 append-like 语义。
- 不把整个 registry 常驻内存。
- 不支持新运行时继续安装旧的单文件 full snapshot。

## 5. 术语

### 5.1 Core snapshot

能够恢复并继续 balance-history 共识相关状态的最小 checkpoint，包含：

- balance history；
- live UTXO；
- block commits；
- snapshot metadata、DB identity、query retention floor 和 state-ref。

core snapshot 不包含 `script_registry` 表。

### 5.2 Base registry sidecar

与某个 core snapshot 高度关联的、不可变、只读 SQLite 文件，包含 snapshot height 及之前
已观察到的 `script_hash -> scriptPubKey` 映射。它有独立 manifest 和签名，不进入 core
snapshot ID。

### 5.3 Live registry overlay

balance-history 正常扫块时写入现有 RocksDB `script_registry` CF 的映射：

- 从高度 `0` 重建时，它覆盖从创世块到当前高度的全部本地观察结果；
- 从 snapshot height `H` 启动时，它通常只包含 `H + 1` 之后的新观察结果；
- 它可以包含 reorg 分支留下的映射，因为 registry 本身是 append-like 辅助缓存。

### 5.4 Registry resolver

RPC 查询层中的只读组合器。它先查询 live overlay，再对未命中的 hash 查询 base sidecar，
最后结合 coverage 状态返回结果。

## 6. 核心不变量

### 6.1 共识不变量

以下字段和结论不得依赖 registry：

```text
balance state
live UTXO state
block commit
historical/current state-ref
total_miner_btc_sats
balance-history query_ready / consensus_ready
usdb-indexer consensus_ready
USDB profile、candidate、energy、K、reward 和 difficulty
```

registry sidecar 的 hash、entry count、安装状态和查询结果不得进入 core snapshot ID 或任何
USDB 共识 payload。

### 6.2 Core 安装不变量

core snapshot 在发布为 live RocksDB 前必须满足：

```text
durable height
  == balance state height
  == UTXO state height
  == latest block commit height
  == snapshot height H
```

安装器必须完成 core artifact hash、签名、DB identity、retention floor 和 state-ref 校验，
再通过 staging DB 原子切换。registry sidecar 的状态不能参与这次原子提交。

### 6.3 Registry 映射不变量

每条映射的 `script_pubkey` 必须重新计算得到记录中的 canonical `script_hash`。在 artifact
构建、sidecar 校验、doctor/audit 或重叠数据检查中发现 overlay 与 sidecar 同 hash 时：

- 字节相同：正常返回；
- 字节不同：标记 registry 为 `conflict`，相关辅助查询返回错误并记录 `ERROR`；
- 冲突不得回滚 core DB，也不得改变 consensus readiness。

普通查询热路径不为 overlay hit 再执行一次 sidecar 查询；sidecar 只接收 overlay miss。单条值的
hash 自校验和独立 doctor/audit 负责发现损坏或重叠冲突，避免为了非共识防御让每次解析都双读磁盘。

## 7. 两种节点运行模式

### 7.1 从高度 0 完整重建

```text
core RocksDB:            从 0 扫描到 tip
live registry overlay:   从 0 扫描到 tip
base registry sidecar:   无
registry coverage:       full_replay
```

该模式维持当前写入逻辑。所有本地观察到的历史 registry 都位于 RocksDB。

### 7.2 从 snapshot 启动

```text
core RocksDB:            core snapshot(H) + H+1..tip
live registry overlay:   H+1..tip
base registry sidecar:   0..H 的发布 artifact，可选
registry coverage:       snapshot_plus_sidecar 或 post_snapshot_only
```

core snapshot 安装后立即允许 balance-history 启动。sidecar 可以在 core 启动之前已经存在，也可以
在服务运行后下载、校验并原子启用。

sidecar 不可用时，余额和共识服务继续运行；只有 snapshot height 及之前的反向解析不完整。

## 8. 存储布局

建议布局：

```text
<balance-history-root>/
|-- db/
|   `-- balance_history/                 # core RocksDB + live registry overlay
|-- auxiliary/
|   `-- script-registry/
|       |-- state.json                   # 当前 sidecar 状态和原子 active pointer
|       `-- bases/
|           `-- <registry-artifact-id>/
|               |-- script_registry_<H>.db
|               |-- script_registry_<H>.manifest.json
|               `-- script_registry_<H>.manifest.sig
`-- bootstrap/
    |-- snapshot-loader.done.json        # 只表示 core snapshot 安装完成
    `-- snapshot-loader.progress.json    # 只记录 core import
```

约束：

- sidecar 文件以 SQLite `immutable=1`、`query_only=true` 打开；
- active pointer 必须通过临时文件加原子 rename 更新；
- sidecar 校验完成前不能成为 active base；
- active sidecar 必须被 artifact GC/purge 逻辑引用保护；
- 删除或替换 sidecar 不得修改 core RocksDB；
- live overlay 继续和 core DB 共用现有 RocksDB 进程，避免引入第二个写服务。

## 9. Artifact 与 Manifest

### 9.1 Core artifact

建议文件：

```text
balance_history_core_<H>.db
balance_history_core_<H>.manifest.json
balance_history_core_<H>.manifest.sig
```

core manifest 延续现有共识字段，但必须增加明确的：

```json
{
  "manifest_version": "balance-history-core-snapshot-manifest:v1",
  "artifact_type": "balance_history_core",
  "snapshot_schema_version": "balance-history-core-snapshot:v1",
  "registry_included": false,
  "core_snapshot_id": "...",
  "core_artifact_id": "...",
  "file_name": "balance_history_core_<H>.db",
  "file_sha256": "...",
  "state_ref": {},
  "db_identity": {},
  "balance_query_floor": 963800,
  "history_query_floor": 963801,
  "signature_scheme": "ed25519",
  "signing_key_id": "...",
  "generated_at": 1725000000
}
```

`core_snapshot_id` 是 `state_ref.snapshot_id`，表示可被共识引用的状态身份；
`core_artifact_id` 额外绑定 schema、状态身份和实际 SQLite 文件 hash，只用于区分发布物字节。

### 9.2 Registry artifact

建议文件：

```text
script_registry_<H>.db
script_registry_<H>.manifest.json
script_registry_<H>.manifest.sig
```

registry manifest 至少包含：

```json
{
  "manifest_version": "balance-history-script-registry-manifest:v1",
  "artifact_type": "balance_history_script_registry",
  "file_name": "script_registry_<H>.db",
  "file_sha256": "...",
  "registry_artifact_id": "...",
  "registry_schema_version": "balance-history-script-registry-sqlite:v1",
  "policy": "auxiliary_seen_scripts_non_consensus_v1",
  "base": {
    "btc_network": "bitcoin",
    "btc_genesis_hash": "...",
    "base_height": 963800,
    "base_block_hash": "...",
    "core_snapshot_id": "..."
  },
  "entry_count": 1541365559,
  "signature_scheme": "ed25519",
  "signing_key_id": "...",
  "generated_at": 1725000001
}
```

`registry_artifact_id` 应由 canonical manifest identity 字段和文件 hash 派生。它与
`core_snapshot_id` 分开，确保替换、缺失或损坏 registry 不改变共识状态身份。
sidecar 启用前必须执行 `validate_against_core`，逐项匹配 `core_snapshot_id`、height、
BTC block hash、network 和 genesis hash。

### 9.3 SQLite schema

首版 sidecar 只需要 metadata 和 registry 两张表：

```sql
CREATE TABLE meta (...);

CREATE TABLE script_registry (
    script_hash   BLOB NOT NULL PRIMARY KEY,
    script_pubkey BLOB NOT NULL
) WITHOUT ROWID;
```

使用 `WITHOUT ROWID` 让 BLOB 主键和 value 位于同一 B-tree，避免当前普通 rowid 表同时维护
table B-tree 与主键索引。生成、verify 和查询工具必须冻结 SQLite 版本下的 schema golden。

批次 1 冻结的 schema 文件为：

- `src/btc/balance-history/src/db/core_snapshot_v1.sql`；
- `src/btc/balance-history/src/db/script_registry_v1.sql`。

### 9.4 Artifact ID 与签名域

两个 artifact ID 均使用 SHA-256，输入编码固定为：

```text
u32_be(domain_byte_length)
|| domain_utf8
|| u64_be(identity_json_byte_length)
|| identity_json_utf8
```

identity JSON 按已冻结 Rust identity struct 的字段顺序做紧凑 UTF-8 JSON 编码。artifact ID
包含文件 SHA-256，但不包含文件名、`generated_at`、签名 scheme 和 signer ID，因此重命名或
更换签名密钥不改变 artifact identity，SQLite 字节变化则一定改变。

签名 payload 使用相同长度前缀结构，但 JSON 是包含 artifact ID、文件名、时间和签名元数据的
完整 manifest。core 与 registry 使用不同 domain：

```text
usdb.balance-history.core-snapshot-artifact-id:v1
usdb.balance-history.core-snapshot-manifest-signature:v1
usdb.balance-history.script-registry-artifact-id:v1
usdb.balance-history.script-registry-manifest-signature:v1
```

manifest JSON 拒绝未知字段。Rust 单元测试固定 artifact ID 与签名 payload SHA-256 golden；
其他语言或工具实现必须通过相同向量，不能自行调整 JSON 字段顺序或编码。

## 10. 查询语义

### 10.1 查询顺序

批量 `resolve_script_hashes` 的顺序固定为：

1. 对全部 hash 执行 RocksDB overlay `multi_get`；
2. 收集未命中项；
3. sidecar 为 ready 时，对未命中项执行只读 SQLite 批量查询；
4. 合并并恢复请求顺序；
5. 对标准脚本按当前 BTC network 转换 address。

sidecar 查询必须有限制的 batch size、bounded SQLite page cache 和可观测的慢查询日志。不能把
完整表或无界结果加载进内存。

### 10.2 结果状态

开发期直接替换旧 `found: bool` 二态，建议每个 item 返回：

```text
found_overlay
found_base
not_found
unresolved
conflict
```

其中：

- `not_found` 只表示在当前声明为完整的 registry coverage 中未找到；
- `unresolved` 表示 snapshot 节点缺少或尚未启用 base sidecar，不能判断旧高度映射；
- `conflict` 表示读取到的 value 无法通过请求 hash 的自校验，或该 hash 已被
  `doctor`/audit 记录为 overlay 与 sidecar 的重叠冲突；
- `source` 可额外返回 `overlay` 或 `base_sidecar`，方便审计。

响应顶层同时携带当前 registry status，避免调用方从单条结果猜测 coverage。

## 11. Registry Readiness 与 Provenance

现有 `ScriptRegistryStatus.available` 只能表达 CF 是否可查询，不能表达历史覆盖。批次 1
已冻结以下替代类型，但在分层 resolver 接入前不替换当前 RPC：

```text
state:
  absent
  disabled
  downloading
  verifying
  ready
  failed
  conflict

coverage_mode:
  full_replay
  snapshot_plus_sidecar
  post_snapshot_only

capabilities:
  script_registry_lookup
  script_registry_complete_coverage

overlay_estimated_count
base_height
base_block_hash
core_snapshot_id
registry_artifact_id
expected_count
last_error
```

规则：

- `query_ready` 和 `consensus_ready` 不读取这些字段；
- `state` 只描述 optional sidecar 生命周期，`coverage_mode` 描述 overlay 与 sidecar
  合并后的总覆盖范围，两者是正交字段；
- `script_registry_complete_coverage` 必须与 coverage 匹配；只有完整 coverage 下的 miss
  才能返回 `not_found`，否则返回 `unresolved`；
- `full_replay` 使用 `state=disabled/absent` 且不携带 sidecar provenance；
  `snapshot_plus_sidecar` 必须使用 `state=ready` 并携带完整 base/artifact provenance；
- `failed/conflict` 产生清晰的 `WARN/ERROR` 和 operator guidance，但不停止核心索引；
- `doctor` 校验 sidecar hash、签名、identity、active pointer 和可读性；
- provenance 分为 core snapshot provenance 与 registry provenance，不能复用一个“安装完成”标志。

## 12. 启动与部署编排

### 12.1 Gate 调整

新启动顺序：

```text
Bitcoin data-start anchor ready
  -> core snapshot loader
  -> balance-history
  -> usdb-indexer
  -> USDB chain

registry download/verify/activate
  -> 独立辅助路径，不阻塞上述 gate
```

Docker 中 `balance-history` 只依赖 core loader 成功，不依赖 registry job。balance-history 容器需要
以只读方式访问已激活 sidecar，或由 node installer 把 active sidecar 放入约定目录。

### 12.2 进度展示

固定展示建议：

```text
Core snapshot      IMPORTING / READY
Script registry    ABSENT / DOWNLOADING / VERIFYING / READY / FAILED (optional)
Bitcoin            SYNCING / READY
Balance history    SYNCING / READY
USDB indexer       SYNCING / READY
USDB chain         SYNCING / READY
```

当核心服务已 ready 而 registry 未完成时：

- overall 仍可为 `READY`；
- 同时输出 `auxiliary=PARTIAL` 或独立 registry 状态；
- 提示只影响历史 script/address 反向解析；
- 不使用 `DEGRADED` 暗示共识服务不安全。

## 13. 工具和接口改动范围

### 13.1 Rust snapshot 组件

- `SnapshotDB` 拆分 core schema 与 registry sidecar schema。
- snapshot generator 分别输出 core 和 registry artifact。
- core installer 删除 registry import 阶段，八阶段进度调整为 core-only 阶段。
- 增加 immutable registry sidecar reader 和 resolver。
- full replay 模式保留当前 RocksDB registry 写入。
- 增加 registry provenance、active pointer 和冲突检测。

### 13.2 `balance-history-snapshot-tool`

- `create`：同一 sealed workspace 可生成 core 和 registry 两个独立 job/artifact。
- `status/list`：分别报告 core 与 registry 的状态、大小、hash 和完成时间。
- `verify`：支持 `--component core|registry|all`。
- `finalize`：core 和 registry 独立 finalize，不互相触发重复 verify。
- `publish`：允许 core 先发布；registry 后发布或省略。
- job state 不允许 registry 失败撤销已完成的 core artifact。

建议的 job 关系：

```text
Seal(H)
  -> BuildCore -> VerifyCore -> CoreComplete
  -> BuildRegistry -> VerifyRegistry -> RegistryComplete
```

两个 build 共享同一个 sealed source identity，但完成状态独立。

### 13.3 发布和对象存储工具

- snapshot release record 增加 `core` 必选对象和 `script_registry` 可选对象。
- core 与 registry 分别上传、checksum、断点续传和发布完成 marker。
- release manifest、network bundle 和 paired checkpoint 只把 core 作为共识依赖。
- public release 可以声明推荐 registry sidecar，但不得把它列入 core acceptance gate。
- installer 必须 pin 当前 active sidecar，防止 cache 清理删除正在查询的文件。

### 13.4 Docker 与 `usdb-node`

- `snapshot-loader` 改名或语义收敛为 core snapshot loader。
- 移除 `balance-history depends_on` 对 registry job 的依赖。
- `setup/up/status/doctor/logs` 增加 registry 独立状态。
- `--with-script-registry` 控制是否后台获取 sidecar；默认策略由 network bundle 明确冻结。
- `--without-script-registry` 允许 core-only 节点，不影响共识能力声明。

### 13.5 RPC、client、CLI 和 explorer

- 扩展 `get_readiness.script_registry`。
- 重构 `resolve_script_hashes` item 状态和 response coverage。
- Rust client、CLI、control-plane 和 explorer 不再把 `found=false` 当作确定不存在。
- 能力声明中区分 `script_registry_lookup` 与 `script_registry_complete_coverage`。

### 13.6 审计和 comparator 工具

- legacy snapshot comparator 支持 core 与 registry 分开比较。
- Electrs/Bitcoin Core 审计工具默认只校验 core，不因 registry sidecar 缺失失败。
- registry 专项审计直接读取 sidecar，并可抽样对比 overlay 与 base。
- 容量报告分别记录 core SQLite、registry SQLite 和 live overlay 大小。

## 14. 失败与恢复

### 14.1 Core 失败

保持现有 fail-closed 规则：core hash、签名、identity、state-ref、flush 或 swap 任一步失败，
balance-history 不启动。

### 14.2 Registry 失败

registry 下载、hash、签名、SQLite integrity 或 open 失败时：

- 不修改 active pointer；
- 保留上一份已验证 sidecar；
- 没有旧 sidecar时进入 `post_snapshot_only`；
- RPC 对 overlay miss 返回 `unresolved`；
- 记录错误、显示修复命令，但核心服务继续运行。

### 14.3 Restart

由于 sidecar 不需要导入：

- 已下载但未验证：继续 verify；
- 已验证但未激活：重新核对 manifest 后原子激活；
- 已激活：直接 immutable open；
- 文件缺失或 hash 变化：撤销 sidecar readiness，但不触碰 core DB。

### 14.4 Reorg

registry 保持 append-like：

- sidecar 绑定发布时的 BTC block identity，用于 provenance；
- sidecar 中包含后来被 reorg 的历史脚本不影响余额或共识；
- overlay 不因 rollback 删除 registry 映射；
- 同 hash 不同脚本仍按 conflict 处理。

## 15. 测试矩阵

### 15.1 单元测试

- overlay hit、base hit、双 miss 和请求顺序保持；
- overlay/base 相同映射与冲突映射；
- `found_overlay/found_base/not_found/unresolved/conflict` 结果；
- full replay、snapshot plus sidecar、post-snapshot-only coverage；
- registry 状态不会改变 core readiness；
- sidecar manifest canonical encoding、ID 和签名 golden vector；
- `WITHOUT ROWID` schema、批量查询上限和 bounded cache。

### 15.2 Snapshot 集成测试

- core artifact 不含 registry 表仍能完整安装并继续花费旧 UTXO；
- core state-ref 与旧 full snapshot 的 core 语义一致；
- registry build 失败不撤销 core complete；
- core 与 registry 可独立 verify/finalize/publish；
- sidecar 缺失、损坏、错误网络、错误高度、错误 core snapshot ID 被拒绝；
- active pointer 原子替换和 restart 恢复。

### 15.3 运行时测试

- 从 0 重建节点不配置 sidecar，registry 全部来自 RocksDB；
- snapshot 节点 core-only 启动后达到 consensus ready；
- sidecar 后置到达后无需重启即可从 `unresolved` 变为 `found_base`；
- snapshot height 后的新脚本立即从 overlay 解析；
- balance-history 扫块和 sidecar 查询并发；
- sidecar 查询超时或 I/O 错误不影响核心 RPC；
- same-height/deep reorg 不改变 registry append-like 语义。

### 15.4 Docker、release 和 live E2E

- core 下载、verify、import、atomic swap 后立即启动服务；
- registry 下载/verify 与核心同步并行但不构成 gate；
- `usdb-node status --watch` 独立显示 registry；
- core-only、with-registry、registry-corrupt、restart、fresh joiner；
- release manifest 缺失 core 必须拒绝，缺失 optional registry 允许启动；
- 100M/1B 级 sidecar 点查延迟、并发读取、page cache 和 FD 上限；
- 对比旧 full import、core-only import、core plus sidecar 三种耗时和磁盘峰值。

## 16. 实施批次

### 批次 1：冻结类型和 artifact 契约（已实现，待评审）

- 冻结 core/registry schema、manifest、ID 和签名域。
- 冻结 coverage、RPC result 和 readiness 状态机。
- 更新 snapshot、RPC、readiness 和发布文档。

### 批次 2：Snapshot 生成与校验工具

- 生成独立 core 和 registry SQLite。
- registry 使用 `WITHOUT ROWID`。
- `create/status/list/verify/finalize/publish` 支持双 artifact 独立状态。
- 增加 schema/golden/integrity 和容量测试。

### 批次 3：Core-only 安装

- 删除 core installer 的 registry import。
- 调整 manifest validation、provenance、marker 和原子 swap。
- 保证 core 安装后立即启动 balance-history。

### 批次 4：分层 Registry Resolver

- 增加 immutable sidecar reader。
- 组合 RocksDB overlay 与 SQLite base。
- 增加 coverage、冲突、慢查询和失败隔离。
- 重构 RPC、Rust client、CLI、control-plane 和 explorer。

### 批次 5：部署和发布闭环

- 更新 object storage record、network bundle、release manifest 和 installer。
- 更新 Docker gate、controller、status、doctor 和日志。
- 增加 core-only 与 optional sidecar 的能力声明。

### 批次 6：集中验证

- deterministic/regtest、restart/reorg/fresh joiner。
- 大文件 sidecar 查询和故障注入。
- 真实主网 snapshot 的启动耗时、磁盘峰值和查询性能对比。
- 完成 release fragment 和运维文档后再进入发布评审。

## 17. 开发期切换策略

当前没有旧 snapshot/runtime 兼容需求，因此：

- 新版本直接 bump snapshot DB、manifest、release record 和 capability schema；
- 不在 installer 中同时保留“full snapshot 导入 registry”和“sidecar 查询”两套路径；
- 当前 `963800` 单文件 full snapshot 继续服务已发布的旧 release，但不作为新 schema 的合法输入；
- 新 release 应重新生成 split artifact；若开发成本需要，可以另做一次性离线转换工具，但该工具
  不进入节点运行时，也不构成长期兼容承诺。

当前远端正在进行的旧版 full snapshot 安装不应因本文而中断。它完成后仍符合对应旧 release 的
既有契约。

## 18. 验收标准

本次重构只有同时满足以下条件才算完成：

1. core snapshot 不包含 registry，安装时间不再随十亿级 registry 线性增长。
2. snapshot 节点没有 sidecar 时可以继续索引并达到 `consensus_ready=true`。
3. full replay 节点仍可从 RocksDB 完整解析其本地观察过的 script hash。
4. snapshot plus sidecar 节点能组合历史 base 和 post-snapshot overlay。
5. registry 的任何失败都不会改变 core state-ref、block commit 或 USDB 共识结果。
6. `resolve_script_hashes` 不再把 coverage 不完整误报为确定不存在。
7. build、verify、publish、install、status、doctor、restart 和 GC 均理解双 artifact 生命周期。
8. 真实主网容量测试证明 core readiness 明显早于旧 full import，且 sidecar 查询延迟满足展示和
   审计用途。
