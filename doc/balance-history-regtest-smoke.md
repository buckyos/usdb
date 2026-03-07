# Balance-History Regtest Smoke 测试说明

本文档提供一个最小可运行的 smoke 测试流程，目标是：

1. 启动本地 `bitcoind -regtest` 单节点；
2. 启动真实 `balance-history` 服务；
3. 通过 RPC 验证服务网络类型与同步高度。

脚本位置：

- [regtest_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_smoke.sh)

## 前置条件

1. 已安装并可执行：
   - `bitcoind`
   - `bitcoin-cli`
   - `cargo`
   - `curl`
2. 当前仓库可正常构建 `balance-history`：
   - `cargo check --manifest-path src/btc/Cargo.toml -p balance-history`

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_smoke.sh
```

成功标志：

1. `get_network_type` 返回 `regtest`；
2. `get_block_height` 达到脚本设置的目标高度（默认 `120`）；
3. 输出 `Smoke test succeeded.`。

## 可调参数（环境变量）

脚本支持以下环境变量：

1. `WORK_DIR`：工作目录（默认自动创建临时目录）。
2. `BITCOIN_DIR`：regtest 数据目录（默认 `${WORK_DIR}/bitcoin`）。
3. `BALANCE_HISTORY_ROOT`：balance-history 根目录（默认 `${WORK_DIR}/balance-history`）。
4. `BTC_RPC_PORT`：bitcoind RPC 端口（默认 `19443`）。
5. `BH_RPC_PORT`：balance-history RPC 端口（默认 `18080`）。
6. `WALLET_NAME`：regtest 钱包名（默认 `bhitest`）。
7. `TARGET_HEIGHT`：要挖的区块高度（默认 `120`）。
8. `SYNC_TIMEOUT_SEC`：等待同步超时秒数（默认 `120`）。

示例：

```bash
WORK_DIR=/tmp/usdb-bh-test \
BTC_RPC_PORT=29443 \
BH_RPC_PORT=28080 \
TARGET_HEIGHT=150 \
src/btc/balance-history/scripts/regtest_smoke.sh
```

## 关键实现点

1. 脚本会生成独立 `config.toml`，并强制：
   - `btc.network = "regtest"`
   - `btc.rpc_url` 指向脚本启动的 bitcoind
   - `sync.local_loader_threshold` 设置为极大值，优先走 RPC client，避免本地 blk 扫描路径影响 smoke 速度
2. 启动 `balance-history` 时使用：
   - `--root-dir <BALANCE_HISTORY_ROOT>`
   - `--skip-process-lock`
3. 退出时会自动尝试关闭 `balance-history` 与 `bitcoind`。

## 已知限制

1. 这是 smoke 测试，不覆盖地址级余额精确性断言。
2. 目前只验证基础可用性（服务可启动、可同步、RPC 可读）。
3. 更深入的场景测试建议基于此脚本扩展（如构造转账交易并校验 `get_address_balance`）。
