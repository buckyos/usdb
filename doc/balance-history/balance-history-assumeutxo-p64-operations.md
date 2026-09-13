# P6.4：indexer 历史输入、reveal 查询与 readiness

日期：2026-09-12。P6.3 已提交为 `8103550`；P6.4 已提交为 `7f52ee2`，本节记录其实现及验证。
后续独立Core/BH/indexer整套服务已在[P6.5验收](./balance-history-assumeutxo-p65-operations.md)通过；本节保留P6.4自身的验收边界。

## 1. 查询来源与正确性边界

原实现有两处隐含 txindex 依赖：

- `UTXOValueManager` 经 `BTCRpcClient.get_utxo` 调用未指定 block hash 的 `getrawtransaction`，读取历史输入金额。
- `InscriptionTransferTracker.calc_create_satpoint` 同样按 txid 单独获取 reveal 交易。

本批移除 indexer 的这两处调用。reveal 必须存在于当前处理的区块，直接从其交易集合定位；
需要累计前序 vin 金额时，按该块 hash 调用 `getblock(hash, 3)`，读取 Core undo 中的 prevout。
[Core RPC 文档](https://bitcoincore.org/en/doc/31.0.0/rpc/blockchain/getblock/)说明 prevout 仅在相应 undo 可用时返回；
[Core 31.1 实现](https://github.com/bitcoin/bitcoin/blob/v31.1/src/rpc/blockchain.cpp)在 `blockToJSON` 读取块对应的 `CBlockUndo`。

这条路径不使用当前 `gettxout`，不依赖 balance-history 有限的 undo 窗口，也不需要任意历史 txid 的全局定位。
即使 indexer 明显落后、创建输入的交易早于 B，或者相应输出现在已经花费，只要**消费块**的 Core block/undo 仍在，就能读取当时的金额。
同块先创建后花费的输出也包含在该块 undo 中。节点继续采用 `prune=0`，完整保留块及 undo。

每个处理块只保留一个惰性输入上下文，供该块的 mint、invalid mint 和 transfer 共用。
不需要前序输入金额时不读取 verbosity-3；首次读取必须整份核验成功后才缓存，失败可重试，换块/恢复时替换或清空。
核验包括请求高度/hash、active-chain confirmations、交易原始字节及顺序、vin 对应 outpoint、
金额精度/范围/总量与重复输入；读取前后再次核对 canonical hash。重组后不能复用另一块的金额。

缺少 reveal、undo、完整 prevout 或发生链身份不匹配时明确失败，不推进该块的持久化同步高度。
不会用零金额、当前 UTXO 或无 block hash 的 `getrawtransaction` 补齐未知数据。
原有 satpoint 计算和 pass/local/system commit 算法不变，不增加数据库表或改动 v1 commit 协议。
`usdb-util` 的旧通用交易查询接口仍为其他调用方保留，不能据此宣称整个仓库不再使用 txindex。

### G=B 的特殊情况

`G>=B` 的配置约束保持不变。但 UTXO 快照是 B 执行后的状态，不包含 B 本身的完整交易或 undo。
Core 的快照链前台验证产生的是 B+1 以后的 undo。若 indexer 从 G=B 开始处理，而 B 的块文件尚未到达，
或其中的业务事件需要缺失的历史输入金额，仍会等待/报错，直到 Core 的该块数据可用。
不能从快照恢复已在 B 花费的输入来绕过这个边界，也不跳过 G 的业务事件。
当前实验 `G=963800>B=935000`，消费块处在快照前台验证区间，符合快速启动路径。

## 2. readiness 与启动编排

服务的 `get_readiness` 新增：

- `block_processing_pending_height`：当前未提交或最近失败的块高度。
- `BlockProcessingPending` blocker：该字段非空时 `consensus_ready=false`。

标记在开始处理块时设置，在该块 SQLite savepoint 和 synced height 完整提交后清除；
失败后保持到成功重试，或完成链/持久状态核对、确认已无待处理块后清除。
普通查询仍可读取此前已提交的状态。这个标记不只表示 Core 缺数据，也包含其他导致块未提交的错误；
错误原因通过原有同步 message/log 给出。重启仍由持久化高度、能量恢复、历史锚点和 reorg 标记决定恢复路径。
上游 BH 就绪、历史覆盖、同高度锚点和 local/system commit 等既有门槛继续生效。

新增独立预检工具 [check_bitcoin_block_data.py](../../docker/scripts/tools/check_bitcoin_block_data.py)，
检查一个明确 canonical 块的完整历史输入能力，单独输出 Core 前台信息与 `getchainstates`，
不要求 IBD=false、headers 追平、后台验证完成或 txindex 就绪。
结果 `scope=one_block_historical_inputs`、`ready=true` **仅表示这个块的数据可用**，不能替代整段覆盖或 USDB consensus readiness。
无非 coinbase 输入的块只能证明块可读取，不能证明其他块的 undo 可用。

已有 `check_bitcoin_readiness.py` 的默认完整节点合同和部署调用保持原样；
P7 再将新查询能力与 node-kit/controller 的启动条件配套集成，不直接删除旧 txindex gate。
快速启动还须选用现有 `usdb.inscription_source="bitcoind"`；继续使用外部 Ord 源或启用 Ord shadow 比较时，
其历史索引和服务可用性仍是独立依赖，不能由 P6.4 的输入查询结果代替。

## 3. 验证证据

[Rust 专项](../../tests/assumeutxo_indexer_inputs.rs)的5项常规测试通过：

| 场景 | 结果 |
| --- | --- |
| 第二个 vin 的 reveal、同块创建并转移、历史输入已花费 | 正确 satpoint/owner，单块只取一次 verbosity-3，无 getrawtransaction/gettxout 调用 |
| 缺少一项 undo 后修复并重试 | 整份响应失败，不缓存部分金额；重试正确 |
| 错误 hash/高度/分支、交易内容或 vin 顺序、非法金额 | 拒绝处理 |
| 读取中重组、同高度替代块、缺失 reveal | 拒绝旧上下文并按新块重新查询 |
| 零金额输入及 fee-loss | 零值保留，既有 satpoint 规则保持 |

真实 Core 31.1 集成测试由[独立运行器](../../tests/run_assumeutxo_p64_live.py)执行：

1. 全新 regtest、`txindex=0/prune=0`；在101块创建 Taproot inscription commit 和前序输入。
2. 在102块包含两输入 reveal 及其同块后续转移，均由 Core 实际验证。
3. Core 再生成2050块至2152，确认未指定块的 `getrawtransaction` 失败、旧输入 `gettxout=null`。
4. 生产 tracker/RPC 代码仍从102块 undo 正确恢复输入金额：reveal 得到 `vout=1/offset=500`，转移得到 `vout=1/offset=300`。
5. 重新创建 tracker/输入上下文再计算，结果相同；新预检读取到4项完整 prevout。

结果位于 `/tmp/usdb-p64-live-k0o_5qmp/result.json`，全部通过，临时 Core 已正常停止。
含构建的运行器耗时6.564秒；Rust 内的真实 Core 操作约0.67秒，不代表主网吞吐。
这是生产查询/转移组件与真实 Core 的集成验证，未运行完整 indexer/BH/Ord 服务链路，
也未重新导入主网 AssumeUTXO 双 chainstate；完整部署、服务重启及端到端承诺对拍属于 P6.5/P7。

回归证据位于 `/tmp/usdb-p64-checks/`：

- 完整 indexer：326通过、8忽略，其中真实 Core 专项另行显式执行并通过。
- usdb-util：61通过、2忽略。
- 新预检：4项通过；原 Bitcoin readiness 回归17项通过。
- workspace clippy（all targets/features、`-D warnings`）、文档构建及 release 构建均通过，结果归档于 `checks.json`。

复验命令（仓库根目录）：

```bash
CARGO_BUILD_JOBS=2 cargo test --offline --locked --manifest-path src/btc/Cargo.toml \
  -p usdb-indexer --bin usdb-indexer assumeutxo_indexer_inputs -- --test-threads=1
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p test_bitcoin_block_data.py -v
PYTHONDONTWRITEBYTECODE=1 python3 tests/run_assumeutxo_p64_live.py \
  --bitcoind /data/btc/bitcoin-31.1/bin/bitcoind
```

运行器只允许它新建的临时空 regtest，保留日志/结果目录，结束时停止该节点；不启动或修改主网节点。

## 4. 主网预检与后续工作

本轮尝试主网只读预检时，既定 `/home/bucky/.bitcoin/.cookie` 不存在，进程检查未发现正在运行的 bitcoind，
因此未执行主网 RPC 探测，也未自行重启原节点。待操作人员安排供块节点后可执行：

```bash
cd /home/bucky/work/usdb
PYTHONDONTWRITEBYTECODE=1 python3 docker/scripts/tools/check_bitcoin_block_data.py \
  --url http://127.0.0.1:8332 --cookie-file /home/bucky/.bitcoin/.cookie \
  --expected-chain main --height 963800 \
  --block-hash 000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551
```

先核对实际 RPC/cookie 路径。成功退出0，失败退出1；保留 JSON 中的 blockers、前台和 chainstates 信息。
若数据未到达，等待该块与 undo 可用后重试；不删除快照、重导入 BH 或把输入缺失解释为余额为零。

P6.4 代码与小规模/真实 Core 组件验收已完成。下一步 P6.5 需要：

- 独立节点上运行原生 BH 与 bitcoind inscription source 的完整 indexer，从 G 开始追平并核对同高度 pass/local/system commitments。
- 验证真实服务中断、重组、缺块/缺 undo 恢复及 readiness，不以本节重新建立输入上下文代替完整服务恢复验收。
- 安排主网相同锚点的原生重导入、历史查询和性能对照；持续保留 prune=0 与 Core 官方后台验证。
- P7 再切换镜像、安装/升级、默认来源及启动 gate，并更新发布身份。
