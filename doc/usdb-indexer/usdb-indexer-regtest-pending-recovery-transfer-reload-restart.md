# USDB-Indexer Pending Recovery Transfer Reload Restart

本文档描述 `Case 9` 的 fault injection 回归，目标是验证：

1. `pass rollback + energy rollback` 已完成后，如果 `transfer_tracker.reload_from_storage()` 被测试故障持续阻断，pending marker 不会被误清除。
2. 进程退出后，marker 会跨重启保留下来。
3. 重启后的 `usdb-indexer` 会优先进入 `resume_pending_upstream_reorg_recovery()`，而不是依赖再次检测 drift。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_pending_recovery_transfer_reload_restart.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_pending_recovery_transfer_reload_restart.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-reorg-plan.md)

## 场景编排

1. 先跑一条空业务面 height-regression reorg baseline。
2. 分配本次独立的 release 文件路径，通过 `USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_RELEASE_FILE` 传给 `usdb-indexer`；文件尚未创建时持续注入 transfer reload 失败。
3. invalidate tip 触发 rollback。
4. 等待第一次 pending recovery 在 `transfer reload` 前被故意打断。
5. 断言：
   - 日志里出现 `Injected reorg recovery transfer reload failure`
   - pending marker 已写入 rollback target
6. 保持故障开启并停止 `usdb-indexer`，随后再次读取 SQLite，确认 marker 已持久化。
7. 创建 release 文件并重启 `usdb-indexer`，允许新进程完成恢复。
8. 断言：
   - 重启后 marker 被读取并最终清除
   - `Pending upstream reorg recovery completed` 已写入日志
   - replacement block 和 `H+1` 继续正常同步

## 运行示例

```bash
BITCOIN_BIN_DIR=/path/to/bitcoin/bin \
bash src/btc/usdb-indexer/scripts/regtest_pending_recovery_transfer_reload_restart.sh
```

## 常用环境变量

1. `TARGET_HEIGHT`：reorg tip 高度，默认 `40`。
2. `REGTEST_RECOVERY_OBSERVER_DELAY_SEC`：看到故障日志后刻意延迟观察，默认 `2` 秒；可设为 `5` 验证更慢的测试调度。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29932 / 29933`。
4. `BH_RPC_PORT` / `USDB_INDEXER_RPC_PORT`：服务 RPC 端口，默认 `29910 / 29920`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer pending recovery transfer reload restart test succeeded.
```

核心验收点：

1. transfer reload 失败后，pending marker 跨进程重启仍然存在。
2. 重启后优先恢复 pending recovery，而不是等待新的 upstream drift。
3. marker 清除后 replacement chain 和 `H+1` 追块继续正常。

## 时序与失败证据

这些用例使用显式释放握手，不要求测试脚本在首次重试的 0.8–1.2 秒内读到 pending marker。默认观察延迟已经超过该窗口；故障在确认状态前持续存在，测试仍受 `SYNC_TIMEOUT_SEC` 限制。每次运行分配新的 release 路径，避免上次测试留下的文件提前放行。

Release 文件变量仅用于故障注入，未设置时不会访问文件或改变正常重试策略。原有按次数注入的 `*_FAILURES` 接口仍保留；这两个用例将对应次数设为 `0`，由 release 文件控制恢复。

通过 `run_reorg_regression.sh` 批量运行时，可设置 `REGTEST_WORK_ROOT`。每个用例及重复运行都会创建独立子目录；nightly runner 将其放入会上传的工作目录，归档实际的 `usdb-indexer/logs/usdb-indexer_rCURRENT.log`。测试失败时，控制台诊断也会输出该文件末尾的日志。
