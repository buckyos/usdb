# BTC Reorg 风险现状与改造计划

## 1. 目的

本文档用于沉淀当前对 BTC reorg 风险的专项分析结论，并作为后续改造工作的统一工作底稿。

本文重点回答以下问题：

- `balance-history` 当前是否具备 reorg 检测与按高度回滚能力。
- `usdb-indexer` 当前是否具备 reorg 检测与按高度回滚能力。
- 当前“落后 tip N 个块”方案的真实边界是什么。
- 后续应该按什么顺序改造，才能把风险从“概率缓释”推进到“确定性处理”。

本文不是最终设计定稿。后续如果形成明确实现方案、数据结构决议、测试验收结论，应继续在本文上追加。

## 2. 背景

当前两个 BTC 侧核心服务都存在重组风险暴露：

1. `balance-history` 负责从 BTC 主链构建余额历史、UTXO 状态和 stable snapshot。
2. `usdb-indexer` 基于 BTC 区块和 `balance-history` 提供的 stable state 继续派生 pass、活跃余额快照、energy 等状态。

目前系统主要通过“落后最新高度 N 个块”来降低短 reorg 影响。

这个方案有意义，但只能降低概率，不能提供正确性保证：

- `N` 不能过大，否则同步延迟和系统效率无法接受。
- 同一高度在 reorg 时可能对应不同 block hash。
- 只用高度而不检查 canonical chain，一旦先前已提交区块后续变成非主链块，系统就需要显式检测并回滚本地状态。

因此，后续目标不能只是继续调 `N`，而必须补齐两个能力：

- reorg 检测
- rollback to height

## 3. 结论先行

### 3.1 总结论

当前系统对 BTC reorg 的处理还停留在“风险缓释”，没有进入“确定性纠错”阶段。

更具体地说：

1. `balance-history` 当前同时缺少完整的 reorg 检测和完整的按高度回滚能力。
2. `usdb-indexer` 已经具备部分按高度回滚的底层积木，但缺少完整的 reorg 检测与回滚编排。
3. 如果要建立稳定的上下游边界，应优先让 `balance-history` 成为上游 reorg barrier，再让 `usdb-indexer` 跟随上游 stable anchor 做本地回滚。

### 3.2 关键判断

- “落后 tip N 个块”不是 reorg 处理机制，只是触发概率缓释手段。
- “单块同步失败回滚”不等于“已提交区块在链重组后回滚”。
- 真正的 reorg 能力必须包含：
  - 检测链分叉
  - 找到共同祖先高度
  - 将数据库状态回滚到指定高度
  - 清理 target height 之后的残留数据
  - 从共同祖先后的 canonical chain 重新继续同步

## 4. 当前实现评估

## 4.1 balance-history

### 4.1.1 当前主循环仍直接追最新高度

当前 `balance-history` 主循环会直接读取 BTC 节点最新高度，并将其作为同步上界。

相关代码：

- [src/btc/balance-history/src/index/indexer.rs](../src/btc/balance-history/src/index/indexer.rs#L149)
- [src/btc/balance-history/src/index/indexer.rs](../src/btc/balance-history/src/index/indexer.rs#L378)

当前没有看到以下机制：

- 基于 block hash 的 canonical chain 连续性检查
- 在每次同步前核验“本地已提交 tip 是否仍是主链 tip 的祖先”
- 将“stable frontier”和“最新可见高度”彻底拆开

这意味着当前更接近“尽快写入最新块”，而不是“只提交已验证稳定的 canonical 块”。

补充当前实现进展：

- `balance-history` 现在已经在 `sync_once()` 入口增加本地 tip 与 canonical chain 的对齐检查。
- 但工程上不能只依赖“下一次进入 sync_once”这个触发点；如果只在检测到 `latest_height > last_height` 时才唤醒主循环，那么同高度 reorg 或 tip 暂时回退都可能被长期压住。
- 因此当前实现已进一步收敛为：等待新块阶段也要在高度变化或 watched tip hash 变化时唤醒同步循环，再进入统一的 reorg reconcile 路径。

这条约束非常关键，因为它决定系统能否及时发现：

1. 高度不变但 block hash 已变化的同高度 reorg。
2. canonical tip 暂时低于本地已同步高度的深 reorg。
3. tip 先回退、后恢复到原高度但 hash 已变化的链切换。

### 4.1.2 本地 blk 文件恢复不是当前 reorg 主要问题

`local_loader` 的恢复逻辑已经具备一定的连续性校验和 tip anchor 校验能力，但它处理的是“本地 blk 索引可否复用”的问题，不是 live block index 的回滚问题。

相关代码：

- [src/btc/balance-history/src/btc/local_loader.rs](../src/btc/balance-history/src/btc/local_loader.rs)

因此，后续处理 reorg 时，不应把主要精力放在 `local_loader` 上，而应放在实时 block-based indexing 的持久化状态回滚上。

### 4.1.3 原子写入只解决单批次崩溃一致性，不解决 reorg 回滚

当前 `balance-history` 已经把单批次 block flush 收敛到一个 RocksDB write batch 中。

相关代码：

- [src/btc/balance-history/src/db/db.rs](../src/btc/balance-history/src/db/db.rs#L645)

这个改动很重要，但它解决的是：

- 一个 block batch 内部写到一半进程崩溃，是否会留下半提交状态。

它没有解决的是：

- 某个区块已经成功提交到 DB，但后来链发生 reorg，这个区块及其后续状态如何按高度撤销。

因此不能把“atomic block flush”误认为已经具备 reorg rollback 能力。

### 4.1.4 当前最核心缺口是 UTXO 与历史状态缺少按块撤销信息

当前 `update_block_state_async` 的语义是：

- 为本批次新增 UTXO 直接写入当前态。
- 为本批次已花费 UTXO 直接删除当前态。
- 为本批次地址余额历史写入按高度记录。
- 为本批次 block commit 写入按高度记录。
- 更新 stable BTC height。

相关代码：

- [src/btc/balance-history/src/db/db.rs](../src/btc/balance-history/src/db/db.rs#L645)

这里最大的问题是：

- 新增 UTXO 的删除在回滚时可以做。
- 但“已花费 UTXO 被删除前的完整值”当前没有看到按块持久化的 undo 记录。

这意味着一旦某个已提交块后续被 reorg 掉，系统并没有足够信息把被该块花掉的旧 UTXO 恢复回来。

这不是小缺口，而是 `balance-history` 无法安全支持目标高度回滚的根因之一。

补充当前实现进展：

- `balance-history` 已经补上按 block 的 undo journal、回滚执行入口和崩溃恢复元数据。
- 当前 undo journal 不是对全历史启用，而是只在接近 canonical tip 的热窗口内生成。
- 对应的 undo 清理也已经收紧为低频、按需执行，不再在历史追块阶段反复扫描 RocksDB。

因此，这一节的风险判断现在应理解为“历史上存在的根因已被识别并进入实现修复”，而不是“当前代码仍完全没有 undo 方案”。

### 4.1.5 clear_blocks 不能替代逻辑状态回滚

当前 `clear_blocks()` 只处理 block 索引相关元数据，不会把 live balance history、UTXO 当前态、stable snapshot 语义一起退回到某个目标高度。

相关代码：

- [src/btc/balance-history/src/db/db.rs](../src/btc/balance-history/src/db/db.rs#L1939)

因此，检测到 reorg 后如果只清块元数据，仍然会留下更深层的数据残留问题：

- future UTXO 残留
- 已删除旧 UTXO 无法恢复
- future balance history entry 残留
- future block commit 残留
- stable height 与底层内容脱钩

### 4.1.6 当前判断

当前 `balance-history` 还不具备安全的“rollback to target height”能力。

如果不新增回滚原语，任何自动 reorg 处理都只能退化为：

- 清空数据库重建
- 或回退到最近完整快照再重放

这两种方式都能作为临时兜底，但不能视为最终方案。

补充当前状态修正：

- undo journal、rollback to height、rollback crash recovery 和 tip 附近热窗口策略已经进入实现。
- reorg 检测也不再只依赖“有更高新块到来”，而是已经开始覆盖同高度 hash 变化与 canonical tip 回退场景。

因此，这里的剩余风险更准确地说是“仍需继续补完集成测试与下游协同”，而不是“上游完全没有自动 reorg 处理框架”。

## 4.2 usdb-indexer

### 4.2.1 当前上游依赖已经收敛到 balance-history stable snapshot

`usdb-indexer` 当前以 `balance-history` 暴露的 stable height 和 snapshot info 作为唯一上游同步参考。

相关代码：

- [src/btc/usdb-indexer/src/index/indexer.rs](../src/btc/usdb-indexer/src/index/indexer.rs#L300)
- [src/btc/usdb-indexer/src/index/indexer.rs](../src/btc/usdb-indexer/src/index/indexer.rs#L308)
- [src/btc/usdb-indexer/src/index/indexer.rs](../src/btc/usdb-indexer/src/index/indexer.rs#L377)

这说明它本身不应该再独立定义一套 BTC canonical 视图，而应跟随上游 stable anchor。

### 4.2.2 当前已经有较强的“单块失败恢复”能力

`usdb-indexer` 已具备以下机制：

- SQLite savepoint，用于保证每个 block 的 pass 侧提交原子性。
- transfer staged state rollback，用于失败时丢弃未提交内存态。
- energy pending marker 和按高度截断，用于恢复未完成或越界的 energy 写入。

相关代码：

- [src/btc/usdb-indexer/src/storage/pass.rs](../src/btc/usdb-indexer/src/storage/pass.rs#L390)
- [src/btc/usdb-indexer/src/index/transfer.rs](../src/btc/usdb-indexer/src/index/transfer.rs#L456)
- [src/btc/usdb-indexer/src/index/transfer.rs](../src/btc/usdb-indexer/src/index/transfer.rs#L477)
- [src/btc/usdb-indexer/src/index/energy.rs](../src/btc/usdb-indexer/src/index/energy.rs#L190)
- [src/btc/usdb-indexer/src/index/energy.rs](../src/btc/usdb-indexer/src/index/energy.rs#L214)
- [src/btc/usdb-indexer/src/index/energy.rs](../src/btc/usdb-indexer/src/index/energy.rs#L225)

这些能力说明 `usdb-indexer` 已经不是完全没有 rollback 基础。

### 4.2.3 但当前还没有完整的“已提交块 reorg 回滚编排”

当前已有能力主要覆盖的是：

- 本次 block 同步失败
- crash recovery
- 启动时清理明显的 future data

例如当前有显式防线要求“目标高度之后不应存在未来数据”。

相关代码：

- [src/btc/usdb-indexer/src/storage/pass.rs](../src/btc/usdb-indexer/src/storage/pass.rs#L740)

但这只是一个 guard，不是 rollback 实现本身。

也就是说：

- 现在系统可以较早发现“数据库已经飘了”。
- 但还不能在发现后自动把 pass、snapshot、energy、上游 anchor 一起退回到共同祖先高度。

### 4.2.4 当前数据模型比 balance-history 更接近可回滚

从现有代码看，`usdb-indexer` 已经具备若干适合 rollback to height 的基础：

- pass history 本来就是按 block height 记录的。
- active balance snapshot 已有按高度清理接口。
- synced BTC height 可单独重写。
- energy 已支持与 pass synced height 对齐和按高度截断。

相关代码：

- [src/btc/usdb-indexer/src/storage/pass.rs](../src/btc/usdb-indexer/src/storage/pass.rs#L409)
- [src/btc/usdb-indexer/src/storage/pass.rs](../src/btc/usdb-indexer/src/storage/pass.rs#L1145)
- [src/btc/usdb-indexer/src/index/energy.rs](../src/btc/usdb-indexer/src/index/energy.rs#L225)

因此，相比 `balance-history`，`usdb-indexer` 的核心问题更像是：

- 缺少一套完整 rollback contract
- 缺少围绕上游 stable anchor 变化的 reorg 检测和回滚编排

而不是底层完全无从下手。

### 4.2.5 adopted upstream snapshot anchor 也必须进入 rollback 范围

`usdb-indexer` 会把上游 `balance-history` 的 stable snapshot anchor 持久化到本地 state 中。

相关代码：

- [src/btc/usdb-indexer/src/index/indexer.rs](../src/btc/usdb-indexer/src/index/indexer.rs#L316)
- [src/btc/usdb-indexer/src/storage/pass.rs](../src/btc/usdb-indexer/src/storage/pass.rs#L475)

这会带来一个很重要的约束：

- 本地 pass/energy 回滚时，不能忘记一起回滚 adopted upstream snapshot anchor。

否则会出现一种伪一致：

- 本地 synced height 已退回
- 本地上游 snapshot height / block hash / block commit 却仍停留在未来高度

这种状态对后续恢复和诊断都会非常危险。

### 4.2.6 当前判断

当前 `usdb-indexer` 不具备完整的自动 reorg 处理能力，但已经拥有一部分关键 rollback 积木。

如果上游 `balance-history` 先变成真正稳定的 reorg barrier，`usdb-indexer` 后续补齐本地 rollback contract 的难度会明显小于 `balance-history`。

## 5. 风险分级

### 5.1 P0 风险

#### P0-1. balance-history 已提交块后续变成非 canonical，但系统无法按高度回滚 UTXO 当前态

直接风险：

- stable snapshot 对应的底层 UTXO 状态错误
- 后续余额计算沿着错误状态继续传播
- 下游即使只消费 stable height，也是在消费错误上游

#### P0-2. usdb-indexer 只发现 future data，但没有标准 rollback contract

直接风险：

- 遇到 reorg 后只能人工清库或全量重建
- 回滚动作容易不完整，留下 snapshot / energy / state_text 残留

#### P0-3. adopted upstream snapshot anchor 未纳入回滚协议

直接风险：

- 本地 durable height 与 adopted upstream identity 脱钩
- 后续同步恢复基于错误锚点继续进行

### 5.2 P1 风险

#### P1-1. stable window 仍然只靠配置值 N，而不是 canonical anchor

直接风险：

- N 调小后风险迅速上升
- N 调大后系统时延恶化
- 运维只能在“性能”和“概率安全”之间被动折中

#### P1-2. reorg 检测、回滚、恢复过程缺少统一日志与观测指标

直接风险：

- 触发原因难以追溯
- 无法快速确认回滚到哪个高度、为何回滚、回滚后是否恢复成功

## 6. 改造目标

后续所有改造应围绕以下目标推进：

1. 任何 stable state 都必须能映射到唯一的 BTC snapshot identity。
2. 任何已提交块一旦被确认不再 canonical，都必须可以回滚到共同祖先高度。
3. 回滚必须清理所有 target height 之后的 future data，而不是只更新一个高度值。
4. 下游服务只应跟随上游 stable anchor，而不应各自定义模糊的“看起来稳定”高度。
5. 检测、回滚、重放、恢复必须具备清晰日志与可测试性。

## 7. 分阶段计划

### 7.1 第一阶段：先把约束和检测面收紧

目标：

- 明确 stable frontier 的定义。
- 明确 reorg 检测触发点。
- 为后续回滚实现建立统一约束。

建议动作：

1. 明确 `balance-history` 的 stable height 是否继续等于当前同步高度，还是改为 tip-confirmation window 后的高度。
2. 在 `balance-history` 同步路径中加入 canonical chain 检测日志，至少记录：
   - local tip height
   - local tip block hash
   - rpc canonical hash
   - 是否发生 mismatch
3. 在 `usdb-indexer` 中明确上游 reorg 检测条件：
   - 上游 stable height 小于本地 adopted snapshot height
   - 或 stable height 相同但 stable block hash 不同
   - 或 stable height 相同但 latest block commit 不同

交付标准：

- 可以清楚观测“是否发生上游 anchor 漂移”。
- 可以清楚界定触发 rollback 的条件。

### 7.2 第二阶段：先让 balance-history 具备真正 rollback 原语

目标：

- 让 `balance-history` 能安全回滚到目标高度。

详细数据模型设计见：

- [doc/balance-history-rollback数据模型设计.md](./balance-history-rollback%E6%95%B0%E6%8D%AE%E6%A8%A1%E5%9E%8B%E8%AE%BE%E8%AE%A1.md)

建议动作：

1. 为每个已提交 block 增加 undo 数据或等价的可恢复信息，至少覆盖：
   - 本块新增的 UTXO
   - 本块花费的 UTXO 及其被花费前的完整值
   - 本块写入的 balance history entry
   - 本块写入的 block commit
2. 新增显式 API，例如：
   - `rollback_to_block_height(target_height)`
3. 回滚协议中要求：
   - 删除 target 之后新增的 UTXO
   - 恢复 target 之后被花费的旧 UTXO
   - 删除 target 之后的 balance history
   - 删除 target 之后的 block commit
   - 更新 stable BTC height
4. 如果完整 undo 方案短期改造量过大，可先实现“回退到最近快照后重放”的临时方案，作为阶段性兜底。

交付标准：

- `balance-history` 能在测试中回滚到指定高度，并恢复正确的 UTXO 和 balance history 结果。

### 7.3 第三阶段：为 usdb-indexer 建立完整 rollback contract

目标：

- 让 `usdb-indexer` 在上游 stable anchor 回退时，可以同步回退本地状态。

建议动作：

1. 新增显式 rollback API，例如：
   - `rollback_to_block_height(target_height, target_anchor)`
2. 回滚协议至少覆盖：
   - 删除 target 之后的 `miner_pass_state_history`
   - 删除 target 之后 mint 的 `miner_passes`
   - 清理 target 之后的 `active_balance_snapshots`
   - 将 energy 对齐到 target height
   - 重写 synced BTC height
   - 重写 adopted `balance-history` snapshot anchor
3. 对于 target 之前已存在、但 target 之后发生过状态迁移的 pass，需要明确：
   - 是通过 history 重建当前态
   - 还是显式维护 reverse mutation

交付标准：

- `usdb-indexer` 可以基于上游 anchor 漂移自动退回共同祖先高度并继续同步。

### 7.4 第四阶段：补齐端到端 reorg 测试与验收

目标：

- 把 reorg 处理从“设计可讲通”推进到“回归可验证”。

建议动作：

1. 在 regtest 场景中制造短 reorg。
2. 验证 `balance-history`：
   - 检测到分叉
   - 回滚到共同祖先
   - 重放后 stable snapshot 正确
3. 验证 `usdb-indexer`：
   - adopted upstream anchor 变化可见
   - 本地 pass / active snapshot / energy 同步回滚
   - 回滚后可继续同步并产出正确结果

交付标准：

- 形成可重复运行的 reorg e2e smoke / regression 测试。

## 8. 建议的实施顺序

推荐顺序如下：

1. 先明确并固化本文档中的约束与检测条件。
2. 先改 `balance-history`，让它成为真正的上游 reorg barrier。
3. 再改 `usdb-indexer`，让它跟随上游 stable anchor 做本地 rollback。
4. 最后补齐 regtest reorg 回归测试。

不建议的顺序是：

- 先在 `usdb-indexer` 做复杂回滚，而上游 `balance-history` 仍然只是“概率稳定”。

那样会导致下游设计建立在不稳定上游之上，后续还要返工一次。

## 9. 当前开放问题

以下问题需要在后续设计阶段继续决议：

1. `balance-history` 的 stable height 是否继续允许接近 tip，还是正式引入 confirm-depth 语义。
2. `balance-history` 的回滚是优先做 block-level undo，还是优先做 snapshot rollback + replay。
3. `usdb-indexer` 的 pass 当前态是通过 history 重建，还是继续保留冗余当前态表并在回滚时双写维护。
4. adopted upstream snapshot anchor 是否需要形成单独的 versioned identity 结构，避免只靠零散 state key 维护。
5. reorg 触发后是否允许服务继续对外提供旧 stable snapshot，还是必须短暂停止对外服务直到回滚完成。

## 10. 后续维护方式

建议后续围绕本文档持续追加以下内容：

- 已确认的设计决议
- 已实现项及提交记录
- 测试覆盖情况
- 尚未关闭的风险项

这样可以避免 reorg 讨论散落在多个对话和临时记录中，后续工程推进也能围绕同一份工作底稿持续收敛。