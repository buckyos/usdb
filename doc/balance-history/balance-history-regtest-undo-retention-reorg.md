# Balance-History Regtest: Undo Retention Reorg

本文档说明低 `undo_retention_blocks` 配置下的热窗口 reorg 场景，目标是验证 undo journal 已经发生裁剪后，最近窗口内的 reorg 仍然可以在服务重启后正确回滚恢复。

## 覆盖目标

1. 使用较小的 `undo_retention_blocks` 和较高频的 `undo_cleanup_interval_blocks`，确认运行期确实发生 undo 裁剪。
2. 在已裁剪旧 undo 的前提下，对最新热窗口内的区块执行离线 reorg。
3. 服务重启后可以成功检测 reorg、回滚本地状态并跟上替代链。
4. 被 reorg 掉的尾部转账余额会回到 0，`get_snapshot_info` 会收敛到替代 tip hash。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_undo_retention_reorg.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_undo_retention_reorg.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_undo_retention_reorg.sh
src/btc/balance-history/scripts/regtest_undo_retention_reorg.sh
```

## 验收标准

脚本成功时会输出 `Undo retention reorg test succeeded.`。