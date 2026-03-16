# USDB-Indexer Pending Recovery Transfer Reload Restart

本文档描述 `Case 9` 的 fault injection 回归，目标是验证：

1. `pass rollback + energy rollback` 已完成后，如果第一次 `transfer_tracker.reload_from_storage()` 失败，pending marker 不会被误清除。
2. 进程退出后，marker 会跨重启保留下来。
3. 重启后的 `usdb-indexer` 会优先进入 `resume_pending_upstream_reorg_recovery()`，而不是依赖再次检测 drift。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_pending_recovery_transfer_reload_restart.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_pending_recovery_transfer_reload_restart.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-reorg-plan.md)

## 场景编排

1. 先跑一条空业务面 height-regression reorg baseline。
2. 用环境变量 `USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES=1` 启动 `usdb-indexer`。
3. invalidate tip 触发 rollback。
4. 等待第一次 pending recovery 在 `transfer reload` 前被故意打断。
5. 断言：
   - 日志里出现 `Injected reorg recovery transfer reload failure`
   - pending marker 已写入 rollback target
6. 在自动重试前停止 `usdb-indexer`，保留 marker。
7. 清除注入环境变量并重启 `usdb-indexer`。
8. 断言：
   - 重启后 marker 被读取并最终清除
   - `Pending upstream reorg recovery completed` 已写入日志
   - replacement block 和 `H+1` 继续正常同步

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
bash src/btc/usdb-indexer/scripts/regtest_pending_recovery_transfer_reload_restart.sh
```

## 常用环境变量

1. `TARGET_HEIGHT`：reorg tip 高度，默认 `40`。
2. `USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES`：注入失败次数，默认 `1`。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29932 / 29933`。
4. `BH_RPC_PORT` / `USDB_RPC_PORT`：服务 RPC 端口，默认 `29910 / 29920`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer pending recovery transfer reload restart test succeeded.
```

核心验收点：

1. transfer reload 失败后，pending marker 跨进程重启仍然存在。
2. 重启后优先恢复 pending recovery，而不是等待新的 upstream drift。
3. marker 清除后 replacement chain 和 `H+1` 追块继续正常。
