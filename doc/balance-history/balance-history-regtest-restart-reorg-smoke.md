# Balance-History Regtest Restart-Reorg Smoke 测试说明

本文档描述一个“服务停机期间发生 reorg”真实 regtest 验证场景，目标是确认 `balance-history` 在已经同步到旧 canonical 链后，即使服务离线、节点完成深回滚并重建替代链，重启后仍能正确识别新 canonical，并把旧链尾中多个地址上的多笔确认余额一起回退。

脚本位置：

- [src/btc/balance-history/scripts/regtest_restart_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_restart_reorg_smoke.sh)
- [src/btc/balance-history/scripts/regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)

## 覆盖目标

1. 先补足 coinbase 成熟区块，再挖出稳定前缀高度。
2. 在即将被回滚的尾部区间里连续确认多笔真实转账，分别打到多个 fresh address，并让 `balance-history` 先同步到原始 tip。
3. 停掉 `balance-history`，保证后续 reorg 完全发生在服务离线期间。
4. 对被影响区间的第一块执行一次 `invalidateblock`，让节点回退整段尾部。
5. 在服务离线期间重新挖出 `REORG_DEPTH` 个空替代块。
6. 重启 `balance-history`，并验证：
   - `get_block_commit(TARGET_HEIGHT)` 收敛到新的 tip hash。
   - `get_snapshot_info.stable_block_hash` 收敛到新的 tip hash。
   - 原先仅存在于旧链尾中的每个地址余额都恢复为 `0`。

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_restart_reorg_smoke.sh
```

## 可调参数

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BITCOIN_DIR`：regtest 数据目录。
4. `BALANCE_HISTORY_ROOT`：balance-history 根目录。
5. `BTC_RPC_PORT`：bitcoind RPC 端口，默认 `28532`。
6. `BTC_P2P_PORT`：bitcoind P2P 端口，默认 `28533`。
7. `BH_RPC_PORT`：balance-history RPC 端口，默认 `28510`。
8. `WALLET_NAME`：测试钱包名，默认 `bhrestartreorg`。
9. `SCENARIO_START_HEIGHT`：在成熟资金高度之上额外推进的原始链尾高度，默认 `45`。
10. `REORG_DEPTH`：服务离线期间的一次性回滚深度，默认 `3`。
11. `TRACKED_TX_COUNT`：旧链尾中要确认并回滚的交易数，默认 `2`，且不得大于 `REORG_DEPTH`。
12. `SEND_AMOUNT_BTC`：每笔被回滚交易的转账金额，默认 `1.25`。
13. `SYNC_TIMEOUT_SEC`：单次等待同步或收敛超时，默认 `120`。

示例：

```bash
WORK_DIR=/tmp/usdb-bh-restart-reorg \
BTC_RPC_PORT=28532 \
BTC_P2P_PORT=28533 \
BH_RPC_PORT=28510 \
SCENARIO_START_HEIGHT=60 \
REORG_DEPTH=4 \
TRACKED_TX_COUNT=3 \
SEND_AMOUNT_BTC=1.0 \
src/btc/balance-history/scripts/regtest_restart_reorg_smoke.sh
```

## 场景设计说明

1. 该场景复用了 deep-reorg 的链形状，但把 reorg 发生时机放到服务停机窗口里，重点验证重启恢复路径。
2. 先让服务完整同步旧 tip，再停服务，避免把“在线 reorg 检测”与“重启后恢复”混在一起。
3. 旧链尾里的多笔转账会分布在多个被回滚高度上，因此比单笔单地址更接近真实 rollback 负载。
4. `invalidateblock` 后仍使用 `generateblock ... transactions=[]` 挖空替代块，避免被回滚交易重新进入替代链。
5. 最终余额断言会逐个检查所有旧链尾收款地址，因此能确认回滚结果不仅是 tip hash 收敛，还包括历史状态真正回退。

## 已知边界

1. 当前只覆盖单次离线窗口内的一段 deep reorg；多轮停启交错 reorg 由独立的 restart-multi-reorg 场景覆盖。
2. 不覆盖超过 undo 热窗口后的兜底恢复路径。
3. 若后续继续增强，可增加：
   - 多地址、多笔交易跨多个被回滚高度的断言
   - UTXO 级别校验