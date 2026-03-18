# USDB-Indexer Regtest Restart Hybrid-Reorg Smoke

本文档描述 `usdb-indexer` 的 restart hybrid reorg smoke，目标是验证：

1. 服务离线期间如果 canonical tail 先发生一次 tip replacement、再发生一次更深的 replacement，重启后仍能直接对齐最终新链。
2. deep rollback + replay 之后，受影响高度段上的 upstream anchor 不会残留旧链 commit。
3. 这类一次停机窗口内的多阶段改链不会留下 pending marker 或重复快照行。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_restart_hybrid_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_restart_hybrid_reorg_smoke.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-framework.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-framework.md)

## 覆盖目标

1. 先挖一个稳定前缀到 `stable_prefix_height`。
2. 再挖 `DEEP_REORG_DEPTH` 个尾部区块到 `target_height`，让三服务先同步到旧 tip。
3. 停掉 `balance-history` 与 `usdb-indexer`。
4. 在服务离线期间：
   - 先从 `affected_height` 执行一次 tip replacement
   - 再对新的 `affected_height` 再做一次 invalidate，并重建整段 replacement tail
5. 重启服务后验证：
   - `get_snapshot_info.stable_block_hash` 收敛到新的 tip hash
   - `get_pass_block_commit(affected_height)` 和 `get_pass_block_commit(target_height)` 都切到新链 commit
   - 受影响区间内只保留一份 `pass_block_commits` / `active_balance_snapshots`
   - pending marker 已清除

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
bash src/btc/usdb-indexer/scripts/regtest_restart_hybrid_reorg_smoke.sh
```

## 常用环境变量

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `32432 / 32433`。
4. `BH_RPC_PORT`：balance-history RPC 端口，默认 `32410`。
5. `USDB_RPC_PORT`：usdb-indexer RPC 端口，默认 `32420`。
6. `SCENARIO_START_HEIGHT`：稳定前缀在当前高度之上额外推进的高度，默认 `45`。
7. `DEEP_REORG_DEPTH`：最终 replacement tail 的深度，默认 `3`。
8. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时，默认 `180`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer restart hybrid reorg smoke test succeeded.
```

同时应满足：

1. `affected_height` 和 `target_height` 两个关键高度上的 upstream commit 都切到了 replacement chain。
2. 受影响高度段里没有重复 `pass_block_commits` / `active_balance_snapshots`。
3. `snapshot_id` 与旧 tip 不同，且 pending recovery marker 最终为清空状态。
