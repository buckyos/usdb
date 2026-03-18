# balance-history rollback 数据模型设计

## 1. 目的

本文档细化 `balance-history` 的 rollback 数据模型设计，目标是为后续实现 `rollback_to_block_height(target_height)` 提供一版可落地的方案。

本文重点关注：

- RocksDB 需要新增哪些列族和元数据。
- 每个已提交 block 需要额外持久化哪些 undo 信息。
- 回滚时如何按高度撤销 UTXO、balance history、block commit 和 stable height。
- 崩溃恢复、快照安装和存量数据迁移如何处理。

本文默认以当前代码实现为基础，不假设重做整个 `balance-history` 存储模型。

## 2. 设计目标

### 2.1 必须满足的目标

1. 检测到 BTC reorg 后，可以把数据库安全回滚到指定高度。
2. 回滚后不能留下 target height 之后的 future data。
3. 回滚必须能恢复 UTXO 当前态，而不只是删除历史记录。
4. 回滚过程必须具备崩溃可恢复性。
5. 新模型应尽量复用当前 `balance_history`、`utxo`、`block_commits` 现有读写方式，避免大规模重构查询链路。

### 2.2 明确不追求的目标

当前阶段不追求：

1. 在一个版本里直接把 `balance-history` 改造成完整的可证明状态树。
2. 在一个版本里支持任意深度、无限历史的零成本回滚。
3. 为每个查询即时生成密码学证明。

当前阶段的核心目标很直接：

- 让已提交块可以被正确撤销。

### 2.3 新增约束

在本轮设计中，再增加两个明确约束：

1. undo journal 只保留有限热窗口，不能无限累积到全历史。
2. rollback 数据收集不能把当前 batch 初始化路径上的大缓存结构整体改造成按 block 分片，否则容易拖慢基于 blk 文件的快速批量加载。
3. undo journal 不应从高度 0 开始对全历史持续构建，而应只在接近当前 canonical BTC tip 的热窗口内启用。
4. prune 检查不应在历史追块阶段反复扫描 RocksDB，只有进入热窗口且 retained 水位需要推进时才应执行实际清理。

## 3. 当前实现约束

## 3.1 现有 forward state

当前 `balance-history` 的核心持久化状态包括：

- `BALANCE_HISTORY_CF`
  - key = `script_hash || block_height_be`
  - value = `delta_i64_be || balance_u64_be`
- `UTXO_CF`
  - key = `outpoint`
  - value = `script_hash || value`
- `BLOCK_COMMITS_CF`
  - key = `block_height_be`
  - value = `btc_block_hash || balance_delta_root || block_commit`
- `META_CF`
  - `btc_block_height`

相关代码：

- [src/btc/balance-history/src/db/db.rs](../../src/btc/balance-history/src/db/db.rs#L17)
- [src/btc/balance-history/src/db/db.rs](../../src/btc/balance-history/src/db/db.rs#L395)
- [src/btc/balance-history/src/db/db.rs](../../src/btc/balance-history/src/db/db.rs#L645)

## 3.2 当前最关键的回滚障碍

当前 block flush 只持久化 forward mutation：

- 新增 UTXO 被写入当前态。
- 被花费 UTXO 被直接删除。
- balance history 以 `script_hash -> height` 的布局写入。
- block commit 按高度写入。

这意味着：

- 删除 target 之后的 block commit 是容易的。
- 删除 target 之后的 balance history entry 从逻辑上是可行的。
- 但恢复“某块花掉的旧 UTXO”当前没有持久化来源。

## 3.3 当前流水线中已经能拿到 spent UTXO 的旧值

虽然 DB 当前没有存 undo journal，但现有 block preload 流水线在处理 `vin` 时，已经会把被引用的旧 UTXO 装入 `PreloadVIn.cache_tx_out`。

相关代码：

- [src/btc/balance-history/src/index/block.rs](../../src/btc/balance-history/src/index/block.rs#L29)
- [src/btc/balance-history/src/index/block.rs](../../src/btc/balance-history/src/index/block.rs#L387)
- [src/btc/balance-history/src/index/block.rs](../../src/btc/balance-history/src/index/block.rs#L462)

这点很关键，它意味着：

- undo 数据不需要在 flush 前再额外回读 RocksDB。
- 当前批处理链路已经具备生成 spent UTXO undo record 的输入数据。

## 3.4 当前 balance history 读取语义决定了 rollback 不必重写旧值

当前按高度查询语义是 at-or-before，而不是 exact-height。

相关代码：

- [src/btc/balance-history/src/db/db.rs](../../src/btc/balance-history/src/db/db.rs#L782)

这带来一个很重要的简化：

- 回滚时，针对 target 之后的 `balance_history` 记录，只要删除这些 exact-height entry，查询自然会回退到更早的记录。
- 不需要在 rollback 时再把“前一条 balance”重写回去。

因此，balance history rollback 的关键不在于保存 inverse balance value，而在于：

- 高效找到“某个高度写了哪些 script_hash 记录”
- 删除这些 exact key

## 4. 设计概览

推荐采用：

- per-block undo journal
- 每个 block 的 forward mutation 和 undo mutation 一起原子提交
- rollback 按 block 从高到低逐块反演

这里的 per-block undo journal 语义需要明确为：只对热窗口内的 block 持久化 undo，而不是从创世高度到当前 tip 的全历史都保留 undo。

这是当前代码基础上改造成本和正确性最平衡的方案。

## 5. 推荐数据模型

## 5.1 新增列族

建议新增以下 RocksDB CF：

1. `BLOCK_UNDO_META_CF`
2. `BLOCK_UNDO_CREATED_UTXOS_CF`
3. `BLOCK_UNDO_SPENT_UTXOS_CF`
4. `BLOCK_UNDO_BALANCE_INDEX_CF`

说明：

- `BLOCK_UNDO_META_CF` 用于描述某个 block 的 undo bundle 边界和摘要信息。
- `BLOCK_UNDO_CREATED_UTXOS_CF` 用于记录该 block 新创建的 UTXO。
- `BLOCK_UNDO_SPENT_UTXOS_CF` 用于记录该 block 花费掉、后续回滚时需要恢复的旧 UTXO。
- `BLOCK_UNDO_BALANCE_INDEX_CF` 用于记录该 block 写入了哪些 `script_hash@height` 余额历史键，便于回滚时精确删除。

当前版本不建议新增单独的 `BLOCK_UNDO_COMMITS_CF`：

- `BLOCK_COMMITS_CF` 本身就是按高度组织的。
- rollback 当前高度时，直接删除该高度 block commit 即可。

## 5.2 BLOCK_UNDO_META_CF

### key

- `block_height_be`，4 字节

### value

建议布局：

- `format_version_u16`
- `btc_block_hash_32`
- `created_utxo_count_u32`
- `spent_utxo_count_u32`
- `balance_entry_count_u32`

可选扩展字段：

- `balance_delta_root_32`
- `block_commit_32`

当前建议先不把可选扩展字段作为必需项；如果后续想增强审计和一致性校验，可以再加版本化扩展。

### 用途

- 判断某个高度是否已经具备 rollback 能力。
- 给 rollback 扫描提供边界和计数校验。
- 记录该高度对应的 BTC block hash，便于诊断日志和链一致性检查。

## 5.3 BLOCK_UNDO_CREATED_UTXOS_CF

### key

- `block_height_be || seq_u32_be`

### value

- `outpoint`
- `script_hash`
- `value_u64_be`

### 说明

从纯功能上讲，回滚创建的 UTXO 只需要 `outpoint` 就能删除。

但仍建议保存完整值：

- 便于审计和日志输出。
- 便于将来做一致性校验。
- 保持 created / spent 两类 undo record 结构对称。

### 为什么不用 outpoint 直接做 key

因为 rollback 按高度执行，最重要的是：

- 能顺序扫描某个高度的所有 created UTXO。

因此 key 以 `height || seq` 组织更直接。

## 5.4 BLOCK_UNDO_SPENT_UTXOS_CF

### key

- `block_height_be || seq_u32_be`

### value

- `outpoint`
- `script_hash`
- `value_u64_be`

### 用途

这是整个 rollback 模型里最关键的 undo 数据。

回滚 block `H` 时，系统需要把该 block 花掉的旧 UTXO 重新写回 `UTXO_CF`。这要求 rollback journal 在 block 提交时就已经把旧值持久化。

### 数据来源

直接来自 block preload 阶段已经填充到 `PreloadVIn.cache_tx_out` 的旧值。

因此该方案与当前预加载流水线是兼容的。

## 5.5 BLOCK_UNDO_BALANCE_INDEX_CF

### key

- `block_height_be || script_hash`

### value

- 预留 1 字节版本位，当前固定为 `0x01`

### 用途

用于回答一个 rollback 必须回答的问题：

- 某个 block height 写入了哪些 `balance_history` 键？

当前 `BALANCE_HISTORY_CF` 的 key 布局是 `script_hash || block_height`，这对按地址查询很高效，但对“删除某个高度的所有 balance entry”不友好。

因此需要一个按高度组织的反向索引。

### 为什么这里不保存 balance 数值

因为 rollback 不需要重写前一条 balance，只需要删除当前高度的那条 entry。

已有查询语义会自动回落到更早记录。

因此这里只需要记录：

- 这个高度影响了哪些 `script_hash`

## 5.6 META_CF 中新增的回滚状态键

建议在 `META_CF` 中新增以下 key：

- `rollback_in_progress`
- `rollback_target_height`
- `rollback_next_height`

说明：

- `rollback_in_progress` 表示当前 DB 正在执行多块回滚。
- `rollback_target_height` 表示目标回滚高度。
- `rollback_next_height` 表示下次恢复时还需要撤销的最高高度。

这样即使进程在回滚过程中崩溃，重启后也能继续回滚而不是进入半回滚状态。

## 5.7 undo 保留窗口与清理边界

### 5.7.1 为什么不能保留全量 undo

如果 undo journal 对所有历史高度永久保留，会带来两个问题：

1. 存储体积会持续增长，而且其中绝大部分旧高度实际上已经不再需要 rollback。
2. RocksDB 的额外 CF 会承担完全没有必要的长期空间和 compaction 成本。

因此，undo journal 必须被明确设计为“热窗口数据”，而不是完整历史数据。

结合当前实现约束，建议进一步明确：

- `undo_retention_blocks` 同时定义 undo 的在线保留窗口和在线启用窗口。
- 只有 `block_height >= latest_btc_height - undo_retention_blocks + 1` 的区块才需要在 flush 时生成 undo bundle。
- 历史追块阶段如果当前 batch 还没有进入这个热窗口，应直接跳过 undo 构建。

### 5.7.2 建议引入 undo retention window

建议新增配置语义，例如：

- `undo_retention_blocks`

语义：

- 只保留最近 `undo_retention_blocks` 个已提交高度的 undo journal。

若当前 stable height 为 `stable_height`，则理论保留下界为：

- `retained_from_height = stable_height.saturating_sub(undo_retention_blocks - 1)`

但真正执行时还应再与以下边界取最大值：

- `rollback_supported_from_height`
- 最近一次 snapshot 安装后的起始可回滚高度

因此推荐最终使用：

- `effective_retained_from_height = max(retained_from_height, rollback_supported_from_height, snapshot_based_rollback_floor)`

### 5.7.3 超出保留窗口后的语义

对于小于 `effective_retained_from_height` 的高度：

- 不再保证存在 undo journal。
- 若真的需要回滚到该范围，只能退化为：
  - snapshot rollback
  - 或全量 resync

这应作为显式系统语义写入日志和状态说明，而不是隐含假设。

### 5.7.4 建议新增的可观测 meta 键

建议增加：

- `undo_retained_from_height`

语义：

- 当前库中仍然承诺具备 rollback journal 的最小高度。

这个值主要用于：

- 运维观察
- 启动时自检
- 发生深 reorg 时快速判断应该走 rollback 还是直接转 snapshot/resync 兜底

### 5.7.5 清理触发时机

建议不要在每个 block flush 的热路径里同步清理历史 undo。

更合适的方式是：

1. 正常 block flush 只负责写入新的 undo journal。
2. 在 sync loop 的较低频节点触发清理，例如：
   - 每处理完一批 blocks 后
   - 或每前进 `M` 个高度后
   - 或在后台维护线程中异步执行

这样可以避免把 undo 清理成本直接叠加到每个 block 的提交延迟上。

结合当前实现，建议把触发条件再收紧为：

1. 只有跨过 `undo_cleanup_interval_blocks` 分桶边界时才尝试 prune。
2. 如果当前 batch 仍完全位于热窗口之外，直接跳过 prune 检查。
3. 如果 DB 里尚未建立 `rollback_supported_from_height`，说明还没有开始保存 undo，也应直接跳过。
4. 如果 `undo_retained_from_height` 已经大于等于本次目标 retained height，也应直接跳过，避免重复扫描 undo meta CF。

### 5.7.6 清理实现要求

由于 undo CF 的 key 都是按 `block_height` 前缀组织，超出窗口的历史 undo 可以按高度区间清理。

设计要求：

1. 清理必须同时覆盖：
   - `BLOCK_UNDO_META_CF`
   - `BLOCK_UNDO_CREATED_UTXOS_CF`
   - `BLOCK_UNDO_SPENT_UTXOS_CF`
   - `BLOCK_UNDO_BALANCE_INDEX_CF`
2. 清理过程中不得误删当前仍处于 retention window 内的数据。
3. 如果系统正处于 `rollback_in_progress`，必须暂停 undo 清理。

### 5.7.7 与 reorg 深度的关系

`undo_retention_blocks` 本质上定义了系统愿意在线处理的最大 reorg 热窗口。

如果实际 reorg 深度超过该窗口：

- 不能继续假设在线 rollback 可完成。
- 应明确切换到 snapshot/resync 兜底流程。

因此，这个值本质上是“在线回滚能力边界”，而不只是一个空间优化参数。

## 6. 写入路径改造

## 6.1 每个 block 需要生成 BlockUndoBundle

建议在 `BatchBlockData` 里新增按 block 组织的 undo 收集结构，例如：

- `Vec<BlockUndoBundle>`

其中每个 `BlockUndoBundle` 至少包含：

- `block_height`
- `btc_block_hash`
- `created_utxos`
- `spent_utxos`
- `touched_script_hashes`

当前 `BatchBlockFlusher` 只在 flush 前汇总出整个 batch 的：

- `new_utxos`
- `spent_utxos`
- `balance_entries`

这对 forward write 足够，但对 rollback 不够，因为 rollback 需要知道“每个高度分别改了什么”。

因此应把 UTXO / balance 变更从“只按 batch 汇总”补充为“同时按 block 分组”。

这里需要额外强调一个实现约束：

- 这里的“按 block 分组”只应作用于 rollback journal 的轻量收集结构，不能反向重塑当前 batch 主缓存结构。

原因是当前 batch 处理主要服务于初始化阶段的 blk 快速批量加载；如果把 `vout_utxos`、`balances` 之类的主工作集整体改造成 block 维度，极易引入额外内存占用和 CPU 开销，拖慢初始化吞吐。

因此，推荐策略是：

- 保留现有 batch 级主缓存结构不变。
- 额外旁路收集最小化的 per-block undo 数据。
- rollback 需要的 block 维度信息，尽量从现有 `PreloadBlock`、`BlockBalanceDelta.entries` 和 `vin.cache_tx_out` 中顺手提取，而不是复制整份 batch 工作集。

## 6.2 created_utxos 的收集

来源：

- 当前 `PreloadVOut` / `vout_utxos`

每个 block 中新增的输出都应进入该 block 的 `created_utxos`。

实现上建议只记录 rollback 需要的最小字段：

- `outpoint`
- `script_hash`
- `value`

不建议把整个 `vout_utxos` map 再复制一份按 block 挂在 `BatchBlockData` 上。

## 6.3 spent_utxos 的收集

来源：

- 当前 `PreloadVIn.outpoint`
- 当前 `PreloadVIn.cache_tx_out`

只有 `need_flush == true` 的 vin 才代表“这个旧 UTXO 确实来自已有持久化状态，需要在 rollback 时恢复”。

同批次内创建又花掉的临时 UTXO 不应写入 spent undo journal，否则会在 rollback 时错误恢复出从未对外持久化过的中间态。

这也意味着：

- spent undo 的构造应尽量在已有 vin 遍历过程中顺手完成。
- 不应为了 undo 再对整批输入额外做第二轮大范围 DB 回读或全量结构重组。

## 6.4 touched_script_hashes 的收集

来源：

- 当前每个 block 已经会聚合出 `BlockBalanceDelta.entries`

由于当前每个 block 对同一 `script_hash` 只生成一条聚合后的 `BalanceHistoryEntry`，因此可以直接从 `entries` 推导 `touched_script_hashes`。

这部分数据量通常显著小于 UTXO 工作集，因此更适合作为 block 维度 rollback 索引的主要入口。

对于初始化批量加载场景，推荐优先复用已经生成好的 `BlockBalanceDelta.entries`，避免为了 rollback 再引入额外的地址级临时集合。

## 6.5 原子提交要求

每次 block batch flush 时，应在同一个 RocksDB `WriteBatch` 中同时写入：

1. `UTXO_CF` forward mutation
2. `BALANCE_HISTORY_CF` forward mutation
3. `BLOCK_COMMITS_CF` forward mutation
4. `META_CF` 中的最新 stable height
5. 所有新的 block undo journal

这样才能保证：

- 一旦 forward state 可见，对应 rollback 数据一定也已经存在。

### 6.6 性能约束与实现建议

考虑到当前 batch 模式主要服务于初始化阶段的快速追平，而追平完成后常态运行下 `batch_size` 往往接近 1，这里建议把实现分成两条约束：

1. 常态正确性优先：
   - 即使 `batch_size = 1`，也必须完整生成 per-block undo journal。
2. 初始化吞吐保护优先：
   - 当 `batch_size > 1` 时，undo 收集必须避免把主缓存结构复制成 `batch x block` 双重维度。

推荐具体做法：

1. `BatchBlockData` 中新增轻量 `block_undo_bundles`，仅保存 rollback 所需的最小记录。
2. `BlockBalanceDelta` 继续作为每 block 的主要 balance 索引来源，不额外复制 balance cache。
3. spent UTXO undo 在 vin 遍历时顺手生成，避免二次扫描整个 batch。
4. created UTXO undo 在 vout 处理时顺手生成，避免从最终 `vout_utxos` 全量反推每个 block。
5. 如果实现复杂度过高，优先允许初始化阶段的 undo 收集先以“额外线性遍历一次已加载 block”实现，但必须验证对 blk 快速加载吞吐的影响；未经基准验证，不应接受明显拖慢初始化的结构性改造。

补充当前已收敛的实现方向：

1. forward state 仍然对所有 block 正常写入。
2. undo bundle 只在热窗口内的 block flush 时生成并与 forward state 原子提交。
3. 因此初始化追块不会为远古历史块额外付出 undo 序列化和落盘成本。

## 7. rollback 执行协议

## 7.1 回滚输入

回滚入口建议定义为：

- `rollback_to_block_height(target_height: u32)`

语义：

- 回滚完成后，DB 的 stable state 应等价于“只处理到了 target_height”。

## 7.2 回滚顺序

建议从当前最高已提交高度开始，按高度递减逐块回滚：

- `current_height`
- `current_height - 1`
- ...
- `target_height + 1`

每次只反演一个 block，并在一个 `WriteBatch` 中完成。

## 7.3 单块回滚动作

对高度 `H` 执行回滚时：

1. 读取 `BLOCK_UNDO_META_CF[H]`
2. 扫描并删除 `BLOCK_UNDO_CREATED_UTXOS_CF` 中该高度所有记录
3. 扫描并恢复 `BLOCK_UNDO_SPENT_UTXOS_CF` 中该高度所有记录到 `UTXO_CF`
4. 扫描 `BLOCK_UNDO_BALANCE_INDEX_CF` 中该高度所有 `script_hash`
5. 对每个 `script_hash` 删除 `BALANCE_HISTORY_CF[script_hash || H]`
6. 删除 `BLOCK_COMMITS_CF[H]`
7. 删除该高度所有 undo journal 记录
8. 将 `btc_block_height` 更新为 `H - 1`
9. 更新 `rollback_next_height`

所有动作必须放在同一个 `WriteBatch` 中。

## 7.4 为什么按高度逐块回滚，而不是一次删整个区间

原因：

1. UTXO 恢复天然是按块逆序的。
2. 多块 rollback 期间需要崩溃可恢复。
3. 逐块回滚可以输出更清晰的日志和进度。
4. 如果中间发现 undo journal 缺失，可以明确停在坏点，而不是把更早的状态也破坏掉。

## 7.5 缓存处理

当前 `balance-history` 还维护内存 `utxo_cache` 和 `balance_cache`。

对 rollback 的 v1 实现，建议采取最保守方案：

- rollback 完成后直接清空两类 cache

原因：

- 数据库回滚是持久化真相。
- 缓存属于可重建状态。
- 尝试精确反演缓存虽然也可行，但容易引入第二套复杂性。

只要 rollback 不是高频动作，清 cache 的代价是可接受的。

## 8. 崩溃恢复设计

## 8.1 为什么必须显式记录 rollback state

单块 forward flush 当前已经是原子写入，但多块 rollback 是一个更长流程。

如果没有显式 rollback state，可能出现：

- 回滚到一半崩溃
- 重启后只看到一个更低的 `btc_block_height`
- 但未来高度的 undo journal 或 block commit 仍残留

因此必须把 rollback 过程本身显式状态化。

## 8.2 建议流程

开始 rollback 前：

1. 设置 `rollback_in_progress = 1`
2. 设置 `rollback_target_height`
3. 设置 `rollback_next_height = current_height`

每回滚完一个 block：

1. 将 `rollback_next_height` 更新为 `H - 1`

全部完成后：

1. 删除 `rollback_in_progress`
2. 删除 `rollback_target_height`
3. 删除 `rollback_next_height`

启动时如果发现 `rollback_in_progress = 1`，则直接恢复 rollback 流程，而不是继续正常同步。

## 9. 与 reorg 检测的边界

本文只细化 rollback 数据模型，不完全展开检测策略。

但数据模型需要满足以下检测边界：

1. 检测层负责找到共同祖先高度 `ancestor_height`。
2. rollback 层只负责把 DB 状态退回到 `ancestor_height`。
3. rollback 完成后，正常同步逻辑再从 `ancestor_height + 1` 沿 canonical chain 继续。

当前建议：

- reorg 检测使用已提交 `BLOCK_COMMITS_CF` 里的 `btc_block_hash` 作为本地锚点，不额外引入新的链锚 CF。

### 9.1 检测触发时机约束

虽然 rollback 层只接收 `ancestor_height`，但工程实现里必须明确一条约束：

- reorg 检测不能只在“准备同步更高新区块”时触发。

原因是仅依赖 `latest_height > last_height` 会漏掉以下场景：

1. 同高度 reorg：本地与 canonical tip 高度相同，但 block hash 已变化。
2. tip 回退：canonical tip 暂时低于本地已同步高度。
3. tip 先回退后恢复：canonical 高度重新回到原高度，但 block hash 已不同。

因此更稳妥的实现约束应是：

1. `sync_once()` 入口始终执行一次本地 tip 与 canonical chain 的对齐检查。
2. 等待新块阶段除了观察高度增长，还要观察 watched tip hash 是否发生变化。
3. 一旦发现高度变化或 hash 变化，应立即唤醒主循环并进入统一的 reorg reconcile 路径。

### 9.2 共同祖先搜索边界

共同祖先搜索不能默认从 `current_height` 直接读取 canonical hash。

更稳妥的边界是：

1. 先读取当前 canonical `latest_btc_height`。
2. 从 `min(current_height, latest_btc_height)` 开始向下搜索共同祖先。
3. 如果 `latest_btc_height < current_height`，要把它视为合法 reorg 场景，而不是直接当作 RPC 异常。

这样可以避免在深 reorg 或节点暂时掉高时，因为读取不存在的 canonical 高度而把正常 rollback 流程误判成错误重试。

## 10. 与 snapshot 的关系

## 10.1 v1 不要求把 undo journal 打进 snapshot

当前建议：

- snapshot 仍以 stable forward state 为主
- 不把 undo journal 作为 snapshot 必需内容

原因：

1. snapshot 主要是状态分发和快速恢复工具。
2. undo journal 主要是在线运行期间的 rollback 工具。
3. 把 undo journal 强行塞进 snapshot 会显著放大 snapshot 体积和复杂度。

## 10.2 这意味着什么

如果节点从某个 snapshot 高度 `S` 安装启动：

- 只能保证从 `S` 往后新增的 block 拥有完整 undo journal
- 若需要回滚到 `S` 之前，仍需要更老 snapshot 或全量重建

这在工程上是可以接受的，因为：

- reorg 处理主要关注近端热区块
- 深历史恢复本就更适合用 snapshot / resync 处理

## 10.3 后续可选增强

后续如果需要，可以再扩展：

- 仅在 snapshot 中附带最近 `K` 个 block 的 undo journal

但不建议把这项能力放进第一阶段实现。

## 11. 兼容性与迁移

## 11.1 对现有存量库的影响

新增 undo CF 后，新的 block 会开始具备 rollback 能力。

但存量 DB 在升级前已提交的旧高度并没有 undo journal，因此存在一个边界：

- rollback 只能保证到“第一批具备 undo journal 的高度”之后。

## 11.2 建议的迁移策略

建议显式引入一个 meta 边界：

- `rollback_supported_from_height`

语义：

- 只有 `height >= rollback_supported_from_height` 的 forward state 才能被自动 rollback。

对于存量部署，有两种选择：

1. 升级后二次全量重建，建立完整 rollback 边界。
2. 升级后仅对新增长度提供 rollback，旧高度不足时退化为 snapshot rollback / resync。

当前建议优先采用第 2 种，降低首轮改造成本。

## 12. 测试要求

至少应补齐以下测试：

1. 单块 rollback：
   - 创建 UTXO
   - 花费 UTXO
   - 写入 balance history
   - 写入 block commit
   - 回滚后全部恢复到前一高度
2. 多块连续 rollback：
   - 从高度 `H` 回滚到 `H - N`
3. 崩溃恢复：
   - rollback 执行中断后重启继续
4. 与 snapshot 安装的边界：
   - snapshot height 之后的块可 rollback
   - snapshot height 之前不可误判为可 rollback
5. reorg e2e：
   - 构造短 reorg
   - 找到共同祖先
   - rollback 后重放正确

## 13. 推荐实现顺序

建议按以下顺序推进：

1. 在 DB 层新增 undo CF 和 meta 键定义。
2. 在 `BatchBlockData` / `BatchBlockFlusher` 中补齐 `BlockUndoBundle` 收集与原子写入。
3. 实现单块 rollback 原语。
4. 实现多块 rollback 和崩溃恢复。
5. 再把检测层接进来，形成完整 reorg 处理路径。

## 14. 当前建议结论

针对 `balance-history`，当前最推荐的方向不是“直接依赖 snapshot 重建”，而是：

- 以 per-block undo journal 作为主 rollback 原语
- 以 snapshot rollback / resync 作为深历史或迁移期兜底

原因是：

1. 它最贴合当前 RocksDB forward state 的布局。
2. 它不要求重写现有查询语义。
3. 它能真正解决 UTXO 当前态无法恢复的问题。
4. 它允许后续把 reorg 处理收敛成标准的“检测 -> rollback -> replay”流程。

这版设计可作为后续实现 `balance-history` rollback 能力的第一版工作底稿。