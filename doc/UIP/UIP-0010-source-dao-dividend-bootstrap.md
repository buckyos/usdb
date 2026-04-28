UIP: UIP-0010
Title: SourceDAO and Dividend Bootstrap
Status: Draft
Type: Standards Track
Layer: ETHW Genesis / System Contracts / Fee Split Activation
Created: 2026-04-27
Requires: UIP-0000, UIP-0008, UIP-0009
Activation: ETHW network activation matrix; first official networks define canonical genesis and bootstrap artifacts before public launch

# 摘要

本文定义 USDB ETHW 链上 SourceDAO / Dividend system contract 的冷启动流程。

UIP-0010 解决的问题是：

- `DividendAddress` 必须在共识层预先确定。
- `Dividend` 合约不能在未初始化状态下直接承接 fee split。
- 新节点加入网络时必须能验证自己使用了同一份 genesis、同一套系统合约 code 和同一条 bootstrap 历史。

本文只定义系统合约冷启动、bootstrap artifact、初始化交易顺序和 fee split activation 边界，不定义 CoinBase 释放公式、手续费比例、矿工奖励比例或 price / real price 规则。

# 动机

USDB 需要把一部分交易手续费或后续经济收入导入 SourceDAO / Dividend 分红池。

普通“链启动后部署合约再决定地址”的方式不适合这里，原因是：

1. ETHW 共识规则需要提前知道 fee split 目标地址。
2. `Dividend` 依赖 `Dao` 地址和初始化参数。
3. 如果 `DividendAddress` 由运行期部署动态决定，会让共识配置依赖链启动后的普通交易结果，形成冷启动循环。

因此，v1 必须把系统地址和 runtime code 纳入网络定义，再用 bootstrap 交易完成初始化，最后在明确高度启用 fee split。

# 非目标

本文不定义：

- fee split 的比例、基数或具体分账公式。
- CoinBase emission、uncle reward 或矿工奖励分配。
- SourceDAO 业务模块的完整治理规则。
- SourceDAO 前端、后端或非共识部署流程。
- 未来系统合约升级机制。

# 术语

| 术语 | 含义 |
| --- | --- |
| `DaoAddress` | SourceDAO 主合约系统地址。 |
| `DividendAddress` | Dividend 分红池系统地址，也是 fee split 的目标地址。 |
| `bootstrapAdmin` | genesis 预置余额的启动账户，用于发送初始化交易。 |
| `canonical_genesis` | 包含系统合约 runtime code 的确定性 genesis JSON。 |
| `source_dao_bootstrap_config` | 启动后初始化 SourceDAO / Dividend 所需的配置。 |
| `bootstrap_state` | SourceDAO bootstrap job 写出的状态快照。 |
| `bootstrap_marker` | 表示 bootstrap 已完成的最小 marker。 |
| `DividendFeeSplitBlock` | ETHW 开始把 fee split 目标金额记入 `DividendAddress` 的激活高度。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 当前部署流程基线

当前 docker / go-ethereum / SourceDAO 原型已经形成以下开发期流程：

1. `run_local_bootstrap.sh prepare`
   - 初始化 `bootstrap.env`。
   - 初始化 `ethw-bootstrap-config.json`。
   - 初始化 `sourcedao-bootstrap-config.json`。
   - 校验两份配置中的共享字段一致。
   - 调用 `geth dumpgenesis --usdb --usdb.bootstrap.config ...` 生成 `ethw-genesis.json`。
2. `bootstrap-init`
   - 复制 canonical genesis、genesis manifest、签名、trusted keys 和 SourceDAO config。
   - 写出 `bootstrap-manifest.json`。
3. `ethw-init`
   - 校验 genesis artifact。
   - 执行 `geth --datadir ... init ethw-genesis.json`。
   - 写出 `ethw-init.done.json`。
4. `ethw-node`
   - 只在 init marker 与 genesis artifact 匹配时启动。
5. `sourcedao-bootstrap`
   - 等待 ETHW RPC ready。
   - 读取 `sourcedao-bootstrap-config.json`。
   - 调用 SourceDAO 工作区脚本初始化 Dao / Dividend 及可选模块。
   - 写出 `sourcedao-bootstrap-state.json` 与 `sourcedao-bootstrap.done.json`。

这些流程是 UIP-0010 的实现参考，但开发期默认值不自动成为 public network final 参数。

# v1 总体规则

USDB v1 推荐采用：

```text
fixed_system_addresses
    -> genesis predeploy runtime code
    -> post-start bootstrap initialization transactions
    -> fee split activation height
```

规则：

- `DaoAddress` 和 `DividendAddress` 必须在 network release 前固定。
- `DaoAddress` 和 `DividendAddress` 的 runtime code 必须进入 canonical genesis `alloc`。
- `bootstrapAdmin` 必须在 genesis 中拥有足够余额发送 bootstrap 交易。
- `DividendFeeSplitBlock` 必须在 ETHW chain config 中固定。
- fee split 不得在 `Dividend` 初始化完成前生效。
- public network 不得依赖节点本地松散配置动态生成不同 genesis。

# System Addresses

`DaoAddress` 与 `DividendAddress` 必须满足：

- 是 EVM 地址。
- 在 public network release manifest 中固定。
- 不能作为普通用户地址分配。
- 必须在 genesis `alloc` 中拥有预置 runtime code。
- 如果地址发生变化，必须生成新的 canonical genesis 和新的 `USDBGenesisHash`。

当前开发期原型值：

```text
DaoAddress      = 0x0000000000000000000000000000000000001001
DividendAddress = 0x0000000000000000000000000000000000001002
```

这些值作为当前开发和测试基线，也可以作为 public testnet / mainnet 的候选预留地址。

public network 最终采用前必须完成一次 release preflight：

- 确认 `DaoAddress` / `DividendAddress` 不与其他 genesis alloc 地址冲突。
- 确认这两个地址只作为 system predeploy 地址使用，不分配给普通用户或 bootstrap 账户。
- 确认 canonical genesis 中这两个地址的 runtime code 非空。
- 确认 release manifest、ETHW chain config、SourceDAO bootstrap config 中的地址完全一致。
- 如果 public release 决定继续使用当前 `0x...1001` / `0x...1002`，这些地址必须进入 release manifest 并在该网络生命周期内视为固定。

# Genesis Predeploy

canonical genesis 必须预置：

- `DaoAddress` 的 runtime code。
- `DividendAddress` 的 runtime code。
- `bootstrapAdmin` 的初始余额。

v1 不建议在 genesis 中预置初始化后的复杂 storage。原因是：

- SourceDAO / Dividend 使用 initializer 语义，storage layout 审计成本更高。
- 初始化交易更容易审计和回放。
- 后续新节点可通过链上历史交易重放得到相同状态。

如果未来选择把初始化后的 storage 写入 genesis，则这些 storage 必须进入 canonical genesis `alloc.storage`，并改变 `USDBGenesisHash`。

# Artifact Commitments

public network release 必须能审计以下 artifact：

| Artifact | 必须性 | 说明 |
| --- | --- | --- |
| `canonical_genesis_json` | 必须 | 含 system contract predeploy。 |
| `USDBGenesisHash` | 必须 | 由 canonical genesis 生成。 |
| `genesis_sha256` | 应该 | 用于文件完整性校验，不替代 `USDBGenesisHash`。 |
| `genesis_manifest` | 必须 | 描述 genesis、chain config、system addresses、code hash。 |
| `release_manifest_signature` | public network 必须 | 证明 release manifest 来自指定发布方。 |
| `trusted_release_signing_keys` | public network 应该 | joiner 用于验证 release manifest signature 的可信公钥集合。 |
| `Dao runtime code hash` | 必须 | 从 SourceDAO artifact 的 deployed bytecode 计算。 |
| `Dividend runtime code hash` | 必须 | 从 SourceDAO artifact 的 deployed bytecode 计算。 |
| `source_dao_bootstrap_config` | 必须 | 启动后初始化参数。 |
| `bootstrap_state` | 必须 | bootstrap job 输出的完整状态。 |
| `bootstrap_marker` | 必须 | bootstrap 完成的最小状态标记。 |

建议 code hash 使用 `keccak256(runtime_code)`，manifest 文件完整性使用 `sha256(file)`。具体 canonical encoding 可以在实现阶段固定，稳定后回写本 UIP。

v1 建议的实现方向：

- `runtime_code` 来自 SourceDAO artifact 的 deployed bytecode，而不是 creation bytecode。
- `runtime_code_hash = keccak256(runtime_code_bytes)`，用于证明 genesis predeploy 的 EVM code identity。
- `artifact_file_sha256 = sha256(artifact_file_bytes)`，用于证明 release artifact 文件完整性。
- JSON artifact canonical encoding、字段排序、hex 大小写和 `0x` 前缀规范先保留为 `TODO`，待实现稳定后再固定。

# Release Manifest Signature

UIP-0010 将可信性分为两层：

1. 共识可信：由 canonical genesis、`USDBGenesisHash`、ETHW chain config、链上交易历史和本地状态校验保证。
2. 发布物可信：由 release manifest signature 保证，用于证明下载到的关键文件来自指定发布方。

release manifest signature 类似安装包、Docker image 或 Linux package repository 的数字签名。它不改变 ETHW 共识规则，也不能替代节点本地的 genesis / chain / contract state 校验。

public network release manifest 应至少承诺以下内容的 hash 或明确值：

```text
network_name
chain_id
network_id
USDBGenesisHash
genesis_file_sha256
activation_matrix_id
DaoAddress
DividendAddress
Dao runtime code hash
Dividend runtime code hash
SourceDAO bootstrap config hash
bootstrap_state hash
bootnodes / discovery config hash
```

推荐 joiner 校验流程：

```text
verify release manifest signature
    -> verify release manifest 中记录的文件 hash
    -> init / start with canonical genesis
    -> sync ETHW chain
    -> verify on-chain SourceDAO / Dividend bootstrap state
```

规则：

- local dev / CI 可以跳过 release manifest signature。
- public testnet 应支持 release manifest signature，可以通过启动参数、安装包配置或配置文件提供 trusted key。
- mainnet 应提供明确的 release signing key 分发和轮换机制。
- `trusted_release_signing_keys` 是供应链安全机制，不是共识规则；不同节点只要使用相同 genesis 和 chain config，最终仍由链同步和状态校验判断是否在同一网络。

# Chain Config Fields

UIP-0010 要求 ETHW chain config 至少表达：

```text
DividendAddress
DividendFeeSplitBlock
fee_split_policy_version
```

语义：

- `DividendAddress == 0x0` 时，fee split 必须视为未启用。
- `DividendFeeSplitBlock == nil` 时，fee split 必须视为未启用。
- 只有 `DividendAddress != 0x0` 且 `DividendFeeSplitBlock` 已到达时，ETHW 才能执行 fee split 状态转换。
- `fee_split_policy_version` 描述后续 UIP-0011 定义的具体分账公式版本。

当前 go-ethereum 原型已有：

```text
ChainConfig.DividendAddress
ChainConfig.DividendFeeSplitBlock
ChainConfig.IsDividendFeeSplit(block_number)
```

# Bootstrap Config

开发期 genesis 生成配置当前使用：

```json
{
  "chainId": 20260323,
  "artifactsDir": "../../SourceDAO/artifacts-usdb",
  "daoAddress": "0x0000000000000000000000000000000000001001",
  "dividendAddress": "0x0000000000000000000000000000000000001002",
  "bootstrapAdminPrivateKey": "<dev-only>",
  "bootstrapAdminBalanceWei": "10000000000000000000",
  "genesisDifficulty": "0x180000",
  "minimumDifficulty": "0x100000",
  "dividendFeeSplitBlock": 16
}
```

public network 配置必须与开发期配置分离：

- 禁止在公开 release artifact 中发布 `bootstrapAdminPrivateKey`。
- public release 可以发布 `bootstrapAdmin` 地址、公钥或多签治理说明。
- `genesisDifficulty`、`minimumDifficulty` 与 UIP-0009 的 final 参数必须一致。
- `dividendFeeSplitBlock` 必须大于预计 bootstrap 完成高度，并留出审计和恢复窗口。

# Bootstrap Admin 管理

`bootstrapAdmin` 是冷启动阶段的临时权限主体，用于发送 SourceDAO / Dividend 初始化交易。

当前 SourceDAO 原型具备以下约束：

- `Dao.initialize()` 将 `bootstrapAdmin` 设置为初始化交易发送者。
- SourceDAO 模块地址 setter 由 `onlyBootstrapAdmin` 控制。
- 模块地址 setter 使用 `onlySetOnce` 语义，已配置模块不能被覆盖。
- 当前原型提供 `transferBootstrapAdmin(address)`，但没有协议级 `finalizeBootstrap()` / revoke 语义。

因此，public network 不应把单一开发 EOA 作为长期 `bootstrapAdmin`。推荐策略：

| 网络类型 | 推荐 `bootstrapAdmin` |
| --- | --- |
| local dev / CI | 临时 EOA，可以由配置文件生成或注入。 |
| public testnet | 多签账户或 threshold signer。 |
| mainnet | 多签账户、治理合约或经组委会确认的 threshold custody。 |

public network bootstrap 完成后，必须明确长期权限归属：

- 可以把 `bootstrapAdmin` 转移给治理多签或治理合约。
- 如果 SourceDAO 最终实现 `finalizeBootstrap()` / `bootstrapFinalized`，可以在 full bootstrap 完成后关闭 bootstrap 权限。
- 在未实现显式 finalization 前，release manifest 必须记录 bootstrap 完成后的 `bootstrapAdmin` 最终地址和管理策略。

是否新增独立 `Bootstrap Admin Governance` UIP 暂不强制。若后续需要定义 key ceremony、签名门限、撤权流程、事故恢复和治理交接，则应该拆成独立 Process / Operational UIP；当前 UIP-0010 只要求 public release 不得依赖未托管的单一私钥长期持有权限。

# Bootstrap Transaction Sequence

v1 最小初始化顺序：

```text
1. Dao.initialize()
2. Dividend.initialize(cycleMinLength, DaoAddress)
3. Dao.setTokenDividendAddress(DividendAddress)
```

要求：

- 以上交易必须由 `bootstrapAdmin` 或协议指定权限账户发送。
- 每笔交易的 tx hash、block number、status 和错误信息必须进入 `bootstrap_state`。
- 如果脚本发现目标状态已经完成，允许跳过交易，但必须校验链上状态与 config 一致。
- 如果链上已有状态与 config 冲突，bootstrap 必须失败，不得继续。

SourceDAO full bootstrap 可以继续初始化其他模块，例如 committee、token、project、lockup、acquired 等。但对 fee split 来说，最小完成条件是：

```text
Dao.bootstrapAdmin == bootstrapAdmin
Dividend.cycleMinLength == cycleMinLength
Dao.dividend == DividendAddress
code(DaoAddress) != empty
code(DividendAddress) != empty
```

public network 如果把 SourceDAO 作为首个 release 的完整治理系统，则 `scope = full` 应成为 release 完成条件，而不是只完成 dao-dividend-only。

`scope = full` 的强制状态应至少包括：

- `DaoAddress` 和 `DividendAddress` runtime code 非空。
- `Dao.initialize()` 已成功。
- `Dividend.initialize(cycleMinLength, DaoAddress)` 已成功。
- `Dao.setTokenDividendAddress(DividendAddress)` 已成功。
- committee、dev token、normal token、token lockup、project、acquired 等 release manifest 声明的 SourceDAO 模块已经完成部署和 wiring。
- `bootstrap_state.final_wiring` 中所有必填模块地址非零，且与链上状态一致。

后续 SourceDAO 模块升级不属于本 UIP 冷启动范围，应走 SourceDAO / proxy / governance 自身升级流程。

# Fee Split Activation

`DividendFeeSplitBlock` 是 fee split 的共识激活高度。

规则：

- `DividendFeeSplitBlock` 之前，ETHW 不得把 fee split 金额记入 `DividendAddress`。
- `DividendFeeSplitBlock` 之后，ETHW 可以按 `fee_split_policy_version` 对交易手续费执行分账。
- `DividendFeeSplitBlock` 必须配置为 bootstrap 初始化完成之后的高度。
- 如果节点在到达 `DividendFeeSplitBlock` 时无法确认 `DividendAddress` 已预置 code，必须 fail closed。

当前开发期原型使用：

```text
DividendFeeSplitBlock = 16
```

该值只用于本地测试，不是 public network final 参数。

public network 的 `DividendFeeSplitBlock` 不需要是一个精确的“最小间隔”。它是 release 计划中的 fee split 生效高度，可以设置在网络启动后数天或一周，以便完成：

- SourceDAO full bootstrap。
- release artifact、bootstrap state 和链上状态复核。
- explorer / monitor / joiner 同步验证。
- 必要时的节点重启或配置修正。

当前 docker 冷启动流程会在 ETHW RPC ready 后紧跟执行 SourceDAO bootstrap，因此 local dev / CI 可以使用很短的激活窗口。public network 应由 activation matrix 固定最终高度。

# Bootstrap State and Marker

bootstrap job 必须输出可审计状态。

最小 `bootstrap_marker`：

```json
{
  "completed": true,
  "completed_at": "YYYY-MM-DDTHH:MM:SSZ",
  "mode": "dev-workspace|public-release",
  "scope": "dao-dividend-only|full",
  "chain_id": "20260323",
  "dao_address": "0x...",
  "dividend_address": "0x..."
}
```

完整 `bootstrap_state` 应至少包含：

- state version。
- status：`running` / `completed` / `error`。
- chain id。
- DAO / Dividend 地址。
- bootstrap admin 地址。
- operation list：
  - operation name。
  - status：`completed` / `skipped` / `error`。
  - tx hash。
  - block number。
  - error message。
- final wiring：
  - `dividend = DividendAddress`。
  - 其他 SourceDAO 模块地址，如果 scope 为 `full`。

`bootstrap_state` / `bootstrap_marker` 本身不是共识输入，不应被视为替代链上状态或 genesis hash 的安全来源。

签名策略：

- local dev / CI 可以不签名。
- public release 应该签名 release manifest，manifest 中引用 canonical genesis、system contract code hash、bootstrap config 和 bootstrap state。
- `bootstrap_marker` 可以不单独签名；如果需要发布给 joiner 或运维系统，应通过已签名 manifest 间接承诺其 hash。
- 签名主体应该是 release operator、组委会多签或后续治理确认的 release signing key。

签名的安全目标是 release artifact provenance 和供应链完整性，不是改变 ETHW 共识。共识仍由 canonical genesis、chain config 和链上交易历史决定。

# Joiner Validation

后续加入网络的节点必须验证：

1. release manifest signature 有效，或者当前网络明确处于 unsigned dev mode。
2. release manifest 中引用的文件 hash 与本地文件一致。
3. 使用同一份 canonical genesis。
4. `USDBGenesisHash` 与 release manifest 一致。
5. system contract runtime code hash 与 manifest 一致。
6. `ChainConfig.DividendAddress` 与 `DividendAddress` 一致。
7. `ChainConfig.DividendFeeSplitBlock` 与 release manifest 一致。
8. 链上 bootstrap 交易已执行并成功。
9. 当前链上状态满足最小完成条件。

joiner 不需要重新执行 bootstrap 交易。它只需要同步链上历史并审计最终状态。

# Trusted Bootstrap Manifest Key

public joiner 可以内置或配置 trusted bootstrap manifest key，用于验证下载到的 release artifact：

- canonical genesis。
- genesis manifest。
- SourceDAO bootstrap config。
- bootstrap state / marker。
- bootnodes / discovery hints。

trusted key 的价值是降低 joiner 对下载渠道、镜像站或手工复制文件的信任要求。它不替代链同步和链上状态校验，也不应成为共识规则的一部分。

v1 建议：

- local dev / CI 不要求 trusted key。
- public network release tooling 应支持 signed manifest 验证。
- public testnet 可以通过启动参数、安装包配置或 release bundle 提供 trusted key。
- mainnet 应考虑把 official release signing key 或 trusted key registry 随客户端 / 安装包分发，并支持 key rotation。
- 是否把 trusted key 编译进客户端、写入安装包配置，还是由 joiner 启动参数提供，留给后续冷启动 / joiner 流程文档确定。

# 与 UIP-0009 的关系

UIP-0009 定义 ETHW chain config、genesis、difficulty、payload version 和网络启动边界。

UIP-0010 在 UIP-0009 基础上进一步定义：

- 哪些 system contract code 进入 genesis。
- 哪些 bootstrap 参数必须进入 release manifest。
- 哪些 post-start bootstrap 交易必须执行。
- `DividendFeeSplitBlock` 的激活前置条件。

如果 UIP-0010 修改 canonical genesis，必须重新生成 UIP-0009 中记录的 `USDBGenesisHash`。

# 与 UIP-0011 的关系

UIP-0011 将定义 CoinBase emission、reward split 和 fee split 公式。

UIP-0010 只提供：

- `DividendAddress`。
- `DividendFeeSplitBlock`。
- `fee_split_policy_version` hook。
- bootstrap 完成状态。

UIP-0011 不应重新定义 SourceDAO / Dividend 冷启动流程。

# 实现影响

go-ethereum:

- `/home/bucky/work/go-ethereum/cmd/geth/usdbbootstrap.go`
- `/home/bucky/work/go-ethereum/cmd/geth/chaincmd.go`
- `/home/bucky/work/go-ethereum/core/genesis.go`
- `/home/bucky/work/go-ethereum/params/config.go`
- `/home/bucky/work/go-ethereum/core/state_transition.go`

SourceDAO:

- `/home/bucky/work/SourceDAO/scripts/usdb_bootstrap_smoke.ts`
- `/home/bucky/work/SourceDAO/scripts/usdb_bootstrap_full.ts`
- `/home/bucky/work/SourceDAO/tools/config/sourcedao-bootstrap-full.example.json`

USDB docker:

- `docker/scripts/tools/run_local_bootstrap.sh`
- `docker/scripts/helpers/bootstrap_local_inputs_common.sh`
- `docker/scripts/entrypoints/bootstrap_init.sh`
- `docker/scripts/entrypoints/ethw_init.sh`
- `docker/scripts/entrypoints/start_sourcedao_bootstrap.sh`
- `docker/compose.bootstrap.yml`

# 测试要求

至少需要覆盖：

- `geth dumpgenesis --usdb --usdb.bootstrap.config` 生成 deterministic genesis。
- `DaoAddress` 和 `DividendAddress` 的 runtime code 非空。
- generated genesis 的 `alloc` 包含 Dao / Dividend code 和 bootstrap admin balance。
- `USDBGenesisHash` 与 generated genesis 一致。
- `DividendAddress` / `DividendFeeSplitBlock` 进入 chain config。
- `IsDividendFeeSplit` 在 `nil`、zero address、激活前、激活后路径正确。
- `Dao.initialize()` 成功。
- `Dividend.initialize(cycleMinLength, DaoAddress)` 成功。
- `Dao.setTokenDividendAddress(DividendAddress)` 成功。
- bootstrap state / marker 可解析且字段一致。
- bootstrap 后重启节点仍保持状态。
- joiner 使用同一 genesis 后可重放 bootstrap 历史并验证最终状态。
- fee split 激活前 `DividendAddress` 不收取协议分账。
- fee split 激活后按 UIP-0011 的规则进入 `DividendAddress`。

# 待审计问题

| 问题 | 当前结论 | 后续动作 |
| --- | --- | --- |
| public testnet / mainnet 的最终 `DaoAddress` 和 `DividendAddress` | 当前 `0x...1001` / `0x...1002` 可以作为候选预留地址。 | public release 前做 address conflict preflight 并写入 release manifest。 |
| SourceDAO artifact / runtime code hash canonical encoding | 方向是 `keccak256(deployedBytecode)`，artifact 文件用 `sha256(file)`。 | JSON / hex canonical encoding 等实现稳定后回写本 UIP。 |
| `bootstrapAdmin` 使用单一临时账户、多签账户或治理合约 | local dev 可用临时 EOA；public network 不应长期依赖单一私钥。 | 讨论多签 / threshold / governance handoff，并决定是否拆独立 UIP。 |
| `bootstrapAdmin` 权限是否需要 finalization 或撤权 | 当前 SourceDAO 有 `onlySetOnce` 和 `transferBootstrapAdmin`，但无协议级 finalize。 | 评估是否为 SourceDAO 增加 `finalizeBootstrap()`。 |
| `DividendFeeSplitBlock` 与 bootstrap 完成高度之间的安全间隔 | 不要求精确最小间隔；public network 应留 release 复核和恢复窗口。 | 在 UIP-0008 activation matrix 中固定每个 public network 的具体高度。 |
| SourceDAO full bootstrap 是否进入 public network 首次 release 强制状态 | 若首个 release 需要完整 SourceDAO 治理系统，则 `scope = full` 应成为完成条件。 | 确认 public testnet / mainnet 的 required module set。 |
| `bootstrap_state` / `bootstrap_marker` 是否需要签名 | marker 本身不是共识输入；public release 应签 manifest，并由 manifest 引用 state/marker hash。 | 设计 release signing key / signer set。 |
| public joiner 是否需要内置 trusted bootstrap manifest key | trusted key 是 artifact provenance 机制，不是共识规则。 | 放到后续 cold-start / joiner 流程中确定嵌入方式和轮换策略。 |
