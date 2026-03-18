# Balance-History Regtest History-Balance-Oracle 测试说明

本文档描述一个基于真实 `bitcoind -regtest` 与真实 `balance-history` 的历史余额对拍场景。该场景不只验证 tip 余额，而是维护一份独立的测试端账本 oracle，并对比 `(address, block_height)` 二维历史余额查询结果。

脚本位置：

- [src/btc/balance-history/scripts/regtest_history_balance_oracle.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_history_balance_oracle.sh)
- [src/btc/balance-history/scripts/regtest_balance_oracle.py](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_balance_oracle.py)
- [src/btc/balance-history/scripts/regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)

## 覆盖目标

1. 创建一组被跟踪地址和一组未被跟踪地址。
2. 在多个连续区块中，按固定 seed 随机生成多笔真实转账。
3. 每个新区块确认后，由测试端 oracle 读取完整区块交易并更新被跟踪地址余额。
4. 在周期性检查点上，验证：
   - 所有被跟踪地址在当前高度的余额与 oracle 一致。
   - 所有被跟踪地址在一个随机抽样历史高度的余额与 oracle 一致。
5. 在场景结束后，对所有被跟踪地址、所有场景高度做一次全量历史回放校验。

## 场景设计说明

1. 该场景的 oracle 不是预先假设钱包如何选币，而是直接解析每个已确认区块中的真实交易输入输出。
2. 对被跟踪地址的输出会进入 oracle 的 tracked UTXO 集；后续若这些 outpoint 被花费，会从对应地址余额中扣除。
3. 由于使用真实区块数据而不是仅凭发送指令推断，该场景可以自然覆盖：
   - 同块内多笔交易
   - 同地址多次入账
   - 被跟踪地址输出后续被钱包再次花费
   - 资金从被跟踪地址流向未被跟踪地址
4. 当前版本聚焦 `get_address_balance` 的历史正确性，尚未把 `delta`、`range` 和 reorg 合并进同一场景。

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_history_balance_oracle.sh
```

## 可调参数

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BITCOIN_DIR`：regtest 数据目录。
4. `BALANCE_HISTORY_ROOT`：balance-history 根目录。
5. `BTC_RPC_PORT`：bitcoind RPC 端口，默认 `28932`。
6. `BTC_P2P_PORT`：bitcoind P2P 端口，默认 `28933`。
7. `BH_RPC_PORT`：balance-history RPC 端口，默认 `28910`。
8. `WALLET_NAME`：测试钱包名，默认 `bhhistoryoracle`。
9. `ADDRESS_COUNT`：被跟踪地址数，默认 `8`。
10. `UNTRACKED_ADDRESS_COUNT`：未被跟踪地址数，默认 `4`。
11. `BLOCK_COUNT`：场景产生的区块数，默认 `18`。
12. `TXS_PER_BLOCK`：每个区块确认前创建的转账数，默认 `3`。
13. `CHECK_INTERVAL`：每隔多少个区块执行一次检查点对拍，默认 `3`。
14. `SEED`：随机种子，默认 `20260311`。
15. `SEND_AMOUNTS_BTC`：候选发送金额列表，默认 `0.10 0.25 0.50 1.00`。
16. `SYNC_TIMEOUT_SEC`：等待同步超时秒数，默认 `120`。

示例：

```bash
WORK_DIR=/tmp/usdb-bh-history-oracle \
BTC_RPC_PORT=28932 \
BTC_P2P_PORT=28933 \
BH_RPC_PORT=28910 \
ADDRESS_COUNT=6 \
BLOCK_COUNT=12 \
TXS_PER_BLOCK=2 \
CHECK_INTERVAL=2 \
SEED=42 \
src/btc/balance-history/scripts/regtest_history_balance_oracle.sh
```

## 已知边界

1. 当前版本主要覆盖 `get_address_balance` 的历史正确性，不直接覆盖 `get_address_balance_delta` 与 range 接口。
2. 当前场景未引入 reorg；它更适合作为后续 snapshot 等价性与 reorg 历史正确性测试的基础数据生成器。
3. 当前用例依赖钱包的真实选币行为，因此不同 Bitcoin Core 主版本理论上可能生成不同交易图；但在同一版本下，固定 seed 仍可稳定复现。