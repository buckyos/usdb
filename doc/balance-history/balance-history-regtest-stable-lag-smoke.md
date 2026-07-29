# Balance-History Regtest Stable Lag Smoke 测试说明

本文档说明 `balance-history` 的 `stable_lag` 专项 smoke 场景，目标是验证：

1. `stable_lag` 来自 BTC network registry scope，不是本地运行时配置；
2. `stable_lag` 不只是 RPC 元字段，而是真的参与索引推进上限计算；
3. 当 BTC tip 继续前进时，`get_block_height` / `get_snapshot_info().stable_height` 始终等于 `tip - stable_lag`；
4. `get_snapshot_info().stable_block_hash` 始终对应 `tip - stable_lag` 的 canonical block hash；
5. `tip < stable_lag` 时 stable height 饱和为 `0`，snapshot/state-ref 共识查询返回 `SNAPSHOT_NOT_READY`；
6. 同一数据目录重启后，snapshot 与历史 state-ref identity 不变；
7. lag 窗口内的 BTC branch replacement 不改变已暴露 stable snapshot，替代分支越过 stable frontier 后才进入新 snapshot；
8. stable head 前进后，旧高度的历史 state-ref 仍可精确重放。

脚本位置：

- [regtest_stable_lag_smoke.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_stable_lag_smoke.sh)
- [regtest_lib.sh](/home/bucky/work/usdb/src/btc/balance-history/scripts/regtest_lib.sh)
- [balance-history-regtest-framework.md](/home/bucky/work/usdb/doc/balance-history/balance-history-regtest-framework.md)

## 前置条件

1. 已安装并可执行：
   - `bitcoind`
   - `bitcoin-cli`
   - `cargo`
   - `curl`
   - `python3`
2. 当前仓库可正常构建 `balance-history`：
   - `cargo check --manifest-path src/btc/Cargo.toml -p balance-history`

## 一键运行

在仓库根目录执行：

```bash
src/btc/balance-history/scripts/regtest_stable_lag_smoke.sh
```

默认参数下，脚本会：

1. 先挖到 BTC tip `3`，保持 `tip < stable_lag=5`；
2. 启动真实 `balance-history`，断言 stable height 为 `0`，snapshot/state-ref 返回 `SNAPSHOT_NOT_READY`；
3. 在 tip 仍为 `3` 时重启，再次验证相同 fail-closed 状态；
4. 继续挖到 BTC tip `20`，验证服务追块并收敛到 stable height `15`；
5. 断言 `get_snapshot_info().stable_lag == 5`，保存 snapshot 与 height `15` state-ref；
6. 干净重启同一数据目录，逐字段比较 snapshot/state-ref；
7. 停止服务并替换 stable frontier 之上的最后 `3` 个 BTC blocks；
8. 重启后验证 stable snapshot/state-ref identity 未改变；
9. 再继续挖 `3` 个块，使替代分支进入 stable view；
10. 验证 stable height 收敛到 `18`，同时 height `15` 的历史 state-ref 仍可精确重放。

成功标志：

1. `get_block_height == get_snapshot_info().stable_height`
2. `get_snapshot_info().stable_lag` 与实际索引行为一致
3. `get_snapshot_info().stable_block_hash == getblockhash(tip - get_snapshot_info().stable_lag)`
4. `tip < stable_lag` 时 `get_snapshot_info` 和 `get_state_ref_at_height` 均返回 `SNAPSHOT_NOT_READY (-32041)`
5. clean restart、lag-window replacement 和 stable-head advance 后，相应历史 identity 比较通过
6. 输出 `Stable lag smoke test succeeded.`

## 可调参数（环境变量）

1. `WORK_DIR`：工作目录（默认自动创建临时目录）
2. `BITCOIN_BIN_DIR`：Bitcoin Core 二进制目录（默认 `/home/bucky/btc/bitcoin-28.1/bin`）
3. `BTC_RPC_PORT`：bitcoind RPC 端口（默认 `29832`）
4. `BTC_P2P_PORT`：bitcoind P2P 端口（默认 `29833`）
5. `BH_RPC_PORT`：balance-history RPC 端口（默认 `29810`）
6. `WALLET_NAME`：regtest 钱包名（默认 `bhstablelag`）
7. `PRE_LAG_TIP_HEIGHT`：首次启动时的 BTC tip，必须小于 lag（默认 `3`）
8. `TARGET_TIP_HEIGHT`：追块后的 BTC tip 高度（默认 `20`）
9. `EXTRA_BLOCKS`：初始断言后追加挖的区块数（默认 `3`）
10. `EXPECTED_STABLE_LAG`：期望的 registry lag（默认 `5`）
11. `REORG_DEPTH`：lag 窗口内替换的 BTC block 数，必须小于 lag（默认 `3`）
12. `SYNC_TIMEOUT_SEC`：等待稳定高度追平的超时秒数（默认 `120`）

示例：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
BTC_RPC_PORT=29832 \
BTC_P2P_PORT=29833 \
BH_RPC_PORT=29810 \
TARGET_TIP_HEIGHT=30 \
EXTRA_BLOCKS=5 \
src/btc/balance-history/scripts/regtest_stable_lag_smoke.sh
```

## 验收重点

1. `balance-history` 本地 DB 高度本身就是 stable height，而不是先追 tip 再在 RPC 层做减法。
2. `stable_lag` 进入 `SnapshotInfo` 后，元信息与实际索引行为保持一致。
3. `stable_lag` 由当前 BTC registry identity 承诺，不依赖本地配置文件。
4. 相同的 canonical tip 和相同的 `stable_lag` 必须导出相同的 stable snapshot identity。
5. `usdb-indexer` 通过 Rust 服务测试覆盖同一 DB 重启后的 snapshot/profile/candidate cursor 重放。
6. `usdb-indexer` 在当前 snapshot 接入、历史 state-ref 回填以及 current/historical RPC 读取入口都将 lag 与本地 embedded registry 比较，不一致返回错误且不暴露经济视图。
7. Go verifier 再次将 profile `external_state.stable_lag` 与 chain config 绑定的 registry golden 比较，形成独立的下游 fail-closed 检查。
8. 本测试只验证 lag 窗口内 replacement 与越过 frontier 后的正常推进，不宣称解决深层 BTC reorg 后既有 USDB selector 的 archive/rewind。

## 2026-07-29 执行结果

- 默认参数的隔离 regtest 矩阵通过：
  - `tip=3` 时 stable height 为 `0`，snapshot/state-ref 均为 `SNAPSHOT_NOT_READY`；
  - below-lag restart 后状态不变；
  - `tip=20` 时 stable height 为 `15`；
  - clean restart 和 depth-3 lag-window replacement 后 snapshot/state-ref identity 不变；
  - `tip=23` 时 stable height 为 `18`，height `15` state-ref 重放不变。
- Rust 定向测试通过：
  - stable target 的 `lag-1 / lag / lag+1` 边界；
  - 当前、持久化 current、持久化 historical 三类 lag mismatch；
  - 同一 DB 重启后的 snapshot/profile/candidate 首页面与 cursor 续页。
- 本轮使用独立临时 regtest datadir 和端口，没有访问或修改本机 BTC mainnet 服务。
