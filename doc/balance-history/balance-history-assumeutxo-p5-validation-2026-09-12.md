# AssumeUTXO P5 语义验收记录

日期：2026-09-12。结论：**P5 的离线状态/查询语义与独立 regtest 恢复验收通过。**

P4 已证明主网 963800 的全量 UTXO、非零余额和逐块 commit 一致；本轮补齐查询、registry、
状态引用、下游承诺派生与分叉恢复，并修复两个来源识别问题。整套 Ord 在线重放、节点 readiness、
小锚点发布及部署切换仍属于 P6/P7。本轮没有启动主网服务、推进实验库高度或等待 Core 后台历史验证。

## 1. 验收矩阵

| 项目 | 证据及范围 | 结果 |
| --- | --- | --- |
| 主网全量状态 | 沿用 P4 的 165,748,439 个 UTXO、59,356,343 个非零余额全量有序投影；既有报告 hash 未变 | 通过，未重跑全量 compare |
| 主网 commit | 本轮重新核对 935000–963800 的 28,801 条完整 commit 记录 | 通过 |
| 主网 state-ref | 963800 的 BTC hash、logical commit、snapshot ID 与参考 commit/元数据一致 | 通过 |
| 主网查询与 registry | SHA-256 首字节 256 个边界各取后续 4 个余额键，共 1,024 个去重样本；点查、范围 delta 收支核对、summary 和脚本反查 | 通过，属于采样 |
| 查询语义 | 真实 regtest 区块的所有出现过的脚本及未知键；点查、精确 delta、范围、summary、timeseries、flow buckets、批查顺序及重复项 | 通过 |
| 下界及异常 | 基线前点查/state-ref、基线 delta、跨基线历史请求均拒绝；未来高度及不匹配 state-ref 返回结构化错误 | 通过 |
| 历史脚本覆盖 | 基线前已经花完且没有活输出的脚本返回 unresolved；基线活输出和后续发现的脚本可解析 | 通过 |
| 下游 pass/local/system | 独立重放与导入重放产生的真实 BH RPC 输出，输入生产 pass collector、indexer 存储及 local/system RPC；三高度比较及错误输入反例 | 通过，承诺链路测试 |
| 恢复与 reorg | 导入检查点续传、错误输入不发布、重复启动、回滚后关闭/重新打开、真实替代分支、跨基线回滚拒绝 | 通过，独立 regtest |
| 整套在线节点 | 真实 Ord 事件到 pass 状态的端到端重放、HTTP/网络、消费者落后、Core 后台验证与 txindex readiness | 留待 P6/P7 |

下游测试使用确定的非空 pass mutation 场景，调用生产承诺、持久化和 RPC 实现；它不生成真实 Ord 铭文。
其输入文件 `tests/fixtures/assumeutxo-p5/downstream-inputs.json` 每次都由 BH 测试重新计算并核对，
不是仅将相同字符串送入 hash 函数。反例保持 snapshot ID 不变：改错一个 BH commit 后，
本块及后续 pass/local/system 承诺改变；改错 active balance 后，local/system 改变。

## 2. 本轮修复

### 2.1 AssumeUTXO 库被误报为完整历史 registry

原 resolver 仅检查旧 core snapshot 的安装来源。AssumeUTXO 使用独立导入标记，
因此会误入 `full_replay` 分支，使更早已花完脚本的 miss 被报告为 `not_found`。
验收用例先复现了错误，修复后通过。

现在识别 AssumeUTXO 导入标记，沿用既有 RPC 契约：

- `coverage_mode=post_snapshot_only`，`script_registry_complete_coverage=false`；
- `base_height/base_block_hash` 指向 Core 快照基线；
- 找到的基线活 UTXO/后续脚本返回 `found_overlay`，miss 返回 `unresolved`；
- 未完成或无法读取导入来源时，不宣称完整历史覆盖；不伪造旧 core/sidecar artifact 身份。

没有增加 wire enum、变更 commit 算法或提升共识语义版本。

### 2.2 旧快照导出入口未拒绝 AssumeUTXO 来源

原导出门禁同样只识别旧安装来源，不能排除缺少基线前 commit/完整脚本历史的 AssumeUTXO 库。
现在在创建任何导出文件前拒绝该来源，防止按现有完整重放 artifact 契约发布不完整数据。
独立用例确认返回明确错误且没有创建输出文件；未来小锚点或新的 artifact 设计另行处理。

## 3. 主网只读核验

实验目录：`/data/usdb-assumeutxo-validation/p4-mainnet-935000-to-963800`。
新命令：`balance-history-assumeutxo-tool audit --samples-per-prefix 4`。

本轮重新计算 37,006,364,672 字节参考 SQLite 的 SHA-256，约 25.4 秒。
数据库以只读模式打开。RPC handler 在进程内使用明确的离线状态上下文运行，没有监听端口，
没有改变网络冻结的 stable lag（仍为 10），不能据此判断在线服务或消费者已经就绪。

| 字段 | 结果 |
| --- | --- |
| 目标高度 | 963800 |
| BTC hash | `000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551` |
| logical commit | `d5981b06db4e1f9d12f3a9e8b66da45001d0b38d3e2a689de3251542922f1e75` |
| snapshot ID | `7b1cf69a6922b55a90f378c3d03daa8e64c4dba48145cae6e0989be5c9730562` |
| 参考 core SHA-256 | `3e3490ac19521647a8513a0ef2961607df4456ba3e3ea6922c1ba750f7fbea61` |
| point/state-ref 下界 | 935000 |
| delta/history 下界 | 935001 |
| 余额/registry 样本 | 1,024 |
| 基线后未变动样本 | 894，余额一致；导入记录高度为935000，delta为0 |
| 基线后变动样本 | 130，末次变动高度、余额及 delta 均与参考一致 |
| 无法导出地址的脚本样本 | 40，按 RPC 的 `standard=false` 口径统计，反查原始脚本及 hash 均通过 |
| wrapper 退出码 | 0 |
| wrapper 总耗时 | 26.225 秒 |
| 子进程最大 RSS | 284,568 KiB，约278 MiB |

基线 delta=0 表示导入的余额断面，不表示真实的935000区块 delta为0；
精确 delta 查询因此从935001开始。基线前最后变动高度/delta无法从活UTXO恢复，这是明确保留的边界。

核验前后对比 `state/` 下531个文件的大小、mtime_ns及inode，全部一致；数据库高度仍为963800。
此次只重算参考文件 hash、逐块 commit 和采样查询，未重新导入、重放或扫描全量状态。

## 4. regtest 与自动化检查

fixture 由独立 Bitcoin Core 31.1 临时 regtest 节点实际生成，使用 `listen=0/connect=0`；生成后停止并清理临时节点。
基线101，403个UTXO；原分支至103、替代分支至104、跨基线分支至105。

- 长期未动脚本：`5151`；基线前已花完脚本：`5161`；后续新脚本：`517551`。
- 包含零金额输出、空脚本、不可作为地址的脚本、基线旧输出后续花费、同块父子交易和花至零余额。
- 正常分叉先明确回滚至101、关闭并重新打开数据库，再重放替代102–104；全部 UTXO、余额、commit及state-ref与完整替代分支一致。
- 回滚至100被拒绝且不改变状态；直接拼接不同分支或跨基线分支被拒绝。本轮没有声称实现自动深 reorg 重建。

| 检查 | 结果 |
| --- | --- |
| balance-history `assumeutxo` | 10通过，包括原P4用例及4项P5用例 |
| balance-history `script_registry` | 24通过，覆盖既有完整重放/sidecar/客户端契约 |
| usdb-indexer `assumeutxo_p5` | 1通过，内部包含两种正常来源及两类错误输入 |
| usdb-indexer `get_state_ref_at_height` | 10通过 |
| workspace `check --all-targets` | 通过 |
| workspace `clippy --all-targets -- -D warnings` | 通过 |
| `cargo doc -p balance-history --no-deps` | 通过 |
| workspace `cargo fmt --check` 及下游 include 测试文件 `rustfmt --check` | 通过 |

新增跨模块用例位于 `tests/assumeutxo_semantics.rs`、`tests/assumeutxo_downstream.rs`，
共享辅助位于 `tests/common/assumeutxo.rs`；生产启动流程未调用离线实验工具。

## 5. 证据与复验操作

实验目录内：

- `p5-semantics-result.json`：全部1,024个样本和本轮检查结果；SHA-256 `5f26300177a309737f75e5a552db5d56482131b5a4e455c99c7047914cbe771d`。
- `p5-audit-runtime.json`：命令、UTC时间、退出码、计时、RSS及二进制hash。
- `p5-checks/`：测试/静态检查日志、文件清单与验收执行摘要。
- 同级 `p4-mainnet-935000-to-963800.p5-audit-20260912.log`：主网核验原始日志。

执行二进制 SHA-256：`59a3680f02909b962b48bf8aea046018a8eb59458e4fa443cd4b45a7df609036`。
P4全量比较报告hash仍为 `69fcfdd4a3e3a25f64adac1bd4ed89228e4955c782d1f3f629a33aa6b5327388`。
首次使用 `/usr/bin/time` 的包装命令因本机未安装该程序而退出127，审计工具没有启动；失败日志保留在同级 `.p5-audit.log`。
后续改用 Python 的 monotonic/resource 记录，以上计时及通过结论来自实际运行成功的命令。

日常复验不需要重新生成 Core fixture，不需要比特币 RPC/cookie：

```bash
cd /home/bucky/work/usdb
CARGO_BUILD_JOBS=2 cargo test --offline --locked --manifest-path src/btc/Cargo.toml -p balance-history --lib assumeutxo -- --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --offline --locked --manifest-path src/btc/Cargo.toml -p usdb-indexer assumeutxo_p5 -- --test-threads=1
```

需要再次核验主网现有结果时，可单独安排下列命令。本轮构建约20秒、audit约26秒；
后续耗时取决于磁盘缓存和负载，audit仍会完整读取37GB参考文件：

```bash
CARGO_BUILD_JOBS=2 cargo build --offline --locked --release --manifest-path src/btc/Cargo.toml -p balance-history --bin balance-history-assumeutxo-tool
set -o pipefail
/home/bucky/work/usdb/src/btc/target/release/balance-history-assumeutxo-tool \
  --root-dir /data/usdb-assumeutxo-validation/p4-mainnet-935000-to-963800 \
  audit --samples-per-prefix 4
```

仅在维护测试输入时重新生成真实 Core fixture；这会替换仓库中的测试文件，随后必须同步生成下游输入：

```bash
python3 tests/common/generate_assumeutxo_fixture.py --bitcoind /data/btc/bitcoin-31.1/bin/bitcoind --p5 --output tests/fixtures/assumeutxo-p5
USDB_P5_UPDATE_FIXTURE=1 CARGO_BUILD_JOBS=2 cargo test --offline --locked --manifest-path src/btc/Cargo.toml -p balance-history --lib assumeutxo_p5_read_only_audit -- --test-threads=1
```

## 6. P6 的起点

可继续设计和实现生产 bootstrap：使用独立小锚点承载 USDB935000 commit，消除部署时对37GB旧core参考的依赖；
再处理 indexer 历史 prevout/reveal 查询、消费者落后与重启、txindex/后台验证/readiness的阶段划分。
之后安排独立新节点和实际 Ord/pass 输入的完整端到端验证，最后接入镜像、安装器和升级恢复。
保留官方后台验证及 `prune=0`。P5通过不代表上述生产替换已经完成。
