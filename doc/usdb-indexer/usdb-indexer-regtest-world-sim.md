# USDB-Indexer Regtest World Simulation

该文档说明如何运行“持续随机仿真”模式：

1. 启动本地 regtest `bitcoind`
2. 启动 `ord server`（为 `ord wallet` 动作提供索引与 RPC）
3. 启动 `balance-history`
4. 启动 `usdb-indexer`
5. 每个区块随机执行一组现实化操作并持续出块

## 脚本位置

- [regtest_world_sim.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim.sh)
- [regtest_world_sim_validator_context.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_context.sh)
- [regtest_world_sim_validator_context_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_context_reorg.sh)
- [regtest_world_sim_validator_candidate_set.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_candidate_set.sh)
- [regtest_world_sim_validator_candidate_set_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_candidate_set_reorg.sh)
- [regtest_world_sim_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh)
- [regtest_world_simulator.py](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_simulator.py)
- [regtest_world_sim_determinism.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_determinism.sh)
- [regtest_world_sim_reorg_determinism.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh)
- [regtest_world_sim_economic_views.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_economic_views.sh)
- [compare_world_sim_reports.py](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/compare_world_sim_reports.py)
- [test_regtest_world_simulator.py](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/test_regtest_world_simulator.py)
- [run_live.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live.sh)
- [run_live_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live_reorg.sh)
- [usdb-indexer-regtest-topology.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-topology.md)
- [usdb-indexer-regtest-world-sim-validator-sampled.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim-validator-sampled.md)
- [usdb-indexer-regtest-world-sim-validator-candidate-set.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim-validator-candidate-set.md)
- [usdb-indexer-regtest-world-sim-validator-sampled-reorg.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim-validator-sampled-reorg.md)
- [usdb-indexer-regtest-world-sim-reorg-determinism.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim-reorg-determinism.md)
- [usdb-indexer-regtest-world-sim-live-reorg.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim-live-reorg.md)
- [usdb-indexer-regtest-world-sim-validator-candidate-set-soak.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim-validator-candidate-set-soak.md)

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
  - `standard_mint`
  - `fixed_collab_mint`
  - `address_collab_mint`
  - `invalid_mint`
  - `transfer`
  - `standard_remint(prev)`
  - `fixed_collab_remint(prev)`
  - `address_collab_remint(prev)`
  - `send_balance`
  - `spend_balance`
  - `noop`
  - remint 只从 actor 当前真实拥有的 `Active / Dormant` pass 中选择 prev；collab mint/remint 会锁定本动作使用的 active standard Leader，避免同块并发动作改变验证前提。
- 每个 tick（每个新区块）会输出：
  - 当前链高度与 usdb `synced_height`
    - 这里的 `synced_height` 对应 `get_sync_status.synced_block_height`，表示 `usdb-indexer` 本地 durable 已提交高度
    - 当前 world-sim 顶层摘要没有单独提升 `balance_history_stable_height`；如果需要分析上游稳定 ceiling，应查看原始 `get_sync_status` 返回值
  - 本块执行动作与失败数
  - 动作后 RPC 验证成功/失败
  - agent 粒度自检（默认开启）：对连续两次抽查中保持同一 Active pass 的区间，读取 balance-history 的完整余额事件，按 UIP-0003 unit、age penalty 和 `u128` 饱和规则逐段重算；同时检查中间事件高度和最终高度的能量、余额和余额年龄起点，不要求两次 BTC 高度相邻。
    - `agent_energy_check_ok` 只统计实际完成数值重算的前进区间；`agent_energy_check_balance_events` 统计这些区间内重放的余额记录。
    - 首次采样、pass 切换及重组重建后的首次采样计入 `agent_energy_check_baseline`。新 pass 的初始/继承能量在这里作为基线，不能把该计数解释为 mint/remint 公式已独立验证。
    - 无 Active pass、同高重复检查分别计入 `agent_energy_check_skipped_no_active`、`agent_energy_check_skipped_same_height`；原有 `agent_self_check_ok` 仍包含结构检查和基线检查。
    - weekly soak 要求严格数值重算达到最低次数，默认 2500 轮、每 5 轮抽查时至少 50 次，并至少重放 1 条余额事件。工作轮数、抽查间隔和 stable lag 保持不变。
    - 该 oracle 独立于 indexer 能量计算，但余额输入来自 balance-history；它不是从 Bitcoin 原始交易独立重建全部 pass 状态的 oracle。
  - 全局交叉检查（低频采样，默认开启）：
    - 对比 raw-energy leaderboard 与 `get_pass_energy`
    - 要求 candidate set 精确等于 active standard 集合，并按 `effective_energy DESC, pass_id ASC` 排序
    - active collab 自身必须保持 `effective=0 / level=0 / factor=10000` 且不进入 candidate
    - 独立解析 fixed/address Leader 绑定，并用两种 breakdown 排序完整遍历 opaque cursor
    - 从 breakdown 每行 raw energy 重算 5000 bps contribution、饱和 aggregate，再与 Leader profile/candidate 的 contribution/effective/count 交叉验证
    - 抽样 active owner 校验 `balance-history` 与 usdb 视图一致性
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
- 可选启用 validator sampled historical validation：
  - 周期性从 UIP-0006 canonical candidate view 抓取一张或多张 active standard pass 历史样本
  - 在 head 继续前进后，按包含完整 version identity 的历史 `ConsensusQueryContext` 重新校验 `state ref / economic profile / candidate set / collab breakdown`
  - `candidate_set` 模式下还会重算 winner，并可选把错误 winner 传入候选比较断言，确认其拒绝；只有一名候选时篡改其 raw energy。该计数表示模拟器候选断言的负向检查，不代表执行过 geth 区块导入。
  - 报告中会出现 `event = "validator_sample_capture"` 与 `event = "validator_sample_validation"`
  - 如果打开 tamper 检测，还会出现 `event = "validator_sample_tamper_validation"`
  - 有限工作轮次结束后进入收尾：只挖空块推进稳定 head，不再安排业务动作、采样或重组，直到全部样本满足 `SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE` 并完成验证。默认最后一轮采样后再挖 2 个 BTC 空块，2500 个工作轮次保持不变。
  - 收尾等待 Ord 与两个索引服务在目标高度及 canonical hash 上收敛；验证失败或仍有 pending 样本时不会输出成功的 `session_end`。
  - `validator_finalization` 记录本次收尾的 `extra_blocks`、`pending_before/after` 和耗时；`session_end` 包含 `completed_work_ticks`、`validator_samples` 的采集/验证/待验证数量，以及收尾证据。恢复点保留在第 N+1 轮；收尾中断后按当前稳定高度继续，不重跑已完成的工作轮次。
- 运行失败时会自动打印关键日志尾部，提升排障速度。
- 可选 deterministic economic bootstrap：
  - 依次构造 standard Leader、fixed collab、address collab 和第二张 standard candidate
  - 先积累一块 raw energy，再 remint Leader，断言 fixed collab 不跟随、address collab 自动跟随
  - 分别 remint fixed/address collab，断言旧 pass `Consumed / raw=0`，新 collab 仅继承 raw energy并重新进入 breakdown
  - 每一步都执行完整 profile/candidate/breakdown 全局交叉检查

`world-sim` 的整体组件关系、读写链路和 reorg 时的侧视变化见：[usdb-indexer-regtest-topology.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-topology.md)。

## Weekly soak 完成门禁

`run_regtest_world_soak_matrix.sh` 汇总最后一次 session 时，除要求所有 `_fail` 为零，还通过 `world_soak_coverage.py` 检查工作身份、完成轮次和正向覆盖证据。默认每个 seed 的要求为：

| 检查 | 2500 轮默认门槛 |
| --- | --- |
| 工作轮次 | 恰好完成 2500 轮，收尾空块不计入 |
| validator 样本 | pending 为 0，captured、validated 与成功计数一致 |
| 成功重组 | 4 次；按 `min(轮数 // 重组间隔, 最大次数)` 计算，最大次数为 0 时不封顶 |
| 正向历史验证 | 至少 12 次，不包含预期的重组失配拒绝 |
| 篡改拒绝 | 至少 12 次实际候选比较拒绝 |
| 严格能量重算 | 至少 50 个区间，且至少覆盖 1 条余额事件 |
| 必需动作 | 10 类动作各至少 1 次结果验证成功 |
| 独立重建对照 | 4 个重组检查点和 1 个最终检查点全部匹配 |

历史验证和篡改拒绝下限为 `max(1, (轮数 // 采样间隔) // 2)`，为无候选或被重组失效的样本留出空间；能量重算下限为 `max(1, (轮数 // 自检间隔) // 10)`，允许新 pass 基线和状态切换。关闭必需检查会使门禁失败。

动作覆盖使用新增的 `<action>_verified`，只有链上结果通过断言后才增加；原有提交阶段的 `_ok` 不能替代它。必需动作包括三类 mint、三类 remint、transfer、send_balance、spend_balance 和 invalid_mint；最后一项要求确认无效 mint 被识别为 Invalid。noop 不参与门禁。旧版 recovery/report 缺少这些证据时不能通过新门禁，需要重新运行。

## 重组后的独立状态重建对照

weekly 默认启用 `SIM_REPLAY_CHECK_ENABLED=1`。每次重组收敛后立即保存原实例在该高度的完整业务视图；完成 2500 个工作轮次和 validator sample 收尾后，再保存最终检查点。所有查询都固定 BTC 高度、canonical hash 和完整历史 state identity，读取前后要求该 identity 不变。

随后 `compare_world_replay.py` 为 balance-history 和 usdb-indexer 创建全新的空数据目录，只复用网络及协议配置，从高度 1 重放最终 canonical Bitcoin 链。两个服务使用独立端口，indexer 连接重建的 balance-history，铭文源固定为 `bitcoind`。每次尝试都创建新目录，不能复用上次重放产生的数据。

新实例不仅要达到相同高度、hash 和 consensus readiness，还要完成历史锚点回填：批量同步可能先发布 head，再补齐历史 state ref。比较前等待所需检查点及最终高度前一块的历史锚点可查询，只重试明确的 `HISTORY_NOT_AVAILABLE`；mismatch 不会被当成暂未就绪而忽略。

对照范围包括：

- 每个检查点的 balance-history state ref、indexer snapshot/local commit/system state identity。
- 全部 pass 的枚举和状态统计、所有权、satpoint、prev、失效原因、fixed/address Leader 绑定，以及所有 pass 的经济 profile。
- 完整 candidate set、每个 standard pass 的 collab breakdown、相关地址的 owner-pass/active-pass 视图和余额；地址集合包含全部 agent、mint owner 及当时 owner。
- 最终检查点额外比较所有 pass 的完整状态历史、能量记录，以及相关地址的完整余额历史；历史 owner 也加入余额对照集合。

分页必须完整遍历，并检查总数、重复行和 continuation。仅排除 SQLite 本地分配的 `event_id`、`last_event_id`，保留事件顺序和全部业务字段。出现差异时保存原检查点、重建结果和 `difference.json`，报告第一个不同字段的路径。相同摘要只用于记录已完成的逐项比较，不替代状态读取和语义检查。

报告新增 `replay_checkpoint` 和 `replay_comparison`。weekly 门禁要求本次 session 的所有重组检查点与最终检查点都比较成功，不能使用旧 session 的成功记录。`session_end` 表示模拟工作及采样完成；启用重放后，driver 成功还要求其后的 `replay_comparison.status=ok`。recovery 文件保留到对照成功，失败后可从 N+1 恢复，无需重新执行工作轮次。

参数：`SIM_REPLAY_OUTPUT_DIR` 保存检查点、比较摘要及两项服务启动日志；matrix 默认放在输出根目录的 `seed-<seed>-replay`，成功清理临时数据库后仍保留。服务内部详细日志位于临时重放目录，失败时随 workspace 保留。`SIM_REPLAY_TIMEOUT_SEC` 默认 1800 秒，限制启动、同步和比较阶段；预算为 2500 轮产生的数万 BTC 区块及历史锚点回填留出余量，实际增量耗时需以完整 weekly 测量。`REPLAY_BH_RPC_PORT`、`REPLAY_INDEXER_RPC_PORT` 设置独立端口，matrix 使用 seed 端口组的 `+13/+14`。普通 world-sim 默认关闭此项，手动启用时需要有限轮次、结构化报告和至少一次重组。

该测试验证“经历重组的实例”与“同一实现从 canonical 链重建的实例”一致，可以发现回滚残留、历史污染和派生视图不一致。它不证明共同实现中的协议算法正确，也不重建 Ord 数据库；独立公式 oracle、Ord 重组校验及 geth 共识测试仍各自承担对应覆盖。

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

UIP-0001 至 UIP-0006 economic view 聚焦回归：

```bash
src/btc/usdb-indexer/scripts/regtest_world_sim_economic_views.sh
```

该入口默认使用 `limit=2` 强制 candidate/breakdown cursor 翻页，执行 deterministic economic bootstrap，并在后续区块完成 candidate-set historical replay；结束时会从 JSONL 报告硬性校验所有必需 action 和零失败指标。

运行期间可同时打开前端页面观察动态变化：

1. `python3 -m http.server 8088`
2. `http://127.0.0.1:8088/web/usdb-indexer-browser/`
3. 页面 RPC endpoint 设置为当前 `USDB_INDEXER_RPC_PORT`（默认 `http://127.0.0.1:28120`）

## 常用环境变量

### 基础编排

- `WORK_DIR`：运行目录（默认临时目录）
- `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录
- `ORD_BIN`：ord 可执行文件
- `BTC_RPC_PORT`、`BTC_P2P_PORT`
- `BH_RPC_PORT`
- `USDB_INDEXER_RPC_PORT`
- `ORD_SERVER_PORT`
- `ORD_POLLING_INTERVAL`：隔离 regtest Ord 的新区块轮询间隔，默认 `200ms`；对照旧配置可设为 `5s`。
- `USDB_UPSTREAM_POLL_INTERVAL_MS`：隔离 regtest indexer 的上游轮询间隔，默认 `200`，范围 `100..60000`。

wrapper 将后者写入 `config.json` 的 `usdb.upstream_poll_interval_ms`。普通节点未配置该字段时，保持 indexer 空闲轮询 `5000ms`、状态监控 `1000ms`；状态监控实际使用 `min(upstream_poll_interval_ms, 1000)`。此配置只控制调度频率，不改变稳定确认深度、链身份或 readiness 判定。

### 钱包与链参数

- `AGENT_COUNT`：仿真代理数量（默认 `5`）
- `PREMINE_BLOCKS`：预挖块数（默认 `140`）
- `FUND_AGENT_AMOUNT_BTC`：每个 agent 初始资金（默认 `4.0`）
- `FUND_CONFIRM_BLOCKS`：资金确认块数（默认 `2`）

### 仿真参数

- `SIM_BLOCKS`：仿真 tick 数（默认 `300`；设置 `0` 可无限运行）。当前 regtest 每轮还挖 10 个稳定确认块，因此 2500 tick 通常产生约 27500 个 BTC 块，另加初始化和重组触发块。
- `SIM_SEED`：随机种子（默认 `42`）
- `SIM_FEE_RATE`：铭文与转移费率（默认 `1`）
- `SIM_MAX_ACTIONS_PER_BLOCK`：每块最大动作数（默认 `2`）
- `SIM_STANDARD_MINT_PROBABILITY`（默认 `0.14`）
- `SIM_FIXED_COLLAB_MINT_PROBABILITY`（默认 `0.04`）
- `SIM_ADDRESS_COLLAB_MINT_PROBABILITY`（默认 `0.04`）
- `SIM_INVALID_MINT_PROBABILITY`（默认 `0.02`）
- `SIM_TRANSFER_PROBABILITY`（默认 `0.18`）
- `SIM_REMINT_PROBABILITY`（默认 `0.12`，平均分配给三种 remint action）
- `SIM_SEND_PROBABILITY`（默认 `0.28`）
- `SIM_SPEND_PROBABILITY`（默认 `0.14`）
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
- `SIM_ECONOMIC_PAGE_LIMIT`：candidate/breakdown cursor 分页大小（默认 `16`）
- `SIM_ECONOMIC_BOOTSTRAP_ENABLED`：是否在随机 tick 前执行 deterministic economic bootstrap（默认 `0`）
- `SIM_VALIDATOR_SAMPLE_ENABLED`：是否启用 validator sampled validation（默认 `0`）
- `SIM_VALIDATOR_SAMPLE_MODE`：`single` 或 `candidate_set`（默认 `single`）
- `SIM_VALIDATOR_SAMPLE_TAMPER_ENABLED`：是否在 `candidate_set` 回放成功后追加 wrong-winner / tamper 检测（默认 `0`）
- `SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS`：每隔多少块抓取一次历史 validator sample（默认 `0`，表示关闭）
- `SIM_VALIDATOR_SAMPLE_SIZE`：每次从 canonical active standard candidate 中抓取多少张（默认 `1`）
- `SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE`：head 至少前进多少块后再回查历史 sample（默认 `2`）
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
SIM_SCRIPTED_CYCLE=standard_mint,fixed_collab_mint,address_collab_mint,send_balance,transfer,standard_remint,fixed_collab_remint,address_collab_remint,spend_balance,noop \
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

带 validator sampled historical validation 的示例：

```bash
SIM_POLICY_MODE=adaptive \
SIM_VALIDATOR_SAMPLE_ENABLED=1 \
SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS=6 \
SIM_VALIDATOR_SAMPLE_SIZE=1 \
SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE=2 \
src/btc/usdb-indexer/scripts/regtest_world_sim_validator_context.sh
```

带 validator sampled validation + deterministic reorg 的示例：

```bash
SIM_POLICY_MODE=adaptive \
SIM_VALIDATOR_SAMPLE_ENABLED=1 \
SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS=6 \
SIM_VALIDATOR_SAMPLE_SIZE=1 \
SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE=2 \
SIM_REORG_INTERVAL_BLOCKS=10 \
SIM_REORG_DEPTH=2 \
src/btc/usdb-indexer/scripts/regtest_world_sim_validator_context_reorg.sh
```

带 candidate-set sampled validation 的示例：

```bash
SIM_VALIDATOR_SAMPLE_ENABLED=1 \
SIM_VALIDATOR_SAMPLE_MODE=candidate_set \
SIM_VALIDATOR_SAMPLE_TAMPER_ENABLED=1 \
SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS=6 \
SIM_VALIDATOR_SAMPLE_SIZE=3 \
SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE=2 \
src/btc/usdb-indexer/scripts/regtest_world_sim_validator_candidate_set.sh
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
- `BASE_BTC_RPC_PORT`、`BASE_BTC_P2P_PORT`、`BASE_BH_RPC_PORT`、`BASE_USDB_INDEXER_RPC_PORT`、`BASE_ORD_SERVER_PORT`：双跑的起始端口组
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
