# Balance-History Regtest Deep-Reorg Smoke 测试说明

本文档描述一个“深回滚”真实 regtest 验证场景，目标是确认 `balance-history` 在一次性回滚多个高度时，仍能正确完成回滚、重放以及受影响地址余额的恢复。

脚本位置：

- [src/btc/balance-history/scripts/regtest_deep_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_deep_reorg_smoke.sh)
- [src/btc/balance-history/scripts/regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)

## 覆盖目标

1. 先补足 coinbase 成熟区块，再挖出一个稳定前缀高度。
2. 在即将被回滚的第一层区块里确认一笔真实转账。
3. 再继续挖出剩余尾部区块，形成 `REORG_DEPTH` 深度的原始链尾。
4. 通过一次 `invalidateblock` 让节点回退到稳定前缀。
5. 重新挖出 `REORG_DEPTH` 个替代块，并验证：
   - `get_block_commit(TARGET_HEIGHT)` 收敛到新的 tip hash。
   - `get_snapshot_info.stable_block_hash` 收敛到新的 tip hash。
   - 原先在被回滚链尾中确认的地址余额恢复为 `0`。

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_deep_reorg_smoke.sh
```

## 可调参数

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BITCOIN_DIR`：regtest 数据目录。
4. `BALANCE_HISTORY_ROOT`：balance-history 根目录。
5. `BTC_RPC_PORT`：bitcoind RPC 端口，默认 `28432`。
6. `BTC_P2P_PORT`：bitcoind P2P 端口，默认 `28433`。
7. `BH_RPC_PORT`：balance-history RPC 端口，默认 `28410`。
8. `WALLET_NAME`：测试钱包名，默认 `bhdeepreorg`。
9. `SCENARIO_START_HEIGHT`：在成熟资金高度之上额外推进的原始链尾高度，默认 `45`。
10. `REORG_DEPTH`：一次性回滚深度，默认 `3`。
11. `SEND_AMOUNT_BTC`：被回滚链尾中确认的转账金额，默认 `1.25`。
12. `SYNC_TIMEOUT_SEC`：单次等待同步或收敛超时，默认 `120`。

示例：

```bash
WORK_DIR=/tmp/usdb-bh-deep-reorg \
BTC_RPC_PORT=28432 \
BTC_P2P_PORT=28433 \
BH_RPC_PORT=28410 \
SCENARIO_START_HEIGHT=60 \
REORG_DEPTH=4 \
SEND_AMOUNT_BTC=1.0 \
src/btc/balance-history/scripts/regtest_deep_reorg_smoke.sh
```

## 场景设计说明

1. 触发回滚时直接 invalidate 被影响区间的第一个区块，从而一次性回滚整个尾部链段。
2. 脚本会先自动补足 `coinbase` 成熟区块，避免被跟踪转账因资金未成熟而失败。
3. 被观察的余额变化来自真正落在“被回滚区间”内的转账，因此比 tip-only 场景更能说明深回滚对历史状态的影响。
4. `invalidateblock` 后会使用 `generateblock` 显式挖空替代块，避免被回滚交易再次进入替代链。
5. 替代链上的区块全部挖到 fresh address，避免重建出与原链尾完全相同的块模板。

## 已知边界

1. 该脚本当前验证的是单地址单笔转账，不覆盖复杂交易图。
2. 不覆盖超过 undo 热窗口后的兜底恢复路径。
3. 若后续继续增强，可增加：
   - 多笔交易跨多个被回滚高度的断言
   - UTXO 级别校验
   - 重启后继续 deep reorg 的恢复场景