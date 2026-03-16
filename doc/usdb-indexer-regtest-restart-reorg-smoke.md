# USDB-Indexer Regtest Restart Reorg Smoke

本文档描述 `usdb-indexer` 的 restart height-regression reorg smoke，目标是验证：

1. `balance-history` 与 `usdb-indexer` 在离线期间发生改链后，重启仍能发现 upstream anchor drift。
2. `usdb-indexer` 重启后会执行 durable rollback + pending recovery，而不是卡在旧链高度。
3. rollback 完成后 future `pass_block_commits`、`active_balance_snapshots` 和 pending marker 都被清理干净。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_restart_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_restart_reorg_smoke.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-reorg-plan.md)

## 覆盖目标

1. 三服务先同步到 `TARGET_HEIGHT = H`。
2. 停掉 `balance-history` 和 `usdb-indexer`。
3. 离线 invalidate tip，使链高回到 `H-1`。
4. 先启动 `balance-history`，再启动 `usdb-indexer`：
   - `balance-history` 回到 `H-1`
   - `usdb-indexer` 也回到 `H-1`
   - 日志中出现：
     - `Detected upstream anchor drift`
     - `Resuming pending upstream reorg recovery`
     - `Pending upstream reorg recovery completed`
5. 再挖 replacement block 回到 `H`，确认 replay 后 adopted anchor 与 local pass block commit anchor 都切到新链。
6. 最后继续再挖一块，确认服务还能继续同步。

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
bash src/btc/usdb-indexer/scripts/regtest_restart_reorg_smoke.sh
```

## 常用环境变量

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29732 / 29733`。
4. `BH_RPC_PORT`：balance-history RPC 端口，默认 `29710`。
5. `USDB_RPC_PORT`：usdb-indexer RPC 端口，默认 `29720`。
6. `TARGET_HEIGHT`：初始同步高度，默认 `40`。
7. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时，默认 `180`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer restart height-regression reorg smoke test succeeded.
```

同时应满足：

1. restart 后 `usdb-indexer` 的本地高度先回到 `H-1`，而不是停留在旧链 `H`。
2. 本地 DB 中没有 `> H-1` 的 future `pass_block_commits` / `active_balance_snapshots`。
3. replacement block 到来后，`get_snapshot_info.latest_block_commit` 与 `get_pass_block_commit(H)` 都更新到新链。
