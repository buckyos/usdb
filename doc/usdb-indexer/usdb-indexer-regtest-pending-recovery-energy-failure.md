# USDB-Indexer Pending Recovery Energy Failure

本文档描述 `Case 8` 的 fault injection 回归，目标是验证：

1. `pass` rollback 已经 durable 成功后，如果 `energy` recovery 被测试故障持续阻断，pending marker 仍会保留。
2. 后续重试不依赖再次检测 upstream drift，也能继续完成 recovery。
3. recovery 完成后 pending marker 会被清除，服务仍可继续 replay replacement block。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_pending_recovery_energy_failure.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_pending_recovery_energy_failure.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-reorg-plan.md)

## 场景编排

1. 先跑一条空业务面 height-regression reorg baseline。
2. 在本次临时目录中分配一个尚不存在的 release 文件，通过 `USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_RELEASE_FILE` 传给 `usdb-indexer`。文件创建前，每次 energy recovery 都会注入失败。
3. invalidate tip 触发 rollback。
4. 等待第一次 pending recovery 在 `energy rollback` 前被故意打断。
5. 断言：
   - SQLite `state.upstream_reorg_recovery_pending_height` 已写入 rollback target
   - `usdb-indexer` 运行日志里出现 `Injected reorg recovery energy failure`
6. 确认 pending 状态后创建 release 文件；不重启进程，等待下一轮 `sync_once()` 自动重试。
7. 断言：
   - pending marker 被清除
   - `Pending upstream reorg recovery completed` 已写入日志
   - replacement block 和后续 `H+1` 仍能正常同步

## 运行示例

```bash
BITCOIN_BIN_DIR=/path/to/bitcoin/bin \
bash src/btc/usdb-indexer/scripts/regtest_pending_recovery_energy_failure.sh
```

## 常用环境变量

1. `TARGET_HEIGHT`：reorg tip 高度，默认 `40`。
2. `REGTEST_RECOVERY_OBSERVER_DELAY_SEC`：看到故障日志后刻意延迟观察，默认 `2` 秒；可设为 `5` 验证更慢的测试调度。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29832 / 29833`。
4. `BH_RPC_PORT` / `USDB_INDEXER_RPC_PORT`：服务 RPC 端口，默认 `29810 / 29820`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer pending recovery energy failure test succeeded.
```

核心验收点：

1. 第一次 recovery 失败后，pending marker 仍然存在。
2. 不需要再次挖块或再次检测 drift，服务会自己重试并完成 recovery。
3. marker 清除后 replacement chain 和 `H+1` 追块继续正常。

## 时序与失败证据

这些用例使用显式释放握手，不要求测试脚本在首次重试的 0.8–1.2 秒内读到 pending marker。默认观察延迟已经超过该窗口；故障在确认状态前持续存在，测试仍受 `SYNC_TIMEOUT_SEC` 限制。每次运行分配新的 release 路径，避免上次测试留下的文件提前放行。

Release 文件变量仅用于故障注入，未设置时不会访问文件或改变正常重试策略。原有按次数注入的 `*_FAILURES` 接口仍保留；这两个用例将对应次数设为 `0`，由 release 文件控制恢复。

通过 `run_reorg_regression.sh` 批量运行时，可设置 `REGTEST_WORK_ROOT`。每个用例及重复运行都会创建独立子目录；nightly runner 将其放入会上传的工作目录，归档实际的 `usdb-indexer/logs/usdb-indexer_rCURRENT.log`。测试失败时，控制台诊断也会输出该文件末尾的日志。
