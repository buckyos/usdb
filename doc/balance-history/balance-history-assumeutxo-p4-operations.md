# AssumeUTXO P4 导入与重放验证操作

日期：2026-09-12。主网935000导入、重放至963800及三项全量投影对比已通过，见[主网复核记录](./balance-history-assumeutxo-p4-validation-2026-09-12.md)。以下命令保留用于复现，无需因旧错误日志重新执行已通过阶段。

本轮已执行：workspace编译检查、独立CLI帮助和fixture扫描、Core 31.1 fixture生成，
6项解析/存储/重放及RPC预检测试通过；初版的3项既有query floor回归已通过。

主网已确认的导入结果来自 `/data/usdb-assumeutxo-validation/p4-mainnet-935000-to-963800/import-result.json`：
164,241,311个UTXO、1,998,414,803,206,779 sat，原文件及Core内容hash均通过，耗时1945.5秒（约32分26秒）。
本次检查时 `state` 磁盘占用18,779,533,312字节（约17.49 GiB），这是导入后的观测值，并非峰值或重放后最终体积。
前两次replay在连接RPC前因cookie消失退出，未写入重放区块；供块节点恢复后成功重放28,800块，约1小时3分40秒。
最终165,748,439个UTXO、59,356,343个非零余额和28,801条commit记录与参考逐项一致。
重放后state约29.24 GiB，具体证据和计时口径见复核记录。无需重新import。

## 1. 这次验证的范围

新增独立 CLI `balance-history-assumeutxo-tool`，默认服务启动不会调用它：

1. 流式解析 Core v2 快照，核对文件 SHA-256、网络/base hash，以及 Core `hash_serialized_3`。
2. 将全部活 UTXO 投影为现有 outpoint/script_hash/value，聚合 935000 余额，建立 live script registry。
3. 从已固定 SHA-256 的 963800 core SQLite 提取 935000 USDB commit 锚点。
4. 通过现有全量节点 RPC 重放 `935001..=963800`，沿用现有余额、delta root、block commit 算法；每批逐块对拍。
5. 在 963800 扫描双方全部 UTXO、非零当前余额及 935000–963800 commit，输出行数和有序内容 SHA-256。

**验证阶段仍读取旧 core 作为锚点和对照，但不从它导入余额或 UTXO，也不读取 registry sidecar。**
后续部署要摆脱这个大参考文件，仍需单独发布、验证小型 USDB 锚点；本原型没有完成发布集成。
余额相等不代表基线前历史元数据相等：基线行 delta=0，表示断面而非真实收入，点查 floor=935000、历史 floor=935001。
RPC 边界、state-ref、stable lag、registry coverage 和整套就绪仍需 P5/P6 验收，禁止将实验目录直接交给生产服务。

本轮小规模用例使用真实 Core 31.1 生成的 101 高度快照及 0–103 原始区块，覆盖 403 个活 UTXO、
零值/空脚本、未成熟 coinbase、CompactSize 多字节 vout、不同批大小续导、重启续放、同块花费、
回滚到基线后重放、越过基线拒绝、缺失 prevout 禁止历史 RPC 回退，以及校验失败不发布。
测试结果另见本文件末尾。主网重放另有全量通过证据，不能据此宣称P5查询/registry/下游契约均已通过。

## 2. Bitcoin 策略与资源

目标部署已选择 **`prune=0`，保留全部区块**，按官方机制继续后台历史验证。
P4 不依赖后台完成，也不把 `validated=false` 当作余额重放失败。
Core 的 txindex/filter 等索引仍从创世顺序构建，整套 indexer 就绪需要后续单独改造。
依据：[Core 31.1 AssumeUTXO 说明](https://github.com/bitcoin/bitcoin/blob/v31.1/doc/assumeutxo.md)。

第一轮 P2 实验节点已经发生裁剪，且已停止。**本操作不重启、清库或修复该实验节点**；
使用当前保留全块的 28.1 节点供块，RPC 解码兼容，不需要先升级现有节点。
将来在新的 `prune=0` 节点上独立冷启动，还要验证不依赖旧节点的完整路径。

初版工具验收时 `/data` 约405 GiB可用、根分区约3 GiB；本次RPC故障排查时，
根分区已释放至约30 GiB、`/data`约388 GiB。运行前必须重新确认：

```bash
df -h / /data
free -h
```

新实验库必须放 `/data`。9.39 GB 源快照和 37.01 GB 参考 core 已存在，本工具不复制它们，
也不产生第二份全量 SQLite 中间库，但会新增 RocksDB UTXO、余额、live registry、重放历史及 WAL/compaction 空间。
实际峰值待主网实测，不以 9.39 GB 源文件大小推断库体积。首次运行建议预留至少 200 GiB 试验余量，
这只是调度预算而非已验证上限；持续观察磁盘，低于 50 GiB 可用时停止本工具、保留目录后再安排扩容。
构建同样需要空间，不能在根分区空间不足时盲目重建整个 workspace。
当前供块节点的 `/home/bucky/.bitcoin` 也位于根分区，长任务前应安排根分区余量；仅把新实验库放到 `/data` 并不能解决供块节点增长所需空间。

## 3. 构建与短检查

```bash
cd /home/bucky/work/usdb
CARGO_BUILD_JOBS=2 cargo build --locked --release \
  --manifest-path src/btc/Cargo.toml -p balance-history \
  --bin balance-history-assumeutxo-tool
src/btc/target/release/balance-history-assumeutxo-tool --help
```

每个 shell 会话先设以下变量。首次使用一个新目录，重试时保持同一个 `P4_RUN`；不要指向已有服务目录。
日志放父目录，以免日志文件导致新 workspace 的“空目录”检查失败。

```bash
cd /home/bucky/work/usdb
P4_TOOL=/home/bucky/work/usdb/src/btc/target/release/balance-history-assumeutxo-tool
P4_RUN=/data/usdb-assumeutxo-validation/p4-mainnet-935000-to-963800
P4_ID=/home/bucky/work/usdb/doc/balance-history/assumeutxo-mainnet-935000.identity.json
P4_SNAPSHOT=/data/btc/mainnet-935000-utxos.dat
P4_CORE=/home/bucky/.usdb/balance-history-snapshot-mainnet/builder/snapshots/000000963800/000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551/core/balance_history_core_963800.db
P4_CORE_SHA=3e3490ac19521647a8513a0ef2961607df4456ba3e3ea6922c1ba750f7fbea61
P4_COOKIE=/home/bucky/.bitcoin/.cookie
mkdir -p /data/usdb-assumeutxo-validation
export RAYON_NUM_THREADS=4
set -o pipefail
```

只读检查供块节点；使用 31.1 CLI 调用现有 28.1 RPC 不会升级服务：

```bash
/data/btc/bitcoin-31.1/bin/bitcoin-cli -datadir=/home/bucky/.bitcoin -rpcport=8332 getblockchaininfo
/data/btc/bitcoin-31.1/bin/bitcoin-cli -datadir=/home/bucky/.bitcoin -rpcport=8332 getblockhash 935000
/data/btc/bitcoin-31.1/bin/bitcoin-cli -datadir=/home/bucky/.bitcoin -rpcport=8332 getblockhash 963800
/data/btc/bitcoin-31.1/bin/bitcoin-cli -datadir=/home/bucky/.bitcoin -rpcport=8332 getblock 000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551 0 >/dev/null
```

应为 `chain=main`、`pruned=false`，且两高度 hash 与身份文件、目标 hash 一致。
重放工具会再次校验网络、目标 hash 和每批 canonical 链，缺块或 RPC 超时会退出并保留进度。

## 4. 长任务 A：独立扫描源快照

只读源文件，输出小型报告；首次运行建议执行，便于单独记录解码速度。后续 import 会重新验证同一输入。

```bash
/usr/bin/time -v "$P4_TOOL" scan \
  --snapshot "$P4_SNAPSHOT" --identity "$P4_ID" \
  --output "${P4_RUN}.scan-result.json" \
  2>&1 | tee -a "${P4_RUN}.scan.log"
```

验收：命令退出码 0，`coins=164241311`，`total_satoshis=1998414803206779`，
`bytes=9387990306`，两种 hash 匹配。扫描需读取和解码整个文件，不是读取头部的快速检查。

## 5. 长任务 B：导入 935000

```bash
/usr/bin/time -v "$P4_TOOL" --root-dir "$P4_RUN" import \
  --snapshot "$P4_SNAPSHOT" --identity "$P4_ID" \
  --reference-core "$P4_CORE" --reference-sha256 "$P4_CORE_SHA" \
  --batch-size 65536 \
  2>&1 | tee -a "${P4_RUN}.import.log"
```

工具先完整核对参考 core 文件 SHA-256，再导入。进度写在 `staging` 内的 RocksDB checkpoint；
`import-progress.json` 是便于观察的副本。全部输入验证通过才将 `staging` 原子改名为 `state`。
验收：退出码 0，`state/assumeutxo-source.json`、`import-result.json` 存在，币数/金额与扫描报告一致。

导入中断后原样重跑命令即可，可更改 `--batch-size`；源路径、身份和参考文件身份必须不变。
工具会重新扫描源文件验证，并跳过已原子提交的前缀，不重复累加余额。
输入身份不一致或 hash 失败时不要改写 `input.json` 强行继续；先查明来源，需要换输入时使用新目录。
已经发布的目录再次 import 会提示使用 status/replay，不覆盖已完成数据。

## 6. 长任务 C：重放至 963800

scan/import 不需要 Bitcoin RPC，完成导入并不表示供块节点仍在运行。开始 replay 前再次检查：

```bash
: "${P4_COOKIE:?请先设置 P4_COOKIE}"
test -r "$P4_COOKIE" && \
  /data/btc/bitcoin-31.1/bin/bitcoin-cli -datadir=/home/bucky/.bitcoin \
    -rpcport=8332 -rpccookiefile="$P4_COOKIE" getblockchaininfo
```

仅在上面的命令成功后执行重放。更新后的工具会先检查 cookie、RPC 连接和网络，再扫描参考 core。

```bash
/usr/bin/time -v "$P4_TOOL" --root-dir "$P4_RUN" replay \
  --height 963800 --rpc-url http://127.0.0.1:8332 \
  --cookie-file "$P4_COOKIE" --batch-size 20 \
  2>&1 | tee -a "${P4_RUN}.replay.log"
```

读取 28,800 块，每批通过现有余额计算器处理，并与 core 中每一块 BTC hash/delta root/commit 对拍。
每批在 WAL 同步后写进度，终点写 `replay-result.json`。该步骤还会重新检查参考文件 SHA-256。
不要同时在这个目录启动其他工具写库；workspace 锁会拒绝重复写入。

失败或人工停止后原样重跑；已提交的区块先重新核对再续放。如果保存的 tip 已不在 canonical 链上，
或 commit 不一致，工具明确停止，不自动删除数据库、忽略差异或越过基线重组。
工具本身不启动 Bitcoin，也不需要历史 `getrawtransaction`；缺失 prevout 直接失败。

若看到 `Failed to update BTC RPC client: I/O error: No such file or directory`，应先检查 cookie 是否存在，
以及供块节点是否已退出；不要重新导入快照。2026-09-12 的一次实测失败是原节点因磁盘不足退出、cookie 消失，
并非 AssumeUTXO 或参考库校验失败。查看 `/home/bucky/.bitcoin/debug.log` 末尾及 `df -h / /data`，
确认原因已排除后按原方式启动供块节点；本机原手动启动的 28.1 实例可使用：

```bash
/home/bucky/btc/bitcoin-28.1/bin/bitcoind -datadir=/home/bucky/.bitcoin -daemonwait
```

以上启动命令只适用于确认原实例已停止的情况；由服务管理器管理的部署应使用对应服务的启动命令。
RPC 检查恢复后，使用相同 `P4_RUN` 重新执行 replay，已经发布的导入数据继续复用。

验收：退出码 0、`status=pass`、height=963800，BTC hash 为上述目标，最终 commit：
`d5981b06db4e1f9d12f3a9e8b66da45001d0b38d3e2a689de3251542922f1e75`。
保留终点前最多 288 块的 undo；不能据 baseline floor 推断整个重放区间都有 undo。

## 7. 长任务 D：全量逻辑状态对比

```bash
/usr/bin/time -v "$P4_TOOL" --root-dir "$P4_RUN" compare \
  2>&1 | tee -a "${P4_RUN}.compare.log"
```

再次验证参考文件 SHA-256，并分别扫描候选 RocksDB 与参考 SQLite 的全部投影行。
不生成百 GB 级别文本导出；结果写 `comparison-result.json`，每个表保存 `[candidate, reference]` 的行数和 hash。
验收：退出码 0，`equal=true`，`utxos`、`balances`、`commits` 三项均一致。
目标参考预期 165,748,439 个 UTXO、59,356,343 个非零余额、28,801 条基线到目标的 commit。
若不一致，保留双方 DB 和报告；当前工具给出表级差异，逐行双向差异样本属于后续 P5 排查。

余额对比仅使用 script_hash+balance，双方排除零余额行；旧脚本最后变动高度/delta 不在等价投影内。
UTXO 比较包含零值输出；commit 比较包含每块 BTC hash、delta root、rolling commit。
这不能替代 RPC 查询、registry coverage、下游 state-ref 和冷启动性能验收。

## 8. 观察、停止与证据

另开 shell，设同样的 `P4_TOOL`/`P4_RUN`，执行：

```bash
"$P4_TOOL" --root-dir "$P4_RUN" status
df -h /data
du -sh "$P4_RUN"
```

`status` 只读原子 JSON 报告，可与长任务并行，不打开正在写入的 RocksDB。
报告中的 running 是上次 checkpoint 状态，不代表进程仍存活；结合原终端/日志判断。
完成后 `replay-progress.json` 仍可能是 `running,height=963800`，以 `replay-result.json` 的 `status=pass` 和 `comparison-result.json` 的 `equal=true` 判定最终结果。
需要暂停时对工具前台进程按 Ctrl-C；不要停止供块的 Bitcoin 服务。导入/重放恢复以 RocksDB 原子批为准，
进度 JSON 只是观测；全量 scan/compare 中断后重新从头扫描。

保存四阶段日志、`*.json`、`git rev-parse HEAD` 和 `git diff`，记录 `/usr/bin/time -v` 的耗时/峰值 RSS、
运行前后磁盘容量及阶段库大小。首次主网已测得扫描约32秒、导入约32分26秒、重放约1小时3分40秒；
这些不包含所有前置检查且使用本地全量节点供块，不能当作其他硬件或公网冷启动的完成时长保证。

## 9. 小规模测试复现

测试 fixture 为 Core 31.1 实际 RPC 导出，不由 Rust 解析器反向生成。无私钥、cookie、钱包或主网数据。
`tests/fixtures/assumeutxo/chain.json` 记录 Core 版本、dump/statistics 和原始区块；身份记录在相邻 JSON。

```bash
CARGO_BUILD_JOBS=2 cargo test --locked --manifest-path src/btc/Cargo.toml \
  -p balance-history --lib assumeutxo -- --test-threads=1
```

需要重新生成独立 fixture 时选择一个新的空目录；脚本临时启动无外联 regtest，完成后停止并清理临时节点：

```bash
python3 tests/common/generate_assumeutxo_fixture.py \
  --bitcoind /data/btc/bitcoin-31.1/bin/bitcoind \
  --output /tmp/assumeutxo-fixture-new
```

新生成区块 hash 与本次不同属正常现象，不应混用不同次生成的 snapshot、identity 和 chain。
