# USDB-Indexer Pending Recovery Energy Failure

本文档描述 `Case 8` 的 fault injection 回归，目标是验证：

1. `pass` rollback 已经 durable 成功后，如果第一次 `energy` recovery 故意失败，pending marker 仍会保留。
2. 后续重试不依赖再次检测 upstream drift，也能继续完成 recovery。
3. recovery 完成后 pending marker 会被清除，服务仍可继续 replay replacement block。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_pending_recovery_energy_failure.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_pending_recovery_energy_failure.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-reorg-plan.md)

## 场景编排

1. 先跑一条空业务面 height-regression reorg baseline。
2. 用环境变量 `USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES=1` 启动 `usdb-indexer`。
3. invalidate tip 触发 rollback。
4. 等待第一次 pending recovery 在 `energy rollback` 前被故意打断。
5. 断言：
   - SQLite `state.upstream_reorg_recovery_pending_height` 已写入 rollback target
   - `usdb-indexer` 运行日志里出现 `Injected reorg recovery energy failure`
6. 不重启进程，直接等待下一轮 `sync_once()` 自动重试。
7. 断言：
   - pending marker 被清除
   - `Pending upstream reorg recovery completed` 已写入日志
   - replacement block 和后续 `H+1` 仍能正常同步

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
bash src/btc/usdb-indexer/scripts/regtest_pending_recovery_energy_failure.sh
```

## 常用环境变量

1. `TARGET_HEIGHT`：reorg tip 高度，默认 `40`。
2. `USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES`：注入失败次数，默认 `1`。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29832 / 29833`。
4. `BH_RPC_PORT` / `USDB_RPC_PORT`：服务 RPC 端口，默认 `29810 / 29820`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer pending recovery energy failure test succeeded.
```

核心验收点：

1. 第一次 recovery 失败后，pending marker 仍然存在。
2. 不需要再次挖块或再次检测 drift，服务会自己重试并完成 recovery。
3. marker 清除后 replacement chain 和 `H+1` 追块继续正常。
