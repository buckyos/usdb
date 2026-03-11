# Balance-History Regtest: Snapshot Recovery

本文档说明 snapshot 生成与恢复的一条端到端 regtest 场景，目标是验证 snapshot 不只是“文件可生成”，而是可以把 stable state 从一个 root 迁移到另一个全新 root，并在恢复后继续追新块。

## 覆盖目标

该场景覆盖以下语义：

1. 在自定义 `root_dir` 下生成 snapshot，文件落点正确。
2. `install-snapshot` 可以把 snapshot 安装到另一个空 root。
3. 恢复后的服务对外 `get_snapshot_info` 与 `get_block_commit` 与源库保持一致。
4. 恢复后的当前 stable 余额查询与 live UTXO 查询和源库 stable state 一致。
5. 恢复后的服务可以继续从同一条 regtest 链上同步新块，而不是停在 snapshot 高度。

注意：当前 snapshot 安装语义恢复的是 stable state，不会重建 snapshot 高度之前的完整历史余额时间线。因此该场景重点校验 snapshot 高度处的稳定视图，以及恢复后向前继续索引的行为。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_snapshot_recovery.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_snapshot_recovery.sh)

## 默认端口

1. Bitcoin RPC: `29232`
2. Bitcoin P2P: `29233`
3. 源 balance-history RPC: `29210`
4. 恢复后 balance-history RPC: `29211`

## 关键环境变量

1. `WORK_DIR`: 总工作目录。
2. `BALANCE_HISTORY_ROOT`: 源库 root。
3. `RESTORE_BALANCE_HISTORY_ROOT`: 恢复目标 root。
4. `BTC_RPC_PORT`: Bitcoin Core RPC 端口。
5. `BTC_P2P_PORT`: Bitcoin Core P2P 端口。
6. `BH_RPC_PORT`: 源服务 RPC 端口。
7. `RESTORE_BH_RPC_PORT`: 恢复后服务 RPC 端口。
8. `BITCOIN_BIN_DIR`: Bitcoin Core 二进制目录。

## 场景步骤

1. 启动 bitcoind，并预热成熟 coinbase 资金。
2. 在源 root 启动 balance-history，构造一段包含“收入、精确花费、再次收入”的地址历史。
3. 记录 snapshot 高度对应的 `stable_block_hash` 和 `block_commit`。
4. 停掉源服务，并通过 `create-snapshot --block-height <stable_height> --with-utxo true` 生成 snapshot。
5. 计算 snapshot 文件的 SHA-256，并在恢复 root 上通过 `install-snapshot --file ... --hash ...` 安装。
6. 启动恢复后的服务，校验：
   - `get_snapshot_info` 一致
   - `get_block_commit` 一致
   - 当前 stable 余额查询一致
   - live UTXO 的存在/不存在语义一致
7. 在恢复后继续发送新交易并挖块，确认服务能从 snapshot 高度继续追上链上新状态。

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_snapshot_recovery.sh
src/btc/balance-history/scripts/regtest_snapshot_recovery.sh
```

如果要并行运行，请显式覆盖端口，例如：

```bash
WORK_DIR=/tmp/usdb-bh-snapshot-recovery-debug \
BTC_RPC_PORT=29332 \
BTC_P2P_PORT=29333 \
BH_RPC_PORT=29310 \
RESTORE_BH_RPC_PORT=29311 \
src/btc/balance-history/scripts/regtest_snapshot_recovery.sh
```

## 验收标准

脚本成功时会输出 `Snapshot recovery test succeeded.`。若失败，脚本会自动打印 bitcoind 与两个 balance-history 实例的日志尾部，便于定位是 snapshot 生成、安装还是恢复后继续同步哪一段出错。