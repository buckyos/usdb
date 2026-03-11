# Balance-History Review 结论与修复方案清单

## 1. 目的

本文档用于沉淀本轮对 `balance-history` 核心服务的两轮 code review 结论，并给出后续可执行的修复方案清单与待做事项。

关注范围主要包括：

- 索引主循环与持久化边界
- 快照生成与安装
- block commit 元数据链路
- 基于 electrs 的校验链路
- 基于本地 blk 文件的加速与恢复链路

本文档不是最终设计文档，而是后续修复工作的工作底稿。后续如有方案落地，应继续在本文档上追加“已完成项”“设计决议”“验收结果”等内容。

## 2. 总体结论

### 2.1 可以肯定的部分

- 当前 `balance-history` 已经具备较完整的索引、快照、验证、RPC 暴露和本地 blk 加速能力。
- 当前实现已经不是简单原型，很多主流程在正常路径下可以工作，并且已经过线上使用验证。
- block commit 链已经进入正式实现，而不是仅停留在文档层。

### 2.2 当前最需要处理的方向

- 把“逻辑上稳定”收敛为“可原子恢复、可一致校验、可完整快照化”的稳定状态。
- 统一快照、RPC、校验器三者对 stable state 的定义。
- 修复恢复链路中的伪恢复问题，避免系统在“以为自己恢复了”的前提下继续运行。

## 3. 两轮 Review 结论汇总

## 3.1 第一轮 Review 重点

第一轮重点看了主索引流程、RocksDB 写入边界、快照生成/安装、block commit 链和服务对外状态暴露。

结论如下：

1. 快照安装过程不是原子替换，而是直接向现有 DB 逐段写入。
2. 快照格式当前没有覆盖 block commit 元数据，但 RPC 已经把 block commit 作为稳定状态的一部分向外暴露。
3. 单批次 block flush 被拆成 UTXO 写入与余额/commit/高度写入两个阶段，中间存在崩溃不一致窗口。
4. 很多核心写路径使用非同步写，真正的落盘边界晚于部分语义上已经对外可见的稳定状态。
5. 本地 blk 索引恢复依赖的 `clear_blocks()` 实现疑似无效，会导致恢复路径建立在脏数据之上。

## 3.2 第二轮 Review 重点

第二轮重点补看了 electrs 校验链路和本地 blk loader 的加载/重建分支。

新增结论如下：

1. latest verifier 使用的是 electrs 当前 tip 余额，而不是 stable height 余额，存在误报风险。
2. local loader 的“最近 10 个 blk 文件可直接复用”判断存在无符号下溢风险。
3. local loader 从 DB 直接加载索引前，缺少足够的连续性与主链一致性校验。

## 4. 关键问题清单

以下问题按风险优先级排序。

### P0-1. 快照安装不是原子替换，存在混合代际状态风险

现状：

- 安装流程先写 balance history，再写 UTXO，最后才更新 `btc_block_height`。
- 如果目标 DB 不是空库，旧数据可能残留。
- 如果安装中途失败，可能留下“新旧混合”的中间状态。

直接风险：

- DB 内部状态不再对应单一 stable height。
- 快照安装失败后难以判断当前库是否还能继续安全使用。
- 下游通过 RPC 看到的 stable height 与底层实际内容可能不一致。

相关代码：

- [src/btc/balance-history/src/index/snapshot.rs](src/btc/balance-history/src/index/snapshot.rs#L277)
- [src/btc/balance-history/src/index/snapshot.rs](src/btc/balance-history/src/index/snapshot.rs#L280)
- [src/btc/balance-history/src/index/snapshot.rs](src/btc/balance-history/src/index/snapshot.rs#L283)

### P0-2. 快照链路缺失 block commit 元数据，stable state 定义不闭合

现状：

- 快照元数据只记录高度、计数、时间和版本。
- 快照安装不导入 block commit 数据。
- 但 RPC `get_snapshot_info` 和 `get_block_commit` 依赖当前 stable height 对应的 commit。

直接风险：

- 快照安装后的服务可能拿不到正确的 `latest_block_commit`。
- 即使高度正确，快照身份和逻辑状态提交链也可能不完整。
- 下游以后如果依赖 commit 做对齐或验收，会被这条缺口阻断。

相关代码：

- [src/btc/balance-history/src/db/snapshot.rs](src/btc/balance-history/src/db/snapshot.rs#L13)
- [src/btc/balance-history/src/index/snapshot.rs](src/btc/balance-history/src/index/snapshot.rs#L277)
- [src/btc/balance-history/src/service/server.rs](src/btc/balance-history/src/service/server.rs#L168)
- [src/btc/balance-history/src/service/server.rs](src/btc/balance-history/src/service/server.rs#L200)

### P0-3. 单批次 block flush 不是一个原子提交单元

现状：

- block flush 先写 UTXO，再写余额历史、block commit 和 `btc_block_height`。
- 这两段写入分别进入不同的 RocksDB 写批次。
- 多处核心写路径使用 `set_sync(false)`。
- 整体 `flush_all()` 只在 `sync_once()` 末尾统一调用。

直接风险：

- 崩溃时可能出现 UTXO 已更新，但 balance history / block commit / stable height 尚未一致更新。
- 恢复后系统缺少明确的 pending/rollback 机制来判断这类中间态。
- 当前外部可见的 stable state 与内部实际 durable 边界之间存在缝隙。

相关代码：

- [src/btc/balance-history/src/index/block.rs](src/btc/balance-history/src/index/block.rs#L571)
- [src/btc/balance-history/src/index/block.rs](src/btc/balance-history/src/index/block.rs#L578)
- [src/btc/balance-history/src/index/block.rs](src/btc/balance-history/src/index/block.rs#L635)
- [src/btc/balance-history/src/index/block.rs](src/btc/balance-history/src/index/block.rs#L680)
- [src/btc/balance-history/src/db/db.rs](src/btc/balance-history/src/db/db.rs#L588)
- [src/btc/balance-history/src/db/db.rs](src/btc/balance-history/src/db/db.rs#L1027)
- [src/btc/balance-history/src/index/indexer.rs](src/btc/balance-history/src/index/indexer.rs#L395)
- [src/btc/balance-history/src/index/indexer.rs](src/btc/balance-history/src/index/indexer.rs#L454)

### P1-1. `clear_blocks()` 当前实现不能真正清空 block 相关列族

现状：

- `clear_blocks()` 名义上要清理 `BLOCKS_CF`、`BLOCK_HEIGHTS_CF`、`BLOCK_COMMITS_CF`。
- 实际上只是对空 key 做 `delete_cf`。

直接风险：

- 清理完成的日志是假的，底层 block 相关数据可能仍然存在。
- local loader 恢复/重建路径可能继续复用脏索引。
- 后续 rebuild 结果可能混入旧数据。

相关代码：

- [src/btc/balance-history/src/db/db.rs](src/btc/balance-history/src/db/db.rs#L1738)
- [src/btc/balance-history/src/db/db.rs](src/btc/balance-history/src/db/db.rs#L1760)
- [src/btc/balance-history/src/db/db.rs](src/btc/balance-history/src/db/db.rs#L1771)
- [src/btc/balance-history/src/db/db.rs](src/btc/balance-history/src/db/db.rs#L1782)

### P1-2. local loader 复用阈值判断存在下溢和误判风险

现状：

- `last_block_file_index >= current_last_blk_file - 10` 直接对 `u32` 做减法。
- 当当前 blk 文件数量较少时，可能发生下溢。
- 该分支命中后会直接从 DB 载入缓存，而不是强校验后再复用。

直接风险：

- debug/release 下表现不一致。
- 启动时可能错误进入“直接复用 DB 索引”分支。
- 在索引不完整、链变化或旧状态残留时继续错误运行。

相关代码：

- [src/btc/balance-history/src/btc/local_loader.rs](src/btc/balance-history/src/btc/local_loader.rs#L392)
- [src/btc/balance-history/src/btc/local_loader.rs](src/btc/balance-history/src/btc/local_loader.rs#L409)
- [src/btc/balance-history/src/btc/local_loader.rs](src/btc/balance-history/src/btc/local_loader.rs#L412)

### P1-3. local loader 从 DB 恢复前缺少索引连续性与主链一致性校验

现状：

- `load_from_db()` 直接加载 blocks 和 block heights。
- `generate_sort_blocks()` 会在 fork 情况下再去 RPC 取主链块，但恢复路径并没有先验证 DB 内索引整体仍然自洽。

直接风险：

- DB 中残留的 block index 与当前节点主链视图可能已经脱钩。
- 启动时可能在错误前提上继续读取本地 blk 文件。

相关代码：

- [src/btc/balance-history/src/btc/local_loader.rs](src/btc/balance-history/src/btc/local_loader.rs#L48)
- [src/btc/balance-history/src/btc/local_loader.rs](src/btc/balance-history/src/btc/local_loader.rs#L129)
- [src/btc/balance-history/src/btc/local_loader.rs](src/btc/balance-history/src/btc/local_loader.rs#L412)

### P2-1. latest verifier 没有锚定 stable height，而是对比 electrs 当前 tip

现状：

- `verify_latest()` 遍历的是本地 DB 当前 latest 记录。
- 但 latest balance 校验调用的是 electrs `get_balance()` / `get_balances()`。
- 这两个接口没有带 block height 锚点。

直接风险：

- electrs 比本地 stable height 更快时会误报 mismatch。
- 当前 `flush + sleep + retry` 更像在规避竞态，而不是解决语义问题。

相关代码：

- [src/btc/balance-history/src/index/verify.rs](src/btc/balance-history/src/index/verify.rs#L30)
- [src/btc/balance-history/src/index/verify.rs](src/btc/balance-history/src/index/verify.rs#L273)
- [src/btc/balance-history/src/index/verify.rs](src/btc/balance-history/src/index/verify.rs#L279)
- [src/btc/balance-history/src/index/verify.rs](src/btc/balance-history/src/index/verify.rs#L314)
- [src/btc/balance-history/src/index/verify.rs](src/btc/balance-history/src/index/verify.rs#L319)

## 5. 修复方案清单

## 5.1 第一阶段：先消除明显的一致性与恢复风险

这一阶段目标是把“明显可能产生脏状态或伪恢复”的问题先收敛掉。

建议项：

1. 重写 `clear_blocks()`，确保真正清空 block 相关列族。
2. 修复 local loader 的阈值判断，使用安全减法，例如 `saturating_sub`。
3. 在 local loader 从 DB 恢复前增加基础一致性校验：
   - block heights 是否连续
   - latest height 与 latest hash 是否可回查
   - block file index 是否仍处于当前本地 blk 文件范围内
4. 任一校验失败时，不再尝试“带病复用”，而是明确进入全量重建。

目标结果：

- 启动恢复路径可预测。
- 清理动作真实有效。
- 不再出现“看上去恢复了，实际上带着脏索引运行”的情况。

### 5.1.0 当前进展

截至当前版本，第一阶段已落地的内容包括：

- `clear_blocks()` 已改为真正清空 `BLOCKS_CF`、`BLOCK_HEIGHTS_CF`、`BLOCK_COMMITS_CF`，并带清理后校验。
- local loader 的恢复阈值判断已改为 `saturating_sub(10)`，消除了下溢风险。
- local loader 已不再“命中阈值就直接复用”，而是改为“先恢复到内存，再做最小一致性校验”。
- 恢复失败路径已实现 `clear_and_rebuild` 前置清理，并补了针对性测试。

当前尚未完成的部分主要是：

- 覆盖真实 rebuild 成功路径的更完整测试。
- `clear_blocks()` 失败时更细粒度的故障注入测试。

### 5.1.1 实施设计：`clear_blocks()` 真正清空 block 相关状态

建议把 `clear_blocks()` 从“删除几个特定 key”改成“显式清空整个 block 相关状态集”。

建议修改点：

1. 保留清理 `META_KEY_LAST_BLOCK_FILE_INDEX`。
2. 对以下列族执行真正的范围删除或遍历删除：
   - `BLOCKS_CF`
   - `BLOCK_HEIGHTS_CF`
   - `BLOCK_COMMITS_CF`
3. 清理完成后追加一次只读校验，确认：
   - `get_last_block_file_index() == None`
   - `get_all_blocks().is_empty()`
   - `get_all_block_heights().is_empty()`
4. 如果任一清理后校验失败，直接返回错误，禁止继续进入 local loader 恢复流程。

建议实现方式：

- 优先使用 RocksDB 的范围删除能力清空整个列族 keyspace。
- 如果当前封装层不便直接做范围删除，则在 DB 层增加通用辅助函数，按 iterator 遍历该 CF 全量删除。
- 不建议继续保留当前“删空 key”写法，因为它会制造伪成功日志。

建议新增或调整的函数：

- 在 DB 层新增一个内部辅助函数，例如：
   - `clear_column_family(cf_name: &str) -> Result<usize, String>`
- 保留对外入口：
  - `clear_blocks() -> Result<(), String>`
- 在测试中新增：
  - `test_clear_blocks_removes_all_block_state()`

日志要求：

- 开始清理时记录目标列族。
- 清理后记录删除条数，或至少记录清理后剩余条数为 0。
- 失败日志中带上列族名和阶段名，避免只有一条泛化错误信息。

### 5.1.2 实施设计：local loader 的恢复判定改为“先验证，再复用”

建议把当前 `build_index()` 中“满足阈值就直接 load_from_db”改成明确的三段式：

1. `should_try_restore_from_db()`
2. `validate_persisted_block_index()`
3. `restore_or_rebuild()`

建议流程如下：

1. 读取 `last_block_file_index`。
2. 读取当前 `current_last_blk_file`。
3. 用安全减法计算复用阈值：
   - `restore_threshold = current_last_blk_file.saturating_sub(10)`
4. 只有在 `last_block_file_index >= restore_threshold` 时才进入“可尝试恢复”分支。
5. 进入恢复分支后，先做校验，再决定是否真正 `load_from_db()`。
6. 任一校验失败：
   - 输出 warning
   - 调用 `clear_blocks()`
   - 进入全量 rebuild

建议不要再出现“命中阈值就直接复用缓存”的分支。

建议新增或调整的函数：

- `should_try_restore_from_db(last_block_file_index, current_last_blk_file) -> bool`
- `validate_loaded_block_index(...) -> Result<(), String>`
- `restore_or_clear_persisted_block_index(...) -> Result<(), String>`
- `rebuild_block_index() -> Result<(), String>`

这样可以把“分支选择”“状态验证”“索引重建”三类职责拆开，后续也更容易测试。

### 5.1.3 实施设计：恢复前一致性校验最小集合

第一阶段不要求一次把所有自愈逻辑做完，但至少应补齐最小一致性检查。

建议校验项：

1. 元数据范围校验
   - `last_block_file_index <= current_last_blk_file`
   - `block_hash_cache` 非空时，其 `block_file_index` 最大值不应大于 `last_block_file_index`
2. 高度连续性校验
   - `get_all_block_heights()` 返回的高度必须从 `0` 开始连续递增
   - 不允许跳高、不允许重复高度
3. 高度到 hash 的可回查校验
   - `sorted_blocks` 中每个 `block_hash` 都必须在 `block_hash_cache` 中存在
4. 主链 tip 对齐校验
   - 取恢复后最高高度 `tip_height`
   - 用 RPC 查询该高度的 `block_hash`
   - 要求 RPC 返回的 `block_hash` 与缓存中的 `tip_hash` 一致

第一阶段可以先不做的事情：

- 不需要在启动时对所有高度做全量 RPC 校验。
- 不需要在恢复阶段重新扫描全部 blk 文件内容。
- 不需要在第一阶段做多点 RPC hash 抽查，保留一个 tip 锚点即可。

目标是先把“明显已经坏了的索引”筛掉，而不是做完整审计。

### 5.1.4 实施设计：恢复失败时的降级与日志策略

当前实现的问题之一不是没有失败分支，而是失败后系统状态描述不够明确。

建议统一以下策略：

1. 恢复前校验失败：
   - 记录 `warn!`
   - 明确写出失败原因、是否将清理 DB block index、是否进入 rebuild
2. `clear_blocks()` 失败：
   - 直接返回 `Err`
   - 阻止继续使用 local loader
3. rebuild 失败：
   - 直接返回 `Err`
   - 由上层决定是否切回 RPC-only 或终止启动
4. rebuild 成功：
   - 重新持久化 block index
   - 写出新的 `last_block_file_index`

建议日志模板：

```rust
warn!(
    "Persisted block index validation failed: module=local_loader, reason={}, action=clear_and_rebuild, db_last_block_file_index={}, current_last_blk_file={}",
    reason,
    last_block_file_index,
    current_last_blk_file
);
```

这样后续排查时可以直接区分：

- 是阈值不满足，正常走 rebuild
- 还是阈值满足，但校验失败后被强制 rebuild
- 还是清理或重建本身失败

### 5.1.5 第一阶段建议测试用例

建议至少补以下测试：

1. `clear_blocks()` 后所有 block 相关 CF 为空。
2. `current_last_blk_file < 10` 时，恢复阈值判断不下溢。
3. DB 中存在断裂高度序列时，`validate_loaded_block_index()` 返回错误。
4. DB 中 tip hash 与 RPC tip hash 不一致时，恢复分支拒绝复用并进入重建。
5. 恢复失败时，persisted block index 会被清理。
6. `clear_blocks()` 失败时，local loader 不继续往下执行。
7. rebuild 成功后，新的 block index 可以再次通过恢复校验。

### 5.1.6 第一阶段实施顺序建议

建议按以下顺序实现，避免交叉回滚：

1. 先修 `clear_blocks()` 和其测试。 已完成。
2. 再抽出 `should_try_restore_from_db()`，消除阈值下溢。 已完成。
3. 再实现 `validate_loaded_block_index()` 与恢复失败降级逻辑。 已完成。
4. 最后补测试，覆盖“脏索引 -> 清理 -> rebuild”的完整启动路径。 进行中。

这样做的原因是：

- `clear_blocks()` 是后续恢复链路的基础动作。
- 阈值修复是最小且独立的改动，适合先落地。
- 校验逻辑引入后，恢复分支的行为会更稳定，也更容易被测试固定下来。

## 5.2 第二阶段：统一 stable state 的提交边界

这一阶段目标是明确并收敛 stable state 的原子提交单元。

建议项：

1. 重新定义单批次 block flush 的提交边界。
2. 优先评估能否把以下内容纳入同一写批次：
   - UTXO 更新
   - balance history 更新
   - block commit 更新
   - `btc_block_height` 更新
3. 如果因为规模或实现限制无法做到单批次原子提交，则引入恢复日志或 pending marker：
   - 批次开始前写 pending 状态
   - 批次完全完成后清除 pending 状态
   - 启动时检测 pending 状态并决定回滚或重放
4. 明确 stable height 只有在对应状态完整提交后才允许推进。

目标结果：

- 崩溃后系统可以判断自己是否处于中间态。
- stable height、block commit、balance history、UTXO 的关系变得可恢复、可解释。

## 5.3 第三阶段：重做快照安装语义

这一阶段目标是把 snapshot install 从“批量导入”升级成“可验证的状态切换”。

建议项：

1. 在设计上二选一：
   - 仅允许向空库安装
   - 先安装到临时目录，再原子替换正式库
2. 无论采用哪种方式，都应在安装开始前明确校验目标状态，而不是默认覆盖。
3. 安装完成前，不应提前暴露新的 stable height。
4. 安装失败后，系统应明确保证“旧库仍可用”或“目标库被标记为不可继续使用”，不能留下语义不明的半成品状态。

目标结果：

- snapshot install 成为可回滚或可重试的明确状态切换过程。
- 运维侧可以清楚知道安装失败后的系统状态。

## 5.4 第四阶段：补齐 block commit 快照链路

这一阶段目标是让快照、RPC 和 stable state 的定义完全闭合。

建议项：

1. 扩展 snapshot schema，使其覆盖 block commit 元数据。
2. 明确 snapshot identity 至少应包含：
   - stable block height
   - stable block hash
   - latest block commit
   - commit protocol version
   - commit hash algo
3. 安装 snapshot 时同步恢复 commit 数据。
4. snapshot 安装完成后，用 `get_snapshot_info` 做一致性校验。

目标结果：

- 快照不再只是“余额 + UTXO 数据包”，而是完整 stable state 的可迁移载体。
- 下游未来如果依赖 commit 做比对，将不再受制于快照能力缺口。

## 5.5 第五阶段：修正 verifier 语义

这一阶段目标是把 verifier 从“偶尔误报”的排查工具，提升为稳定验收工具。

建议项：

1. latest verifier 改为按 stable height 校验，而不是直接用 electrs 当前 tip。
2. 如果 electrs 无法直接按高度返回 latest 余额，则统一走 `calc_balance(height)` 慢路径。
3. 仅在确认 electrs tip 与本地 stable height 一致时，才允许启用 `get_balance()` 快路径。
4. 将“校验时使用的高度”明确打印到日志中。

目标结果：

- verifier 的失败更可信。
- 验证结果反映的是 stable state 语义，而不是两个系统谁更快。

## 6. 待做事项

以下事项按建议顺序排列，可直接作为后续开发 checklist 使用。

- [ ] 修复 `clear_blocks()`，并补单元测试验证列族确实被清空。
- [x] 修复 `clear_blocks()`，并补单元测试验证列族确实被清空。
- [x] 修复 local loader 阈值判断下溢问题。
- [x] 为 local loader 增加恢复前一致性检查函数。
- [x] 明确 local loader 在恢复失败时的降级策略和日志格式。
- [x] 为恢复失败后清理 persisted block index 补充针对性测试。
- [ ] 设计并落地单批次 block flush 的原子提交方案。
- [ ] 如果无法单批次原子提交，设计 pending marker / recovery journal 方案。
- [ ] 明确 stable height 推进条件，并补文档说明。
- [ ] 设计 snapshot install 的空库安装或原子替换方案。
- [ ] 扩展 snapshot schema，纳入 block commit 元数据。
- [ ] 安装 snapshot 后补充 `get_snapshot_info` 一致性校验。
- [ ] 调整 verifier，使 latest 校验锚定 stable height。
- [ ] 为崩溃恢复和中途失败场景补充 fault injection / recovery tests。

## 7. 测试与验收建议

后续每一阶段落地后，建议至少补以下验证：

1. 恢复测试：
   - block index 脏数据恢复
   - snapshot install 中途失败恢复
   - block flush 中途失败恢复
2. 一致性测试：
   - stable height 与 block commit 对齐
   - snapshot install 前后 `get_snapshot_info` 一致
   - verifier 在 electrs tip 领先时不误报
3. 回归测试：
   - regtest smoke 继续可跑通
   - 常规地址余额查询语义不回退

## 8. 建议的实施顺序

建议采用以下顺序推进：

1. 先修恢复链路：`clear_blocks()`、local loader 阈值与恢复校验。
2. 再收敛持久化原子性：block flush 边界、stable height 推进、pending marker。
3. 然后重做 snapshot install 语义，并补齐 block commit 快照链路。
4. 最后调整 verifier，使其严格基于 stable state 语义工作。

原因：

- 第一阶段工作量相对可控，但收益很高。
- 第二、三阶段会影响状态边界定义，应在恢复路径稳定后处理。
- verifier 调整依赖前面稳定状态语义收敛，否则容易反复改。

## 9. 后续维护方式

建议后续基于本文档持续更新以下内容：

- 已完成事项
- 设计决议
- 尚未关闭的风险项
- 测试结果与回归结论

相关文档：

- [doc/balance-history-rpc.md](./balance-history-rpc.md)
- [doc/balance-history-rpc_en.md](./balance-history-rpc_en.md)
- [doc/balance-history-regtest-smoke.md](./balance-history-regtest-smoke.md)
- [doc/usdb-双链共识接入问题风险与改造清单.md](./usdb-%E5%8F%8C%E9%93%BE%E5%85%B1%E8%AF%86%E6%8E%A5%E5%85%A5%E9%97%AE%E9%A2%98%E9%A3%8E%E9%99%A9%E4%B8%8E%E6%94%B9%E9%80%A0%E6%B8%85%E5%8D%95.md)