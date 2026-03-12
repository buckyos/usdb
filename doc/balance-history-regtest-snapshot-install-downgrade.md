# Balance-History Regtest: Snapshot Install Downgrade

本文档说明“旧 snapshot 覆盖较新 live DB”的场景，目标是验证 install-snapshot 的切换语义，以及降级后是否还能重新追上链上更高高度。

## 覆盖目标

1. 同一 root 先同步到较新高度，再安装较旧快照时，stable state 会切回旧快照高度。
2. 降级安装后，较新高度新增的 live UTXO 不应继续可见。
3. 降级安装仍然会保留一份旧 live DB backup，说明 snapshot swap 走的是同一套原子切换逻辑。
4. 放开 `max_sync_block_height` 后，服务可以从旧快照重新追上链上更高高度。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_snapshot_install_downgrade.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_snapshot_install_downgrade.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_snapshot_install_downgrade.sh
src/btc/balance-history/scripts/regtest_snapshot_install_downgrade.sh
```

## 验收标准

脚本成功时会输出 `Snapshot install downgrade test succeeded.`。