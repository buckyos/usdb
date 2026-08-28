# Balance-History Stable-Lag Reorg 深度边界矩阵

本文档定义 `stable_lag=10` 下 BTC reorg 深度边界的 deterministic regtest。该矩阵验证
`balance-history` 对 lag 窗口内分叉和首次越过 stable frontier 的分叉能够按统一规则处理，
并使不同启动模式最终收敛到相同状态。

## 测试入口

从仓库根目录执行：

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh stable-lag-reorg
```

直接入口：

```bash
bash src/btc/balance-history/scripts/regtest_stable_lag_reorg_depth_matrix.sh
```

单独复现一个深度：

```bash
EXPECTED_STABLE_LAG=10 REORG_DEPTH=11 \
bash src/btc/balance-history/scripts/regtest_stable_lag_reorg_depth_case.sh
```

## 链构造

每个深度使用独立 bitcoind datadir、钱包、端口和三套 balance-history DB：

1. 挖到 coinbase 可花费高度，再追加固定 prefix。
2. 向测试地址转入 `1.25 BTC`，并把该交易挖在 stable frontier 高度 `H`。
3. 再挖 `stable_lag=10` 个区块，使 BTC tip 固定为 `H+10`。
4. online 和 offline 两个实例先同步到 `H`，交叉比较初始状态。
5. 停止 offline 实例，在 BTC tip 高度不变的条件下构造 replacement branch。
6. 依次验证仍在线的实例、已有 DB 的 offline restart，以及空 DB 的 fresh joiner。

替代分支使用新的 coinbase 地址并显式挖空块。这样既不会把回到 mempool 的 tracked
transaction 重新打包，也不会因空块内容完全相同而命中 Bitcoin Core duplicate block。

## 深度边界

| Reorg depth | 替换区间起点 | Stable block `H` | 预期余额 |
| --- | --- | --- | --- |
| `lag - 1 = 9` | `H + 2` | 不变 | `125000000 sat` |
| `lag = 10` | `H + 1` | 不变 | `125000000 sat` |
| `lag + 1 = 11` | `H` | 被替换 | `0 sat` |

前两种场景只替换 lag 窗口内尚未进入稳定视图的区块，因此 snapshot、historical
state-ref、block commit 和 tracked balance 都必须保持不变。

`lag+1` 首次越过 stable frontier。online 实例必须检测同高度 hash 变化并回滚/重放；
offline restart 必须在启动时修复已有 DB；fresh joiner 必须从 replacement canonical
chain 重建。三者最终结果必须完全一致。

## 交叉断言

每种启动模式都在同一个 stable height 上读取并 canonicalize：

- `get_snapshot_info`
- `get_state_ref_at_height`
- `get_block_commit`
- `get_address_balance`

矩阵要求：

1. reorg 前 online/offline 两个独立 DB 的四项结果完全相同。
2. `depth <= lag` 时 reorg 前后四项结果完全相同。
3. `depth = lag + 1` 时 stable hash、snapshot、state-ref、block commit 均发生变化，
   tracked balance 从 `125000000` 变为 `0`。
4. reorg 后 online、offline restart、fresh joiner 的四项结果逐字节相同。
5. 每个实例都必须达到 `get_readiness().consensus_ready = true`。

## 可配置项

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `EXPECTED_STABLE_LAG` | `10` | 当前 regtest registry 固定 lag |
| `REORG_DEPTH` | `9` | 单案例深度，只接受 `lag-1/lag/lag+1` |
| `PREFIX_BLOCKS` | `3` | tracked block 前的额外区块数 |
| `SEND_AMOUNT_BTC` | `1.25` | tracked transfer 金额 |
| `SYNC_TIMEOUT_SEC` | `180` | 同步、hash 收敛和 readiness 超时 |
| `MATRIX_WORK_DIR` | `/tmp` 临时目录 | 三案例工作目录 |
| `BASE_PORT` | `30710` | 矩阵第一组服务端口基数 |
| `PORT_STRIDE` | `40` | 不同深度之间的端口跨度 |

## 2026-07-29 历史验证结果（lag=5）

本地 Bitcoin Core 28.1 隔离 regtest 已通过全部三个深度和九条启动/恢复路径。完整
suite 用时约 137 秒；没有连接或修改本机 BTC mainnet 服务。

该测试证明 balance-history 在 stable frontier 边界上的 deterministic convergence。
它不解决已经被 USDB 共识引用的 stable BTC block 后续发生深层 reorg 时，USDB 链自身
采用 archive、人工恢复还是 deterministic rewind 的治理与恢复策略。

## 2026-08-28 当前验证结果（lag=10）

本地 Bitcoin Core 28.1 隔离 regtest 已通过 `depth=9/10/11` 全部三个深度。
每个深度下 online、offline restart、fresh joiner 均在 stable height `105` 收敛到
相同 snapshot、state-ref、block commit 和余额：

- `depth=9` 与 `depth=10` 未替换 stable block，四项 identity 保持不变；
- `depth=11` 首次替换 stable block，三种启动模式均检测并重放 replacement chain；
- 测试未连接或修改本机 BTC mainnet 服务。
