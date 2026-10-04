# MinerPass 操作资格改造计划

更新时间：2026-10-04。

本计划落实 [issue #51 收敛方案](https://github.com/buckyos/usdb/issues/51#issuecomment-5894524579)，协议草案为 [UIP-0016](../UIP/UIP-0016-miner-pass-operation-eligibility.md)。用户已同意按该方案推进；草案、代码合并、委员会状态和网络激活是不同事项。

本文件区分当前事实与后续实现任务。规范草案已提交为 usdb `c0f362b`，Go 兼容锁同步提交为 `beb9a3b2e`；来源证据与交易前余额层已提交为 usdb `cacdcd3`，对应 Go 兼容锁为 `ba631d72b`。一次性开户资格、状态机及诊断改进已提交为 usdb `e8303f9`，Go 兼容锁为 `ab52b498a`。阶段 4 的完整区块执行/恢复、审计接口及仅执行 MinerPass v2 的收敛已提交为 usdb `dcd41f6`，配套 Go 提交为 `889d74197`。最小真实服务闭环已提交为 usdb `e8c73d9`、Go `aea0a7af9`。nightly/weekly 矩阵迁移已提交为 `0452f5c`，UIP 覆盖复核为 `bda0d58`，control-plane 引导与核验为 `a28cf4c`。testnet-v1 网络包已提交为 `c025ce5`，配套 Go 为 `09a9402d4`。旧 embedded catalog/ID 保持冻结，但不再是本程序支持的执行规则。下列任务不能仅因有文档或测试名称就标记完成。

## 当前交付状态

- 协议与执行：仅执行 MinerPass V2，三路径资格、来源取证、完整区块恢复及审计已接通。
- 版本分层修正：当前 JSON schema 为 v1，操作状态机为 v2；r4 后修正版已完成本地实现与隔离验收；正式发布与远程 CI 资格另行确认，见[验收记录](miner-pass-schema-rule-separation.md)。
- 测试：隔离真实服务、矩阵迁移及 UIP 覆盖审查已有分批记录；完整 weekly 和当前 tag 的 CI 结论以对应运行证据为准。
- 用户工具：control-plane 草案与入金前核验已实现；正式网络签名和广播继续由外部钱包完成，见[冷钱包手册](../handbook/miner-pass/cold-wallet.md)。
- 网络包：testnet-v1 的身份、registry、genesis 和发布配置已冻结，见[网络包说明](../publish/usdb-testnet-v1-network-bundle.md)。线上重置、实际安装与多节点验收不是本文件已经完成的事项。

下列“该批／本批”描述保留各阶段当时的实现与验证边界，不表示后续工作仍未开始。

## 前批已提交：来源证据与交易前余额

- `usdb-util::BTCRpcClient::get_block_prevouts` 返回完整 `BlockPrevouts`：精确块/交易字节、金额、脚本、创建高度、coinbase 标志。同块 prevout 与真实创建输出交叉检查；旧金额 API 保持原要求。
- `btc/mint_evidence.rs` 定位受支持 Ord envelope 的实际 sat 和接收 owner；按 spent prevout 创建高度查询历史 commit，覆盖早于 index origin/base 的位置。历史缓存最多保留 4 个块，按规范链锚校验后复用；不使用 txindex、当前 gettxout 或浏览器数据。
- `usdb-util::prove_commit_source` 使用左闭右开输入区间反查来源，并实际验证允许的 ECDSA/Schnorr 签名。确定不支持的脚本/sighash、coinbase 来源与数据不可用分开返回。
- `btc/transaction_balance.rs` 按所有 BTC 交易建立精确 sat 余额。RPC 路径固定读取同锚 H 的区块后余额，反推块前余额，支持 query floor 为 H；读取前后复核 snapshot identity 与 Core hash。空/不完整响应不能当作零。
- 该提交只提供独立证据层，不改变生产规则。下述状态机批次继续使用这些上下文；JSON v2 parser/activation dispatch、RPC 对外审计字段和钱包流程仍待后续完成。

验收入口：

```bash
cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer -p usdb-util
python3 tests/run_miner_pass_evidence_live.py --bitcoind /path/to/bitcoind
```

独立证据测试覆盖 11 组场景：来源签名/annex/篡改、sat 区间及零值输入、同块和历史 commit、缺失 undo 恢复与 reorg、严格 prevout、pointer/歧义/unbound/burn、coinbase、交易前余额以及 snapshot floor RPC 的错误响应。真实 Core 28.1 在全新 `txindex=0`、不裁剪 regtest 中接受 8 类真实交易：P2PKH ALL、P2WPKH ALL、P2TR DEFAULT、P2TR ALL+annex，以及不支持的 ANYONECANPAY/NONE/SINGLE/script-path；正确定位第 3 个来源输入，包含真实零值前置输入、同块/历史 commit、重开一致性及旧分支拒绝。runner 保存原始 commit/reveal 交易和结果到独立临时目录。

上述真实 Core 测试是前批证据层的验收记录，本批状态机验收不重复声明为新的真实节点测试。query floor 边界已通过 RPC 夹具验收；**完整 AssumeUTXO 装载、后台验证及历史 undo 可用性的服务级验收仍属于阶段 6**。独立上下文重开不等于数据库/快照恢复验收。


## 前批已提交：一次性资格与状态机核心

- `storage/pass.rs::has_ever_valid_owner` 从既有规范持有历史派生资格，包含全部有效 mint 和成功 owner transfer。读取 writer 视图，能看到同块已执行事件；Invalid、终态 pass 的无效转移不占用，消费/烧毁/转出不清除旧 owner 的历史。没有新增资格表、缓存或数据库编码。
- `index/pass/eligibility.rs::on_mint_pass_v2` 统一首次开户、同地址操作和单来源跨地址继承。仅零余额、无有效持有历史、空 prev 可豁免来源授权；带 prev 始终按真实来源 owner 验证。入口接收已经过 schema 校验、由真实铭文解析得到的字段，尚不承担生产 JSON 分派。
- 所有 prev 和 Leader 绑定先校验，再执行休眠和消费；跨地址只处理列出的源 pass，同地址保留旧 Active 休眠语义。消费前再次核对同一来源 owner，能量结算沿用已有整块余额和逐 prev 继承损耗公式。
- 新入口要求 SQLite writer 事务、同高度 energy pending 与 mutation collector，以及同块链证据/余额上下文。协议 Invalid 只记录新证失败；运行时错误由调用方中止并恢复整块，不承诺单次 mint 自动跨库回滚。v1 仅抽出共用的创建/消费步骤，执行顺序与 mutation 编码保持不变。
- 运行时错误与对应日志统一带铭文 ID、高度、交易 ID、owner 和 satpoint；不一致分支同时报告预期与实际高度/区块哈希/owner/satpoint，未初始化状态明确显示 `None`。诊断补充不改变参与 mutation root 的 `InvalidMint.error_reason`。

验收新增于 `tests/miner_pass_eligibility.rs`，共 20 个测试，使用 `tests/common/miner_pass_state.rs` 的隔离 SQLite/RocksDB 与签名链证据夹具：

- 首次开户的 0/1 sat 边界、历史占用、空余额不重置资格、第三方不能替换既有 Active 或消费其 prev、同地址修改收益地址及跨地址多 prev 继承。
- 同 reveal 多 mint 共用交易前余额但按顺序观察资格；先转入有效 pass 再出现无效 mint 时保留真实 transfer；重复竞争只能消费一次。
- 缺少来源证据返回运行错误、恢复后重试；在第二个 prev 消费和新 pass 能量写入处注入失败，验证块恢复撤销部分消费。
- 能量已 finalize 而 SQLite 未提交时重开恢复；关闭后复制双库、重开、回滚和重放得到相同 mutation/root 与能量，旧 owner 历史仍保留。
- standard/collab 共用资格规则，Leader 轮换后固定 ID 与地址引用均不自动跟随；同块先清空源余额可能在继承前触发能量损耗，不能承诺迁移总共只损失 5%。

该批回归：`usdb-indexer` 368 passed / 10 ignored，`usdb-util` 75 passed / 2 ignored；workspace check、格式检查、indexer Clippy 与文档构建通过。测试只使用临时目录，无在线节点操作。

该批验收边界：上述事件顺序测试直接调用状态机；双库复制恢复不是签名 checkpoint 导出/安装验收。生产联合验收和来源审计接口的本批进展见下；完整服务级重放仍属阶段 6，不能将 M01–M18 全部标为已完成。历史资格依赖自本作用域 index origin 起的完整规范历史；后续激活/导入校验不能把只含当前 Active 或缺历史的旧数据集视为合格。

## 前批已提交：单规则执行、整块恢复与审计

- 移除 `MinerPassRules::V1/V2` 分支、旧 mint 状态机及 `calc_create_satpoint` 旧推导入口；早期实现将 schema/state 同时升级；r4 后分层修正为 parser 和 control-plane payload 只生成/接受 `v:1`，Rust/Go 只执行 schema v1 + state machine v2。
- 保留 source/scope/revision、高度查询、active set/state identity、checkpoint 和 P2P 升级框架。旧 catalog 可解码并核对冻结身份，但旧/混合/未知规则不能执行；未来不支持的 checkpoint 在对应块业务写入前停止。
- `tests/fixtures/miner-pass-v2/catalog.json` 从 H=0 使用新规则。`catalog-staged.json` 额外加入不激活公式的 Planned 修订，Go 显式开发 catalog 使用相同黄金向量。未修改旧默认 catalog、默认 chain config 或发布包；使用者必须显式配置新作用域及精确 ID，使用新数据集。
- 生产采集复核完整规范候选集合，拒绝外部源遗漏/重复/内容、分类或编号不一致。整块共享来源证据和覆盖全部 BTC 交易的余额；同交易 transfer 在 mint 前，同 reveal 按 envelope index 执行。
- 区块异常回滚 SQLite、energy 和 tracker。外层发布失败时先退出 writer savepoint，再按 durable 高度恢复 energy/重载 tracker；恢复失败门禁同时约束下一块及无新块的同步循环。
- 新增同 savepoint 发布/回滚的辅助审计和 `get_pass_mint_audit`（含 Rust client）；读取核对已提交区块及 reveal 链锚，缺失/损坏审计报错，临时证据缺失不写永久 Invalid。
- mutation、经济公式、query/state-view/commit/BH 编码不变；来源与资格结果由既有 mutation 和绑定 v2 的 active set 进入状态身份，审计本身不作为资格输入或新增 mutation root。

测试迁移删除了允许直接注入未授权 mint 的旧 mock 场景，不保留仅供测试的旧执行器。对应覆盖由以下用例承担：

| 旧场景职责 | 当前覆盖入口 |
| --- | --- |
| 首次开户、替换、单/多 prev、重复消费、Leader 与协作 | `tests/miner_pass_eligibility.rs`：真实签名来源 + SQLite/RocksDB |
| mint/transfer 顺序、Invalid、schema、余额 1 sat 干扰 | `tests/miner_pass_activation.rs`：生产构造器、原生源、完整区块/真实 tracker |
| settle/finalize/外层发布失败、重试、重开、回滚与完整重放 | 上述两个文件及 `tests/miner_pass_block_recovery.rs` |
| 能量数值、投影、无 mint 区块结算、上游 reorg | 保留 `energy.rs`、`energy_timeline.rs`、`indexer_behavior.rs` 原专项测试 |
| txindex=0 的花费输入及 sat 转移 | `tests/assumeutxo_indexer_inputs.rs`，不再把普通 sat 映射当作 mint 授权 |
| 旧规则拒绝、未来高度停止、作用域/版本身份、跨语言互验 | `tests/miner_pass_rule_versions.rs`、Rust startup/checkpoint 测试与 Go registry/profile/import 测试 |

本批回归：indexer 343 passed / 10 ignored、util 76 passed / 2 ignored、checkpoint tool 17 passed、control-plane 38 passed；Go verifier、ethash、core、miner、geth 命令测试通过。

这些测试使用隔离夹具和临时数据库；未连接或改动在线节点。完整真实 Core + AssumeUTXO + v2 服务管线仍在阶段 6；control-plane 本批仅改 JSON 版本，source/recipient 引导及最终核验仍在阶段 5。

复现：

```bash
cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer -p usdb-util -p usdb-indexer-checkpoint-tool -p usdb-control-plane
cargo run --manifest-path src/btc/Cargo.toml -p usdb-util --bin generate_go_btc_activation_golden -- --catalog tests/fixtures/miner-pass-v2/catalog-staged.json /home/bucky/work/go-ethereum/internal/usdb/testdata/miner_pass_v2_activation_golden.json --check
# 在配套 go-ethereum 仓库执行；新 Go 工具链按仓库兼容策略使用 -ldflags=-checklinkname=0：
go test ./internal/usdb ./consensus/ethash ./core ./miner
go test -ldflags=-checklinkname=0 ./cmd/geth
```

## 已提交：V2 共用测试基线与最小真实服务闭环

- `regtest_reorg_lib.sh` 为临时 indexer 显式选择隔离 V2 catalog；Go profile runner 的临时 genesis 使用同一 registry。未修改生产默认、测试网发布参数或在线数据。
- 新公共工具按实际地址选取 confirmed cardinal UTXO，排除所有带铭文的输出，并传给 Ord `--satpoint`。测试保留选币证据、Ord commit/reveal 结果和 `get_pass_mint_audit` 响应。
- 真实 Core 28.1、Ord 0.29.0、balance-history、indexer 验证首次开户、同地址 prev 重铸、跨地址 prev 继承；第三方向既有权益地址伪造 prev 的铭文为 Invalid，原 pass 仍 Active。继承前后旧 pass UTXO 保持原位置。
- 新 pass 入金后具有正能量；真实 Geth 出块、独立 profile/难度/奖励计算及第二个 Geth 节点相同高度/hash 检查通过。
- 既有在线矿工分支也迁移至 V2；外部转移后 remint、运行中 selector 切换及 indexer 正常停启后挖矿恢复已单独回归通过。
- nightly 的 `go-profile` 首个场景启用此闭环；其余旧 V1 交易场景、revision 切换、reorg/world-soak/weekly 用例仍需逐项迁移和整组运行，不能报告完整 nightly/weekly 已通过。

复现入口、原始产物与本次边界见 [最小真实服务验收](miner-pass-v2-live-acceptance.md)。本次使用 P2TR 钱包、`txindex=1` 和未裁剪完整 regtest；不代表已完成 hash 地址冷钱包、全部签名形式、完整余额清扫、AssumeUTXO 或签名 checkpoint 验收。

当时确定的执行顺序为：最小闭环 → 扩展 nightly/weekly 攻击、恢复与长期测试 → control-plane 钱包引导 → 网络参数冻结及真实部署。下面阶段 5/6 的编号沿用原计划，不代表仍要求先做控制面。

## 已提交：nightly/weekly 矩阵迁移

- 新增真实 Ord 攻击/协作矩阵：非参与持币地址、强制替换/消费 prev、归零旧 owner、Dormant 跨地址继承、两种协作引用在 Leader 轮换后的断开与重新绑定。
- 在同一链上验证上述状态的重组撤销、进程崩溃重开和空数据库完整重放，逐项比较 pass、mint audit 及系统身份。
- 迁移 protocol、reorg、historical、validator 与 world-sim 的 V2 JSON/catalog/source；测试必须指定实际来源 sat，不能以钱包名代替来源地址。
- 移除回归入口中已不存在的旧精确测试名，并在执行前验证测试存在；retention 测试不再修改绑定的 index origin 来冒充裁剪。
- world-sim 保留长期覆盖门槛；本地短程演练与完整 weekly 长跑分别记账。完整重放复制同一 external catalog，不复制派生数据库。

本批的运行命令、结果和剩余边界见 [V2 矩阵验收](miner-pass-v2-matrix-acceptance.md)。公开网络参数、生产默认、control-plane 引导与线上服务均未改动。

## 已完成的前置条件

| 前置 | 本地提交 | 提供的能力 |
| --- | --- | --- |
| 规则作用域与 registry 隔离 | usdb `f276ab4` / go-ethereum `7e18a18e7` | 同 BTC 来源可有独立 rules scope；配置、状态身份、双库存储和 checkpoint 校验 |
| P2P fork ID 纳入 USDB checkpoint | usdb `f9cc98f` / go-ethereum `f4399c509` | 新高度进入握手/ENR 共用 fork 列表；不保证既有 peer 自动断开或相同高度规则内容可识别 |

旧 v0 元数据仅作历史记录；新程序不重放旧规则。改造在独立数据目录和显式 v2 catalog 中验收。v1 网络包已在发布准备阶段冻结；测试网重置和真实部署仍由发布验收独立记录。

## 方案启动时的实现差距（后续批次进展见上）

| 范围 | 启动时源码入口 | 当时差距与实现方向 |
| --- | --- | --- |
| schema 分类 | `src/btc/usdb-indexer/src/index/content.rs` | 当前只接受 v1，尚未按目标 BTC 高度选择 v1/v2 parser；禁止给用户一个可选择旧执行器的 v 字段 |
| sat 位置 | `index/transfer.rs::calc_create_satpoint` | 能定位 reveal 输入和 commit outpoint，但仍写死输入 offset 0；须显式限定 Ord envelope 支持范围并验证真实 sat |
| 区块输入证据 | `usdb-util/src/btc/prevout.rs`、`btc/rpc.rs`、indexer `btc/utxo.rs` | verbosity-3 已校验规范块、交易字节/顺序和 prevout 金额，但丢弃脚本/创建高度等信息，未反查 commit 输入来源 |
| 区块内资格余额 | `index/indexer.rs`、`index/indexer/block_events.rs` | 事件执行只遍历 mint/transfer；整块余额结算不能直接用作交易前资格余额，需覆盖所有 BTC 交易 |
| prev 状态机 | `index/pass.rs::validate_mint_state`、`on_mint_pass` | 验证和消费复核均比较 mint_owner；新模式必须传入经过验证的 source owner，不能只改一处比较 |
| 有效持有历史 | `storage/pass.rs` | 已有 mint、owner transfer、状态历史；先验证是否能完整派生 ever_valid_owner，再决定物化索引；不能只看当前 Active |
| 状态承诺与恢复 | `index/pass_commit.rs`、`storage/pass.rs`、`storage/energy.rs`、checkpoint tool | 审查新来源/资格信息是否有新增共识编码；旧 hash、旧高度回放和上游 balance-history 身份须保持可重放 |
| 控制面 | `usdb-control-plane/src/server.rs::prepare_btc_mint_context`、`models.rs` | 当前只有 owner_address，按目标查询 Active/prev；需拆 source/recipient 和最终核验；execute 仍仅 development |
| Go 版本支持 | `internal/usdb/activation.go`、registry/profile 验证 | 当前 BTC profile 只接受 v1 集合；须与实际受支持的新组合和 golden 一起改，不先扩大白名单 |

路径如未带 crate 前缀，默认位于 `src/btc/usdb-indexer/src/`。这些代码位置来自方案启动时的源码核查，不沿用 issue 中旧 revision 的行号；已实现部分以上方分批验收记录为准。

## 实施顺序与退出条件

### 1. 规范与向量基线

当前已建立 UIP-0016 Draft 和 M01–M18 期望表；Ord 子集与来源签名形式已落实到已提交的证据层测试。v2 分派与业务 canonical 编码的本批结论见阶段 4：

- JSON `v=1` 与独立的 state-machine v2 从新网络 origin 启用；schema v1 不得绕过新资格规则，撤回的 schema v2 和旧状态机均拒绝。
- Ord 当前锁定依赖是 0.24.2；固定支持的 envelope 子集与 satpoint 向量，尤其 pointer、unbound、非首输入及歧义输入。不得把 envelope offset 当 sat offset。
- P2PKH/P2WPKH/P2TR key-path 的支持形式、sighash 白名单和 annex 处理。
- 新 mutation/查询字段的 canonical 编码与最低必要版本影响；资格缓存必须可从已承诺历史重建，或另行承诺。

退出条件：每条路径有明确有效/无效/数据不可用分类，所有未支持的链上形式有确定结果，历史规则可完整描述。生产激活高度不属于实现前必须猜测的参数。

### 2. 来源证据与交易前余额（独立层已实现并验收）

先建立无状态证据层，再接业务写入，减少把链数据问题误写成 Invalid 的风险。

- 将区块输入缓存扩展为有金额、锁定脚本、创建高度及规范链锚的 spent prevout 上下文。旧金额 API 可保留适配，缓存只发布完整已验证结果。
- 用 reveal spent prevout 的创建高度定位 commit 所属块；同块 commit 从当前块读取。校验 commit txid/vout、原始交易及前后链锚，再取得 commit 输入证据。
- 若历史 Core block/undo 不可用，则暂停当前块。需要历史 fallback 时，必须具备同等链上认证和确定性，不允许浏览器 From/gettxout/猜测来源。
- 取得目标 E 的 H-1 精确余额并按全块交易顺序计算每个交易前余额；同块普通支付、coinbase 及输入脚本对应的变化不能遗漏。若快照 query floor 为 H，需用同锚 H 的区块后余额减完整全块 delta 精确反推，或报告证据不可用；必须验收 origin/base 首块，不能让部署在该边界永久等待不存在的 H-1 记录。
- 缺数据重试前不缓存部分证据；同高度另一 block hash 不能复用旧缓存。

退出条件：多输入多输出和 sat 边界向量通过；同块/历史 commit、1 sat、零值输入、无效铭文前转账通过；RPC 失败/reorg 后能重新加载；真实 `txindex=0` 的隔离 Core 可完成证明。

### 3. 一次性资格与原子状态机（核心已实现并验收）

- 资格读取覆盖有效 mint、成功 owner transfer 和自 origin 起的历史；Invalid 不占用，消费/烧毁/转出不重置原 owner。
- 按 UIP-0002 的事件顺序更新资格；同一 reveal 多 mint 共用交易前余额，但资格观察顺序不同。
- 统一选择首次开户、同地址、跨地址三条路径；来源签名限制仅对后两条强制。
- 跨地址仅处理 prev 引用的源 Active，未引用的 pass 保持原状态；同地址保留旧 Active 休眠逻辑。
- 所有 prev 与 Leader 预校验先完成，再写入；消费前复核使用同一 source owner。
- 复用整块 SQLite savepoint、energy pending/finalize 和 tracker staging 恢复机制，并补故障注入。无效 mint 不能抹掉同交易真实 transfer。

退出条件：M01–M18 对 standard/collab 的适用场景通过；失败没有部分消费；重复竞争只成功一次；同块/跨块余额迁移能量结果明确；block rollback、重开、快照恢复和完整重放一致。

核心测试已通过；阶段 4 已接入 active-set 分派、真实 ordered block executor 和 tracker 恢复，并完成隔离激活边界测试。签名 checkpoint 与完整服务级重放在阶段 6 收尾。

### 4. 版本、完整区块、状态身份和查询（已提交）

- 按 registry 的 active set 校验支持范围；当前只接受 schema v1 + state-machine v2，旧状态机、撤回的 schema v2 和未知组合拒绝。parser/状态机接入 ordered block executor，同块共享完整链证据与交易前余额，运行时错误必须恢复 energy、SQLite 与 tracker staging。原批次的双 v2 配置已由 r4 后的版本分层修正替代。
- 新 scope 的 v2 测试 catalog 显式 pin；旧 embedded registry 和 v0 发布包身份不变，新程序拒绝旧规则。
- 来源、操作路径、资格拒绝原因通过审计接口提供；缺少链上证据与确定的协议 Invalid 使用不同错误分类。
- state commitment 若新增编码，先定义向量，再同步 Rust、Go、checkpoint 与黄金文件；不能只更新版本字符串。
- 新 registry revision 的验证使用独立 dataset；第一批仍未提供现有 dataset 的在线 revision 迁移。

退出条件：origin 首块、旧 schema 拒绝、未知版本失败关闭、两作用域互不影响和 Rust/Go 互验通过。query ready 不得被误当作来源历史已经可用于共识。

### 5. 控制面与钱包流程（已实现）

- prepare 输入/显示分开 source D、recipient E；建议 prev 来自 D，展示所有将消费和保留的 pass。
- 展示收益/Leader 配置、source proof 支持范围、当前资格、观察高度和预期路径。draft 观测不是将来 reveal 必然有效的保证。
- 开户和迁移增加完成核验：交易确认、指定铭文 Active、来源与配置匹配、prev 消费结果、残留 UTXO。
- 未核验成功不提示入金/清扫余额；小额干扰或地址抢占时提示换新地址；Leader 迁移提示协作者重绑。
- 开发 execute 的真实选币需保证 sat 来自 D，并保护 prev UTXO；生产钱包没有验收的能力继续明确显示未支持。

退出条件：prepare 不把 wallet_name 当来源；缺证据不显示成功；首开/同地址/cross-owner 三流程及异常恢复可操作；前后端契约测试通过。

本批落点：`usdb-control-plane/src/mint.rs`、钱包页 `MinerPassMint.tsx`。prepare/execute 必填 D/E；
verify 以用户预期配置及精确 inscription ID 独立核验。indexer 新增只读 `get_pass_mint_source` 和
owner 列表 `ever_valid_owner`，无共识状态或存储格式变更。固定 Leader 与地址跟随的预检条件分别处理。
开发执行强制 regtest + 开关 + Ord 规范链就绪，锁定实际 cardinal satpoint，保护旧证；正式页面只读生成草案与核验。

验证入口：

- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-control-plane`：资格、完整配置、来源、确认数、prev、分页、缺证据、协作绑定与公开网络执行拒绝。
- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer`：首开来源重建、规范链变化/缺证据不改写审计、历史占用及既有状态恢复回归。
- `tests/test_control_plane_mint_browser.py`：隔离认证控制台 + Chromium，覆盖三路径、协作、错误清除、迟到响应、开发广播后仍须核验；沿用 `tests/test_control_plane_wallet_browser.py` 验证钱包隔离。
- `tests/run_control_plane_mint_live.py --bitcoind <path> --ord <path>`：预先构建 debug 二进制及网页，在全新临时目录启动真实 Core / Ord / BH / indexer / control-plane；首次、同地址、跨地址到 P2WPKH 均经真实 API 广播核验，旧证 UTXO 保持位置，收益配置篡改及占用后伪造开户被工具拒绝。退出停止本次子进程，保留日志。

Rust 回归由现有 fast gate 自动包含；浏览器与真实服务脚本可独立复跑。完整 weekly 留给后续 CI，未运行在线升级或重置。


### 6. 真实签名与发布验收

在新建的隔离 regtest 数据目录中，用真实签名的 commit/reveal 验收：

1. P2PKH、P2WPKH、P2TR key-path 允许形式，以及至少一组链上有效但 USDB 不支持的 sighash/script 形式。
2. 同块和不同块 commit/reveal，多输入非首来源，pointer/unbound/歧义形式。
3. 首次开户与攻击拒绝、同地址 remint、多 prev 跨地址继承、外部 pass 买入后激活。
4. 跨块 reorg、故障重启、snapshot 恢复，以及增量与从 origin 完整重放一致。
5. `txindex=0`、AssumeUTXO 基线、commit 早于 index origin/base 的历史取证；明确是否需要额外历史数据保留及下载就绪门槛。
6. 普通付款到恶意 commit 后可被用于 pass 操作的接受边界，不能写成“攻击已被阻止”。

可复用 `tests/assumeutxo_indexer_inputs.rs`、`tests/run_assumeutxo_p64_live.py`、`tests/common/minting.py` 和 world-sim/reorg 工具，但新测试不得连接或重置在线节点。现有测试通过不自动构成 v2 验收。

退出条件：上述证据均可复现，部署能够提供所需历史输入，UI 不夸大安全性，才进入 testnet-v1 网络参数/catalog/genesis/checkpoint/镜像与重置公告冻结。正式网仍使用独立作用域及按高度升级策略。

## 完成判定

只有上述链上取证、资格、状态机、重放、工具和真实签名验收全部完成，才可报告“MinerPass 新方案已实现”。规范草案完成、某个攻击单测通过、registry 能加载 v2，均不能替代整个验收。

本轮不需要把 BIP-322、OP_RETURN、commit 同时迁移余额或 Leader 自动跨地址跟随加入依赖；这些均保留为后续增强。
