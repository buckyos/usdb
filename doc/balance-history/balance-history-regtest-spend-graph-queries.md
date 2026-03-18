# Balance-History Regtest: Spend Graph Queries

本文档说明一个面向复杂交易图查询语义的 regtest 场景，目标是验证多地址 fanout、定向 spend 和批量范围查询在真实链上的结果一致性。

## 覆盖目标

1. 构造单笔 fanout 交易，把资金同时发到多个被跟踪地址。
2. 使用指定 outpoint 的原始交易，把已跟踪 UTXO 再分发到其他被跟踪地址与未跟踪地址，形成可预测的 spend graph。
3. 验证单地址余额范围查询在多个高度上的 delta 序列。
4. 验证批量 `get_addresses_balances` 与 `get_addresses_balances_delta` 的顺序保持、精确高度、范围结果和缺失值语义。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_spend_graph_queries.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_spend_graph_queries.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_spend_graph_queries.sh
src/btc/balance-history/scripts/regtest_spend_graph_queries.sh
```

## 验收标准

脚本成功时会输出 `Spend graph query test succeeded.`。