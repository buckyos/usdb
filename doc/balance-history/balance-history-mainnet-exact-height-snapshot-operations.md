# Balance-History 主网 Exact-Height Snapshot 操作指南

## 1. 目标与边界

本文说明如何在正式硬件上制作一个 BTC 主网 `balance-history` exact-height snapshot，供其他
节点安装后从目标高度继续同步。产物不是余额导出，而是一个可恢复 checkpoint，包含目标高度
的完整 balance history、全部 live UTXO、block commit 和 script registry。

本文只使用独立的 `balance-history-snapshot-tool`。不要把普通 `balance-history
create-snapshot` 命令替换进生产流程；后者不会负责从独立持久化 workspace 精确同步到目标
高度，也不提供这里要求的 job 恢复和按高度/分支管理。

容量测试中的 `100K/250K/1M` 指在隔离 regtest 中人工构造相应数量的 live UTXO，用于观察
工具的扩展趋势。正式主网 snapshot 会处理高度 `H` 上真实的全量状态，不能直接用合成测试
结果推算最终时长、内存或文件大小。

本文示例使用当前目标机器上已验证的路径：

```text
Bitcoin Core binaries: /home/bucky/btc/bitcoin-28.1/bin
Bitcoin mainnet datadir: /home/bucky/.bitcoin
Bitcoin RPC: http://127.0.0.1:8332
USDB data root: /home/bucky/.usdb
```

USDB 正式服务默认统一使用 `~/.usdb` 命名空间。推荐生产入口使用独立的
`~/.usdb/balance-history-snapshot-mainnet`；如果根盘容量不足，应把专用磁盘直接 mount 到该
目录，或显式设置 `SNAPSHOT_ROOT`，而不是复用在线服务目录。

截至 2026-08-23 对当前目标机器的只读预检结果：

- bitcoind 是未裁剪主网节点，`blocks == headers`，且不处于 initial block download；
- Bitcoin Core 已使用约 `870 GB`，其中 `blocks/` 约 `811 GB`；
- 机器内存约 `62 GiB`，当前 available 约 `59 GiB`；
- 根盘约 `2.2 TB`，清理旧 `~/.usdb/balance-history` 数据后约有 `550 GiB` 可用，使用率约
  `74%`。

当前机器已通过主网节点只读 preflight 和 1M warm/cold-advisory 合成容量测试，可以继续更大
容量评估。`550 GiB` 可用空间仍不自动等于“主网全量构建容量已证明充足”；首次全量 builder
应记录 workspace、artifact 和 validation install 的实际增长，并设置磁盘监控和停止阈值。

### 1.1 主网全量构建内存基线

2026-08-25，首次同步到目标高度 `963800` 的任务在 durable height `742912` 后被 Linux OOM
killer 终止。旧配置没有显式 cache limit，旧默认算法在 `62.8 GiB` 物理内存上计算出约
`54.8 GiB` cache budget，只留下固定 `8 GiB` 给 RocksDB、批处理对象、文件页缓存、分配器和
系统。该容量是按 cache entry 估算的逻辑上限，不是进程 RSS 硬上限，因此不能作为安全的
整机内存边界。

当前主网脚本改为按有效 cgroup/物理内存生成显式预算，默认参数为：

```text
SNAPSHOT_CACHE_BUDGET_PERCENT=66
SNAPSHOT_MAX_MEMORY_PERCENT=80
UTXO : balance cache = 1 : 3
```

在本机 `67,426,422,784` bytes 有效内存上，对应：

```text
UTXO cache       = 11,125,359,759 bytes  (~10.36 GiB)
balance cache    = 33,376,079,278 bytes  (~31.08 GiB)
total cache      = 44,501,439,037 bytes  (~41.45 GiB)
pressure trigger = 53,941,138,227 bytes  (80%)
```

两类 cache 合计约占有效内存三分之二，不是各自占三分之二。剩余约 `21.35 GiB` 不属于 cache
预算；cache logical limit 与 80% pressure trigger 之间另有约 `8.79 GiB`，供 RocksDB 和运行时
开销增长。exact-height `sync_to_height` 也会启动 cgroup-aware monitor，达到阈值后主动缩减
cache。

### 1.2 真实主网历史高度 smoke

2026-08-24 已使用本机未裁剪 Bitcoin Core 28.1 主网数据，在隔离 snapshot root 中完成高度
`10000` 的真实链路测试：

- canonical block hash 为
  `0000000099c744455f58e6c6e98b671e1bf7f37346bfd4cf5d0274ad8ee660cb`；
- 从创世同步并严格停在高度 `10000`，记录 `10000` 个 block commit 和 `9494` 个 live UTXO；
- release `create` 用时约 `365s`，artifact SHA-256 为
  `f674cefd1dc4be9bb87c3c14cb643fd364b38870314809046302ea9bab6991d8`；
- 独立 verify、signed install、release tar 和 tar checksum 全部通过；
- 同高度重跑返回 `resumed=true`、`already_complete=true`，snapshot ID 和 SHA-256 不变；
- 同一 builder 增量创建高度 `10001` 用时约 `1s`，block commit 和 UTXO 计数都按真实新区块
  推进，随后独立 verify 通过。

该测试证明 snapshot tool 能读取真实主网 blk 数据，并完成 exact-height、恢复、增量和签名安装
闭环。高度 `5000/10000` 处于 Bitcoin 早期，交易密度、script 类型和 UTXO 规模都很低，因此
适合作为真实数据功能 smoke，不代表现代主网状态的容量、I/O 或兼容性覆盖。后续应逐级增加
包含更多交易与新 script 时代的历史目标，最终仍需对计划发布高度执行全量构建和验证。

## 2. 完成条件

目标高度 `H` 的完整 checkpoint 必须满足：

```text
durable balance-history height
  == balance state height
  == UTXO state height
  == latest block commit height
  == H
```

快照身份由以下四项共同确定，不能只按高度识别：

```text
network + H + BTC block hash at H + snapshot ID
```

`--expected-block-hash` 会固定本次任务的 BTC 分支，但它不等于 BTC finality。发布方仍需选择
确认深度，并在分发前重新确认高度 `H` 的 canonical hash 没有变化。

### 2.1 `~/.usdb` 目录边界

推荐目录布局如下：

```text
~/.usdb/
├── balance-history/                    # 在线服务数据，snapshot 脚本禁止复用
├── balance-history-snapshot-mainnet/   # 离线 snapshot 生产根目录
│   ├── builder/                        # workspace、jobs 和 immutable artifacts
│   ├── config/                         # 首次 init 后冻结的 builder config
│   ├── targets/                        # 固定的 H -> BTC block hash
│   ├── validation/                     # 每个 artifact 的独立 signed install
│   └── releases/                       # reports、records 和可选 tar/SHA-256
└── secure/snapshot-keys/               # 私钥和公开 key material
```

这实现的是“统一数据命名空间、严格隔离状态目录”。snapshot workspace 不能直接使用
`~/.usdb/balance-history`，否则离线构建、恢复或 validation install 会和在线服务争用或污染
同一数据库。

主网全量构建需要专用磁盘时，优先把整个磁盘 mount 到：

```text
/home/bucky/.usdb/balance-history-snapshot-mainnet
```

也可以设置 `SNAPSHOT_ROOT=/mnt/.../balance-history-snapshot-mainnet`。应移动整个 snapshot root，
不要只把 `builder/tmp` 和 `builder/snapshots` 分散到不同文件系统；`.partial` artifact 的原子
发布依赖同一文件系统 rename。

### 2.2 推荐生产脚本

环境检查、release 构建、签名 key/config 初始化、目标 hash 固定、断点续跑、验证安装和打包
已封装为：

```text
src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh
```

从 USDB 仓库执行：

```bash
cd /home/bucky/work/usdb
SNAPSHOT_SCRIPT=src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh

bash "$SNAPSHOT_SCRIPT" paths
bash "$SNAPSHOT_SCRIPT" init
bash "$SNAPSHOT_SCRIPT" preflight --height "$H"
bash "$SNAPSHOT_SCRIPT" create --height "$H"
bash "$SNAPSHOT_SCRIPT" resume-verify --height "$H" # 仅在 status 为 verifying 时执行
bash "$SNAPSHOT_SCRIPT" status --height "$H"
bash "$SNAPSHOT_SCRIPT" finalize --height "$H"
bash "$SNAPSHOT_SCRIPT" prepare-release --height "$H" # 可选：上传前审查 release record
bash "$SNAPSHOT_SCRIPT" publish --height "$H"
bash "$SNAPSHOT_SCRIPT" archive --height "$H" # 可选：仅在需要离线归档时执行
```

日常创建只要求显式提供目标高度。第一次 `create` 会查询并持久化该高度的 canonical block
hash；后续恢复、verify 和 finalize 都复用该记录。如果 hash 已不再 canonical，脚本会失败，
不会静默切换分支。

常用覆盖项：

| 环境变量 | 默认值 | 用途 |
| --- | --- | --- |
| `SNAPSHOT_ROOT` | `~/.usdb/balance-history-snapshot-mainnet` | builder、validation 和 release 根目录 |
| `SNAPSHOT_SIGNER_ID` | `usdb-mainnet-snapshot-v1` | 首次初始化后冻结的 signer ID |
| `SNAPSHOT_KEY_ROOT` | `~/.usdb/secure/snapshot-keys` | key material 目录 |
| `BITCOIN_BIN_DIR` | `/home/bucky/btc/bitcoin-28.1/bin` | Bitcoin Core binaries |
| `BITCOIN_DATA_DIR` | `~/.bitcoin` | 未裁剪主网 datadir |
| `SNAPSHOT_MIN_CONFIRMATIONS` | `144` | 目标高度最小确认数 |
| `SNAPSHOT_POLL_INTERVAL_SECS` | `30` | 等待 stable range 的轮询周期 |
| `SNAPSHOT_CACHE_BUDGET_PERCENT` | `66` | 两类 cache 合计占有效内存的比例 |
| `SNAPSHOT_MAX_MEMORY_PERCENT` | `80` | 整机/cgroup 达到该比例时开始缩减 cache |

脚本子命令：

- `init`：主网节点预检、构建 release binary、首次 keygen、冻结 builder config；
- `preflight`：只读检查网络、IBD/prune、tip、目标 hash/确认数、路径和文件系统；
- `create`：固定目标身份，创建或恢复 exact-height job；
- `resume-verify`：复用已经生成的临时 SQLite，仅执行验证和发布，不打开 RocksDB/indexer；
- `status/list`：查看持久化 builder/job 状态；
- `verify`：重开 immutable artifact 并再次检查 canonical hash；
- `finalize`：verify 并完成所有发布路径都要求的独立 `trust_mode=signed` 安装，不创建 tar；
- `archive`：从已经 finalize 的 immutable artifact 创建可选 tar 和 checksum；
- `prepare-release`：从 pinned target 和 finalized 目录推导全部 provenance，生成 content-addressed record；
- `publish`：幂等 prepare 后，通过 AWS CLI 上传 artifact，最后发布 release record。

`init` 可重复执行，但不会覆盖已有私钥。网络、路径、signer 等冻结字段不一致时仍会要求恢复
原参数或使用新的 `SNAPSHOT_ROOT`；只有 `utxo_max_cache_bytes`、
`balance_max_cache_bytes`、`max_memory_percent` 三个非共识运行参数可以原子刷新，下一次
`create` 会在开始同步前同步更新已有 workspace config。因此 OOM 或人工停止后可以调整内存
预算并从 durable height 继续，不需要删除数据库。`finalize` 也可重复执行，只会复核已有
validation marker；`archive` 单独复核或创建 tar/checksum。

下面第 3 至第 13 节保留展开后的手工步骤，主要用于审计、排障和理解脚本行为。正常生产操作
优先使用上述脚本，避免手工漏掉 hash pin、signed validation 或 release record 校验。

## 3. 生产前检查

### 3.1 检查 Bitcoin Core

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin
BITCOIN_DATA_DIR=/home/bucky/.bitcoin
BITCOIN_CLI="$BITCOIN_BIN_DIR/bitcoin-cli"

"$BITCOIN_CLI" -datadir="$BITCOIN_DATA_DIR" getblockchaininfo
"$BITCOIN_CLI" -datadir="$BITCOIN_DATA_DIR" getnetworkinfo
```

至少确认：

- `chain` 是 `main`；
- `initialblockdownload` 是 `false`；
- `pruned` 是 `false`，本机保留从创世开始的 blk 数据；
- `blocks == headers`，节点已经追上当前 tip；
- `/home/bucky/.bitcoin/.cookie` 对运行 snapshot tool 的用户可读。

该工具只读访问 bitcoind，但会在 builder root 中维护独立 RocksDB workspace。不要把正在提供
服务的 balance-history root 用作 builder root。

### 3.2 检查硬件与目录

```bash
BUILDER_ROOT=/home/bucky/.usdb/balance-history-snapshot-mainnet/builder
RELEASE_ROOT=/home/bucky/.usdb/balance-history-snapshot-mainnet/releases
KEY_ROOT=/home/bucky/.usdb/secure/snapshot-keys

mkdir -p "$BUILDER_ROOT" "$RELEASE_ROOT" "$KEY_ROOT"
df -h "$BUILDER_ROOT" "$RELEASE_ROOT"
df -i "$BUILDER_ROOT" "$RELEASE_ROOT"
free -h
```

`workspace/`、`tmp/` 和 `snapshots/` 都位于 builder root 下。`tmp/` 到最终 artifact 的 rename
依赖同一文件系统，不能把它们拆到不同 mount 后再用软链接规避。正式运行前应为 RocksDB
workspace、临时 SQLite、最终 artifact 和打包文件同时预留空间。

如果还要在同机做 signed validation install，应按以下同时存在的峰值估算，而不是只看最终
snapshot 文件大小：

```text
mutable RocksDB workspace
  + temporary/final snapshot artifact
  + validation install DB
  + optional offline archive
  + filesystem safety margin
```

当前尚未取得主网全量实测大小，因此本文不冻结一个拍脑袋的最小容量值。首次运行前应先把
builder 和 release 目录放到专用盘，并设置磁盘监控和人工停止阈值。

## 4. 构建 release 二进制

从 USDB 仓库执行：

```bash
cd /home/bucky/work/usdb
cargo build --release --manifest-path src/btc/Cargo.toml \
  -p balance-history \
  -p balance-history-snapshot-tool

BALANCE_HISTORY=/home/bucky/work/usdb/src/btc/target/release/balance-history
SNAPSHOT_TOOL=/home/bucky/work/usdb/src/btc/target/release/balance-history-snapshot-tool
```

记录本次使用的代码 revision 和 Bitcoin Core 版本：

```bash
git rev-parse HEAD
"$BITCOIN_BIN_DIR/bitcoind" --version
sha256sum "$BALANCE_HISTORY" "$SNAPSHOT_TOOL"
```

## 5. 生成并保护签名 key

签名对象、文件格式、接收方信任引导和密钥轮换边界见
[Balance-History Snapshot 签名与信任说明](./balance-history-snapshot-signing.md)。本节只记录
主网 snapshot 生成机的操作步骤。

正式分发建议强制 signed install。首次生成 signer：

```bash
SIGNER_ID=usdb-mainnet-snapshot-v1

"$BALANCE_HISTORY" \
  --root-dir "$BUILDER_ROOT/keygen" \
  snapshot-keygen \
  --key-id "$SIGNER_ID" \
  --out-dir "$KEY_ROOT"
```

输出包括：

```text
<SIGNER_ID>.signing-key.json
<SIGNER_ID>.public-key.json
<SIGNER_ID>.trusted-keys.json
```

要求：

- `signing-key.json` 是私钥，只留在受控生产环境，不得随 snapshot 分发；
- `public-key.json` 或 `trusted-keys.json` 应通过独立可信渠道发布；
- 记录 key ID 和公开 key 文件 SHA-256；
- builder config 必须在第一次 create 前绑定 signer，已有 builder root 不接受配置静默变化。

## 6. 创建生产配置

创建 `/home/bucky/.usdb/balance-history-snapshot-mainnet/config/builder.toml`：

```toml
root_dir = "/home/bucky/.usdb/balance-history-snapshot-mainnet/builder/workspace"

[btc]
network = "bitcoin"
data_dir = "/home/bucky/.bitcoin"
rpc_url = "http://127.0.0.1:8332"

[ordinals]
rpc_url = "http://127.0.0.1:"

[electrs]
rpc_url = "tcp://127.0.0.1:50001"

[sync]
local_loader_threshold = 500
batch_size = 128
utxo_max_cache_bytes = 11125359759
balance_max_cache_bytes = 33376079278
max_memory_percent = 80
max_sync_block_height = 4294967295
undo_retention_blocks = 64
undo_cleanup_interval_blocks = 16

[rpc_server]
host = "127.0.0.1"
port = 8292

[snapshot]
trust_mode = "signed"
signing_key_file = "/home/bucky/.usdb/secure/snapshot-keys/usdb-mainnet-snapshot-v1.signing-key.json"
```

`root_dir` 和 `max_sync_block_height` 会由 snapshot builder 在内存中收敛到自己的 workspace
和目标 `H`；配置文件仍应写清楚，方便人工审计。snapshot 创建不依赖 ord/electrs RPC，
但当前配置 schema 要求这两个 section 存在。

上面的 cache 字节数是当前目标机器的示例，不应复制到不同内存限制的主机。正式入口会调用：

```bash
balance-history-snapshot-tool --json memory-plan \
  --cache-budget-percent 66 \
  --max-memory-percent 80
```

并把返回的具体字节数写入 builder config。修改两个 `SNAPSHOT_*_PERCENT` 环境变量后应先重新
执行 `init` 和 `preflight`，确认输出的 memory plan，再执行 `create` 恢复任务。

如果 bitcoind 使用非默认认证，应在 `[btc]` 中显式填写 `auth`。当前机器省略 `auth` 时会
读取 `data_dir/.cookie`。

## 7. 冻结目标高度

确认深度是发布策略，不是工具常量。下面以 `144` 个确认作为操作示例：

```bash
CONFIRMATIONS=144
TIP=$("$BITCOIN_CLI" -datadir="$BITCOIN_DATA_DIR" getblockcount)
H=$((TIP - CONFIRMATIONS + 1))
H_HASH=$("$BITCOIN_CLI" -datadir="$BITCOIN_DATA_DIR" getblockhash "$H")

printf 'tip=%s\nheight=%s\nblock_hash=%s\nconfirmations=%s\n' \
  "$TIP" "$H" "$H_HASH" "$CONFIRMATIONS"
```

这里按 Bitcoin Core 常用口径把 tip block 计为 1 confirmation，因此确认数公式为
`TIP - H + 1`。

把 tip、`H`、`H_HASH`、确认深度、选择时间和代码 revision 写入发布记录。不要在 snapshot
仍在构建时把同一 builder root 改为另一个目标高度。

## 8. 创建、恢复与观察进度

首次运行：

```bash
CONFIG=/home/bucky/.usdb/balance-history-snapshot-mainnet/config/builder.toml
CREATE_REPORT="$RELEASE_ROOT/create-${H}-${H_HASH}.json"

"$SNAPSHOT_TOOL" \
  --root-dir "$BUILDER_ROOT" \
  --json \
  create \
  --height "$H" \
  --expected-block-hash "$H_HASH" \
  --config "$CONFIG" \
  --poll-interval-secs 30 \
  >"$CREATE_REPORT"
```

发生正常错误、主机重启或进程中止后，先读取 job 状态。`prepare/syncing/sealed/building`
继续使用完全相同的 `H` 和 `H_HASH` 重跑 `create`；workspace 的 RocksDB 同步进度会保留，
未完成的 SQLite 导出会从头重建：

```bash
"$SNAPSHOT_TOOL" \
  --root-dir "$BUILDER_ROOT" \
  --json \
  create \
  --height "$H" \
  --expected-block-hash "$H_HASH" \
  --poll-interval-secs 30 \
  >"$CREATE_REPORT"
```

如果状态为 `verifying`，禁止再次执行 `create`。此时临时目录中的 DB、manifest 和签名已经
生成，应显式恢复验证：

```bash
"$SNAPSHOT_TOOL" \
  --root-dir "$BUILDER_ROOT" \
  --json \
  resume-verify \
  --height "$H" \
  --expected-block-hash "$H_HASH" \
  >"$CREATE_REPORT"
```

`resume-verify` 不打开 mutable workspace，也不持有 RocksDB/indexer cache。首次 full verify
中断时会从文件 hash 开始重新验证，但不会重新生成 SQLite；如果 `complete.json` 已经写入，
则直接继续原子发布。`status` 中的 `verification.phase`、`phase_started_at` 和 `heartbeat_at`
用于区分长时间磁盘扫描与无进展进程。

单独查看状态：

```bash
"$SNAPSHOT_TOOL" --root-dir "$BUILDER_ROOT" --json status --height "$H"
"$SNAPSHOT_TOOL" --root-dir "$BUILDER_ROOT" --json list
```

如果状态中仍有 `active_job_height`，必须先按其 stage 使用 `create` 或 `resume-verify` 完成，
不能直接开始 `H+1`。

## 9. 完成后复核

```bash
VERIFY_REPORT="$RELEASE_ROOT/verify-${H}-${H_HASH}.json"

"$SNAPSHOT_TOOL" \
  --root-dir "$BUILDER_ROOT" \
  --json \
  verify \
  --height "$H" \
  --block-hash "$H_HASH" \
  >"$VERIFY_REPORT"

CURRENT_HASH=$("$BITCOIN_CLI" -datadir="$BITCOIN_DATA_DIR" getblockhash "$H")
test "$CURRENT_HASH" = "$H_HASH"
```

比较 create/verify 的 `file_sha256`、snapshot ID、BTC block hash、UTXO count、balance-history
count、block-commit count 和 script-registry count。任何字段不一致都不得发布。

从 create report 解析 artifact 路径：

```bash
ARTIFACT_DIR=$(python3 - "$CREATE_REPORT" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["artifact_dir"])
PY
)
ARTIFACT_PATH="$BUILDER_ROOT/$ARTIFACT_DIR"

find "$ARTIFACT_PATH" -maxdepth 1 -type f -printf '%f\n' | sort
sha256sum "$ARTIFACT_PATH"/*
```

完整 artifact 应包含：

```text
snapshot_<H>.db
snapshot_<H>.manifest.json
snapshot_<H>.manifest.sig
complete.json
```

当前 SQLite snapshot schema 为 `version=3`，manifest schema 为
`balance-history-snapshot-manifest:v3`。两者除状态与文件哈希外还显式冻结：

- `balance_query_floor = H`：安装后可完整回答的最早 at-or-before 点余额高度；
- `history_query_floor = H + 1`：安装后可完整回答的最早精确 delta/历史区间高度；
- `db_identity`：schema/data-model version、service、BTC network 和 genesis hash。

snapshot 内保留的 `H` 之前 block commit 只用于审计，不能作为这些历史余额状态仍可查询的声明。

### 9.1 RocksDB Identity 与重建边界

snapshot 的发布身份和安装后的 RocksDB identity 是两层不同约束。发布身份继续由
`network + H + BTC block hash + snapshot ID` 确定；每个 balance-history RocksDB 还会在
`meta/db_identity` 中冻结：

- identity serialization version；
- service name；
- RocksDB schema/key encoding version；
- balance-history data model version；
- BTC network 和对应 genesis hash。

数据库每次打开都会比较完整 identity。network/genesis、schema 或 data model 任一不一致都
fail closed；非空但没有 identity 的旧数据库也会被拒绝。当前仍处于开发阶段，不提供旧数据
迁移：应移动或删除旧 RocksDB 后从创世重建，或者向空 root 安装使用当前代码生成的 snapshot。

`snapshot ID` 还承诺 registry 固定的 `stable_lag`。当前 mainnet/regtest 均为 `10`；使用
旧 `stable_lag=5` 生成的 manifest 即使文件哈希和内部 snapshot ID 自洽，也会在替换 live DB
前因 expected state-ref 不一致而被拒绝。已有构建工作区必须用当前 binary 重新完成 create、
verify 和 signed install 演练；禁止手工修改旧 manifest 后重新签名。

当前 data model 为
`balance-history-data-model:bip30-generations-core-unspendable-v2`。其中 unspendable 与
Bitcoin Core `CScript::IsUnspendable()` 完全一致：首字节为 `OP_RETURN`，或 script 长度大于
10,000 bytes 的输出都不进入 UTXO、余额历史和辅助 script registry。不要使用
rust-bitcoin deprecated `is_provably_unspendable` 替代该规则；后者还包含 illegal opcode，
语义更宽。该变化会影响 rolling block commit，因此旧 data-model snapshot 必须重建。

SQLite meta 与 manifest 必须携带完全相同的 source DB identity；signed manifest 的文件哈希和
签名同时覆盖 SQLite 文件及 manifest identity。snapshot install 还会先创建新的 staging
RocksDB，写入接收节点当前配置的 expected identity，并要求 expected、SQLite、manifest、
staging 四者一致后才原子替换 live DB。旧 SQLite schema、旧 manifest 或 identity mismatch
都不能通过重新安装被标记成当前数据模型。

### 9.2 用接收方信任策略试安装

`snapshot-tool verify` 会重开 artifact 并核对 DB、manifest、state-ref、计数和文件 hash；正式
发布前还必须在独立 validation root 中执行一次 signed install，以接收方视角验证 detached
signature 和 trusted key。

创建 `/home/bucky/.usdb/balance-history-snapshot-mainnet/validation/manual/config.toml`：

```toml
root_dir = "/home/bucky/.usdb/balance-history-snapshot-mainnet/validation/manual"

[btc]
network = "bitcoin"
data_dir = "/home/bucky/.bitcoin"
rpc_url = "http://127.0.0.1:8332"

[ordinals]
rpc_url = "http://127.0.0.1:"

[electrs]
rpc_url = "tcp://127.0.0.1:50001"

[sync]
local_loader_threshold = 500
batch_size = 128
max_sync_block_height = 4294967295
undo_retention_blocks = 64
undo_cleanup_interval_blocks = 16

[rpc_server]
host = "127.0.0.1"
port = 8293

[snapshot]
trust_mode = "signed"
trusted_keys_file = "/home/bucky/.usdb/secure/snapshot-keys/usdb-mainnet-snapshot-v1.trusted-keys.json"
```

validation root 必须是独立目录，不能指向 builder workspace 或在线服务目录：

```bash
VALIDATION_ROOT=/home/bucky/.usdb/balance-history-snapshot-mainnet/validation/manual
SNAPSHOT_FILE="$ARTIFACT_PATH/snapshot_${H}.db"

"$BALANCE_HISTORY" \
  --root-dir "$VALIDATION_ROOT" \
  install-snapshot \
  --file "$SNAPSHOT_FILE"
```

只有该命令在 `trust_mode = "signed"` 下成功后才能继续发布。此 validation root 可保留用于
发布审计，也可以在确认没有进程使用后删除并重建；不要把它当成生产 builder 的增量基础。

## 10. 发布和可选离线归档

Balance-history binary、public trusted-key catalog 和 snapshot 的整体发布编排见
[Balance-History 发布与 Snapshot 分发](../publish/balance-history-release-and-snapshot-distribution.md)。
S3-compatible object storage 的 direct-file 发布、content-addressed record 和断点下载见
[Snapshot 对象存储发布与安装](../publish/balance-history-snapshot-object-storage.md)。

默认节点分发使用 direct-file release record，不创建 tar：

```bash
bash "$SNAPSHOT_SCRIPT" publish --height "$H"
```

不要发布 mutable workspace、job state、签名私钥或 validation DB。Direct-file 发布集合由
content-addressed record 固定，包含 snapshot DB、manifest、signature 和 `complete.json` 的逐文件
size/SHA-256/object key，以及 trusted catalog hash 和 producer revision。

只有离线介质或冷备明确要求单文件时才额外执行：

```bash
bash "$SNAPSHOT_SCRIPT" archive --height "$H"
```

可选离线归档交付集合包括：

- snapshot tar 和 tar 的 SHA-256；
- signer 的 public key 或 trusted-keys 文件；
- `H`、BTC block hash、snapshot ID 和 manifest hash；
- 生产代码 revision、Bitcoin Core 版本和生成时间；
- 接收方应使用的 trust mode。

tar hash 只用于可选归档的传输完整性；真正的内容信任链是可信渠道提供的 public key、manifest detached
signature，以及 manifest 内固定的 snapshot DB SHA-256 和 state-ref。

面向节点的默认分发不要求下载 tar。`snapshot_distribution.py prepare` 从本节 artifact、trusted
catalog 和 `finalize` 生成的 `signed-install-complete.json` 建立逐文件 release record；`upload` 使用
AWS CLI 发布原始 DB/sidecars，并最后发布 content-addressed record。这样不会在接收方产生 tar 与解包 DB
同时存在的额外峰值空间。

## 11. 接收方安装

默认接收方应按对象存储文档执行 `usdb-node snapshot install --record-url ...`。以下步骤只用于可选
离线 tar 交付；接收方应使用新的或已停止服务的 balance-history root，并保持 DB、manifest 和
signature 三个文件相邻：

```bash
TARGET_ROOT="$HOME/.usdb/balance-history"
DOWNLOAD_DIR=/path/to/usdb-snapshot-downloads
H=<release-height>
H_HASH=<release-block-hash>
PACKAGE_NAME="balance-history-mainnet-${H}-${H_HASH}.tar"
INCOMING="$DOWNLOAD_DIR/balance-history-mainnet-${H}-${H_HASH}"
TRUSTED_KEYS=/etc/usdb/snapshot-keys/usdb-mainnet-snapshot-v1.trusted-keys.json

mkdir -p "$INCOMING"
(
  cd "$DOWNLOAD_DIR"
  sha256sum -c "$PACKAGE_NAME.sha256"
)
tar -C "$INCOMING" -xf "$DOWNLOAD_DIR/$PACKAGE_NAME"
```

接收方 `config.toml` 必须使用主网 BTC 配置并强制 signed trust：

```toml
[snapshot]
trust_mode = "signed"
trusted_keys_file = "/etc/usdb/snapshot-keys/usdb-mainnet-snapshot-v1.trusted-keys.json"
```

不要把生产方的 `signing_key_file` 放入接收方配置。停止服务后安装：

```bash
SNAPSHOT_FILE="$INCOMING/snapshot_${H}.db"
test -f "$SNAPSHOT_FILE"

"$BALANCE_HISTORY" \
  --root-dir "$TARGET_ROOT" \
  install-snapshot \
  --file "$SNAPSHOT_FILE"
```

安装器会先在 staging DB 中验证 manifest、可信签名、文件 hash 和 state-ref，再原子替换目标
DB。安装成功后启动服务，让它从 `H` 继续同步到当前 stable height。

## 12. 启动后验收

服务启动并开放 RPC 后检查：

```bash
RPC_URL=http://127.0.0.1:8292

curl -s -X POST "$RPC_URL" -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"get_snapshot_provenance","params":[]}'

curl -s -X POST "$RPC_URL" -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"get_snapshot_info","params":[]}'

curl -s -X POST "$RPC_URL" -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"get_readiness","params":[]}'

curl -s -X POST "$RPC_URL" -H 'content-type: application/json' \
  --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"get_state_ref_at_height\",\"params\":[{\"block_height\":${H}}]}"
```

至少确认：

- provenance 的 `verification_state` 是 `signature_verified`；
- `signature_present` 和 `signature_verified` 都是 `true`；
- `signing_key_id` 与发布记录一致；
- provenance 的 `installed_block_height`、snapshot ID 和 snapshot file SHA-256 与发布记录一致；
- `get_state_ref_at_height(H)` 的 stable block hash 和 snapshot ID 与发布记录一致；
- 追块完成后 readiness 为 consensus ready；
- 抽样余额、历史 state-ref 和 live UTXO 查询正常。

## 13. 后续增量高度

只有 `H` 已完成并通过验证后，才能用同一 builder root 创建更高目标。`H+N` 会复用 workspace
进行增量同步，但仍生成独立的按高度和 block hash 管理的 immutable artifact。

不要修改已初始化 builder root 的 config。需要更换网络、signer、数据目录或关键 sync 配置时，
使用新的 builder root，并保留旧 root 和发布记录供审计。

## 14. 当前仍需人工冻结的发布策略

工具已经保证 exact-height、完整 UTXO、可恢复构建、hash/signature/state-ref 校验和原子安装，
但以下内容仍属于正式发布流程，而不是代码内的固定共识参数：

- 主网 snapshot 的确认深度；
- signer key 的轮换、托管和撤销方式；
- artifact 保留周期和镜像分发位置；
- 正式硬件可接受的最长生成时间、峰值内存和磁盘空间阈值；
- 深层 BTC reorg 后已发布 snapshot 的撤回和替换流程。

当前机器已记录 100K、250K 和 1M regtest capacity 指标。1M 基线说明当前二进制和机器能稳定
完成更大规模 export/install，但仍不替代主网 artifact 本身的完整验证和物理 I/O 监控。
