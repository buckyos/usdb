# USDB-Indexer Regtest Restart Same-Height Reorg

本文档描述 `usdb-indexer` 的 restart same-height reorg smoke，目标是验证：

1. 服务离线期间发生 same-height replacement 时，`usdb-indexer` 不会因为“高度没变”而漏掉 drift。
2. 重启后的 recovery 会把 `snapshot_id`、adopted upstream commit 和 local pass commit anchor 一起切到 replacement chain。
3. replacement height 上不会残留旧链的重复 `pass_block_commits` 或 `active_balance_snapshots`。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_restart_same_height_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_restart_same_height_reorg.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-reorg-plan.md)

## 覆盖目标

1. 三服务先同步到 `TARGET_HEIGHT = H`，记录旧链：
   - `snapshot_id`
   - `latest_block_commit`
   - `get_pass_block_commit(H).balance_history_block_commit`
2. 停掉 `balance-history` 与 `usdb-indexer`。
3. 离线 invalidate 旧 tip，并立即挖一个空 replacement block，使链高仍然保持在 `H`。
4. 重启 `balance-history` 和 `usdb-indexer`：
   - `stable_block_hash` 切到 replacement hash
   - `latest_block_commit` 切到 replacement commit
   - `snapshot_id` 变化
   - 本地 `pass_block_commit` anchor 变化
   - 日志中出现 reorg detect / resume / completed 轨迹
5. replacement height 上 `pass_block_commits` 和 `active_balance_snapshots` 仍然只有一条有效记录。
6. 最后继续挖到 `H+1`，确认服务可继续追新块。

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
bash src/btc/usdb-indexer/scripts/regtest_restart_same_height_reorg.sh
```

## 常用环境变量

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29832 / 29833`。
4. `BH_RPC_PORT`：balance-history RPC 端口，默认 `29810`。
5. `USDB_RPC_PORT`：usdb-indexer RPC 端口，默认 `29820`。
6. `TARGET_HEIGHT`：初始同步高度，默认 `40`。
7. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时，默认 `180`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer restart same-height reorg smoke test succeeded.
```

关键判定是：

1. 最终高度仍为 `H`，但 `snapshot_id`、`latest_block_commit` 和 local pass commit anchor 都已经变化。
2. replacement height 上 `pass_block_commits` / `active_balance_snapshots` 仍然只有一条记录。
3. pending recovery marker 最终被清除，说明 restart 后 recovery 完整走完。
