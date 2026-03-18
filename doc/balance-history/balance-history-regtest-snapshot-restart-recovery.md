# Balance-History Regtest: Snapshot Restart Recovery

本文档说明 snapshot 安装恢复后的重启场景，目标是验证恢复出来的 stable state 在服务重启前后保持一致，并且重启后仍能继续追新块。

## 覆盖目标

1. snapshot 安装后的 `get_snapshot_info`、`get_block_commit`、当前 stable 余额和 live UTXO 在首次启动时正确。
2. 恢复后的服务重启一次后，上述状态保持不变。
3. 恢复后的服务在重启后继续同步新块，状态推进正确。
4. 追到新块后再次重启，新的 stable state 依然保持一致。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_snapshot_restart_recovery.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_snapshot_restart_recovery.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_snapshot_restart_recovery.sh
src/btc/balance-history/scripts/regtest_snapshot_restart_recovery.sh
```

## 验收标准

脚本成功时会输出 `Snapshot restart recovery test succeeded.`。
