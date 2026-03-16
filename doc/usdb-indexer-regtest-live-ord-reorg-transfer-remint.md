# USDB-Indexer Live Ord Reorg Transfer/Remint

本文档描述 `usdb-indexer` 第二阶段第一条 live ord reorg 专项回归，目标是验证：

1. 真实 `bitcoind + ord + balance-history + usdb-indexer` 链路下，`mint -> transfer -> remint(prev)` 业务态可以先完整落链。
2. 当承载 `remint(prev)` 的 tip block 被回退时，`usdb-indexer` 会把 pass state、energy、active balance snapshot 一起回滚到 transfer height。
3. replacement block 不再包含 remint 交易时，旧链上的新 pass 会消失，不留下 future pass / future energy / future snapshot。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_live_ord_reorg_transfer_remint.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_reorg_transfer_remint.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer-regtest-reorg-plan.md)

## 场景编排

1. 预挖矿并启动独立 `ord` 服务。
2. 通过 ord CLI 构造：
   - `mint(pass_old)`
   - `transfer(pass_old -> wallet_b)`
   - `remint(prev=pass_old) -> pass_new`
3. 启动 `balance-history` 和 `usdb-indexer`，先验证旧链业务态：
   - `pass_old` 在 remint height 为 `dormant`
   - `pass_new` 在 remint height 为 `active`
   - `get_active_balance_snapshot(remint_height)` 为正值
4. invalidate remint 所在 tip，等待高度回退到 transfer height。
5. 验证 rollback 后业务态：
   - `pass_old` 恢复为 `dormant`
   - `pass_new` 不再存在
   - `get_pass_energy(pass_new, at_or_before)` 返回 `ENERGY_NOT_FOUND`
   - `get_active_balance_snapshot(transfer_height)` 变为 `0/0`
6. 再挖一个空 replacement block 回到原高度，确认 replacement 链继续保持 “只有 transfer，没有 remint” 的状态。
7. 最后再挖一个空块，验证服务还能继续同步。

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_reorg_transfer_remint.sh
```

## 常用环境变量

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `ORD_BIN`：ord 可执行文件路径。
4. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29532 / 29533`。
5. `ORD_RPC_PORT`：ord HTTP 端口，默认 `29530`。
6. `BH_RPC_PORT`：balance-history RPC 端口，默认 `29510`。
7. `USDB_RPC_PORT`：usdb-indexer RPC 端口，默认 `29520`。
8. `PREMINE_BLOCKS`：预挖矿块数，默认 `130`。
9. `INSCRIBE_CONFIRM_BLOCKS`：首个 mint 的确认块数，默认 `2`。
10. `TRANSFER_CONFIRM_BLOCKS` / `REMINT_CONFIRM_BLOCKS`：transfer 和 remint 的确认块数，默认都为 `1`。
11. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时，默认 `300`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer live ord height-regression reorg transfer/remint test succeeded.
```

核心验收点：

1. rollback 到 transfer height 后，future `pass_block_commits`、`active_balance_snapshots` 和 pending recovery marker 都被清干净。
2. replacement height 上：
   - `pass_old` 仍然是 `dormant`
   - `pass_new` 彻底消失
   - active balance snapshot 为 `0/0`
3. `get_snapshot_info.latest_block_commit` 和 `get_pass_block_commit(H).balance_history_block_commit` 已切到 replacement chain。
