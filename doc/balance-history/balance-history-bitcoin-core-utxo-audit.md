# Balance-History Bitcoin Core UTXO 抽样审计

本文档描述如何使用 Bitcoin Core 的 `scantxoutset` 和 `gettxout`，对
`balance-history` 当前 stable snapshot 做只读抽样审计。该工具不依赖 Electrs，适合开发期
回归和 public release 前主网检查。

## 验证范围

工具验证以下独立路径能够得到相同结果：

1. 从 Bitcoin Core 历史区块输出中确定性抽取 `scriptPubKey`。
2. 独立计算 Electrum-compatible `script_hash = reverse(sha256(scriptPubKey))`。
3. 通过 `scantxoutset raw(...)` 汇总 Bitcoin Core 当前 UTXO 余额。
4. 通过 `get_addresses_balances(block_height=stable_height)` 查询 balance-history 余额。
5. 对抽中的 live outpoint 再调用 `gettxout(..., include_mempool=false)`，核对 value 和
   `scriptPubKey`。

这是一项当前 UTXO 断面审计，不验证任意历史高度的完整 movement/history。Electrs
历史审计和 sampled block replay 仍是后续独立任务。

## Stable Lag 对齐

`scantxoutset` 扫描 Bitcoin Core chainstate 的实际锚点由返回值 `height/bestblock`
决定，不能预先假设它等于 RPC 调用前的 tip。balance-history 则公开
`stable_height/stable_block_hash/stable_lag`。

两者高度通常相差 `stable_lag`。工具会读取区间
`(stable_height, scantxoutset.height]` 的完整区块，并排除在该区间内发生过以下任一事件的
候选脚本：

- 新 output 使用该脚本；
- 任一 input 花费该脚本控制的 prevout。

只有未被触及的脚本，其 stable-height 余额才必然等于 scan-height 余额。input script
依赖 `getblock verbosity=3` 的 `vin.prevout`；如果 recent undo 数据不能提供 prevout，工具
直接失败，不退化成无界的逐交易 txindex 扫描。

运行末尾还会重新检查：

- Bitcoin Core tip 仍等于 `scantxoutset` 的 `height/bestblock`；
- Bitcoin Core 在 `stable_height` 的 canonical block hash 未变化；
- balance-history 在 `stable_height` 的 block commit 仍引用同一 BTC block hash。

任一锚点变化都会 fail closed，调用者应重新执行。

## 主网执行

前置条件：

- Bitcoin Core JSON-RPC 可用，并保留 stable lag 窗口内的 block/undo 数据；
- balance-history RPC 已达到 query-ready，且当前 stable snapshot 有完整 block commit；
- 两个服务连接同一 BTC network；
- 运行窗口内没有其它 `scantxoutset` 任务。

示例：

```bash
PYTHONDONTWRITEBYTECODE=1 python3 \
  src/btc/balance-history/scripts/audit_bitcoin_core_utxo_sample.py \
  --bitcoin-rpc-url http://127.0.0.1:8332 \
  --bitcoin-cookie-file /home/bucky/.bitcoin/.cookie \
  --balance-history-url http://127.0.0.1:28010 \
  --expected-network bitcoin \
  --sample-size 32 \
  --oversample-factor 4 \
  --source-lookback-blocks 2016 \
  --source-block-count 24 \
  --max-gettxout-checks 256 \
  --seed 20260827 \
  --output /tmp/balance-history-mainnet-utxo-audit.json
```

候选脚本来自 stable frontier 之前固定 lookback 内的确定性随机区块，不依赖
balance-history script registry。这样能让样本来源与被审计系统保持独立。`seed`、抽中的
source heights、script、双方余额、UTXO 数量和断面 hash 都写入报告，可用于复跑和 review。

如果 lag window 内活跃脚本较多，工具可能因为可比较样本不足而失败。此时应增大
`--oversample-factor`，不能关闭 lag-window 排除。

## 测试入口

纯单元测试：

```bash
PYTHONDONTWRITEBYTECODE=1 python3 \
  src/btc/balance-history/scripts/test_audit_bitcoin_core_utxo_sample.py
```

隔离 regtest 黑盒测试：

```bash
bash src/btc/balance-history/scripts/regtest_bitcoin_core_utxo_audit.sh
```

该 regtest 构造多地址 UTXO，在 `stable_lag=5` 下实际调用 `scantxoutset`、
`gettxout` 和 `getblock verbosity=3`，并断言 lag-window 内重复出现的 mining script 被排除。
它也已加入 `run_regtest_suite.sh correctness`。

## 限制

- 从历史区块 output script 抽样不是全 UTXO 集的均匀随机抽样；固定 seed 的目标是可重复、
  独立和发现真实脚本兼容问题，不用于统计推断。
- `scantxoutset` 会遍历完整 chainstate，运行成本高于普通 RPC，不应接入 fast CI。
- `gettxout` 只对有界 outpoint 样本交叉验证；完整余额仍以 `scantxoutset` 聚合为准。
- 工具要求审计期间 Core tip 不推进。主网恰逢出块时失败属于预期安全行为，重新执行即可。
- 任意历史高度余额和完整交易历史仍需 Electrs 优化路径或独立 block replay oracle。
