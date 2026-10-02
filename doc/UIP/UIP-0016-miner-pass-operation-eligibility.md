UIP: UIP-0016
Title: Miner Pass Operation Eligibility and Cross-Owner Inheritance
Status: Draft
Type: Standards Track
Layer: BTC Application / Consensus Input
Created: 2026-10-02
Requires: UIP-0000, UIP-0001, UIP-0002, UIP-0003, UIP-0004, UIP-0006, UIP-0008
Supersedes: UIP-0001 v1 mint eligibility and UIP-0002 v1 same-owner inheritance only after scoped activation
Activation: Planned; no network or height activated by this draft
Affected-Version-Fields: inscription_schema_version, pass_state_machine_version; other fields subject to encoding review
Activation-Matrix: See activation section
Backwards-Compatibility: Replay pre-activation rules; retain existing valid pass history
Test-Cases: See acceptance vectors and implementation plan

# 摘要

本文把 [issue #51 的收敛方案](https://github.com/buckyos/usdb/issues/51#issuecomment-5894524579) 整理为 MinerPass 下一阶段的协议草案。三个操作路径共用一个资格入口：首次零余额开户、同地址操作、单来源跨地址继承。

本文是实现基线草案，不代表实现已经完成或某一网络已激活。具体实施状态见 [实施计划](../usdb-indexer/miner-pass-operation-eligibility-plan.md)。此前完成的 registry 作用域隔离和 P2P fork ID 修正只是前置条件，不会自动启用本文规则。

# 动机与承诺

最低承诺是：持有人未参与相关花费时，第三方仅靠赠送铭文，不得替换其已有 pass、消费其 prev，或借用其已有 BTC 余额建立攻击者选择的收益/协作绑定。

来源检查证明来源 owner 参与了 commit 花费，不证明其理解 USDB 内容，也不证明其签名授权了最终 reveal 接收者。向恶意 commit 地址普通付款、错误铸造及被诱导付款仍属于首版接受并必须披露的风险。推荐 pass 地址专用于持币和明确的 pass 操作；钱包使用建议不替代索引器强制执行的有效性检查。

独立密钥的未暴露公钥哈希地址可以用于长期持有 BTC/pass。地址轮换减少长期暴露，但不承诺全程隐藏公钥或把暴露窗口固定在几个 block。本文不引入 BIP-322、OP_RETURN 操作授权、commit 同时迁移全部余额、Leader 稳定身份或永久退休地址状态。

# 术语

- `source_owner`（D）：真实铭文 sat 在 commit 交易中对应输入 prevout 的锁定脚本身份。
- `mint_owner`（E）：reveal 中实际接收该 sat 的输出锁定脚本身份，沿用 UIP-0001 owner 表示。
- `commit_outpoint`：reveal 的对应输入所花费的 commit 输出。中间 Taproot 地址 C 不是 D。
- `balance_before_tx(E)`：当前 reveal 执行前，E 的精确可计入 BTC 余额，单位 sat，沿用 balance-history 的脚本/可花费输出口径。
- `ever_valid_owner(E)`：本作用域内、当前 ordered event 之前，E 是否曾通过有效 mint 或有效 pass 的 owner transfer 获得过 pass。
- `can_open(E)`：`balance_before_tx(E) == 0 && !ever_valid_owner(E)`。
- `source_proof`：绑定规范链 block、交易、具体输入及 sat 偏移，并证明支持的签名形式覆盖 commit 输出的链上证据。

“有效”不等于“当前 Active”。曾有效 mint、后来休眠/消费/转出/烧毁，不能恢复原 owner 的开户资格。Invalid mint 及其转移不占用资格。有效 transfer 的判定沿用被激活状态机的语义，不能把未发生 owner 更新的终态事件臆造为成功转移。

# 规范

## 三条路径

| 路径 | 必须满足的条件 | 成功效果 |
| --- | --- | --- |
| 首次开户 | `can_open(E)`，`prev=[]`；允许第三方出资 | E 创建 Active，不替换或消费其他 owner 的 pass |
| 同地址操作 | 合规 `source_proof` 且 `D == E`；prev 如非空，均当前属于 D 且可继承 | 沿用同 owner remint：旧 Active 休眠，合法 prev 消费，E 创建 Active |
| 跨地址继承 | 合规 `source_proof` 且 `D != E`；`can_open(E)`；prev 非空、均当前属于 D 且可继承 | 仅结算和消费列出的 prev，E 创建 Active |

优先判定无 prev 的开户豁免；其余操作必须经过来源检查。首次开户仍须正确定位 sat 和 E，只豁免来源签名支持范围要求，不能把未知接收 owner 当作任意地址。E 零余额不豁免非空 prev 的来源检查。来源脚本不是首版支持形式时，同地址操作和跨地址继承无效，但不能因此禁止真正符合开户条件的第三方出资 mint。

禁止跨地址覆盖已有账户；禁止一次继承多个来源 owner。D 和 E 按锁定脚本身份比较，不能比较钱包名、浏览器 From、输入是否包含地址或首个输入地址。

## 交易前余额与历史资格

对高度 H 的第 N 笔交易：

```text
balance_before_tx(E, txN)
  = balance(E, H-1)
  + sum(delta(E, tx_i), i < N)
```

- 所有 BTC 交易都更新余额上下文，包括普通转账、无效铭文交易以及没有 USDB 事件的交易。
- 当前 reveal 的输入/输出不提前作用于交易前余额；同一交易中的全部 mint 共用该交易前余额。
- 资格按 UIP-0002 ordered events 更新。先执行的有效开户或有效 transfer 可以阻止同交易后续 mint 使用开户豁免。
- 精确余额 1 sat 也不满足开户条件，不能用 energy 的 balance units 为零代替。
- checked arithmetic 溢出、不完整历史和上游读取错误不得转换成 0。证据缺失属于可重试的数据不可用，暂停该块并回滚未提交状态，不得永久写入协议 Invalid。
- `ever_valid_owner` 必须来自本作用域自 index origin 起的规范历史，包含新规则激活前的有效历史。可以使用可重建的物化索引，但不得在升级、重启、地址归零或切换 current registry 时清空。
- reorg 应撤回被断开区块首次产生的资格记录；若更早规范历史已有记录，则资格仍被占用。快照必须足以恢复同一结果，不能仅保存当前 Active 集合。

该公式规定余额语义，不强制 RPC 必须直接返回 H-1。如果快照只有 H 的区块后余额，可以在同一规范链锚下以完整且已验证的全块交易增量反推 H-1，再得到每笔交易前余额；减法、覆盖范围和脚本口径都必须严格校验。缺少任一输入脚本/金额或基线余额时，不能使用这个替代路径。

开户余额只决定操作资格。UIP-0002/0003 的整块能量结算边界保持独立，不改成逐交易能量结算。

## sat 来源与签名覆盖

从真实 reveal envelope 定位其铭文 sat，再反向找到 commit 输出中的偏移。对于 commit 的输出 j、其中偏移 k：

```text
q = sum(commit.output[m].value, m < j) + k
source input i 满足：
  sum(input[n].value, n < i) <= q < sum(input[n].value, n <= i)
source input offset = q - sum(input[n].value, n < i)
```

区间为左闭右开；零金额输入不能提供 sat。必须验证完整输入顺序和金额、输出索引、offset 边界、具体 prevout 脚本及交易所属规范链，不能任选祖先或来源输入。多输入交易中，即使 D 出现在输入列表中，只要实际 sat 来自 A，就不能认定 D 是来源。

Ord envelope 的 `offset` 是 envelope 序号，不可直接当作 sat 偏移。Ord pointer 可以改变 sat 归属；不得无条件使用当前实现的 `offset=0`。首版实现必须冻结支持的 envelope 子集，并对不支持的 pointer、unbound、歧义形式给出确定的协议错误。保留同一 reveal input 上多个 USDB mint 的歧义拒绝规则。支持范围与固定 Ord 版本的原始交易向量必须在启用前一起审查。

依赖来源的路径，首版支持范围为：

| 输入 prevout 类型 | 必须识别的花费形式 | sighash |
| --- | --- | --- |
| P2PKH | 标准单签 scriptSig，与 prevout 公钥哈希匹配 | ECDSA ALL (`0x01`) |
| 原生 P2WPKH | 单签 witness，与 prevout witness program 匹配 | ECDSA ALL (`0x01`) |
| P2TR | 明确识别 key-path；不能把 script-path 的某个 witness item 误作签名 | DEFAULT（64 字节签名）或 ALL（65 字节且末字节 `0x01`） |

不接受 NONE、SINGLE、ANYONECANPAY；首版不覆盖其他脚本组合。Taproot annex 的识别和支持范围必须列入原始交易向量，不能仅凭 witness 总长度猜测 key-path。该白名单约束 USDB 来源证明，不改变 Bitcoin 交易有效性，不限制接收地址必须为 Taproot。

取证来自已经通过 Bitcoin 共识验证的规范块，并核对交易字节、输入 prevout 和链锚；仅检查任意字节末尾的 sighash 标记不构成来源证明。ALL/DEFAULT 的输入与输出覆盖依据 [BIP-143](https://github.com/bitcoin/bips/blob/master/bip-0143.mediawiki) 和 [BIP-341](https://github.com/bitcoin/bips/blob/master/bip-0341.mediawiki)。

## 状态原子性与继承

执行前先完成 schema、接收资格、来源、所有 prev 和 Leader 绑定校验。协议校验失败只记录该 mint 的 Invalid，不得部分休眠、消费或改变已有 pass 的收益/协作字段。该 BTC 交易实际触发的 transfer/burn 独立按既有规则发生；不能为了使失败 mint 看起来无副作用而撤销真实 transfer。

同地址路径延续 UIP-0002 的虚拟旧 Active 校验。跨地址路径仅将 prev 列出的源 Active 在虚拟前置状态中视为 Dormant，再结算并消费；源 owner 未被列出的其他 pass 不自动休眠或作废。prev 的 owner 使用执行该 mint 时的 ordered state，不使用 mint_owner 历史值。重复、缺失、已消费、已烧毁、Invalid、异来源 prev 均拒绝。

state validation 和 consume 前复核必须使用同一个经过验证的 D。运行时 I/O 失败按整块恢复机制回滚；跨 SQLite、energy store、transfer tracker 的原子效果必须通过故障注入验证，不能把数据库写失败当作用户的无效操作。

钱包应默认保护待继承 pass UTXO，不用于 commit 出资。若此前真实 transfer 已改变其 owner，则不能继续凭旧 D 消费。外部交易得到的 Active pass 跨 owner 转移后仍变为 Dormant，接收方 remint 激活需要来源检查，且不能恢复开户豁免。

## 能量与协作

继续沿用 UIP-0003 的逐个 prev 继承折损、余额惩罚和饱和累加；不得将迁移描述成“总共只损失 5%”。同块先转余额再 mint、先 mint 后转余额，以及跨块迁移必须分别验收。

standard/Leader 与 collab 共用同一资格入口。Leader 从 D 迁往 E 后，固定 `leader_pass_id` 和 `leader_btc_addr=D` 都不自动跨地址跟随；协作者通过自己的合规 remint/继承重新绑定。未重绑的解析和贡献继续按 UIP-0004，不新增隐式 Leader 迁移表。

# 版本与激活

本草案建议新 mint JSON 使用整数 `v: 2`，沿用现有业务字段，不新增 `sig/src/dest`。source/destination 从链上取得。建议原版本族演进为：

| 字段 | 激活前 | 激活后候选 |
| --- | --- | --- |
| inscription_schema_version | uip-0001-miner-pass-inscription:v1 | uip-0001-miner-pass-inscription:v2 |
| pass_state_machine_version | uip-0002-pass-state-machine:v1 | uip-0002-pass-state-machine:v2 |
| energy / effective-energy / level formulas | 已有 v1 | 参数与数学公式不变时保持 v1 |
| query / state-view / commit protocol | 已发布版本 | 在接口与 canonical encoding 审查时决定，不能预先宣称全不变 |

两个 v2 版本族须作为受支持组合分派；不能允许 v2 schema 搭配 v1 状态机。active version set 由指定 BTC source、rules scope、registry ID 及 BTC 高度选择，JSON v 不负责选择宽松执行器。激活后新出现的 `v:1` mint 必须 Invalid，不能绕过来源检查；激活前按原规则重放，已有合法 v1 pass 可以作为新操作的 prev。

若 qualification 完全由已有已承诺历史派生，缓存不是独立共识状态；若新增不可派生字段或改变规范编码，则必须定义其 commitment、版本和快照恢复规则。不能只升级一个版本字符串。尤其当前 commit 版本还关联 balance-history，必须区分 pass mutation 编码与上游快照协议的影响，避免无必要地改变 BTC 基础数据身份。

| BTC source | USDB rules scope | 激活高度 | 状态 |
| --- | --- | --- | --- |
| btc-regtest | 隔离验收 catalog，具体名称由测试夹具固定 | 测试指定 H | Planned |
| btc-mainnet | 现有 legacy / testnet-v0 | 不添加本文激活 | Planned；旧规则保持 |
| btc-mainnet | 待发布 testnet-v1 | 待冻结网络包 | Planned |
| btc-mainnet | 未来正式网作用域 | 独立审议 | Planned |

代码合并不等于激活。registry 隔离、USDB chain checkpoint 绑定、Go 版本支持及发布包必须一致。P2P fork ID 仅纳入 USDB checkpoint 高度，不替代对 BTC 高度规则与版本身份的验证。

激活不追溯证明旧 pass 获得过授权，也不自动修复历史恶意绑定。旧网如需强制处理存量须有独立迁移规则；可丢弃的测试网重置属于发布阶段操作，不能借本草案改写原网历史。

# 数据可用性与工具

正式节点必须具备重建来源证明所需的 commit 交易及其输入 prevout 证据，不得只在开启 txindex 的开发机成功。当前 reveal block 的 verbosity-3 数据能提供其花费的 commit 输出信息，但不等于已经得到 commit 所有输入的脚本/金额；commit 的来源块可能早于 index origin 或 AssumeUTXO base。

实现需要显式验证 commit 块定位、同块 commit/reveal、历史 block/undo 保留及 snapshot 恢复路径。证据未就绪时报告不可用于共识，并可恢复重试；不能回退当前 `gettxout`、无认证的浏览器数据或金额默认零。所需保留策略必须写入节点部署要求。

control-plane prepare 必须区分 D 与 E，按 D 查询可继承 pass，同时检查 E 的资格；返回三条路径、prev 清单、收益/Leader 绑定、风险与可用性状态。prepare 的观测不能保证未来上链时仍有资格，最终以 reveal 的规范链状态为准。

首次开户应生成新 E，mint 后核验确认深度、索引 Active、来源和完整配置，再转入余额。跨地址继承应先核验 E 上的新 pass 与 prev 消费结果，再迁移余额并检查 D 的找零和残留 pass UTXO。普通钱包显示“收到铭文”或广播成功不能作为入金依据。

当前 execute 仅限 development runtime；生产签名广播、钱包选币适配和真实 sat 来源保证需要独立验收。不得把钱包名当成 D，也不得把新增提示文案当作钱包已支持跨地址继承。

# 安全与剩余风险

允许第三方为未开户零余额地址首次 mint，因此仍接受空地址抢占、小额转账干扰，以及向公开空地址预埋绑定后等待未来入金。推荐“先验收、后入金”，遇抢占优先换地址。

普通付款到恶意 commit 地址的反例必须保留为预期可能通过的测试；此路径的来源参与不能宣传为业务内容授权。额外签名可另案增强，不属于本次前置条件。

# 验收向量

下表是规范期望，尚不是实现已经通过的测试报告。余额单位均为 sat；历史资格是在该 mint ordered event 前观察的状态。来源合规表示真实 sat 映射与签名覆盖均已验证。

| 编号 | 输入条件 | 期望 |
| --- | --- | --- |
| M01 | E 余额 0、无有效历史、prev 空，第三方出资 | 首次开户 Active |
| M02 | E 余额 1、无有效历史、D!=E、prev 空 | Invalid；不得绑定既有余额 |
| M03 | E 余额 0、曾有有效 pass，D!=E、prev 空 | Invalid；归零不能再次豁免 |
| M04 | E 只有 Invalid 历史、余额 0、prev 空 | 不占用历史资格，可开户 |
| M05 | E 有既有 Active，攻击者出资，prev 空或引用 E 的 pass | Invalid；原 Active、能量与绑定不受该 mint 改变 |
| M06 | D==E，来源合规，prev 空 | 同地址 remint；旧 Active Dormant |
| M07 | D!=E，E 可开户，prev 为 D 的 Active 与 Dormant，来源合规 | 原子跨地址继承；指定 prev Consumed |
| M08 | M07 中包含另一 owner 的 prev 或重复/终态 prev | 整个 mint Invalid；合法 prev 也不部分消费 |
| M09 | D!=E，E 有余额或有效历史，来源合规且 prev 合法 | Invalid；不覆盖目标账户 |
| M10 | E 块前 0，同块前一笔普通交易给 E 1 sat | 不满足开户余额条件 |
| M11 | E 块前 1，同块此前花费全部余额，且无有效 pass 历史 | reveal 前余额 0；可走首次开户，不要求完整 BTC 历史从未使用 |
| M12 | 同一 reveal 有两个指向 E 的独立输入 mint，前余额 0 | 共用前余额；首次成功后第二个不得再次使用开户豁免，仍可单独满足同地址路径 |
| M13 | 同一交易先发生有效 pass transfer 到 E，再执行第三方 mint | transfer 占用资格，后续 mint 不获开户豁免 |
| M14 | 所需 H-1 余额、commit 或 prevout 暂缺 | 暂停/回滚该块并重试，不记录永久 Invalid |
| M15 | 断开首次开户所在块，再以另一规范分支重放 | 撤回该分支的资格占用，其他历史资格保留 |
| M16 | 激活高度 H-1 / H / H+1 出现新 v1 mint | H-1 使用原规则；H 起拒绝；旧 v1 prev 不因此作废 |
| M17 | D 普通付款到恶意预承诺 commit，后 reveal 回 D，来源合规 | 可以通过；是明确接受的业务授权边界 |
| M18 | standard/collab 各执行 M01/M05/M07，Leader 从 D 迁 E | 相同资格规则；旧协作者绑定不自动跟随 |

sat 映射数值向量：commit 两输入分别为 A=700、D=2300；输出依次为 A 找零 700、commit C=1200、其他找零 1000（fee=100）。C 的 offset 0 位于累计输入偏移 700，恰好属于输入 1 的 D，不能取输入 0。将首输出改为 600、其他找零改为 1100 后，C 的 offset 0 位于 A；即使输入列表仍有 D，也不能取得 D 权限。

来源签名向量必须覆盖三类允许形式、ALL/DEFAULT、NONE/SINGLE/ANYONECANPAY、Taproot script-path/annex、非首输入和零值输入。不支持的合法 BTC 花费必须得到一致的 USDB 结果；RPC 断流必须是可重试错误。上述抽象向量还需扩展成可复现的原始签名交易、canonical mutation 和重放结果。

# 参考实现与待完成事项

当前行为的代码入口和分批任务见 [实施计划](../usdb-indexer/miner-pass-operation-eligibility-plan.md)。在启用 v2 前还必须完成：Ord 子集与签名序列化向量审查、可用的历史来源取证、资格派生/缓存及回滚、原子跨地址状态机、版本/承诺兼容、RPC 与控制面核验、真实签名 regtest 验收。

本草案中的 v2 值是实现候选，不修改现有 embedded registry、testnet-v0 bundle、chain genesis 或在线服务。正式激活高度和节点重置另行按发布流程冻结。
