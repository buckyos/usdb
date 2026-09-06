# 拆分快照全量对拍与 Electrs 抽样操作

本文针对 BTC 高度 `963800` 的已发布 core＋script registry 快照。两条路径都是只读审计，
直接读取本地 immutable SQLite，不需要重新下载、导入 RocksDB 或停止 balance-history。
全量扫描会使用较多磁盘 I/O，建议与 electrs 抽样顺序执行。

与旧操作的主要区别：全量对拍用 `--core-snapshot-db` 代替当前侧 `--balance-history-root`；
electrs 的 `--snapshot-db` 指向新 core；两者均额外传入 `--script-registry-db`。
Manifest 默认从各自 DB 旁的同名 `.manifest.json` 读取。

## 1. 构建和路径准备

在同一个 Bash 会话中执行。先重新构建，避免继续调用拆分适配之前的 release binary：

```bash
cd /home/bucky/work/usdb
cargo build --release --manifest-path src/btc/Cargo.toml \
  -p balance-history-snapshot-tool -p balance-history-electrs-audit

H=963800
HASH=000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551
OLD=/data/.usdb/balance-history-snapshot-mainnet/builder/snapshots/000000963800/$HASH
NEW=/home/bucky/.usdb/balance-history-snapshot-mainnet/builder/snapshots/000000963800/$HASH
AUDIT_RUN=/home/bucky/.usdb/balance-history-snapshot-mainnet/releases/reports/audits/h${H}-$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$AUDIT_RUN"
echo "Audit reports: $AUDIT_RUN"
set -o pipefail
```

保存打印出的 `AUDIT_RUN`，重连终端后恢复该路径。长任务可在已有的 tmux/screen 会话中运行。

## 2. 旧整体快照与新拆分产物全量对拍

```bash
./src/btc/target/release/balance-history-snapshot-tool \
  --root-dir "$AUDIT_RUN/tool" --json compare-legacy \
  --snapshot-db "$OLD/snapshot_$H.db" \
  --core-snapshot-db "$NEW/core/balance_history_core_$H.db" \
  --include-script-registry \
  --script-registry-db "$NEW/script-registry/script_registry_$H.db" \
  --height "$H" --parallelism 4 --integrity-check off \
  --output "$AUDIT_RUN/legacy-vs-split-full.json" \
  2>&1 | tee -a "$AUDIT_RUN/legacy-vs-split-full.log"
```

此命令完整扫描余额、live UTXO、block commit 和 registry 四张表。`--integrity-check off`
只关闭重复 SQLite integrity pragma，不关闭逐行对拍；这里复用已完成的 snapshot verify/finalize。
如果本轮还要重新核验两个新 DB 的完整 SHA-256，加上 `--verify-file-hash`。该选项不重新核验
旧整体快照的文件 hash，旧输入仍应使用此前已验证的 immutable 文件。

验收条件：进程退出码为 0，JSON 顶层 `ok=true`、`unexpected_difference_rows=0`、
`script_registry_compared=true`，且 `tables` 包含四张表。允许的历史语义差异仅限比较器中已经
冻结的 BIP30 和 unspendable 规则；规则和逐表报告字段见
[旧快照跨版本语义对拍](./balance-history-legacy-snapshot-semantic-comparison.md)。

全量对拍目前没有断点续跑；完整报告在结束时写出，中断后需要重新扫描。若只想先检查核心三表，
移除 `--include-script-registry` 和 `--script-registry-db`，并使用另一个报告文件名。

## 3. 新拆分快照与 Electrs 抽样对拍

以下配置沿用本机已验证的 electrs：`/data/.electrs/config.toml` 中
`index_lookup_limit=20000`，进程已在该配置写入后使用它启动。
`--confirm-electrs-restarted-with-config` 表达这一事实；它不会修改配置或重启服务。
如果运行实例发生变化，应重新确认所用配置。保护机制详见
[Electrs 工具说明](../../src/btc/balance-history-electrs-audit/README.md)。

```bash
AUDIT_SAMPLES=32
AUDIT_SEED=usdb-mainnet-963800-split-smoke-v1

./src/btc/target/release/balance-history-electrs-audit \
  --snapshot-db "$NEW/core/balance_history_core_$H.db" \
  --script-registry-db "$NEW/script-registry/script_registry_$H.db" \
  --electrs-config /data/.electrs/config.toml \
  --confirm-electrs-restarted-with-config \
  --sample-count "$AUDIT_SAMPLES" --seed "$AUDIT_SEED" \
  --concurrency 2 --transaction-cache-mib 128 \
  --output-dir "$AUDIT_RUN" \
  2>&1 | tee -a "$AUDIT_RUN/electrs.log"
```

默认访问本机 `127.0.0.1:50001`；默认 75% 正余额、25% 零余额样本。本次同 seed 的 32 条
smoke 已全部匹配，0 差异、0 跳过、0 错误。要增加覆盖面，可以增加 `AUDIT_SAMPLES`，或换一个
明确记录的 `AUDIT_SEED`；工具会自动生成新的报告与 checkpoint 文件名。保持相同输入和参数
会复现相同样本，不是重新随机选择。

验收条件：退出码为 0，最终报告 `summary.complete=true`、`summary.ok=true`，且
`summary.matched` 等于样本数。默认 skipped 也会失败，不要用 `--allow-skipped` 隐藏验收缺口。
重算两个新 DB 的 hash 可追加 `--verify-file-hash`；只检查输入和样本计划可追加 `--plan-only`，
但计划模式不联系 electrs，不能作为抽样对拍通过的证据。

## 4. 中断、重试和归档

- Electrs 支持断点：恢复相同 `AUDIT_RUN`、样本数、seed、黑名单、输入文件和 hash 验证模式后，
  重跑第 3 节命令。不要重新执行第 1 节生成新的时间目录后再期待复用旧 checkpoint。
- 旧 v1 checkpoint 不可复用；新 v2 绑定 core、sidecar 和 hash 验证模式。更换这些输入会产生
  新 run ID。全量对拍的重跑始终从头开始。
- 保留 JSON、日志、checkpoint 和本次工具的 Git revision。公开发布记录验证、文件 hash 校验、
  全量语义对拍和 electrs 抽样是不同证据，应分别归档。
