# USDB-Indexer Regtest World-Sim Reorg Determinism

本文档描述 `world-sim + deterministic reorg` 的双跑一致性检查。目标不是证明“所有区块哈希字面值都相同”，而是验证同一个 `seed` 下，经过同样的业务动作和同样的 reorg 注入后，报告里的稳定业务摘要仍然可重复。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh)
- [src/btc/usdb-indexer/scripts/regtest_world_sim_determinism.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_determinism.sh)
- [src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh)
- [src/btc/usdb-indexer/scripts/compare_world_sim_reports.py](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/compare_world_sim_reports.py)

## 覆盖目标

1. 顺序运行两次完全相同配置的 `world-sim + reorg`。
2. 两次运行都输出独立 JSONL 报告。
3. 比较器校验以下摘要仍然一致：
   - `session_start` 的 deterministic 配置
   - `session_end.final_metrics`
   - 每个 tick 的关键业务摘要
   - 每次 `event = "reorg"` 的稳定字段
4. 比较器故意不比较下面这些天然会抖动的字段：
   - `ts_ms`
   - `original_tip_hash`
   - `replacement_tip_hash`
   - `balance_history_stable_hash`
   - `usdb_stable_hash`
   - 各类 `elapsed_ms`

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_world_sim_reorg_determinism.sh
```

## 常用环境变量

1. `SIM_SEED`：双跑共用的 seed，默认 `20260318`。
2. `SIM_BLOCKS`：每次运行的 tick 数，默认 `80`。
3. `AGENT_COUNT`：agent 数，默认 `6`。
4. `SIM_POLICY_MODE`：默认 `scripted`。
5. `SIM_SCRIPTED_CYCLE`：默认 `mint,send_balance,transfer,remint,spend_balance,noop`。
6. `SIM_REORG_INTERVAL_BLOCKS`：默认 `20`。
7. `SIM_REORG_DEPTH`：默认 `3`。
8. `SIM_REORG_MAX_EVENTS`：默认 `2`。
9. `WORK_DIR`：双跑总目录，默认 `/tmp/usdb-world-determinism-reorg`。
10. `BASE_BTC_RPC_PORT`、`BASE_BTC_P2P_PORT`、`BASE_BH_RPC_PORT`、`BASE_USDB_RPC_PORT`、`BASE_ORD_SERVER_PORT`：双跑的起始端口组。
11. `PORT_STRIDE`：第二轮相对第一轮的端口偏移，默认 `100`。

## 验收标准

脚本成功时会输出：

```text
Determinism check passed.
```

同时：

1. `run1` 和 `run2` 各自都有 `world-sim-report.jsonl`。
2. 比较器输出 `reports are deterministic`。
3. 如果包含 `reorg` 事件，则两次运行的 `tick / depth / rollback heights / rebuild summary / cross-check summary` 一致。
