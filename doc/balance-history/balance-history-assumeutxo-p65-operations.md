# P6.5：整套服务验收与主网长任务

日期：2026-09-12。P6.4 已提交为 `7f52ee2`；本批增加可重复运行的整套服务验收及只读主网锚点核对工具。

**独立 regtest 的 Core → 原生 balance-history → indexer 链路验收通过。主网原生重导入、相同锚点复核与性能记录尚待执行，因此 P6.5 尚未全部验收完成。**
本批修复了整套复验发现的 indexer 重组锚点覆盖竞态；commit算法、数据库格式、镜像和部署默认gate保持原契约。
本批已提交为 `22cf06c`。部署改造已另行进入[P7 入口接入与后续计划](./balance-history-assumeutxo-p7-deployment-plan.md)，可与本手册主网长任务并行。
服务基于P6.4提交及本节修复构建，测试用交易生成入口仅在测试构建中启用。

## 1. 实际运行的链路

[运行器](../../tests/run_assumeutxo_p65_services.py)在唯一 `/tmp/usdb-p65-services-*` 下启动：

- Bitcoin Core 31.1：`regtest/txindex=0/prune=0`，禁止外部连接，使用新建数据目录。
- 原生 BH：已核验 B=101 的真实 Core UTXO 文件及旧 C(B) 检查点，G=102；LocalLoader 阈值0，覆盖新原生取块路径。
- 参考 BH：从0完整重放，LocalLoader 阈值4294967295，使用 RPC 取块。
- 两个真实 indexer：均从 G=102 开始，分别连接以上 BH；使用 `inscription_source=bitcoind`，关闭 Ord shadow，不启动外部 Ord。

[交易生成器](../../tests/assumeutxo_service_fixture.rs)构造真实 Taproot USDB mint 和竞争分支的转移交易，
由 Core 实际验证；没有使用 fixture inscription source 代替索引流程。
原生 indexer 经[故障代理](../../tests/common/assumeutxo_services.py)访问 Core，代理平时透传真实 RPC，
仅在指定阶段返回缺块或移除 verbosity-3 的 prevout。它不删除 Core blk/rev 文件。

这里的 Core 从固定区块前缀启动并分段接收后续块，**没有在 regtest 调用 `loadtxoutset` 创建两个 chainstate**。
该部分沿用独立的 P2 验证；本节证明真实服务在渐进供块条件下的行为，不推断公网下载、主网冷启动或后台验证耗时。
Go 链、control-plane、外部 Ord、Docker/node-kit 的部署交付仍属于 P7。

## 2. 验收结果

| 场景 | 实测结果 |
| --- | --- |
| BTC tip=101，G=102 尚未到达 | BH 完成导入后停在私有 staging，`waiting_for_blocks/published=false`；indexer RPC 可访问，query/consensus 均为false |
| 等待 G 时强制中断 BH | 相同目录重新启动后恢复等待，随后封存 G 并进入正常同步 |
| 103块 mint 缺 undo | indexer 保持持久化高度102，pending=103，query=true、consensus=false；核对工具拒绝验收 |
| 保持缺 undo 故障并强制中断 indexer | 重启后再次实际读到缺失 prevout，仍不能推进；恢复供数后完整处理103，没有重复 mint |
| 104块转移缺块 | indexer 保持103，pending=104、consensus=false；仍能查询103已提交的矿工证 |
| 正常链102–108 | 每块 BH commit/state-ref、pass commit、历史 local/system state-ref 均与从0重放一致；两地址余额、pass snapshot、能量查询一致 |
| 真实 mint/转移 | 103铸造 Active pass；104从 A 转移到 B 后 Dormant，satpoint 按两输入金额正确移动 |
| 余额归零的脚本反查 | A在104余额归零后仍返回地址和script，`found_overlay`；无需 registry sidecar，覆盖明确为 `post_snapshot_only` |
| 查询下界 | 101余额、101 state-ref、102精确delta均返回 `STATE_NOT_RETAINED (-32048)`；102及以后的余额查询正常 |
| 封存后强制中断两个原生服务并移除本轮源快照副本 | 重启不依赖源文件，108全部状态/承诺与中断前和参考服务相同 |
| 替换104起分支 | BH和indexer都回滚到103，追至稳定109；102–109全部承诺和查询与参考一致 |
| 替代转移保持同一 owner A | Pass为Active，余额700000000 sat，109的有效能量41000，验证了非零能量及重组后的状态差异 |
| 历史交易索引 | Core `getindexinfo={}`；原生indexer代理记录 `getrawtransaction=0`，实际使用 `getblock(hash,3)` |

修复后连续三轮全部通过，汇总位于 `/tmp/usdb-p65-fixed-runs.json`：

| 轮次 | 完整报告 | 包含构建、故障重试和退出的耗时 |
| --- | --- | --- |
| 1 | `/tmp/usdb-p65-services-ae6lmgiw/result.json` | 30.757秒 |
| 2 | `/tmp/usdb-p65-services-hjh5gv2_/result.json` | 27.682秒 |
| 3 | `/tmp/usdb-p65-services-_5u0x2p9/result.json` | 27.684秒 |

每轮正常链48项、替代分支55项返回值比较通过，封存后重启重复核对正常链。
原始区块、配置、日志及二进制/测试源码hash保留在各自目录。脚本返回值比较之外，
还有明确的owner/satpoint/状态/余额/非零能量断言，锚点采集前后readiness也与所采commit核对。
三轮所有历史高度的BH、pass与snapshot互相绑定关系另行复核通过，记录于 `/tmp/usdb-p65-history-links.json`。

第三轮采样 `VmHWM`：消费Core约61.9MiB、原生BH约48.4MiB、原生indexer约23.4MiB；
停止后的整个测试目录文件逻辑大小约38.5MiB。它们仅属于403个基线Coin的小型regtest，
不是主网内存/磁盘预算，也不是实际磁盘分配或运行峰值。构建缓存、页缓存、全主机IO和下载量不包含在这些数值中。

服务二进制 SHA-256：

```text
bitcoind       986e63b3c8770f08d0059820ad3dd085d1ab9e1bea23946c243f858a06888a08
balance-history 6b345ab8636bf2afaeafaf874f6ca429d6ec8f5909585c57c2b76045acdab325
usdb-indexer    f273cbdb8f63a6c5636f3932603195f391447d281a2c1ca330fdcb6eeef4fc22
```

修复后完整indexer回归327通过、9忽略（其中交易生成测试由整套运行器显式执行）；
workspace check、Clippy（all-targets/all-features、`-D warnings`）、格式、release构建及两个运行器CLI帮助检查通过。
工具拒绝不可用RPC、优化模式运行及覆盖旧报告的检查也通过。

### 2.1 验收捕获并修复的重组竞态

失败现场保留于 `/tmp/usdb-p65-services-i3xqgv5p/result.json`。在参考的全量重放组中，
indexer已经把104–108重放到新分支，但收尾使用了同步开始时轮询缓存中的旧108 snapshot，
将新108历史锚点覆盖成旧分支；随后109正常提交，当前readiness恢复就绪，108历史state-ref仍错误。
SQLite与两分支区块证据确认：108的pass commit属于新分支，历史snapshot却属于旧分支，导致local/system commit不一致。
这是共享indexer收尾路径的竞态，不是Core快照UTXO内容或nativeBH余额差异。

修复在 `persist_balance_history_snapshot_anchor` 写入前，将轮询snapshot与随该块持久化的锚点逐项匹配
（高度、BTC hash、BH commit、stable lag、commit协议和hash算法）。缺失或不匹配时拒绝覆盖，等待刷新后的snapshot；
已提交的块与历史锚点保留，后续重试可直接采用正确锚点。

[确定性回归](../../tests/assumeutxo_stale_anchor.rs)让状态轮询保持旧快照、逐块供数返回新分支：
修复前错误地返回成功，见 `/tmp/usdb-p65-stale-anchor-before.log`；修复后对旧分支和仅hash不符两种情况均拒绝，
历史锚点保持正确，刷新后采用成功且不重复处理区块。完整327项回归同时通过。
上表第1、2轮真实服务日志实际触发了该保护：拒绝旧108快照后自动重试并追至109，所有历史高度对拍通过。
修复前的偶发成功运行不作为修复版本的最终验收证据。

## 3. 重复运行

在仓库根目录执行：

```bash
CARGO_BUILD_JOBS=2 cargo build --offline --locked --release \
  --manifest-path src/btc/Cargo.toml -p balance-history -p usdb-indexer \
  --bin balance-history --bin usdb-indexer
PYTHONDONTWRITEBYTECODE=1 CARGO_BUILD_JOBS=2 python3 tests/run_assumeutxo_p65_services.py \
  --bitcoind /data/btc/bitcoin-31.1/bin/bitcoind
```

可用 `--balance-history`、`--indexer` 指定其他待验收二进制。运行器自动编译并调用专用Rust交易生成测试，
输出唯一结果目录；只有全部断言通过才写 `status=pass` 并退出0。不能用 `python -O` 或 `PYTHONOPTIMIZE` 运行。
成功、失败和Ctrl+C均收尾本轮子进程；数据和日志保留，不操作既有服务或删除其目录。
强制中断步骤使用SIGKILL，仅针对运行器刚创建的测试进程。

区块生成时间参与hash，每次运行的G/hash及其后commit可能不同；比较基于同一次运行的相同canonical链，
不能把两次运行随机生成的区块hash直接互比。B=101之前的前缀、源快照及旧检查点固定。

## 4. 主网待执行步骤

本轮检查时 `/home/bucky/.bitcoin/.cookie` 不存在；主机上发现的Core/BH/indexer均为本轮临时regtest。
未自行启动旧主网节点。磁盘当时根分区可用223GiB、`/data`可用359GiB；这是时点读数，执行前需要重新核对。

主网不需要再次做 P4 的旧37GB SQLite核验，也不需要恢复已删除的旧snapshot workspace。
需要执行的是**生产原生入口**的935000导入、重放、独立状态校验，以及下游实际服务就绪。

### 步骤1：准备供块源和独立配置

按[P6.2第4节](./balance-history-assumeutxo-p62-operations.md#4-主网长任务准备独立目录与配置)创建独立root与配置，
保留同一终端的 `P62_ROOT/P62_SOURCE/P62_BTC_DATA/P62_BTC_RPC/P62_RPC_PORT/P62_BIN`。
使用 `prune=0` 的Core 31.1供块源；Core继续官方后台验证。原P2已裁剪实验节点不能当作完整重放区块源。

已有符合当前checkpoint-v1身份的原生目录时复用它；不要为同一验收再创建第二份主网大库。
旧实验v2或P4验证库不是这种可复用的原生服务目录。

### 步骤2：核对RPC条件

```bash
test -r "$P62_SOURCE"
test -r "$P62_BTC_DATA/.cookie"
df -h / /data
PYTHONDONTWRITEBYTECODE=1 python3 docker/scripts/tools/check_bitcoin_block_data.py \
  --url "$P62_BTC_RPC" --cookie-file "$P62_BTC_DATA/.cookie" \
  --expected-chain main --height 963800 \
  --block-hash 000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551
```

该工具只证明指定块的历史输入可用。若G尚未到达，可以先执行步骤3的导入，等Core供块后再补本步；
最终发布仍需tip至少963810。RPC和cookie路径以实际节点为准，cookie缺失时不要继续照抄旧路径。

### 步骤3：安排原生导入长任务

```bash
/usr/bin/time -v -o "$P62_ROOT/bootstrap-time.txt" \
  "$P62_BIN" --root-dir "$P62_ROOT" bootstrap \
  > "$P62_ROOT/bootstrap-result.json" 2> "$P62_ROOT/bootstrap.log"
```

这是本批最耗时步骤：约1.64亿Coin、28,800块重放、投影扫描和独立余额聚合；尚无主网原生入口实测，
不提供未经测量的ETA。另一个终端观察 `bootstrap-progress.json`、`bootstrap.log` 和磁盘剩余空间。
保留整个日志及 `time -v`；磁盘峰值、读写量和各阶段耗时需另行采样，结束后的 `du` 不是峰值证据。
中断后沿用同一root恢复；重新运行前用不同的日志文件名保留前次证据。

### 步骤4：启动独立BH，固定在G

```bash
ss -ltn 'sport = :28341'
"$P62_BIN" --root-dir "$P62_ROOT" --skip-process-lock --max-block-height 963800
```

此命令需要单独终端持续运行。保持P6.2配置中的 `sync.max_sync_block_height=963800`，
先完成同锚点检查，再调整追平上限。也可直接用本步正常启动替代步骤3，启动中自动执行bootstrap；
同一个root只运行一个写入者。

### 步骤5：配置并启动独立indexer

回到持有P62变量的终端，建立独立indexer目录；28342须空闲：

```bash
export P65_INDEXER_ROOT=$(mktemp -d /data/usdb-assumeutxo-validation/p65-indexer.XXXXXX)
export P65_INDEXER_PORT=28342
ss -ltn 'sport = :28342'
python3 - <<'PY'
import json, os
from pathlib import Path
root = Path(os.environ["P65_INDEXER_ROOT"])
config = {
    "bitcoin": {
        "network": "bitcoin", "rpc_url": os.environ["P62_BTC_RPC"],
        "auth": {"CookieFile": str(Path(os.environ["P62_BTC_DATA"]) / ".cookie")}
    },
    "ordinals": {},
    "balance_history": {"rpc_url": "http://127.0.0.1:" + os.environ["P62_RPC_PORT"]},
    "usdb": {
        "genesis_block_height": 963800,
        "inscription_source": "bitcoind", "inscription_source_shadow_compare": False,
        "rpc_server_host": "127.0.0.1", "rpc_server_port": int(os.environ["P65_INDEXER_PORT"])
    }
}
with (root / "config.json").open("x") as output:
    json.dump(config, output, indent=2)
print(root)
PY
/home/bucky/work/usdb/src/btc/target/release/usdb-indexer \
  --root-dir "$P65_INDEXER_ROOT" --skip-process-lock
```

indexer可以早于BH就绪启动。没有外部Ord依赖；等待期间 `consensus_ready=false` 是预期状态。
本轮隔离验收已证明该启动顺序，无需在主网再次强制杀进程或删除9GB源快照。

### 步骤6：采集主网验收结果

在第三个终端执行（output必须是尚不存在的路径）：

```bash
cd /home/bucky/work/usdb
PYTHONDONTWRITEBYTECODE=1 python3 tests/check_assumeutxo_p65_mainnet.py \
  --bh-port 28341 --indexer-port 28342 \
  --output /tmp/usdb-p65-mainnet-anchor-first.json
```

[核对工具](../../tests/check_assumeutxo_p65_mainnet.py)只读RPC，要求两个服务均在963800并且consensus-ready；
核对封存记录中的P4全量UTXO/余额投影、内置C(935000)、旧C(963800)、查询floors，
并核对BH → pass → local → system的同高度引用及采集前后身份。若尚未就绪或任何字段不符则退出非0并保留失败报告。
工具读取封存时生成的独立校验记录，**不会再次扫描主网全库**；也不把单实例内部引用一致当成独立主网indexer对拍。

### 步骤7：正常重启并比较

用各自前台终端Ctrl+C正常停止两个验收服务；保持相同配置、root及G上限重启。
重复步骤6但将输出改为 `/tmp/usdb-p65-mainnet-anchor-restart.json`。
比较两份报告的 `bootstrap`、`anchor.balance_history_commit`、`anchor.balance_history_state_ref`、
`anchor.indexer_state_ref`、`anchor.pass_commit`、`anchor.local_state`、`anchor.system_state`，应完全相同。
readiness中的瞬时进度/message不参与重启前后相等比较。日志不得出现重新导入或重新扫描源快照的阶段。

### 步骤8：后续追平和性能对照

通过G验收后，按P6.2手册放开BH上限，indexer自动跟随稳定高度。
若要比较主网后续pass/local/system值，需要同G、同算法版本、同高度/hash的独立参考indexer；
不能把不同业务G或不同inscription来源的实例直接对拍。实际主网区间可能没有mint，非空业务路径以本节regtest证据为准。
LocalLoader性能对照见[P6.3手册](./balance-history-assumeutxo-p63-operations.md)，另行安排磁盘和运行时间。

完成后归档：供块Core版本/chainstates、原生bootstrap阶段耗时和峰值资源、上述两份主网报告、
继续追平区间及对应承诺、参考数据来源。P7再处理镜像、默认inscription来源、旧快照部署流程和启动gate。
