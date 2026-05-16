UIP: UIP-0015
Title: Auxiliary Hashpower Pool
Status: Draft
Type: Standards Track
Layer: ETHW Reward / Auxiliary BTC Hashpower
Created: 2026-05-16
Requires: UIP-0000, UIP-0001, UIP-0006, UIP-0007, UIP-0008, UIP-0009, UIP-0011
Activation: Not enabled in v1 public networks; activation requires a future aux pool policy version and ETHW activation matrix entry

# 摘要

本文定义辅助算力池的协议边界和首版设计纲要。

辅助算力池的目标是允许持有矿工证的 BTC 矿工支付手续费，向 USDB ETHW 网络提交可验证的 BTC 辅助算力证明，并在辅助算力池启用后参与 UIP-0011 预留的 CoinBase 份额。

v1 public network 不默认启用辅助算力池。初始版本应表达为 disabled policy version，而不是本地开关：

```text
aux_pool_policy_version = 0
aux_pool_active = false
aux_pool_coinbase_atoms = 0
miner_coinbase_atoms = coinbase_emission_atoms
```

只有在 UIP-0015 进入 Final、证明格式和分配规则完成审计、并通过 UIP-0008 activation matrix 在指定 ETHW 高度激活 `aux_pool_policy_version > 0` 后，才允许启用辅助算力池分账。

# 动机

经济模型设计大纲为辅助算力池预留了 25% CoinBase 份额：

```text
miner_coinbase_base = CoinBase * 0.75
aux_pool_reward     = CoinBase * 0.25
```

同时，大纲要求辅助算力提交必须满足：

- 提交者持有矿工证。
- 提交的是最近 2 个 BTC 高度以内的有效 BTC 算力。
- 有效算力门槛大于 BTC 出块难度的 75%。

但辅助算力池涉及 BTC 工作量证明格式、BTC 高度锚定、重复提交、多提交者分配、提交者身份绑定和 spam control。若这些规则没有确定，就把 aux pool 直接放入 public network 共识路径，会引入不可重放或不可审计的风险。

因此，本文先明确：

- aux pool 在 v1 是可升级预留机制，不是默认启用机制。
- UIP-0011 只消费 aux pool 的启用状态和分账结果，不定义证明细节。
- 所有辅助算力提交必须最终能从 ETHW 链上状态和已承诺的 BTC 历史视图重放验证，不能依赖 live BTC RPC 或外部临时状态。
- aux pool 不应引入独立的本地 `enabled` boolean；是否 active 必须由 activation matrix、policy version 和链上承诺状态共同决定。

# 非目标

本文首个 Draft 不定义：

- 最终的辅助算力证明二进制格式。
- 最终的 aux pool system contract ABI。
- BTC mining pool 私有 share 协议。
- BTC bridge、WBTC 或跨链资产转移。
- SourceDAO / Dividend 内部二次分润逻辑。
- 具体 public network 激活高度。

# 术语

| 术语 | 含义 |
| --- | --- |
| `aux_pool` | 辅助算力池。启用后接收 UIP-0011 预留的 CoinBase 份额。 |
| `aux_submitter` | 提交辅助算力证明的矿工证持有人。 |
| `aux_hashpower_submission` | 一次辅助算力提交记录。 |
| `effective_hashpower_proof` | 可验证的 BTC 辅助算力证明。 |
| `btc_reference_height` | 证明引用的 BTC 区块高度。 |
| `btc_reference_hash` | 证明引用的 BTC 区块 hash。 |
| `btc_reference_difficulty` | 证明引用高度对应的 BTC difficulty / work target。 |
| `submission_window` | 辅助算力提交可接受的 BTC 高度窗口。 |
| `miner_pass_binding` | 辅助算力证明与 USDB 矿工证 owner / pass 的绑定规则。 |
| `aux_pool_policy_version` | 辅助算力池证明、分配和状态转换规则版本。 |
| `aux_pool_recipient` | UIP-0011 aux pool reward 的接收地址或 system contract。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 常量

首版草案使用以下参数名：

| 参数 | 值 | 状态 | 说明 |
| --- | --- | --- | --- |
| `AUX_POOL_POLICY_VERSION` | `1` | Draft | 辅助算力池首个 policy version。 |
| `AUX_POOL_INITIAL_POLICY_VERSION` | `0` | v1 固定 | public network 首发时从 disabled policy version 开始。 |
| `AUX_POOL_COINBASE_BPS` | `2500` | 来自 UIP-0011 | aux pool 启用后的 CoinBase 份额。 |
| `MINER_COINBASE_BPS_WHEN_AUX_ENABLED` | `7500` | 来自设计大纲 | aux pool 启用后的 miner 基础 CoinBase 份额。 |
| `AUX_HASHPOWER_THRESHOLD_BPS` | `7500` | Draft | 提交的有效 BTC 算力门槛，目标为大于 BTC difficulty 的 75%。 |
| `AUX_BTC_HEIGHT_LOOKBACK` | `2` | Draft | 只接受最近 2 个 BTC 高度以内的证明。 |

`AUX_HASHPOWER_THRESHOLD_BPS` 的边界是待审计项：最终规则必须明确使用严格 `>` 还是 `>=`。

# 激活边界

aux pool 必须独立激活，不得因为 UIP-0011 激活而自动启用。

public network 首发状态：

```text
aux_pool_policy_version = 0
aux_pool_recipient = 0x0000000000000000000000000000000000000000
aux_pool_verifier_code_hash = 0x0000000000000000000000000000000000000000000000000000000000000000
```

在该状态下，UIP-0011 的 CoinBase split 必须按未启用辅助算力池执行：

```text
miner_coinbase_atoms = coinbase_emission_atoms
aux_pool_coinbase_atoms = 0
```

是否启用 aux pool 是派生状态，不是本地配置开关：

```text
aux_pool_active(block_height)
    = active_version_set(block_height).aux_pool_policy_version > 0
      AND aux_pool_recipient != 0x0000000000000000000000000000000000000000
      AND aux_pool_verifier_code_hash == expected_aux_pool_verifier_code_hash
```

`active_version_set(block_height)` 由 UIP-0008 activation matrix 按目标 ETHW chain、network 和 block height 查询得到。

如果未来某个新网络希望从 genesis 启用辅助算力池，不应设置本地 genesis boolean，而应在该网络 activation matrix 中写入：

```text
component = aux_pool_policy_version
version = 1
activation_anchor = ethw_block
activation_value = 0
```

启用 aux pool 至少需要：

1. UIP-0015 进入 Final。
2. `aux_pool_policy_version` 固定。
3. `aux_pool_recipient` 或 aux pool system contract 固定。
4. proof verifier 规则固定。
5. reward distribution 规则固定。
6. activation matrix 明确 ETHW chain、network、activation block。
7. 如果使用 system contract，`aux_pool_verifier_code_hash` 必须固定并可由节点校验。

# 高层流程

辅助算力池启用后的目标流程：

1. `aux_submitter` 持有或控制一个有效矿工证。
2. `aux_submitter` 生成 `effective_hashpower_proof`。
3. 证明引用一个可重放的 BTC 历史点：

```text
btc_reference_height
btc_reference_hash
btc_reference_difficulty
```

4. 证明进入 ETHW 共识可见数据，建议通过 aux pool system contract / system transaction 承载，而不是塞入 `header.Extra`。
5. ETHW 执行层验证：

```text
proof is canonical
proof references accepted BTC history
proof work > threshold
proof is not duplicated
proof is bound to submitter miner pass
proof is inside submission window
```

6. 验证通过后，aux pool state 记录可参与分配的提交。
7. UIP-0011 在 aux pool active 且当前区块有有效 aux pool state 时，把 CoinBase 拆分给 miner 和 aux pool。

# 证明承载边界

UIP-0007 的 `ProfileSelectorPayload` 需要保持短小稳定，不应承载完整辅助算力证明。

辅助算力证明可能包含 BTC header、share、nonce、merkle 相关字段或提交者绑定字段，体积和验证成本都高于 `header.Extra` 适合承载的范围。因此，v1 草案倾向：

- ETHW block header 只保留 UIP-0007 selector。
- aux pool proof 通过普通交易、system transaction 或 system contract call 进入执行层。
- accepted submission 进入 ETHW state，并由 `stateRoot` 承诺。
- reward split 只消费已经进入 parent state 或当前 state transition 中按规则生效的 aux pool state。

具体 proof ABI 和 storage key 是待审计项。

# BTC 历史锚定

辅助算力证明必须引用一个确定的 BTC 历史点。验证者重放历史区块时，不得实时查询外部 BTC RPC 来决定证明是否有效。

可选方案：

| 方案 | 说明 | 风险 |
| --- | --- | --- |
| ETHW 内置 BTC header relay | ETHW 链上维护可验证 BTC header chain / difficulty state。 | 实现成本高，但 replay 最清晰。 |
| USDB indexer state commitment | 通过 UIP-0006 / UIP-0008 绑定稳定 BTC 历史视图。 | 需要明确 ETHW 如何验证该视图，不可退化为信任外部服务。 |
| proof 自带 BTC header chain segment | 每个 proof 携带足够 BTC headers。 | 体积和 gas 成本可能过高。 |

Draft 阶段不选定最终方案。Final 前必须固定 BTC reference validation 方案，并覆盖 BTC reorg 语义。

# Miner Pass Binding

辅助算力提交必须绑定 USDB 矿工证，避免没有矿工证的外部 BTC 算力直接领取 aux pool 份额。

需要固定的绑定要素：

```text
submitter_pass_id
submitter_owner_script_hash or owner_btc_addr
submitter_eth_address
submission_eth_sender
btc_reference_height
proof_nonce or proof_id
```

待审计问题：

- 绑定 subject 使用 `pass_id` 还是 owner / BTC address。
- remint 后是否允许旧 pass 的提交继续有效。
- collab pass 是否允许作为 `aux_submitter`。
- dormant / consumed / burned pass 的提交如何处理。

当前 Draft 倾向：

- 使用 active miner pass 的 `pass_id` 作为主绑定对象。
- 只有 active pass 可以提交新的 aux proof。
- old pass remint 后，旧 pass 未结算提交是否继续有效需要单独审计。

# 重复提交与反作弊

aux pool 必须防止同一份 BTC 工作量被重复领取。

至少需要定义：

- `proof_id` canonical encoding。
- 同一 proof 被同一 pass 重复提交时的处理。
- 同一 proof 被不同 pass 重复提交时的处理。
- proof 在不同 ETHW fork / network 重放时的隔离字段。
- proof 在不同 reward window 重放时的隔离字段。
- 低成本垃圾提交的手续费或押金规则。

`proof_id` 应至少绑定：

```text
chain_id
network_id
aux_pool_policy_version
btc_reference_hash
btc_reference_height
proof_work_commitment
submitter_pass_id
```

最终格式进入 Final 前必须给出 test vector。

# Reward Distribution

## 未启用

未启用 aux pool 时：

```text
aux_pool_coinbase_atoms = 0
miner_coinbase_atoms = coinbase_emission_atoms
```

## 启用

启用后，UIP-0011 使用：

```text
aux_pool_coinbase_atoms
    = floor(coinbase_emission_atoms * 2500 / 10000)

miner_coinbase_atoms
    = coinbase_emission_atoms - aux_pool_coinbase_atoms
```

整除余数归 miner。

## 无有效提交时

如果 aux pool 已启用，但当前分配窗口没有任何有效提交，必须在 Final 前固定处理方式。

可选方案：

| 方案 | 行为 | 风险 |
| --- | --- | --- |
| ReturnToMiner | aux share 回到当前出块 miner。 | 激励接近未启用 aux pool，可能削弱辅助算力提交动力。 |
| CarryForward | aux share 留在 aux pool 合约，进入后续分配。 | 需要池内余额和后续分配规则。 |
| DividendOrDAO | aux share 进入 Dividend / DAO。 | 需要和 UIP-0010 / SourceDAO 治理边界对齐。 |
| NotMinted | aux share 不发行。 | 会影响 issued supply 语义，需要 UIP-0011 一起升级。 |

当前 Draft 不做选择。aux pool 不得在该规则未固定前启用。

# 与其他 UIP 的关系

- UIP-0011 定义 CoinBase emission 和 75% / 25% split 的消费边界。
- UIP-0007 不承载完整辅助算力证明，只承载 consensus profile selector。
- UIP-0008 定义 aux pool policy version 和 activation matrix 的网络化激活。
- UIP-0009 定义 ETHW chain config 中是否预留 aux pool 相关字段。
- UIP-0006 可提供与 submitter pass、owner、energy 相关的审计视图，但不能替代 ETHW 共识验证。

# 状态与重放

aux pool accepted submission 必须进入 ETHW state，并随 `stateRoot` 被承诺。

推荐状态边界：

| 状态 | 建议位置 | 说明 |
| --- | --- | --- |
| `aux_pool_policy_version` | activation matrix / active version set | 当前高度生效的 aux pool policy version；`0` 表示 disabled。 |
| `aux_pool_recipient` | chain config / reserved storage | UIP-0011 aux reward recipient。 |
| `aux_pool_verifier_code_hash` | chain config / reserved storage | aux pool verifier runtime code hash 或 system contract code hash。 |
| accepted submissions | aux pool system contract storage | 证明提交量可能较大，不适合固定少量 reserved slots。 |
| distribution checkpoints | aux pool system contract storage | 用于 reward claim 和历史审计。 |

所有状态更新必须支持 ETHW reorg 回滚。

# 测试要求

Final 前至少需要覆盖：

- aux pool 未启用时 CoinBase 100% 归 miner。
- aux pool 启用后 75% / 25% 分账，rounding remainder 归 miner。
- proof BTC reference 超出最近 2 个 BTC 高度时被拒绝。
- proof work 低于门槛时被拒绝。
- proof work 等于 75% 门槛时按最终 `>` / `>=` 规则处理。
- 同一 proof 重复提交时被拒绝或按最终规则幂等处理。
- 不同 pass 竞争同一 proof 的处理路径。
- inactive / dormant / consumed / burned pass 提交被拒绝。
- ETHW reorg 后 accepted submissions 和分配状态正确回滚。
- 历史区块重放不依赖 live BTC RPC。
- `header.Extra` 不包含完整 proof，仍符合 UIP-0007 payload 长度约束。

# 待审计问题

| 问题 | 当前倾向 | 需要确认 |
| --- | --- | --- |
| aux pool 是否在 v1 public network 启用 | 不启用，初始 `aux_pool_policy_version = 0`。 | activation matrix 是否显式记录 disabled version，还是只记录后续启用版本。 |
| 有效算力证明格式 | 通过 system contract / system tx 提交，不进 `header.Extra`。 | 具体 proof ABI、canonical encoding 和 test vector。 |
| BTC reference validation | 不允许 live BTC RPC。 | 选择 BTC header relay、USDB state commitment 或 proof 自带 header segment。 |
| 最近 2 个 BTC 高度窗口 | 保留设计大纲口径。 | 以 BTC height、BTC hash 还是 ETHW 已知 BTC state 计算窗口。 |
| 75% 门槛边界 | 使用 `AUX_HASHPOWER_THRESHOLD_BPS = 7500`。 | 严格 `>` 还是 `>=`。 |
| miner pass binding | 倾向绑定 active pass `pass_id`。 | owner / BTC address 继承、remint、collab pass 是否允许。 |
| 多提交者竞争 | 未定。 | 按先到先得、按 work 比例、按窗口聚合还是其他方式。 |
| 无有效提交时 aux share | 未定。 | ReturnToMiner / CarryForward / DividendOrDAO / NotMinted。 |
| spam control | 未定。 | 手续费、押金、slash 或 rate limit。 |
| aux pool recipient / verifier hash | 未定。 | system contract 地址和 runtime code hash 是否需要 UIP-0009 / UIP-0010 一起固定。 |
| 证明数据保留周期 | 未定。 | 全量链上保留、状态压缩还是 event + state checkpoint。 |

# 下一步

1. 选择 BTC reference validation 方案。
2. 起草 `effective_hashpower_proof` canonical encoding。
3. 决定 active pass binding 规则。
4. 决定多提交者和无提交者分配规则。
5. 明确 aux pool system contract 是否作为 genesis predeploy。
6. 为 `proof_id`、threshold boundary 和 reorg path 编写 test vector。
7. 确认 UIP-0011 是否需要在 aux pool 激活前增加额外 policy version 字段。
