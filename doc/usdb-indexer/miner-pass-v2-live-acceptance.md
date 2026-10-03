# MinerPass V2 最小真实服务验收

本批在隔离环境验证 Core → Ord commit/reveal → balance-history → indexer → Geth 出块/独立节点验证，不操作在线节点。属于 [UIP-0016 实施计划](miner-pass-operation-eligibility-plan.md) 的阶段性验收，不是 testnet-v1 发布资格。

## 复现

需要 Bitcoin Core 28.1、Ord 0.29.0、Rust 工具链，以及配套 `go-ethereum` 工作区。最低配套提交为 USDB `e8c73d9`、Go `aea0a7af9`，后者的兼容锁已指向该 USDB 提交。

在 USDB 根目录执行：

```bash
USDB_GO_BIN=/path/to/go python3 tests/run_miner_pass_v2_profile_live.py \
  --ord-bin /path/to/ord \
  --bitcoin-bin-dir /path/to/bitcoin/bin \
  --geth-repo ../go-ethereum
```

入口先分别构建两个 Rust 服务，再由 Go 共用脚本构建当前 Geth。可用 `--geth-bin /path/to/candidate-geth` 指定已核验的当前候选二进制；不会自动核验任意外部二进制是否对应工作区源码。

默认新建 `/tmp/usdb-miner-pass-v2-profile-*`，选择空闲本机端口。`--work-dir` 只接受空目录；所有服务数据和日志路径都放在其中。脚本退出会停止本次创建的服务，保留数据和日志。执行失败查看 `build.log` / `runner.log` / 各服务日志，不把进程存活当作验收通过。

该测试使用 `btc-regtest / miner-pass-v2-fixture`，stable lag 10、index origin 1；仅改生成的测试配置和 genesis。不会更新 embedded registry、公开网络 genesis 或发布包。

## 自动断言

| 场景 | 断言 |
| --- | --- |
| 首次开户 | 出资地址与接收地址分离，reveal 前余额 0、无有效持有历史、audit 为 `first_opening` |
| 同地址重铸 | 实际 sat 来源地址等于接收地址；允许的 Taproot key-path 签名；旧 prev Consumed、新 pass Active |
| 第三方伪造 prev 赠予 | 铭文 Invalid，错误码 `INELIGIBLE_RECIPIENT`；既有 pass 仍 Active |
| 跨地址继承 | 真实来源为旧 owner，接收地址是全新零余额地址；audit 为 `cross_owner`，只消费指定 prev |
| 保护旧铭文 | 选币排除铭文 UTXO；同地址/跨地址操作前后的既有铭文位置不变 |
| 经济查询 | 新 pass 入金后能量大于 0；registry、V2 schema/state、rules scope 与黄金向量一致 |
| Geth | 新 pass selector、BTC 状态身份、难度、累计发行与奖励余额符合独立计算；新节点从 genesis 同步至相同高度和 hash |

`go-ethereum/scripts/usdb/run_long_ci.sh nightly go-profile` 的首个 profile 场景已启用 `MINER_PASS_V2_TRANSITIONS=1`。其余 nightly/weekly 场景仍按后续计划迁移；本批没有运行整个 shard 或远程 CI。

## 本次实测记录

2026-10-02（US/Pacific），基于 USDB `dcd41f6`、Go `889d74197` 和本批未提交测试改动，Core 28.1 / Ord 0.29.0 完成上述路径。真实 Geth 最终产生 8 个区块，独立计算的奖励总额与余额一致，第二个节点导入 8 块并通过相同 head 检查。

本机产物目录：`/tmp/usdb-miner-pass-v2-profile-0p003wpy`，`run.json` 的 `status=passed`、`exit_code=0`。目录是本机临时证据，不是已上传的 CI artifact。

- `run.json`：仓库 HEAD、工作区状态、端口、Geth binary SHA-256、最终退出码。
- `usdb/source-selection-*/`：确认 UTXO 和待保护铭文清单。
- `usdb/*.ord-result.json`：Ord 返回的 commit/reveal 交易信息。
- `usdb/v2-audit-*.json`、`v2-transitions.json`：三条成功路径及拒绝记录。
- `geth/mined_blocks.json`、`geth/validator.log`、`runner.log`：区块、独立节点导入、逐块核算与最终结果。

另对既有 `MINER_LIVE_STATE_CHECK=1` 分支做了独立回归（同样全新目录）：外部转移后接收方用自身 cardinal UTXO remint；Geth 进程保持不变，selector 切至新 pass；正常停止 indexer 后挖矿停滞，重启 indexer 后自动恢复。共核算 53 个区块，产物在 `/tmp/usdb-miner-pass-v2-live-state-OWZEfD`。这是服务中断/恢复回归，不替代进程崩溃、双库写入故障及 reorg 恢复矩阵。

配套快速回归共 33 项通过：USDB 选币/config 安全边界 4 项、共用 regtest shell 12 项；Go profile oracle 5 项、V2 scope/旧规则拒绝 3 项、validator outage gate 9 项。修改的 Python 编译检查、Shell 语法和两个仓库 `git diff --check` 通过；本批不改 Rust/Go 生产源码，服务均由当前源码编译。

最初的脚本对接发现 audit RPC 需要单元素参数数组，以及 profile oracle 缺少 rules scope。修正测试脚本后全新重跑通过；本批没有因此修改生产共识代码。

## 后续覆盖

这次钱包为 P2TR，完整 Core 开启 `txindex=1`。不据此宣称 hash 地址冷钱包全流程、完整余额迁移、全部签名类型、协作/多 prev、同块竞争、普通付款攻击边界、深 reorg、故障恢复、签名 checkpoint 或 AssumeUTXO 已完成服务级验收。

下一批复用公共 catalog 和显式来源构造工具，迁移攻击与协作矩阵、reorg/重启/完整重放、snapshot/checkpoint、`txindex=0`/AssumeUTXO 和 world-soak；然后运行完整 nightly/weekly。control-plane 引导继续后移。

后续 nightly/weekly 迁移与扩展验收单独记录在 [V2 矩阵验收](miner-pass-v2-matrix-acceptance.md)，不改变本页原批次的验收范围。
