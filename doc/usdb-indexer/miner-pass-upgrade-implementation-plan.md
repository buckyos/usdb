# UIP-0017 多区间升级实施与测试计划

状态：A 已提交为 `4dd2333`；B 已提交为 `72e6a87`；C 已提交为 `2ca2694`；D 已完成本地实现与验证，待评审提交；E–G 尚未实施。

协议依据：[UIP-0017](../UIP/UIP-0017-miner-pass-upgrade-and-legacy-rights.md) 第一版草案已由用户确认并提交为 `36726a5`；这不表示委员会流程完成或目标网络已激活。本文记录当前实现缺口、分批交付和测试验收，不分配正式 schema v2 / 状态机 v3，不改变现行网络参数。

## 1. 目标与交付边界

支持同一 pass 跨多个规则区间，保留其合法铸造历史，按各事件高度执行增长、操作及继承。区块写入、只读历史查询、恢复和 USDB validator 必须使用一致的历史上下文。

第一轮交付是可扩展的执行框架和隔离验收：生产版本继续只注册现有 schema v1 / 状态机 v2 / 公式 v1。未来具体升级须另行定义业务版本、兼容转换、网络高度及激活配置。旧的、不安全的状态机 v1 不恢复执行。

通用框架提供显式边界转换的执行位置和恢复约束；实际冻结、扣减、撤销资格或后代追踪规则仍须有独立 UIP，不能加入运营者可随意调用的作废接口。

## 2. 当前实现与缺口

以下为制定计划时的源码核查结果；A–D 批次的交付与验证见第 7–10 节，E–G 仍为待实施内容。

| 模块 | 已有基础 | 所需补充 |
| --- | --- | --- |
| [activation.rs](../../src/btc/usdb-util/src/activation.rs) | registry revision、scope、按高度 lookup、未知规则拒绝；`validate_btc_indexer` 只接受现有组合 | 区间枚举与版本族边界合并；区分结构合法、实现能力及上下文解析；为执行器提供完整规则上下文。 |
| [source.rs](../../src/btc/usdb-indexer/src/inscription/source.rs)、[content.rs](../../src/btc/usdb-indexer/src/index/content.rs) | 源无关 reveal 发现、严格 schema v1 分类 | 将 reveal 高度选出的 schema 契约传入分类器；不能只改 `MINER_PASS_MINT_SCHEMA_VERSION` 常量。 |
| [mint_rules.rs](../../src/btc/usdb-indexer/src/index/indexer/mint_rules.rs)、[eligibility.rs](../../src/btc/usdb-indexer/src/index/pass/eligibility.rs) | 本地重解析、来源/余额/prev 校验、失败原子性 | 本地和外部 discovery 使用相同上下文；通过规则执行器保持旧资格并约束新操作。 |
| [energy.rs](../../src/btc/usdb-indexer/src/index/energy.rs)、[energy_formula.rs](../../src/btc/usdb-indexer/src/index/energy_formula.rs) | 懒结算、余额惩罚、历史投影、终态冻结 | `calc_incremental_growth`、`settle_active_balance_change`、projection 都直接使用固定公式；须共享分段结算内核。 |
| [pass.rs](../../src/btc/usdb-indexer/src/index/pass.rs) | 当前 owner/state 校验及逐 prev 折损后求和 | 消费前按路径结算到事件高度，验证转换/兼容声明，使用该高度的继承策略。 |
| [effective_energy.rs](../../src/btc/usdb-indexer/src/index/effective_energy.rs)、[server.rs](../../src/btc/usdb-indexer/src/service/server.rs) | 历史快照、候选集/协作批量派生、带 registry 的查询身份 | raw/effective/level/factor 同时贯通上下文；删除业务路径对“当前固定公式”的隐式依赖。 |
| [indexer.rs](../../src/btc/usdb-indexer/src/index/indexer.rs)、[pass_commit.rs](../../src/btc/usdb-indexer/src/index/pass_commit.rs) | 块前版本校验、ordered events、SQLite savepoint、RocksDB pending/finalize、tracker 恢复与 mutation commit | 边界转换须进入同一原子执行/回滚流程，空块也执行必要边界动作；派生缓存不能改变 mutation 序列。 |
| [indexer_rules.rs](../../src/btc/usdb-util/src/indexer_rules.rs)、[storage/energy.rs](../../src/btc/usdb-indexer/src/storage/energy.rs) | source/scope/origin/精确 registry ID 的双库绑定；记录保存能量、年龄、余额和高度 | 明确恢复记录的状态表示；现有绑定禁止更换 current ID 后原目录续写，不能取消该保护来实现升级。 |
| `go-ethereum/internal/usdb/{activation,btc_activation_registry,profile_formula}.go` | scoped registry 向量、profile surface、按版本的公式检查入口 | 新支持的 surface/公式必须有独立实现和黄金向量；不得仅接受未知版本字符串。 |

现有 [miner_pass_activation.rs](../../tests/miner_pass_activation.rs) 验证的是当前规则、未知未来组合拒绝、继承及恢复；不是两个已支持 MinerPass 版本之间的成功切换。现有 Go `activation-upgrade` 和 `economic-activation-upgrade` 也不能代替 BTC pass 三阶段执行验收。

Go 当前对 raw energy 做版本支持及数值表示检查，并校验 effective energy/level 等派生关系，不重放全部 BTC 能量账本。BTC 累计正确性必须由 Rust 独立参考模型、完整索引与重放对照证明，不能只以 Go 接受 profile 为验收依据。

## 3. 设计决策

### 3.1 共享规则时间表与显式上下文

在 `usdb-util` 增加只读时间表视图（类型名称实施时确定），复用现有 registry，不创建第二份激活配置。提供指定高度 lookup、区间迭代、所需版本/转换的支持检查，以及两个 revision 在目标历史前缀内的执行语义比较。

上下文至少携带 BTC source、rules scope、精确 registry ID、目标高度及 active version set。区块执行在写入前解析一次并向下传递；历史查询按请求上下文解析。版本族独立分派，不能把 schema/state/energy 的数字强行绑定。

pass 保留已接纳的铸造事实和状态历史。第一批不为每张 pass 复制时间表，也不新增不可派生的“出生规则”共识字段。若后续状态表示确实需要新增持久字段，单独评审编码、commit 与快照影响。

### 3.2 先固定现有行为，再引入多规则路径

首先把现有算法封装为明确的版本实现，再接入路由。生产注册表只包含已定义、已支持的组合，不支持用户注入公式或默认采用最高版本。

未知未来版本可以存在于尚未到达的 registry 区间；已处理区间必须完全受支持。到达不支持的区块时保持现有“业务写入前停止”语义。查询跨过未知区间也应报错，不能返回部分累计值。另提供只读的未来支持检查用于提前暴露升级需求，不能由检查结果暗中激活规则。

### 3.3 分段结算内核

用一个无数据库副作用的内核接受已有状态、目标上下文和规范事件，合并规则边界、余额变化及 pass 状态变化，依序计算。写入结算与只读投影共用此内核；调用者决定持久化哪些规范记录。

时间表应可缓存并支持按高度定位，候选集批量查询共用已解析区间。生产结算复杂度应随实际规则边界和余额/状态事件数量增长，避免对每张 pass 从 mint 高度逐块扫描；逐块模型用于独立测试对照。

必须保留 `energy`、`owner_balance`、`active_block_height`、pass state 等足够信息。不得把 `active_block_height` 当作增长结算起点。每个升级边界有显式兼容或转换步骤；跨三个区间需要走完两次转换，不推定直接跳转兼容。

第一版测试约定：状态记录代表已处理完高度 h；从 a 到 b 的单位块结算以 `(a,b]` 的目标块高度选择规则，H 块属于新规则。该约定在单版本下须严格退化为 UIP-0003 的 `b-a`，并用 H-1/H/H+1 独立向量锁定。涉及非恒等状态转换时，增长、转换、H 块事件的先后顺序必须由具体测试规范/未来升级 UIP 明确，不推定所有版本共用一种顺序。

余额扣减、转移、burn、remint 继续遵循已有块内顺序。新增框架不能借机改变单版本既有行为。特别覆盖同高度多操作、余额不足一个 unit、逐 prev 舍入与饱和边界。

### 3.4 存量状态、转换与承诺

默认升级不重新检查存量 mint 准入。当前高度的继承操作读取该高度的规范状态，沿已定义路径结算/转换后，再应用当前继承条件及折损。

区分两类边界处理：

- 可由已承诺历史唯一派生的公式切换：可以保持稀疏能量记录并惰性投影，但各种执行/查询方式必须结果一致。
- 改变持久化状态或权益的转换：必须定义确定的 canonical 动作/顺序和状态表示。需要新 mutation 类型或新增非派生字段时，先评审 commit/state-view/storage 版本，不能直接扩展现有序列化后沿用旧版本。

块级转换纳入现有 savepoint/pending-energy/tracker 发布链。不能根据“本次恰好查询了哪些 pass”产生规范状态转移。大规模存量处置是否全量物化或可证明等价地派生，留给具体 UIP 的性能与承诺设计；本轮不虚构通用批量作废机制。

### 3.5 历史查询不能只更换版本标签

对指定 registry 的查询，必须确认数据集执行的历史与所请求 revision 在相关 origin 至目标高度范围内一致。只比较目标高度的 active set 不足以证明一致：中间曾使用不同规则，即使最后版本相同，累计状态也可能不同。

历史兼容 revision 可共享一致前缀的数据；规则分歧后的状态禁止直接换标签复用。应明确拒绝该上下文或路由到经验证的独立数据集，不能偷偷重算或改写当前库。包括 RPC 缓存、分页 cursor、candidate set、审计记录和系统状态身份在内，都须保持这一边界。

### 3.6 数据集升级单列

第一轮继续保留精确 registry ID 绑定。可在一开始就加载包含多个未来区间的隔离 catalog，证明同一数据集跨高度执行；也可用独立新数据集从 origin 重放升级后的历史。

“协议支持按高度升级”不等于“已支持向运行中的数据集追加 current registry”。未来原地升级需独立实现：检查同 source/scope/origin、记录不可改写、已提交历史前缀等价、未来规则支持情况；在安全停写边界协调双库身份与恢复记录，覆盖崩溃/回退。该阶段完成前，不修改 `upgrade-release` 让它绕过现有绑定或承诺免重建。

## 4. 分批实施顺序

每批同时补测试，不把测试集中推迟到最后。所有批次在现行网络行为不变的条件下开发；未来协议启用另行验收。

| 批次 | 修改范围与交付 | 退出条件 |
| --- | --- | --- |
| A：时间表与上下文 | registry 区间/前缀比较、版本能力边界、typed context；接到块前校验，保持现有执行器 | 单版本结果与已有向量一致；多族边界无漏段；未知区间 fail closed；不改网络身份。 |
| B：schema / 状态机分派 | discovery 与本地重解析统一上下文；固定现有 schema v1/state v2 执行器；继承资格上下文 | 真实 ordered pipeline 验证 reveal 边界、新旧 mint 分类、旧证保留和失败无副作用；仅测试构建注册额外规则。 |
| C：能量与继承分段 | 共用纯结算内核；接入余额更新、投影、Dormant/burn/consume 与逐 prev 折损 | 三阶段参考模型逐块对拍；惰性和事件驱动一致；所有当前单版本数值回归通过。 |
| D：边界执行、存储与恢复 | 必要转换进入区块原子流程；明确状态表示和 snapshot/checkpoint 恢复；审计 commitment 是否需升级 | 空块触发、重复重启、各发布故障窗口、跨多个边界 reorg 全部一致，不能只比较进度高度。 |
| E：完整查询与 Go 联调 | 所有经济 RPC 采用同一上下文；历史前缀校验、缓存/cursor；Rust/Go scoped 向量与验证入口 | profile/energy/candidate/collab/level 互相一致；分歧 registry 拒绝换标签；独立 Go validator 接受规范分支并拒绝篡改。 |
| F：真实服务与 CI | 真实 Core/Ord/BH/indexer/Geth 升级测试、连续运行与重放对照；接入 nightly/weekly | 留存完整配置、二进制标识、区块/状态/能量/commit 证据；普通发布包不支持测试专用规则。 |
| G：数据集原地升级（后续独立批次） | 历史前缀等价证明、双库身份切换与中断恢复、工具兼容策略 | 不修改已提交历史，停机续写与新库重放一致；未通过前继续走独立 dataset。 |

A–F 建立协议执行能力，G 改善运维升级路径。具体生产 schema/state/公式版本的定义和激活不包含在这张交付表中。

## 5. 测试夹具与独立预期值

复用 [tests/common/miner_pass_pipeline.rs](../../tests/common/miner_pass_pipeline.rs) 的真实 SQLite/RocksDB、tracker、Core/BH RPC 夹具；函数单测留在模块中，跨模块用例放仓库根 `tests/`，共享辅助放 `tests/common/`。

建议新增的测试文件随对应批次创建，不预先写空测试：`tests/miner_pass_upgrade_rules.rs`、`tests/miner_pass_upgrade_energy.rs`、`tests/miner_pass_upgrade_recovery.rs`、`tests/miner_pass_upgrade_queries.rs`。

隔离测试定义 origin < H1 < H2 的三个阶段：至少两段采用不同增长参数；至少一次非恒等转换使跳过边界可被检测；单独设置 schema 接受集合变化及新 mint 准入收紧。另设只升级一个版本族的对照，避免多项同时变化掩盖分派错误。

测试规则使用明确的 conformance 身份，不冒充正式 v3/schema v2 激活。优先通过 `cfg(test)` 注入；真实服务阶段若需 Cargo feature/Go build tag，必须同时限定 regtest 与专用 rules scope，且正常发布二进制拒绝测试 catalog。现行公开 registry、genesis、黄金向量不替换为测试数据。

独立参考模型按块推进，使用手工可核对的整数向量/大整数上界计算，不调用被测路由或分段函数来生成预期值。对比最终数值，也对比每个关键高度的状态、owner、prev 消费、审计和 commit。未知未来版本、撤回的 JSON v2 和旧状态机 v1 拒绝测试继续保留。

## 6. 验收矩阵与 CI 分层

| 编号 | 核心场景与断言 | 主层级 |
| --- | --- | --- |
| U01 | 每个版本族独立 lookup、同高合并、Planned 不执行、scope 隔离；区间连续且唯一 | fast |
| U02 | H1/H2 的 -1/0/+1 边界、commit 在前 reveal 在后；mint 当块/空区间计数无偏移 | fast + nightly |
| U03 | 不同铸造区间的旧 Active 保留、新同类 mint 被拒；Dormant/终态不复活 | fast + nightly |
| U04 | 三阶段增长、部分减仓/清零/再入金、单位年龄、超大数及饱和；参考模型逐高度对拍 | fast |
| U05 | 长期无交易跨多次升级；连续逐块、惰性投影、余额事件结算和多次读取一致 | fast + nightly |
| U06 | 非恒等转换、兼容声明缺失、逐段路径缺失、终态表示处理；不得默认跳过 | fast |
| U07 | 多个不同铸造阶段 prev、同址/跨地址继承、逐项折损、重复消费、无授权来源 | fast + nightly |
| U08 | 继承/转移/burn/余额变化与 H 同块，失败不部分消费；边界动作的明确先后顺序 | fast + nightly |
| U09 | Leader/collab 固定 ID 与地址绑定、有效能量聚合、资格变化、候选集排序 | fast + nightly |
| U10 | energy/profile/candidate/collab/range/exact/at-or-before/审计接口语义；只读请求不写库 | fast |
| U11 | registry 相同前缀可读；不同中间历史但末尾版本相同仍拒绝复用；错误 scope/ID/cursor | fast + nightly |
| U12 | H 块无 pass 交易仍处理必要转换；能量 finalize 前后、tracker 发布、SQLite 提交失败 | fast + nightly |
| U13 | 从 H2 后重组回 H1 前并重放；权益/能量/prev/审计/commit 全一致，旧分支缓存失效 | nightly + weekly |
| U14 | 各区间 snapshot 恢复和离线节点跨多个区间追赶；从 origin 重建与持续运行一致 | nightly + weekly |
| U15 | 普通构建拒绝 conformance；不支持版本在生效块前停止，历史可读部分保持准确 | fast + nightly |
| U16 | 双链 checkpoint 与 BTC anchor 边界、独立 validator 重放；分别观察累计账本和链上奖励 | nightly |
| U17 | 随机余额/转移/继承/协作、反复跨边界 reorg、故障恢复；固定 seed、可重放事件日志 | weekly |
| U18 | 追加规则的数据身份升级、中断/回退、未提交与已提交高度边界 | G 批次独立验收 |

fast 使用 Rust 模块与根目录管线测试、Go profile/activation 黄金向量；遵循项目 fmt/clippy/check 门槛。nightly 新增明确命名的 MinerPass 多区间用例，接入 Go `scripts/usdb/run_long_ci.sh` 的 indexer-protocol/go-activation 相关阶段并评估时长，不用现有成功标签冒充新覆盖。

weekly 在已有 world-sim/reorg/recovery 设施中扩展固定 seed 矩阵。每次场景报告保存源码 SHA、catalog/作用域、激活高度、工具版本及日志。服务 ready 必须满足各依赖的实际同步/索引条件，不能只检查进程存在；测试后仅停止自己的隔离服务。

## 7. A 批次交付与当前验证状态

### 7.1 已实现

- [activation_timeline.rs](../../src/btc/usdb-util/src/activation_timeline.rs)：从既有 registry 构建不可变时间表，合并全部 Active 版本族边界，复用原有 lookup 与 identity 算法。提供按高度查找、包含首尾高度的区间枚举、区间能力检查及完整历史前缀比较；定位复杂度为 O(log 边界数)，区间遍历不逐块扫描。
- `BtcRuleContext` 绑定 BTC source/rules scope、精确 registry ID、目标高度和受支持的 active version set；字段私有且共享不可变缓存，不能由调用方直接改写高度或身份。检查声明规则与取得可执行上下文分开，未知未来版本可以被检查，但不能进入执行。
- [indexer.rs](../../src/btc/usdb-indexer/src/index/indexer.rs)：缓存 catalog 各 revision 的时间表，在区块业务写入前解析上下文并传给 mint 收集/事件处理，校验事件高度一致。启动时在绑定/历史游标修复前检查 origin 至已处理高度的每个区间，防止末端版本相同却掩盖中间不支持的规则。
- 原 active-version 查询方法通过上下文复用既有接口；没有修改生产版本常量、network/registry/genesis 配置、active-version-set hash、双库绑定格式或数据布局。

### 7.2 测试与验证（2026-10-05）

新增 [miner_pass_upgrade_rules.rs](../../tests/miner_pass_upgrade_rules.rs) 的 11 个用例覆盖多族独立选择、同高激活、三段时间表、H-1/H/H+1、最大高度、非 Active 状态、缺失规则、无效 registry、作用域隔离、不可变上下文，以及中间规则不同但末端相同的前缀比较。既有 fixture 的固定 identity 向量同时保持不变。

扩展 [miner_pass_activation.rs](../../tests/miner_pass_activation.rs) 的真实 SQLite/RocksDB 管线：schema/state/energy 的未知或已撤回版本均在块写入前拒绝，原有 pass/审计/进度保持一致；验证 exact registry 上下文与审计 identity；模拟存储经过不支持的中间区间，重启必须拒绝且不改写进度、绑定或历史游标。

本地验证：

- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-util`：87 通过、2 忽略。
- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer`：348 通过、10 忽略。
- workspace `cargo check`、相关两 crate 的 `cargo clippy --all-targets -- -D warnings`、`usdb-util` 公共 API 文档构建和格式检查通过。

### 7.3 明确保留的后续边界

当前生产执行器仍只支持 schema v1 / 状态机 v2 / 公式 v1。三段用例验证时间表和拒绝边界，不代表已经执行三种能量公式。能力检查还不涉及未来的状态转换函数。

前缀比较目前是共享底层能力，完整 RPC 历史一致性防护留给 E；不能据此绕过精确 registry ID 的数据绑定。旧数据原地升级仍属 G，没有修改升级工具或触发网络重置。本批没有运行在线服务升级，也没有触发远端 CI。

B 的后续交付见第 8 节。C 实现分段结算，D/E 补齐状态恢复与查询一致性，F 完成真实服务及 nightly/weekly 验收。

## 8. B 批次交付与当前验证状态

### 8.1 已实现

- [rules.rs](../../src/btc/usdb-indexer/src/index/rules.rs) 明确区分编译期 schema 与状态机执行器，按 `BtcRuleContext` 独立选择。生产支持集合仍为 schema v1 / 状态机 v2；未注册的版本没有默认回退。
- [source.rs](../../src/btc/usdb-indexer/src/inscription/source.rs)、compare source 和 indexer 本地重解析统一接收 reveal 高度的上下文，拒绝混入其它高度或 BTC 网络的铭文。保留原始 JSON 的严格校验，重复字段不会因版本分派而被重新序列化抹掉。无上下文的内容工具接口仍明确使用冻结的 v1 契约，不用于区块 discovery。
- [content.rs](../../src/btc/usdb-indexer/src/index/content.rs) 固定 v1 字段语法并显式分派；payload `v` 的契约与默认夹具版本常量解耦，未来添加执行器必须同时声明对应解析路径。
- mint、Invalid 记录、转移与 burn 都经过状态规则入口。mint 按当前 schema 处理新 payload，按当前状态机执行准入及 prev 校验；已有 pass 的铸造事实不按当前 schema 重解析。状态入口在写入前检查事件高度、BTC 网络、rules scope 及配置中固定的 registry ID。
- 时间表允许使用编译期执行器能力检查，按同一检查缓存各区间的支持结果，startup、块前校验和版本查询保持一致。生产没有动态加载规则或通过配置开关放宽版本支持的入口。

### 8.2 隔离执行器与验收（2026-10-05）

仅在 `cfg(test)` 下注册两项 conformance 规则，且要求 `btc-regtest` 与 `miner-pass-upgrade-conformance` 同时匹配：schema 使用独立 wire marker `901` 和严格 v1 字段语法；状态规则保留 v2 生命周期，但拒绝新的协作 mint。它们不代表正式 JSON v2 或状态机 v3，也不修改公开 catalog。

[miner_pass_upgrade_dispatch.rs](../../tests/miner_pass_upgrade_dispatch.rs) 新增 9 项测试，复用真实 SQLite/RocksDB 和 Core/BH RPC 夹具：

- H-1/H/H+1 的 schema 切换；commit 在 H-1、reveal 在 H 时使用 H 的规则，compare 双源和本地重解析一致。
- 新 schema 的铭文继承旧 schema 的 pass；在区块发布失败后完整回滚 pass/prev、审计、能量和进度，修复故障后重试成功。
- 只收紧状态机准入时，旧协作证继续有效并增长，新协作 mint 失败不消费 prev，后续合规继承仍可消费旧证；payload 继续使用 v1。
- 错误高度、网络、规则域、registry revision 被拒绝；测试规则不能越过 scope 或其它版本族的支持检查。
- 外部来源使用陈旧 schema 分类时，本地 canonical 重解析拒绝该区块；不留下业务记录，恢复来源后可重试。
- 新选择的 schema 仍拒绝重复键、未知字段、错误类型及撤回的 JSON v2。
- 旧 schema 的 pass 在新 schema/state 区间正常转移或 burn；随后空块继续保持 Dormant/Burned，不重新激活。

[miner_pass_production_dispatch.rs](../../tests/miner_pass_production_dispatch.rs) 通过 Cargo 集成测试直接启动普通、非 `cfg(test)` 的 `usdb-indexer` 二进制，分别验证 conformance schema/state 在打开 pass/energy 存储前被拒绝。测试只使用临时目录、不获取全局进程锁，并为自身子进程设置退出等待上限。

本地验证：indexer 单元/管线测试 357 通过、10 忽略，普通二进制集成测试 1 通过；`usdb-util` 87 通过、2 忽略。workspace 编译、相关 crate 的 Clippy（全部 targets、warnings 视为错误）、格式和公共 API 文档构建通过。既有 Rust/Go BTC activation 及 testnet-v1 activation 黄金向量检查通过；新增集成 target 由现有 fast gate 的 `cargo test --workspace` 自动覆盖，尚未触发远端 CI。

### 8.3 下一批与保留边界

生产 registry、genesis、版本常量、持久化字段、identity 算法和现行能量公式保持不变。本批不要求数据重建或网络重置；尚未进行在线升级或远端 CI 验收。

B 交付时的能量增长与继承仍执行现有 v1 公式；C 的分段结算交付见第 9 节。完整多区间历史 RPC、reorg、快照及 Go validator 验收仍按 D–F 推进；原地更换数据集 registry 的 G 批次继续独立。


## 9. C 批次交付与当前验证状态

### 9.1 已实现

- [energy_settlement.rs](../../src/btc/usdb-indexer/src/index/energy_settlement.rs) 提供无存储副作用的共享内核，绑定当前数据集的不可变 registry 时间表。记录高度表示该块已完成；增长覆盖 `(record_height, target_height]`，逐段选择公式，逐边界显式检查转换/兼容路径。不会从 mint 高度逐块扫描，也不会用 `active_block_height` 替代结算起点。
- 生产执行器继续封装现有 UIP-0003 v1 的整数增长、penalty、年龄重置和继承折损。测试构建才注册额外公式，并且继续同时限制 regtest 与 conformance scope；未知规则、缺失第一或后续转换、反向投影均明确报错。
- 余额事件写入、余额历史懒结算、只读 raw-energy 投影及 Dormant/burn 使用同一内核。懒结算先检查整个规则路径，防止后续规则不支持时已经写入前半段事件。只读投影返回 `Result`，现有 effective-energy 和 leaderboard 调用方传播错误，不返回部分能量。
- `prev` 先结算/转换到继承高度，再选该高度的折损；仍逐项 floor 后饱和求和。单证的结算和折损检查先于 `Consumed` 写入，整笔继承继续由既有区块 savepoint/pending-energy 流程保护。
- 不新增持久化字段，不改变 pass 权益状态、存储编码、公开 registry、genesis 或 identity 算法；schema v1 / 状态机 v2 / 能量公式 v1 的现网数值语义保持不变。

### 9.2 三阶段测试契约与验收（2026-10-05）

[miner_pass_upgrade_energy.rs](../../tests/miner_pass_upgrade_energy.rs) 定义独立的逐块参考模型，不调用生产公式、版本路由或区间结算函数生成预期结果。测试契约仅用于验证框架：

| 区间 | 每 unit 每块增长 | 边界转换 | 当前继承保留比例 |
| --- | --- | --- | --- |
| `[0,10)` | 1 | 无 | 95% |
| `[10,20)` | 2 | H=10 增长前，对已有 raw energy 饱和乘 2 | 90% |
| `[20,+∞)` | 3 | H=20 显式保持表示不变 | 75% |

测试边界转换只改变数值表示，不改变状态或年龄。Dormant 转换表示但不增长，终态零值保持零；余额事件在 H 块增长之后执行，测试 penalty 使用事件所在区间的增长率与完整保留年龄。此顺序是本测试契约，不能代替未来具体升级 UIP 对转换顺序和量纲的规定。

新增 10 项测试覆盖：

- H-1/H/H+1 手工向量及各区间出生的 checkpoint；三阶段逐块参考、稀疏投影与密集 checkpoint 一致；其它版本族的边界不能重复应用能量转换。
- Active/Dormant/Consumed/Burned/Invalid、零余额与不足一 unit、整数舍入、`u128` 饱和和纯内核 `u32::MAX` 高度。
- 升级当块增减仓、同高度多次操作、清零/再入金和 penalty 年龄；真实 RocksDB 的懒结算与余额事件写入一致，重复应用幂等，只读查询不生成 checkpoint。
- 缺失第一/第二转换边、跨过未知中间区间后又回到已知版本、错误 scope/network 均拒绝；后续转换失败不留下前半段余额记录。
- 不同区间的旧 prev 先转换、再按当前规则逐证折损；同址/跨址继承保持一致，多证求和饱和且消费后为零。
- 真实 SQLite/RocksDB、Core/BH RPC、tracker 的完整区块路径：长期无 pass 操作跨边界，H2 继承旧 Dormant/Active；发布失败回滚后重试、重启，能量与参考值一致。

原有能量/生命周期测试改用现行 schema v1 / 状态机 v2 共享配置，保留既有数值断言。普通二进制集成测试增加两种测试能量版本的拒绝检查，确认它们不能打开业务存储。

本地验证：indexer 单元/管线测试 367 通过、10 忽略，普通二进制集成测试 1 通过；`usdb-util` 87 通过、2 忽略。相关 crate 严格 Clippy、workspace 编译、格式与公共 API 文档构建通过。

### 9.3 下一批与保留边界

C 交付时计划由 D 审查边界动作、规范状态表示/承诺及完整故障、重组、快照恢复矩阵，结果见第 10 节。本批测试转换是从已承诺历史可确定派生的 raw-energy 数值转换，没有实现批量取消存量权益；单个发布失败重试测试不代替 D 的完整恢复验收。

现有 raw-energy 查询已共用内核，但完整历史 registry 前缀约束、缓存/cursor、effective/level 独立升级和 Go 联调仍属于 E。生产没有启用新的公式，也没有运行真实服务升级或远端 nightly/weekly；F、G 的验收和原地数据集升级限制保持不变。


## 10. D 批次交付与当前验证状态

### 10.1 边界执行与状态表示

- [rule_transition.rs](../../src/btc/usdb-indexer/src/index/rule_transition.rs) 为每条已注册 schema/state/energy 转换边声明策略。空块也在 pending-energy 与业务写入之前检查相邻边界；启动检查 origin 至持久化高度的整个路径。不能因为版本各自受支持，就默认两者之间存在兼容转换。
- 现有 schema/state 转换只改变新操作的准入，保留已接受的铸造事实和存量权益。能量边界检查与分段结算共用 `EnergyTransition` 声明；测试 H1 使用非恒等表示转换，H2 使用显式恒等转换。
- 本批注册的转换均可从稀疏 checkpoint、绑定的 registry 及历史余额唯一派生。保持已有能量编码：记录高度是该块完成后的规范状态，不新增每证版本字段、边界标记或全量 checkpoint；只读投影不写入记录，也不生成 mutation。
- 承诺审查结论：现有 pass commit 承诺有序 mutation 与 BTC/BH 锚点，local/system state identity 另外绑定 registry、active set、pass commit 和余额快照。纯派生能量转换不需要新增 mutation 或修改这些编码；恢复验收同时比较能量值，不能只凭 commit 相同推断能量正确。未来若引入物化权益处置或不可派生字段，仍须由具体 UIP 明确动作、顺序及 storage/commit/state-view 版本，不能沿用本批的纯转换策略。

### 10.2 启动与失败恢复修正

- 初始化时先依据已提交 SQLite 高度修复能量尾部，再初始化 tracker，并完成持久化的 reorg recovery。尚未提交首块的数据集统一使用 `origin - 1`（下界为 0）作为基线，避免首块失败后错误回退到高度 0、随后误报能量库落后。
- 能量恢复先检查高度是否落后、已提交历史的高度标记是否缺失、pending 是否覆盖已提交 pass 历史，再进行尾部删除。上述不一致快照被拒绝，不会先删除已提交能量记录。
- 保留精确 registry 的双库绑定，不允许通过改配置采用另一 revision。这里的高度检查不是任意损坏检测器；恢复仍要求来自同一次完整、验证过的成对快照。
- 更新手工构造旧快照的测试夹具，显式设置 origin 和配对的能量进度；未放宽生产检查来适配不完整夹具。

### 10.3 恢复矩阵与验证（2026-10-05）

[miner_pass_upgrade_recovery.rs](../../tests/miner_pass_upgrade_recovery.rs) 增加 10 项父测试，以及仅供父进程精确调用的 ignored 子进程夹具。所有路径使用真实 SQLite/RocksDB、生产块执行与 upstream reorg 恢复流程，以及隔离的 Core/BH RPC 夹具。

| 场景 | 覆盖与断言 |
| --- | --- |
| 无 pass 操作的 H1/H2 空块 | 独立逐块参考值、无额外能量 checkpoint、空 mutation root；重复重启不重复转换。 |
| 缺少转换路径 | 即使无 pass，H 块和已有历史的启动检查仍拒绝；不留下该块进度、pending 或 commit。 |
| 首块、H1、H2 发布窗口 | pending、事件执行、能量写入、能量 finalize、pass commit 写入、tracker 发布、SQLite 高度写入及提交后共 8 个阶段；每阶段分别返回错误和子进程直接退出，共 48 个组合。 |
| 真实 SQLite 写入失败 | H2 多 prev 继承完成后，用数据库 trigger 拒绝高度写入；两库、prev、审计、余额与 commit 回滚，移除故障后重试。 |
| 跨两次升级的 upstream reorg | 从 H2 后回到 H1 前，替换含协作 mint 和跨地址继承的分支；后续真实 UTXO 转移验证 tracker 已清除旧分支状态。 |
| reorg 中断恢复 | SQLite rollback、能量回滚、tracker reload 三阶段分别错误返回与进程退出，共 6 个组合；持久标记驱动重启完成恢复，重复启动幂等。 |
| 各区间 checkpoint 恢复 | H1-1、H1、H2-1、H2、H2+2 停止写入并复制成对数据库，恢复后跨区间追赶，与从 origin 重放一致。 |
| 不一致能量元数据 | 高度落后、pending 覆盖已提交高度、缺少高度标记三种情况均拒绝，保留已有非零能量记录。 |

恢复比较逐高度的 raw energy、精确稀疏记录、pass/owner/satpoint、prev 消费、mint audit、状态历史、余额快照、BTC/BH anchors、pass commit 和 local/system state identity。SQLite 本地历史行号与 reorg 运维计数不作为分支等价条件；reorg 次数另行断言。独立逐块参考函数移至 [tests/common/miner_pass_energy_reference.rs](../../tests/common/miner_pass_energy_reference.rs)，继续用于 C 和 D。

本地验证：indexer 单元/管线测试 377 通过、11 忽略（其中新增 1 项只由父测试调用的退出夹具），普通二进制集成测试 1 通过；`usdb-util` 87 通过、2 忽略。相关 crate 的严格 Clippy、workspace 编译、格式、公共 API 文档构建、发布片段校验及共享技能同步检查通过。现有 fast gate 的 Rust 测试会自动收集本批父测试；未触发远端 CI。

### 10.4 保留边界

生产仍只支持 schema v1 / 状态机 v2 / 能量公式 v1，公开 registry、genesis、数据编码和 identity 算法不变，不要求重建或网络重置。新的故障注入器与额外公式均仅在测试构建存在，没有增加普通程序的故障环境变量开关。

子进程退出跳过 Rust 析构，覆盖实际数据库的进程崩溃恢复，不宣称验证了断电或硬件写入持久性。checkpoint 用例是停止写入后的本地成对复制，不代替生产签名快照工具、跨服务 checkpoint 或在线节点升级验收。

下一批 E 处理完整经济查询、历史 registry 前缀一致性、缓存/cursor 和 Go 联调；F 接入真实 Core/Ord/BH/indexer/Geth 与 nightly/weekly；G 处理数据集 registry 原地升级。本批没有部署节点，也没有触发远端 CI。
