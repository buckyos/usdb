# USDB-Indexer Regtest E2E Smoke

该文档说明如何一键启动本地 regtest 三服务链路：

1. `bitcoind (regtest)`
2. `balance-history`
3. `usdb-indexer`

并完成最小联调断言：

1. 两个 RPC 服务都返回 `regtest`
2. `balance-history` 与 `usdb-indexer` 同步高度达到目标高度
3. 可选转账断言：`get_address_balance` 返回值与发送金额一致

脚本位置：

- [regtest_e2e_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_e2e_smoke.sh)

## 前置条件

1. 可用的 Bitcoin Core 二进制（默认路径 `/home/bucky/btc/bitcoin-28.1/bin`）
2. 本地可执行 `cargo`、`curl`、`python3`
3. 仓库可构建：
   - `cargo check --manifest-path src/btc/Cargo.toml -p balance-history -p usdb-indexer`

## 运行方式

在仓库根目录执行：

```bash
src/btc/usdb-indexer/scripts/regtest_e2e_smoke.sh
```

## 常用环境变量

1. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录
2. `WORK_DIR`：临时工作目录
3. `BTC_RPC_PORT`：bitcoind RPC 端口
4. `BH_RPC_PORT`：balance-history RPC 端口
5. `USDB_RPC_PORT`：usdb-indexer RPC 端口
6. `TARGET_HEIGHT`：初始出块高度
7. `SYNC_TIMEOUT_SEC`：同步超时秒数
8. `ENABLE_TRANSFER_CHECK`：是否执行转账断言（默认 `1`）
9. `SEND_AMOUNT_BTC`：转账断言金额（默认 `1.0`）
10. `MIN_SPENDABLE_BLOCK_HEIGHT`：转账断言所需最小可花费高度（默认 `101`）
11. `CURL_CONNECT_TIMEOUT_SEC`：RPC 连接超时秒数（默认 `2`）
12. `CURL_MAX_TIME_SEC`：单个 RPC 请求最大耗时秒数（默认 `5`）

示例：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
TARGET_HEIGHT=101 \
BTC_RPC_PORT=19460 \
BH_RPC_PORT=18091 \
USDB_RPC_PORT=18111 \
src/btc/usdb-indexer/scripts/regtest_e2e_smoke.sh
```

## 关键实现说明

1. `usdb-indexer` 使用 `bitcoind` 铭文源，并关闭 ord 依赖监控：
   - `usdb.inscription_source = "bitcoind"`
   - `usdb.monitor_ord_enabled = false`
2. 脚本启动 `usdb-indexer` 时显式传入：
   - `--root-dir <USDB_INDEXER_ROOT>`
   - `--skip-process-lock`
3. 退出时会自动按顺序停止：
   - `usdb-indexer` RPC `stop`
   - `balance-history` RPC `stop`
   - `bitcoind` RPC `stop`
4. 当开启转账断言且 `TARGET_HEIGHT < MIN_SPENDABLE_BLOCK_HEIGHT` 时，脚本会自动提高有效出块高度，确保 coinbase 可花费。
