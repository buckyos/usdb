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

本文只定义 chain config 和 bootstrap 边界，不定义具体 reward、fee split、CoinBase emission、collab bonus 或 price 公式。相关公式由后续 UIP 定义，并通过本文定义的 version 字段在 ETHW chain config 中激活。

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
- SourceDAO / dividend pool 的合约初始化细节。
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

这些值是当前实现基线，不自动等同于 public mainnet final 参数。进入 public testnet / mainnet 前必须重新确认。

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
- public testnet / mainnet 参数必须通过私链和测试网出块数据确认。
- 若后续引入 level-based difficulty，`base_difficulty` 仍由 ETHW PoW difficulty policy 产生，USDB level 只参与折算。

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

ETHW chain config / protocol params 必须允许 `header.Extra` 至少容纳该 payload。当前 go-ethereum 原型将：

```text
MaximumExtraDataSize = 160
```

v1 规则：

- USDB reward / difficulty consensus 激活后，普通区块 `header.Extra` 必须正好等于 UIP-0007 payload。
- `MaximumExtraDataSize` 可以大于 `107`，作为未来扩展上限。
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
- `MaximumExtraDataSize >= 107`。
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

1. public testnet / mainnet 的最终 `ChainID`、`NetworkId` 和 network name。
2. public testnet / mainnet 的最终 `GenesisDifficulty` 和 `MinimumDifficulty`。
3. `DifficultyBoundDivisor` 和 `DurationLimit` 是否沿用当前值。
4. `USDBGenesisHash` 是否在 chain config finalization 后更新。
5. 是否在 UIP-0009 中固定 `MaximumExtraDataSize = 160`，还是只规定 `>= 107`。
6. bootnodes / DNS discovery 何时进入内置配置。
7. fee split / dividend pool 是否从 genesis 启用，还是等待 UIP-0010 / 后续 UIP。
8. SourceDAO / dividend system contract 是否需要 genesis predeploy。
