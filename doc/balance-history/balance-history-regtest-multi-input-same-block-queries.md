# Balance-History Regtest: Multi-Input Same-Block Queries

本文档说明一个面向多输入聚合与同块多次命中地址语义的 regtest 场景，目标是验证单个高度内针对同一 script hash 的多次增减会被聚合为一条逻辑 balance delta。

## 覆盖目标

1. 先把资金分别发到两个被跟踪地址，形成两个可控输入 UTXO。
2. 通过一笔多输入原始交易同时花费这两个被跟踪 UTXO，并把部分资金重新打回其中一个地址。
3. 在同一个区块里再追加一笔到该地址的普通转账，制造“同一高度多次命中同一地址”的真实链场景。
4. 验证精确高度 delta、future-height at-or-before 余额查询，以及批量 range/delta 查询都只返回聚合后的单条高度记录。

## 入口脚本

- [src/btc/balance-history/scripts/regtest_multi_input_same_block_queries.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_multi_input_same_block_queries.sh)

## 运行示例

```bash
chmod +x src/btc/balance-history/scripts/regtest_multi_input_same_block_queries.sh
src/btc/balance-history/scripts/regtest_multi_input_same_block_queries.sh
```

## 验收标准

脚本成功时会输出 `Multi-input same-block query test succeeded.`。