# Balance-History Regtest: Restart Same-Block Aggregate Reorg

本文档描述一个组合场景：先在同一个区块内对同一地址制造多次命中并形成聚合后的 delta，再在服务停止期间离线改链，验证 balance-history 重启后会把整块聚合结果完整回滚。

## 覆盖目标

1. 先构造两个被跟踪地址的可花费 UTXO。
2. 在同一个新区块里完成一笔多输入聚合交易，并额外再向其中一个地址发送一笔转账，形成同块内多次命中。
3. 启动阶段确认该高度只记录一条聚合后的 delta。
4. 停止服务、离线使该聚合块失效、挖出空 replacement block。
5. 重启服务并验证：
   - block commit 切到 replacement tip
   - 被聚合块带来的地址余额全部回滚
   - range 查询中该聚合高度记录消失

## 入口脚本

- [src/btc/balance-history/scripts/regtest_restart_same_block_aggregate_reorg.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_restart_same_block_aggregate_reorg.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_restart_same_block_aggregate_reorg.sh
src/btc/balance-history/scripts/regtest_restart_same_block_aggregate_reorg.sh
```

## 验收标准

脚本成功时会输出 `Restart same-block aggregate reorg test succeeded.`。