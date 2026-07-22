Title: UIP-0008 Activation Registry Implementation Notes
Status: Working Notes
Related: UIP-0008, UIP-0009
Created: 2026-04-26

# 摘要

本文记录 UIP-0008 在当前多服务架构中的实现边界。当前结论是：

```text
按共识所有权分别定义，运行时本地解析，发布时用 manifest 关联
```

不再维护同时包含 BTC、USDB chain 和多个网络的全局 runtime registry。

# 配置所有权

| 配置 | 权威来源 | Lookup context | 消费方 |
| --- | --- | --- | --- |
| BTC pass / energy / state-view versions | network-scoped BTC registry | BTC network + `btc_height` | balance-history、usdb-indexer |
| USDB chain payload / difficulty / reward policy versions | USDB chain genesis / `ChainConfig.usdb.activations[]` | USDB chain + `usdb_block` | miner、header validator、reward transition |
| 跨链 release 关联 | audit-only release manifest | release artifact identity | CI、部署工具、reviewer |

核心约束：

- BTC 服务只加载配置 network 对应的一个 registry 文件。
- USDB chain 节点只从本地 chain config 取得 expected USDB chain versions。
- companion RPC 只返回 payload 指向的历史 BTC economic state，不回答 USDB chain 规则是否激活。
- control-plane 可以汇总和审计这些 identity，但不能成为共识路径上的 activation service。

# BTC Registry Artifacts

当前文件：

```text
src/btc/usdb-util/activation-registry/btc-mainnet.json
src/btc/usdb-util/activation-registry/btc-regtest.json
```

schema：

```text
uip-0008-btc-activation-registry:v1
```

每个文件包含一个顶层 `scope = network_type + network_id`，record 只包含 BTC-side version family 和 `activation_height`。文件内不得出现其他 BTC network 或 USDB-chain family。

当前 canonical ID：

```text
btc-mainnet = bb751626eb1415bbc349e77f58cb412908584842cbf7d786262b7bd1f6a7d39e
btc-regtest = 22d820e6ec242b61f63473f279c41a4103af5cff13206b1925fd415cceaaf83d
```

两个 registry 当前激活相同的九个 BTC v1 family，所以 `active_version_set_id` 相同：

```text
01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691
```

testnet3、testnet4 和 signet 尚无独立 artifact，配置这些 network 时必须 fail closed，不能回退到 mainnet 或 regtest。

# USDB Chain Config

go-ethereum 的 `ChainConfig.usdb.activations[]` 是 USDB chain activation 的唯一运行时来源。

当前实现已覆盖：

- activation record 严格按 USDB block 排序且禁止同高冲突。
- `USDBConsensusAt(blockNumber)` 返回目标高度最新的完整 version set。
- `CheckCompatible` 拒绝修改已生效 activation，并给出 rewind height。
- genesis JSON roundtrip 保留完整 activation records。
- miner、validator 和 reward transition 消费同一个 resolved chain-config profile。
- CLI 仅提供 RPC URL、timeout、selected pass 等运行参数，不能启用或覆盖共识规则。

USDB chain 不读取 Rust BTC registry JSON，也不通过 RPC 查询 expected `payload_version`、`difficulty_policy_version` 或 reward policy version。companion service 不可用时 miner/validator fail closed，是因为历史 BTC profile 不可验证，不是因为 USDB chain activation lookup 依赖 RPC。

# State Identity

```text
activation_registry_id = hash(network-scoped BTC registry)
active_version_set_id  = hash(BTC active version set at target height)
local_state_commit     = hash(commit_protocol_version, snapshot_id, active_version_set_id, derived_state_root)
system_state_id        = hash(snapshot_id, local_state_commit)
```

边界：

- `snapshot_id` 只承诺 upstream balance-history state。
- `activation_registry_id` 是 BTC source-network registry 的审计 identity。
- `active_version_set_id` 进入 local state commit，承诺目标 BTC 高度实际使用的规则。
- USDB chain-config activation identity 由 USDB chain genesis / chain config 自己承诺，不合并进 BTC registry ID。

# Cross-chain Release Manifest

当前 audit artifact：

```text
src/btc/usdb-util/release-manifest.json
```

它记录：

- BTC registry artifact path、network scope 和 canonical ID。
- USDB-chain network ID、chain ID、genesis hash、chain-config source 和 activation authority。

manifest 用于 release review、CI 和部署审计。它不得：

- 参与 BTC registry ID 或 `active_version_set_id` 计算。
- 为 USDB chain header validation 提供 expected version。
- 通过运行时 RPC 动态覆盖任一链的本地配置。

# 服务行为

## balance-history

- 按配置 BTC network 加载对应 registry。
- 启动、batch 写入和历史 state-ref 查询都按目标 BTC height 校验 `balance_history_semantics_version`。
- 不解释 pass energy 或任何 USDB chain policy。

## usdb-indexer

- 启动时校验配置 genesis height 和 durable synced height。
- 每个 block mutation 前按目标 BTC height 解析并校验完整 BTC v1 set。
- UIP-0006 external state 返回 `activation_registry_id + active_version_set + active_version_set_id`。
- historical profile、candidate、breakdown 和 cursor 必须冻结相同 external state。

## go-ethereum

- 本地 chain config 决定 expected USDB chain versions。
- historical profile resolver 校验 BTC registry/set identity 的格式、自洽性及同一次 replay 内稳定性。
- BTC registry identity 漂移、profile 字段篡改或 companion service 不可用时停止组块或拒绝区块。

# 测试状态

Rust registry tests 覆盖：

- per-network embedded lookup 与 v1 family surface。
- registry scope mismatch。
- 未配置 network fail closed。
- BTC registry 拒绝 USDB chain family。
- activation boundary、duplicate height、supersedes、planned record。
- canonical record ordering、network-scoped registry ID golden。
- release manifest 重算 BTC registry ID，并固定 USDB chain ID / genesis hash / authority。

Go tests继续覆盖 active-version-set codec/golden、chain-config activation boundary、`CheckCompatible`、genesis roundtrip、miner/validator version guard 和 RPC failure mapping。

# 后续事项

1. 正式 BTC source network registry 的 indexing origin / activation height review。
2. 正式 USDB chain testnet/mainnet genesis、chain ID 和 activation blocks 冻结。
3. release manifest 签名和发布流程。
4. UIP-0011 至 UIP-0015 实现后，将 USDB chain staging `0` policy 替换为正式 activation records。
5. 如增加 BTC testnet3/testnet4/signet，必须分别新增 registry 文件、golden ID 和 live replay matrix。
