# P6.2 原生 bootstrap：实现、验证与主网操作

日期：2026-09-12。本批采用“可信旧 commit 检查点 + 原生导入重放”，保持与从 0 重放一致。
P6.1 的独立状态扫描保留为校验工具；此前实验性的 v2 初始种子不再用于原生启动。
P6.2 批次提交为 `a897975`。本手册的操作步骤已同步 P6.3 的并行启动与按需本地读取行为；第3节保留 P6.2 原始验收记录。

## 1. 已实现的路径

配置 `[bootstrap]` 后，正常 `balance-history` 启动会在打开业务 RPC 之前执行：

```text
校验配置、BTC 网络、B 固定 hash；G 可用时核验其固定 hash
  -> bootstrap-staging 中导入 Core UTXO 文件
  -> 安装内置 C(B)/D(B)，随 Core 稳定区块到达逐段重放 B+1..G
  -> 计算 G 的规范 UTXO/余额投影
  -> 从持久化 UTXO 独立重新聚合，逐 script 校验余额
  -> 原子封存 G 的状态校验结果、原 v1 commit、来源和查询/回滚下界
  -> 将私有 RocksDB 目录原子发布到服务 db/balance_history
  -> 启动正常 indexer/RPC，按原 commit v1 接块
```

也可先运行 `balance-history --root-dir <目录> bootstrap`，只完成初始化并退出，便于单独安排长任务。

- 无需旧 core SQLite、script-registry sidecar 或 P4 input.json；已审核的旧 commit 检查点随代码内置，无需另行下载。
- 业务 genesis G 必须满足 G>=B；当前手册用963800作实验起点，正式网可选择更晚高度及对应 canonical BTC hash。
- 导入阶段的 UTXO、余额、registry 和进度一起写入同步 WAL；完整文件 hash 与 Core 逻辑承诺均通过后才进入重放。
- 重启可更换批大小。导入重试会重扫输入，并核对已写 UTXO 前缀，不能直接跳过先前损坏输入留下的记录。
- 封存后的正常重启不再读取输入快照，不重复导入、重放或执行全表校验；数据库中的来源记录继续保留。
- 半成品不发布；普通 indexer 和 RPC 也检查 native 身份/封存状态。旧库不能通过打开 bootstrap 配置原地转换。
- 原生 RocksDB 使用独立 schema 身份，识别其导入历史/恢复边界；旧 v2 实验库和本轮 v1 检查点库不能混用，不执行自动转换。
- 余额查询从 G 开始，精确变化历史从 G+1 开始。G 以前内部重放的余额行暂时保留，但不作为公开历史；G 以前的内部 commit 和截至 G 的 undo 在封存时删除。
- 所有活 UTXO（包括零金额）和已观察到的 registry 映射保留，余额归零不删除映射。registry 不声明 B 以前完整历史覆盖。
- P6.3 原生模式按积压量选择本地取块，缺块回退 RPC；不再使用旧 LocalLoader 的创世连链/单追加文件假设。
  默认阈值500块，详见[P6.3操作与验收](./balance-history-assumeutxo-p63-operations.md)。
- 可以在 Core 的快照链激活后开始导入，无需等待 G 或最新高度。导入后只消费已达 stable lag 的区块，
  缺少下一稳定块时保持私有 staging 并等待；发布仍要求 G 达到稳定深度和完整校验通过。

## 2. 检查点与版本边界

快照是执行完 B 后的状态，内置 C(B) 之后，从 B+1 起使用与全量重放相同的算法：

```text
SHA256(UTF8("balance-history:block-commit:v1")
       || height:u32BE || btc_block_hash:raw32
       || balance_delta_root:raw32 || previous_block_commit:raw32)
```

G 高度保留重放所得的完整 commit 记录和真实 delta root，不使用独立状态摘要替换种子。
G=B 时直接保留检查点中的 C(B)/D(B)，随后从 B+1 正常同步。
独立 [Python 编码器](../../tests/common/bootstrap_rolling_golden.py)和[固定向量](../../tests/fixtures/bootstrap-rolling-v1.json)
从真实 regtest C(101) 接续计算 C(102)、C(103)，与旧全量重放记录一致。

主网935000的[检查点](../../src/btc/balance-history/src/bootstrap/checkpoints/mainnet-935000.json)绑定源快照完整身份、
数据模型、commit协议、C(B)及D(B)，其来源见[P6检查点证据](./balance-history-assumeutxo-p6-bootstrap-design.md#31-检查点身份与生成证据)。
主网配置不能覆盖检查点；私有 regtest 可在 `bootstrap.identity.regtest_checkpoint` 提供显式检查点，需同样绑定快照身份。
支持新的主网快照需先核对同一旧链的检查点，再随代码发布；不从任意配置或未经核验的下载文件接受历史 commit。

`get_snapshot_info`、`get_block_commit` 和历史 state-ref 均报告 `commit_protocol_version=1.0.0`。
同一业务区间内，余额、完整 block-commit、state-ref 和下游 pass/local/system 值应与全量重放相同。
`get_bootstrap_info` 返回 native 阶段、快照来源、`checkpoint`、G 的状态投影、`origin_commit`、
`origin_balance_delta_root` 及独立 `origin_state_digest`；后者只用于状态校验，不参与滚动 commit。
旧 `get_snapshot_provenance` 保留其 core snapshot 安装语义，原生库返回 null。

余额语义、stable lag、snapshot_id、下游算法版本维持原契约。
原生数据库 schema 为 `balance-history-rocksdb-schema:native-checkpoint-v1`；仍使用独立目录，
不得把先前 v2 实验状态改写元数据后冒充兼容库。生产默认配置、镜像和发布集成仍属于 P7。

## 3. 本批验证证据

本节记录检查点方案本轮结果；此前 v2 种子的测试不作为本方案的 commit 兼容验收。

当前自动测试覆盖：

- 原生导入、封存及正常接块的 commit、delta root 与从 0 重放逐块一致。
- 同一真实 regtest 链的 B=101/B=102 两份 Core 快照，G=102；更换基线及 G=B 时仍保持状态/commit一致。
- 导入和重放取消后恢复、改变批大小、封存后发布恢复、删除临时输入后的重启。
- 正常浅重组、跨G深重组拒绝、旧库/配置不匹配、旧实验v2元数据及替换检查点的拒绝。
- 未支持主网快照、主网自定义检查点、G<B的拒绝；更晚业务 G 的配置接受与 RPC canonical 检查。
- 损坏文件、损坏导入前缀、总金额相同但脚本间余额被转移的拒绝。
- 真实 native RPC 生成[固定下游输入](../../tests/fixtures/assumeutxo-native-downstream.json)，
  通过生产 pass commitment、storage 和 RPC 验证 local/system 与全量重放一致。

| 自动检查 | 结果 |
| --- | --- |
| balance-history 库回归 | 199 passed、1 ignored；排除2项2000万条缓存容量测试，命令耗时56.827秒 |
| 旧P5及原生检查点下游验收 | 2 passed，pass/local/system 与全量重放一致；命令耗时16.872秒（含编译） |
| workspace Clippy，all-targets/all-features，`-D warnings` | 通过 |
| workspace API文档构建 | 通过，6.946秒 |
| release服务及工具构建 | 通过；最终二进制用于下面真实进程验收 |
| 独立Python状态/rolling向量、完整下游fixture相等 | 通过 |
| 主网检查点只读提取与内置记录对比 | 通过，未扫描大表或读取旧SQLite文件 |
| 格式、diff检查、CLI帮助及不存在状态目录的拒绝 | 通过 |

库回归命令：`cargo test --offline --locked --manifest-path src/btc/Cargo.toml -p balance-history --lib -- --skip test_address_balance_cache_size --skip test_utxo_cache_size --test-threads=1`。
下游过滤器：`-p usdb-indexer --bin usdb-indexer assumeutxo_p5`。运行日志位于 `/tmp/usdb-p6-checkpoint-checks/`。

真实 Core 31.1/BH 进程验收：`/tmp/usdb-p6-checkpoint-live-060z41gt/result.json`，全部通过，测试进程已正常停止。

| 场景 | 结果 |
| --- | --- |
| B=101/G=102 自动原生启动，正常同步103 | commit、余额、state-ref 匹配全量重放固定输入；协议1.0.0 |
| 移除临时输入文件后重启 | 相同commit/状态；不依赖输入快照重新扫描 |
| 重组后重启并追至稳定105 | 正确回滚到102并应用新分支 |
| 在同一重组链上另建从创世块同步的正常BH实例 | 102–105完整commit记录全部一致，105的state-ref一致 |

Core 未启用 txindex（`getindexinfo={}`）。上述小型 regtest 合计6.944秒，不代表主网冷启动耗时。
G=102 的旧 commit 为 `83e2873c70906fa31263ac9be91d604285fd0a946133343fac97696fee307a96`；
103 的旧 commit 为 `3f7bb7bd5728e2a2fd91c3a6727315de22108620e7d0aab8c59b1ea8677a4e81`。
验收服务二进制 SHA-256 为 `15222ba6943b8d295edee172cc221dd3cc7b2c11ede8a25e1c6e995f13e8e8c6`。
这是确定性跨模块及 Core/BH 验收，不是完整 Ord 铸造重放，也不替代 P6.3 稀疏 blk 文件验收。

B=102快照由 Bitcoin Core 31.1 从已固定的同一组区块生成，生成身份和 Core 统计记录在
[fixture目录](../../tests/fixtures/assumeutxo-p5/base-102/)；生成命令：

```bash
python3 tests/common/generate_assumeutxo_fixture.py \
  --bitcoind /data/btc/bitcoin-31.1/bin/bitcoind \
  --chain tests/fixtures/assumeutxo-p5/chain.json --snapshot-height 102 \
  --output <新的空输出目录>
```

本轮没有启动主网重导入、替换正在运行的主网服务或重跑 Core AssumeUTXO 双 chainstate 验证。
P2 结果继续独立记录；主网完整流程的性能与整套在线端到端仍待后续验收。

## 4. 主网长任务：准备独立目录与配置

本节待操作人员安排。现有935000输入文件仍可用；不依赖已删除的旧快照 workspace。
完整流程包含约1.64亿条 UTXO 导入、28,800块重放、逻辑投影扫描和独立余额重新聚合，不能用小型 regtest 估算耗时。
独立聚合使用有界内存批次和临时 RocksDB，会额外占用磁盘并产生随机读写；需同时预留最终库、staging/compaction 峰值、
原始快照、构建缓存和 Bitcoin 持续增长空间。

1. 在 `/home/bucky/work/usdb` 构建（本批已构建过，源码变化后重建）：

   ```bash
   CARGO_BUILD_JOBS=2 cargo build --offline --locked --release \
     --manifest-path src/btc/Cargo.toml -p balance-history --bins
   ```

2. 在同一个终端设置独立目录与现有供块节点信息。以下沿用本机前述8332端口和cookie位置；执行前确认仍与实际节点一致。

   ```bash
   export P62_ROOT=$(mktemp -d /data/usdb-assumeutxo-validation/p62-mainnet.XXXXXX)
   export P62_SOURCE=/data/btc/mainnet-935000-utxos.dat
   export P62_BTC_DATA=/home/bucky/.bitcoin
   export P62_BTC_RPC=http://127.0.0.1:8332
   export P62_RPC_PORT=28341
   P62_BIN=/home/bucky/work/usdb/src/btc/target/release/balance-history
   test -r "$P62_SOURCE"
   test -r "$P62_BTC_DATA/.cookie"
   echo "$P62_ROOT"
   ```

   供块节点必须能随前台推进提供935001起所有所需 canonical 区块。启动导入只需 active chain 已到基线935000；
   无需预先追到963800。最终封存/发布要求 tip 至少963810，以满足已有 stable lag=10。
   目标生产节点继续采用 `prune=0` 和官方后台验证；不能用第一轮已裁剪 P2 实验节点代替完整供块源。
   本阶段使用现有全量节点供块时，记录为本地验证，不推断公网 AssumeUTXO 冷启动耗时。

3. 生成配置（只写入上一步新建目录）：

   ```bash
   python3 - <<'PY'
   import json
   import os
   from pathlib import Path

   root = Path(os.environ["P62_ROOT"])
   destination = root / "config.toml"
   if destination.exists():
       raise SystemExit("Refusing to overwrite an existing config")
   text = f'''[btc]
   network = "bitcoin"
   data_dir = {json.dumps(os.environ["P62_BTC_DATA"])}
   rpc_url = {json.dumps(os.environ["P62_BTC_RPC"])}
   [ordinals]
   [electrs]
   [sync]
   batch_size = 20
   local_loader_threshold = 500
   max_sync_block_height = 963800
   utxo_max_cache_bytes = 268435456
   balance_max_cache_bytes = 268435456
   [rpc_server]
   host = "127.0.0.1"
   port = {int(os.environ["P62_RPC_PORT"])}
   [bootstrap]
   snapshot_file = {json.dumps(os.environ["P62_SOURCE"])}
   import_batch_size = 20000
   replay_batch_size = 20
   [bootstrap.identity]
   origin_height = 963800
   origin_block_hash = "000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551"
   [bootstrap.identity.snapshot]
   network = "bitcoin"
   base_height = 935000
   base_hash = "0000000000000000000147034958af1652b2b91bba607beacc5e72a56f0fb5ee"
   file_sha256 = "e572ddbe456d254f05fb004cebe225bdb3656074b66f0e9b1c7fa83e1301d486"
   hash_serialized_3 = "e4b90ef9eae834f56c4b64d2d50143cee10ad87994c614d7d04125e2a6025050"
   '''
   destination.write_text(text)
   print(destination)
   PY
   ```

## 5. 执行、复核与恢复

4. 在可持续运行的终端执行独立 bootstrap；正常运行时 stdout 是最终来源记录，stderr 包含扫描/重放进度：

   ```bash
   "$P62_BIN" --root-dir "$P62_ROOT" bootstrap \
     > "$P62_ROOT/bootstrap-result.json" 2> "$P62_ROOT/bootstrap.log"
   ```

   另一个终端查看实际目录下的 `bootstrap.log` 和 `bootstrap-progress.json`。
   以退出码0和最终 `phase=sealed` 为成功；日志中的阶段性报告不能替代封存结果。
   第一次导入和恢复都会校验输入；独立余额聚合及对比也报告进度。
   `phase=waiting_for_blocks` 表示等待下一稳定块，不是导入失败或已经就绪；进度包含 `height/target/btc_tip/published`。
   若要一次启动后自动提供服务，可用第6步的正常服务命令替代本步独立 bootstrap 命令；
   同一 root 同时只运行一个写入者，RPC 会在 bootstrap 完整封存后才启动。

5. 核对最终记录的 `origin` 与 P4 已通过的投影：

   - G/hash 与上面的配置一致，`commit_protocol_version=1.0.0`。
   - UTXO：`rows=165748439`，SHA-256 `86cba93334b2a6b6862a00d070617a0bdfded56b8eec2a25c41c2dcff82faedd`。
   - 非零余额：`rows=59356343`，SHA-256 `7e9e427332cf8bf95a52cd8689e2b17c732593b69a3f7f062c03dd93ab92449b`。
   - 两张表总 satoshi 相同。`origin_commit` 必须等于旧 C(963800)：
     `d5981b06db4e1f9d12f3a9e8b66da45001d0b38d3e2a689de3251542922f1e75`。
   - `checkpoint.block_commit` 必须等于旧 C(935000)：
     `6108f77e4abaafbc3a7a246024e942483c18fb37a3c710209ea6294c5617fe81`。
   - 另行归档 `origin_state_digest`，它是逻辑状态校验摘要，不与 rolling commit 比较。

6. 确认28341端口空闲后，在独立目录启动服务并保持963800：

   ```bash
   ss -ltn 'sport = :28341'
   "$P62_BIN" --root-dir "$P62_ROOT" --skip-process-lock --max-block-height 963800
   ```

   `--skip-process-lock` 仅用于本机与现有服务并行的独立验收；根目录和RPC端口必须独立。
   正式部署使用标准进程锁。查询 `get_readiness`、`get_bootstrap_info`、`get_snapshot_info`、963800的state-ref，
   确认余额/历史 floors 为963800/963801、commit协议为1、registry不声明全历史覆盖。
   正常停止并重启，核对原 v1 commit、状态摘要和查询结果相同，且没有新的输入扫描日志。

验收期间保留输入文件，不必为了验证重启再删除9GB快照；删除输入后的重启已在独立真实regtest完成。
需要继续追平时，先正常停止服务，将 `sync.max_sync_block_height` 调整为4294967295，并在再次启动时省略 `--max-block-height`。

恢复规则：

- Ctrl+C/SIGTERM 请求合作式取消，当前数据库批次收尾后退出；重跑同一配置继续，尚未封存的库不提供业务查询。
- 断电/SIGKILL 后读取同步 WAL 检查点恢复。输入校验阶段会重扫；重放从已保存高度继续；封存后发布前中断可重新发布。
- 身份变化、旧库混用、损坏前缀或跨G重组会明确失败，不自动覆盖数据库。保留现场并选用新目录处理；不能只改元数据绕过错误。
- 正常结束/合作式取消会清理本次私有 `native-verify-*` 临时聚合目录；强制终止可能留下此类派生目录。
  先确认任务已停止并保留日志，再按实际目录处理，不删除主库或快照输入。

主网完整流程的时间、内存和磁盘峰值仍待本节运行后记录。当前源码默认按积压量选择 RPC/本地读取；
如需纯 RPC 对照，将 `local_loader_threshold` 设为4294967295，并使用另一独立目录，见 P6.3 手册。
