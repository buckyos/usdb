# P8.2：合并 BH 基线快照制作与发布

本阶段将单文件基线快照接入现有 `SNAPSHOT_SCRIPT create → finalize → publish`。
显式选择 `--snapshot-type baseline`；原 `balance-history` 和 `assumeutxo` 分支继续保留。
新产物包含 G 余额、全部活 UTXO、原 C(G)、精简 registry 和原始 G 区块，详见
[逻辑契约与阶段计划](balance-history-unified-baseline-snapshot-plan.md)。

当前只能制作高度等于业务 genesis 的快照。发布成功不表示当前 installer 已支持安装；
安装/续块属于 P8.3，bundle 和 controller 接入属于 P8.4。本手册中的路径变量是操作模板，
本次开发没有转换主网大文件、上传真实对象或改动 node1。

## 1. 共用准备

更新源码并构建 snapshot-tool。复用原有 `snapshot-keys`、签名者及 S3/R2 上传配置，无需生成新密钥。
下面选用独立的制作目录；已有 UTXO 发布目录也可复用，新分支存放于独立 `baseline` 子目录。

```bash
cd /home/bucky/work/usdb
cargo build --release --manifest-path src/btc/Cargo.toml -p balance-history-snapshot-tool
export SNAPSHOT_SCRIPT=/home/bucky/work/usdb/src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh
export SNAPSHOT_ROOT=/data/btc/bh-baseline-release
export SNAPSHOT_KEY_ROOT=/home/bucky/.usdb/secure/snapshot-keys
export SNAPSHOT_SIGNER_ID=usdb-mainnet-snapshot-v1
export SNAPSHOT_TOOL_BIN=/home/bucky/work/usdb/src/btc/target/release/balance-history-snapshot-tool
G=963800
G_HASH=000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551

bash "$SNAPSHOT_SCRIPT" paths --snapshot-type baseline --height "$G" --block-hash "$G_HASH"
```

`SNAPSHOT_KEY_ROOT` 应设置为现有密钥的真实目录；读取文件名为
`${SNAPSHOT_SIGNER_ID}.signing-key.json` 和 `${SNAPSHOT_SIGNER_ID}.trusted-keys.json`。
转换旧文件时，同一可信公钥目录也必须信任旧 core 和 registry 的签名者。
目录内容在任务启动后冻结，重试时不能替换密钥、可信目录或来源文件。

默认沿用 `SNAPSHOT_S3_BUCKET`、`SNAPSHOT_S3_ENDPOINT_URL`、`SNAPSHOT_PUBLIC_BASE_URL`、
`SNAPSHOT_AWS_PROFILE`、`SNAPSHOT_S3_UPLOAD_CONCURRENCY` 和 `SNAPSHOT_S3_CHUNK_SIZE_MIB`。
`baseline` 分支不自动启动 bitcoind，不初始化旧 builder，也不自动生成私钥。

来源与输出目录应分开，并预留输出 DB 及 SQLite 临时聚合/排序空间。
可以设置 `SQLITE_TMPDIR` 到空间充足的现有目录；输入旧 registry 仍须全文件哈希扫描，
但不需要恢复完整 RocksDB，也不把整个 registry 加载到内存。主网磁盘峰值尚未实测。

## 2. 三种 create 来源，选一种

### A. 转换已有签名 core＋registry（已有 G 旧快照时优先）

准备原始两份 manifest；对应 DB 和 `.manifest.sig` 必须位于各自 manifest 的同目录。
工具校验两份签名、全文件哈希、schema 和同一 core/G/hash 身份，再按新规则裁剪合并。
不会改写旧文件。不能只提供 core，也不能使用其他高度的 registry 补配。

另需 G 原始区块二进制，用于覆盖 G 内创建后花掉和不可花费的输出脚本。
若从 RPC 获取，可使用以下模板；`BTC_CLI` 和 `BTC_DATA_DIR` 指向操作者已有节点：

```bash
set -o pipefail
"$BTC_CLI" -datadir="$BTC_DATA_DIR" getblock "$G_HASH" 0 \
  | python3 -c 'import sys; sys.stdout.buffer.write(bytes.fromhex(sys.stdin.read().strip()))' \
  > "$GENESIS_BLOCK_FILE"

bash "$SNAPSHOT_SCRIPT" create --snapshot-type baseline --height "$G" --block-hash "$G_HASH" \
  --core-manifest "$OLD_CORE_MANIFEST" --registry-manifest "$OLD_REGISTRY_MANIFEST" \
  --genesis-block "$GENESIS_BLOCK_FILE" --batch-size 20000
```

这里的 `GENESIS_BLOCK_FILE`、`OLD_CORE_MANIFEST`、`OLD_REGISTRY_MANIFEST` 需先设置为真实绝对路径。
区块文件必须是二进制，不能直接保存 RPC 的十六进制文本。

### B. 导出现成的精确 G RocksDB

`OFFLINE_BH_ROOT` 是包含 `db/balance_history` 的独立 BH 根目录，必须停止写入、精确到 G。
支持从 0 全量重放库和已在 G/hash 封存的原生 AssumeUTXO 库；不接受尚未封存或已超过 G 的数据库。

```bash
bash "$SNAPSHOT_SCRIPT" create --snapshot-type baseline --height "$G" --block-hash "$G_HASH" \
  --source-root "$OFFLINE_BH_ROOT" --genesis-block "$GENESIS_BLOCK_FILE"
```

工具以只读方式分页读取，记录来源文件指纹并在续导/收尾复核。
导出期间不要启动该来源的 BH 服务、执行 compaction 或移动其文件。

### C. 在工具管理的独立 workspace 中同步到 G，再导出

提供完整 BH TOML 配置；BTC RPC、认证文件、BTC 数据目录、原生 snapshot 等路径使用绝对路径。
配置不含 `[bootstrap]` 时，从 0 同步；含原有原生 `[bootstrap]` 时，从指定 AssumeUTXO 断面导入重放。
两种模式均复用 BH 的同步逻辑和可用的本地区块加速机制。
配置中的 network 必须匹配命令，原生配置的 origin 必须匹配 G/hash。

```bash
bash "$SNAPSHOT_SCRIPT" create --snapshot-type baseline --height "$G" --block-hash "$G_HASH" \
  --config "$BASELINE_BUILDER_CONFIG"
```

工具将 `root_dir` 改为自身管理的独立 workspace，将同步上限固定为 G；即使 BTC 已追到更高，
也只制作 G。原配置文件和现有服务目录不会被覆盖。配置冻结在 workspace 内，可能含 RPC 认证信息，
仅供本地恢复，不进入发布清单。该模式不需要 `--genesis-block`，同步完成后会自行获取并校验 G 区块。

首次同步到 G 前发生中断，重复原 `create --config ...`，复用已有同步状态。
此时尚无 baseline 导出 job，`status` 不提供同步阶段进度，使用制作日志查看；job 创建后可用下一节命令。
不要手动清空 workspace，也不要用另一个配置覆盖已冻结任务。

## 3. 状态与中断恢复

```bash
bash "$SNAPSHOT_SCRIPT" status --snapshot-type baseline --height "$G" --block-hash "$G_HASH"
bash "$SNAPSHOT_SCRIPT" resume-verify --snapshot-type baseline --height "$G" --block-hash "$G_HASH"
```

`resume` 与 `resume-verify` 在本分支等价：继续导出，并完成验签和语义校验。
无需重复来源参数；来源仍在原路径时也可重复原 `create`，但来源和身份必须保持一致。
批大小可调整为 1..20000，默认 20000。单个 job 同时只允许一个写入者。

| 持久阶段 | 恢复行为 |
| --- | --- |
| prepared/exporting | 重新认证来源；保留已提交 UTXO、余额、registry 批次，从持久游标继续 |
| verifying | 导出数据已完整，不再读取旧来源；重新执行聚合、文件哈希和语义验证 |
| complete | 重新验证冻结的完整 manifest 和文件；不改写完成产物 |

批次数据与游标在同一 SQLite 事务中提交；崩溃不会跳过未落盘行或重复插入已提交行。
`checkpoint` 中的数量是该阶段已消费的输入行数，余额历史裁剪时不等于最终余额行数。
文件扫描输出累计字节和耗时，语义扫描输出阶段/行数；没有可靠总量时不伪造百分比或 ETA。
文件哈希、临时排序和语义扫描本身没有保存扫描游标，中断后重新扫描；旧输入哈希在数据阶段重试时也会重算。
只有持久状态已到 `verifying` 或 `complete` 后，后续恢复才不再依赖旧来源。
旧来源已迁移或归档时使用 `resume-verify`，不要重新传入已不存在的来源路径。

## 4. finalize 与 publish

```bash
bash "$SNAPSHOT_SCRIPT" verify --snapshot-type baseline --height "$G" --block-hash "$G_HASH"
bash "$SNAPSHOT_SCRIPT" finalize --snapshot-type baseline --height "$G" --block-hash "$G_HASH"
bash "$SNAPSHOT_SCRIPT" publish --snapshot-type baseline --height "$G" --block-hash "$G_HASH"
```

`create` 已完成签名和自检；单独 `verify` 供人工复核，可省略。
`finalize` 再验证签名、文件哈希、SQLite 完整性及逻辑内容，冻结来源身份、工具/脚本摘要和文件清单。
本阶段不恢复 RocksDB；安装后的服务验收仍属于 P8.3/P8.5。
`finalize` 可重复执行并保留首次封存记录。封存和发布只需可信公钥文件，私钥文件可以离线保存。

`publish` 要求已 finalize，会自动生成 release record。
若需先审阅公开文件清单，可在发布前运行同参数的 `prepare-release`。
上传对象固定为 DB、manifest、签名和可信公钥目录；工作游标、生产配置、来源路径与私钥不上传。
公钥目录用于分发，安装端仍需受信的预期公钥/目录摘要，不能把下载同目录的公钥直接当作信任根。

每个上传对象都完成匿名公开下载的全文件长度和 SHA-256 校验后，才上传内容寻址的 record。
遇到 403、网络失败等问题，修复公开访问后重复 `publish`：已完成且元数据一致的 S3 对象可复用，
但仍重新检查公开内容；同名对象不一致则拒绝覆盖。未完成的单次 multipart 上传由原 AWS 上传机制处理，
不承诺跨进程保留其分片游标。全部成功才写 `publish-result.json`。

```bash
bash "$SNAPSHOT_SCRIPT" verify-published --snapshot-type baseline --height "$G" --block-hash "$G_HASH"
```

默认目录布局：

```text
$SNAPSHOT_ROOT/builder/baseline/workspaces/<G:012>-<hash>/  # 仅管理式同步来源
$SNAPSHOT_ROOT/builder/baseline/jobs/<G:012>-<hash>/
  job.json / genesis.block / staging/ / artifact/
$SNAPSHOT_ROOT/releases/baseline/<G:012>-<hash>/
  artifact-finalized.json / snapshot.trusted-keys.json / records/ / publish-result.json
```

`artifact/` 中为 `balance_history_baseline_G.db`、`.manifest.json` 和 `.manifest.sig`。
record 使用 `usdb-baseline-snapshot-release-record:v1`，公开对象前缀为
`balance-history/baseline/<network>/<G>/<DB-SHA256>/`；record 位于
`snapshot-records/baseline/v1/<record-SHA256>.json`。它与旧 split record 分开，避免旧 installer 误识别。
