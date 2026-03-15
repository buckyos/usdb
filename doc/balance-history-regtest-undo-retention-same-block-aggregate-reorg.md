# Balance-History Regtest: Undo-Retention Same-Block Aggregate Reorg

本文档描述一个组合场景：先在线推进链高直到旧 undo 记录已被裁剪，再在 undo retention 热窗口里构造一个“同块多次命中同一地址”的聚合块，最后在服务停止期间离线改链，验证 balance-history 重启后仍能把该聚合块完整回滚。

## 覆盖目标

1. 在线推进 canonical tip，确认服务日志已经出现 undo retention prune 完成记录。
2. 在热窗口内先挖一个 funding block，为两个被跟踪地址生成可花费 UTXO。
3. 在下一个块里同时执行：
   - 一笔多输入聚合交易
   - 一笔额外打回同一地址的转账
4. 验证该高度对目标地址只记录一条聚合后的 delta。
5. 停止服务、离线使该聚合块失效、挖出空 replacement block、重启服务。
6. 验证 replacement height 上所有受影响地址的 exact-height delta 都为空，并且余额回滚到 funding block 后的状态。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_undo_retention_same_block_aggregate_reorg.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_undo_retention_same_block_aggregate_reorg.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_undo_retention_same_block_aggregate_reorg.sh
src/btc/balance-history/scripts/regtest_undo_retention_same_block_aggregate_reorg.sh
```

## 验收标准

脚本成功时会输出 `Undo retention same-block aggregate reorg test succeeded.`。