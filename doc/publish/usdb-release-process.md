# USDB Release 总体流程

## 1. 目标与边界

本文是 USDB 多仓库 release 和上线的编排入口，不替代 UIP、组件设计或具体运维手册。当前已有
三仓 Fast CI、GHCR candidate image、跨仓 candidate manifest 和 Environment-protected GitHub Release；
最终 snapshot 合并与节点部署批准仍以可审计的人工流程为基线。

涉及的主要代码库和产物：

| 领域 | 代码库/组件 | 主要发布物 |
| --- | --- | --- |
| BTC 数据层 | `usdb` / balance-history | binary、配置、trusted-key catalog、可选 snapshot |
| BTC 协议索引 | `usdb` / usdb-indexer | binary、CLI、配置、activation registry artifact |
| 控制与部署 | `usdb` / control-plane、Docker | binary/image、compose、env template、操作脚本 |
| USDB 链节点 | `go-ethereum` | geth binary/image、genesis、chain config |
| SourceDAO | `SourceDAO` | contract artifacts、bootstrap 参数、validation report |
| BTC full node | Bitcoin Core | digest-pinned image、上游签名/checksum、SBOM |
| 外部依赖 | ord 等 | 固定版本、来源和 checksum |

## 2. Release 身份

Release ID 使用以下格式：

```text
usdb-{testnet|mainnet}-v{network-generation}-r{release-sequence}
```

- `vN` 是 network generation。改变 chain ID、genesis、block-0 identity 或其他需要清空链数据的
  网络身份时递增，并从 `r1` 重新开始。
- `rN` 是同一 network generation 上不可移动的 deployment release sequence。兼容 binary/image、
  运维配置、部署 artifact 或对未来 activation 的代码支持变化时递增。
- 在线 UIP/policy 升级由 activation matrix 的 policy version 表达，不用 `vN` 代替。
- 两仓同名 annotated tag 和 release manifest 必须使用同一个 release ID；已发布 tag 不得移动或复用。

例如 `usdb-testnet-v0-r2` 可以继续运行在 `v0` genesis 上；只有重置为新的网络身份时才进入
`usdb-testnet-v1-r1`。

每次 release 必须先冻结一份 release manifest，至少记录：

- release ID、目标网络和生成时间；
- `usdb`、`go-ethereum`、`SourceDAO` 的 commit；
- UIP/activation registry revision 和 chain config/genesis hash；
- Rust/Go/Node、Bitcoin Core、ord 和容器工具版本；
- 所有实际使用的 binary/image/config/contract/snapshot artifact 的 SHA-256 或 OCI digest；
- release 提供 snapshot 时记录 content-addressed record URL/hash、height、BTC block hash、snapshot ID、
  signer ID 和 trusted-key catalog SHA-256；节点是否采用它仍属于本地显式选择；
- 负责构建、复核和批准发布的人员；
- 已运行的测试集合及原始报告路径。

`go-ethereum/scripts/usdb/ci-revisions.json` 可以作为最近一次联合验证基线，但不是自动跟随
HEAD 的 release manifest，也不是共识输入。正式 release 必须显式冻结自身 revision 集合。

## 3. 发布阶段

### 3.1 参数冻结

状态：`partial/candidate`

- 冻结目标网络、chain ID、genesis、activation registry binding；
- 冻结 SourceDAO bootstrap 参数和 system addresses；
- 选择 balance-history `full-sync`、fresh-indexer `signed-snapshot` 或 `paired-checkpoint`；artifact 模式在
  deployment release 中冻结 signer、catalog 和所选 artifact 的高度/hash，但 checkpoint 高度不进入
  network/genesis identity；
- 每个 network bundle 独立冻结自己的 BTC `index_origin_height`。全新 indexer 使用 snapshot 时，
  snapshot 高度不得高于该网络的 origin；推荐发布恰好位于 origin 的 full-UTXO snapshot。更高 snapshot
  必须配对已签名 indexer checkpoint，并通过恢复后的历史 state-ref 重算；
- 冻结镜像 tag、binary version 和配置 schema；
- 列出 development-only、fake policy 和未激活功能，确认不会误入 public profile。
- 将 bootstrap admin 明确分类为 development fixture、testnet signer 或 mainnet signer；testnet 与
  mainnet 各自使用独立托管身份，public bundle 只记录公开地址并拒绝已知 development admin。

### 3.2 可重复构建

状态：`implemented/candidate`

- Fast CI 成功后从主分支冻结 commit 构建 GHCR candidate images；
- 记录工具链和外部依赖版本；
- 生成 artifact hash；
- 禁止从未记录的本地 binary 或 mutable host path 直接发布；
- 对跨仓库生成物执行 golden vector 和 roundtrip 校验。
- Bitcoin Core release image 验证三个固定上游 signer，并单独发布 provenance/SBOM。

镜像 workflow、digest/attestation 和跨仓 candidate manifest 见
[GitHub CI 镜像与跨仓 Release 发布](./github-ci-image-and-release-publishing.md)。
Bitcoin 独立生命周期和同步门禁见
[Bitcoin Core Release Image 与同步操作](./bitcoin-core-release-and-sync-operations.md)。
Snapshot 与 fresh-indexer/paired-checkpoint 的兼容边界见
[Balance-history Snapshot 与 Indexer Checkpoint 兼容规则](./balance-history-indexer-checkpoint-compatibility.md)。

### 3.3 组件测试

状态：`implemented/manual`

- Rust unit/integration、clippy 和脚本 ShellCheck；
- Go unit/integration、USDB profile/activation/reward E2E；
- SourceDAO tests 和 bootstrap validation；
- deterministic regtest、reorg/restart/replay、world simulator；
- 目标硬件容量、PoW 校准和真实 BTC 历史数据测试。

具体命令继续由各组件测试矩阵维护，release manifest 只引用执行报告，不复制测试说明。

### 3.4 Artifact 签名与信任引导

状态：`partial`

- Snapshot 使用独立 Ed25519 signer 签署 manifest；
- balance-history release bundle 可以携带 public trusted-key catalog，但 full-sync 不依赖 catalog 建立状态；
- SourceDAO/genesis 等其他 artifact 使用各自冻结的签名与 hash 方案；
- OCI candidate image 已生成 GitHub provenance attestation；GitHub Release 已固定并复核 asset digest，
  但 binary 和最终 GitHub Release 独立签名仍待落地，
  且不得复用 snapshot 私钥；
- 私钥不进入 Git、普通 CI、普通节点镜像或公开 release bundle。

### 3.5 Staging/Devnet 演练

状态：`manual`

- 使用最终 artifact，不从 workspace 重新构建；
- 执行 bootstrap、restart、joiner；选用 paired checkpoint 时再执行中断续跑和恢复后 state-ref 门禁；
- 验证 activation 边界、SourceDAO readiness、reward/fee/system state；
- 记录完整服务版本、state-ref、genesis hash、snapshot provenance 和 RPC readiness；
- 执行失败恢复和回滚演练。

### 3.6 Public 发布与上线

状态：`blocked`

在正式 CI、release signing、public network 参数和上线审批流程冻结前，不能把开发期构建称为
public release。上线时至少需要：

1. 发布 release manifest、binary/image digest、trusted public catalogs 和操作手册；
2. 初始化或导入各节点数据；
3. 按 bootstrap、validator/miner、普通 joiner 顺序启动；
4. 执行链上和链下 readiness 检查；
5. 确认观察窗口内没有 activation、reward、fee、snapshot provenance 或 state-ref 异常；
6. 保存发布证据并明确后续升级/回滚入口。

## 4. 发布门禁

以下任一项不满足时不得发布：

- revision、activation、genesis 或 SourceDAO 参数未冻结；
- worktree 含未审计修改，或 artifact 无法追溯到 commit；
- snapshot/private bootstrap key 泄露到 bundle、image、日志或 Git；
- testnet/mainnet bootstrap admin 使用已知 development 地址，或 mainnet 复用 testnet signer；
- 选择 snapshot 时，trusted-key catalog 没有独立可信 hash 来源；
- 必要测试未运行，或报告只来自开发 mock；
- joiner 无法从公开 artifact 完成冷启动；
- 没有磁盘、内存、PoW 或服务不可用时的停止条件；balance-history 至少应通过
  `compose.test-32gb.yml` 的显式 cache/cgroup 基线测试；
- Bitcoin Core 不是 mainnet full node、`txindex` 未同步到 tip，或 RPC 暴露到公网；
- 没有回滚、撤包、key compromise 或错误 genesis 的处置人和入口。

## 5. 下一步文档拆分

随着流程落地，本目录建议继续增加：

- `release-manifest-promotion.md`：复核 candidate 已冻结的 snapshot identity，补齐测试证据和批准记录后发布；
- `usdb-chain-genesis-release.md`：genesis、chain config 和 activation binding 发布；
- `sourcedao-release-and-bootstrap.md`：合约 artifact 和 bootstrap；
- `github-ci-image-and-release-publishing.md`：OCI candidate build、SBOM、digest、attestation 和跨仓 manifest；
- `public-network-launch-runbook.md`：上线时间线、角色、检查和回滚；
- `release-key-management.md`：不同 signer 的存储、轮换和失陷处理。

这些文档应在对应实现或参数即将冻结时补齐，不提前虚构尚不存在的自动化。
