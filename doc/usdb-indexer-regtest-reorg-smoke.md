# USDB-Indexer Regtest Reorg Smoke

本文档描述 `usdb-indexer` 的第一条空业务面 reorg smoke，目标是验证：

1. 真实 `bitcoind + balance-history + usdb-indexer` 三服务启动后能同步到固定高度。
2. tip 高度回退时，`usdb-indexer` 会跟随上游 stable anchor 一起回滚到共同祖先。
3. 回滚后 `pass_block_commits`、`active_balance_snapshots` 和 adopted upstream snapshot anchor 不留下 future data。
4. replacement block 到来后，`usdb-indexer` 能继续 replay 并恢复正常同步。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_smoke.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-reorg-plan.md)

## 覆盖目标

1. 初始同步到 `TARGET_HEIGHT` 时：
   - `get_sync_status.synced_block_height == TARGET_HEIGHT`
   - `get_snapshot_info` adopted anchor 对齐当前 upstream stable hash / latest commit
   - 空系统下 `get_active_balance_snapshot(TARGET_HEIGHT)` 为 `0/0`
2. 使 tip `H` 失效但暂不立即挖 replacement block：
   - `balance-history` synced height 回到 `H-1`
   - `usdb-indexer` synced height 也回到 `H-1`
   - 本地 SQLite 中 `pass_block_commits` 和 `active_balance_snapshots` 不再保留 `> H-1` 的 future row
3. 再挖 replacement block 回到 `H`：
   - `balance-history` block commit 切到新的 canonical hash
   - `usdb-indexer get_snapshot_info` 跟随切到新的 adopted anchor
   - `get_pass_block_commit(H)` 的 upstream anchor commit 更新为 replacement commit
4. 最后继续再挖一块，确认服务能继续正常同步。

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
bash src/btc/usdb-indexer/scripts/regtest_reorg_smoke.sh
```

## 常用环境变量

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29332 / 29333`。
4. `BH_RPC_PORT`：balance-history RPC 端口，默认 `29310`。
5. `USDB_RPC_PORT`：usdb-indexer RPC 端口，默认 `29320`。
6. `TARGET_HEIGHT`：初始同步高度，默认 `40`。
7. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时，默认 `180`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer height-regression reorg smoke test succeeded.
```

同时应满足：

1. rollback 阶段 `usdb-indexer` 确实回到 `H-1`。
2. replay 后 `get_snapshot_info.stable_block_hash` 收敛到 replacement tip hash。
3. 额外再挖一个新区块后，两个服务都能继续追高。
