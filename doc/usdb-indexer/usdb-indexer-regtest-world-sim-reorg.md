# USDB-Indexer Regtest World-Sim Reorg

本文档描述 `world-sim` 的下一阶段组合回归：在持续随机业务流里注入确定性 BTC reorg，并验证 `balance-history`、`usdb-indexer` 和模拟器本地视图在改链后仍能继续一致推进。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh)
- [src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh)
- [src/btc/usdb-indexer/scripts/regtest_world_sim.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim.sh)
- [src/btc/usdb-indexer/scripts/regtest_world_simulator.py](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_simulator.py)
- [doc/usdb-indexer-regtest-world-sim.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim.md)
- [doc/usdb-indexer-regtest-world-sim-reorg-determinism.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim-reorg-determinism.md)

## 覆盖目标

1. 先跑一段带真实 agent 行为的 world-sim。
2. 每隔 `SIM_REORG_INTERVAL_BLOCKS` 个 tick，对最近 `SIM_REORG_DEPTH` 个 canonical blocks 做一次 deterministic replacement：
   - invalidate rollback start block
   - 挖同高度 empty replacement chain
3. 等待 `ord`、`balance-history`、`usdb-indexer` 一起收敛到 replacement tip。
4. reorg 后重建模拟器内部 `owned_passes / active_pass_id / invalid_passes / pass_owner_by_id` 视图，避免后续动作沿用旧链本地缓存。
5. 在 replacement tip 上立即跑一次 global cross-check，再继续后续随机业务。

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh
```

## 常用环境变量

1. `SIM_BLOCKS`：仿真 tick 数，默认 `80`。
2. `AGENT_COUNT`：agent 数，默认 `6`。
3. `SIM_POLICY_MODE`：默认 `scripted`，方便稳定复现。
4. `SIM_SCRIPTED_CYCLE`：默认 `mint,send_balance,transfer,remint,spend_balance,noop`。
5. `SIM_REORG_INTERVAL_BLOCKS`：每隔多少个 tick 注入一次 reorg，默认 `20`。
6. `SIM_REORG_DEPTH`：每次替换最近多少个 blocks，默认 `3`。
7. `SIM_REORG_MAX_EVENTS`：单次运行最多注入多少次 reorg，默认 `2`。
8. `SIM_GLOBAL_CROSS_CHECK_INTERVAL_BLOCKS`：global cross-check 频率，默认 `5`。

## 验收标准

脚本成功时会输出：

```text
World simulation finished successfully.
```

同时结构化报告里应出现：

1. `session_start` 中的 reorg 配置字段。
2. 至少一条 `event = "reorg"` 的 JSONL 记录。
3. `session_end.final_metrics.reorg_ok > 0` 且 `reorg_fail = 0`。

如果还需要验证“同 seed 下带 reorg 的报告仍然可重复”，应继续执行：

```bash
src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh
```
