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

# 术语映射

| 实现对象 | 规范术语 | 含义 |
| --- | --- | --- |
| BTC JSON `records[]` item | BTC activation record | 一个 BTC-side version family 在指定 BTC 高度的记录。 |
| `BtcActivationRegistryCatalog` 中一个 revision | BTC registry revision | 单个 BTC source / rules scope registry 的完整不可变快照；numeric revision 只表示 catalog 顺序。 |
| `activation_registry_id` | BTC registry identity | 完整 registry revision canonical encoding 的哈希，也是 USDB checkpoint 实际绑定的值。 |
| `ChainConfig.usdb.activations[]` item / `USDBConsensusActivation` | USDB activation checkpoint | 一个 USDB block 起生效的完整 USDB version set、BTC registry binding 和 BTC anchor max age。 |
| `ChainConfig.usdb.activations[]` | USDB activation schedule | 同一 USDB network 按 block 严格排序的全部 checkpoints。 |

本文不使用裸“每条 activation”描述实现要求。BTC record、BTC registry revision 和 USDB checkpoint 是三个不同层级，不能互换。

# 配置所有权

| 配置 | 权威来源 | Lookup context | 消费方 |
| --- | --- | --- | --- |
| BTC pass / energy / state-view versions | source / rules-scope registry | BTC source + rules scope + pinned registry ID + `btc_height` | usdb-indexer |
| BTC 余额历史与 stable frontier | source-scoped registry / bootstrap contract | BTC source + `btc_height` | balance-history；语义和 lag 必须满足 indexer 消费契约 |
| USDB chain payload / difficulty / reward policy versions | USDB chain genesis / activation schedule | USDB chain + `usdb_block` | miner、header validator、reward transition |
| 跨链 release 关联 | audit-only release manifest | release artifact identity | CI、部署工具、reviewer |

核心约束：

- indexer 加载配置 BTC source / rules scope 对应的 immutable revision catalog，并 pin 精确 current ID；历史查询可以按 ID 读取同一 catalog 的旧 revision。BTC source 相同不再隐含同一 USDB 规则域。
- USDB chain 节点只从本地 chain config 取得 expected USDB chain versions。
- USDB chain config 的每个 activation checkpoint 固定一个
  `btcActivationRegistryId` 和正数 `btcAnchorMaxAgeBlocks`；前者约束 payload
  引用的 BTC historical profile identity，后者约束 UIP-0007 同一 anchor 的连续复用。
- companion RPC 只返回 payload 指向的历史 BTC economic state，不回答 USDB chain 规则是否激活。
- control-plane 可以汇总和审计这些 identity，但不能成为共识路径上的 activation service。

# BTC Registry Artifacts

当前文件：

```text
src/btc/usdb-util/activation-registry/btc-mainnet.json
src/btc/usdb-util/activation-registry/btc-regtest.json
src/btc/usdb-util/activation-registry/btc-regtest-revision-2.json
```

schema：

```text
uip-0008-btc-activation-registry:v2
```

每个文件包含一个顶层
`scope = network_type + network_id + stable_lag_blocks`。当前 mainnet/regtest 固定
`stable_lag_blocks = 10`；该值进入 registry canonical ID，balance-history 只能从 scope
读取，禁止本地配置覆盖。同一 catalog 的后续 revision 必须保持整个 scope 不变。
public freeze 前调整 lag 会重写 draft artifact 并重生成所有下游 identity；public freeze
后则需要新的 versioned lag 语义或新网络，普通 registry revision 与 USDB activation
checkpoint 不能单独改变该值。BTC activation record 只包含一个 BTC-side version family
和 `activation_height`。文件内不得出现其他 BTC network 或 USDB-chain family。

当前 canonical ID：

```text
btc-mainnet = a6350cd6a68755ea64edf537f35c1eca4421a970e2ecfd67aaa29075aae57224
btc-regtest revision 1 (current) = bfd8c7e41ab4035db64e52eb9ea55050c08211c2ae4c2a88d8b2fc17ae1718b0
btc-regtest revision 2 (staged)  = adcca18bb4eccd4715bb0d6ec69c7b3d5e09065fac0cb33b145db7b621f59fba
```

两个 legacy registry 当前激活相同的九个 BTC v1 family，所以旧格式 `active_version_set_id` 相同；新 scoped set 不沿用这一跨域共享身份：

```text
01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691
```

testnet3、testnet4 和 signet 尚无独立 artifact，配置这些 network 时必须 fail closed，不能回退到 mainnet 或 regtest。

# 独立规则域与显式配置

新增 registry schema `uip-0008-btc-activation-registry:v3` 在原 scope 内增加 `rules_scope`。token 为 1 至 64 个 ASCII 字符，匹配 `[a-z0-9]+(?:-[a-z0-9]+)*`；`legacy` 只用于解释旧配置，不能声明为 v3 scope。这里的 source 仍然可以是 `btc-mainnet`，并不需要另建一条 Bitcoin 测试链。

外部 catalog 的结构如下，`registries` 内必须放完整且按历史顺序排列的 registry 文档：

```text
{
  "schema_version": "uip-0008-btc-activation-registry-catalog:v1",
  "current_registry_id": "<由指定 revision 规范编码计算的 64 位十六进制 ID>",
  "registries": [<完整 v3 registry revision 1>, <完整 v3 registry revision 2>]
}
```

catalog 必须保持 scope、source、schema、stable lag 一致，保留原历史记录，禁止重复 identity 和不连续的 revision 历史。文件追加 revision 不会自动切换 current；`current_registry_id` 也必须等于配置 pin。

以下是 indexer `config.json` 的 `usdb` 节选，示例 scope 仅为说明，不代表已发布网络：

```text
"usdb": {
  "genesis_block_height": 963800,
  "rules_scope": "isolated-upgrade-test",
  "activation_registry_id": "<与 catalog current_registry_id 完全相同的 ID>",
  "activation_registry_catalog_file": "/network/btc-rules-catalog.json"
}
```

- 三项都是兼容性的 optional 字段；独立 scope 必须同时提供三项，不能只设置 scope 后回退到 embedded registry。
- 未设置 scope 或设置 `legacy` 且不设置 catalog 文件时，沿用旧 embedded catalog；可以同时 pin 旧 current ID，错误 pin 必须失败。
- Rust 服务中的 catalog 相对路径按服务 root 解析。部署 renderer 要求绝对且存在的文件；发布容器通过 `/network` 只读挂载冻结 artifact。
- resolver 在打开索引数据前校验 source、scope、catalog 和 current pin；未知公式版本仍在处理对应高度前失败关闭。
- 暴露给 RPC 的新 active set 带 `scope: {network_id, rules_scope}`，使用 `usdb-active-version-set:v2` hash domain；旧 flat JSON 和 `usdb-active-version-set:v1` hash 保持不变。
- v3 registry 使用 `usdb-btc-activation-registry:v3` hash domain，scope token 参与 registry 哈希。仅区分配置目录却不区分 state identity 不满足隔离要求。

当前未新增正式 scoped catalog 或修改旧 Go golden。`generate_go_btc_activation_golden --catalog <文件> [--catalog <文件> ...] [--check] [输出路径]` 支持显式生成 scoped golden；无 `--catalog` 时保持原 legacy 输出。发布一个新 scope 仍需冻结 Go 本地支持 artifact、chain-config binding 和网络包，不能只把 JSON 放进 indexer 就声称链节点已支持它。

# 索引库与 Checkpoint 绑定

部署层的 dataset marker 之外，indexer 在 pass SQLite `state_text` 和 energy RocksDB `meta` 分别以 `indexer_rules_binding` 键持久化同一 `IndexerRulesBinding`（`usdb-indexer-rules-binding:v1`），包含 BTC source、rules scope、index origin 与精确 current registry ID。启动必须先核对两库再完成绑定；scope 正确但配入另一 revision、另一 origin 或混合两库，也必须拒绝继续运行。

- 新的空 dataset 可以首次绑定所选 scope。scoped 启动在绑定前合并两库的索引历史与持久进度检查；任一库已有索引状态而另一库缺少 binding 时必须拒绝，不能把误删 pass/energy 库解释成可自动补空或截断的恢复场景。两库都没有索引历史、仅一侧已完成首次 binding 写入时，允许重试完成初始化。
- 非空且未绑定的旧 dataset 只允许 legacy 兼容接管；已有 `snapshot_history_start_height` 时必须先核对配置 origin。缺少该旧字段时仍依赖原部署可信的 origin 配置，旧 metadata 也不能单独证明 BTC source；首次写入绑定本身不是数据来源的追溯证明。新 scope 必须使用独立数据集重建，不能借兼容接管绕过隔离。
- 已绑定数据要求全部字段匹配。只改配置、复制到新目录或改部署 marker 不能解除 DB 内部绑定。
- 同 scope catalog 可追加 revision，但本批没有实现 current ID 切换的在线 DB migration；current pin 变化须重建独立 dataset。这是持久化实现限制，与 catalog 的历史追加规则分别验收。
- checkpoint 完整复制 SQLite/RocksDB，绑定随文件库存哈希和签名进入 artifact。恢复前必须验证目标配置 pin 等于 manifest ID，双库绑定与配置相符，并离线重算 state-ref；新 scope 不接受缺少绑定的历史 checkpoint。
- scoped checkpoint 导出时在 artifact 的 `data/` 内写入 `rules-catalog.json`，该文件纳入已有 file inventory、operation ID 和 manifest 签名。standalone verify 从它恢复 catalog，并核对 manifest registry ID 与 BTC source，因此不依赖导出主机上的 catalog 路径。恢复目标仍使用本地冻结配置和双库 binding 校验，artifact 中的 catalog 不得覆盖目标 scope 或 current pin。
- legacy checkpoint 不新增 `rules-catalog.json`；checkpoint manifest schema 保持不变。旧 legacy checkpoint 可兼容缺少绑定的旧格式，恢复后由 indexer 执行 legacy 首次绑定。已存在但错误的绑定不得被覆盖。

数据源复用仅限 Bitcoin Core、满足消费者语义/lag/保留范围的 balance-history 等兼容上游。两个可写 indexer 实例不能共享同一 SQLite/RocksDB 目录。

# USDB Chain Config

go-ethereum 的 `ChainConfig.usdb.activations[]` 是 USDB activation schedule，也是 USDB chain activation 的唯一运行时来源。

schedule 中每个 `USDBConsensusActivation` 都是完整 activation checkpoint，不是单项
policy delta。`btcActivationRegistryId` 绑定允许引用的 BTC registry revision；
`btcAnchorMaxAgeBlocks` 绑定该阶段的 anchor age 上限；二者都不提供 USDB-chain
version。miner/validator 必须把它们与同一 checkpoint 的完整 `versions` 一起解析，
并在内嵌 Go golden catalog 中按 payload BTC 高度查询 expected set。

当前实现已覆盖：

- activation checkpoints 严格按 USDB block 排序且禁止同高冲突。
- `USDBConsensusAt(blockNumber)` 返回目标高度最新的完整 version set。
- `CheckCompatible` 拒绝修改已生效 checkpoint，并给出 rewind height。
- genesis JSON roundtrip 保留完整 activation schedule。
- genesis JSON roundtrip 保留每个 checkpoint 的 `btcActivationRegistryId`、
  `btcAnchorMaxAgeBlocks` 和 `btcAnchorPolicyVersion`；`CheckCompatible` 允许修改
  尚未生效的 future 参数，并拒绝修改已经生效的 binding / max age / version。
- miner、validator 和 reward transition 消费同一个 resolved chain-config profile。
- CLI 仅提供 RPC URL、timeout、selected pass 等运行参数，不能启用或覆盖共识规则。

USDB chain 不读取 Rust BTC registry JSON，也不通过 RPC 查询 expected `payload_version`、`difficulty_policy_version` 或 reward policy version。companion service 不可用时 miner/validator fail closed，是因为历史 BTC profile 不可验证，不是因为 USDB chain activation lookup 依赖 RPC。

# P2P 激活兼容识别

`go-ethereum/core/forkid.gatherForks` 在原有顶层 `*Block` 反射收集后，追加 `ChainConfig.USDB.Activations[].Block`，然后复用原排序、去重和去除 0 的逻辑。没有 USDB 配置时保持原行为。这里只收 USDB 区块高度，不读取 BTC registry 的 BTC 激活高度，也不按 checkpoint 的版本字段差异过滤条目。

调用链保持共用：

- `forkid.NewID`、`NewIDWithChain` 和 `NewFilter` / `NewStaticFilter` 使用同一 `gatherForks`。
- `eth/handler.go` 为 Status 握手计算 `NewID`，`eth/protocols/eth/handshake.go` 通过 fork filter 检查远端 Status。
- `eth/protocols/eth/discovery.go` 的 `currentENREntry` 使用 `NewID`；协议 ENR 属性与 head 事件更新共享该路径。已有 ENR updater 在 head 变化后发布更新，不新增独立激活日程。

如果 H 是旧节点 fork 列表缺失的新高度，升级节点在 H 前可接受相同 checksum、`Next=0` 的旧节点；本地 head 到达 H 后，新握手会拒绝这种旧声明。已升级但还在 H 前同步的节点可用旧 checksum、`Next=H` 证明它知道下一高度，继续追块。动态 filter 按当前 head 判断，回退跨越 H 时也应重新计算当前兼容视角。

fork ID 的 Hash 是 4 字节 CRC32，Next 是后续分叉高度。它不承诺版本字段或 registry ID；两个节点同高使用不同 registry、不同规则，或 H 恰与旧列表已有顶层 fork 同高，都可能得到相同 fork ID。链配置兼容检查、本地 registry 绑定及区块共识校验仍是最终防线。

这批改造没有建立定时重新检查现有 peer 的机制，因此已连接节点不会在 H 自动断线；它也不保证旧链停止或全网强制升级。testnet-v0 当前只有 block 0 checkpoint，去 0 后其 fork ID 保持不变。

# State Identity

```text
activation_registry_id = hash(BTC source / rules-scope registry)
active_version_set_id  = hash(BTC active version set at target height)
local_state_commit     = hash(commit_protocol_version, snapshot_id, active_version_set_id, derived_state_root)
system_state_id        = hash(snapshot_id, local_state_commit)
```

边界：

- `snapshot_id` 只承诺 upstream balance-history state。
- `snapshot_id` 的 canonical input 包含 `stable_lag`；UIP-0006 external state 显式返回
  该值，Go validator 再与本地 BTC registry scope 交叉校验。
- `activation_registry_id` 是 BTC source / rules scope registry revision identity；indexer current query 使用已 pin 的 catalog current，historical query 和 USDB chain config 可以固定同域的具体旧/新 revision。
- `active_version_set_id` 进入 local state commit，承诺目标 BTC 高度实际使用的规则；scoped set 同时承诺 source 与 rules scope，因此不同域即使公式版本相同也不会共享 local/system state identity。
- USDB activation schedule/checkpoint identity 由 USDB chain genesis / chain config 自己承诺，不合并进 BTC registry ID。

# Cross-chain Release Manifest

当前 audit artifact：

```text
src/btc/usdb-util/release-manifest.json
```

它记录：

- BTC registry artifact path、source/rules scope、revision/current 和 canonical ID；既有 legacy manifest 的字节和含义保持不变。
- USDB-chain network ID、chain ID、genesis hash、chain-config source、activation authority
  和按高度排序的完整 activation checkpoints，包括 registry binding、anchor max age 和
  全部 policy versions。

默认 embedded artifact 与 Go golden 继续使用旧 v3 schema，字节不变。新增 `uip-0008-cross-chain-release-manifest:v4` 在 BTC binding 上携带 optional `rules_scope`：legacy 省略，scoped binding 必填。它按 `(network_id, rules_scope)` 分组验证连续 revision 和唯一 current，允许同一审计文件包含 legacy 及多个独立 scope 的 catalog。同一 USDB chain 的 checkpoint 序列不能跨 source/scope；必须在结构验证阶段拒绝这种绑定。

`CrossChainReleaseManifest::validate_btc_catalog_bindings` 对 manifest 与所提供 catalogs 执行双向完整核对，拒绝缺项、额外 revision、scope/source 不匹配以及 current/revision/ID 漂移。生成工具支持：

```text
generate_go_release_manifest_golden [--manifest path] [--catalog path]... [--check] [output-path]
```

`--catalog` 要求同时指定 `--manifest`；`--check` 要求 `output-path`。默认无参数仍输出 legacy embedded artifact。只指定外部 `--manifest` 时仅执行 manifest 自身校验；需要完整 artifact 审计时必须提供该 manifest 涵盖的全部 catalogs。工具输出是审计材料，不为链节点安装 registry 或激活规则。

manifest 用于 release review、CI 和部署审计。它不得：

- 参与 BTC registry ID 或 `active_version_set_id` 计算。
- 为 USDB chain header validation 提供 expected version。
- 通过运行时 RPC 动态覆盖任一链的本地配置。

# 服务行为

## balance-history

- 按配置 BTC network 加载对应 revision catalog；未指定历史 ID 的本地路径使用 current revision。
- stable sync target 固定为 `min(max_sync_height, observed_btc_tip - stable_lag_blocks)`；
  lag 从 registry scope 读取，不再存在运行时全局 `0` 常量。
- 启动、batch 写入和历史 state-ref 查询都按目标 BTC height 校验 `balance_history_semantics_version`。
- 不解释 pass energy 或任何 USDB chain policy。

## usdb-indexer

- 启动时校验配置 source/scope/current ID、双库存储绑定、genesis height 和 durable synced height。
- 每个 block mutation 前按目标 BTC height 解析并校验完整 BTC v1 set。
- UIP-0006 external state 返回
  `stable_lag + activation_registry_id + active_version_set + active_version_set_id`。
- historical profile、candidate、breakdown 和 cursor 必须冻结相同 external state。

## go-ethereum

- 本地 chain config 决定 expected USDB chain versions。
- Rust generator 从同一 network-scoped revision catalog 生成含
  `stable_lag_blocks` 的 Go golden artifact；`--check` 模式用于 CI/release drift 检查。
- 第二个 Rust generator 将 release manifest 确定性展开为 Go golden；Go 测试把它与
  `USDBChainConfig` 和 `USDBGenesisHash` 全字段比较。
- historical profile resolver 使用 `target USDB activation checkpoint 的 BTC registry ID + payload BTC height` 本地解析 expected set，再校验 RPC registry/set identity、canonical set hash 和历史状态选择器。
- profile resolver 按 expected set 中的 raw-energy、effective-energy 和 level formula version 显式分派；本地未支持版本 fail closed。
- BTC registry identity 漂移、profile 字段篡改或 companion service 不可用时停止组块或拒绝区块。

# 测试状态

Rust registry tests 覆盖：

- per-network embedded lookup 与 v1 family surface。
- registry source / rules scope mismatch、current pin mismatch、外部 catalog 缺项与未知字段。
- 相同 BTC 输入和公式版本在不同 scope 中的 registry/active-set/state identity 隔离。
- 一个测试 scope 追加独立升级日程不改变另一个 scope 的 lookup 结果。
- legacy JSON、registry hash、active set hash 与默认 Go golden 不变。
- 双库绑定、未绑定旧库接管限制、跨 scope/origin/revision 重开拒绝，以及 checkpoint 配置/manifest/双库一致性。
- 未配置 network fail closed。
- BTC registry 拒绝 USDB chain family。
- activation boundary、duplicate height、supersedes、planned record。
- canonical record ordering、network-scoped registry ID golden。
- release manifest v3 重算全部 BTC revision ID，并固定 revision/current、USDB chain ID /
  genesis hash / authority / 完整 activation checkpoints；Go golden test 自动拒绝
  genesis、anchor age 或 policy version 漂移。

Go tests 覆盖 generated multi-revision registry/set golden、payload-height lookup、
unknown/tampered registry、active-version-set codec、per-checkpoint binding / anchor-max
边界、`CheckCompatible`、genesis roundtrip、formula dispatch、UIP-0007 parent transition、
miner/validator version guard 和 RPC failure mapping。`usdb_activation_conformance`
build tag 额外提供保留 policy `65535`，只用于验证真实第二版本分派、restart/reorg
和旧二进制 fail closed，不定义未来 production v2 公式。

# 改造批次与验收边界

第一批实现规则域、显式 catalog pin、RPC/state identity、持久化和 checkpoint 隔离，以及部署配置传递；部署 renderer、冻结 selector、catalog 只读挂载和 v0 identity 兼容由隔离测试覆盖。第二批将嵌套 USDB checkpoints 接入共用 fork ID 高度收集，覆盖激活边界、混合版本握手与 ENR 的同源行为。

两批均不激活新的 MinerPass schema、开户/来源检查/继承规则，不发布或重置 testnet-v1，不执行在线 DB migration，也不自动迁移现有节点。这里的隔离测试不能代替后续真实网络升级和新业务规则验收。已有 v0 bundle、registry artifact、激活高度和历史解释保持不变。

# 后续事项

1. 正式 BTC source network registry 的 indexing origin / activation height review。
2. 正式 USDB chain testnet/mainnet genesis、chain ID 和 activation blocks 冻结。
3. release manifest 签名和发布流程。
4. UIP-0011 至 UIP-0015 实现后，通过新的完整 USDB activation checkpoints 将 staging `0` policy 替换为正式版本。
5. 如增加 BTC testnet3/testnet4/signet，必须分别新增 registry 文件、golden ID 和 live replay matrix。
