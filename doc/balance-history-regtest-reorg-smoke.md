# Balance-History Regtest Reorg Smoke 测试说明

本文档提供一个最小可运行的 reorg smoke 测试流程，目标是验证：

1. 启动真实 `bitcoind -regtest` 与真实 `balance-history` 服务；
2. 先把服务同步到固定高度；
3. 在服务进入等待阶段后，触发一次 tip reorg；
4. 校验 `balance-history` 能把该高度的 block commit 更新到新的 canonical block hash。

脚本位置：

- [src/btc/balance-history/scripts/regtest_reorg_smoke.sh](../src/btc/balance-history/scripts/regtest_reorg_smoke.sh)
- [src/btc/balance-history/scripts/regtest_lib.sh](../src/btc/balance-history/scripts/regtest_lib.sh)
- [doc/balance-history-regtest-framework.md](./balance-history-regtest-framework.md)

## 前置条件

1. 已安装并可执行：
   - `bitcoind`
   - `bitcoin-cli`
   - `cargo`
   - `curl`
   - `python3`
2. 当前仓库可正常构建 `balance-history`：
   - `cargo check --manifest-path src/btc/Cargo.toml -p balance-history`

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_reorg_smoke.sh
```

成功标志：

1. 服务先同步到脚本设定高度；
2. 脚本使 tip block 失效并立即挖出替代块；
3. `get_block_commit(TARGET_HEIGHT)` 最终返回新的 `btc_block_hash`；
4. `get_snapshot_info.stable_block_hash` 最终与新 tip hash 一致；
5. 输出 `Reorg smoke test succeeded.`。

## 这个脚本在验证什么

该脚本重点覆盖的是整体性 reorg 处理链路，而不是单个纯函数：

1. 服务已同步完成后处于等待阶段；
2. 期间 BTC 发生链切换；
3. `balance-history` 能被及时唤醒，而不是长期停留在旧 tip；
4. 检测层找到共同祖先；
5. rollback 与 replay 完成后，稳定快照重新对齐新的 canonical chain。

由于脚本会“invalidate tip 后立即挖出替代块”，实际命中的内部触发路径可能有两种：

1. 服务先观察到 canonical 高度回退；
2. 服务直接观察到同高度 tip hash 已变化。

两种路径对最终正确性都应该收敛到同一结果，因此该 smoke 测试接受这两种实现细节。

## 可调参数（环境变量）

脚本支持以下环境变量：

1. `WORK_DIR`：工作目录（默认自动创建临时目录）。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BITCOIN_DIR`：regtest 数据目录。
4. `BALANCE_HISTORY_ROOT`：balance-history 根目录。
5. `BTC_RPC_PORT`：bitcoind RPC 端口，默认 `28232`。
6. `BTC_P2P_PORT`：bitcoind P2P 端口，默认 `28233`。
7. `BH_RPC_PORT`：balance-history RPC 端口，默认 `28210`。
8. `WALLET_NAME`：regtest 钱包名，默认 `bhreorg`。
9. `TARGET_HEIGHT`：初始同步高度，默认 `40`。
10. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时秒数，默认 `120`。

示例：

```bash
WORK_DIR=/tmp/usdb-bh-reorg \
BTC_RPC_PORT=28232 \
BTC_P2P_PORT=28233 \
BH_RPC_PORT=28210 \
TARGET_HEIGHT=60 \
src/btc/balance-history/scripts/regtest_reorg_smoke.sh
```

## 已知限制

1. 该脚本主要验证 block commit / stable block hash 的链对齐，不覆盖复杂地址余额断言。
2. 该脚本只覆盖单节点 regtest 的 tip reorg，不覆盖多节点网络传播时序。
3. 若后续需要更强验收，可在此基础上继续加入：
   - 指定地址余额在 reorg 前后的变化断言
   - 多次连续 reorg
   - 深度超过 undo 热窗口时的 snapshot / resync 兜底行为
4. 当前脚本已经改为依赖共享的 `regtest_lib.sh`，新的 reorg 场景建议直接在共享生命周期之上叠加断言步骤。