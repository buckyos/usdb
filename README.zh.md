# USDB

[English](./README.md) | [中文](./README.zh.md)

USDB 不是一个孤立的“索引查询工程”，而是 **整个 BTC 质押 / 矿工证铭文双链系统的 BTC 侧基础设施仓库**。  
它负责把 Bitcoin 链上的区块、余额历史、铭文状态以及矿工证（pass）相关的派生状态，收敛成一组 **可回放、可审计、可用于 ETHW 挖矿端和验块端消费的外部状态服务**。

从系统角色看，这个仓库要解决的是：**如何把 BTC 侧矿工证系统变成 ETHW 可以稳定依赖的外部共识输入**。  
因此它关心的不只是“能不能查到数据”，而是：

- 高度 `H` 时的 BTC 外部状态到底是什么
- reorg、restart、crash 后这份历史状态还能不能被稳定重放
- ETHW validator 拿到区块里记录的 `(height, state ref, pass info)` 后，能不能在未来继续按历史语义重放校验

换句话说，这个仓库当前承载的是整个系统里最关键的 BTC 基础层：

- `balance-history`：提供地址余额历史、稳定高度、块哈希和块级提交视图
- `usdb-indexer`：把 BTC 数据与 `balance-history` 结果进一步加工成矿工证状态、活跃余额快照、能量、candidate-set 等更贴近挖矿规则的派生状态
- 围绕 reorg、restart、historical replay、validator payload、world-sim 的系统性测试框架

这些能力最终服务的对象，是上层 ETHW 的出块、候选选择和验块逻辑。

项目背景和更完整的双链目标见：
- [USDB 双链共识接入问题、风险与改造清单](./doc/usdb-%E5%8F%8C%E9%93%BE%E5%85%B1%E8%AF%86%E6%8E%A5%E5%85%A5%E9%97%AE%E9%A2%98%E9%A3%8E%E9%99%A9%E4%B8%8E%E6%94%B9%E9%80%A0%E6%B8%85%E5%8D%95.md)
- [BTC Consensus RPC 错误契约设计](./doc/btc-consensus-rpc-error-contract-design.md)

## Why This Exists

如果上层 ETHW 系统要把 BTC 侧矿工证状态当作外部输入，例如：

- 矿工资格
- pass 状态
- 历史能量值
- 候选集合与 winner 选择结果

那么仅有“能查到数据”是不够的，还必须解决：

- reorg 下的稳定锚点
- 历史状态回放
- 跨服务一致的 snapshot / system state identity
- `rpc_alive` 与 `consensus_ready` 的区分
- 统一的错误模型和历史校验语义

这个仓库就是围绕这些问题做工程化收敛，把“矿工证铭文系统”里最容易引发跨链不一致的 BTC 侧基础层先打稳，再把这份状态暴露给 ETHW 使用。

## Repository Layout

```text
.
├── doc/                     # 设计、协议、测试、风险与改造文档
│   ├── balance-history/
│   └── usdb-indexer/
├── src/
│   └── btc/
│       ├── balance-history/       # BTC 地址余额历史服务
│       ├── balance-history-cli/   # 相关 CLI
│       ├── usdb-indexer/          # USDB 派生状态索引服务
│       ├── usdb-indexer-cli/      # 相关 CLI
│       └── usdb-util/             # 共享类型、版本、commit/hash 工具
└── web/
    ├── balance-history-browser/
    └── usdb-indexer-browser/
```

BTC Rust workspace 位于 [src/btc/Cargo.toml](./src/btc/Cargo.toml)，当前成员包括：

- `balance-history`
- `balance-history-cli`
- `usdb-indexer`
- `usdb-indexer-cli`
- `usdb-util`

## Core Components

### `balance-history`

职责：

- 从 `bitcoind` 建立地址余额历史视图
- 提供稳定高度、块哈希、块级提交信息
- 为下游提供历史 `state ref` 与可审计的 snapshot 信息
- 作为整个矿工证派生计算的 BTC 余额事实层

文档入口：
- [Balance-History Docs](./doc/balance-history/README.md)

### `usdb-indexer`

职责：

- 基于 BTC 数据和 `balance-history` 输出构建矿工证 / pass 状态机
- 维护历史 `state ref`、local state commit、system state id
- 提供活跃余额快照、能量、leaderboard、historical replay 等查询能力
- 为 ETHW 挖矿端、candidate-set 选择逻辑、validator / 验块端提供稳定接口

文档入口：
- [USDB-Indexer Docs](./doc/usdb-indexer/README.md)

### Testing / Simulation

这个仓库当前一大特色是测试基础设施比较完整，已经覆盖：

- smoke / e2e
- reorg / same-height reorg / multi-block reorg
- restart / pending recovery / crash consistency
- historical state ref replay
- validator block-body / candidate-set / version mismatch / upgrade path
- world-sim / deterministic reorg / determinism / soak

如果你想从测试体系理解项目，优先看：

- [USDB-Indexer Regtest Framework](./doc/usdb-indexer/usdb-indexer-regtest-framework.md)
- [USDB-Indexer 下一阶段综合测试计划](./doc/usdb-indexer/usdb-indexer-next-stage-combined-test-plan.md)
- [USDB-Indexer Validator Block-Body E2E 设计](./doc/usdb-indexer/usdb-indexer-validator-block-body-e2e-design.md)

## Current State

按当前仓库状态，可以把项目理解为：

- **BTC 侧矿工证基础层已经比较完整**
  - snapshot identity
  - local state commit
  - system state id
  - readiness contract
  - shared consensus RPC errors
  - historical state ref replay
- **面向 ETHW 消费侧的 validator-style replay 已经打通**
  - 单 pass
  - multi-pass / candidate-set
  - version mismatch / upgrade
  - restart / crash / not-ready window
  - world-sim sampled validation
- **真实 prune / retention floor** 仍然是未来演进方向
  - 当前 `STATE_NOT_RETAINED` 仍基于 `genesis_block_height` 的简化模型

## Getting Started

### 1. Read the docs

建议按这个顺序看：

1. [USDB 双链共识接入问题、风险与改造清单](./doc/usdb-%E5%8F%8C%E9%93%BE%E5%85%B1%E8%AF%86%E6%8E%A5%E5%85%A5%E9%97%AE%E9%A2%98%E9%A3%8E%E9%99%A9%E4%B8%8E%E6%94%B9%E9%80%A0%E6%B8%85%E5%8D%95.md)
2. [BTC Consensus RPC 错误契约设计](./doc/btc-consensus-rpc-error-contract-design.md)
3. [USDB-Indexer Docs](./doc/usdb-indexer/README.md)
4. [Balance-History Docs](./doc/balance-history/README.md)

### 2. Run Rust tests

```bash
cargo test --manifest-path src/btc/Cargo.toml -p balance-history
cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer
```

### 3. Run focused regtests

例如 `usdb-indexer` 的专项回归：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/run_reorg_regression.sh
```

### 4. Run world-sim

```bash
bash src/btc/usdb-indexer/scripts/regtest_world_sim.sh
```

或者直接跑更重的 validator candidate-set 长跑入口：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/run_live_validator_candidate_set.sh
```

## Design Themes

贯穿整个仓库的设计主题主要是：

- **历史状态可回放**
  - 回答“高度 H 时，ETHW 应该消费哪一份 BTC 外部状态”
- **服务活着不等于可用于共识**
  - 区分 `rpc_alive` 与 `consensus_ready`
- **错误必须结构化**
  - `HEIGHT_NOT_SYNCED`
  - `SNAPSHOT_NOT_READY`
  - `*_MISMATCH`
  - `HISTORY_NOT_AVAILABLE`
  - `STATE_NOT_RETAINED`
- **reorg 是一等公民**
  - 不是异常路径，而是设计输入
- **测试先行收敛协议语义**
  - 通过 regtest / world-sim 把矿工证状态、历史回放、validator payload 等复杂边界提前打透

## Documentation Index

- 总设计与风险
  - [USDB 双链共识接入问题、风险与改造清单](./doc/usdb-%E5%8F%8C%E9%93%BE%E5%85%B1%E8%AF%86%E6%8E%A5%E5%85%A5%E9%97%AE%E9%A2%98%E9%A3%8E%E9%99%A9%E4%B8%8E%E6%94%B9%E9%80%A0%E6%B8%85%E5%8D%95.md)
  - [BTC Consensus RPC 错误契约设计](./doc/btc-consensus-rpc-error-contract-design.md)
- 子系统索引
  - [USDB-Indexer Docs](./doc/usdb-indexer/README.md)
  - [Balance-History Docs](./doc/balance-history/README.md)
- 关键测试与规划
  - [USDB-Indexer Regtest Framework](./doc/usdb-indexer/usdb-indexer-regtest-framework.md)
  - [USDB-Indexer 下一阶段综合测试计划](./doc/usdb-indexer/usdb-indexer-next-stage-combined-test-plan.md)
  - [USDB-Indexer Validator Block-Body E2E 设计](./doc/usdb-indexer/usdb-indexer-validator-block-body-e2e-design.md)

## Status

项目仍在活跃开发中。当前更适合作为：

- 架构与协议验证仓库
- BTC 质押矿工证系统的 BTC 侧基础设施层
- BTC 外部状态共识化的工程实验场
- 高强度 regtest / world-sim 回归基础设施

而不是一个已经冻结接口、承诺长期兼容的最终产品发布仓库。
