# AssumeUTXO P6：生产 bootstrap 设计与分步实施

## 1. 决策与当前边界

2026-09-14 补充：可选的业务 genesis 合并快照加速方案见
[P8 精简合并基线快照计划](./balance-history-unified-baseline-snapshot-plan.md)。
P8.1 先实现统一导出与校验；无 BH 快照时继续使用本文的原生导入重放流程，生产安装接入另行验收。

日期：2026-09-12。P4/P5 验证批次已提交为 `ce94958`；其结果继续作为历史证据。
P6.1 已提交为 `cdf4f7c`；P6.2 检查点批次已提交为 `a897975`，实现及实测见[原生启动操作文档](./balance-history-assumeutxo-p62-operations.md)。
当前方案采用可信旧 commit 检查点与原生导入重放，保持与从创世块重放相同的 v1 commit 链。
`cdf4f7c` 中以业务起点状态生成新种子的方案已被本次检查点方案替代，未作为生产协议发布。

- 当前实验使用 Bitcoin 导入基线 `B=935000`、USDB 业务起点 `G=963800`，均表示应用该块后的状态。
  **业务 genesis 必须满足 `G >= B`**；正式网可以选择更晚的 G，配置固定其 canonical BTC hash。
  原生配置不再硬编码 G=963800，但目前受支持的主网快照检查点仍只有 B=935000。
- 新生产 bootstrap 取消对历史 **core snapshot、script-registry snapshot 文件**的依赖。
  Bitcoin 原始 UTXO 快照及其身份校验仍是输入；代码内置经过验证的 C(B) 检查点，导入、聚合余额并重放 `B+1..=G`。
- 只需完整保留 B 时仍有活 UTXO 的脚本，以及 B 之后观察到的脚本。余额后来归零时，保留已建立的反向映射。
  金额为零的活 UTXO 也保留；不能以“当前余额为零”作为删除 UTXO 或 registry 的规则。
- **保留 LocalLoader**，P6.3 已接入按实际积压量启用的原生路径，不随旧快照机制删除。
- 节点采用 `prune=0`，继续运行 Bitcoin Core 官方后台历史验证；快速启动依赖所需前台数据可用，后台进度单独展示。

当前已实现 P6.1 独立状态摘要/只读计算，以及 P6.2 检查点驱动的原生导入、重放、封存、服务启动和 RPC 接入。
P6.3 已提交为 `8103550`，实现与 Core 并行的导入/逐段重放及本地取块，见[P6.3 验收与操作](./balance-history-assumeutxo-p63-operations.md)。
P6.4 已提交为 `7f52ee2`，接入按消费块查询历史输入、块内定位 reveal 及 readiness，见[P6.4 验收与操作](./balance-history-assumeutxo-p64-operations.md)。
P6.5 已提交为 `22cf06c`，修复了重组后旧轮询snapshot覆盖新历史锚点的竞态；独立 Core/BH/indexer 整套验收连续三轮通过，含真实mint/转移、逐块commit对拍、缺块/undo及强制中断恢复、重组和查询下界，见[P6.5 验收与主网步骤](./balance-history-assumeutxo-p65-operations.md)。
本轮检查点兼容验收结果见 P6.2 操作文档；之前 v2 种子的实测不作为当前 commit 兼容证据。主网原生重导入、同锚点服务复核和本地读取性能待安排；部署默认切换属于P7。
现有 P4 `import/replay` 继续按旧验证协议使用 reference core；不能把新增计算入口等同于已经移除生产依赖。

## 2. P6 拆分

| 步骤 | 范围 | 验收边界 | 状态 |
| --- | --- | --- | --- |
| P6.1 | 定义 G 高度状态摘要及可信旧 commit 检查点，提供只读检查器 | 全量重放与导入重放结果相同；无需旧快照文件；状态摘要与 rolling commit 分离，独立编码向量一致 | 本批次实现，小规模测试通过；主网完整状态扫描待安排 |
| P6.2 | 原生 bootstrap 生命周期与版本接入 | 仅需 Core 快照与内置检查点的新库可导入、重放、封存 G 并正常接块；中断恢复、查询下界、回滚和下游身份一致 | 已实现，小规模与真实Core/BH进程验收通过；主网长任务待安排 |
| P6.3 | LocalLoader 独立适配与并行 bootstrap | 后台尚未补齐 B 以前区块时仍可加速；支持两个追加文件、乱序、部分尾部、XOR、重组与重启；G 稳定前可导入并逐段重放 | 已实现；7项专项、206项库回归及真实 Core/BH 渐进同步通过，主网性能待安排 |
| P6.4 | indexer 历史 prevout/reveal 查询与 readiness | 不依赖全量 txindex 追平；落后消费者和历史已花费输出仍正确；不把 Core 前台可用等同于整套就绪 | 已实现；326项indexer回归通过，真实Core关闭txindex/落后2050块的输入、reveal及同块转移验证通过；整套服务交付留待P6.5/P7 |
| P6.5 | 整套端到端验收与运维步骤 | 独立 regtest、主网相同锚点复核、冷启动/恢复/资源证据完整 | 独立Core/BH/indexer整套验收通过；主网原生重导入、同锚点复核及性能长任务待操作人员安排 |

P7 已开始服务配置与入口接入，可与主网长任务并行，见[P7 部署改造计划](./balance-history-assumeutxo-p7-deployment-plan.md)。
Bitcoin 31.1 镜像、node-kit、安装/升级与发布身份集成仍待完成。原数据目录升级 P3 独立安排。

## 3. 同一条历史 commit 链

B 高度的 AssumeUTXO 文件提供应用 B 后的 Coin 集合，不包含 USDB 历史 commit。
内置的 `C(B)` 与这份状态配对；第一个重放块为 B+1。全量重放和快照导入共用唯一的 v1 计算路径：

```text
C(h) = SHA256(
    UTF8("balance-history:block-commit:v1")
    || height:u32BE || btc_block_hash:raw32
    || balance_delta_root:raw32 || C(h-1):raw32
)
```

`balance_delta_root` 沿用原有 v1 编码，包含当块按 script 排序的余额变化及最终余额。
C(B) 相同、导入状态等价且消费相同 canonical 区块时，后续每块的 delta root 和 commit 都与全量重放一致。
G 只决定服务的历史/恢复边界，**不在 G 重置 commit，不把状态摘要写入 commit 链**。
更换受支持的 B 时，必须提供同一条历史链上与新快照配对的检查点；G 始终满足 G>=B。

### 3.1 检查点身份与生成证据

[内置主网检查点](../../src/btc/balance-history/src/bootstrap/checkpoints/mainnet-935000.json)绑定：

- Bitcoin 网络、B、BTC block hash、快照文件 SHA-256、Core `hash_serialized_3`。
- balance-history 数据模型和旧 commit 协议 `1.0.0`。
- C(B)，以及同一历史记录中的 `balance_delta_root(B)`，以保证 G=B 时完整 block-commit RPC 记录也相同。

当前 C(935000) 为 `6108f77e4abaafbc3a7a246024e942483c18fb37a3c710209ea6294c5617fe81`，
该块 delta root 为 `3265637bf1b3ec9bbd6936d5d76740890a81183234695106cb96a8e89d6dc398`。
两项于本轮从保留的 P4 实验 RocksDB 只读提取；P4 已核对含基线的 28,801 条完整 commit 记录与旧参考库一致。
旧参考 core SHA-256 为 `3e3490ac19521647a8513a0ef2961607df4456ba3e3ea6922c1ba750f7fbea61`，
验收见[P4固定身份及全量比较](./balance-history-assumeutxo-p4-validation-2026-09-12.md)。

Core 验证 UTXO 集合，不验证 USDB 历史承诺。新增检查点需独立核对来源、算法版本及重放结果后随代码发布；
主网配置不能覆盖检查点。`regtest_checkpoint` 只允许私有 regtest 链显式提供，且同样绑定源快照身份。
每台部署节点使用已发布检查点，不需要从 0 重建 balance-history；Core 官方后台历史验证保持不变。

只读提取候选检查点的命令为：

```bash
balance-history-assumeutxo-tool checkpoint --state-dir <停写的状态目录> --identity <SnapshotIdentity.json>
```

该命令只读一条已保留的历史 commit，**提取不等于批准**；它不重新验证快照内容或整个历史链。

### 3.2 独立状态摘要与 RPC 契约

状态摘要 `origin_state_digest` 用于比较不同导入路径的逻辑状态，独立于 C(G)。
它按以下顺序编码后做单次 SHA-256：

1. 长度前缀 UTF-8 字符串：`balance-history-bootstrap-state:v1`、commit 协议 `1.0.0`、数据模型版本。
2. 网络 genesis hash raw32、G 的 u32BE、G 的 BTC block hash raw32。
3. 依次编码 UTXO 和余额表的行数 u64BE、总 satoshi u64BE、有序投影 hash raw32。

UTXO 投影按 `txid:raw32 || vout:u32BE` 降序，每行追加 script hash raw32 与金额 u64BE；保留零金额输出。
余额投影为每个 script 在 G 时的最新非零余额，按原始 script hash 降序，编码 hash raw32 和余额 u64BE。
不纳入旧历史行、registry 总量、导入高度、物理文件布局或旧 commit。字符串长度使用 u32BE；
BTC raw hash 字节顺序与常见 RPC 显示 hex 相反，script hash 使用数据库原始字节。

封存后元数据分别记录 `origin_commit`、`origin_balance_delta_root` 和 `origin_state_digest`。
正常 commit RPC/state-ref 均报告 `1.0.0`；相同高度下，余额、commit、state-ref 及下游 pass/local/system 均应与全量重放一致。
`get_bootstrap_info` 单独返回快照来源、内置检查点、阶段及独立状态摘要。

余额查询从 G 开始，精确变化历史从 G+1 开始；即使保留了 D(G)，G=B 时仍不能从 UTXO 快照还原当块逐地址变化。
B 以前的历史余额不属于业务需求。registry 保留 B 时活脚本和之后观察到的映射，不声明全历史覆盖。
原生库使用 `balance-history-rocksdb-schema:native-checkpoint-v1`，避免旧实验 v2 库被按 v1 打开。
数据目录不做静默转换；已有服务及发布配置不因本轮测试自动切换。

## 4. 原生 bootstrap 生命周期（P6.2/P6.3）

```text
核验 Bitcoin 快照身份 / Core 逻辑承诺
  -> staging 导入 B 的 UTXO、余额与 live scripts
  -> 安装内置 C(B)/D(B)，按 canonical 区块重放 B+1..G，补充 registry
  -> 校验并封存 G 状态，保留原 v1 commit / 写入查询下界与来源
  -> 原子发布为可用数据库
  -> 正常同步 G+1..tip，沿用原 v1 rolling commit 与既有重组规则
```

实施要求：

- Core RPC 可用且快照基线 B 已在 active chain 后即可导入；不要求前台已到 G、最新高度或后台历史验证已完成。
  重放目标随 Core 推进，为 `min(G, active_tip - stable_lag)`；没有下一稳定块时可取消地等待并保存进度。
  G 的固定 hash 在其可用时核验；发布仍要求 G 达到稳定深度，并通过完整状态校验。当前 lag=10、G=963800 时，发布至少需要 active tip=963810。
- 新入口无需旧 core、registry sidecar 或外部分发的锚点文件；检查点随代码内置，旧 P4 工具保留为验收工具。
- 在 staging 的导入/重放阶段持续记录进度和可恢复状态；半成品不能被正常服务当成已就绪库。
- G 之前允许内部重放元数据，公开查询按 floors 限制。封存将状态审计、原 commit、检查点和 floors 原子关联。
- 从 UTXO 聚合余额必须独立验证；当前只读计算器的双表总额相同不能证明每个 script 一致。
- 把来源、Core 快照校验、业务状态校验和后台验证进度分开记录；不重新实现或跳过官方历史验证。
- 余额零行可按保留契约压缩；registry 一旦观察到映射则继续保留，以支持后续矿工证地址反查。
  业务需要解析 B 以前已完全消失且以后未再观察到的脚本时，明确返回覆盖范围状态，不能声称全历史覆盖。
- 基线回滚下界和 undo 保留窗口显式设置。跨越可恢复边界的深重组进入恢复状态，不返回不正确的余额/承诺。
- 在原子发布、中途退出、重复启动、导入损坏、重组及版本不匹配场景验收后，才切换生产默认入口。

## 5. LocalLoader 独立适配（P6.3）

相同 canonical 区块的交易内容不因 AssumeUTXO 改变，文件物理布局及下载顺序却不能假设相同。
Core 31.1 为 `NORMAL` 和 `ASSUMED` 分别维护 blockfile cursor；两个文件可以独立追加，
文件编号不等同于业务所需的连续高度区间。
依据：[Core blockfile cursors](https://github.com/bitcoin/bitcoin/blob/v31.1/src/node/blockstorage.h#L236)、
[文件分配实现](https://github.com/bitcoin/bitcoin/blob/v31.1/src/node/blockstorage.cpp#L767)。

旧全历史 LocalLoader 的两个假设不再用于原生模式，旧入口仍保留：

- `generate_sort_blocks()` 从全零 prev_hash/height=0 开始连链。B 以前区块尚未完整到达时，不能据此发现 B 以后的连续区间。
- `file_indexer` 只排除最大编号文件，将其余文件视为完成；不能覆盖两个 cursor 的追加行为。

P6.3 已按以下要求接入独立 `canonical_loader` / `canonical_index` 路径：

1. 以已核验 B/hash 或恢复点的 canonical hash 开始，建立需要消费的 height/hash 序列；不要等待创世到 B 的连续本地索引。
2. 物理层记录 `block_hash -> (file, offset, length)`，逻辑层按 canonical height/hash 选块，验证块内容与父 hash。
3. 每个文件独立保存已完成记录的偏移，重扫所有可能增长的文件，正确处理预分配零尾、未写完记录与 XOR。
   仅凭连续两次文件大小相同也不能永久认定文件已封存。
4. 覆盖乱序下载、重复记录、旧分叉、重启及双 cursor 轮换；重组后重新核对 canonical 映射和恢复点。
5. 本地缺块或尾部尚未完成时显式回退 RPC，并继续增量索引；不能默默回到“等历史补齐”。
6. 对同一连续区间比较 RPC 与 LocalLoader 的块 hash、UTXO/余额及 v1 commits；小规模一致性已通过，主网实际加速收益待测。

每批按 `min(阶段目标, active_tip - stable_lag) - 已处理高度` 判断积压，超过
`sync.local_loader_threshold`（默认500）才启用本地读取；接近目标时使用 RPC，运行后重新积压也可再次启用。
bootstrap 阶段目标为 G，正常同步阶段为配置的最大高度。本地缺文件、缺块、部分写入或校验失败均回退 RPC。
索引独立保存于 `<root>/local-block-index/<source-identity>`，只记录候选物理位置；消费时核验 canonical hash、
父链、Merkle root、witness commitment 和重复 txid。每次最多扫描4096条候选记录，各文件保留独立游标，
mtime/大小变化及周期性尾部复查共同处理预分配追加，不读取 Core 的内部 LevelDB。

并行启动减少等待，但不能据此断言 LocalLoader 大多数时候不会使用：导入 UTXO 期间 Core 仍在下载，
停机恢复、处理速度差异和更大的 G-B 都可能形成积压。它是按需加速项，不是启动前置条件。

该步骤保留 LocalLoader 的加速职责；txindex 和 indexer 历史交易定位属于 P6.4，不能用文件扫描通过代替其验收。

## 5a. indexer 历史输入与 readiness（P6.4）

- reveal 从当前处理块定位，历史输入金额从 `getblock(hash, 3)` 的 Core undo 读取；不依赖 txindex、当前 live UTXO 或 BH undo 保留窗口。
- 每块惰性加载并校验完整响应，缓存仅限一个精确块；缺失 undo、交易内容/输入身份不符或重组时失败并重试，不推进持久化高度。
- readiness 新增 `block_processing_pending_height` / `BlockProcessingPending`，在块未提交或失败期间阻止共识就绪。
  新的单块数据预检单列前台与 chainstates，不将其成功等同于 USDB 整套就绪。
- G>=B 保持有效；G=B 时快照不包含 B 自身的交易/undo，若业务处理需要这些尚未到达的数据，仍需等待该块数据可用。
  G>B 时消费块位于快照链前台验证区间，当前实验963800>935000符合此条件。
- 外部 Ord 历史源、默认部署 readiness gate 和镜像启动编排仍需独立处理；快速路径使用现有 bitcoind inscription source。
  详细实现、验收和操作见[P6.4手册](./balance-history-assumeutxo-p64-operations.md)。

## 6. P6.1 只读主网计算操作（待人工安排）

本批次没有执行主网全量扫描。该操作顺序读取两张逻辑表，耗时取决于缓存、历史行数与并发磁盘负载，
不从此前 P5 抽样耗时推断 ETA。无需启动/升级/重启 bitcoind，也不需要 RPC cookie 或旧 core 文件。
要求目标 **balance-history 实验库没有写入者且停留在 963800**；保留它作为后续对照，不继续 replay。

在仓库根目录构建：

```bash
CARGO_BUILD_JOBS=2 cargo build --offline --locked --release \
  --manifest-path src/btc/Cargo.toml -p balance-history \
  --bin balance-history-assumeutxo-tool
```

开始扫描（输出目录使用新建的唯一目录；下列命令不会改写实验库）：

```bash
P6_RUN=$(mktemp -d /data/usdb-assumeutxo-validation/p6-origin-963800.XXXXXX)
P6_TOOL=/home/bucky/work/usdb/src/btc/target/release/balance-history-assumeutxo-tool
P6_STATE=/data/usdb-assumeutxo-validation/p4-mainnet-935000-to-963800/state
"$P6_TOOL" origin-state \
  --state-dir "$P6_STATE" --network bitcoin --height 963800 \
  --block-hash 000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551 \
  > "$P6_RUN/origin.json" 2> "$P6_RUN/origin.log"
```

执行前可另开终端，以刚才生成的实际目录查看 `tail -f <目录>/origin.log`。
记录命令退出码；只有退出码 0 且 JSON 完整才是成功。日志每 10 秒报告扫描行数、投影行数和耗时，
每张表结束时输出行数与金额；中止只读计算可以重跑，不产生导入半成品。

预期核对：

- `identity.origin_height=963800`，BTC hash 与命令一致，`activated=false`。
- UTXO 行数 `165748439`，digest `86cba93334b2a6b6862a00d070617a0bdfded56b8eec2a25c41c2dcff82faedd`。
- 非零余额行数 `59356343`，digest `7e9e427332cf8bf95a52cd8689e2b17c732593b69a3f7f062c03dd93ab92449b`。
- 两张表总金额相等，并归档独立 `origin_state_digest`、日志及本次工具源码身份。

上述两个 digest 来自 P4 已通过的相同投影；主网 `origin_state_digest` 尚未计算，不预填预期值；它不替代已知的旧 C(963800)。
工具使用 RocksDB 同一个一致读视图检查高度/hash并扫描双表，仍要求停写以明确实验边界。
它检查的是已索引状态，不独立验证 canonical chainwork，也不宣布 Core 后台验证或整套服务已就绪。

## 7. 自动验收

当前检查点方案的测试、真实进程证据和主网长任务步骤统一记录于
[P6.2 操作文档](./balance-history-assumeutxo-p62-operations.md#3-本批验证证据)。
P6.3 的稀疏文件、双文件追加、RPC 回退和真实进程渐进启动结果见[P6.3 验收](./balance-history-assumeutxo-p63-operations.md#3-验证结果)。

核心验收包括：从 0 重放与 B=101/B=102 两份真实 Core 快照在同一 G 的状态、完整 commit 记录一致；
G=B 边界、正常接块、重启与重组、未知或被覆盖检查点的拒绝、旧实验 v2 元数据拒绝、独立摘要和 rolling 编码向量、
以及下游 pass/local/system 身份一致。主网原生长任务独立安排，不能用小规模结果代替主网性能记录。
