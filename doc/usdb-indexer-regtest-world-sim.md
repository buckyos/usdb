# USDB-Indexer Regtest World Simulation

该文档说明如何运行“持续随机仿真”模式：

1. 启动本地 regtest `bitcoind`
2. 启动 `ord server`（为 `ord wallet` 动作提供索引与 RPC）
3. 启动 `balance-history`
4. 启动 `usdb-indexer`
5. 每个区块随机执行一组现实化操作并持续出块

## 脚本位置

- [regtest_world_sim.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim.sh)
- [regtest_world_sim_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh)
- [regtest_world_simulator.py](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_simulator.py)
- [regtest_world_sim_determinism.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_determinism.sh)
- [regtest_world_sim_reorg_determinism.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh)
- [compare_world_sim_reports.py](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/compare_world_sim_reports.py)
- [run_live.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live.sh)
- [run_live_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live_reorg.sh)
- [usdb-indexer-regtest-topology.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-topology.md)
- [usdb-indexer-regtest-world-sim-reorg-determinism.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-world-sim-reorg-determinism.md)
- [usdb-indexer-regtest-world-sim-live-reorg.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-world-sim-live-reorg.md)

## 核心能力

`get_sync_status` 的完整字段语义见：[usdb-indexer-sync-status-model.md](./usdb-indexer-sync-status-model.md)。

- 真实 agent 模型（有状态）：
  - 每个 agent 有独立钱包、BTC 地址、script hash、persona（`holder`/`trader`/`farmer`/`adversary`）
  - 支持两种策略模式：
    - `adaptive`：动作选择基于上一个状态与最近动作（Markov 风格偏置）
    - `scripted`：按固定动作序列轮转（用于稳定复现与排障）
  - 每个区块内限制“单 agent 最多一次参与”，避免同块多动作互相覆盖
  - 支持按区块逐步扩容 active agents，模拟用户增长
- 随机操作类型（按概率）：
  - `mint`
  - `invalid_mint`
  - `transfer`
  - `remint(prev)`
  - `send_balance`
  - `spend_balance`
  - `noop`
- 每个 tick（每个新区块）会输出：
  - 当前链高度与 usdb `synced_height`
    - 这里的 `synced_height` 对应 `get_sync_status.synced_block_height`，表示 `usdb-indexer` 本地 durable 已提交高度
    - 当前 world-sim 顶层摘要没有单独提升 `balance_history_stable_height`；如果需要分析上游稳定 ceiling，应查看原始 `get_sync_status` 返回值
  - 本块执行动作与失败数
  - 动作后 RPC 验证成功/失败
  - agent 粒度自检（默认开启）：每块对选中 agent 的 active pass 做能量数值校验（与公式推导一致）
  - 全局交叉检查（低频采样，默认开启）：每 K 块对比 `leaderboard top N` 与 `get_pass_energy`，并抽样 active owner 校验 `balance-history` 与 usdb 视图一致性
  - pass 总量 / active / invalid
  - active address 总余额
  - 能量榜首摘要
- 支持固定 `seed`，保证场景可复现。
- 支持可控 deterministic reorg 注入：
  - 按固定 tick 间隔替换最近 `N` 个 canonical blocks
  - reorg 后等待 `ord` / `balance-history` / `usdb-indexer` 全部收敛
  - 重建模拟器本地 pass ownership 视图，再继续后续随机业务
- 可选输出结构化 JSONL 报告（每个 tick 一条记录），便于后续离线分析。
  - tick 事件包含 `tick_action_type_counts`，便于“同 seed 双跑”时对比关键序列统计。
  - tick 事件的 `synced_height` 也是 `get_sync_status.synced_block_height` 的摘要值，不应解读成上游稳定高度。
  - 如果启用 reorg 注入，报告中还会出现 `event = "reorg"` 的单独事件。
- 运行失败时会自动打印关键日志尾部，提升排障速度。

`world-sim` 的整体组件关系、读写链路和 reorg 时的侧视变化见：[usdb-indexer-regtest-topology.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-topology.md)。

## 运行示例

```bash
src/btc/usdb-indexer/scripts/regtest_world_sim.sh
```

Long-run live preset (recommended for direct start):

```bash
src/btc/usdb-indexer/scripts/run_live.sh
```

This wrapper preloads a high-pressure profile (default `200 agents` + `5000 blocks`), and every variable at the top of the script is documented for quick tuning.

Long-run live preset with periodic deterministic reorg:

```bash
src/btc/usdb-indexer/scripts/run_live_reorg.sh
```

This wrapper keeps the same long-run style, but enables periodic replacement-chain injection for soak testing.

运行期间可同时打开前端页面观察动态变化：

1. `python3 -m http.server 8088`
2. `http://127.0.0.1:8088/web/usdb-indexer-browser/`
3. 页面 RPC endpoint 设置为当前 `USDB_RPC_PORT`（默认 `http://127.0.0.1:28120`）

## 常用环境变量

### 基础编排

- `WORK_DIR`：运行目录（默认临时目录）
- `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录
- `ORD_BIN`：ord 可执行文件
- `BTC_RPC_PORT`、`BTC_P2P_PORT`
- `BH_RPC_PORT`
- `USDB_RPC_PORT`
- `ORD_SERVER_PORT`

### 钱包与链参数

- `AGENT_COUNT`：仿真代理数量（默认 `5`）
- `PREMINE_BLOCKS`：预挖块数（默认 `140`）
- `FUND_AGENT_AMOUNT_BTC`：每个 agent 初始资金（默认 `4.0`）
- `FUND_CONFIRM_BLOCKS`：资金确认块数（默认 `2`）

### 仿真参数

- `SIM_BLOCKS`：仿真区块数（默认 `300`；设置 `0` 可无限运行）
- `SIM_SEED`：随机种子（默认 `42`）
- `SIM_FEE_RATE`：铭文与转移费率（默认 `1`）
- `SIM_MAX_ACTIONS_PER_BLOCK`：每块最大动作数（默认 `2`）
- `SIM_MINT_PROBABILITY`（默认 `0.20`）
- `SIM_INVALID_MINT_PROBABILITY`（默认 `0.02`）
- `SIM_TRANSFER_PROBABILITY`（默认 `0.20`）
- `SIM_REMINT_PROBABILITY`（默认 `0.10`）
- `SIM_SEND_PROBABILITY`（默认 `0.30`）
- `SIM_SPEND_PROBABILITY`（默认 `0.15`）
- `SIM_SLEEP_MS_BETWEEN_BLOCKS`：每块间隔毫秒（默认 `0`）
- `SIM_FAIL_FAST`：动作失败是否立刻退出（`1` 开启）
- `SIM_INITIAL_ACTIVE_AGENTS`：初始 active agents 数（默认 `3`）
- `SIM_AGENT_GROWTH_INTERVAL_BLOCKS`：每隔多少块扩容一次 active agents（默认 `30`）
- `SIM_AGENT_GROWTH_STEP`：每次扩容增加的 agent 数（默认 `1`）
- `SIM_POLICY_MODE`：策略模式（`adaptive` 或 `scripted`，默认 `adaptive`）
- `SIM_SCRIPTED_CYCLE`：`scripted` 模式的动作序列（逗号分隔）
- `SIM_REPORT_ENABLED`：是否启用 JSONL 结构化报告（默认 `1`）
- `SIM_REPORT_FILE`：报告文件路径（默认 `$WORK_DIR/world-sim-report.jsonl`）
- `SIM_REPORT_FLUSH_EVERY`：报告刷盘频率（按事件条数，默认 `1`）
- `SIM_AGENT_SELF_CHECK_ENABLED`：是否启用 agent 自检（默认 `1`）
- `SIM_AGENT_SELF_CHECK_INTERVAL_BLOCKS`：每隔多少块执行一次自检（默认 `1`）
- `SIM_AGENT_SELF_CHECK_SAMPLE_SIZE`：每次自检采样多少 active agents（默认 `0`，表示全量）
- `SIM_GLOBAL_CROSS_CHECK_ENABLED`：是否启用全局交叉检查（默认 `1`）
- `SIM_GLOBAL_CROSS_CHECK_INTERVAL_BLOCKS`：每隔多少块执行一次全局交叉检查（默认 `20`）
- `SIM_GLOBAL_CROSS_CHECK_LEADERBOARD_TOP_N`：每次检查的能量榜前 N 条（默认 `20`）
- `SIM_GLOBAL_CROSS_CHECK_OWNER_SAMPLE_SIZE`：每次检查抽样的 active owner 数（默认 `16`，`0` 表示全量）
- `SIM_REORG_INTERVAL_BLOCKS`：每隔多少个 tick 注入一次 deterministic reorg（默认 `0`，表示关闭）
- `SIM_REORG_DEPTH`：每次 reorg 替换最近多少个 canonical blocks（默认 `3`）
- `SIM_REORG_MAX_EVENTS`：单次运行最多注入多少次 reorg（默认 `1`，`0` 表示不限制）
- `DIAG_TAIL_LINES`：失败诊断时每个日志文件打印的尾部行数（默认 `120`）

## 示例：长时间持续运行

```bash
SIM_BLOCKS=0 \
SIM_SEED=20260308 \
SIM_SLEEP_MS_BETWEEN_BLOCKS=300 \
AGENT_COUNT=8 \
src/btc/usdb-indexer/scripts/regtest_world_sim.sh
```

固定动作序列模式示例（便于复现）：

```bash
SIM_POLICY_MODE=scripted \
SIM_SCRIPTED_CYCLE=mint,send_balance,transfer,remint,spend_balance,noop \
src/btc/usdb-indexer/scripts/regtest_world_sim.sh
```

带 deterministic reorg 注入的组合回归示例：

```bash
SIM_POLICY_MODE=scripted \
SIM_REORG_INTERVAL_BLOCKS=20 \
SIM_REORG_DEPTH=3 \
SIM_REORG_MAX_EVENTS=2 \
src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh
```

## 同 seed 双跑一致性检查

用于快速发现非确定性问题（并发、缓存、时序）：

```bash
src/btc/usdb-indexer/scripts/regtest_world_sim_determinism.sh
```

可选参数（环境变量）：

- `SIM_SEED`：两次运行都使用同一个 seed
- `SIM_BLOCKS`：每次运行的区块数
- `WORK_DIR`：双跑总工作目录
- `RUN1_WORK_DIR`、`RUN2_WORK_DIR`：单次运行工作目录
- `RUN1_REPORT_FILE`、`RUN2_REPORT_FILE`：两次报告路径
- `BASE_BTC_RPC_PORT`、`BASE_BTC_P2P_PORT`、`BASE_BH_RPC_PORT`、`BASE_USDB_RPC_PORT`、`BASE_ORD_SERVER_PORT`：双跑的起始端口组
- `PORT_STRIDE`：`run2` 相对 `run1` 的端口偏移，默认 `100`

脚本会顺序运行两次 `regtest_world_sim.sh`，然后调用 `compare_world_sim_reports.py` 对比：

- `session_end.final_metrics`
- 每个 tick 的关键字段（默认不比较 txid/inscription id）
- 如果报告里包含 `reorg` 事件，还会比较每次 reorg 的稳定字段（如 tick、rollback 高度、重建后的 pass 行数和 cross-check 摘要），但不会比较区块哈希这类天然会变化的字段

带 deterministic reorg 的双跑入口：

```bash
src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh
```

## 说明

- 该模式优先用于“持续行为观测”与“协议回归压力验证”，不是严格确定性单测替代。
- 若需要严格断言，请继续使用 `run_regression.sh` 与固定场景脚本。
- 如果要分析“本地 durable 高度”和“上游稳定高度”是否同时收敛，应把 world-sim 摘要里的 `synced_height` 与单独拉取的 `get_sync_status.balance_history_stable_height` 结合起来看，而不是把摘要字段当成完整同步状态。
- 如果启用了 reorg 注入，模拟器会在 replacement tip 上重建本地 ownership 视图；这一步的目标是保证后续随机动作继续基于新链，而不是沿用旧链缓存。
