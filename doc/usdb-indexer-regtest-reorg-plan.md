# USDB-Indexer Regtest Reorg 专项测试计划

本文档给 `usdb-indexer` 新增的 upstream reorg rollback / recovery 设计一套专项回归计划，目标是把这次改动从“单测已覆盖”推进到“真实 regtest 链路可重复验证”。

这份计划参考 `balance-history/scripts` 当前的组织方式：

1. 共享能力下沉到 shell 级共享库。
2. 每个场景保留独立入口脚本。
3. 每个场景配一份短文档，明确目标、运行方式和验收标准。

## 1. 目标

这批专项测试需要覆盖的不只是“发现 reorg 以后高度退回”，还要覆盖这次改动引入的完整 contract：

1. 上游 stable anchor 漂移能被检测到。
2. `usdb-indexer` 能找到共同祖先并回滚 durable pass state。
3. adopted upstream snapshot anchor 会一起回滚，不留下 future anchor。
4. `active_balance_snapshots`、`pass_block_commits`、current pass state 不会残留 future data。
5. `energy` 能对齐到 pass synced height。
6. `transfer tracker` 内存态在 rollback 后会重载，而不是继续沿用旧链缓存。
7. rollback 完成后服务还能继续正常追新块。
8. 如果 recovery 中途失败，pending marker 能保证下次重试或重启后继续完成恢复。

## 2. 分层策略

不建议把所有复杂度都塞进一条大脚本里。建议保留三层：

### 2.1 单元测试层

这一层已经有基础：

1. `height regression`
2. `same-height reorg`
3. `pending recovery after energy failure`
4. `pending recovery after restart`

这一层继续承担：

1. 回滚协议的精确状态机验证。
2. fault injection。
3. SQLite durable state 细粒度断言。

### 2.2 shell regtest smoke 层

这一层参考 `balance-history` 的脚本组织方式，覆盖：

1. 真实 `bitcoind + balance-history + usdb-indexer` 生命周期。
2. upstream reorg 触发。
3. RPC 观测面与日志验收。

这一层优先做无 inscription 或少量 inscription 的 deterministic 场景。

### 2.3 live ord regtest 层

这一层复用 `regtest_live_ord_e2e.sh` 的能力，覆盖：

1. 真实 mint / transfer / remint(prev) 业务形态。
2. rollback 后 pass state / energy / active snapshot 的联合正确性。
3. 同块内复杂业务顺序与 reorg 叠加场景。

## 3. 脚手架建议

`usdb-indexer` 当前已有：

1. `regtest_e2e_smoke.sh`
2. `regtest_live_ord_e2e.sh`
3. `regtest_world_sim.sh`
4. `regtest_scenario_runner.py`

但还缺少像 `balance-history/scripts/regtest_lib.sh` 这样的 reorg 共享库。建议先补一个：

- `src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh`

第一版共享库建议只做三件事：

1. 复用现有服务启动/停止逻辑，统一 `bitcoind` / `balance-history` / `usdb-indexer` 生命周期。
2. 封装 reorg 原语：
   - `invalidateblock`
   - `reconsiderblock`（如果需要）
   - `mine replacement block`
   - `stop/restart balance-history`
   - `stop/restart usdb-indexer`
3. 封装常用断言：
   - 等待 `balance-history` 收敛到目标 `block hash / block commit`
   - 等待 `usdb-indexer` 收敛到目标 `synced height / snapshot info`
   - exact-height 断言某高度的 `pass commit / active snapshot / pass energy` 是否存在

原则上：

1. 服务生命周期与 reorg 编排放在 shell。
2. 复杂 RPC 断言继续复用 `regtest_scenario_runner.py` 或临时 JSON 场景文件。
3. 需要直接观测 pending marker 时，可以接受用 `sqlite3` 读本地 DB，因为当前 RPC 没暴露这个内部状态。

## 4. 用例矩阵

### 4.1 P0 必做

#### Case 1. 空业务面高度回退 reorg

- 建议脚本：`regtest_reorg_smoke.sh`
- 类型：shell regtest
- 目的：
  - 验证上游 stable height 回退时，`usdb-indexer` 会一起退回共同祖先高度。
- 关键步骤：
  - 先跑到高度 `H`
  - 让 `balance-history` / `usdb-indexer` 都追到 `H`
  - 使 `H` 失效并挖出 replacement chain，令新的 stable height 回到 `< H` 或共同祖先高度
- 关键断言：
  - `get_sync_status.synced_block_height` 回退
  - `get_snapshot_info` 中 adopted anchor 切换到 replacement chain
  - `H` 之后的 `pass_block_commit` / `active_balance_snapshot` 不再可见
  - rollback 后继续挖一个新区块，`usdb-indexer` 能继续同步

#### Case 2. 空业务面同高度 reorg

- 建议脚本：`regtest_same_height_reorg_smoke.sh`
- 类型：shell regtest
- 目的：
  - 验证 stable height 不变，但 `stable_block_hash / latest_block_commit` 变化时，`usdb-indexer` 不会漏检。
- 关键步骤：
  - 在高度 `H` 形成旧 tip
  - 离线 invalidate `H`
  - 重新挖一个 replacement block，保持高度仍为 `H`
- 关键断言：
  - `get_sync_status.synced_block_height` 仍为 `H`
  - `get_snapshot_info.snapshot_id / stable_block_hash / latest_block_commit` 发生变化
  - replacement height 上 future 残留被清干净

#### Case 3. live ord 单块 transfer/remint reorg

- 建议脚本：`regtest_live_ord_reorg_transfer_remint.sh`
- 类型：live ord regtest
- 目的：
  - 验证有真实 pass 状态迁移时，reorg 后 pass / energy / active snapshot 一起回滚。
- 关键步骤：
  - 复用 `transfer_remint` 基线：`mint -> transfer -> remint(prev)`
  - 让最后一个业务块成为被 reorg 的块
  - replacement block 不包含旧业务交易
- 关键断言：
  - 被 replacement 掉的新 pass 不再存在或不再 active
  - prev pass 状态恢复到 replacement 链对应状态
  - `get_pass_energy` 不保留旧链 remint 带来的能量结果
  - `get_active_balance_snapshot` 与 replacement 链一致

#### Case 4. live ord 同高度 transfer/remint reorg

- 建议脚本：`regtest_live_ord_same_height_reorg_transfer_remint.sh`
- 类型：live ord regtest
- 目的：
  - 专测“高度不变、业务块替换”的最危险场景。
- 关键步骤：
  - 旧 `H` 含 transfer 或 remint
  - replacement `H` 为空块或包含不同业务组合
- 关键断言：
  - 被撤销交易对应的 pass state / energy / snapshot 在 exact-height 查询中消失
  - `get_snapshot_info.latest_block_commit` 切到 replacement commit
  - replacement 后继续追 `H+1` 能正常工作

### 4.2 P1 高价值

#### Case 5. restart 后高度回退 reorg

- 建议脚本：`regtest_restart_reorg_smoke.sh`
- 类型：shell regtest
- 目的：
  - 验证服务离线期间改链，重启后仍能完成 rollback + replay。
- 关键步骤：
  - 停止 `balance-history` 与 `usdb-indexer`
  - 离线改链
  - 先启动 `balance-history`，再启动 `usdb-indexer`
- 关键断言：
  - `usdb-indexer` 重启后直接对齐新 anchor，而不是卡在旧 anchor
  - 日志中有明显的 `upstream reorg detected / pending recovery resumed` 轨迹

#### Case 6. restart 后同高度 reorg

- 建议脚本：`regtest_restart_same_height_reorg.sh`
- 类型：shell regtest
- 目的：
  - 验证“服务离线 + same-height replacement”不会因为高度没变而漏掉 recovery。

#### Case 7. 多块 rollback 业务混合场景

- 建议脚本：`regtest_live_ord_multi_block_reorg.sh`
- 类型：live ord regtest
- 建议业务形态：
  - `mint`
  - `passive_transfer`
  - `same_owner_multi_mint`
  - `duplicate_prev_inherit`
- 目标：
  - 不是做全笛卡尔积，而是挑一个两到三块的组合链，验证多块 rollback 时当前态重建正确。
- 关键断言：
  - current pass 表与 replacement 链重新计算结果一致
  - consumed / dormant / active 状态没有串链
  - leaderboard 与单 pass energy 查询一致

### 4.3 P2 fault injection

这组场景需要先加少量 test-only 注入开关，建议通过 env 或 config debug 字段实现，不建议直接靠随机故障。

#### Case 8. pending recovery: energy rollback 失败后重试

- 建议脚本：`regtest_pending_recovery_energy_failure.sh`
- 需要能力：
  - 让 `pass` rollback 成功后，第一次 `energy` recovery 故意失败
- 关键断言：
  - DB 中 pending marker 存在
  - 下一次重试不依赖再次检测 drift，也能继续完成 recovery
  - marker 最终清除

#### Case 9. pending recovery: transfer reload 失败并跨重启恢复

- 建议脚本：`regtest_pending_recovery_transfer_reload_restart.sh`
- 需要能力：
  - 第一次 `reload_from_storage()` 注入失败
- 关键断言：
  - pending marker 在进程退出前仍然存在
  - 重启后优先进入 `resume_pending_upstream_reorg_recovery()`
  - 恢复成功后 marker 清除

## 5. 统一验收清单

每个 reorg 场景至少要从下面清单中选出对应断言，不要只验“脚本没报错”：

1. `balance-history` stable hash / latest commit 已切到 replacement chain。
2. `usdb-indexer get_sync_status.synced_block_height` 与 replacement 链一致。
3. `get_snapshot_info` 里的 adopted upstream anchor 已切换。
4. replacement height 上不应该存在旧链残留的 `pass snapshot / pass commit / energy record / active balance snapshot`。
5. rollback 完成后继续追一块，服务还能正常同步。
6. 如果场景有 pass，至少同时断言：
   - `get_pass_snapshot`
   - `get_pass_energy`
   - `get_active_balance_snapshot`
7. 如果场景有 pending recovery，必须断言 marker 生命周期：
   - 写入
   - 恢复前仍存在
   - 恢复成功后清除

## 6. 推荐实现顺序

建议按下面顺序推进，而不是一次性把所有脚本铺满：

1. 先抽 `regtest_reorg_lib.sh`
2. 先做 `Case 1 / Case 2`
3. 再做 `Case 3 / Case 4`
4. 再做 `Case 5 / Case 6`
5. 再做 `Case 7`
6. 最后补 `Case 8 / Case 9` 这类 fault injection

原因：

1. 先把“空业务面 anchor rollback”打通，能验证基础编排和观测面。
2. 再把真实 pass 业务场景叠上去，避免一开始就把问题混在 ord 交易构造里。
3. fault injection 依赖额外测试钩子，应该放在最后一层。

## 7. 建议新增文件清单

第一批建议：

1. `src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh`
2. `src/btc/usdb-indexer/scripts/regtest_reorg_smoke.sh`
3. `src/btc/usdb-indexer/scripts/regtest_same_height_reorg_smoke.sh`
4. `src/btc/usdb-indexer/scripts/regtest_live_ord_reorg_transfer_remint.sh`
5. `src/btc/usdb-indexer/scripts/regtest_live_ord_same_height_reorg_transfer_remint.sh`
6. `doc/usdb-indexer-regtest-reorg-smoke.md`
7. `doc/usdb-indexer-regtest-same-height-reorg-smoke.md`
8. `doc/usdb-indexer-regtest-live-ord-reorg-transfer-remint.md`
9. `doc/usdb-indexer-regtest-live-ord-same-height-reorg-transfer-remint.md`

第二批再补：

1. restart 类场景
2. `src/btc/usdb-indexer/scripts/regtest_live_ord_multi_block_reorg.sh`
3. `doc/usdb-indexer-regtest-live-ord-multi-block-reorg.md`
4. `src/btc/usdb-indexer/scripts/regtest_pending_recovery_energy_failure.sh`
5. `src/btc/usdb-indexer/scripts/regtest_pending_recovery_transfer_reload_restart.sh`
6. `doc/usdb-indexer-regtest-pending-recovery-energy-failure.md`
7. `doc/usdb-indexer-regtest-pending-recovery-transfer-reload-restart.md`

## 8. 当前决策

基于现有代码和脚手架，建议把第一批工作的范围收敛为：

1. 先抽共享 shell 库。
2. 先补 2 条空业务面 reorg smoke。
3. 再补 2 条 live ord reorg 场景。

这样可以最快把这次 rollback / recovery 的主链路覆盖到真实 regtest，而不会一开始就把 fault injection、world-sim、所有业务组合一起卷进来。
