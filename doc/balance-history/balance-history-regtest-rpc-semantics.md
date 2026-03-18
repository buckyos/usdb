# Balance-History Regtest RPC-Semantics 测试说明

本文档描述一个专门覆盖 `balance-history` RPC 语义边界的 regtest 场景。该场景的重点不是 reorg，而是把接口文档里定义的 latest、exact、range、batch 顺序和 live UTXO 语义固定成回归测试。

脚本位置：

- [src/btc/balance-history/scripts/regtest_rpc_semantics.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_rpc_semantics.sh)
- [src/btc/balance-history/scripts/regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)

## 覆盖目标

1. 验证 `get_address_balance` 的 latest 语义。
2. 验证 `get_address_balance` 的 `block_height` 查询使用 at-or-before 语义。
3. 验证 `get_address_balance_delta` 的 `block_height` 查询使用 exact 语义。
4. 验证 `block_range` 的空区间与非空区间返回形态。
5. 验证批量接口输出顺序与输入顺序严格一致，重复输入不去重。
6. 验证 `get_live_utxo` 只查询当前 live UTXO 视图，不会回退到历史 outpoint 查询。

## 场景设计说明

1. 场景先构造一组可预测的确认转账，并在每次确认后锁住被跟踪 outpoint，避免钱包后续选币把语义测试扰乱。
2. 场景显式插入一个空块，用来验证某个高度“没有该地址变更”时，latest 与 delta 的返回差异。
3. 最后再发送一笔未被跟踪地址的转账，驱动钱包花费已锁定之外的 UTXO，以验证 `get_live_utxo` 在 outpoint 已 spend 后返回 `None`。

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_rpc_semantics.sh
```

## 可调参数

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BITCOIN_DIR`：regtest 数据目录。
4. `BALANCE_HISTORY_ROOT`：balance-history 根目录。
5. `BTC_RPC_PORT`：bitcoind RPC 端口，默认 `29032`。
6. `BTC_P2P_PORT`：bitcoind P2P 端口，默认 `29033`。
7. `BH_RPC_PORT`：balance-history RPC 端口，默认 `29010`。
8. `WALLET_NAME`：测试钱包名，默认 `bhrpcsemantics`。
9. `SYNC_TIMEOUT_SEC`：等待同步超时秒数，默认 `120`。

## 已知边界

1. 当前用例主要覆盖成功路径与已定义语义，不包含非法 script hash、非法 outpoint、future range 等负向输入测试。
2. 当前用例是语义专项，不与 reorg 或 snapshot 组合；后续可以把这些语义断言复用到更复杂场景里。