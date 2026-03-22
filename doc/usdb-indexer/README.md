# USDB-Indexer Docs

本目录收拢 `usdb-indexer` 相关文档，尤其是后续持续扩展的 regtest / world-sim 文档。

## 接口与模型

- [usdb-indexer-rpc-v1.md](./usdb-indexer-rpc-v1.md)
- [usdb-indexer-sync-status-model.md](./usdb-indexer-sync-status-model.md)
- [usdb-indexer-readiness-design.md](./usdb-indexer-readiness-design.md)

## Regtest 框架与规划

- [usdb-indexer-regtest-framework.md](./usdb-indexer-regtest-framework.md)
- [usdb-indexer-next-stage-combined-test-plan.md](./usdb-indexer-next-stage-combined-test-plan.md)
- [usdb-indexer-regtest-reorg-plan.md](./usdb-indexer-regtest-reorg-plan.md)
- [usdb-indexer-regtest-topology.md](./usdb-indexer-regtest-topology.md)
- [usdb-indexer-regtest-e2e-smoke.md](./usdb-indexer-regtest-e2e-smoke.md)

## Reorg 与恢复专项

- [usdb-indexer-regtest-reorg-smoke.md](./usdb-indexer-regtest-reorg-smoke.md)
- [usdb-indexer-regtest-same-height-reorg-smoke.md](./usdb-indexer-regtest-same-height-reorg-smoke.md)
- [usdb-indexer-regtest-restart-reorg-smoke.md](./usdb-indexer-regtest-restart-reorg-smoke.md)
- [usdb-indexer-regtest-restart-same-height-reorg.md](./usdb-indexer-regtest-restart-same-height-reorg.md)
- [usdb-indexer-regtest-restart-multi-reorg-smoke.md](./usdb-indexer-regtest-restart-multi-reorg-smoke.md)
- [usdb-indexer-regtest-restart-hybrid-reorg-smoke.md](./usdb-indexer-regtest-restart-hybrid-reorg-smoke.md)
- [usdb-indexer-regtest-pending-recovery-energy-failure.md](./usdb-indexer-regtest-pending-recovery-energy-failure.md)
- [usdb-indexer-regtest-pending-recovery-transfer-reload-restart.md](./usdb-indexer-regtest-pending-recovery-transfer-reload-restart.md)

## Live Ord 场景

- [usdb-indexer-regtest-live-ord-reorg-transfer-remint.md](./usdb-indexer-regtest-live-ord-reorg-transfer-remint.md)
- [usdb-indexer-regtest-live-ord-same-height-reorg-transfer-remint.md](./usdb-indexer-regtest-live-ord-same-height-reorg-transfer-remint.md)
- [usdb-indexer-regtest-live-ord-multi-block-reorg.md](./usdb-indexer-regtest-live-ord-multi-block-reorg.md)

## World-Sim

- [usdb-indexer-regtest-world-sim.md](./usdb-indexer-regtest-world-sim.md)
- [usdb-indexer-regtest-world-sim-validator-sampled.md](./usdb-indexer-regtest-world-sim-validator-sampled.md)
- [usdb-indexer-regtest-world-sim-validator-sampled-reorg.md](./usdb-indexer-regtest-world-sim-validator-sampled-reorg.md)
- [usdb-indexer-regtest-world-sim-reorg.md](./usdb-indexer-regtest-world-sim-reorg.md)
- [usdb-indexer-regtest-world-sim-reorg-determinism.md](./usdb-indexer-regtest-world-sim-reorg-determinism.md)
- [usdb-indexer-regtest-world-sim-live-reorg.md](./usdb-indexer-regtest-world-sim-live-reorg.md)

跨组件与总设计文档仍保留在 `doc/` 根目录，例如：

- [../btc-consensus-rpc-error-contract-design.md](../btc-consensus-rpc-error-contract-design.md)
- [../btc-reorg风险现状与改造计划.md](../btc-reorg%E9%A3%8E%E9%99%A9%E7%8E%B0%E7%8A%B6%E4%B8%8E%E6%94%B9%E9%80%A0%E8%AE%A1%E5%88%92.md)
- [../usdb-双链共识接入问题风险与改造清单.md](../usdb-%E5%8F%8C%E9%93%BE%E5%85%B1%E8%AF%86%E6%8E%A5%E5%85%A5%E9%97%AE%E9%A2%98%E9%A3%8E%E9%99%A9%E4%B8%8E%E6%94%B9%E9%80%A0%E6%B8%85%E5%8D%95.md)
