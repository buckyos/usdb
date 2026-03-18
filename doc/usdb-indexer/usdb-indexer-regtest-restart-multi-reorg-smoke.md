# USDB-Indexer Regtest Restart Multi-Reorg Smoke

本文档描述 `usdb-indexer` 的 restart multi-reorg smoke，目标是验证：

1. 服务经历多轮“停机 -> tip replacement -> 重启”后，仍能持续完成 anchor drift 检测、rollback 和 replay。
2. same-height replacement 不会只在第一轮有效，重复轮次里 `snapshot_id`、`latest_block_commit` 和本地 `pass_block_commit` 仍会切到新链。
3. 多轮恢复后不会残留 pending marker、重复 `pass_block_commits` 或重复 `active_balance_snapshots`。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_restart_multi_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_restart_multi_reorg_smoke.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-framework.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-framework.md)

## 覆盖目标

1. 三服务先同步到 `TARGET_HEIGHT = H`。
2. 之后连续执行 `REORG_ROUNDS` 轮：
   - 先挖一个原始 tip block 到 `H + round`
   - 停掉 `balance-history` 和 `usdb-indexer`
   - 离线 invalidate 这个 tip，再挖一个 empty replacement block 保持高度不变
   - 重启服务并等待新链收敛
3. 每一轮都验证：
   - `snapshot_id` 变化
   - `latest_block_commit` 变化
   - `get_pass_block_commit(height)` 的 upstream anchor 变化
   - replacement height 上只有一条 `pass_block_commits` / `active_balance_snapshots`
   - pending marker 已清除
4. 最后再追一块，确认服务还能继续同步。

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
bash src/btc/usdb-indexer/scripts/regtest_restart_multi_reorg_smoke.sh
```

## 常用环境变量

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `32332 / 32333`。
4. `BH_RPC_PORT`：balance-history RPC 端口，默认 `32310`。
5. `USDB_RPC_PORT`：usdb-indexer RPC 端口，默认 `32320`。
6. `TARGET_HEIGHT`：初始同步高度，默认 `40`。
7. `REORG_ROUNDS`：重复 tip replacement 轮数，默认 `3`。
8. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时，默认 `180`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer restart multi reorg smoke test succeeded.
```

同时应满足：

1. 每一轮 replacement 后，`get_snapshot_info.stable_block_hash` 都切到新块 hash。
2. 每一轮 replacement 后，`snapshot_id`、`latest_block_commit`、`get_pass_block_commit(height)` 都不再沿用旧链结果。
3. 本地 DB 不会在 replacement height 留下重复行，也不会残留 pending recovery marker。
