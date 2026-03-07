# USDB-Indexer Regtest E2E Smoke

该文档说明如何一键启动本地 regtest 三服务链路：

1. `bitcoind (regtest)`
2. `balance-history`
3. `usdb-indexer`

并完成最小联调断言：

1. 两个 RPC 服务都返回 `regtest`
2. `balance-history` 与 `usdb-indexer` 同步高度达到目标高度
3. `usdb-indexer` RPC 语义断言（`get_rpc_info`、`get_sync_status`、`get_active_passes_at_height`、`get_invalid_passes`、`get_active_balance_snapshot`）
4. 可选转账断言：`get_address_balance` 返回值与发送金额一致

脚本位置：

- [regtest_e2e_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_e2e_smoke.sh)
- [regtest_scenario_runner.py](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_scenario_runner.py)
- [transfer_balance_assert.json](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/scenarios/transfer_balance_assert.json)

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

1. `SCENARIO_RUNNER`：Python 场景执行器路径（默认仓库内 `regtest_scenario_runner.py`）
2. `SCENARIO_FILE`：可选 JSON 场景文件路径（为空时走内置默认场景）
3. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录
4. `WORK_DIR`：临时工作目录
5. `BTC_RPC_PORT`：bitcoind RPC 端口
6. `BH_RPC_PORT`：balance-history RPC 端口
7. `USDB_RPC_PORT`：usdb-indexer RPC 端口
8. `TARGET_HEIGHT`：初始出块高度
9. `SYNC_TIMEOUT_SEC`：同步超时秒数
10. `ENABLE_TRANSFER_CHECK`：是否执行转账断言（默认 `1`）
11. `SEND_AMOUNT_BTC`：转账断言金额（默认 `1.0`）
12. `MIN_SPENDABLE_BLOCK_HEIGHT`：转账断言所需最小可花费高度（默认 `101`）
13. `CURL_CONNECT_TIMEOUT_SEC`：RPC 连接超时秒数（默认 `2`）
14. `CURL_MAX_TIME_SEC`：单个 RPC 请求最大耗时秒数（默认 `5`）

示例：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
TARGET_HEIGHT=101 \
BTC_RPC_PORT=19460 \
BH_RPC_PORT=18091 \
USDB_RPC_PORT=18111 \
src/btc/usdb-indexer/scripts/regtest_e2e_smoke.sh
```

使用自定义 JSON 场景文件：

```bash
SCENARIO_FILE=/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/scenarios/transfer_balance_assert.json \
SEND_AMOUNT_BTC=0.25 \
src/btc/usdb-indexer/scripts/regtest_e2e_smoke.sh
```

## 关键实现说明

1. `usdb-indexer` 使用 `bitcoind` 铭文源，并关闭 ord 依赖监控：
   - `usdb.inscription_source = "bitcoind"`
   - `usdb.monitor_ord_enabled = false`
2. shell 脚本仅负责服务编排（启动/停止/配置），核心链上断言由 Python 场景脚本执行。
3. Python 场景支持步骤类型：
   - `log`
   - `wait_balance_history_synced`
   - `wait_usdb_synced`
   - `assert_usdb_state`
   - `send_and_confirm`
   - `assert_balance_history_balance`
4. 脚本启动 `usdb-indexer` 时显式传入：
   - `--root-dir <USDB_INDEXER_ROOT>`
   - `--skip-process-lock`
5. 退出时会自动按顺序停止：
   - `usdb-indexer` RPC `stop`
   - `balance-history` RPC `stop`
   - `bitcoind` RPC `stop`
6. 当开启转账断言或传入 `SCENARIO_FILE` 且 `TARGET_HEIGHT < MIN_SPENDABLE_BLOCK_HEIGHT` 时，Python 场景脚本会自动提高有效高度，确保 coinbase 可花费。
7. 当前 smoke 场景未构造 USDB 铭文，预期断言为：
   - active pass 列表为空
   - invalid pass 列表为空
   - active balance snapshot 为 `0/0`
