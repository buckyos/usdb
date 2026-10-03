# UIP 测试覆盖复核：MinerPass V2

2026-10-03，基于 USDB `0452f5c`、go-ethereum `c69e034fe` 加本批未提交改动。复核对象是 UIP-0000–0016 与两仓库的实际测试、夹具、CI 入口及断言。本批不改生产共识规则、embedded registry、公开网络 genesis 或在线数据。

V2 指 MinerPass schema/state-machine；energy、selector、commit 等独立版本族仍可为 v1。旧规则的拒绝向量、冻结 registry 身份和历史文档不能机械替换为 v2。Draft 中尚未冻结的未来政策也不能因为有测试构建就算已实现生产规则。

## 发现并修复

| 问题 | 修复及验证方式 |
| --- | --- |
| Go 能量增长、历史稳定性/同高度替换、upstream fault 使用 V1 mint；前两者 genesis 仍指向旧 registry | mint 改 V2；同地址铸造明确选取该 owner 的 cardinal sat；genesis 选择共享 V2 registry；以真实 Core/Ord/indexer/Geth 运行原有经济和历史断言 |
| upstream fault 的 fresh validator 只复制 config，没有复制 external catalog | 复制同一 catalog 字节，保持 pin 不变；从空目录启动 B/C 并检查状态一致性 |
| AssumeUTXO P6.5 夹具使用 V1、默认 registry，以及 OP_1 编码的 envelope 标签 | 使用 V2 及显式 catalog，将标签改为字节推送；保留非首输入、非零 sat offset；补充 mint version、首次开户余额/历史及双服务 audit 相等断言 |
| M11 只有余额计数器单测，缺少完整索引执行断言 | 新增 `prior_balance_spent_before_reveal_allows_opening_without_source_authorization`：跨来源 mint，在旧余额未花完时 Invalid，同块先花完时 first_opening Active；已有 M10 验证相反的同块入金边界 |
| Go profile verifier 缺少重算 ID 后的 scope/旧规则篡改组合 | 在 `TestVerifierResolveProfileRejectsSelectorIdentityMismatch` 增加缺失/外网 scope、旧 schema/状态机四例；攻击者自洽重算 ID 仍不能绕过 chain-config registry 校验 |
| bootstrap mock 仍宣告旧规则；单节点 fake-PoW 在初始化交易前到达 Dividend fee gate | mock 与其隔离 genesis 使用 V2；双节点重生成的两个 genesis 均先选择 V2 再比较；单节点采用既有双节点的 1s 出块节奏，不放宽 fee gate 或 anchor 规则 |
| 新辅助测试、部分真实服务测试不在持续入口 | fast 接入 BTC 两个 MinerPass 工具测试及 Go profile V2 oracle；nightly go-profile 增加增长和历史稳定性，indexer-protocol 增加 P6.5；通过 `--work-dir` 将 snapshot 证据纳入 diagnostics |
| UIP 把历史测试状态描述为当前实现 | UIP-0001 明确当前测试只接受 V2；UIP-0005 更新链侧已有校验；UIP-0006 将旧规模/soak 报告与 V2 资格分开 |

## 按 UIP 对照

路径以 USDB 为默认；Go 路径属于相邻 go-ethereum 仓库。下表是需求到断言的映射，不是仅按名称统计测试数量。

| UIP | 核心要求及现有测试 | 本轮结论 / 边界 |
| --- | --- | --- |
| 0000 | 规则分层、版本与证据边界；由下列逐项测试及 activation 约束体现 | 文档治理要求不等同可执行测试；公开网络冻结另行验收 |
| 0001 | `index/content.rs` 严格字段、三种互斥 mint、canonical ID、重复 key/prev；`miner_pass_activation.rs` 各高度拒绝 V1 | 已全量回归；真实 V1 夹具迁移；历史正文保留 |
| 0002 | `miner_pass_eligibility.rs` 原子 prev、Active/Dormant/Consumed/Burned、实际来源与 owner 历史；activation 全区块、失败回滚和 reopen | 已回归；现行约束以 0016 为准，外部 transfer 不自动激活 |
| 0003 | `index/energy_formula.rs` 整数单位、年龄/损失、逐 prev 折损、uint128 饱和；energy timeline 和 storage 往返 | 已回归；真实增长脚本同时重算 USDB 奖励和累计状态 |
| 0004 | `index/effective_energy.rs`、`service/server.rs` 两种 Leader 引用、衍生贡献；eligibility 跨地址轮换/协作原子性；安全矩阵 | 单测已回归；上一批 V2 live 证据另见矩阵记录；不把旧 100K 容量报告当本轮新结果 |
| 0005 | Go `profile_formula_test.go` 全阈值与 ceiling；`verifier_test.go` level/factor 派生值；`consensus/ethash/consensus_test.go` miner/verifier 一致 | 已回归；生产 formula v1 与仅测试 activation 版本分别运行 |
| 0006 | `service/server.rs`/`economic_cursor.rs` 固定历史 context、候选排序、cursor 绑定/篡改/retention；Go verifier/builder；真实 head advance、same-height replacement | 已回归并恢复持续入口；旧容量/长期结果须按版本重新确认 |
| 0007 | Go payload/anchor/verifier/builder 测试：111-byte selector、历史身份固定、age/overflow、服务错误；upstream fault 真正验块停顿/恢复 | 单测已回归；服务矩阵结果见下表 |
| 0008 | Rust activation/snapshot/checkpoint/storage 绑定与 Go registry golden、scope、payload 高度分派；`core/forkid` 嵌套 checkpoints | 已回归，新增自洽身份篡改拒绝；catalog 不复制的 fixture 缺口已修复 |
| 0009 | Go params/forkid/consensus：链配置、USDB Extra、最低难度、升级边界及 reorg | 已回归；网络发布配置/公开节点接入不是本轮资格 |
| 0010 | Go core bootstrap integration；隔离 SourceDAO bootstrap smoke、restart/joiner runner | mock 已迁移；生产 artifact/签名发布验收仍单列，不把 fake-PoW 称为真实工作量证明 |
| 0011 | Go `core/usdb_fee_test.go` 退款后费用、逐 tx 舍入、gate 原子失败；`usdb_block_import_test.go`/`usdb_economics_test.go` reward stateRoot、replay/restart；ethash beneficiary 校验 | 已回归；实际服务 profile oracle 重算发行、奖励和状态槽 |
| 0012 | `TestPrepareKTransitionMatchesIndependent50405StepOracle` 独立整窗口 oracle；满窗口替换、损坏状态拒绝、parent state 回滚 | 已回归；当前样本/历史窗口不从 current head 补算 |
| 0013 | fixed-price range golden、parent state mismatch、activation current-block transition、状态槽与 replay | 已回归；动态价格不属于当前实现承诺 |
| 0014 | disabled nominal 路径、默认构建拒绝 formal/未知 quote；economic conformance v2/v3 的 current-block difficulty/reward 一致 | 三组测试 tag 单独回归；不代表 formal v1 已冻结或生产可用 |
| 0015 | disabled 分配、默认构建拒绝未实现政策；测试 tag 的有效/无效 split、金额守恒及原子失败 | 已回归；真实辅助算力 proof/submission 仍属后续协议，不用假策略冒充覆盖 |
| 0016 | evidence/eligibility/activation 三层及真实安全矩阵：签名与 sat 来源、首次/同址/跨址、协作、reorg/retry/audit | 补 M11 完整流水线，保留 M17 普通付款可改受益人的预期边界；P6.5 增加 snapshot floor 下真实 V2 开户 |

## 本轮验证记录

环境：Core 28.1、Ord 0.29.0、Go 1.26 compatibility linker、Rust workspace；均使用独立临时目录。真实服务使用既有生产二进制，本批只变测试与文档。`/tmp` 路径是本机证据，不是可下载的 CI artifact。

| 验证 | 结果 | 本机证据 |
| --- | --- | --- |
| Rust workspace | 773 passed、13 ignored；ignored 不计为通过；空的 bin/doc target 不计为用例 | `/tmp/usdb-uip-review-rust-workspace.log` |
| MinerPass 完整流水线 | 11 passed，包括 M11 对照 | `/tmp/usdb-uip-review-pipeline.log` |
| Go 默认回归 | internal/usdb、forkid、usdbstate、params 全包；ethash/miner/core 的 USDB/升级/经济相关筛选通过 | `/tmp/usdb-uip-review-go.log`、`/tmp/usdb-uip-review-consensus.log` |
| Go 测试构建 | activation conformance、economic v2/v3 的 internal/usdb 和 ethash 通过 | `/tmp/usdb-uip-review-activation.log` |
| Python/oracle | BTC 工具 8、Go V2 profile 3、mock 4、long CI 16、upstream oracle 24 通过 | `/tmp/usdb-uip-review-*-python.log`、`/tmp/usdb-uip-review-longci.log`、`/tmp/usdb-uip-review-upstream-oracle.log` |
| V2 能量增长 | 通过；阶段能量 0→2000，16 个 USDB blocks 的余额/发行/窗口状态重算一致 | `/tmp/usdb-uip-review-growth.log` |
| V2 历史验块 | 通过；BTC head 前进后独立节点导入旧上下文；同高度替换拒绝旧 selector | `/tmp/usdb-uip-review-history.log`、`/tmp/usdb-uip-review-replacement.log` |
| P6.5 双服务 | 通过；缺失 block/undo、崩溃重启、查询 floor、原链 48 项/重组 55 项比较，以及双侧 mint audit 一致 | `/tmp/usdb-uip-review-assumeutxo-final/result.json` |
| bootstrap smoke | 通过；V2 mock 下初始化、DAO 绑定与真实交易存款断言 | `/tmp/usdb-uip-review-bootstrap-final.log` |
| upstream fault | 18 场景全部通过，518.65s；包含两次恢复中断、旧 validator 不重启自行恢复、fresh C 历史导入及规范状态一致 | `/tmp/usdb-uip-review-upstream/run-JMjZjL/output/summary.json` |
| bootstrap restart/joiner | 通过；checkpoint 篡改/管理员污染拒绝、256 块前后费用分流、重启及 fresh follower、历史 stateRoot 一致 | `/tmp/usdb-uip-review-joiner.log` |
| 静态检查 | Rust fmt、workspace all-target clippy、修改的 shellcheck、diff check 通过 | `/tmp/usdb-uip-review-clippy.log` |

早期发现的 P6.5 非规范 envelope、bootstrap 在初始化前到达 fee gate均先失败再修正，最终运行使用独立目录；一次错误的 Rust 模块筛选命中 0 个测试，不计入结果，随后按实际模块名运行 11 项。本表只记录明确完成的成功验收。

## 持续入口与剩余资格

- fast：Rust workspace + 两个 V2 Python 工具；Go 历史/经济/registry 回归 + V2 profile oracle。
- nightly：go-profile 覆盖 profile、增长、历史、同高度替换、失败矩阵和 anchor；indexer-protocol 加入 P6.5，保留上一批攻击/协作矩阵。
- weekly：upstream-fault-matrix 使用 V2；world-soak 仍按 2500 轮 × seeds 41/42/43 原门槛，未降低为短跑。

本轮仍不等于全部 nightly/weekly 或新版测试网发布资格。完整 V2 多 seed 长跑、当前版本的容量资格、签名 checkpoint 服务、生产 Core loadtxoutset 双 chainstate/主网规模验收及完整发布流水线仍须分别完成。P6.5 使用已知 Core 前缀提交与 snapshot 文件导入，不是生产 Core loadtxoutset 性能结论。Go 本地用现代兼容工具链，发布固定 Go 工具链和 CI qualification 仍需另验。

完成本批评审后可提交两仓库并更新兼容锁，再执行正式长期资格。control-plane 和网络发布准备继续后移；没有发现必须在这批改变生产共识规则的问题。
