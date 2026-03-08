# Balance-History Regtest Smoke 测试说明

本文档提供一个最小可运行的 smoke 测试流程，目标是：

1. 启动本地 `bitcoind -regtest` 单节点；
2. 启动真实 `balance-history` 服务；
3. 通过 RPC 验证服务网络类型与同步高度；
4. 构造一笔真实转账并校验 `get_address_balance` 返回值。

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
3. 转账校验开启时（默认开启）`get_address_balance` 的余额与发送金额严格一致；
4. 输出 `Smoke test succeeded.`。

## 可调参数（环境变量）

脚本支持以下环境变量：

1. `WORK_DIR`：工作目录（默认自动创建临时目录）。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录（默认 `/home/bucky/btc/bitcoin-28.1/bin`，若不存在则回退 PATH）。
3. `BITCOIN_DIR`：regtest 数据目录（默认 `${WORK_DIR}/bitcoin`）。
4. `BALANCE_HISTORY_ROOT`：balance-history 根目录（默认 `${WORK_DIR}/balance-history`）。
5. `BTC_RPC_PORT`：bitcoind RPC 端口（默认 `28132`）。
6. `BH_RPC_PORT`：balance-history RPC 端口（默认 `28110`）。
7. `WALLET_NAME`：regtest 钱包名（默认 `bhitest`）。
8. `TARGET_HEIGHT`：要挖的区块高度（默认 `120`）。
9. `SYNC_TIMEOUT_SEC`：等待同步超时秒数（默认 `120`）。
10. `ENABLE_TRANSFER_CHECK`：是否执行真实转账与余额断言（`1` 开启，默认 `1`）。
11. `SEND_AMOUNT_BTC`：转账校验金额（BTC，默认 `1.25`）。

示例：

```bash
WORK_DIR=/tmp/usdb-bh-test \
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
BTC_RPC_PORT=28132 \
BH_RPC_PORT=28110 \
TARGET_HEIGHT=150 \
SEND_AMOUNT_BTC=1.0 \
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
3. 可选转账断言步骤会：
   - 创建接收地址并发送 `SEND_AMOUNT_BTC`
   - 出块确认后按该高度调用 `get_address_balance`
   - 断言余额与发送金额对应 satoshi 严格一致
4. 退出时会自动尝试关闭 `balance-history` 与 `bitcoind`。

## 已知限制

1. 该脚本只覆盖单地址单笔转账的正确性，不覆盖复杂交易图（多输入/多输出/找零策略差异）。
2. 不覆盖重组（reorg）与异常恢复场景。
3. 更深入场景（如多地址批量、范围查询、回滚恢复）建议在此脚本基础上扩展。
