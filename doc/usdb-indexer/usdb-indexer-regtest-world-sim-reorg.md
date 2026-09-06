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
   - 回滚前从原链区块保存非 coinbase 交易原文及高度，实际断开深度为 `stable_lag_blocks + SIM_REORG_DEPTH`
   - invalidate rollback start block
   - 按原高度和块内顺序重放交易，保留 commit/reveal 依赖及交易锁定高度；逐块检查交易列表、高度与新块哈希，最后要求 mempool 为空
   - 在原 raw tip 之上挖一个空触发块，使 Ord 0.23.3 开始检查新链的父块
3. 等待 `ord`、`balance-history`、`usdb-indexer` 一起收敛；Ord 必须同时满足 `blockcount == BTC raw tip + 1` 和 tip hash 相等。
4. reorg 后重建模拟器内部 `owned_passes / active_pass_id / invalid_passes / pass_owner_by_id` 视图，避免后续动作沿用旧链本地缓存。
5. 在 replacement tip 上立即跑一次 global cross-check，再继续后续随机业务。
6. weekly 额外启用 `SIM_REPLAY_CHECK_ENABLED=1`，立即保存每次重组后的完整状态；工作结束后用空数据库重新同步 balance-history/indexer，并对照这些历史检查点与最终状态。失败后保留 N+1 恢复点，不重跑工作轮次。范围、门禁及时间预算见 [独立状态重建对照](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim.md#重组后的独立状态重建对照)。

Bitcoin Core 28.1 的 `invalidateblock` 只尝试将最先断开的 10 个块中的交易放回 mempool；深度 3 加稳定滞后 10 会断开 13 个块。因此不能用回滚后的 mempool 代替断链交易清单，否则钱包可能保留未确认交易占用的输入，导致后续 Ord 报 `wallet contains no cardinal utxos`。

`tests/test_regtest_world_reorg.py` 使用真实 Bitcoin Core 与 Ord 覆盖 10、11、13 个断开块边界，确认原 commit/reveal 被重放后，再从同一钱包完成新一次铭刻。可设置 `BITCOIN_BIN_DIR` 和 `ORD_BIN` 后直接执行；weekly world-soak 在长跑前执行此测试。

独立的精确高度重组用例使用 `bitcoind` 铭文后端，先在原目标高度验证 balance-history/indexer 的回滚、替换和历史拒绝，再调用 `regtest_finish_ord_reorg` 推进到 Ord 所需触发高度并验证最终收敛。这保留了等高替换断言，也避免在 Ord 无法检测重组的高度等待它。

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
