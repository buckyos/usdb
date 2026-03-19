# Balance-History Regtest Stable Lag Smoke 测试说明

本文档说明 `balance-history` 的 `stable_lag` 专项 smoke 场景，目标是验证：

1. `stable_lag` 是协议常量，不是本地运行时配置；
2. `stable_lag` 不只是 RPC 元字段，而是真的参与索引推进上限计算；
3. 当 BTC tip 继续前进时，`get_block_height` / `get_snapshot_info().stable_height` 始终等于 `tip - stable_lag`；
4. `get_snapshot_info().stable_block_hash` 始终对应 `tip - stable_lag` 的 canonical block hash。

脚本位置：

- [regtest_stable_lag_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_stable_lag_smoke.sh)
- [regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)
- [balance-history-regtest-framework.md](/home/bucky/work/usdb/doc/balance-history/balance-history-regtest-framework.md)

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
src/btc/balance-history/scripts/regtest_stable_lag_smoke.sh
```

默认参数下，脚本会：

1. 挖到 BTC tip `20`；
2. 启动真实 `balance-history`；
3. 从 `get_snapshot_info().stable_lag` 读取当前协议 lag；
4. 验证服务 stable height 收敛到 `tip - stable_lag`；
5. 再继续挖 `3` 个块；
6. 再次验证服务 stable height 仍然收敛到新的 `tip - stable_lag`。

成功标志：

1. `get_block_height == get_snapshot_info().stable_height`
2. `get_snapshot_info().stable_lag` 与实际索引行为一致
3. `get_snapshot_info().stable_block_hash == getblockhash(tip - get_snapshot_info().stable_lag)`
4. 输出 `Stable lag smoke test succeeded.`

## 可调参数（环境变量）

1. `WORK_DIR`：工作目录（默认自动创建临时目录）
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录（默认 `/home/bucky/btc/bitcoin-28.1/bin`）
3. `BTC_RPC_PORT`：bitcoind RPC 端口（默认 `29832`）
4. `BTC_P2P_PORT`：bitcoind P2P 端口（默认 `29833`）
5. `BH_RPC_PORT`：balance-history RPC 端口（默认 `29810`）
6. `WALLET_NAME`：regtest 钱包名（默认 `bhstablelag`）
7. `TARGET_TIP_HEIGHT`：初始 BTC tip 高度（默认 `20`）
8. `EXTRA_BLOCKS`：初始断言后追加挖的区块数（默认 `3`）
9. `SYNC_TIMEOUT_SEC`：等待稳定高度追平的超时秒数（默认 `120`）

示例：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
BTC_RPC_PORT=29832 \
BTC_P2P_PORT=29833 \
BH_RPC_PORT=29810 \
TARGET_TIP_HEIGHT=30 \
EXTRA_BLOCKS=5 \
src/btc/balance-history/scripts/regtest_stable_lag_smoke.sh
```

## 验收重点

1. `balance-history` 本地 DB 高度本身就是 stable height，而不是先追 tip 再在 RPC 层做减法。
2. `stable_lag` 进入 `SnapshotInfo` 后，元信息与实际索引行为保持一致。
3. `stable_lag` 作为协议常量存在，不依赖本地配置文件。
4. 相同的 canonical tip 和相同的 `stable_lag` 必须导出相同的 stable snapshot identity。
