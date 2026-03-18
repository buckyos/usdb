# USDB-Indexer Live Ord Multi-Block Reorg

本文档描述 `usdb-indexer` `Case 7` 的 live ord 多块 rollback 回归，目标是验证：

1. 真实 `bitcoind + ord + balance-history + usdb-indexer` 链路下，连续多块业务状态迁移可以先完整落链。
2. 当 reorg 一次性回退三块业务块时，`miner_passes` 当前态会按 surviving history 正确重建，而不是残留旧链状态。
3. rollback 后的 replacement 链上，`leaderboard`、单 pass `energy`、`pass snapshot` 和 `active balance snapshot` 会重新对齐到新链。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_live_ord_multi_block_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_multi_block_reorg.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-reorg-plan.md)

## 场景编排

1. 用 ord CLI 构造旧链业务面：
   - `mint(pass1)`
   - `transfer(pass1 -> wallet_b)`
   - `remint(prev=pass1) -> pass2`
   - `penalty baseline funding`
   - `penalty spend`
   - `duplicate remint(prev=pass1) -> pass3`
2. 启动 `balance-history` 和 `usdb-indexer`，确认旧链 tip 上：
   - `pass1 = consumed`
   - `pass2 = dormant`
   - `pass3 = active`
   - `get_pass_energy_leaderboard(scope=active)` 的 top1 是 `pass3`
   - SQLite `miner_passes` 当前态表同样是 `consumed / dormant / active`
3. invalidate 第一个 penalty block，一次性回退：
   - penalty baseline
   - penalty spend
   - duplicate remint
4. 验证 rollback ancestor 上：
   - `pass1 = consumed`
   - `pass2 = active`
   - `pass3` 已消失
   - future `pass_block_commits` / `active_balance_snapshots` / pending marker 都被清掉
   - SQLite `miner_passes` 只剩两条 current row
5. 再挖三个空 replacement block 回到原 tip 高度，确认 replacement 链上仍然只有 `pass1 + pass2` 两条有效 pass。
6. 最后再追一块，验证服务还能继续同步。

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_multi_block_reorg.sh
```

## 常用环境变量

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `ORD_BIN`：ord 可执行文件路径。
4. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29732 / 29733`。
5. `ORD_RPC_PORT`：ord HTTP 端口，默认 `29730`。
6. `BH_RPC_PORT`：balance-history RPC 端口，默认 `29710`。
7. `USDB_RPC_PORT`：usdb-indexer RPC 端口，默认 `29720`。
8. `PREMINE_BLOCKS`：预挖矿块数，默认 `130`。
9. `INSCRIBE_CONFIRM_BLOCKS`：首个 mint 的确认块数，默认 `2`。
10. `TRANSFER_CONFIRM_BLOCKS` / `REMINT_CONFIRM_BLOCKS`：transfer 和 remint 的确认块数，默认都为 `1`。
11. `PENALTY_FUND_CONFIRM_BLOCKS` / `PENALTY_SPEND_CONFIRM_BLOCKS`：penalty 两步的确认块数，默认都为 `1`。
12. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时，默认 `300`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer live ord multi-block reorg test succeeded.
```

核心验收点：

1. 多块 rollback 后，`miner_passes` 当前态表从 `3` 条恢复为 `2` 条，`pass3` 被彻底清理。
2. replacement 链 tip 上：
   - `pass1` 仍是 `consumed`
   - `pass2` 恢复为 `active`
   - `pass3` 在 `get_pass_snapshot` 中返回 `null`，在 `get_pass_energy` 中返回 `ENERGY_NOT_FOUND`
3. `get_pass_energy_leaderboard(scope=active)` 的 top1 从旧链 `pass3` 切换为 replacement 链 `pass2`，并与单 pass `get_pass_energy` 返回的 energy 数值一致。
4. `get_snapshot_info.latest_block_commit` 和本地 `get_pass_block_commit(H)` anchor 已切到 replacement chain。
