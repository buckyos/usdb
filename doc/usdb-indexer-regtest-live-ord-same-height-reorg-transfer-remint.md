# USDB-Indexer Live Ord Same-Height Reorg Transfer/Remint

本文档描述 `usdb-indexer` 第二阶段第二条 live ord same-height reorg 回归，目标是验证：

1. 旧 tip `H` 含 `remint(prev)` 业务块时，same-height replacement 不会因为“高度没变”而漏掉 drift。
2. `snapshot_id`、adopted upstream block commit、local pass block commit anchor 都会切到 replacement chain。
3. 被 replacement 掉的 remint 结果会从 pass state、energy、active balance snapshot 中一并消失。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_live_ord_same_height_reorg_transfer_remint.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_same_height_reorg_transfer_remint.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-reorg-plan.md)

## 场景编排

1. 用 ord CLI 构造旧链业务面：
   - `mint(pass_old)`
   - `transfer(pass_old -> wallet_b)`
   - `remint(prev=pass_old) -> pass_new`
2. 启动 `balance-history` 和 `usdb-indexer`，确认旧链上：
   - `pass_old` 为 `dormant`
   - `pass_new` 为 `active`
   - `get_active_balance_snapshot(H)` 为正值
3. 记录旧链 `snapshot_id`、`latest_block_commit` 和本地 `pass_block_commit` anchor。
4. invalidate 旧 tip，并立即挖一个空 replacement block，使链高重新回到 `H`。
5. 验证 replacement 后：
   - `snapshot_id` 变化
   - `latest_block_commit` 变化
   - `get_pass_block_commit(H).balance_history_block_commit` 变化
   - `pass_old` 仍为 `dormant`
   - `pass_new` 不再存在
   - `get_active_balance_snapshot(H)` 为 `0/0`
6. 再继续挖一个空块，确认服务可继续同步到 `H+1`。

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_same_height_reorg_transfer_remint.sh
```

## 常用环境变量

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `ORD_BIN`：ord 可执行文件路径。
4. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29632 / 29633`。
5. `ORD_RPC_PORT`：ord HTTP 端口，默认 `29630`。
6. `BH_RPC_PORT`：balance-history RPC 端口，默认 `29610`。
7. `USDB_RPC_PORT`：usdb-indexer RPC 端口，默认 `29620`。
8. `PREMINE_BLOCKS`：预挖矿块数，默认 `130`。
9. `INSCRIBE_CONFIRM_BLOCKS`：首个 mint 的确认块数，默认 `2`。
10. `TRANSFER_CONFIRM_BLOCKS` / `REMINT_CONFIRM_BLOCKS`：transfer 和 remint 的确认块数，默认都为 `1`。
11. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时，默认 `300`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer live ord same-height reorg transfer/remint test succeeded.
```

关键判定不是单纯“高度仍然等于 `H`”，而是：

1. `snapshot_id` 和 upstream/local commit anchor 都已经切到 replacement chain。
2. 被撤销的 `pass_new` 在 `get_pass_snapshot` 中返回 `null`，在 `get_pass_energy` 中返回 `ENERGY_NOT_FOUND`。
3. replacement height 上 `active_balance_snapshot` 和 pass stats 都与 “只有 transfer，没有 remint” 的链状态一致。
