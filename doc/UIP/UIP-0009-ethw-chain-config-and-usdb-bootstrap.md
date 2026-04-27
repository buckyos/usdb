UIP: UIP-0009
Title: ETHW Chain Config and USDB Bootstrap
Status: Draft
Type: Standards Track
Layer: ETHW Chain Config / Genesis / Consensus
Created: 2026-04-26
Requires: UIP-0000, UIP-0005, UIP-0007, UIP-0008
Activation: ETHW network activation matrix; first official networks activate v1 from genesis

# 摘要

本文定义 USDB ETHW 链的 chain config、genesis、PoW bootstrap 参数和 USDB 共识扩展版本字段。

UIP-0009 的目标是把 USDB 作为一条新的 ETHW-compatible PoW 链从 genesis 启动，而不是继续保留“从既有 Ethereum / ETHW 网络迁移”的历史语义。

本文只定义 chain config 和 bootstrap 边界，不定义具体 reward、fee split、CoinBase emission、collab bonus 或 price 公式。SourceDAO / Dividend 冷启动由 UIP-0010 定义，相关经济公式由后续 UIP 定义，并通过本文定义的 version 字段在 ETHW chain config 中激活。

# 动机

当前 go-ethereum 代码基线来自 ETHW fork，仍保留大量历史迁移字段：

- Merge / terminal total difficulty 过渡字段。
- ETHW 从 Ethereum 分叉时使用的 fork block 和 chain id switch。
- 传统 Ethereum difficulty bomb 路径。
- 主网历史 fork 高度。

USDB 目标是一条新的链：

- 从 genesis 即使用 PoW。
- 从 genesis 即启用 USDB profile selector。
- 从 genesis 即启用 USDB level-based difficulty policy。
- 不兼容既有 Ethereum / ETHW 主网历史。

因此，USDB 需要一套独立 chain config，而不是把已有 ETHW mainnet 配置改几个参数后继续复用。

# 非目标

本文不定义：

- `ProfileSelectorPayload` 的二进制格式，见 UIP-0007。
- `level -> difficulty_factor_bps` 公式，见 UIP-0005。
- `base_difficulty -> real_difficulty` 的完整 difficulty policy 公式。
- reward、fee split、CoinBase emission、uncle reward、collab bonus 或分红池公式。
- SourceDAO / dividend pool 的合约初始化细节，见 UIP-0010。
- public mainnet / public testnet 最终 ChainID、NetworkId 或 bootnodes。

# 术语

| 术语 | 含义 |
| --- | --- |
| `USDBChainConfig` | USDB ETHW 链专用 chain config。 |
| `USDBGenesis` | USDB ETHW 链 genesis 配置。 |
| `payload_version` | UIP-0007 `ProfileSelectorPayload` 二进制布局版本。 |
| `difficulty_policy_version` | `level -> real difficulty` ETHW 共识算法版本。 |
| `reward_rule_version` | ETHW reward 计算规则版本。 |
| `fee_split_policy_version` | ETHW fee split / dividend pool 规则版本。 |
| `genesis_activation` | 某规则从 genesis / block 0 起生效。 |
| `legacy_ethw_migration_fields` | ETHW fork 旧链迁移字段，例如 `EthPoWForkBlock`、`ChainID_ALT`、`TerminalTotalDifficulty`。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# Chain Identity

USDB ETHW 链必须拥有独立身份：

| 字段 | 要求 |
| --- | --- |
| `ChainID` | 必须独立于 Ethereum、ETHW 和其他公开网络。 |
| `NetworkId` | 必须独立，默认应等于或明确映射到 `ChainID`。 |
| `GenesisHash` | 必须由 USDB genesis 唯一确定。 |
| `NetworkName` | 应使用 `usdb` 或后续确认的公开网络名。 |
| `Bootnodes` | 开发期可以为空；公开网络必须显式定义。 |

当前 go-ethereum 原型使用：

```text
USDBNetworkID = 20260323
USDBGenesisDifficulty = 8192
USDBMinimumDifficulty = 8192
MaximumExtraDataSize = 160
P2P default port for --usdb = 31303
USDBBootnodes = []
USDBV5Bootnodes = []
```

这些值是当前实现基线。`ChainID`、`NetworkId` 和 network name 可以沿用当前原型命名继续开发，但不自动等同于 public mainnet final 参数。进入 public testnet / mainnet 前必须重新核实并冻结。

# Genesis Policy

USDB v1 genesis 应满足：

- `config = USDBChainConfig`。
- `difficulty = USDBGenesisDifficulty`。
- `alloc = {}`，默认无预挖、无普通账户预分配。
- `extraData` 可包含短文本标识，但不得承载协议状态。
- `timestamp`、`gasLimit`、`baseFee` 等字段必须在最终 genesis 文件中固定。

例外：

- 如果 SourceDAO、dividend pool 或系统合约需要 genesis predeploy，必须由对应 UIP 或 bootstrap 文档单独定义。
- 一旦 genesis 包含系统合约 code/storage，这些 code/storage 必须进入 genesis hash，且 public network 不得再静默修改。

# USDBGenesisHash 生成与作用

`USDBGenesisHash` 是 USDB ETHW 网络的 genesis block hash。它不是人工指定的业务 id，而是由最终 genesis spec 生成 block 0 后得到的区块 hash。

当前 go-ethereum 原型中的生成口径等价于：

```text
USDBGenesisHash = DefaultUSDBGenesisBlock().ToBlock().Hash()
```

测试中也必须用以下两条路径交叉校验同一个值：

```text
DefaultUSDBGenesisBlock().ToBlock().Hash()
DefaultUSDBGenesisBlock().MustCommit(memory_db).Hash()
```

## 直接进入 genesis hash 的数据

`USDBGenesisHash` 直接绑定 genesis block header 以及由 `alloc` 派生出的 state root。实际影响 hash 的内容包括：

- genesis header 字段：
  - `nonce`
  - `timestamp`
  - `extraData`
  - `gasLimit`
  - `difficulty`
  - `mixHash`
  - `coinbase`
  - `baseFeePerGas`，当 genesis 所采用的 chain config 在 block 0 启用 London 时必须固定。
  - `parentHash`、`number`、`gasUsed` 等 block 0 固定字段。
- genesis `alloc` 派生的 state root：
  - 每个预置账户地址。
  - 每个账户的 `balance`。
  - 每个账户的 `nonce`。
  - 每个账户的 runtime `code`。
  - 每个账户的 `storage`。

因此，如果正式网络采用 SourceDAO / Dividend system contract predeploy，则以下内容都会通过 `alloc` 进入 `USDBGenesisHash`：

- `DaoAddress`。
- `DaoAddress` 对应的 runtime code。
- `DividendAddress`。
- `DividendAddress` 对应的 runtime code。
- `bootstrapAdmin` 的初始余额，如果该余额在 genesis 中预置。
- 任何直接写入 genesis 的 system contract storage。

## 不直接进入 genesis hash 但必须同时冻结的数据

`ChainConfig` 本身通常不是 genesis block header 的直接 RLP 字段。go-ethereum 会把 chain config 以 genesis hash 为 key 写入数据库，并在后续共识校验中使用它。

因此，下列字段不一定直接改变 `USDBGenesisHash`，但必须与 genesis hash 一起作为同一份 public network release artifact 冻结：

- `ChainID` / `NetworkId`。
- EVM fork schedule。
- `EthPoWForkBlock` / `ChainID_ALT` 等遗留兼容字段的 USDB 填充值。
- `payload_version`、`difficulty_policy_version`、`reward_rule_version`。
- `MaximumExtraDataSize`。
- `DividendAddress`、`DividendFeeSplitBlock`、`fee_split_policy_version` 等后续 fee split hook。
- bootnodes / DNS discovery release 列表。

如果这些字段在同一个 `USDBGenesisHash` 下被不同节点配置成不同值，节点可能在后续出块或验证时发生共识分叉。因此 public testnet / mainnet 发布时，必须同时发布：

```text
canonical_genesis_json
canonical_chain_config
USDBGenesisHash
release_manifest_hash_or_signature
```

## bootstrap 参数与 genesis hash 的边界

启动后的 SourceDAO 初始化交易不直接进入 `USDBGenesisHash`，除非它们的结果被预先写成 genesis `alloc.storage`。

例如：

- `Dao.initialize()` 作为 block 0 之后的交易执行时，不进入 genesis hash。
- `Dividend.initialize(...)` 作为 block 0 之后的交易执行时，不进入 genesis hash。
- `Dao.setTokenDividendAddress(...)` 作为 block 0 之后的交易执行时，不进入 genesis hash。

这些 post-start bootstrap 交易由 UIP-0010 定义，并通过 release manifest、bootstrap state marker、交易 hash 或激活高度审计。

如果未来选择把初始化后的 storage 直接写入 genesis，则对应 storage 必须进入 `alloc`，并会改变 `USDBGenesisHash`。

## 更新要求

在以下内容 finalization 之后，必须重新生成并更新 `USDBGenesisHash`：

- `DefaultUSDBGenesisBlock()` 的任意字段。
- public network 的 canonical genesis JSON。
- `GenesisDifficulty`、`gasLimit`、`timestamp`、`extraData`、`baseFeePerGas` 等 genesis header 字段。
- system contract predeploy 地址、runtime code、初始余额或 storage。
- bootstrap admin 初始余额，如果该余额在 genesis 中预置。

如果只修改 chain config 中不直接进入 genesis header/state root 的字段，也必须重新发布 chain config manifest，并评估是否需要新网络或显式 fork activation；不得在已发布网络上静默修改。

# EVM Fork Schedule

USDB 是新链，不需要重放 Ethereum 历史 fork 时间线。

v1 应从 genesis 固定启用现代 EVM 规则集。当前 go-ethereum 原型已经将以下 fork block 设为 `0`：

- `HomesteadBlock`
- `EIP150Block`
- `EIP155Block`
- `EIP158Block`
- `ByzantiumBlock`
- `ConstantinopleBlock`
- `PetersburgBlock`
- `IstanbulBlock`
- `BerlinBlock`
- `LondonBlock`
- `ShanghaiBlock`

未来如果需要启用新的 EVM fork，应通过 ETHW chain config 和 UIP-0008 activation matrix 显式定义。

# PoW 与 Difficulty Bootstrap

USDB v1 必须长期使用 PoW，不使用 Merge / PoS transition 语义。

要求：

- 禁止 difficulty bomb。
- difficulty 从 genesis 起走无炸弹 PoW 路径。
- 不使用 ETHW 从旧链 fork 时的 difficulty reset 特判。
- `GenesisDifficulty` 和 `MinimumDifficulty` 必须显式低于传统 ETHW 默认值，以适应早期低算力网络。

当前原型基线：

```text
USDBGenesisDifficulty = 8192
USDBMinimumDifficulty = 8192
DifficultyBoundDivisor = 2048
DurationLimit = 13
```

说明：

- `8192` 是开发期 bring-up 值，不是 public mainnet final 值。
- 本地测试已经发现 `8192` 可能过低，虽然 ETHW difficulty 会自动调整，但从过低起点恢复到合理区间可能较慢。
- public testnet / mainnet 参数必须通过私链和测试网出块数据确认。
- 若后续引入 level-based difficulty，`base_difficulty` 仍由 ETHW PoW difficulty policy 产生，USDB level 只参与折算。

## Difficulty Retarget 参数

当前 go-ethereum 原型继续沿用：

```text
DifficultyBoundDivisor = 2048
DurationLimit = 13
```

语义：

- `DifficultyBoundDivisor` 控制每个 block 难度调整步长，核心项是 `parent_difficulty / DifficultyBoundDivisor`。值越小，单块调整幅度越大；值越大，难度变化越平滑但响应更慢。
- `DurationLimit` 是旧 Frontier difficulty 路径中的出块时间阈值：当新区块时间间隔低于该值时提高难度，否则降低难度。
- 当前 USDB 使用的 ETHW no-bomb 路径主要采用 Byzantium 风格的时间项，其中核心公式包含 `((timestamp - parent.timestamp) // 9)` 和 `parent_difficulty / 2048`；因此 `DifficultyBoundDivisor` 仍直接影响调整速度，`DurationLimit` 主要保留为旧路径兼容参数。

v1 建议：

- 优先调参 `GenesisDifficulty` 和 `MinimumDifficulty`，不要先修改 `DifficultyBoundDivisor` 或 `DurationLimit`。
- 只有当测试网数据证明 difficulty retarget 曲线本身不合适时，才单独调整 `DifficultyBoundDivisor` 或 difficulty policy。
- `GenesisDifficulty` / `MinimumDifficulty` 的正式值必须作为 public testnet / mainnet finalization 的待办事项。

# Legacy ETHW Migration Fields

USDB 不应依赖旧 ETHW 迁移语义。

正式语义：

- `TerminalTotalDifficulty` 必须为空或被忽略。
- `TerminalTotalDifficultyPassed` 不得影响 USDB 链。
- `EthPoWForkBlock` 不应表示“从旧链切换到 ETHW”的迁移点。
- `ChainID_ALT` 不应承载 USDB 链的第二套 replay protection id。

如果当前 go-ethereum 代码结构仍要求这些字段存在，v1 可以使用兼容填充值：

```text
EthPoWForkBlock = 0
EthPoWForkSupport = true
ChainID_ALT = ChainID
TerminalTotalDifficulty = nil
```

这些值的含义是“USDB 从 genesis 就是单一 PoW 链”，不是 ETHW fork 迁移。

# Header Extra Capacity

UIP-0007 `ProfileSelectorPayload` v1 固定长度为 `107 bytes`。

ETHW chain config / protocol params 必须允许 `header.Extra` 容纳该 payload。v1 固定：

```text
MaximumExtraDataSize = 160
```

v1 规则：

- USDB reward / difficulty consensus 激活后，普通区块 `header.Extra` 必须正好等于 UIP-0007 payload。
- `MaximumExtraDataSize` 固定为 `160`，作为当前 v1 的共识上限和未来小幅扩展余量。
- validator 必须按 UIP-0007 的精确 payload size 校验当前版本，不能因为上限是 `160` 就接受任意长度。

# USDB Consensus Version Fields

USDB chain config 必须显式包含或可确定以下版本：

| 字段 | 类型建议 | v1 值 | 激活 |
| --- | --- | --- | --- |
| `payload_version` | uint8 | `1` | genesis |
| `difficulty_policy_version` | uint16 | `1` | genesis |
| `reward_rule_version` | uint16 | `1` | genesis |
| `fee_split_policy_version` | uint16 / optional | `TBD` | genesis 或后续 UIP |
| `coinbase_emission_policy_version` | uint16 / optional | `TBD` | genesis 或后续 UIP |

边界：

- `payload_version` 只描述 `header.Extra` payload 编码。
- `difficulty_policy_version` 描述 ETHW 如何把 UIP-0005 的 `difficulty_factor_bps` 应用到 PoW difficulty。
- `reward_rule_version` 描述 ETHW reward 规则。
- fee split 和 CoinBase emission 由后续 UIP 定义，本文只保留 chain config hook。

首个正式 ETHW 网络必须从 genesis 启用 `difficulty_policy_version = 1`，不得使用 `0` 表示未启用。

# USDB Companion Service 依赖

USDB ETHW miner 和 validator 都依赖本地 USDB companion service。

要求：

- miner 没有 USDB state view 时，不得继续生产 USDB consensus block。
- validator 无法按 payload 历史 context 重放 USDB profile 时，必须 fail closed。
- validator 必须校验 `payload_version`、`difficulty_policy_version` 和 chain config expected version。
- validator 不得查询 USDB current head 来验证旧块。

当前 go-ethereum 原型已有：

- miner-side USDB payload builder。
- validator-side USDB reward verifier。
- `--miner.usdb.*` flags。
- `--ethash.usdb.*` flags。

这些实现仍使用旧 `RewardPayloadV1` 命名和 105-byte payload，正式实现必须迁移到 UIP-0007 `ProfileSelectorPayload` 107-byte 结构。

# Reward / Difficulty / Fee Split 边界

本文只定义 chain config 版本入口，不定义公式。

推荐边界：

- `difficulty_policy_version = 1`：由 ETHW 使用 UIP-0005 的 `difficulty_factor_bps` 折算 block difficulty。
- `reward_rule_version = 1`：由后续 reward / CoinBase UIP 定义。
- `fee_split_policy_version`：由后续 fee split / dividend UIP 定义。

旧 go-ethereum 备忘中的 reward-only、mock level、`0.5..2.0` multiplier 等设计，只能作为实现历史参考。正式规则以后续 UIP 为准。

# SourceDAO / Dividend Bootstrap 边界

SourceDAO / dividend pool 与 fee split 是同一个冷启动问题，不应在 UIP-0009 内直接定义完整合约流程。该流程由 UIP-0010 处理。

当前 docker / go-ethereum 原型已经形成开发期流程：

1. `bootstrap-init` 复制并校验 ETHW genesis artifact、manifest、SourceDAO bootstrap config。
2. `ethw-init` 使用 canonical genesis 执行 `geth init`，并写入 genesis hash marker。
3. `ethw-node` 启动 USDB ETHW 节点。
4. `sourcedao-bootstrap` 等待 ETHW RPC ready 后，调用 SourceDAO 工作区脚本完成 `Dao` / `Dividend` bootstrap，并写入 state / marker。

该流程说明：

- 现在已经有完整开发期部署链路，但它依赖本地 SourceDAO workspace 和外部 bootstrap config，尚不是协议标准。
- 正式协议应单独定义内置系统地址、SourceDAO / Dividend code 来源、bootstrap admin、初始化交易顺序、`DividendFeeSplitBlock` 激活高度和审计 artifact。
- UIP-0009 只保留 chain config hook，例如 `fee_split_policy_version`、`DividendAddress`、`DividendFeeSplitBlock` 等字段的占位边界。
- 是否从 genesis 预置 SourceDAO / Dividend code，以及是否在启动后由 bootstrap 交易初始化，应由单独 UIP 标准化。

UIP-0010 的核心原则暂定为：

```text
fixed_system_addresses
    -> genesis predeploy runtime code
    -> post-start bootstrap initialization transactions
    -> fee split activation height
```

# Bootstrap Network Defaults

开发期建议：

- `--usdb` 使用独立 p2p 默认端口 `31303`。
- HTTP / WS / Auth RPC 端口可暂时沿用 go-ethereum 默认值。
- bootnodes / DNS discovery 初期为空。
- 多机 devnet 使用外部 `bootnodes.txt` 或 `static-nodes.json`。

公开网络要求：

- public testnet / mainnet 必须定义稳定 bootnodes 或明确的发现机制。
- network id、genesis hash 和 bootnodes 必须在 release artifact 中固定。
- 节点不得自动连接 Ethereum / ETHW 默认 bootnodes。

bootnodes / DNS discovery 与公开网络冷启动流程绑定，当前作为 public network finalization 待办事项。后续需要覆盖：

- 第一批 bootnode 的生成、签名和发布方式。
- 新 joiner 如何取得 canonical genesis、bootnodes、trusted manifest 和 SourceDAO bootstrap 状态。
- bootnodes 何时进入内置配置，何时只作为外部 release artifact 分发。

# Activation

USDB 是新链。首个 public testnet 和 mainnet 上线时，v1 chain config 和 USDB consensus 版本应从 genesis / block 0 激活：

```text
payload_version = 1
difficulty_policy_version = 1
reward_rule_version = 1
```

不需要保留 pre-standard ETHW 兼容窗口。

未来升级必须通过 UIP-0008 activation matrix 和 ETHW chain config fork/version 字段定义。

# 与 UIP-0008 Activation Registry 的关系

UIP-0008 定义 activation registry 的通用机制。UIP-0009 定义 ETHW chain config 中必须进入 registry 或 chain config 的 USDB 字段。

建议：

- `payload_version`、`difficulty_policy_version`、`reward_rule_version` 必须进入 ETHW chain config 或 activation registry。
- `activation_registry_id` 可以先作为审计字段暴露。
- 当 activation registry canonical encoding 固定后，ETHW 节点启动时应校验 expected registry id。

# Chain Config 示例

示例只表达结构，不是 public network final 参数：

```json
{
  "chainId": 20260323,
  "networkId": 20260323,
  "homesteadBlock": 0,
  "eip150Block": 0,
  "eip155Block": 0,
  "eip158Block": 0,
  "byzantiumBlock": 0,
  "constantinopleBlock": 0,
  "petersburgBlock": 0,
  "istanbulBlock": 0,
  "berlinBlock": 0,
  "londonBlock": 0,
  "shanghaiBlock": 0,
  "ethPoWForkBlock": 0,
  "ethPoWForkSupport": true,
  "chainIdAlt": 20260323,
  "terminalTotalDifficulty": null,
  "usdb": {
    "payloadVersion": 1,
    "difficultyPolicyVersion": 1,
    "rewardRuleVersion": 1,
    "maximumExtraDataSize": 160,
    "genesisDifficulty": "8192",
    "minimumDifficulty": "8192"
  }
}
```

# 实现影响

go-ethereum:

- `params/config.go`
- `params/protocol_params.go`
- `params/bootnodes.go`
- `core/genesis.go`
- `cmd/geth`
- `cmd/utils`
- `consensus/ethash`
- `internal/usdb`
- `miner`

USDB:

- UIP-0008 activation registry。
- UIP-0006 state view registry / version exposure。
- regtest / E2E scripts。

# 测试要求

至少需要覆盖：

- `--usdb` 使用 `USDBChainConfig`。
- `DefaultUSDBGenesisBlock()` hash stable。
- `ChainID == NetworkId` 或明确映射。
- `ChainID_ALT == ChainID` 的兼容填充值不触发 chain id switch。
- no Merge transition。
- no difficulty bomb path。
- USDB minimum difficulty 使用 `USDBMinimumDifficulty`。
- `MaximumExtraDataSize == 160`。
- 当前 payload 精确长度校验为 `107`。
- `payload_version` mismatch fail closed。
- `difficulty_policy_version` mismatch fail closed。
- USDB reward / difficulty versions 从 genesis 生效。
- bootnodes 默认不连接 Ethereum / ETHW 网络。
- public network 缺少 bootnodes / network id / genesis hash 时不得发布。

# 与旧 go-ethereum 备忘的关系

本文参考以下实现备忘，但以当前 UIP 结论为准：

- `/home/bucky/work/go-ethereum/docs/usdb/usdb-chain-bootstrap-notes.md`
- `/home/bucky/work/go-ethereum/docs/usdb/usdb-ethw-reward-integration.md`
- `/home/bucky/work/go-ethereum/docs/usdb/usdb-ethw-reward-e2e-plan.md`
- `/home/bucky/work/go-ethereum/docs/usdb/usdb-pass-level-difficulty-and-collab-bonus.md`
- `/home/bucky/work/go-ethereum/docs/usdb/usdb-ethw-fee-split-integration.md`

其中已经被当前 UIP 更新的旧结论包括：

- `RewardPayloadV1` 应迁移为 `ProfileSelectorPayload`。
- payload v1 长度从原型 `105 bytes` 调整为 `107 bytes`。
- `difficulty_policy_version` 进入 payload 作为显式承诺，但 expected version 来自 chain config。
- level / real difficulty 由 UIP-0005 和 ETHW difficulty policy 定义。

# 待审计问题

1. public testnet / mainnet 的最终 `ChainID`、`NetworkId` 和 network name。当前可延续原型命名，正式上线前必须复核。
2. public testnet / mainnet 的最终 `GenesisDifficulty` 和 `MinimumDifficulty`。当前 `8192` 是开发期值，且本地测试显示可能过低，需要专项调参。
3. `DifficultyBoundDivisor` 和 `DurationLimit` 是否沿用当前值。v1 暂建议优先不改，除非测试网数据证明 retarget 曲线需要调整。
4. `USDBGenesisHash` 必须在 chain config、genesis、system contract predeploy 和 bootstrap 参数 finalization 后更新。
5. bootnodes / DNS discovery 何时进入内置配置，何时作为 release artifact 分发。
6. UIP-0010 中 SourceDAO / dividend pool / fee split 冷启动流程是否采用固定地址、genesis predeploy、启动后初始化交易和激活高度的组合方案。
