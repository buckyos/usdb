# Balance-History 旧快照跨版本语义对拍

## 1. 目的与边界

`balance-history-snapshot-tool compare-legacy` 用于把旧版 schema v2 SQLite snapshot
与当前代码从创世块重新生成的 RocksDB 固定在同一 BTC 高度后逐项对拍。它解决的是跨版本
正确性审计问题，不承担以下职责：

- 不允许生产 snapshot installer 接受旧 schema；installer 仍只接受当前 schema 并 fail closed。
- 不迁移或修改旧 snapshot。
- 不修改当前 RocksDB。
- 不把未知差异自动解释为版本变化。

当前冻结的预期差异只有：

1. 两个 BIP30 duplicate coinbase 特例修正后的余额记录；
2. 这两个高度的 `balance_delta_root`，以及由此产生的后续 rolling `block_commit` 变化；
3. 旧实现曾保留、当前按 Bitcoin Core 规则排除的 unspendable output 数据。

两侧 rolling `block_commit` 链还会分别重新验算。普通余额、UTXO、BTC block hash、脚本或
commit 链差异均属于 unexpected difference，命令返回非零状态。

## 2. 对拍范围

默认比较共识和恢复相关的三组状态：

- 每个 script hash 在目标高度的最新非零余额记录；
- 目标高度的完整 live UTXO 集；
- 从起始记录到目标高度的 block commit 链。

增加 `--include-script-registry` 后再比较辅助 script registry。主网旧快照的 registry 可能有
十亿级记录，是耗时最长的阶段，建议先完成默认三表对拍，再单独运行完整对拍。

比较器使用 256 个有序 key shard，内存占用有界；`--parallelism` 控制并发 shard 数，默认
为 4。报告只保留每张表有限数量的示例，但所有差异都会计数。

## 3. 执行前置条件

1. 当前代码重放的 RocksDB 已经到达目标高度，且服务使用 `--max-block-height` 冻结在该高度。
2. 目标高度和旧 snapshot 的 BTC block hash 已经独立确认一致。
3. 停止 `balance-history` 服务。只读 RocksDB 可以与服务并存，但主网库打开和全表扫描会消耗
   大量 CPU、内存和磁盘 I/O，不应与同步进程并行运行。
4. 旧 SQLite 文件保持 immutable，不从其所在目录覆盖输出报告。
5. 确认有足够的时间和 I/O 预算。完整 script registry 对拍可能显著长于其它三表。

构建工具：

```bash
cd /home/bucky/work/usdb/src/btc
cargo build --release -p balance-history-snapshot-tool
```

## 4. 首轮共识状态对拍

```bash
ROOT=/home/bucky/.usdb/balance-history-mainnet-audit
OLD_ROOT=/home/bucky/.usdb/balance-history-snapshot-mainnet/builder/snapshots
HEIGHT=963800
HASH=000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551
HEIGHT_PADDED=$(printf '%012d' "$HEIGHT")
OLD_DB="$OLD_ROOT/${HEIGHT_PADDED}/${HASH}/snapshot_${HEIGHT}.db"
REPORT=/home/bucky/.usdb/balance-history-snapshot-mainnet/releases/reports/legacy-v2-vs-replay-${HEIGHT}-consensus.json

./target/release/balance-history-snapshot-tool --json compare-legacy \
  --balance-history-root "$ROOT" \
  --snapshot-db "$OLD_DB" \
  --height "$HEIGHT" \
  --parallelism 4 \
  --integrity-check off \
  --output "$REPORT"
```

本地旧 snapshot 已经完成独立 file hash、manifest 和 verify 时，可以使用
`--integrity-check off` 避免在语义扫描前再次完整读取大文件。来源不确定或未做完整校验时，
使用默认 `quick`；发布级调查可显式使用 `full`。关闭 integrity check 不会跳过表结构、行编码、
元数据计数和语义比较。

## 5. 完整辅助状态对拍

默认三表通过后，追加 script registry：

```bash
FULL_REPORT=/home/bucky/.usdb/balance-history-snapshot-mainnet/releases/reports/legacy-v2-vs-replay-${HEIGHT}-full.json

./target/release/balance-history-snapshot-tool --json compare-legacy \
  --balance-history-root "$ROOT" \
  --snapshot-db "$OLD_DB" \
  --height "$HEIGHT" \
  --include-script-registry \
  --parallelism 4 \
  --integrity-check off \
  --output "$FULL_REPORT"
```

进度写到 stderr，JSON 写到 stdout 和 `--output` 指定文件。打开主网 RocksDB 本身可能需要较长
时间和超过 1 GiB 内存；在第一条 table progress 出现前不要仅凭低瞬时输出判断进程卡死。

## 6. 报告与验收

顶层关键字段：

- `ok`：只有 unexpected difference 总数为 0 时才为 `true`；
- `legacy_meta`：旧 snapshot 高度、schema 和生产时记录的各表计数；
- `target_btc_block_hash`：全表扫描前已确认两侧一致的目标 BTC block hash；
- `current_db_identity`：当前 RocksDB 的网络/schema identity；
- `expected_difference_rows`：冻结规则解释的差异总数；
- `unexpected_difference_rows`：必须调查的差异总数；
- `tables`：逐表扫描数、精确匹配数、分类计数、有限示例和耗时。

验收要求：

1. 命令退出状态为 0，且报告 `ok=true`。
2. 旧 snapshot 元数据计数与实际扫描数一致。
3. expected difference 只能属于文档第 1 节列出的冻结分类。
4. 默认三表通过是当前跨版本核心正确性 gate；发布前再完成 script registry 全量对拍。
5. 保存报告、当前代码 commit、旧 snapshot manifest/hash 和运行参数，形成可重复审计记录。

任何 unexpected difference 都不能通过增加宽泛白名单直接消除。应先定位具体 script、outpoint
或高度，确认是否为实现缺陷；只有经过协议和实现 review 的确定性历史规则才能加入比较器。
