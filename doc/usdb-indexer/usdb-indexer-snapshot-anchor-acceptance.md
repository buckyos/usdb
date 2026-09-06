# 历史 snapshot anchor 原子提交与恢复验收

## 契约

新块 H 的历史 anchor、pass block commit、业务 SQLite 状态、连续覆盖位置与同步高度必须在同一块事务
提交。RPC 只能发布已提交高度。历史覆盖必须从配置索引起点连续推进；仅存在 head anchor 不表示中间历史完整。
旧库存在缺口时，即使本地和上游高度相同，也必须保持 `consensus_ready=false` / `HistoryBackfillPending`。

连续覆盖位置使用 SQLite `state` 中的 `snapshot_history_start_height` 和 `snapshot_history_next_height`。
启动重算、正常提交、回填提交和 reorg 回退都维护这个位置。恢复操作不修改已提交业务高度；已有数据可直接升级，
不需要重新导入 snapshot 或重建 indexer。能量库继续遵循原有跨库一致性恢复流程。

## 验收用例

集成用例位于仓库根目录 `tests/indexer_snapshot_anchors.rs`，通过 indexer 的测试模块复用可注入上游
fixture，SQLite 与 RocksDB 使用真实临时数据库。共识入口用例位于 `service/server.rs`。

| 场景 | 必须满足的结果 |
| --- | --- |
| 新库正常扫描多块 | 每个高度都有匹配 pass commit 的历史 anchor；额外历史 state-ref 请求数为 0 |
| 中间块缺少上游 commit | 只保留前一块的高度与 anchor；重试后一起推进 |
| anchor 已写、同步高度写入失败 | 该块 anchor、pass commit、余额快照和覆盖位置全部回滚 |
| 写 savepoint 尚未提交 | 独立读取看不到新高度或新覆盖位置 |
| 提交前进程直接退出 | 重开数据库后只见前一块，不依赖 Rust Drop 执行回滚 |
| 提交后进程直接退出 | 重开数据库后高度、anchor 和业务状态一起保留 |
| 回填一个批次中后续行非法 | 整个批次及覆盖位置回滚 |
| 旧库回填 RPC 中途失败 | 已提交的 64 行批次保留，其余不计入就绪位置 |
| 失败后重启且上游不再出块 | 从第一个缺口继续，仅请求缺失行，完成后恢复就绪 |
| 返回另一条链的 commit 或错误高度 | 拒绝回填，保留 `HistoryBackfillPending` |
| 回填收到 shutdown | 不发布未完成状态，后续可恢复 |
| 旧 cursor 声称完整但历史有洞 | 启动从实际行重新定位第一个缺口 |
| reorg 回退再重放 | 删除旧分支后缀并回退覆盖位置；替换分支写入新的 anchor |
| 高度达到 u32 最大值 | 完整前缀可表示，不产生溢出或永久 pending |
| 双方高度相同但中间 anchor 缺失 | 普通查询可用，共识未就绪，严格历史查询返回 `SNAPSHOT_NOT_READY` |
| head 存在但缺口跨重启保留 | 重启后仍未就绪，不能凭 head 越过中间缺口 |

## 执行

从仓库根目录执行：

```bash
cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer snapshot_anchor_acceptance
cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer history_gap
cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer
bash src/btc/usdb-indexer/scripts/regtest_reorg_smoke.sh
```

`crash_child` 是显式 ignored 的子进程入口，由父验收用例在独立临时目录中执行两次；它不代表崩溃场景被跳过。
全量测试同时覆盖原有能量恢复、同高度 reorg、回滚后重启和严格历史查询。真实 regtest 使用独立数据目录，
覆盖 Bitcoin → balance-history → indexer 的追平、回退和替换分支继续索引，不连接主网数据目录。

上述用例验证逻辑和进程退出恢复；不等价于物理断电或文件系统损坏测试。

## 本次验证结果（2026-09-06）

- Indexer 全量测试：317 项通过、0 失败、7 个独立入口 ignored；其中崩溃子进程入口已由父用例实际执行。
- 独立 regtest：同步至 40，回退至 29，替代分支同步至 40，重启后继续至 41，共识就绪恢复且 reorg epoch 保留。
- 停止测试进程后核对落盘数据：1–41 的 anchor 连续完整，全部匹配对应 pass block commit；覆盖游标为 42，SQLite 完整性检查通过。
