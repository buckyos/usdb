# AssumeUTXO P6：生产 bootstrap 设计与分步实施

## 1. 决策与当前边界

日期：2026-09-12。P4/P5 验证批次已提交为 `ce94958`；其结果继续作为历史证据。
本设计落实随后确认的新方向，不再要求新网络状态兼容旧 balance-history commit。

- Bitcoin 导入基线 `B=935000`；USDB 业务起点 `G=963800`，表示应用该块后的状态。
- 新生产 bootstrap 取消对历史 **core snapshot、script-registry snapshot、旧 USDB commit 小锚点**的依赖。
  Bitcoin 原始 UTXO 快照及其身份校验仍是输入；balance-history 仍需导入、聚合余额并重放 `B+1..=G`。
- 只需完整保留 B 时仍有活 UTXO 的脚本，以及 B 之后观察到的脚本。余额后来归零时，保留已建立的反向映射。
  金额为零的活 UTXO 也保留；不能以“当前余额为零”作为删除 UTXO 或 registry 的规则。
- **保留 LocalLoader**，单列 P6.3 适配，不随旧快照机制删除。
- 节点采用 `prune=0`，继续运行 Bitcoin Core 官方后台历史验证；快速启动依赖所需前台数据可用，后台进度单独展示。

截至本批次，已实现 P6.1 的候选起点承诺编码、只读计算入口及小规模验收。
生产初始化、RPC/网络版本激活、LocalLoader、indexer 历史交易查询和部署流程仍待接入。
现有 P4 `import/replay` 继续按旧验证协议使用 reference core；不能把新增计算入口等同于已经移除生产依赖。

## 2. P6 拆分

| 步骤 | 范围 | 验收边界 | 状态 |
| --- | --- | --- | --- |
| P6.1 | 定义 G 高度状态身份及新初始 commit，提供只读计算器 | 全量重放与导入重放结果相同；不读取旧快照，不依赖旧 commit；独立编码向量一致 | 本批次实现，小规模测试通过；主网新承诺扫描待安排 |
| P6.2 | 原生 bootstrap 生命周期与正式版本接入 | 无旧 snapshot/小锚点的新库可导入、重放、封存 G 并正常接块；中断恢复、查询下界、回滚和下游身份一致 | 下一实现步骤 |
| P6.3 | LocalLoader 独立适配 | 后台尚未补齐 B 以前区块时仍可加速；支持两个追加文件、乱序、部分尾部、XOR、重组与重启 | 待实现 |
| P6.4 | indexer 历史 prevout/reveal 查询与 readiness | 不依赖全量 txindex 追平；落后消费者和历史已花费输出仍正确；不把 Core 前台可用等同于整套就绪 | 待设计与实现 |
| P6.5 | 整套端到端验收与运维步骤 | 独立 regtest、主网相同锚点复核、冷启动/恢复/资源证据完整 | 待安排 |

P7 再完成 Bitcoin 31.1 镜像、node-kit、安装/升级与发布身份集成。原数据目录升级 P3 独立安排。

## 3. 为什么初始承诺固定在业务起点 G

AssumeUTXO 文件是 B 高度的 Coin 集合；它没有旧余额历史或 USDB rolling commit。
P4/P5 使用旧 935000 小锚点证明了兼容旧算法的重放正确性，这不意味着新流程必须继续携带该锚点。

新流程在 G 计算完整逻辑状态的承诺，作为后续 rolling commit 的起点。同一 Bitcoin 网络、G/hash、
数据模型及当前逻辑状态，应得到同一个结果，无论数据库来自完整重放还是较早的受支持 UTXO 快照。
不将导入高度 B、下载文件 SHA-256、RocksDB 文件布局、旧 registry 总量、历史 delta 或旧 commit 写入该承诺。
它们作为来源及诊断信息另行保留。

候选身份包含：

| 字段 | 含义 |
| --- | --- |
| `schema_version` | `balance-history-bootstrap-origin:v1`，起点身份编码域 |
| `commit_protocol_version` | `2.0.0`，候选新 commit 版本；本批次不激活 |
| `network` | Bitcoin 网络，二进制编码使用该网络 genesis hash |
| `origin_height` / `origin_block_hash` | G 及该高度固定 canonical BTC hash |
| `data_model_version` | `balance-history-data-model:bip30-generations-core-unspendable-v2` |
| `utxos` | 所有活 outpoint 的行数、总金额及规范投影 SHA-256 |
| `balances` | 每个 script 最新非零余额的行数、总金额及规范投影 SHA-256 |

B 必须不晚于 G。未来只有高于 G 的 UTXO 快照时，不能从中恢复 G 已被花费的输出或 G 后完整历史。
若要使用这种新快照，需要另行定义可信业务 checkpoint 和历史保留契约，不能只替换 B 配置。

### 3.1 规范二进制编码

所有整数均为无符号大端；所有 SHA-256 为单次 SHA-256，输出小写 64 位 hex。
Bitcoin txid/block hash 使用内部 32 字节顺序，与 RPC 常见的显示 hex 相反；script hash 使用数据库原始 32 字节。
JSON 仅用于展示，JSON 字段顺序、空白和字符串格式不参与计算。

1. UTXO 按 `txid[32] || vout:u32` 的字节序降序排列。
   每行编码为 `txid[32] || vout:u32 || script_hash[32] || value:u64`，共 76 字节。
   对全部行连接计算 SHA-256；包括 value=0 的输出。
2. 余额对每个 script 选择 G 时最新一行，忽略旧行的高度与 delta，排除最新余额为零的 script。
   按原始 script hash 降序排列，每行 `script_hash[32] || balance:u64`，共 40 字节。
   对全部行连接计算 SHA-256。
3. 起点 preimage 依次连接下列字段后计算 SHA-256：

   ```text
   len:u32 || UTF8(schema_version)
   len:u32 || UTF8(commit_protocol_version)
   len:u32 || UTF8(data_model_version)
   network_genesis_hash[32]
   origin_height:u32
   origin_block_hash[32]
   utxos.rows:u64 || utxos.total_sats:u64 || utxos.sha256[32]
   balances.rows:u64 || balances.total_sats:u64 || balances.sha256[32]
   ```

总金额均为 satoshi。UTXO 与余额总金额必须相等；这个检查不能替代逐 script 的余额聚合正确性验证。
独立 Python 编码器及 Rust 数据库投影测试共同固定字节序、零金额行为和字段顺序：
[编码器](../../tests/common/bootstrap_origin_golden.py)、[固定向量](../../tests/fixtures/bootstrap-origin-v1.json)。
regtest 合成向量 commit 为 `e2bb3760e24e5ef768ff6831b66882eff230adbebe754dd2dfcae6bddcb4fbd8`，不是主网结果。

### 3.2 与正式 commit/查询契约的关系

G 的新 commit 表示完整起点状态，不伪造 G 当块收入或 delta。后续块从该种子滚动，
具体 v2 rolling 编码、网络激活及黄金向量在 P6.2 接入时一并固定。
本批次的 `2.0.0` 只出现在候选计算结果中，不修改 embedded network 或运行中 RPC。

按 G 压缩生产基线时，拟定余额查询从 G 开始，精确变化历史从 G+1 开始；
若选择额外保留 G 当块真实 delta，则须显式定义另一历史保留边界，不能把初始余额当作收入。
P4 实验库现有的 B/B+1 查询下界不在本批次改写。公开可查询高度继续遵循冻结的 stable lag。

P6.2 需要同步变更数据库 bootstrap 身份、provenance、RPC state-ref、commit version、snapshot_id 的身份语义、
indexer pass/local/system state 以及网络配置的版本选择。旧库须显式识别和迁移到独立新状态，不能静默混用 v1/v2。
P5 的旧 commit 等价证据继续成立，但不自动覆盖新版本下游结果。

## 4. 原生 bootstrap 生命周期（P6.2）

```text
核验 Bitcoin 快照身份 / Core 逻辑承诺
  -> staging 导入 B 的 UTXO、余额与 live scripts
  -> 按 canonical 区块重放 B+1..G，补充 registry
  -> 校验并封存 G 状态，写入新起点 commit / 查询下界 / 来源
  -> 原子发布为可用数据库
  -> 正常同步 G+1..tip，应用新 rolling commit 与重组规则
```

实施要求：

- 新入口不接受必填旧 core、registry sidecar 或旧 USDB anchor；旧 P4 工具保留为验收工具。
- 在 staging 的导入/重放阶段持续记录进度和可恢复状态；半成品不能被正常服务当成已就绪库。
- G 之前允许内部重放元数据，但不对外发布旧 commit 语义。封存应将状态、种子、版本和 floors 原子关联。
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

当前本地实现的两个待改假设：

- `generate_sort_blocks()` 从全零 prev_hash/height=0 开始连链。B 以前区块尚未完整到达时，不能据此发现 B 以后的连续区间。
- `file_indexer` 只排除最大编号文件，将其余文件视为完成；不能覆盖两个 cursor 的追加行为。

适配步骤与验收：

1. 以已核验 B/hash 或恢复点的 canonical hash 开始，建立需要消费的 height/hash 序列；不要等待创世到 B 的连续本地索引。
2. 物理层记录 `block_hash -> (file, offset, length)`，逻辑层按 canonical height/hash 选块，验证块内容与父 hash。
3. 每个文件独立保存已完成记录的偏移，重扫所有可能增长的文件，正确处理预分配零尾、未写完记录与 XOR。
   仅凭连续两次文件大小相同也不能永久认定文件已封存。
4. 覆盖乱序下载、重复记录、旧分叉、重启及双 cursor 轮换；重组后重新核对 canonical 映射和恢复点。
5. 本地缺块或尾部尚未完成时显式回退 RPC，并继续增量索引；不能默默回到“等历史补齐”。
6. 对同一连续区间比较 RPC 与 LocalLoader 的块 hash、UTXO/余额及 v2 commits，再测实际加速收益。

该步骤保留 LocalLoader 的加速职责；txindex 和 indexer 历史交易定位属于 P6.4，不能用文件扫描通过代替其验收。

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
"$P6_TOOL" origin-commit \
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
- 两张表总金额相等，并归档新的候选 `origin_commit`、日志及本次工具源码身份。

上述两个 digest 来自 P4 已通过的相同投影；新主网 `origin_commit` 尚未计算，不预填预期值。
工具使用 RocksDB 同一个一致读视图检查高度/hash并扫描双表，仍要求停写以明确实验边界。
它检查的是已索引状态，不独立验证 canonical chainwork，也不宣布 Core 后台验证或整套服务已就绪。

## 7. 本批次自动验收

2026-09-12 本地检查结果：

| 检查 | 结果 |
| --- | --- |
| `cargo test -p balance-history --lib origin_commit` | 3 passed |
| `cargo test -p balance-history --lib assumeutxo::` | 12 passed；与上一过滤器重叠2项，共覆盖13项测试 |
| Python 重新生成固定向量 | 与提交候选 fixture 字节完全一致 |
| workspace `cargo fmt --all -- --check` | 通过 |
| workspace `cargo clippy --all-targets --all-features -- -D warnings` | 通过，8.354秒 |
| `cargo doc -p balance-history --no-deps` | 通过，2.529秒 |
| release 工具构建 | 通过，10.496秒 |
| CLI 顶层及 `origin-commit --help` | 通过 |
| CLI 指向不存在的实验状态目录 | 正确失败，不创建目录，stdout 不产生伪成功 JSON |

Cargo 检查使用 `--offline --locked --manifest-path src/btc/Cargo.toml`；测试使用临时数据库和固定 regtest 区块，未启动或写入主网服务。
静态检查、构建与 CLI 日志位于本机 `/tmp/usdb-assumeutxo-p6-checks/`，该路径是临时运行证据，不属于发布产物。

覆盖的行为：

- 独立 Python 编码器与 Rust 哈希实现匹配。
- 真实 regtest 区块完整重放与 AssumeUTXO 导入重放在同一 G 得到相同 identity/commit。
  删除临时测试中的 reference core 和 P4 input.json 后，仍可独立计算。
- 改变旧 commit、旧 delta root、历史 delta、零余额记录及附加 registry 映射，结果不变。
- 合成数据库的投影匹配 Python 向量，包含零金额活 UTXO；真实余额/UTXO 变化使 commit 改变。
- 错网络、错高度/hash、未对齐的双表总金额和超出目标高度的历史行明确失败。

实现：[起点身份与编码](../../src/btc/balance-history/src/bootstrap.rs)、
[一致读视图扫描](../../src/btc/balance-history/src/db/bootstrap.rs)、
[跨模块验收](../../tests/assumeutxo_origin.rs)。
