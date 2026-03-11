# Balance-History Regtest Restart-Hybrid-Reorg Smoke 测试说明

本文档描述一个“服务离线期间先发生 tip reorg，再发生 deep reorg”的真实 regtest 验证场景，目标是确认 `balance-history` 在一次停机窗口中经历多阶段 canonical 切换后，仍能在重启后把余额状态和 UTXO 当前态一起恢复到新链结果。

脚本位置：

- [src/btc/balance-history/scripts/regtest_restart_hybrid_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_restart_hybrid_reorg_smoke.sh)
- [src/btc/balance-history/scripts/regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)

## 覆盖目标

1. 先构造一个稳定前缀。
2. 在稳定前缀之后的 tip block 中一次确认多笔转账。
3. 再在更深的尾段里确认另一笔转账，并继续挖出剩余尾段区块。
4. 让 `balance-history` 先同步到旧 tip，并校验：
   - 所有被跟踪地址余额存在。
   - 所有被跟踪 outpoint 的 UTXO 当前态存在。
5. 停掉 `balance-history`，在服务离线期间先执行一次 tip reorg，再执行一次 deep reorg，并重建整段替代链。
6. 重启 `balance-history` 后校验：
   - `get_block_commit(TARGET_HEIGHT)` 收敛到新的 canonical hash。
   - `get_snapshot_info.stable_block_hash` 收敛到同一个新 hash。
   - 所有被跟踪地址余额恢复为 `0`。
   - 所有被跟踪 outpoint 在 UTXO 当前态中消失。

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_restart_hybrid_reorg_smoke.sh
```

## 可调参数

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BITCOIN_DIR`：regtest 数据目录。
4. `BALANCE_HISTORY_ROOT`：balance-history 根目录。
5. `BTC_RPC_PORT`：bitcoind RPC 端口，默认 `28732`。
6. `BTC_P2P_PORT`：bitcoind P2P 端口，默认 `28733`。
7. `BH_RPC_PORT`：balance-history RPC 端口，默认 `28710`。
8. `WALLET_NAME`：测试钱包名，默认 `bhrestarthybridreorg`。
9. `SCENARIO_START_HEIGHT`：稳定前缀在成熟资金高度之上额外推进的高度，默认 `45`。
10. `TIP_TX_COUNT`：最先进入 tip block、随后被 tip reorg 回滚的交易数，默认 `2`。
11. `DEEP_REORG_DEPTH`：后续 deep reorg 的尾段深度，默认 `3`。
12. `SEND_AMOUNT_BTC`：每笔被跟踪交易金额，默认 `1.25`。
13. `SYNC_TIMEOUT_SEC`：单次等待同步或收敛超时，默认 `120`。

## 场景设计说明

1. 该场景把“tip 回滚”和“deep 回滚”放进同一个离线窗口里，验证一次重启要同时吸收两段 canonical 变化。
2. 除地址余额外，还会对被跟踪 outpoint 做 UTXO 当前态断言，因此能直接观察 rollback 后 live UTXO 是否被正确删除。
3. tip 段里被跟踪的 wallet outpoint 会在确认后立即 `lockunspent false` 锁定，避免后续 deep 转账错误复用这些输入，污染回滚断言。
4. 替代链继续使用 `generateblock ... transactions=[]` 显式挖空，避免旧交易重新进入新链。
5. 场景里的 `get_utxo` 查询走的是服务端最小 RPC，并要求 outpoint 参数使用 `"txid:vout"` 的字符串形式。

## 已知边界

1. 当前 UTXO 级校验仍基于单输出转账，不覆盖复杂多输入多输出花费图。
2. 该场景聚焦当前态 UTXO 是否正确删除，不额外校验 rollback 后恢复出的历史 spent UTXO 内容。
3. 测试脚本虽然优先走优雅 `stop` RPC，但仍保留超时后的强制退出兜底，以避免失败用例卡住清理流程。