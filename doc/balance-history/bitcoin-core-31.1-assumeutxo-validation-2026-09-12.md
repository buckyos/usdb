# Bitcoin Core 31.1 / AssumeUTXO 第一轮验证记录

## 状态与范围

- 日期：2026-09-12；run-id：`20260912T105003Z`。
- 对应计划：[AssumeUTXO 验证与重构大纲](./balance-history-assumeutxo-validation-and-refactor-plan.md)。
- 实验根目录：`/data/usdb-assumeutxo-validation/20260912T105003Z`。
- 结论：**本轮Bitcoin侧前台验证通过**。935000快照加载/恢复、前台追平、与原28.1同锚点完整UTXO承诺和输出样本均一致。
- 已完成：二进制来源/完整性验证、独立regtest、主网加载与恢复、跨快照边界prevout对拍、前台全量状态对拍和收尾核验。
- 后台验证未完成：结束前历史chainstate到105439，快照chainstate的 `validated=false`。
- 实验31.1节点已正常停止，数据及用户原始下载均保留；原28.1主网和regtest继续运行。
- 本轮不包含：生产镜像构建/切换、现有28.1数据目录升级、balance-history导入器与963800语义对拍。

## 1. 输入身份

| 输入 | 实际值 |
| --- | --- |
| 二进制目录 | `/data/btc/bitcoin-31.1/bin` |
| 归档 | `/data/btc/bitcoin-31.1-x86_64-linux-gnu.tar.gz` |
| 归档 SHA-256 | `b80d9c3e04da78fb6f0569685673418cf686fadba9042d926d13fb87ff503f9e` |
| RPC 版本 | `310100` |
| 快照文件 | `/data/btc/mainnet-935000-utxos.dat` |
| 快照大小 | `9,387,990,306` bytes，约8.74 GiB |
| 快照文件 SHA-256 | `e572ddbe456d254f05fb004cebe225bdb3656074b66f0e9b1c7fa83e1301d486` |
| 格式 | `utxo\xff`，version 2，mainnet magic `f9beb4d9` |
| 声明 UTXO 数 | `164,241,311` |
| 基线 block hash | `0000000000000000000147034958af1652b2b91bba607beacc5e72a56f0fb5ee` |

用户已提供下载文件；原始下载 URL 与下载耗时未提供，不能计入本次测得的启动时间。
文件 hash 与头部检查不等同于 Core 的 UTXO 内容承诺校验；后者由本轮 Core 加载日志及重启后在935000计算的完整 UTXO 承诺确认。

### 发布签名与现有镜像策略差异

从 [31.1 官方发布目录](https://bitcoincore.org/bin/bitcoin-core-31.1/) 获取 `SHA256SUMS` 和签名文件，
验证以下五名发布者的签名：achow101、fanquake、hebasto、Emzy、Sjors。

- achow101 的公钥来源和文件 hash 与当前镜像固定值一致。
- 其他四名发布者的公钥取自官方 `bitcoin-core/guix.sigs` 固定提交
  `f6a216c90095f5e316318760da6411fe465b7482`，记录了公钥文件 hash、主/子密钥指纹。
- 已下载归档与签名校验文件一致；`bitcoind`、`bitcoin-cli`、`bitcoin-wallet`、`bitcoin-tx`、
  `bitcoin-util` 五个已解压文件分别与归档中的同名文件 SHA-256 一致。
- 实际执行版本为 `v31.1.0`。

当前 Dockerfile 要求 achow101、laanwj、0xb10c 三个固定签名同时存在。本次31.1官方校验文件不满足这一组合，
所以**现有镜像签名策略仍未通过，不能只改 BITCOIN_VERSION 就判定镜像升级完成**。
本轮五签名验证仅用于独立实验，未修改或绕过现有镜像发布检查。

原始证据位于实验目录的 `reports/preflight.json`、`artifacts/signature-status.txt`、
`artifacts/signature-status-final.txt`、`artifacts/additional-signers-provenance.json`。

## 2. 独立 regtest：通过

在新的 `regtest/` 目录执行，约3.057秒完成，实验节点已正常关闭。

| 检查 | 结果 |
| --- | --- |
| 31.1 启动及 legacy JSON-RPC 1.0 请求 | 通过 |
| 生成101块，读取完整块、coinbase和UTXO | 通过 |
| 发送1.25 BTC并确认，查询已花费输入的历史交易 | 通过 |
| txindex追平后无block hash的getrawtransaction | 通过 |
| 计算hash_serialized_3 | 通过 |
| 正常退出/重启后，height/hash/UTXO数/总金额/UTXO承诺不变 | 通过 |

最终regtest高度102，UTXO数103，总金额5100 BTC。
本项仅验证这组RPC与恢复路径，不代替完整USDB/Ord回归。
原始证据：`reports/regtest-smoke.json`、`reports/regtest-console.log`。

## 3. 主网实验设置

- 独立目录 `bitcoin/`，独立配置/cookie/端口，禁用钱包及txindex/filter/coinstats索引。
- dbcache 4096 MiB、maxmempool 32 MB、4个脚本验证线程、进程nice=10。
- 裁剪目标65536 MiB；这不是整个实验目录的空间上限，chainstate和报告另计。
- 监测 `/data` 可用空间和实验进程RSS；初始停止阈值为160 GiB磁盘余量、12 GiB RSS。
  首次导入触发RSS阈值后，复核宿主机仍有约58.6 GiB可用内存，恢复实验采用24 GiB RSS阈值，并增加16 GiB宿主机可用内存保留线。
  这些是脚本采样后的正常停止条件，不是cgroup硬限制。
- 仅连接现有28.1主网节点供块，因此本轮是本地供块正确性实验，不是公网冷启动性能测量。
- headers齐备后，仅关闭实验节点网络进行快照导入和基线重启检查；原主网节点保持服务。
- 已在前台追平后进行同height/hash全量UTXO哈希及确定性outpoint样本对拍，随后正常关闭实验节点并保留数据。
- 后台全量验证状态单独记录；本轮前台对拍通过不代表后台已完成，也不代表balance-history已支持AssumeUTXO。

原始设置：`reports/mainnet-experiment-settings.json`。
`reports/mainnet-progress.json` 为最后一次运行中采样，最终结论以 `reports/mainnet-result.json` 和 `reports/final-audit.json` 为准。

## 4. 快照加载与恢复：通过

以下时间均为UTC。

- 10:59:29：开始加载935000快照；同步headers约11秒，关闭实验网络时传统chainstate已到28432。
- 11:04:14：Core日志记录全部164,241,311条UTXO加载完成，随后进行内容校验。
- 11:05:20：实验脚本采到RSS `12,958,494,720` bytes，超过初始12 GiB阈值，触发停止流程。
- 11:05:36：Core日志记录 `validated snapshot` 与 `successfully activated snapshot`，基线hash与预期一致。
- 11:05:40：实验节点正常退出，已导入数据保留。没有OOM或快照内容校验失败。
- 11:08:52：恢复节点保持网络关闭，重新扫描935000完整UTXO集，核验结果如下。
- 11:08:57：再次正常退出/重启，基线height/hash与snapshot chainstate身份保持一致，开始前台同步。

| 基线核验项 | 实际值 |
| --- | --- |
| height | `935000` |
| txouts | `164,241,311` |
| total_amount | `19,984,148.03206779 BTC` |
| hash_serialized_3 | `e4b90ef9eae834f56c4b64d2d50143cee10ad87994c614d7d04125e2a6025050` |
| 与Core内置承诺比较 | 一致 |
| 重启恢复 | 通过 |

首次运行的失败是实验资源阈值触发；其 `loadtxoutset` RPC结果未被脚本归档，不能伪造为已采集。
首次报告中的 `snapshot_content_verified=false` 表示该脚本没有取得成功结果文件；
后续使用Core成功激活日志与恢复后的完整状态扫描补齐证据。
初版脚本在线程池等待导入RPC退出后才执行节点停止，因此阈值触发到实际停止有延迟；恢复脚本已调整为先请求正常停止。

证据：`bitcoin/debug.log`、`reports/attempt-1-memory-threshold/`、
`reports/snapshot-content-verification.json`、`reports/snapshot-restart.json`。
两版运行脚本分别归档为 `reports/usdb-assumeutxo-mainnet.py` 与 `reports/usdb-assumeutxo-mainnet-resume.py`。

## 5. 前台追平与同锚点对拍：通过

- 11:43:27：Core验证至963800，BTC hash为 `000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551`。
- 11:47:31：前台与原节点在966673追平，`initialblockdownload=false`。
- 11:48:34：两个节点完成同锚点全量UTXO对拍，首次尝试通过，扫描及等待共60.852秒。
- 11:48:50：链头更新后，在966674完成65个outpoint对拍，其中10个双方有UTXO、55个双方返回null。
- 11:48:52：实验节点正常退出。

完整UTXO对拍结果：

| 字段 | 两节点共同结果 |
| --- | --- |
| height | `966673` |
| bestblock | `0000000000000000000056d807be8a14b0b24369fa915884600203d575c74a43` |
| txouts | `165,200,153` |
| total_amount | `20,083,126.13065653 BTC` |
| hash_serialized_3 | `cf0b99cd7997d57a47fd5b85875309542cd8edb5ea344d5ba49a5d5816da7a55` |
| transactions | `114,377,927` |

两个LevelDB的物理 `disk_size` 不同；该字段不是逻辑状态一致性的要求。
样本按固定规则从其锚点、前1/10/100/1000块选取，并包含coinbase输出；比较时禁用mempool视图，前后核对锚点未变。
样本锚点为966674 / `000000000000000000007d2808cf91188f65f6a103fd0dca2dce4b09294af773`，与全量扫描锚点分开记录。

最终历史chainstate仅到105439，快照chainstate到966674且仍为 `validated=false`。
因此本项证明AssumeUTXO前台与原全量节点的UTXO状态一致，**没有证明后台全量验证完成，也没有证明balance-history在963800的语义等价**。
证据：`reports/full-utxo-comparison.json`、`reports/outpoint-samples.json`、`reports/mainnet-result.json`。

### 同步期间刷盘导致的RPC超时

第二次运行于11:28:35记录 `Flushing large (21255624 entries) UTXO set to disk`。
同步监测随后调用 `getblockchaininfo` 超过其30秒超时，脚本将这一暂时不可响应误判为运行失败并请求正常停止。
Core于11:29:15完成该段处理并记录高度953853，11:29:17正常退出。没有数据校验失败。

第三版脚本在同步等待期间重试RPC传输超时，同时保留8小时阶段限时、进程状态检查和资源停止条件。
从已保存的953853状态继续，不重复加载快照；重启时再次核对实际height/hash与原节点一致、snapshot来源仍为935000。
第二次报告保存在 `reports/attempt-2-rpc-timeout/`，恢复证据为 `reports/foreground-resume.json`，
脚本为 `reports/usdb-assumeutxo-mainnet-continuation.py`。

本轮包含两次验证脚本中断与人工检查间隔；最终总墙钟时间必须按此解释，不能报告成无中断冷启动基准。

### 自动裁剪提前删除重放区块：已实测

11:31恢复时，实验节点 `pruneheight` 已从935001推进至942207。
11:32:30在前台954853再次请求935001，得到 `-1: Block not available (pruned data)`；
原28.1节点仍能读取该块。该块此前用于2604个prevout对拍时可读，说明这是实验期间自动裁剪后的实际可用性变化。

Core 31.1在有历史chainstate时，将 `-prune` 目标除以二，为两条chainstate分配预算；
IBD裁剪还为剩余区块增加缓冲空间。因此 `prune=65536` 不能解释为向前重放区间独占64 GiB、一直保留到balance-history消费。
依据：[FindFilesToPrune](https://github.com/bitcoin/bitcoin/blob/v31.1/src/node/blockstorage.cpp#L297)。

这不妨碍P2的当前UTXO状态对拍，但证明本轮实验节点不能独立提供完整的935001至963800重放区间。
P4可使用原全量节点供块；P6必须验证消费者进度协调或独立有界归档，不能以单纯增大裁剪配置代替保留保证。
证据：`reports/pruned-suffix-observation.json`。

### 快照边界的旧输出花费：通过

在后台尚未验证至快照、前台仍处于IBD时，读取快照后第一块935001的 `getblock(hash, 3)`，
将两节点该块1204笔交易的2604个输入逐一比较：outpoint、coinbase标识、创建高度、金额、原始script及交易fee均一致。
其中一例输入创建于934999，金额 `0.01168361 BTC`；又与原28.1的历史交易查询结果独立核对一致。

对同一旧交易，新节点无block hash的 `getrawtransaction` 返回预期的 `-5`（未启用txindex）。
这证明已保留的快照后block/undo可以提供输入金额与script，适合作为后续历史prevout查询改造的验证依据；
它不能提供任意旧交易的完整内容，也不保证落后消费者所需undo不会被裁剪。
证据：`reports/snapshot-boundary-prevout-check.json`。

## 6. 下一阶段输入准备

只读查询现有963800 core SQLite，已找到935000 `BlockCommitEntry`：

- BTC hash与本轮Core快照基线相同。
- balance delta root：`3265637bf1b3ec9bbd6936d5d76740890a81183234695106cb96a8e89d6dc398`。
- block commit：`6108f77e4abaafbc3a7a246024e942483c18fb37a3c710209ea6294c5617fe81`。

使用现有源码的commit公式，检查了935000至963800共28,801条已有链接（包含从934999到基线的链接），
与表中结果及manifest的963800最终commit一致；两端BTC hash与原28.1节点一致。
manifest文件hash与仓库release record一致。原始证据：`reports/balance-history-anchor-candidate.json`。

这只是候选输入准备：本轮未重新扫描core数据库文件hash，也未从逐块余额变化重新计算delta roots，
更未执行AssumeUTXO到balance-history的导入。因此该候选仍需P4/P5绑定来源并进行语义验证，不能单独作为已验证余额的结论。

## 7. 时间、资源与收尾

| 项目 | 本轮实测 |
| --- | --- |
| headers准备 | 约11秒；包含节点启动，区块由本机原节点提供 |
| 快照加载至Core成功激活 | 约6分7秒，10:59:29–11:05:36 |
| 首次主网实验启动至前台追平 | 48分14.804秒，含两次脚本中断及检查间隔 |
| 基线重启后至前台追平的墙钟跨度 | 38分34.055秒，含第二次中断及恢复间隔 |
| 首次主网实验启动至最终停止 | 49分35.503秒，含全部检查与恢复间隔 |
| 各次运行采样中的最大RSS | `14,829,051,904` bytes，约13.81 GiB；不是操作系统精确高水位 |
| 最终实验Bitcoin目录占用 | `46,725,959,680` bytes，约43.52 GiB |
| 其中blocks目录 | `34,454,294,528` bytes，约32.09 GiB |
| 其中snapshot chainstate | `12,250,267,648` bytes，约11.41 GiB |
| 其中历史chainstate | `8,151,040` bytes；后台尚未完成，不代表最终占用 |
| 收尾时/data可用空间 | 约404.51 GiB |

目录数字采用 `du -s -B1` 的已分配空间，子目录已包含在Bitcoin目录内，不能重复相加。
用户提供的8.74 GiB快照位于 `/data/btc/`，不在上表实验Bitcoin目录占用中。
本轮没有原始下载耗时、无中断冷启动测量或后台验证完成时的容量数据，不能据此承诺公网部署耗时或最终总空间。

收尾核验：

- 原主网PID22355、原regtest PID1985118仍为原28.1可执行文件；主网RPC版本280100。
- 原主网在966674，非裁剪、IBD=false，txindex和basic block filter index均同步，无RPC警告。
- 实验RPC端口39332已关闭，实验cookie已正常移除，快照输入和独立数据目录保留。
- 两次中断报告保留原始fail状态；最终续跑报告为pass，不能以最终结果覆盖中断原因。

证据：`reports/final-audit.json`。本次仅更新计划和验证记录，未改业务代码、镜像默认版本或原节点启动方式。

## 8. 对下一批工作的结论

1. 可以推进P4：从已验证的935000输入流式生成balance-history基线，带入候选commit并重放至963800，再执行P5全量语义对拍。
2. P4首轮使用原28.1全量节点供块；本轮实验节点已有裁剪缺口，不能当作完整重放区块源。
3. P6同时处理历史交易依赖、区块/undo消费进度、独立就绪状态，以及导入内存和刷盘期间的RPC暂时不可响应。
4. P3原数据目录升级与镜像集成仍待单独执行；本轮独立31.1成功不等于原目录升级、候选镜像和整套USDB回归通过。
