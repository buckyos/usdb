# P6.3：并行 bootstrap 与按需 LocalLoader

日期：2026-09-12。上一批可信旧 commit 检查点与原生 bootstrap 已提交为 `a897975`。
本节记录其后的 P6.3 实现和验收；主网长任务及整套部署默认切换尚未执行。

## 1. 启动顺序与就绪边界

balance-history 可以与 Core 的前台同步同时运行，不必等待最新高度、业务 genesis G 或后台历史验证完成才开始导入。
需要区分“导入完成，可以逐块重放”和“整个 bootstrap 完成，可以发布业务状态”：

1. Core 加载受支持的 UTXO 快照并激活其 chainstate，前台从 B 追平，后台从创世块验证旧历史。
2. Core RPC 可用、active chain 已到 B 且 B hash 正确后，balance-history 导入同一快照，并核验完整文件身份与逻辑承诺。
3. 安装可信 C(B)/D(B)，持续重放至 `min(G, active_tip - stable_lag)`。暂无下一稳定块时等待，Core 下载继续进行。
4. G 达到稳定深度，canonical hash、状态投影和独立余额聚合全部通过后，原子封存并发布数据库，再启动业务 RPC。
5. 正常同步 G+1 以后的稳定区块。Core 后台验证继续遵循官方机制，各节点分别执行。

Core 的前后台并行机制见[官方设计](https://github.com/bitcoin/bitcoin/blob/v31.1/doc/design/assumeutxo.md)。
本实现不替代 Core 的历史验证；也不把 Core 前台可用等同于整套 USDB 就绪。
当前 stable lag=10，因此实验 G=963800 的发布条件至少是 active tip=963810。
G 可按正式网业务设定，但必须 G>=B；当前内置主网检查点仅支持 B=935000。

等待期间 `<root>/bootstrap-progress.json` 示例：

```json
{"phase":"waiting_for_blocks","height":102,"target":115,"btc_tip":112,"published":false}
```

该阶段不开放业务 RPC。等待可合作式取消，重启继续使用持久化导入/重放状态。
已完成输入核验并进入重放的恢复不重新导入；未完成输入核验的导入恢复仍会重扫、检查前缀。
G 的固定 hash 在 Core 已有该高度时核验，封存前再次核对；等待过程中发现重组会重新核对恢复点，
在可恢复边界内回滚并清空相应缓存，跨越基线或固定 G 身份不匹配时明确失败。

服务二进制已支持上述顺序。node-kit/controller 的默认启动编排仍属于 P7，本轮未修改。

## 2. LocalLoader 的职责与实现

LocalLoader 是区块内容的可选加速来源。canonical 高度、hash 和链选择始终来自 Core RPC，
本地 `blk*.dat` 不要求从创世块到 B 连续，也不要求 Core 后台历史补齐。

每批积压为 `min(阶段目标, active_tip - stable_lag) - 已处理高度`：

- 超过 `sync.local_loader_threshold` 才尝试本地读取，默认500块；等于或小于阈值使用 RPC。
- bootstrap 的阶段目标为 G，正常同步的目标受 `max_sync_block_height` 限制。
- 每批重新判断，启动时没有积压、后来落后也能启用；接近目标时自动使用 RPC。
- 本地缺目录、缺块、文件尚未写完、候选块损坏或索引不可用均回退 RPC；不等待全量本地索引完成。

并行启动并不保证 LocalLoader 大部分时间不会使用。balance-history 导入 UTXO 期间 Core 仍会下载；
导入时间、双方处理速度、停机时长和 G-B 都会影响积压。此前[P4主网导入](./balance-history-assumeutxo-p4-validation-2026-09-12.md)
耗时1945.5秒，可说明导入窗口并非可以忽略，但不能用它预测当前原生路径的耗时或本地命中率。

实现入口：

- [canonical_loader.rs](../../src/btc/balance-history/src/btc/canonical_loader.rs)：按需选择来源、RPC canonical 排序及回退。
- [canonical_index.rs](../../src/btc/balance-history/src/btc/canonical_index.rs)：独立的候选物理位置索引。
- [native.rs](../../src/btc/balance-history/src/bootstrap/native.rs)：先导入、随稳定区块到达逐段重放、校验与发布。

索引位于 `<root>/local-block-index/<source-identity>`，与权威余额库、Core 内部 LevelDB 分离。
它绑定 blocks 目录、网络 magic、XOR key；缓存只记录 `block_hash -> file/offset/length`。
所有文件分别保存游标，支持稀疏编号、乱序记录、多个同时追加的文件及最新文件。
每次最多扫描4096条候选记录，只读取记录头并跳过 payload；文件大小/mtime 变化触发继续扫描，
即使元数据未变也会在下一次刷新且距上次检查满30秒时复查尾部，不将文件永久视为封存。
预分配零尾、部分帧、XOR 的绝对文件偏移以及重启后的续扫均独立处理。

读取本地候选块时完整解码，并校验请求 hash、Merkle root、witness commitment 和重复 txid；
批次还核验 canonical 父链连续性和末块 hash，避免旧分叉或取块中的重组定义逻辑顺序。
重复 txid 检查覆盖追加相同末尾交易却保持 Merkle root 不变的情况。
这些检查用于校验本地候选内容，不重新实现 Core 的共识验证。

日志关键字包括 `module=canonical_local_loader`、`local_enabled`、`blocks_behind`、
`local_blocks/rpc_blocks/indexed_records`；其中 indexed_records 包括重查尾部，不是唯一块数。
此缓存可在服务停止后重建，不属于余额/commit 状态，正常运行不需要人工删除。
原生库继续采用正常数据库持久化策略；旧的全历史 LocalLoader 入口和其导入策略仍留给旧路径。
历史 prevout/reveal 定位与 txindex 依赖属于 P6.4，未因本地读取通过而解决。

## 3. 验证结果

[专项集成测试](../../tests/assumeutxo_local_loader.rs)共7项通过，覆盖：

| 场景 | 验收 |
| --- | --- |
| B 以前本地块缺失、B 后块乱序放置 | 原生 bootstrap 无历史 payload RPC 调用，状态和旧 v1 commit 与全量重放相同 |
| 两个文件在预分配大小不变时分别追加、XOR、部分尾部 | 已完成块本地读取；未完成块回退 RPC，补全后本地读取 |
| 多种帧截断、重复记录、旧分叉、重启 | 不消费错误尾部或旧分叉；持久化游标支持恢复 |
| 超过4096条记录的扫描预算 | 首次未命中回退 RPC；重启继续增量扫描 |
| 交易内容损坏、witness 损坏、重复末尾交易保持 Merkle root | 拒绝本地候选并回退 RPC |
| Core 未到 G / G 稳定深度前启动 | 可以先导入，再随下一稳定块到达重放，未封存时不发布 |
| 等待期间取消后恢复 | 已核验输入无需再次导入，可继续重放并封存 |

回归和构建结果，原始记录 `/tmp/usdb-p63-checks/checks.json`：

| 检查 | 结果 |
| --- | --- |
| balance-history 库功能回归 | 206通过、1忽略；排除2项大容量缓存压力测试 |
| usdb-indexer `assumeutxo_p5` 下游承诺 | 2通过 |
| workspace clippy，all targets/features，`-D warnings` | 通过 |
| workspace 文档构建 | 通过 |
| balance-history release binaries | 通过 |

复验命令（仓库根目录，构建并发按本机预算设置）：

```bash
CARGO_BUILD_JOBS=2 cargo test --offline --locked --manifest-path src/btc/Cargo.toml \
  -p balance-history --lib assumeutxo_local_loader -- --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --offline --locked --manifest-path src/btc/Cargo.toml \
  -p balance-history --lib -- --skip test_address_balance_cache_size \
  --skip test_utxo_cache_size --test-threads=1
```

另以真实 Core 31.1 和 release balance-history 进程完成渐进同步验证。
证据：`/tmp/usdb-p63-live-3g8h3j6c/result.json`；本轮临时节点和服务均已正常停止。

| Core active tip | balance-history 状态，B=101/G=115/lag=10 |
| --- | --- |
| 101 | 已导入101，等待后续稳定块，业务 RPC 未开放 |
| 112，尚未到 G | 已重放至102，等待，业务 RPC 未开放 |
| 124 | 已重放至114，等待 G 的稳定深度，业务 RPC 未开放 |
| 125 | 重放115并独立校验、封存，RPC 就绪 |

本次将阈值设为0以覆盖真实 blk 文件读取，102–115共14块全部本地命中、payload RPC 为0。
使用 Core 实际生成的 XOR/预分配文件；删除本次临时输入快照后，已封存服务可正常重启。
另建从创世块重放的实例，同高度完整 block-commit 和 state-ref 均一致。
C(115) 为 `de90370696a222bf48cc2327e82e3eb800c72ec4495dd8426064cfae7875c45f`。
服务二进制 SHA-256：`368f9af47aa19b34f8209835343456eecf19c9a77b7252a5623912184630952c`。
Core `getindexinfo={}`；小型进程验证耗时9.526秒。

这次渐进测试使用受控 regtest active tip，未重新执行主网 AssumeUTXO 双 chainstate 加载。
双文件并行追加由上述文件集成测试覆盖；P2 主网机制证据继续独立保留。
小型验证证明行为与正确性，不代表主网吞吐、命中率或整套 Ord/USDB 就绪时间。

## 4. 主网长任务与性能对照（待人工安排）

继续使用[P6.2操作步骤](./balance-history-assumeutxo-p62-operations.md#4-主网长任务准备独立目录与配置)，
每次新建独立 root，保持现有主网服务和对照状态不变，目标 Core 使用 `prune=0`。
该手册已经移除启动前必须到963810的要求；963810仍是本次 G 最终发布的最低 tip。

1. 构建当前源码，记录二进制 SHA-256；核对快照身份、Core RPC/cookie 及对应 blocks 目录。
   不输出 cookie 内容。记录 Core 前台 tip 与后台验证进度，二者分别观察。
2. 先按默认 `local_loader_threshold=500` 在独立目录执行 bootstrap，或直接启动正常服务让其自动初始化。
   不同时对同一 root 启动这两个命令。blocks 目录不可用时可正常走 RPC。
3. 记录输入扫描/导入、等待区块、重放、状态扫描、独立聚合、封存各阶段时间，以及内存、磁盘和本地/RPC计数。
   `waiting_for_blocks` 的时间应单列，不能计作纯重放吞吐。
4. 按 P6.2 第5步核对 G 的 UTXO/非零余额投影、C(B)、C(G) 与来源记录；继续完成服务启动及恢复检查。
5. 若安排性能对照，在另一个独立 root 使用相同输入/G/稳定深度和资源预算，
   将 `local_loader_threshold=4294967295` 关闭本地加速，串行完成纯 RPC 对照。
   记录 Core tip、冷热缓存和并发磁盘负载差异，比较完整 commit 与状态投影一致后再比较重放耗时。

本轮未启动以上主网长任务，不能预填收益或 ETA。两个完整实验 root 都需要预留导入、数据库、
独立余额聚合和 compaction 的空间；可分时安排，但不自动清理既有验证结果。
下一实现步骤为 P6.4 的 indexer 历史 prevout/reveal 查询与 readiness；P6.5/P7 继续负责整套验收和部署交付。
