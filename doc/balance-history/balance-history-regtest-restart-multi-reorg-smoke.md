# Balance-History Regtest Restart-Multi-Reorg Smoke 测试说明

本文档描述一个“服务每轮都在离线窗口中错过 reorg”的真实 regtest 验证场景，目标是确认 `balance-history` 即使多次经历“先同步旧 tip -> 停服务 -> 节点离线完成 tip reorg -> 重启恢复”，仍能在每轮后重新收敛到新的 canonical，并把该轮同一个 tip block 里多个地址的确认余额一起回退到 `0`。

脚本位置：

- [src/btc/balance-history/scripts/regtest_restart_multi_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_restart_multi_reorg_smoke.sh)
- [src/btc/balance-history/scripts/regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)

## 覆盖目标

1. 启动真实 `bitcoind -regtest` 和真实 `balance-history`。
2. 先补足 coinbase 成熟区块，再同步到场景目标高度。
3. 每一轮都先创建多笔真实转账，分别打到多个 fresh address，再统一挖一个确认块。
4. 每一轮都停掉 `balance-history`，然后在服务离线期间执行一次 tip 级别 `invalidateblock + empty replacement block`。
5. 每一轮重启后都校验：
   - `get_block_commit(current_height)` 收敛到新的 canonical hash。
   - `get_snapshot_info.stable_block_hash` 收敛到同一个新 hash。
   - 该轮所有被跟踪地址余额都恢复为 `0`。

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_restart_multi_reorg_smoke.sh
```

## 可调参数

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BITCOIN_DIR`：regtest 数据目录。
4. `BALANCE_HISTORY_ROOT`：balance-history 根目录。
5. `BTC_RPC_PORT`：bitcoind RPC 端口，默认 `28632`。
6. `BTC_P2P_PORT`：bitcoind P2P 端口，默认 `28633`。
7. `BH_RPC_PORT`：balance-history RPC 端口，默认 `28610`。
8. `WALLET_NAME`：测试钱包名，默认 `bhrestartmultireorg`。
9. `SCENARIO_START_HEIGHT`：在成熟资金高度之上额外推进的初始场景高度，默认 `40`。
10. `REORG_ROUNDS`：服务离线错过 reorg 的轮数，默认 `2`。
11. `TRACKED_TX_COUNT`：每轮在同一个待回滚 tip block 中确认的交易数，默认 `2`。
12. `SEND_AMOUNT_BTC`：每笔确认后再回滚的转账金额，默认 `1.25`。
13. `SYNC_TIMEOUT_SEC`：单次等待同步或收敛超时，默认 `120`。

示例：

```bash
WORK_DIR=/tmp/usdb-bh-restart-multi-reorg \
BTC_RPC_PORT=28632 \
BTC_P2P_PORT=28633 \
BH_RPC_PORT=28610 \
SCENARIO_START_HEIGHT=60 \
REORG_ROUNDS=3 \
TRACKED_TX_COUNT=3 \
SEND_AMOUNT_BTC=1.0 \
src/btc/balance-history/scripts/regtest_restart_multi_reorg_smoke.sh
```

## 场景设计说明

1. 该场景复用了 multi-reorg 的多轮节奏，但把每轮 reorg 都放到服务离线窗口里，重点验证多次重启恢复的稳定性。
2. 每轮都会先创建多笔新交易，再通过一个 tip block 统一确认，因此单轮里可以同时观察多个地址的余额出现与回退信号。
3. 替代块仍使用 `generateblock ... transactions=[]` 显式挖空，避免被回滚交易重新进入替代链。
4. 每轮都会重新检查 `block_commit` 与 `stable_block_hash`，确保不是只在最后一轮才观察到收敛。

## 已知边界

1. 当前覆盖的是多轮 tip reorg，不覆盖单轮深度大于 1 的离线多轮组合回滚。
2. 该脚本当前每轮验证的是同一 block 内的多笔单输出转账，不覆盖复杂交易图。
3. 若后续继续增强，可增加：
   - 离线窗口内连续多次 invalidate/mining
   - UTXO 级别校验