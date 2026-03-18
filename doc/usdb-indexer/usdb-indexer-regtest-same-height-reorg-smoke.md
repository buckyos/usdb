# USDB-Indexer Regtest Same-Height Reorg Smoke

本文档描述 `usdb-indexer` 的空业务面 same-height reorg smoke，目标是验证：

1. replacement block 与旧 block 高度相同，但 canonical hash / latest block commit 已变化。
2. `usdb-indexer` 不会因为“高度没变”而漏掉 upstream anchor 漂移。
3. adopted upstream snapshot info、local pass block commit anchor 和 snapshot id 都会切到 replacement chain。

## 入口脚本

- [src/btc/usdb-indexer/scripts/regtest_same_height_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_same_height_reorg_smoke.sh)
- [src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh)
- [doc/usdb-indexer-regtest-reorg-plan.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-reorg-plan.md)

## 覆盖目标

1. 三服务先同步到 `TARGET_HEIGHT = H`。
2. 记录 old tip 的：
   - BTC block hash
   - balance-history block commit
   - `usdb-indexer get_snapshot_info.snapshot_id`
   - `usdb-indexer get_pass_block_commit(H).balance_history_block_commit`
3. 触发 same-height replacement：
   - invalidate 旧 tip
   - 立即挖一个空 replacement block，使链高重新回到 `H`
4. 验证最终状态：
   - `balance-history` 在 `H` 的 block commit hash 已更新
   - `usdb-indexer get_snapshot_info.stable_block_hash` 已更新
   - `usdb-indexer get_snapshot_info.latest_block_commit` 已更新
   - `usdb-indexer get_snapshot_info.snapshot_id` 已变化
   - `usdb-indexer get_pass_block_commit(H).balance_history_block_commit` 已变化
5. 再继续挖一块，验证服务可继续同步到 `H+1`。

## 运行示例

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
bash src/btc/usdb-indexer/scripts/regtest_same_height_reorg_smoke.sh
```

## 常用环境变量

1. `WORK_DIR`：工作目录，默认自动创建临时目录。
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录。
3. `BTC_RPC_PORT` / `BTC_P2P_PORT`：bitcoind 端口，默认 `29432 / 29433`。
4. `BH_RPC_PORT`：balance-history RPC 端口，默认 `29410`。
5. `USDB_RPC_PORT`：usdb-indexer RPC 端口，默认 `29420`。
6. `TARGET_HEIGHT`：初始同步高度，默认 `40`。
7. `SYNC_TIMEOUT_SEC`：同步与 reorg 收敛超时，默认 `180`。

## 验收标准

脚本成功时会输出：

```text
USDB indexer same-height reorg smoke test succeeded.
```

成功的核心判定不是“服务没挂”，而是：

1. 最终高度仍然是 `H`。
2. 但 adopted upstream anchor 与 local pass commit anchor 都已经切到了 replacement chain。
3. snapshot id 也随之变化，证明这不是旧链状态被误保留。
