# USDB-Indexer Regtest World Simulation

该文档说明如何运行“持续随机仿真”模式：

1. 启动本地 regtest `bitcoind`
2. 启动 `ord` 临时服务（仅用于构造铭文交易）
3. 启动 `balance-history`
4. 启动 `usdb-indexer`
5. 每个区块随机执行一组现实化操作并持续出块

## 脚本位置

- [regtest_world_sim.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim.sh)
- [regtest_world_simulator.py](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_simulator.py)
- [run_live.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live.sh)

## 核心能力

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
  - 当前链高度与 usdb 同步高度
  - 本块执行动作与失败数
  - 动作后 RPC 验证成功/失败
  - pass 总量 / active / invalid
  - active address 总余额
  - 能量榜首摘要
- 支持固定 `seed`，保证场景可复现。
- 可选输出结构化 JSONL 报告（每个 tick 一条记录），便于后续离线分析。
- 运行失败时会自动打印关键日志尾部，提升排障速度。

## 运行示例

```bash
src/btc/usdb-indexer/scripts/regtest_world_sim.sh
```

Long-run live preset (recommended for direct start):

```bash
src/btc/usdb-indexer/scripts/run_live.sh
```

This wrapper preloads a high-pressure profile (default `200 agents` + `5000 blocks`), and every variable at the top of the script is documented for quick tuning.

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

## 说明

- 该模式优先用于“持续行为观测”与“协议回归压力验证”，不是严格确定性单测替代。
- 若需要严格断言，请继续使用 `run_regression.sh` 与固定场景脚本。
