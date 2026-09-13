# AssumeUTXO P7：部署改造计划与入口接入

日期：2026-09-12。P6.5 服务验收与重组锚点修复已提交为 `22cf06c`。
更新：2026-09-13，P7.1 已提交为 `6c3836d`；P7.2 镜像与 Core 导入编排已实现，见[P7.2 操作及验收](./balance-history-assumeutxo-p72-operations.md)。
P7.2 基础已提交为 `eb6c8c2`；后续补充[自有源及签名分发](./balance-history-bitcoin-artifact-distribution.md)，正式源与生产公钥待发布环境配置。

## 1. 当前结论与剩余范围

**可以在主网长任务进行期间并行开发部署脚本。** P6 已实现原生 bootstrap、可信旧 commit 检查点、
按需 LocalLoader、渐进重放和不依赖全量 txindex 的 indexer 输入查询。
独立 regtest 的 Core/BH/indexer 正常同步、真实 inscription、重启和重组验收已通过。
主网生产入口的重新导入、同锚点复核、资源与性能记录仍需完成，不能将原型 P4 的主网结果替代该验收。

| 工作 | 现状 | 后续验收 |
| --- | --- | --- |
| P6 主网验收 | 操作手册与探针已具备，尚未执行长任务 | 原生导入至 G、UTXO/余额摘要、v1 commit、服务同锚点、重启恢复及资源记录 |
| P6.3 LocalLoader 性能 | 稀疏/乱序/XOR/追加/回退正确性已通过 | 主网同区间 RPC 与本地读取的状态一致及实际耗时对照 |
| P3 原目录升级 | 独立 31.1 二进制已验证；原 28.1 数据目录升级尚未执行 | 正常停止旧进程、按升级手册切换、复核链状态/索引/读块/重启 |
| P7.1 服务配置和入口 | 已提交 `6c3836d`，入口回归通过 | 原生配置身份、启动行为、错误配置阻断、旧模式回归 |
| P7.2 Core 镜像与快照编排 | 已实现；本地镜像、隔离容器及HTTPS/RPC恢复通过，主网长任务待安排 | 31.1 供应链校验、下载恢复、loadtxoutset 生命周期、前后台独立状态 |
| P7.3 node-kit 与整套编排 | 待实现 | bundle/Compose/controller、资源阶段、readiness、chain/control-plane 集成 |
| P7.4 安装/升级与发布 | 待执行 | 新机器冷启动、故障恢复、升级路径、CI、镜像及发布身份更新 |

P6 主网手册：[P6.2 原生导入](./balance-history-assumeutxo-p62-operations.md)、
[P6.3 LocalLoader 对照](./balance-history-assumeutxo-p63-operations.md)、
[P6.5 服务与资源复核](./balance-history-assumeutxo-p65-operations.md)。
可合并一次原生导入产生的证据，避免按章节反复导入同一主网快照；比较时必须固定同一 G/hash 和代码身份。

## 2. 新部署流程

1. 安装经过校验的 Core 31.1 与 USDB 服务版本，固定 Bitcoin 网络、快照 B/hash、业务 G/hash，要求 G>=B。
2. 下载或接收原始 UTXO 文件，校验完整文件身份。文件以只读方式供 Core 与 BH 使用。
3. 新 Core 数据目录运行 `prune=0`，取得所需 headers 后加载快照；已激活或已完成历史验证的目录按链状态恢复，避免重复导入。
4. Core 快照链激活后启动 BH 导入。Core 前台追平、后台历史验证和 BH 导入可以并行；
   BH 按稳定区块到达逐段重放至 G，积压超过阈值时使用 LocalLoader，缺本地块则回退 RPC。
5. indexer 可以先启动并重试上游访问；它的共识就绪仍取决于 BH 发布、消费块/undo 可用及对应状态承诺。
   不以端口开放或 Core 前台高度作为整套服务就绪的替代。
6. BH 封存 G 后提供余额与后续历史；chain/control-plane 按实际下游 readiness 启动。
7. Core 官方后台历史验证继续进行，独立展示进度。每个新快照节点仍执行该过程；USDB 前台可用不必等待它结束。

快照 B 的 Coin 集合没有 B 自身的交易及 undo。G=B 时若 indexer 需要这些数据，需要等待该块数据可用；
G>B 的业务块位于快照链前台验证区间。部署不得通过修改 genesis 或省略缺失输入绕过数据可用性检查。

## 3. P7.1 本批实现

新增 `docker/scripts/tools/assumeutxo_bootstrap.py`，由原有 TOML/JSON 渲染脚本在
`SNAPSHOT_MODE=assumeutxo` 时调用；其他模式仍走既有路径。

- 从 Rust 的 `bootstrap/checkpoints/mainnet-935000.json` 读取同一快照身份，服务镜像复制这份小型目录。
  配置只输出 snapshot identity；C(B)/D(B) 仍由 Rust 内置检查点匹配取得，没有新增外部 commit 注入入口。
- 明确校验 B、G、G/hash、来源、批量参数和配置组合。仅支持当前已具备可信检查点的 Bitcoin 主网 B=935000。
- 原子替换配置文件，权限 `0600`；正确转义 JSON/TOML 中的凭据和 Unicode，不在渲染日志输出凭据。
  错误配置保留原文件并在启动进程前退出。
- `snapshot_loader.sh`、`script_registry_installer.sh` 在原生模式仅校验配置后退出，
  不调用 `install-snapshot`，不创建旧 core 安装标记，也不安装 registry sidecar。
- `start_balance_history.sh` 生成原生配置后直接执行正常 BH 服务入口，由 Rust 管理导入/重放/封存/恢复。
- `start_usdb_indexer.sh` 原生模式省略固定时限的上游 TCP 等待，直接启动已有重试和 readiness 机制；
  inscription source 固定为 `bitcoind`，不启用 Ord shadow compare。

这批是**服务入口基础接入**。`usdb-node setup/up` 的模式校验、Compose 环境变量/挂载及 controller 尚未接入新模式；
目前不能仅在现有 `node.env` 中设置 `SNAPSHOT_MODE=assumeutxo` 就得到完整的新部署流程。
上述为P7.1边界。P7.2已新增31.1镜像与独立Core overlay；现有网络bundle、node-kit默认模式和运行中的节点仍未切换。

### 配置约定

| 环境变量 | 含义/默认值 |
| --- | --- |
| `SNAPSHOT_MODE` | 显式选择 `assumeutxo` |
| `BTC_NETWORK` | `bitcoin`，当前入口不支持其他 Bitcoin 网络 |
| `BH_ASSUMEUTXO_BASE_HEIGHT` | 默认935000，目前唯一支持的主网检查点 |
| `USDB_GENESIS_BLOCK_HEIGHT` | 必填 G，范围 B..4294967294，BH 与 indexer 共用 |
| `BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH` | BH 必填 G 的小写64位 BTC hash，G=B 时必须等于快照基线 hash |
| `BH_ASSUMEUTXO_SNAPSHOT_FILE` | BH 必填，服务内可见的绝对路径；首次导入由 Rust 校验文件及承诺 |
| `BH_ASSUMEUTXO_IMPORT_BATCH_SIZE` | 默认20000，范围1..1000000 |
| `BH_ASSUMEUTXO_REPLAY_BATCH_SIZE` | 默认20，范围1..100 |
| `BH_SYNC_LOCAL_LOADER_THRESHOLD` | 默认500；继承按积压量启用的语义 |
| `BH_SYNC_MAX_SYNC_BLOCK_HEIGHT` | 默认u32最大值；不得小于 G |
| `BH_SCRIPT_REGISTRY_ENABLED` | 必须为0，表示不安装旧 sidecar；原生 observed-script registry 仍然工作 |
| `INSCRIPTION_SOURCE` / `INSCRIPTION_SOURCE_SHADOW_COMPARE` | `bitcoind` / `false` |

原有 `BH_ROOT_DIR`、`BTC_DATA_DIR`、RPC 地址/认证、缓存及 indexer 查询参数继续支持。
旧 core 文件/manifest、paired-checkpoint manifest、registry record/artifact 和 inscription fixture 必须为空。
indexer 容器只需网络、B/G 及连接配置，不需要挂载原始 UTXO 文件或取得 BH 的 origin hash 环境变量；
它仍通过既有 RPC/state-ref 契约校验上游状态。

渲染器不要求原始文件此刻存在：导入时由 Rust 校验，已经封存的数据库重启不会重新扫描文件。
文件是否可回收要由后续 Core/BH 生命周期共同判断，不能只看 Core 的 `loadtxoutset` 返回。
新导入必须使用独立 BH 目录，入口不能把旧库原地转换为原生库；库身份及恢复点由 Rust 校验。

仅验证/生成配置（不会启动节点）：

```bash
# 先设置上表中的必填变量及本机专用输出目录，再执行：
python3 docker/scripts/tools/assumeutxo_bootstrap.py validate
python3 docker/scripts/tools/assumeutxo_bootstrap.py validate --service indexer
bash docker/scripts/helpers/render_balance_history_config.sh "$BH_ROOT_DIR/config.toml"
bash docker/scripts/helpers/render_usdb_indexer_config.sh "$USDB_INDEXER_ROOT_DIR/config.json"
```

### 本批验证

```bash
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_assumeutxo_deployment.py -v
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s docker/scripts/tools -p 'test_snapshot_loader.py' -v
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s docker/scripts/tools -p 'test_balance_history_memory_profile.py' -v
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s docker/scripts/tools -p 'test_testnet_bitcoin_release.py' -v
```

8项新入口集成测试及21项旧安装/缓存/部署回归通过。新测试使用临时目录、fake 服务进程和随机 localhost TCP 端口，
覆盖 G=B/G>B、错误身份/参数保留配置、不写旧安装数据、并行启动入口、凭据转义及脱离源码树的镜像目录布局。
六个变更 shell 脚本通过 ShellCheck；CLI `--help` 通过。
另将生成的配置交给当前 production BH 二进制执行 `bootstrap`：TOML 与 Rust 原生配置校验通过，
在预设不可达 BTC RPC 的 `get_block_hash` 处退出，没有进行快照导入；记录于 `/tmp/usdb-p71-rust-config-smoke.log`。
该探针证明配置解析接通，不是主网 bootstrap 或容器交付验收。本批未构建镜像。

## 4. P7.2：Core 镜像与快照编排

- 升级 Bitcoin Dockerfile、workflow 标签及版本身份为31.1，核对官方归档 SHA-256 和实际发布签名。
  现有28.1镜像固定的三签名组合与31.1实际发布者不同，不能只修改版本号或取消签名校验。
- 定义下载状态文件、断点续传和最终文件校验，磁盘预算同时包括原始文件、Core 双 chainstate、完整 blocks/undo 和 BH staging。
- 以 Core RPC 的 canonical B/hash、snapshot chainstate/验证完成状态判定导入或恢复；
  处理首次导入、正在加载、RPC超时、进程中断、加载完成但本地标记未写完、历史验证完成后的单 chainstate。
- 明确 `prune=0`；原生快速路径不要求全量 txindex。旧目录已有 txindex 的状态与后续启停另行验证，
  不依靠通用健康检查静默改变索引配置。
- 将“Core RPC可连接”“快照链已激活”“所需块/undo可读”“后台验证完成”分别报告，避免长期任务受旧短超时限制。

## 5. P7.3：node-kit、Compose 与 controller

- 让 bundle/安装器共同提供 B/G/hash 与快照身份，模式校验和发布清单一致；不手改已发布清单的哈希以临时绕过校验。
- 将原始文件挂载给 Core 和 BH，BTC blocks 只读挂载给 BH；旧 core/registry 下载和 marker 依赖从新模式图中移除。
- 替换新模式的启动条件：Core 激活快照链后启动 BH/indexer，不等 Core 全量历史/txindex，也不等 tip 到 G 才开始导入。
  下游共识与 chain/control-plane 继续要求实际 BH/indexer 状态身份和 readiness。
- 分配 Core 导入、BH 导入/重放、前后台并行、稳定运行阶段的资源预算。
  现有 Bitcoin-only IBD 可获得较高内存比例；与 BH 并行前必须重算总预算，不能直接叠加各自缓存默认值。
- 状态命令展示下载、Core load/foreground/background、BH import/replay/sealed、indexer pending/ready，
  保留实际起止时间、吞吐和错误，不把后台未完成显示为部署失败。

## 6. P7.4：交付验收与默认切换

完成新机器全流程、离线重启、下载/导入中断、错误文件身份、磁盘不足及新旧模式升级测试；
将 P6.5 的精确同锚点验证扩展到实际 Compose、Go chain 与 control-plane。
旧28.1数据目录升级与已有BH库的选择分开处理：前者不需要重新下载已有区块，后者不进行无定义的原地格式转换。

归档镜像 digest、配置/bundle身份、服务版本、快照来源、耗时、峰值资源及最终状态。
在主网原生验收、安装/升级和部署测试通过后，再更新默认模式、CI、安装文档和发布身份。
P7.2/P7.3 的实现可以先推进；这些验收条件约束最终默认切换和发布。
