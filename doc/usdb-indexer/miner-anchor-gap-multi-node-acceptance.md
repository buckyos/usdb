# Miner anchor gap 独立多节点验收

对应 [UIP-0018](../UIP/UIP-0018-miner-btc-anchor-gap-policy.md) 的 C 阶段。
本批聚焦高度对照、混合策略、延迟、复用耗尽与自动恢复；经济状态变化与双轴 activation
边界的多节点组合继续单独推进。它们已有的 Go 历史组块回归不能替代多节点验收。

## 实现边界

- Go 基线：`a9550af7c`；USDB/node-kit 基线：`a13b873`。
- 新增 `tests/miner_gap_acceptance.py`、`tests/common/anchor_gap_evidence.py` 和
  `scripts/usdb/run_miner_gap_matrix.sh`，复用既有独立多节点服务夹具。
- 每次运行从新目录建立 3 套 Core、BH、indexer、geth 数据；两种 pass/受益地址。
  混合场景的三个进程分别使用 0/2/1，C 与 A 使用同一矿工身份，B 使用第二个身份。
- 只有封块计算使用 1200ms delayed fake PoW；真实执行 P2P、历史状态 RPC、selector 验证、
  fork choice、状态执行和账本奖励。此处不测算力竞争概率或真实收益优势。
- 旧 fault/restart 矩阵显式固定 gap=0，保留原来的故障条件；新的对照从 genesis 开始，
  不复用 gap=0 已经推高的 USDB parent。混合场景则有意验证该父块约束。
- 延迟门只阻塞同步工作线程读取指定 Bitcoin raw block；不伪造 indexer 的成功响应。
  B 阻塞 BH，C 阻塞 indexer，并检查 C 的 BH 可以独立前进。

## 场景与门限

| 任务 | 场景 | 必须证明的结果 |
| --- | --- | --- |
| Nightly `miner-anchor-gap` | gap 0/1/2 × validator lag 1/2，共 6 个独立运行 | `H=L-gap`；`gap>=lag` 时，即使全局 CatchingUp 也能验证历史块；其余情况实际产生指定高度查询失败并等待 |
| 同上 | gap=2、lag=3 | 等待两个相继 USDB 区块；放开上游后自动导入既有 tip，无人工重连/chain 重启/新增区块触发 |
| 同上 | gap=2、M=3、BTC 暂停 | 顺序使用 `L-2,L-1,L`，各自 age 为 0..3；完全耗尽后停止，BTC 增加一个高度后原 miner 自动继续 |
| Weekly `miner-anchor-gap-soak` | 0/2/1 混合 | 真实工作记录证明 gap 被父 anchor 压为 0，随 L 前进恢复为 1、2，再次压缩；anchor 永不回退 |
| 同上 | 连续批量 3 个 BTC 块，延迟 10/30/60/120 秒 | 同样的高度 gap 不等于固定秒数缓冲；错误、等待和释放后自动导入均留下证据 |
| 同上 | 混合 gap 的既有故障矩阵 | BH/indexer 延迟、RPC 断开、双矿工竞争与不同 anchor 分支，恢复后规范链、系统状态、双方余额一致；坏块依然拒绝 |

每个场景核对真实进程身份和 peer 连续性，防止重启/重连掩盖重试缺陷。
恢复后的校验逐块比较 hash、selector、state root、USDB system slots 和受益地址余额，
并比较三套独立上游数据库的状态摘要。

## 观测口径

`summary.json` 的 `work` 来自生产者实际提交的 `miner_getUSDBWorkStatus.work`，
在封块期间读取，再与落地块的 parent、块号、矿工、pass、高度、snapshot/system ID 和 age
逐项关联；不用较晚的 `attempt` 或验证者自身 L 推算生产者实际 gap。

`observations` 按节点、block hash、完整状态身份分别记录：

- 状态首次被探测可用的时间、实际验证查询失败时间、首次观察到规范导入的时间。
- `validation_wait_seconds` 仅在观察到首次失败和导入时提供；这是本机采样值。
- 真实网络收包时间尚未接入，因此 `received_at`、`receipt_to_import_seconds` 和由收包
  推导的状态提前量保持 `null`，不把查询时间冒充网络收包时间。
- `parent_wait` 表示后继样本产生时其父块仍受不可用状态阻塞；并非声称已捕获后继块的收包事件。
- 未完成样本保留 `unresolved=true`；`before-recovery.json` 保存解除延迟前的快照。
  观察进程重新开始时，历史时间保持未知；不同分支的相同高度不共享时间。

`recovery_seconds` 统计解除延迟到导入完成且上游就绪的整段恢复时间；单块验证等待
另看 `validation_wait_seconds`。不需要等待的对照仍须释放上游并恢复追平状态。

RPC 审计日志附单机 monotonic 起止时间。没有跨主机校时，也不据此给出全网最快节点、
概率、p95 性能或 PoW 利润结论。Fast 中的重启/分支投影用例只检查报告语义，
并不等同于真实重组和观察进程重启验收。

## 执行与产物

使用 Go 仓库的统一 CI 入口，沿用 revision lock 和外部 regtest 工具：

```bash
scripts/usdb/run_long_ci.sh nightly miner-anchor-gap
scripts/usdb/run_long_ci.sh weekly miner-anchor-gap-soak
```

编译可通过现有 `--prepare-only` / `--run-only` 分开；单个服务矩阵受 900 秒预算约束。
若要复用已构建二进制，先准备 `$MATRIX_WORK_ROOT/bin/{geth,invalidblock,balance-history,usdb-indexer}`，
指定 `GETH_BIN`、`BITCOIN_BIN_DIR`、`ORD_BIN`、`MATRIX_WORK_ROOT`、`MATRIX_OUTPUT_DIR` 后运行：

```bash
bash scripts/usdb/run_miner_gap_matrix.sh nightly
bash scripts/usdb/run_miner_gap_matrix.sh weekly
```

两组并行时必须使用不同的 `MATRIX_PORT_BASE`。每个子场景仍自行新建数据目录，服务退出后
保留诊断数据；不要指向在线节点的数据目录。结果目录按场景分开，包含 summary、canonical
blocks、上游 RPC/delay 日志及各进程日志；长 CI 继续自动收集失败诊断。

## 本地验收记录（2026-10-14）

本批测试已提交为 Go `a391fd3ac`，文档随本记录提交；尚未发布或取得远端 CI 结果。以下均为本机隔离 regtest 验收。
Go 使用 canonical 1.18.5，Rust 1.91.0，Bitcoin Core 28.1、Ord 0.23.3。
本地 USDB 的 Rust 源码及 `miner_pass_regtest.py` 与 CI 当前锁定的 `d793cc3` 一致。

| 验证 | 结果 |
| --- | --- |
| Nightly 8 个独立运行 | 全部通过；6 组基本对照、gap=2/lag=3、M=3 连续让步/耗尽恢复 |
| Weekly 6 个独立运行 | 全部通过；混合策略、4 个秒级延迟档位、混合 gap 故障矩阵 |
| 混合策略复跑 | 全新数据复跑通过；实际 gap 序列 `2,1,0,0,1,2,1,0,0`，各工作与落地块身份相符 |
| 竞争分支恢复 | 混合故障矩阵的 14 项门禁通过，实际替换 3 个旧分支块；最终 70 块规范链、系统 slots、双方奖励与三套上游摘要一致 |
| 小 M | L=148 时依次使用 146/147/148，各 age=0..3；耗尽后保持停止，新增高度 149 后无重启恢复，age=0 |
| 故障分类 | 临时依赖延迟不进入坏块记录；构造的 invalid gasUsed 块明确拒绝 |
| Fast 定向回归 | 67 项通过：新证据投影 5、既有多矿工 7、上游故障 29、重启 6、CI 编排 20 |
| 静态校验 | ShellCheck、Python 解析、workflow YAML、release fragment、文档链接与 diff 检查通过 |

基础对照的两条独立路径结果相同：gap=0 对 lag=1/2 均等待；gap=1 对 lag=1 可直接验证、
对 lag=2 等待；gap=2 对 lag=1/2 可直接验证，对 lag=3 等待。等待场景解除延迟后自动导入
冻结的原 tip；未等待场景在上游仍追块时已完成验证。

本机证据位置：

- `/tmp/usdb-gap-matrix/acceptance-evidence.json`：逐运行结果与摘要 SHA-256、构建二进制 SHA-256。
- `/tmp/usdb-gap-matrix/nightly-final/`、`/tmp/usdb-gap-matrix/weekly-final/`：14 个运行的 summary、日志和区块。
- `/tmp/usdb-gap-matrix/mixed-repeat/`：最终混合策略复跑。
- `/tmp/usdb-gap-nightly-final.log`、`/tmp/usdb-gap-weekly-final.log`：完整套件输出，退出码均为 0。

连续场景验证期间修复了旧夹具额外端口预检未设置 `SO_REUSEADDR` 的问题：已结束连接的
TIME_WAIT 不再误报为现存服务占用，实际监听冲突仍拒绝。生产服务和共识代码没有改动。
本地最初下载的 CI Ord 二进制因宿主 GLIBC 不匹配无法启动，最终使用本机兼容的相同版本
Ord；不把这个环境失败计入协议判断。在线节点没有部署、重启或数据变更。

## 后续工作

- 将首次 mint/remint、继承、协作、失效与 BTC/USDB 双轴 activation 边界加入混合 gap 的
  真实多节点组合；按所选 H 检查经济语义，不能只比较最终余额相同。
- 完善真实收包、排队/淘汰、逐状态首次可用时间的完整事件链；补真实观察进程重启和
  BTC 重组身份切换场景。当前只有投影回归和 USDB 竞争分支恢复证据。
- 短间隔、分批提交和更长多轮时序继续扩展；当前秒级对照使用一次批量生成 3 个 BTC 块。
- UIP-0018 D 的真实 PoW 收益分析、跨主机测试网延迟分布与发布验收另行记录。
