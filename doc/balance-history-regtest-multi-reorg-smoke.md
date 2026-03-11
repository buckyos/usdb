# Balance-History Regtest Multi-Reorg Smoke 测试说明

本文档描述一个连续多次 reorg 的真实 regtest 验证场景，目标是确认 `balance-history` 在同一高度反复发生 tip 切换时，仍能持续完成回滚、重放和稳定快照收敛。

脚本位置：

- [src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh)
- [src/btc/balance-history/scripts/regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)

## 覆盖目标

1. 启动真实 `bitcoind -regtest` 和真实 `balance-history`。
2. 先同步到固定 `TARGET_HEIGHT`。
3. 对同一 tip 连续执行多轮 `invalidateblock + replacement block`。
4. 每一轮都校验：
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
9. `TARGET_HEIGHT`：初始同步高度，默认 `40`。
10. `REORG_ROUNDS`：连续 reorg 轮数，默认 `2`。
11. `SYNC_TIMEOUT_SEC`：单次等待同步或收敛超时，默认 `120`。

示例：

```bash
WORK_DIR=/tmp/usdb-bh-multi-reorg \
BTC_RPC_PORT=28332 \
BTC_P2P_PORT=28333 \
BH_RPC_PORT=28310 \
TARGET_HEIGHT=60 \
REORG_ROUNDS=3 \
src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh
```

## 场景设计说明

1. 每轮 replacement block 都会挖到 fresh address，避免出现 duplicate block 模板导致的假失败。
2. 每轮开始前都会先比对节点 tip hash 与 `balance-history` 当前 block commit hash，确认上一轮已经完全收敛。
3. 该脚本仍然是单节点 regtest 场景，重点验证服务自身的 rollback/replay 稳定性，而不是 P2P 网络传播时序。

## 已知边界

1. 该脚本当前固定在同一 `TARGET_HEIGHT` 上连续 reorg，不覆盖更深层级的多高度回滚。
2. 不覆盖地址余额在多轮 reorg 前后的细粒度断言，只覆盖 block commit 和 stable snapshot 的链对齐。
3. 若后续继续增强，可增加：
   - 更深回滚高度
   - 多轮后再追加新交易和余额断言
   - 重启 `balance-history` 后继续 reorg 的恢复场景