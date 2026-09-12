# Balance-History AssumeUTXO 验证与重构大纲

## 1. 状态与目标

- 状态：Draft，作为后续分批实施与验收的工作入口。
- 建立日期：2026-09-12。
- 当前基线：USDB `2b8f15e`；Bitcoin 主网；USDB index origin `963800`。
- 本次完成：源码/上游机制核对、宿主机盘点、本大纲，以及31.1独立二进制验证、regtest冒烟、935000快照加载与恢复、前台追平及同锚点全量UTXO对拍。
- 首轮结果：Bitcoin前台验证通过，后台未完成，实验节点已正常停止并保留数据；见[第一轮实测记录](./bitcoin-core-31.1-assumeutxo-validation-2026-09-12.md)。
- P4进展：主网935000导入、重放至963800和全量UTXO/非零余额/逐块commit对比均通过，6项小规模测试通过。原cookie故障已恢复；见[主网复核记录](./balance-history-assumeutxo-p4-validation-2026-09-12.md)及[操作步骤](./balance-history-assumeutxo-p4-operations.md)。
- 尚未执行：镜像构建、现有服务升级及balance-history运行时重构。

目标是验证并实现：从 Bitcoin Core 支持的 `935000` UTXO 快照恢复完整 UTXO 与脚本余额，
重放 `935001..=963800` 共 `28,800` 个区块，从 `963800` 起提供正确余额及后续历史服务，
逐步替代大型 USDB core snapshot，并缩短整套节点首次可用时间。

必须分别证明 Bitcoin 快照可用、Bitcoin 状态一致、balance-history 状态一致、USDB 全链路可用。
任何一个较早阶段通过，都不能代替后续阶段的验收。

## 2. 已确定的方向

1. 首先准备 **Bitcoin Core 31.1 的独立版本环境**。无需先替换现有 28.1 主网节点；旧节点可继续作为全量历史对照源。
2. 在新目录启动 31.1，下载并加载 935000 快照，是第一轮 Bitcoin 侧验证的主路径。
3. 现有数据目录的 28.1→31.1 升级是独立兼容性验证，可在新版本冒烟验证后安排，不是快照导入实验的前置依赖。
4. 首轮只替换数据初始化路径，保留现有余额与 commit 算法。兼容旧 commit 链时，需要额外核验的小型 USDB 锚点。
5. 不将“31.1 启动成功”“loadtxoutset 成功”“前台追平”“后台验证完成”“USDB consensus-ready”合并成一个结论。
6. 935000 是导入基线，963800 是服务起点，二者不得混用。未来更换快照时，基线必须不晚于需要恢复的最早状态高度。
7. 用户已确认目标节点按全块磁盘要求部署：采用 `prune=0`，遵循官方后台历史验证；不设计跳过后台验证或自动裁剪旧块的默认方案。

## 3. 当前基线与容量

以下为 2026-09-12 的只读观测，执行下一阶段前需刷新。

| 项目 | 当前证据 |
| --- | --- |
| 本机主网程序 | `/home/bucky/btc/bitcoin-28.1/bin/bitcoind` |
| 主网数据目录 | `/home/bucky/.bitcoin` |
| 主网 RPC 版本 | `280100`，`/Satoshi:28.1.0/` |
| 主网观测高度 | blocks=headers=`966657`，`initialblockdownload=false` |
| 主网存储/索引 | `pruned=false`；txindex 与 basic block filter index 均同步至 `966657` |
| 另一运行实例 | 同版本 regtest，数据目录 `/tmp/usdb-world-soak-ord-reorg-fix-smoke/bitcoin`；不是待升级主网实例 |
| 根分区 | `/dev/sda1`，约 99% 已用、29 GiB 可用 |
| `/data` | `/dev/sda3`，约 458 GiB 可用 |
| 发布镜像构建 | `docker/Dockerfile.bitcoin-core` 固定 Bitcoin `28.1`、archive hash 与签名验证 |
| 963800 core 文件 | `37,006,364,672` bytes，约 34.46 GiB |
| 963800 registry 文件 | `196,696,915,968` bytes，约 183.19 GiB；为可选组件 |

主网观测 block hash：
`000000000000000000022d3064dd51eb343e3b1eb985a23d3cc676dea2567f9c`。

容量结论：实验大文件放在 `/data` 下的新目录；不能按当前剩余容量规划第二份无限增长的非裁剪全链，
也不能先复制整个旧主网数据目录。目录实际归属、资源限制和容量预算在执行时核实。
预算需同时包含下载文件、两份 chainstate、保留区块/undo、实验数据库、导出文件与排序临时文件。

## 4. Bitcoin 升级与快照加载的区别

### 4.1 原数据目录升级

Bitcoin Core 官方支持从旧版本直接升级：正常关闭旧进程，等待退出，再以新程序打开原数据目录。
本项目以复用现有 blocks、chainstate 和索引为目标，不预设清库或 `-reindex`；若出现数据库迁移或索引兼容问题，
先保留日志并判定原因，不自动重建。依据：[31.1 升级说明](https://bitcoincore.org/en/releases/31.1/)。

升级验收覆盖：

- 明确服务管理方式、实际可执行路径、配置来源、数据目录与挂载，避免影响另一 regtest 实例。
- 记录停机前高度/hash、索引状态和配置；建立适合实际数据规模的恢复方案。
- 配置兼容检查包含已移除选项和资源参数；31 的默认 dbcache 与 mempool 策略有变化。
- 若现有实例加载 BDB legacy wallet，需单列钱包迁移；Core 30 起不能直接加载该格式，不能把链数据兼容等同于钱包兼容。
- 正常关闭 28.1，确认退出后启动 31.1；两个进程不得写同一个数据目录，不通过硬链接共享可写数据库。
- 验证原高度/hash 保留、随后正常接块、txindex/filter index 可用，以及 balance-history、indexer 和相关测试 RPC 的兼容性。
- 回退依赖实际数据库兼容性与恢复点，不能仅凭保留 28.1 二进制就承诺可原地降级。

配置/钱包变化依据：[31.0 发布说明](https://bitcoincore.org/en/releases/31.0/)、
[30.0 Wallet 变化](https://bitcoincore.org/en/releases/30.0/#wallet)。

### 4.2 空目录加载 AssumeUTXO

新节点使用独立 datadir、配置、cookie、RPC/P2P 端口和日志，不读取旧节点的 chainstate/索引目录。
现有已追平节点不应通过回滚、清库或重建来扮演快照冷启动节点。

Core 31.1 内置的主网 935000 身份：

| 字段 | 值 |
| --- | --- |
| base height | `935000` |
| base block hash | `0000000000000000000147034958af1652b2b91bba607beacc5e72a56f0fb5ee` |
| AssumeUTXO UTXO commitment | `e4b90ef9eae834f56c4b64d2d50143cee10ad87994c614d7d04125e2a6025050` |

最后一项是 Core 定义的 UTXO 内容承诺，**不是下载文件的 SHA-256**。
依据：[31.1 chainparams](https://github.com/bitcoin/bitcoin/blob/v31.1/src/kernel/chainparams.cpp#L151)。

加载前需已同步包含基线区块的有效 headers 链；还要满足 Core 的加载条件，包括没有已激活的 snapshot chainstate、
mempool 为空等。正式执行脚本按 31.1 实际 RPC 错误处理，不放宽校验或修改 chainparams。
依据：[ActivateSnapshot](https://github.com/bitcoin/bitcoin/blob/v31.1/src/validation.cpp#L5166)。

加载完成后，前台从基线追平，后台继续下载验证旧链。首轮记录 `getchainstates` 的每条 chainstate 与 `validated`，
不以 `initialblockdownload=false` 替代后台验证完成。索引仍从创世顺序构建，不能用 txindex 追平作为快照前台可用的条件。
依据：[AssumeUTXO 使用说明](https://github.com/bitcoin/bitcoin/blob/v31.1/doc/assumeutxo.md)、
[getchainstates](https://bitcoincore.org/en/doc/31.0.0/rpc/blockchain/getchainstates/)。

## 5. 分阶段工作与验收

| 阶段 | 工作 | 完成条件 | 当前状态 |
| --- | --- | --- | --- |
| P0 | 基线、容量、依赖盘点 | 来源、版本、目录和实验边界明确 | 本文已记录；执行前刷新 |
| P1 | 31.1 独立二进制及候选镜像准备 | 二进制来源验证、版本确认、独立 regtest 冒烟通过 | 独立二进制与冒烟通过；现有镜像固定三签名组合不满足31.1，候选镜像待处理 |
| P2 | 独立 Bitcoin AssumeUTXO 验证 | 935000 加载、前台可用、同锚点状态对拍；后台验证另列结果 | 前台验证通过：966673全量UTXO及966674样本一致；后台到105439，尚未完成 |
| P3 | 现有 28.1 数据目录升级兼容验证 | 原数据复用与现有消费者回归通过 | 待安排；不阻塞 P2 |
| P4 | balance-history 最小导入/重放原型 | 935000 UTXO+余额基线、小锚点、重放到963800 | 通过：主网导入、重放、三项全量投影对比一致 |
| P5 | balance-history 语义等价验证 | 全量状态、逐块 commit、历史边界、重启/reorg 验收通过 | 离线语义与独立regtest通过；主网28,801条commit/1,024个查询样本及下游承诺链路通过，整套在线端到端留待P6/P7；见[P5记录](./balance-history-assumeutxo-p5-validation-2026-09-12.md) |
| P6 | 整套节点快速启动与依赖改造 | 不依赖全量 txindex 完成即可达成实际 USDB 就绪 | 目标采用prune=0；其余待设计/实现 |
| P7 | 镜像、安装器和发布集成 | 新鲜安装与升级实测、证据归档、发布身份更新完成 | 待执行 |

P1 的隔离环境可先服务 P2；生产默认镜像和现有节点切换在对应兼容性验证后进行。
软件升级本身不改变 chain ID、index origin 或 UIP 版本；如果后续改变共识可观察语义，则另行设计版本与激活。

### P1：准备 31.1

1. 获取官方 31.1 二进制，固定平台、archive SHA-256、签名及验证所用公钥来源，安装在带版本号的独立路径。
2. 分别记录 `bitcoind --version`、`bitcoin-cli --version` 和文件 hash；不覆盖旧目录或通用程序链接。
3. 使用临时 regtest 验证启动、出块、正常退出/重启，以及依赖的 block、transaction、UTXO RPC。
4. 复核镜像中固定的多签名验证策略在 31.1 发布物上的实际签名覆盖；不只修改版本字符串，不静默跳过签名验证。
5. 候选镜像核对以下传播点：
   - `docker/Dockerfile.bitcoin-core`；
   - `.github/workflows/usdb-bitcoin-image.yml` 的 tag、OCI version 与报告；
   - `docker/scripts/tools/release_candidate_resolver.py` 的版本匹配及相关测试；
   - `docker/scripts/tools/resource_policy.py` 的 dbcache 假设；
   - `docker/scripts/tools/build_world_sim_images.sh` 与 sibling go-ethereum 的 `scripts/usdb/prepare_regtest_tools.sh`。

只修改确实属于 Bitcoin 的版本锁定；Docker Engine 版本要求和用于解析旧日志的 fixture 不做机械替换。
外部 Ord 与嵌入式 ord parser 的升级不混入本批次。

### P2：独立 Bitcoin 验证

#### 实验布局与资源

实验布局（首轮已创建 `20260912T105003Z`，原始快照保留在用户提供的 `/data/btc/`）：

```text
/data/usdb-assumeutxo-validation/<run-id>/
  artifacts/          # 下载快照、校验信息、固定版本工具
  bitcoin/            # 新 31.1 datadir，不与旧节点共享可写文件
  balance-history/    # P4/P5 实验库
  comparisons/        # 全量对拍中间数据、差异样本
  reports/            # 去除认证信息的命令、RPC、时间/资源记录
```

首轮 Bitcoin-only 实验曾设置裁剪预算。后续目标采用 `prune=0`，按完整区块、双chainstate暂存和索引需求预留磁盘，并设置明确的内存和容量停止阈值。
RPC 只绑定 loopback，使用独立 cookie；端口在执行前检查，不能直接复用旧节点参数。

第一轮裁剪实验的限制如下；这是已有实验数据的约束，不再作为目标部署的裁剪设计要求：

- 第一轮验证可以由现有非裁剪 28.1 节点提供后续实验所需历史区块，并在报告中标明来源。
- 若评估完整冷启动，必须证明新节点能提供全部尚未消费的 `935001..=963800` 及后续区块/必要 undo。
- 自动裁剪可能抢先删除这些数据。目标使用 `prune=0` 保留全部区块，另行验证实际缺块恢复与容量监控；若未来重新考虑裁剪，再设计消费进度协调。
- 首轮已实测：`prune=65536` 时，在前台尚未到963800之前，935001已被裁剪；双chainstate期间Core将裁剪目标除以二，IBD另有缓冲。
  详见[实测记录](./bitcoin-core-31.1-assumeutxo-validation-2026-09-12.md)及[31.1裁剪实现](https://github.com/bitcoin/bitcoin/blob/v31.1/src/node/blockstorage.cpp#L297)。
- 现有节点仍可供块只说明实验可继续，不构成摆脱现有全量节点依赖的性能证据。

#### 执行顺序

1. 获取公开的 935000 快照，记录来源、实际 URL、大小、文件 hash、获取时间及传输耗时；来源待执行时核验。
2. 先完成下载和基础校验，再启动独立新节点并同步 headers，及时执行 `loadtxoutset`，避免先完成传统 IBD。
3. 使用匹配版本的 `bitcoin-cli`、显式实验 datadir 和 `-rpcclienttimeout=0` 调用 `loadtxoutset`。
4. 保存加载结果的 `base_height`、`tip_hash`、`coins_loaded` 以及 Core 校验日志，匹配第 4.2 节身份。
5. 跟踪前台追平、前台首次可用、后台进度；记录实际高度/hash，不将加载返回的基线高度误当成随后节点的当前 tip。
6. 比较两个节点在同一 height **和** block hash 的完整 UTXO 统计，以及独立确定的 outpoint/script 样本。
7. 验证实验节点正常退出/重启后，快照来源、状态和前台同步可恢复。
8. 后台追至基线并通过完整验证时，单独补充完成证据；实验提前结束时如实记录“后台验证未完成”。

`loadtxoutset` 的原始快照保留到 P4/P5 完成，因为它也是 balance-history 导入器的输入，不能在 Core 加载后立即清理。
若公开快照无法获得，可在后续专用参考实例生成；不在正在服务的主网节点上直接执行历史 rollback 导出。
`dumptxoutset rollback` 会暂时回滚节点并暂停网络，不属于只读 RPC。
依据：[dumptxoutset 实现和接口约束](https://github.com/bitcoin/bitcoin/blob/v31.1/src/rpc/blockchain.cpp#L2853)。

#### 对比方法与限制

- 完整 UTXO 比较显式使用同一算法，例如两版本均支持的 `hash_serialized_3`，并核对 height、bestblock、txouts、total_amount。
- 两个实时节点先后调用 RPC 可能跨越新块；以 RPC 返回的实际锚点为准，不同锚点不得直接比较。
- 当前旧节点未报告 coinstatsindex；不能假设可向 `gettxoutsetinfo` 传入任意历史高度。
- `hash_serialized_3` 不支持任意历史高度查询。优先在双方同 tip 的窗口取样；未对齐则重试或安排专用参考状态。
- `gettxoutsetinfo` 无索引时会扫描完整 UTXO 集，是需记录 I/O 成本的实验步骤，不加入频繁状态轮询。
- 普通 `scantxoutset`/`gettxout` 对应实际 chainstate，不能拿其 tip 结果直接证明 963800 的余额。
- P2 的同 tip Core 对拍证明 Bitcoin 侧一致；963800 精确状态与 balance-history 的比较由 P4/P5 完成。

算法和高度限制依据：[28.1 RPC](https://github.com/bitcoin/bitcoin/blob/v28.1/src/rpc/blockchain.cpp#L854)、
[31.1 RPC](https://github.com/bitcoin/bitcoin/blob/v31.1/src/rpc/blockchain.cpp#L1001)。

### P4：balance-history 最小原型

1. 新增独立 bootstrap 导入入口，流式解析已验证的 Bitcoin 快照。核验网络、基线/hash、格式、记录完整性和内容身份，
   记录 Core 验证所对应的精确输入文件；不能把下载文件 SHA-256 当成 Core 的 UTXO 承诺。
2. 解码完整 Coin 字段后校验，再投影为本项目 `outpoint -> (script_hash, value)`；聚合每个脚本的完整余额。
   保留所有活 UTXO，包括零金额与未成熟 coinbase 输出；不把脚本白名单或钱包可花费规则引入余额语义。
3. 基线记录与真实历史变化分开表达。快照没有旧地址的最后变动高度/delta，不得伪造历史收入。
4. 由已核验的完整重放结果提供 935000 `BlockCommitEntry`，固定网络、BTC hash、算法版本与来源；
   再重放 `935001..=963800`。该小锚点仍是 USDB 自己的可信输入，Core 不验证它。
5. 首轮使用 RPC 按 canonical height/hash 获取区块，不触发当前从创世排序的 LocalLoader。
6. 在完整导入模式下，缺失 prevout 必须明确报错；不得通过现有 `getrawtransaction` 回退掩盖漏导入。
7. 导入使用 staging、完成标记和原子发布；将来源/验证状态纳入 provenance，使中断重试不会发布半成品。
8. 明确 balance/history/state-ref 查询下界与 rollback floor；基线以前的请求返回未保留，跨越基线的深 reorg 进入显式恢复路径。
9. 用快照中的 live scripts 建立 registry coverage，再补充后续新脚本；不能声称覆盖基线前全部历史脚本。

963800 是已应用该块后的余额断面。公开查询仍遵循网络冻结的 stable lag；离线精确状态对拍与在线服务就绪分开验收，
不能为了在该高度开放查询而临时修改 stable lag。起点是否提供该块的 exact delta，以及历史范围从哪一块开始，
需按导入/压缩后真实保留的数据设置两个 query floor。

可参考 Core 官方 [utxo_to_sqlite.py](https://github.com/bitcoin/bitcoin/blob/v31.1/contrib/utxo-tools/utxo_to_sqlite.py)
作为独立解析/对照工具。其输出不是 USDB core snapshot，转换成功也不是语义验收。
优先流式对拍；若使用 SQLite 中间库，先核算额外空间与排序成本。

### P5：同高度语义验收

主网固定目标：`963800`，BTC block hash：
`000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551`。

| 验收项 | 要求 |
| --- | --- |
| 活 UTXO | 与完整重放参考在相同高度/hash 对比全部 outpoint、value、script_hash，保留双向差异与样本 |
| 余额 | 按全部脚本聚合对比；覆盖长期未动、区间内花完、零余额和非标准脚本，不只比较总金额 |
| Commit | 核验935000锚点；后续逐块 delta root/commit 与完整重放结果一致 |
| 状态引用 | 比较 latest_block_commit、snapshot_id 及下游 pass/local/system state，不能只比较 snapshot_id |
| 查询语义 | 区分基线元数据与真实 delta；分别核对点查、范围、聚合、无历史/零余额及查询 floor |
| Registry | 核对已声明覆盖范围内的解析；缺失更早历史脚本应明确返回 coverage 状态 |
| 恢复 | 导入中断、重复启动、正常退出/重启，保证不会暴露部分状态 |
| Reorg | 在独立 regtest 构造跨基线旧输出花费、同块花费、重组与重启；不在主网参考节点制造 reorg |

现有 core snapshot 可作为参考输入，但要绑定其文件 hash、生成版本及来源；文件签名/完整性不能单独证明它的语义。
测试预期区分“余额与 commit 等价”及“基线前元数据无法恢复”，明确可接受差异，不能把所有行不等都归为预期差异。
若需要重新定义 RPC 或 commit 契约，先停止兼容性结论，再提交单独的语义/版本设计。

### P6/P7：整套节点重构与交付

- 替换 indexer 的历史 prevout 金额与 reveal transaction 查询来源；处理消费者落后时 UTXO 已被花费的情况。
  当前 live-UTXO RPC 和有限 undo 窗口不能自动覆盖所有历史查询。
- 区块采用 `prune=0` 完整保留；另行设计 prevout/undo 的可用性与缺块、落后、重启恢复，不能把保留全块等同于 txindex 已追平。
- 将前台可用、后台验证进度、balance-history 就绪和 indexer 共识就绪分别公开；不只删除旧 readiness 检查。
- 调整资源政策、候选镜像与 release digest、node-kit 配置、bootstrap 来源选择、provenance、诊断和恢复流程。
- 在不依赖旧本机全量节点的环境中跑完整冷启动，确认缺省路径不再暗中等待全量 txindex。
- 保留旧 core snapshot 作为验证参考/可选启动方式，是否继续发布由测得的时间、空间与运维成本决定。
- 发布前完成独立 regtest、相同锚点主网对拍及安装/升级验证；只有代码或镜像构建通过不能标记发布就绪。

## 6. 证据与性能记录

每次运行使用唯一 run-id，报告包含：

- USDB/Bitcoin/工具版本、文件 hash、镜像 digest（如使用镜像）、配置摘要和数据来源；不保存 cookie/password。
- 快照文件身份、Core UTXO 承诺、基线/目标高度及 block hash、小型 USDB 锚点身份。
- 下载、headers、加载、前台追平、后台验证、BH 导入/重放、整套 consensus-ready 的独立耗时。
- 峰值内存、磁盘读写量、下载量、峰值及最终占用；区分 artifacts、chainstate、blocks、RocksDB、registry 和临时文件。
- 每项验收的 `pending/pass/fail`、原始证据、差异样本和未完成项；失败运行也保留报告。

与现有方案的性能比较必须采用相同硬件预算、网络/缓存条件和服务边界。
若新节点从本机旧节点取块，明确标记为本地供块实验，不推断公网冷启动耗时。

## 7. 下一批具体工作

1. [P5离线语义与regtest验收已通过](./balance-history-assumeutxo-p5-validation-2026-09-12.md)，保留当前963800实验状态作为参考；下一阶段从独立小锚点、生产bootstrap及indexer历史查询/readiness设计开始，整套在线端到端另行验收。
2. P6采用prune=0，纳入刷盘期间RPC超时、内存及完整区块空间开销；当前已裁剪的P2实验节点仍不作为完整重放区块源。
3. 原目录升级P3单独安排；镜像集成需处理31.1实际发布签名与现有固定三签名组合的差异，并核算导入额外内存，不能只修改版本和dbcache。

本大纲不等于已完成上述升级或实验；后续每批工作更新阶段状态并链接实际报告。

## 8. 本地实现入口

- [Core snapshot schema](../../src/btc/balance-history/src/db/core_snapshot_v1.sql)
- [快照安装/导出](../../src/btc/balance-history/src/index/snapshot.rs)
- [余额计算与 commit 链](../../src/btc/balance-history/src/index/block.rs)
- [历史 state-ref](../../src/btc/balance-history/src/service/state_ref.rs)
- [RPC 查询语义](../../src/btc/balance-history/src/service/rpc.rs)
- [LocalLoader](../../src/btc/balance-history/src/btc/local_loader.rs)
- [Indexer 输入金额查询](../../src/btc/usdb-indexer/src/btc/utxo.rs)
- [Bitcoin RPC 旧交易回退](../../src/btc/usdb-util/src/btc/rpc.rs)
- [现有 Bitcoin readiness](../../docker/scripts/tools/check_bitcoin_readiness.py)
- [963800 发布记录](../../docker/networks/testnet-v0/snapshots/balance-history-snapshot-release-record.json)
- [现有 Core UTXO 抽样审计](./balance-history-bitcoin-core-utxo-audit.md)
- [拆分快照对拍操作](./balance-history-split-snapshot-audit-operations.md)
