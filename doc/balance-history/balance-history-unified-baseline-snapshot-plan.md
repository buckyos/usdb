# Balance-history 精简合并基线快照计划

## 1. 目标与边界

Bitcoin 继续使用官方 AssumeUTXO，BH 可选择恢复业务 genesis G 的合并快照，
省去每台节点导入 B 的 UTXO、重放 B+1..G 的工作。无 BH 快照时继续使用原生 bootstrap。
现阶段固定快照高度等于业务 genesis；不把 H>G 的当前余额快照当作能恢复 G..H 业务历史的快照。

允许两种制作来源：从 0 全量重放的精确高度数据库，以及 AssumeUTXO 导入重放后封存的精确高度数据库。
两者使用相同规范化导出规则。旧 core＋registry 也可以通过一次性转换成为来源。
业务状态、精简 registry、查询下界和 C(G) 应相同；来源记录、生成时间和文件 SHA-256 不要求相同。

保留现有 signer、snapshot-keys、对象存储、可信公钥分发与 installer → setup → doctor → up。
不改变 rolling commit 算法，不伪造来源，不把全量重放库标成经过 AssumeUTXO 原始文件验证的库。
本计划不修改或重启 node1 当前同步任务，不自动提交、发布或执行主网大文件转换。

## 2. 统一逻辑内容

G 当前为 963800；B 当前为 935000。高度与对应 BTC hash 均作为显式身份。

| 表 | 规范化规则 |
| --- | --- |
| balances | G 时每个 script 的非零余额；高度隐含为 G，历史 delta 不进入断面 |
| utxos | G 时全部活 outpoint/script_hash/value，保留零金额输出 |
| block_commits | G 的完整原 v1 记录：BTC hash、delta root、C(G)；不重新计算 commit 种子 |
| script_registry | 活 UTXO 引用的所有脚本，加 G 区块全部输出脚本；按 script hash 去重 |
| genesis_block | G 的原始区块，校验 hash、Merkle root 和 witness commitment，用于独立验证 registry 边界 |
| meta | 版本、网络、G/hash、数据模型、commit 协议、查询下界、表摘要与 registry 覆盖规则 |

G 区块输出补充包括块内已经花掉的输出和不可花费输出；不能只按余额>0筛 registry。
一旦开始处理 G+1 以后区块，继续追加脚本映射，不因余额归零而删除。
范围外脚本的 miss 必须是 unresolved，不能宣称全历史不存在。
余额查询下界为 G；精确 delta/history 下界为 G+1；G 行是断面，不是真实收支事件。

文件采用独立版本的单 SQLite DB＋一份 manifest＋一份签名。旧 core v1 的
registry_included=false 契约保持原意，禁止往旧格式直接加表后继续使用其签名域。
表摘要采用显式排序和编码，来源信息不参与逻辑摘要；文件哈希和签名覆盖具体产物及来源。

## 3. 实施阶段

| 阶段 | 工作 | 验收 |
| --- | --- | --- |
| P8.1 | 统一逻辑契约；只读精确高度 RocksDB 导出；单文件校验与签名；两种来源对拍 | 同一 G 下逻辑内容一致；零金额、老地址、块内花费、错误余额/脚本和不完整来源测试；现有正式入口不自动切换 |
| P8.2 | 接入 snapshot-tool 的 create/resume/verify/finalize；旧 core＋registry 转换；复用 wrapper 和发布 record | 同一私钥、对象存储和命令流程；中断恢复；旧格式转换输入验签；文件/manifest/签名身份绑定 |
| P8.3 | staging 安装、来源元数据、查询/回滚 floors、完整状态校验和正常续块 | 两种来源恢复后 G/G+1 查询与 commit 相同；损坏、中断、重装、重组边界测试 |
| P8.4 | 配置/打包/installer/setup/doctor/up/controller 接入 | Core AssumeUTXO 与 BH 快照为独立配置；匹配快照恢复，无快照原生重放；校验失败明确停止；进度不混淆 |
| P8.5 | 完整服务验收与主网制作/容量测试 | G→tip BH/indexer/chain 对拍；记录制作、下载、安装、磁盘峰值与时长；整理操作步骤供安排长任务 |

当前批次实现 P8.1，验收结果记录于第 7 节；P8.2–P8.5 尚未接入。后续阶段不得仅因 exporter 通过便标为完成。
尤其单文件可验证不表示现有生产 installer 已能接受新格式。

## 4. 校验与性能要求

- 制作端校验余额与 UTXO 按 script 聚合逐项一致；不以总金额相同代替。
- 每条 registry 必须满足 hash(scriptPubKey)=script_hash；集合必须恰好覆盖规定范围。
- 逻辑摘要绑定五个内容表及 G/hash/协议/覆盖范围；G 的原 commit 与独立状态摘要分开。
- 制作端从一致的只读视图读取状态。拒绝非精确高度、回滚中、未封存原生库和未知数据身份。
- 限制批次和缓存，临时排序/去重可落盘；不能把主网全部 UTXO 或 registry 收进内存。
- 日志记录阶段、累计行数和耗时；最终验证通过前，临时输出不能被识别为完整发布物。
- 安装继续验签、全文件哈希、SQLite 完整性、schema/身份、状态一致性及 floors；
  S3 元数据或公开 Range 可用性检查不替代这些校验。
- 私钥不进入 DB、manifest、node-kit 或日志；新格式复用旧密钥但使用独立签名域。

## 5. 后续主网操作

先用小规模离线/隔离测试通过各阶段。主网制作需独立输出目录及明确的精确高度来源，
不回退或覆盖正在运行的服务数据库。不自动重新扫描 196 GB registry 或导出几十 GB 文件。
优先评估已有 963800 core＋registry 的一次性转换；其输入哈希和签名必须重新核对。
新原生节点已经越过 G 时，不可把当前 UTXO 冒充成 G 的 UTXO；需使用精确 G 的来源或单独制作。

## 6. P8.1 离线命令与摘要编码

本批在现有 `balance-history-snapshot-tool` 增加 `export-baseline` 和 `verify-baseline`。
这是供后续编排复用的底层入口；尚未由 `SNAPSHOT_SCRIPT create/finalize/publish` 调用，
也尚不能通过当前生产 installer 安装。命令帮助可先独立查看：

```bash
cargo build --manifest-path src/btc/Cargo.toml -p balance-history-snapshot-tool
BASELINE_TOOL=/home/bucky/work/usdb/src/btc/target/debug/balance-history-snapshot-tool
"$BASELINE_TOOL" export-baseline --help
"$BASELINE_TOOL" verify-baseline --help
```

制作前准备：

1. 来源为停止写入的独立 BH 根目录，包含 `db/balance_history`，已精确同步到 G。
   原生来源还必须已在同一 G/hash 封存。工具以只读 RocksDB 视图导出，并检查来源文件集合是否发生变化。
   若现有服务已经超过 G，另行准备来源；本工具不回退服务、不裁剪原数据库。
2. 提供 G 的原始 Bitcoin 区块二进制文件（包含 witness，不能直接使用 RPC 返回的十六进制文本）。
   文件与显式传入的 G/hash、数据库中 G 的 commit 必须匹配。
3. 使用现有 BH snapshot 私钥 JSON 和可信公钥目录 JSON；无需生成新密钥。
4. 输出目录必须尚不存在，父目录已存在，且与来源互不包含。为 SQLite 临时聚合/排序预留额外磁盘，
   可在启动前设置 `SQLITE_TMPDIR` 到空间充足的独立临时目录；64 MiB 页缓存并不是进程总内存上限。

下面是参数模板，路径变量需由操作者填入；不代表已安排主网导出：

```bash
"$BASELINE_TOOL" --root-dir "$BASELINE_LOG_ROOT" --json export-baseline \
  --source-root "$OFFLINE_BH_ROOT" \
  --network bitcoin --height 963800 \
  --expected-block-hash 000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551 \
  --genesis-block "$GENESIS_BLOCK_FILE" \
  --output-dir "$BASELINE_OUTPUT" \
  --signing-key "$SNAPSHOT_SIGNING_KEY" \
  --trusted-keys "$SNAPSHOT_TRUSTED_KEYS"

"$BASELINE_TOOL" --root-dir "$BASELINE_LOG_ROOT" --json verify-baseline \
  --manifest "$BASELINE_OUTPUT/balance_history_baseline_963800.manifest.json" \
  --trusted-keys "$SNAPSHOT_TRUSTED_KEYS"
```

成功输出一个目录，内含 `balance_history_baseline_963800.db`、
`balance_history_baseline_963800.manifest.json` 和对应 `.manifest.sig`。
仅在全文件、签名和语义自检通过后公布最终目录。失败会保留输出父目录内的 `.baseline-*`
临时目录供诊断，不作为完成产物；P8.1 不支持断点续导，续导能力由 P8.2 补齐。
单独 `verify-baseline` 验证签名中的身份与内容；部署方仍需把该身份与网络 bundle 的预期 G/hash 比对。

逻辑编码固定如下，所有金额单位为 satoshi：

| 表 | 排序 | 每行参与 SHA-256 的字节 |
| --- | --- | --- |
| balances | script_hash 原始字节升序 | hash[32] + balance u64 BE |
| utxos | outpoint 原始字节升序 | outpoint[36] + script_hash[32] + value u64 BE |
| block_commits | height 升序 | height u32 BE + block_hash[32] + delta_root[32] + C(G)[32] |
| script_registry | script_hash 原始字节升序 | hash[32] + script 长度 u64 BE + 原始 script |
| genesis_block | 唯一 id=1 | block 长度 u64 BE + 原始 block |

outpoint 使用现有 `OutPointCodec`：txid 原始 32 字节＋vout u32 BE；BTC hash/txid 使用库的原始字节顺序，
不是显示用十六进制字符串。各表保存 `{rows, sha256}`；表名按字典序排列。
整体 `logical_sha256` 为下列字节串的 SHA-256：

```text
u32_BE(domain.len) || "balance-history-baseline-state:v1" ||
u64_BE(json.len) || compact_UTF8_JSON(BaselineState)
```

JSON 使用 `BaselineState` 字段顺序 `identity,tables`；identity 依次为
`network,height,block_hash,data_model_version,commit_protocol_version,registry_policy,balance_query_floor,history_query_floor`；
各表摘要字段顺序为 `rows,sha256`，无多余空白。独立 Python 检查器位于
`tests/common/baseline_snapshot_golden.py`，用于核对跨实现编码一致性。
manifest 使用独立签名域 `usdb.balance-history.baseline-manifest-signature:v1`，
其签名覆盖完整 manifest，包括具体文件 SHA-256、来源和生成时间。
验证器保证产物内部一致且由可信制作者签署；原 C(G) 的历史正确性沿用已验收来源/可信检查点，
不会把断面余额重新计算的摘要当作 rolling commit 的证明。

## 7. P8.1 验收记录

使用仓库内真实 Core 生成的 regtest 夹具，分别从 0 全量重放和 B=101 原生导入重放至 G=103：

- 五个内容表、查询下界、原 C(G) 和整体逻辑摘要一致；来源信息如实保留差异。
- 独立 Python 实现重新计算全部表摘要和整体摘要，与 Rust 一致。
- 零金额 UTXO 保留；G 内创建后花掉的脚本仍进入 registry；无关历史映射被排除。
- 可信密钥重新签署后的错误余额（含总金额不变）、缺失/错误/多余映射、错误 commit 高度和额外 schema 均被拒绝。
- manifest 被修改而未重新签名、未知签名者、额外 SQLite WAL/journal、错误来源高度/hash、未封存原生来源和已有输出目录均被拒绝。

本批回归：BH 库测试 211 通过、1 项需要本地主网旧快照的测试按原配置跳过；
snapshot-tool 库和 CLI 单测共 25 通过。新增 3 项测试覆盖上述真实夹具及损坏矩阵。
fixture 的断面为 4 行非零余额、406 个活 UTXO、9 条 registry、1 条 commit 和 1 个原始 G 区块；
两种来源的 `logical_sha256` 均为
`2040d80f844736e55d7a6d71683c72d9c08c701750caef7a94a142c5b7b2a960`。
`cargo fmt --check`、相关包 `cargo clippy --all-targets -- -D warnings`、API 文档构建、
release fragment 校验、新命令帮助和实际校验失败退出路径均通过。

主网大小、耗时、临时排序空间和恢复后的服务表现尚未验证；这些是 P8.3/P8.5 的验收内容。
