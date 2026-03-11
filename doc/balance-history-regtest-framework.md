# Balance-History Regtest Framework 说明

本文档说明 `balance-history` 当前的 regtest 脚本框架，目标是让后续 smoke、reorg、范围查询、恢复场景都复用同一套基础设施，而不是复制整段脚本。

## 入口文件

- 共享库：[src/btc/balance-history/scripts/regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)
- 基础 smoke 场景：[src/btc/balance-history/scripts/regtest_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_smoke.sh)
- reorg smoke 场景：[src/btc/balance-history/scripts/regtest_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_reorg_smoke.sh)
- 多次 reorg smoke 场景：[src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh)
- 深回滚 reorg smoke 场景：[src/btc/balance-history/scripts/regtest_deep_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_deep_reorg_smoke.sh)
- 重启后 reorg smoke 场景：[src/btc/balance-history/scripts/regtest_restart_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_restart_reorg_smoke.sh)
- 重启后多轮 reorg smoke 场景：[src/btc/balance-history/scripts/regtest_restart_multi_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_restart_multi_reorg_smoke.sh)
- 重启后混合 reorg smoke 场景：[src/btc/balance-history/scripts/regtest_restart_hybrid_reorg_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_restart_hybrid_reorg_smoke.sh)
- 历史余额 oracle 场景：[src/btc/balance-history/scripts/regtest_history_balance_oracle.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_history_balance_oracle.sh)
- RPC 语义专项场景：[src/btc/balance-history/scripts/regtest_rpc_semantics.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_rpc_semantics.sh)
- Snapshot 生成/恢复场景：[src/btc/balance-history/scripts/regtest_snapshot_recovery.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_snapshot_recovery.sh)

## 设计目标

1. 每个测试实例拥有独立的工作目录、Bitcoin datadir、balance-history root-dir。
2. 每个测试实例显式配置独立的 BTC RPC 端口、BTC P2P 端口、balance-history RPC 端口。
3. 场景脚本只保留“业务步骤”和断言，把生命周期与通用等待逻辑下沉到共享库。
4. 新场景默认可以并行运行，只要调用者为各实例分配不冲突的端口。

## 共享库提供的能力

`regtest_lib.sh` 当前封装了以下能力：

1. Bitcoin Core 二进制解析：优先 `BITCOIN_BIN_DIR`，回退到 `PATH`。
2. 工作目录初始化：`WORK_DIR`、`BITCOIN_DIR`、`BALANCE_HISTORY_ROOT`。
3. bitcoind 生命周期：启动、停止、PID 探测。
4. bitcoind 端口隔离：同时设置 `-rpcport` 与 `-port`。
5. 钱包初始化：创建或加载指定 `WALLET_NAME`。
6. balance-history 配置生成、服务启动与优雅停止。
7. balance-history JSON-RPC 调用与等待同步辅助函数。
8. 常见辅助逻辑：金额转 sat、地址转 script hash、地址余额断言、UTXO 断言、等待 block commit hash 收敛。
9. 成熟资金预热：自动补足可花费的 coinbase 区块，避免转账前资金未成熟。
10. 空替代块辅助：在需要时通过 `generateblock` 显式挖不包含 mempool 交易的替代块。
11. 失败诊断输出：测试失败时自动打印 balance-history 与 bitcoind 日志尾部。
12. 服务重启辅助：停止并重启 balance-history，然后等待 RPC 再次 ready。
13. 历史对拍辅助：按高度读取完整区块 JSON，配合独立 Python oracle 校验 `(address, height)` 余额。
14. CLI 复用辅助：可在自定义 `root_dir` 下直接调用 `balance-history` 的 snapshot 子命令，避免脚本重复拼接 `cargo run`。

## 关闭与查询约束

1. `regtest_stop_balance_history` 默认先调用 `stop` RPC，再等待子进程退出；只有超时后才回退到 `kill -9`。
2. 后台 `cargo run` 进程在服务已退出后可能短暂进入 zombie 状态，脚本需要显式 `wait` 回收，不能只靠 `kill -0` 判断。
3. `get_live_utxo` 的 JSON-RPC 参数必须使用 rust-bitcoin 的 human-readable `OutPoint` 形式，也就是单个字符串 `"txid:vout"`，不能发送 map 结构。
4. `get_live_utxo` 只查询 balance-history 当前 DB 中的 live UTXO 视图；它不会像内部索引链路那样在 miss 时回退到 bitcoind 拉取历史交易输出。

## 场景脚本的最小模式

新的场景脚本建议维持如下结构：

```bash
#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  # 场景自己的区块构造、服务启动、断言逻辑
}

main "$@"
```

## 推荐端口策略

为避免多实例冲突，建议一组场景使用连续端口：

1. `BTC_RPC_PORT = base`
2. `BTC_P2P_PORT = base + 1`
3. `BH_RPC_PORT = base + 10`

例如：

1. smoke：`28132 / 28133 / 28110`
2. reorg：`28232 / 28233 / 28210`
3. 新场景可继续用 `28332 / 28333 / 28310`

## 如何新增一个场景

1. 新建一个薄入口脚本，只定义该场景需要的默认端口、目标高度和日志前缀。
2. `source regtest_lib.sh`。
3. 复用共享初始化逻辑启动 bitcoind 和 balance-history。
4. 只编写该场景独有的链操作与 RPC 断言。
5. 为该场景新增一份文档，明确目标、环境变量和验收条件。

## 目前边界

当前框架仍然是 shell 级别的轻量封装，还没有像 `usdb-indexer/scripts/regtest_scenario_runner.py` 那样抽象出统一的声明式 scenario runner。现阶段这样做的原因是：

1. `balance-history` 当前最紧急的是把真实节点下的端到端验证铺开。
2. smoke 与 reorg 两类场景在生命周期上高度相似，抽共享库已经能显著减少重复。
3. 当前已经扩展到 multi-reorg 场景，说明共享库路线可持续；如果场景数量继续增多，再演进成 Python runner 会更稳妥。

## 后续扩展方向

1. 增加多地址、多交易图、范围查询一致性场景。
2. 覆盖更复杂的 UTXO 花费图，而不只是单输出转账。
3. 覆盖 snapshot 生成、安装和恢复后继续追块的一致性场景。
4. 覆盖服务离线更久、跨多高度组合回滚后再恢复的场景。
5. 视复杂度再决定是否引入 Python 场景 runner。