UIP: UIP-0013
Title: Price and Real Price Update Rules
Status: Draft
Type: Standards Track
Layer: ETHW Reward / Economic Policy
Created: 2026-05-07
Requires: UIP-0000, UIP-0008, UIP-0009, UIP-0011
Activation: ETHW network activation matrix; first official networks use fixed price policy from genesis

# 摘要

本文定义 USDB CoinBase 公式使用的 BTC 价格状态顶层架构。

核心规则：

- UIP-0011 只消费本链 `stateRoot` 已承诺的 `price_atoms_per_btc`。
- `price_atoms_per_btc` 使用 USDB atoms 表示 `1 BTC` 对应的 USDB native token 数量。
- 首版 public network 从 genesis 启用 `FixedPrice` policy。
- `FixedPrice` v1 中 `price_atoms_per_btc == real_price_atoms_per_btc == const_price_atoms_per_btc`。
- 启动期不预先固定 `PRICE_REPORT_START_HEIGHT`。
- 价格机制升级通过 activation matrix / price policy range 指定，不通过隐式配置或实时外部查询决定。
- 以太坊 DeFi 引用和 USDB 自有 DeFi 挂单证明是后续可选 price source policy，必须由独立 UIP 定义。

# 动机

UIP-0011 的 CoinBase 释放公式需要 `price_atoms_per_btc`：

```text
target_supply_atoms
    = floor(total_miner_btc_sats * price_atoms_per_btc / BTC_SATS_PER_BTC)
```

如果价格输入来自 validator 实时查询外部 RPC，区块验证会依赖外部节点状态、archive 可用性、source chain reorg 和 RPC 诚实性。这不适合作为 ETHW 共识路径。

因此，本文将价格设计为本链可重放的协议状态：

```text
UIP-0011 reward verifier
    reads price_atoms_per_btc from parent ETHW state
```

价格状态如何初始化、升级和按历史高度重放由本文定义。复杂的外部 DeFi / 本链 DeFi 价格证明留给后续独立 UIP。

# 非目标

本文不定义：

- 以太坊主链 DeFi 合约状态证明格式。
- Ethereum finalized header registry 或 light client。
- USDB 自有 WBTC / BTC bridge。
- USDB 自有 DeFi orderbook / AMM 合约。
- 矿工双边挂单证明和订单锁定期。
- `price` 向动态 `real_price` 收敛的完整算法。
- 外部 oracle committee 签名格式。

这些机制可以作为未来 price source policy 独立标准化。本文只定义它们接入 UIP-0011 前必须满足的顶层状态边界。

# 术语

| 术语 | 含义 |
| --- | --- |
| `price_atoms_per_btc` | CoinBase 公式实际使用的算法价格，单位为 USDB atoms per BTC。 |
| `real_price_atoms_per_btc` | 价格来源机制上报或证明的实际价格，单位为 USDB atoms per BTC。 |
| `const_price_atoms_per_btc` | `FixedPrice` policy 中的固定价格常量。 |
| `PricePolicyRange` | 在特定 ETHW 网络和高度范围内生效的一条 price policy 配置。 |
| `price_source_kind` | 价格来源类型，例如固定价格、外部以太坊 DeFi 引用或 USDB 自有 DeFi 挂单证明。 |
| `price_policy_version` | 价格状态转换规则版本。 |
| `price_state` | ETHW reserved system account storage 中由 `stateRoot` 承诺的当前价格状态。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 单位与编码

价格单位：

```text
price_atoms_per_btc = USDB atoms paid for 1 BTC
```

首版使用 UIP-0011 中的最小单位：

```text
USDB_ATOMS_PER_USDB = 1_000_000_000_000_000_000
BTC_SATS_PER_BTC    = 100_000_000
```

因此：

```text
100_000 USDB / BTC
    = 100_000 * USDB_ATOMS_PER_USDB
    = 100_000_000_000_000_000_000_000 atoms / BTC
```

对外 JSON / RPC 中的价格必须使用 canonical decimal string。链内 storage 使用 `uint256`。

禁止使用浮点数表示价格、百分比或收敛计算。

# Price Source Kind

首版定义以下 `price_source_kind` 编号：

| 编号 | 名称 | 状态 | 说明 |
| --- | --- | --- | --- |
| `1` | `FixedPrice` | v1 启用 | 固定算法价格，不接受矿工报价。 |
| `2` | `ExternalEthereumDefiReference` | 保留 | 引用以太坊主链 DeFi 状态，证明格式由后续 UIP 定义。 |
| `3` | `UsdbNativeDefiOrderBacked` | 保留 | 使用 USDB 自有 DeFi 双边挂单约束，证明格式由后续 UIP 定义。 |

保留 source kind 不得在 public network 中启用，除非对应独立 UIP 已 Final，并通过 activation matrix 激活。

# Price Policy Range

启动期无法提前确定动态报价开始高度，因此本文不定义全局 `PRICE_REPORT_START_HEIGHT`。

价格 policy 必须用按高度生效的 range 表示：

```text
PricePolicyRange {
    ethw_chain_id: uint64
    ethw_network_type: enum
    start_block: uint64
    end_block_exclusive: optional uint64
    price_policy_version: uint32
    price_source_kind: uint32
    const_price_atoms_per_btc: optional uint256
    price_source_policy_ref: optional bytes32
}
```

规则：

- 同一 `(ethw_chain_id, ethw_network_type)` 下的 range 必须连续且不重叠。
- genesis 必须存在一条从 `start_block = 0` 或 `genesis block` 开始的 range。
- 对任意历史区块高度，validator 必须能唯一解析 active `PricePolicyRange`。
- `end_block_exclusive` 可以由下一条 range 的 `start_block` 推导，但 machine-readable registry 中必须能得到确定区间。
- range 变更属于共识规则升级，必须进入 UIP-0008 activation matrix 或等价 activation registry。

示例：

```text
[0, H1):
    price_source_kind = FixedPrice
    const_price_atoms_per_btc = 100_000_000_000_000_000_000_000

[H1, H2):
    price_source_kind = FixedPrice
    const_price_atoms_per_btc = 120_000_000_000_000_000_000_000

[H2, ...):
    price_source_kind = UsdbNativeDefiOrderBacked
    price_source_policy_ref = <future UIP / policy id>
```

该模型允许启动期先固定价格，必要时通过后续升级调整固定价格，也允许跳过外部以太坊 DeFi 方案，直接进入 USDB 自有 DeFi 方案。

# Reserved System Storage

价格状态必须存放在 ETHW reserved system account storage 中，并由每个区块的 `stateRoot` 承诺。

建议定义：

```text
USDB_SYSTEM_STATE_ADDRESS       = <TODO>

PRICE_ATOMS_PER_BTC_SLOT       = <TODO>  // uint256
REAL_PRICE_ATOMS_PER_BTC_SLOT  = <TODO>  // uint256
PRICE_POLICY_VERSION_SLOT      = <TODO>  // uint32 encoded as uint256
PRICE_SOURCE_KIND_SLOT         = <TODO>  // uint32 encoded as uint256
PRICE_POLICY_RANGE_ID_SLOT     = <TODO>  // bytes32 encoded / referenced as uint256-compatible commitment
```

必填状态：

- `PRICE_ATOMS_PER_BTC_SLOT`
- `REAL_PRICE_ATOMS_PER_BTC_SLOT`
- `PRICE_POLICY_VERSION_SLOT`
- `PRICE_SOURCE_KIND_SLOT`
- `PRICE_POLICY_RANGE_ID_SLOT`

这些 slots 是协议状态，普通 EVM 交易、用户合约和 SourceDAO / Dividend 合约不得直接修改。

# FixedPrice Policy

首版 public network 使用：

```text
price_policy_version = 1
price_source_kind = FixedPrice
const_price_atoms_per_btc = 100_000_000_000_000_000_000_000
```

该常量对应：

```text
100_000 USDB / BTC
```

在 `FixedPrice` range 内：

```text
price_atoms_per_btc_N = const_price_atoms_per_btc
real_price_atoms_per_btc_N = const_price_atoms_per_btc
```

规则：

- miner price report 禁用。
- `price` 向 `real_price` 的收敛逻辑禁用，因为二者恒等。
- `price_atoms_per_btc` 不得为 `0`。
- 如果 activation registry 对当前高度解析不到唯一 `FixedPrice` range，区块必须无效。

如果启动期需要调整固定价格，必须新增一条 `FixedPrice` range，而不是修改旧 range：

```text
old range [A, B):
    const_price_atoms_per_btc = 100_000 * USDB_ATOMS_PER_USDB

new range [B, C):
    const_price_atoms_per_btc = 120_000 * USDB_ATOMS_PER_USDB
```

旧区块必须继续按旧 range 重放。

# Genesis 初始化

首个 official public network 的 genesis 必须初始化：

```text
PRICE_ATOMS_PER_BTC_SLOT      = 100_000_000_000_000_000_000_000
REAL_PRICE_ATOMS_PER_BTC_SLOT = 100_000_000_000_000_000_000_000
PRICE_POLICY_VERSION_SLOT     = 1
PRICE_SOURCE_KIND_SLOT        = 1
PRICE_POLICY_RANGE_ID_SLOT    = active fixed-price range id
```

`PRICE_POLICY_RANGE_ID_SLOT` 的 canonical encoding 由 activation registry 实现规范固定。在该规范 Final 前，本文保留 `<TODO>`。

# State Transition

验证区块 `N` 时，validator 必须把 reward 使用的 price 与区块执行后写入的 child price state 分开处理。

UIP-0011 reward 使用 parent state 中已经生效的价格：

```text
reward_price_atoms_per_btc_N = read(PRICE_ATOMS_PER_BTC_SLOT from parent state)

compute UIP-0011 CoinBase using reward_price_atoms_per_btc_N
```

然后 validator 按区块 `N` 的 active range 计算 child state 中的新 price state：

```text
parent_price_state = read price slots from parent state
active_range = resolve PricePolicyRange(chain_id, network_type, block_number_N)

if active_range.price_source_kind == FixedPrice:
    price_atoms_per_btc_N = active_range.const_price_atoms_per_btc
    real_price_atoms_per_btc_N = active_range.const_price_atoms_per_btc
else:
    price_atoms_per_btc_N = future_price_source_transition(...)
    real_price_atoms_per_btc_N = future_price_source_transition(...)

write price slots for block N child state
```

为避免同一区块内先更新价格再领取更高 CoinBase，price state transition 写入的 child state 最早只能影响下一个区块的 UIP-0011 reward。

推荐执行顺序：

```text
reward_price_atoms_per_btc_N = read(PRICE_ATOMS_PER_BTC_SLOT from parent state)

compute UIP-0011 CoinBase using reward_price_atoms_per_btc_N

apply block N price state transition
write PRICE_ATOMS_PER_BTC_SLOT for child state
```

如果在 activation 边界上需要切换 `FixedPrice` 常量，则新常量从边界区块执行后的 child state 开始可见，最早影响下一个区块的 UIP-0011 reward。

该 one-block delay 是 v1 推荐语义。实现阶段如果选择让 activation 边界区块立即使用新 price，必须在 ETHW reward transition 中明确定义，并补充边界测试。本文 Draft 阶段倾向 one-block delay，以避免 parent state / child state 语义混淆。

# Dynamic Price Policy 接入边界

后续动态价格 policy 必须满足：

- 输出 `price_atoms_per_btc` 和 `real_price_atoms_per_btc`，单位仍为 `uint256 atoms / BTC`。
- 输出状态必须写入 reserved system storage，并由 `stateRoot` 承诺。
- UIP-0011 不得实时查询外部链、外部 RPC 或 DeFi 合约。
- block `N` 内的 price update 最早只能影响 block `N+1` 的 reward，除非后续 UIP 明确重定义边界语义。
- 历史区块重放必须只依赖当时 active range、parent ETHW state、区块内携带的可验证数据，以及已经写入本链 state 的 source-chain header / proof registry。

本文只固定 `FixedPrice` v1。动态 price source policy 必须在独立 UIP 中显式定义：

- 是否要求每个出块 miner 都提交 price update。
- 如果当前块没有有效 price update，是否允许延续 parent price。
- price update 失败时区块是否无效，还是仅忽略 update 并延续旧 price。
- 有效 price update 是否有额外奖励、手续费或强制义务。
- price update 写入 child state 后，从哪个区块开始影响 UIP-0011 reward。

在动态 price source policy Final 前，public network 不得启用 miner price report。

# 历史重放与查询边界

历史重放必须是 validator 本地可确定执行，不得依赖当前外部服务状态。

按 `price_source_kind` 区分：

| `price_source_kind` | 历史重放输入 | 禁止事项 |
| --- | --- | --- |
| `FixedPrice` | active `PricePolicyRange`、parent ETHW price state。 | 不得查询外部价格源。 |
| `ExternalEthereumDefiReference` | 区块内携带的 source proof，或 parent ETHW state 中已承诺的 source-chain header / proof registry。 | 不得实时查询 Ethereum RPC、archive RPC 或 current DeFi state。 |
| `UsdbNativeDefiOrderBacked` | parent ETHW state 中的本链 DeFi contract storage，按区块交易确定性执行。 | 不得通过 RPC 查询 current head；不得绕过本地 state trie / EVM state transition。 |

说明：

- 本链 DeFi 查询不是外部 RPC 依赖。validator 可以在执行区块时读取 parent ETHW state 中的合约 storage，并通过 deterministic EVM / state transition 得到 child state。
- 外部 Ethereum DeFi 查询必须转化为可重放证明。仅提供 `(chain, contract, height)` 不足以作为共识输入。
- RPC 可以作为浏览器、调试工具或离线审计的查询方式，但不得成为区块有效性的必要条件。

## External Ethereum DeFi Reference

`ExternalEthereumDefiReference` 是保留方案。

后续 UIP 如果启用该方案，至少必须定义：

- source chain id。
- source finalized block number。
- source finalized block hash。
- source state root。
- source contract address。
- account proof / storage proof。
- price field 或 orderbook root 的 canonical decoding。
- source finality 和 staleness window。
- source block monotonicity 或 replacement rule。
- 证明失败时的 fail closed 规则。

仅携带 `(chain, contract, height)` 不足以作为共识证明。validator 禁止通过实时 RPC 查询外部链来决定区块有效性。

## Usdb Native DeFi Order Backed

`UsdbNativeDefiOrderBacked` 是保留方案。

后续 UIP 如果启用该方案，至少必须定义：

- USDB 自有 BTC / WBTC 资产和 bridge / wrapper 边界。
- DeFi 合约地址、订单簿或 AMM state。
- 双边挂单价格带，例如 `0.8x / 1.2x` 或 `0.9x / 1.1x`。
- 最小订单规模，例如 `min(1 WBTC, miner_btc_balance * 5%)` 或固定 `1 WBTC`。
- 订单锁定期、取消延迟和资金 escrow 规则。
- `miner_btc_balance` 来源和历史查询高度。
- 报价者身份与出块 miner pass 的绑定。

在该方案 Final 前，不得把本链 DeFi 报价作为强制 public network policy。

# Reorg 语义

ETHW reorg 时：

- price state storage 必须随 ETHW state 回滚。
- activation range 解析必须按被重放区块的 chain / network / height 得到同一 policy。
- fixed price range 不依赖 current config 的可变值。
- 动态 policy 如果已启用，必须使用区块内携带的可重放证明或 parent state，不得重新查询 current external state。

# 与 UIP-0011 的关系

UIP-0011 只消费：

```text
price_atoms_per_btc
```

UIP-0011 不定义：

- fixed price 常量。
- price policy range。
- real_price 更新证明。
- price convergence。
- external / native DeFi 证明格式。

在本文 Final 前，UIP-0011 不能进入 Final。

# 与 UIP-0008 的关系

Price policy range 是 activation matrix 的一类记录。

实现阶段应该能按以下输入查询 active price policy：

```text
(ethw_chain_id, ethw_network_type, ethw_block_height)
    -> PricePolicyRange
```

如果 `price_policy_version`、`price_source_kind`、fixed price constant 或动态 source policy 发生变化，必须新增 activation record，不得就地修改历史记录。

# 测试要求

至少需要覆盖：

- genesis price slots 初始化为 `100_000 USDB / BTC`。
- `FixedPrice` range 内 `price == real_price == const_price`。
- miner price report 在 `FixedPrice` range 内无效或被忽略。
- UIP-0011 reward 使用 parent state price，而不是同一区块内更新后的 child state price。
- 固定价格升级后，旧区块按旧 range 重放，新区块按新 range 重放。
- activation range 缺失、重叠或不连续时 fail closed。
- `price_atoms_per_btc == 0` 时区块无效。
- reorg 后 price slots 回滚。
- JSON / RPC 中大整数价格使用 canonical decimal string。
- 历史重放不需要访问外部 RPC。

# 待审计问题

| 问题 | 当前草案结论 | 后续动作 |
| --- | --- | --- |
| 启动期价格 | v1 固定 `100_000 USDB / BTC`。 | 与 public testnet / mainnet 经济参数一起复核。 |
| `PRICE_REPORT_START_HEIGHT` | 不预先定义；由后续 activation range 决定。 | 在动态 price source UIP Final 后再激活。 |
| 固定价格是否可升级 | 可以，通过新增 `FixedPrice` range 调整。 | 定义治理/activation 操作流程和公告窗口。 |
| reward 是否使用 parent price | 当前草案倾向使用 parent state price，price update 影响下一块。 | 与 go-ethereum reward transition 实现一起审计。 |
| 动态阶段是否每块必须更新 price | 本文不固定；`FixedPrice` v1 不需要 miner update。 | 后续 dynamic price source UIP 必须定义 mandatory / optional / scheduled update 规则。 |
| 未提交有效 price update 时是否延续旧 price | 本文不固定；`FixedPrice` v1 恒定延续。 | 后续 dynamic price source UIP 必须定义 carry-forward 或 fail-closed 规则。 |
| price update 是否需要额外激励 | 本文不固定。 | 后续 dynamic price source UIP 评估更新激励、惩罚或强制义务。 |
| `PRICE_POLICY_RANGE_ID_SLOT` canonical encoding | `<TODO>`。 | 等 activation registry 实现规范固定。 |
| Ethereum DeFi 与 USDB native DeFi 顺序 | 二者都是可选后续 policy，不强制阶段二再阶段三。 | 独立 UIP 评估成熟度和实现成本。 |
| 双边挂单参数 | 不在本文固定。 | 后续 DeFi price source UIP 统一 `0.8/1.2` 与 `0.9/1.1` 差异。 |
| `price` 向 `real_price` 的动态收敛 | `FixedPrice` 下禁用。 | 动态 price source UIP 再固定整数收敛公式。 |
