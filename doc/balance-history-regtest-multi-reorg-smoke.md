# Balance-History Regtest Multi-Reorg Smoke 测试说明

本文档描述一个连续多次 reorg 的真实 regtest 验证场景，目标是确认 `balance-history` 在连续多轮“交易确认 -> tip 回滚 -> 替代块重放”过程中，仍能持续完成回滚、重放、余额恢复和稳定快照收敛。

脚本位置：

- [src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh)
- [src/btc/balance-history/scripts/regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)

## 覆盖目标

1. 启动真实 `bitcoind -regtest` 和真实 `balance-history`。
2. 先补足 coinbase 成熟区块，再同步到场景目标高度。
3. 每一轮都先创建一笔真实转账并挖块确认，再执行 `invalidateblock + replacement block`。
4. 每一轮都校验：
   - `get_address_balance` 对应地址的余额先出现、后回退为 `0`。
   - `get_block_commit(TARGET_HEIGHT)` 收敛到新的 canonical hash。
   - `get_snapshot_info.stable_block_hash` 收敛到同一个新 hash。

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh
```

## 可调参数

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BITCOIN_DIR`：regtest 数据目录。
4. `BALANCE_HISTORY_ROOT`：balance-history 根目录。
5. `BTC_RPC_PORT`：bitcoind RPC 端口，默认 `28332`。
6. `BTC_P2P_PORT`：bitcoind P2P 端口，默认 `28333`。
7. `BH_RPC_PORT`：balance-history RPC 端口，默认 `28310`。
8. `WALLET_NAME`：测试钱包名，默认 `bhmultireorg`。
9. `SCENARIO_START_HEIGHT`：在成熟资金高度之上额外推进的初始场景高度，默认 `40`。
10. `REORG_ROUNDS`：连续 reorg 轮数，默认 `2`。
11. `SEND_AMOUNT_BTC`：每轮确认后再回滚的转账金额，默认 `1.25`。
12. `SYNC_TIMEOUT_SEC`：单次等待同步或收敛超时，默认 `120`。

示例：

```bash
WORK_DIR=/tmp/usdb-bh-multi-reorg \
BTC_RPC_PORT=28332 \
BTC_P2P_PORT=28333 \
BH_RPC_PORT=28310 \
SCENARIO_START_HEIGHT=60 \
REORG_ROUNDS=3 \
SEND_AMOUNT_BTC=1.0 \
src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh
```

## 场景设计说明

1. 每轮都会先创建新地址并发送一笔真实转账，确保 reorg 前后有可观测的余额差异。
2. 脚本会先自动补足 `coinbase` 成熟区块，避免 `sendtoaddress` 因资金未成熟而失败。
3. 每轮在 `invalidateblock` 后都会使用 `generateblock` 显式挖空替代块，避免 mempool 中的原交易重新进入替代链。
4. 每轮 replacement block 都会挖到 fresh address，避免出现 duplicate block 模板导致的假失败。
5. 每轮开始前都会先比对节点 tip hash 与 `balance-history` 当前 block commit hash，确认上一轮已经完全收敛。
6. 该脚本仍然是单节点 regtest 场景，重点验证服务自身的 rollback/replay 稳定性，而不是 P2P 网络传播时序。

## 已知边界

1. 该脚本当前主要覆盖 tip 级别的连续 reorg，不覆盖一次性深度大于 1 的回滚。
2. 该脚本只验证单地址单笔转账的回滚，不覆盖复杂交易图。
3. 若后续继续增强，可增加：
   - 多地址或多输入交易的回滚断言
   - UTXO 级别校验
   - 重启 `balance-history` 后继续 reorg 的恢复场景