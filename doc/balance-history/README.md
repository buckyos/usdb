# Balance-History Docs

本目录收拢 `balance-history` 相关文档，减少 `doc/` 根目录里的混排。

## 设计与接口

- [balance-history-rpc.md](./balance-history-rpc.md)
- [balance-history-rpc_en.md](./balance-history-rpc_en.md)
- [balance-history-rollback数据模型设计.md](./balance-history-rollback%E6%95%B0%E6%8D%AE%E6%A8%A1%E5%9E%8B%E8%AE%BE%E8%AE%A1.md)
- [balance-history-review-remediation-plan.md](./balance-history-review-remediation-plan.md)

## Regtest 框架

- [balance-history-regtest-framework.md](./balance-history-regtest-framework.md)
- [balance-history-regtest-smoke.md](./balance-history-regtest-smoke.md)
- [balance-history-regtest-reorg-smoke.md](./balance-history-regtest-reorg-smoke.md)
- [balance-history-regtest-multi-reorg-smoke.md](./balance-history-regtest-multi-reorg-smoke.md)
- [balance-history-regtest-restart-reorg-smoke.md](./balance-history-regtest-restart-reorg-smoke.md)
- [balance-history-regtest-restart-multi-reorg-smoke.md](./balance-history-regtest-restart-multi-reorg-smoke.md)
- [balance-history-regtest-restart-hybrid-reorg-smoke.md](./balance-history-regtest-restart-hybrid-reorg-smoke.md)

## 快照与恢复

- [balance-history-regtest-snapshot-recovery.md](./balance-history-regtest-snapshot-recovery.md)
- [balance-history-regtest-snapshot-restart-recovery.md](./balance-history-regtest-snapshot-restart-recovery.md)
- [balance-history-regtest-snapshot-install-repeat.md](./balance-history-regtest-snapshot-install-repeat.md)
- [balance-history-regtest-snapshot-install-retry.md](./balance-history-regtest-snapshot-install-retry.md)
- [balance-history-regtest-snapshot-install-failure.md](./balance-history-regtest-snapshot-install-failure.md)
- [balance-history-regtest-snapshot-install-corrupt.md](./balance-history-regtest-snapshot-install-corrupt.md)
- [balance-history-regtest-snapshot-install-downgrade.md](./balance-history-regtest-snapshot-install-downgrade.md)

## 查询语义与专项用例

- [balance-history-regtest-rpc-semantics.md](./balance-history-regtest-rpc-semantics.md)
- [balance-history-regtest-history-balance-oracle.md](./balance-history-regtest-history-balance-oracle.md)
- [balance-history-regtest-spend-graph-queries.md](./balance-history-regtest-spend-graph-queries.md)
- [balance-history-regtest-multi-input-same-block-queries.md](./balance-history-regtest-multi-input-same-block-queries.md)
- [balance-history-regtest-loader-switch.md](./balance-history-regtest-loader-switch.md)
- [balance-history-regtest-deep-reorg-smoke.md](./balance-history-regtest-deep-reorg-smoke.md)
- [balance-history-regtest-undo-retention-reorg.md](./balance-history-regtest-undo-retention-reorg.md)
- [balance-history-regtest-undo-retention-same-block-aggregate-reorg.md](./balance-history-regtest-undo-retention-same-block-aggregate-reorg.md)
- [balance-history-regtest-restart-same-block-aggregate-reorg.md](./balance-history-regtest-restart-same-block-aggregate-reorg.md)

跨组件与总设计文档仍保留在 `doc/` 根目录，例如：

- [../btc-reorg风险现状与改造计划.md](../btc-reorg%E9%A3%8E%E9%99%A9%E7%8E%B0%E7%8A%B6%E4%B8%8E%E6%94%B9%E9%80%A0%E8%AE%A1%E5%88%92.md)
- [../usdb-双链共识接入问题风险与改造清单.md](../usdb-%E5%8F%8C%E9%93%BE%E5%85%B1%E8%AF%86%E6%8E%A5%E5%85%A5%E9%97%AE%E9%A2%98%E9%A3%8E%E9%99%A9%E4%B8%8E%E6%94%B9%E9%80%A0%E6%B8%85%E5%8D%95.md)
