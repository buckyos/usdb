# USDB-Indexer Regtest Framework 说明

本文档说明 `usdb-indexer` 当前的 regtest 脚本框架，目标是把通用 smoke、reorg、restart、live ord 和 pending recovery 场景组织成可复用、可批量执行的一套入口，而不是零散脚本集合。

如果需要先理解整套测试栈里 `bitcoind`、`ord`、`balance-history`、`usdb-indexer` 和 `world-sim` 的连接关系，可先看拓扑说明：[doc/usdb-indexer-regtest-topology.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-topology.md)。

## 入口文件

- 共享库：[src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- 通用回归入口：[src/btc/usdb-indexer/scripts/run_regression.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_regression.sh)
- reorg 专项回归入口：[src/btc/usdb-indexer/scripts/run_reorg_regression.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_reorg_regression.sh)
- world-sim 回归入口：[src/btc/usdb-indexer/scripts/regtest_world_sim.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim.sh)
- world-sim reorg 入口：[src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh)
- world-sim determinism 入口：[src/btc/usdb-indexer/scripts/regtest_world_sim_determinism.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_determinism.sh)
- world-sim reorg determinism 入口：[src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh)
- world-sim live reorg soak 入口：[src/btc/usdb-indexer/scripts/run_live_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live_reorg.sh)
- 空业务面高度回退场景：[src/btc/usdb-indexer/scripts/regtest_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_smoke.sh)
- 空业务面同高度场景：[src/btc/usdb-indexer/scripts/regtest_same_height_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_same_height_reorg_smoke.sh)
- restart 高度回退场景：[src/btc/usdb-indexer/scripts/regtest_restart_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_restart_reorg_smoke.sh)
- restart 同高度场景：[src/btc/usdb-indexer/scripts/regtest_restart_same_height_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_restart_same_height_reorg.sh)
- restart 多轮 reorg 场景：[src/btc/usdb-indexer/scripts/regtest_restart_multi_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_restart_multi_reorg_smoke.sh)
- restart 混合 reorg 场景：[src/btc/usdb-indexer/scripts/regtest_restart_hybrid_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_restart_hybrid_reorg_smoke.sh)
- live ord 单块 reorg 场景：[src/btc/usdb-indexer/scripts/regtest_live_ord_reorg_transfer_remint.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_reorg_transfer_remint.sh)
- live ord 同高度场景：[src/btc/usdb-indexer/scripts/regtest_live_ord_same_height_reorg_transfer_remint.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_same_height_reorg_transfer_remint.sh)
- live ord 多块 rollback 场景：[src/btc/usdb-indexer/scripts/regtest_live_ord_multi_block_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_multi_block_reorg.sh)
- pending recovery energy failure 场景：[src/btc/usdb-indexer/scripts/regtest_pending_recovery_energy_failure.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_pending_recovery_energy_failure.sh)
- pending recovery transfer reload restart 场景：[src/btc/usdb-indexer/scripts/regtest_pending_recovery_transfer_reload_restart.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_pending_recovery_transfer_reload_restart.sh)

## 分层组织

`usdb-indexer` 当前脚本大致分三层：

1. 通用 smoke 层：
   - `regtest_e2e_smoke.sh`
   - `regtest_scenario_runner.py`
   - 负责基础 RPC 语义、空业务面功能面
2. reorg 专项层：
   - `regtest_reorg_*`
   - `regtest_restart_*`
   - `regtest_pending_recovery_*`
   - 负责 upstream anchor drift、rollback、restart resume、多阶段离线 reorg、pending marker 生命周期
3. live ord 业务层：
   - `regtest_live_ord_*`
   - 负责真实 mint / transfer / remint(prev) 以及多块业务 rollback
4. world-sim 压力层：
   - `regtest_world_sim.sh`
   - `regtest_world_sim_reorg.sh`
   - `regtest_world_sim_determinism.sh`
   - `regtest_world_sim_reorg_determinism.sh`
   - `run_live_reorg.sh`
   - 负责长时间随机业务流、交叉检查、多次 deterministic reorg 注入、同 seed 双跑一致性检查，以及更长时段的 soak

## 共享库提供的能力

`regtest_reorg_lib.sh` 当前封装了以下能力：

1. Bitcoin Core / ord 二进制解析。
2. 工作目录、bitcoind、balance-history、usdb-indexer、ord 服务生命周期。
3. 空 replacement block、wallet funding、live ord inscription/send helper。
4. `balance-history` / `usdb-indexer` RPC 调用与等待同步辅助。
5. SQLite 断言与 marker 生命周期轮询。
6. pass snapshot / pass energy / pass stats / active balance snapshot 断言。
7. `pass_block_commits`、`active_balance_snapshots`、`miner_passes` 当前态检查。
8. runtime fault injection 环境变量透传：
   - `USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES`
   - `USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES`

## 推荐运行方式

常规协议回归：

```bash
bash src/btc/usdb-indexer/scripts/run_regression.sh
```

专项 reorg 回归：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/run_reorg_regression.sh
```

只跑 reorg smoke 和 pending recovery，不跑 live ord：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
RUN_LIVE_ORD_REORG_SUITE=0 \
bash src/btc/usdb-indexer/scripts/run_reorg_regression.sh
```

## 端口策略

`run_reorg_regression.sh` 默认使用独立的端口段并按 `PORT_STRIDE=100` 为每个 case 分配一组端口：

1. `BASE_BTC_RPC_PORT`
2. `BASE_BTC_P2P_PORT`
3. `BASE_BH_RPC_PORT`
4. `BASE_USDB_RPC_PORT`
5. `BASE_ORD_RPC_PORT`

这样每条场景即使顺序执行，也不会依赖上一条场景的端口释放时序。

## 目前边界

当前 `run_regression.sh` 默认还不会自动跑整套 reorg 专项，因为这批场景比基础 smoke 更重，且部分依赖 `ord`。专项回归改为显式入口：

1. 需要时单独执行 `run_reorg_regression.sh`
2. 或在 `run_regression.sh` 中通过环境变量开启

## 后续扩展方向

1. 如果 reorg 场景继续增多，可以把 `run_reorg_regression.sh` 进一步拆成 smoke/live/fault 三个子套件。
2. 如果专项场景开始需要声明式参数矩阵，可以再把 shell runner 演进成 Python 编排器。
3. 如果 world-sim 后续开始覆盖 reorg，可以再决定是否把它并入同一个 runner。
4. 当前已经有独立 `regtest_world_sim_reorg.sh`，但还没有并入默认 `run_regression.sh`。
