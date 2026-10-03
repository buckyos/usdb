UIP: UIP-0016
Title: Miner Pass Operation Eligibility and Cross-Owner Inheritance
Status: Draft
Type: Standards Track
Layer: BTC Application / Consensus Input
Created: 2026-10-02
Requires: UIP-0000, UIP-0001, UIP-0002, UIP-0003, UIP-0004, UIP-0006, UIP-0008
Supersedes: UIP-0001 v1 mint eligibility and UIP-0002 v1 same-owner inheritance for fresh development networks
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
- `ever_valid_owner` 必须来自本作用域自 index origin 起的规范历史，包含该作用域全部有效历史。可以使用可重建的物化索引，但不得在升级、重启、地址归零或切换 current registry 时清空。
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

Ord envelope 的 `offset` 是 envelope 序号，不可直接当作 sat 偏移。Ord pointer 可以改变 sat 归属；不得无条件使用旧实现的 `offset=0`。

当前唯一执行的 v2 证据层采用以下明确子集：

- reveal 花费原生 P2TR 输出，叶版本为 `0xc0`，并校验 script/control block 与 commit 输出公钥的承诺关系。
- 每个 reveal input 仅允许一个 envelope；可以位于非首输入，不把 inscription index 当作 input index。首版也拒绝同输入混合其他协议的多 envelope，比仅拒绝多个 USDB mint 更保守。
- 不接受 pointer（包括指向默认位置或编码无效的 pointer）、pushnum、stutter、duplicate field、incomplete field、unrecognized even field。
- 对支持的 envelope，实际铭文位于对应输入的首 sat；输入金额必须非零。输出位置仍按所有前置输入金额与输出区间计算，绝非固定 reveal output 0。
- 无绑定 sat、进入手续费或不可花费输出的形式不支持。普通接收脚本不要求 P2TR。

上述拒绝是确定的“不支持”，不是数据缺失；缺少 Core 证据则仍为可重试错误。来源 commit 若为 coinbase，没有签名输入，不能用于同地址/跨地址来源授权；真正符合首次开户条件的路径无需取得来源授权。子集已接入逐块校验 active set 的生产区块管线，并有隔离 RPC/双库存储测试；网络激活前仍须完成真实服务整体验收。

依赖来源的路径，首版支持范围为：

| 输入 prevout 类型 | 必须识别的花费形式 | sighash |
| --- | --- | --- |
| P2PKH | 标准单签 scriptSig，与 prevout 公钥哈希匹配 | ECDSA ALL (`0x01`) |
| 原生 P2WPKH | 单签 witness，与 prevout witness program 匹配 | ECDSA ALL (`0x01`) |
| P2TR | 明确识别 key-path；不能把 script-path 的某个 witness item 误作签名 | DEFAULT（64 字节签名）或 ALL（65 字节且末字节 `0x01`） |

不接受 NONE、SINGLE、ANYONECANPAY；首版不覆盖其他脚本组合。Taproot annex 按 BIP-341 从 witness 最后一项识别并纳入签名哈希，移除 annex 后必须恰好剩一个 key-path 签名。64 字节为 DEFAULT，65 字节只允许末字节 `0x01`；不接受显式附加 `0x00`。不能仅凭原始 witness 总长度猜测 key-path。P2PKH 要求两个最小 push 的标准 scriptSig 和空 witness；P2WPKH 要求空 scriptSig、两个 witness item 与压缩公钥。除公钥哈希/输出公钥匹配外，还对完整交易摘要实际执行 ECDSA/Schnorr 验签。该白名单约束 USDB 来源证明，不改变 Bitcoin 交易有效性，不限制接收地址必须为 Taproot。

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

当前开发测试网可彻底重置，生产程序只支持 MinerPass v2，不保留 v1 parser/状态机或历史执行分支。新 mint JSON 使用整数 `v: 2`，沿用现有业务字段，不新增 `sig/src/dest`；source/destination 从链上取得。

| 字段 | 本次唯一执行版本 |
| --- | --- |
| inscription_schema_version | uip-0001-miner-pass-inscription:v2 |
| pass_state_machine_version | uip-0002-pass-state-machine:v2 |
| energy / effective-energy / level formulas | 数学规则不变，保持 v1 |
| query / state-view / commit protocol | 保持原版本；新增辅助审计使用独立 schema |

registry 的 BTC source、rules scope、revision、按高度查询及 active-version/state identity 框架保留。启动、每个区块、历史查询和 checkpoint 验证只接受上述成对 v2；v1、混合或未知组合拒绝执行。所有已处理高度的新 `v:1` mint 都记为 Invalid，不能选择宽松执行器。重置后的网络从 origin 使用 v2，不导入或继承旧开发网的 v1 pass/能量/状态。

未来正式网若升级到 v3/v4，必须另行实现并测试升级前历史语义、激活边界和重放兼容；保留框架不表示当前已实现这些未知版本。

若 qualification 完全由已有已承诺历史派生，缓存不是独立共识状态；若新增不可派生字段或改变规范编码，则必须定义其 commitment、版本和快照恢复规则。不能只升级一个版本字符串。尤其当前 commit 版本还关联 balance-history，必须区分 pass mutation 编码与上游快照协议的影响，避免无必要地改变 BTC 基础数据身份。

本次实现保留 `PassBlockMutation` 的字段和编码、逐块 rolling commit、经济 query/state-view 及 balance-history 版本。新资格依赖规范链输入和既有持有历史，结果仍使用已有 mint/Invalid/状态转移 mutation；`active_version_set_id` 把成对 v2 和作用域纳入现有 local/system state 身份。Rust/Go 使用同一隔离 catalog 的黄金向量互验，保持旧默认 catalog/ID 原样；Go 额外识别显式选择的隔离 regtest v2 catalog，不改变默认 chain config。

v2 的规范事件集合由当前完整区块按锁定 Ord parser 解析。若配置的数据源漏报、重复、内容/有效分类不一致，或使用非规范 envelope 编号，整块作为数据源错误停止并重试，不能静默接受差异。执行使用链上规范内容和编号。

v2 的 Invalid 记录沿用原 mutation 格式，但无受支持 sat 位置的铭文使用明确占位：`mint_owner` 为全零 32 字节 script hash，satpoint 为 `<reveal_txid>:4294967295:0`；这些值不代表收款地址或可花费输出，不能占用开户资格或用作有效 pass 跟踪种子。此时辅助审计的 `recipient=null`。有受支持 sat 位置的 schema Invalid 仍记录真实 owner/satpoint；schema 校验错误优先于 envelope 不支持错误。旧 v1 网络记录仅作历史资料，不由新程序重放。

辅助表 `miner_pass_mint_audit` 记录已完成 v2 尝试的来源位置/签名形式、交易前余额、历史资格、成功路径或协议拒绝原因。它与 pass 状态在同一个区块 savepoint 发布和回滚，不作为资格输入，也不新增到 mutation root。运行时缺证据/I/O 失败中止整块，不持久化 Invalid 或成功审计；日志上下文不进入 canonical Invalid reason。审计可由规范链和持有历史重放生成，不是独立的来源授权证明或 Merkle proof。RPC 语义见 [get_pass_mint_audit](../usdb-indexer/usdb-indexer-rpc-v1.md#8a-get_pass_mint_audit)。

| BTC source | USDB rules scope | 激活高度 | 状态 |
| --- | --- | --- | --- |
| btc-regtest | miner-pass-v2-fixture | 自 H=0 | 隔离开发 catalog；Rust 显式 pin，Go 可显式选用；不是默认网络 |
| btc-mainnet | 现有 legacy / testnet-v0 | 不添加本文激活 | 默认 catalog/ID 冻结；新程序拒绝其 v1 规则 |
| btc-mainnet | 待发布 testnet-v1 | 待冻结网络包 | Planned |
| btc-mainnet | 未来正式网作用域 | 独立审议 | Planned |

代码合并不等于激活。registry 隔离、USDB chain checkpoint 绑定、Go 版本支持及发布包必须一致。P2P fork ID 仅纳入 USDB checkpoint 高度，不替代对 BTC 高度规则与版本身份的验证。

本次不提供旧开发网在线升级或数据迁移。发布前须冻结新的作用域、catalog 和网络包，以全新数据集启动；实际重置另行实施，不能把旧 registry ID 重新解释成 v2。

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
| M16 | origin 起任意高度出现 v1 mint；registry 指定旧/混合/未来规则 | v1 mint Invalid；不支持的规则拒绝执行，未来边界在业务写入前停止 |
| M17 | D 普通付款到恶意预承诺 commit，后 reveal 回 D，来源合规 | 可以通过；是明确接受的业务授权边界 |
| M18 | standard/collab 各执行 M01/M05/M07，Leader 从 D 迁 E | 相同资格规则；旧协作者绑定不自动跟随 |

sat 映射数值向量：commit 两输入分别为 A=700、D=2300；输出依次为 A 找零 700、commit C=1200、其他找零 1000（fee=100）。C 的 offset 0 位于累计输入偏移 700，恰好属于输入 1 的 D，不能取输入 0。将首输出改为 600、其他找零改为 1100 后，C 的 offset 0 位于 A；即使输入列表仍有 D，也不能取得 D 权限。

来源签名向量必须覆盖三类允许形式、ALL/DEFAULT、NONE/SINGLE/ANYONECANPAY、Taproot script-path/annex、非首输入和零值输入。不支持的合法 BTC 花费必须得到一致的 USDB 结果；RPC 断流必须是可重试错误。上述抽象向量还需扩展成可复现的原始签名交易、canonical mutation 和重放结果。

# 参考实现与待完成事项

当前行为的代码入口及分批验收见 [实施计划](../usdb-indexer/miner-pass-operation-eligibility-plan.md)。唯一 v2 parser/状态机、按高度支持检查、真实区块执行器/tracker、审计 RPC 和回滚重放已在隔离 Core/BH RPC 夹具及真实 SQLite/RocksDB 上接通。仍须完成控制面引导/核验、签名 checkpoint 导出安装、真实 Core/AssumeUTXO/BH 完整管线与历史取证整体验收，才能进入发布激活。

本草案中的 v2 值已用于隔离开发执行；不修改旧默认 embedded registry、testnet-v0 bundle、chain genesis 或在线服务。正式激活高度和节点重置另行按发布流程冻结。
