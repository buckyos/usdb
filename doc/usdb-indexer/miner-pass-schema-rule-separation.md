# MinerPass JSON 与操作规则版本分层验收

2026-10-04，本地工作区基于 USDB `16bc16b`、go-ethereum `64213ea43` 完成以下改动和验收。记录包含未提交改动，不代表公开发布或远程 CI 资格。

## 当前合同

| 层次 | 当前版本 | 职责 |
| --- | --- | --- |
| mint JSON / inscription schema | `v: 1` / `uip-0001-miner-pass-inscription:v1` | 字段、类型、互斥形态及严格 JSON 校验 |
| MinerPass state machine | `uip-0002-pass-state-machine:v2` | 首次开户、来源证明、持有历史、同地址操作及跨地址继承 |
| 程序发布 | `testnet-v1-rN` | 固定源码与发布产物，不由铭文选择执行规则 |

Rust 与 Go 只接受 schema v1 + state-machine v2。JSON `v` 必须为整数 1；此前开发版的整数 2 不作为兼容别名，旧状态机也不恢复。无效版本不能绕过 UIP-0016 资格检查。

control-plane 草案、完成核验、handbook、RPC 示例和 nightly/weekly 测试载荷同步使用 schema v1。独立版本族没有随之整体降级，历史默认 registry 和旧发布 fragment 保持冻结。

## 身份与部署边界

testnet-v1 registry 和网络包由权威 catalog 重新生成，Rust/Go 使用一致 golden：

- registry ID：`53b4bfed53b55a4accbd947d04d113a684f1af896d39d2d9bd984e07ad543f32`。
- active-version-set ID：`4145c4cfb1a18558260b130610cf741b58ac25f5264cc5277da34365a42c8599`。
- chain/network ID 仍为 `202610030`；BTC origin、创世分配和 genesis block hash 保持不变。
- genesis 配置中的 registry 绑定及相关文件校验和、runtime compatibility identity 更新。

本次按可重置开发网处理，不迁移此前 indexer 或 USDB-chain 状态。indexer 使用新派生目录；USDB-chain 目录名未变，部署必须显式准备空目录。Bitcoin/BH 基础数据按既有兼容合同复用。正式网不能照搬 origin 替换规则，仍需独立设计历史规则与激活边界。

## 本地验证

| 验证 | 结果 |
| --- | --- |
| Rust workspace 全量测试 | 783 passed，13 ignored；fmt、workspace check、all-targets/all-features clippy 通过 |
| Go fast CI | Go 1.18.5 canonical 与 Go 1.26.0 compatibility 均通过，包含版本组合拒绝及 profile 校验 |
| 网络包和发布工具 | 101 项通过；网络包重建逐字节检查通过 |
| 模拟器、回放及共用测试工具 | 99 项通过；required regression runner 通过 |
| 跨语言身份 | 默认历史、隔离开发、testnet-v1 registry 和包内 catalog 的 golden 检查通过 |
| control-plane | 前端类型检查、构建、真实服务加浏览器夹具验收通过；覆盖首开、同地址、跨地址、协作、失败与过期草案 |
| 脚本与发布说明 | 修改的 ShellCheck、release fragment 校验及两仓 diff 检查通过 |

真实隔离服务重新运行 Core 28.1 → Ord 0.29.0 → balance-history → indexer → Geth：

- schema v1 首次开户、同地址重铸、跨地址继承均成功；第三方向既有 owner 伪造 prev 赠予被拒绝，原 pass 保持有效。
- 新 pass 入金后能量有效，profile 身份与新 golden 一致。
- 实际产生 15 个 USDB 区块，独立节点同步至相同 head，并通过逐块奖励、价格、余额和难度校验。

本机证据在 `/tmp/usdb-miner-pass-v2-profile-0wx8z022`：`run.json` 为 `passed`，退出码 0；`runner.log`、mint audit 和 Geth 输出保存完整验证结果。该目录为临时本地证据，不是公开 CI artifact。

复现真实服务验收：

```bash
python3 tests/run_miner_pass_v2_profile_live.py \
  --ord-bin /path/to/ord \
  --bitcoin-bin-dir /path/to/bitcoin/bin \
  --geth-repo ../go-ethereum
```

本批未运行完整 nightly/weekly、未更新在线节点、未创建 tag 或发布。提交后还需同步 Go 的 USDB revision lock，再以冻结 revision 执行新版本 CI；本地测试通过不能替代这些发布步骤。
