# USDB 独立 DAO：Bootstrap 参数、部署与验收核对

> 本文的发现与探针记录为修复前基线。后续工具修复、验收规则与验证结果见文末“工具改进跟进”，
> 不应把旧扫描计数或旧失败记录作为当前代码的运行结果。

日期：2026-09-06。状态：Review / Proposal，尚未修改合约、发布配置或测试机。

最初核对按 **USDB 独立 DAO** 重新确定参数。随后用户明确同意测试网复用源 DAO 的原始代币分配，
并从源链读取当前委员会成员；独立 DAO 的项目身份和其他经济/治理参数仍需单独确定。
本文参数表保留最初审查基线，最新源链导入结果见文末“测试网源链参数导入”；建议值不是已冻结的 release 参数。

核对版本：SourceDAO `66b2f46`，go-ethereum `8798e54b9`，USDB `f8841d3`。本轮未通过 SSH 核实测试机
最新高度，也未启用 miner、发送交易、重建镜像或改变持久数据。

## 1. 结论与实施顺序

当前配置和工具不适合直接作为 USDB 独立 DAO 的公开启动输入。建议先完成：

1. 冻结独立 DAO 的公开参数和签名权限归属；将临时连接信息与公开参数分开。
2. 修复发布命令的 `rpcUrl` 阻塞、bootstrap 交易日志恢复、strict 验收与受审 artifact 的绑定，以及 opcode audit 空跑。
3. 修复现存治理审计阻塞项并完成合约资格验证，生成一致的最终 artifact / bundle。
4. 以受控 candidate 模式启用首个 miner，执行 bootstrap 和验收，再开放公共网络入口。

`Dividend.bootstrapFinalized()` 是手续费门禁的就绪标记，不能单独证明治理、代码身份、分配或公共发布
验收已经通过。初始化交易需要区块，因此可以先冻结和演练部署计划，再开启受控 miner；不能在 block 0
没有出块时要求链上 bootstrap 已完成。

## 2. 参数核对

权威输入是 `docker/networks/testnet-v0/artifacts/sourcedao-bootstrap-config.json`。
当前数组、成员、游标及权重与 SourceDAO 的历史 `scripts/deprecated/deploy_all_as_sourcedao.ts` 一致。
这是历史脚本对照结果，本轮未查询 BuckyOS 正式网链上当前状态。

| 参数 | 当前值 / 语义 | 独立 USDB DAO 建议 |
| --- | --- | --- |
| DAO / Dividend 地址 | 固定 `0x…1001` / `0x…1002`，genesis 直接预置代码 | 按 UIP-0010 保留 system address；不是可随意替换的普通代理 |
| Bootstrap signer | `0x0b5223FD…310c9`，genesis 预置 10 USDB | 测试网 signer 是否继续使用需确认；正式网使用独立 custody。修改地址必须同步 genesis 与 bundle |
| 分红最小周期 | `cycleMinLength=60` 秒 | 建议公共测试网按 **604800 秒（7 天）** 演练；快速周期留在隔离 regtest。7 天是建议，也与历史部署脚本一致，并非普遍强制标准 |
| 每笔 gas 上限 | `8,000,000`，当前 genesis block gas limit 为 `30,000,000` | 可先保留为交易上限，同时逐笔估算、设置手续费上限、计算总预算；不能只验证数字为正 |
| 开发治理代币 | `BuckyOS Develop DAO Token / BDDT`，18 decimals | 改为独立 USDB DAO 命名，symbol 单独确认，避免与原生 USDB 混淆 |
| 普通治理代币 | `BuckyOS DAO Token / BDT`，fresh supply 为 0 | 独立命名；保留当前按 DevToken 转换产生的模型前，确认这就是预期经济模型 |
| DevToken 总量 | `2,100,000,000` 个 | 总量无通用“正式网标准”，应由 USDB 经济方案明确，不能直接继承 |
| 初始分配 | 10 个历史地址，合计 `146999999.999999999999999994` 个，约 7% | 全部重新定义用途、地址、数量与归属；最大旧地址占已释放量约 74.75%，不能默认保留 |
| 未释放储备 | `1953000000.000000000000000006` 个保留在 DevToken 合约自身 | 明确项目奖励、团队、生态等额度；配置总量不是全部立即流通 |
| 委员会 | 3 个历史成员；普通决议要求过半，即 2/3 | 使用独立成员；若有足够独立运营者，建议 5 人委员会，对应 3/5。成员数量应与实际责任人和可用性匹配 |
| 开发者投票倍率 | `400 → 120` 表示 **4 倍 → 1.2 倍** | 明确 USDB 的投票权政策，不把它当手续费比例。当前合约要求两个值均大于 100，若希望 1:1 必须修改合约 |
| Proposal cursor | `7`，下一笔 proposal 使用该 ID | fresh DAO 建议从 `1` 开始，不带入历史编号 |
| Project / investment cursor | `4 / 4`，并未导入前四条业务记录 | fresh DAO 建议均从 `1` 开始；当前代码也允许 `0`，需统一 ID 约定 |
| 主项目与目标版本 | `Buckyos / 1.0.0` | 建议改为显式 USDB 项目身份与实际里程碑。倍率变化由链上项目版本发布时间触发，不是升级软件版本就触发 |
| TokenLockup | `Buckyos / 1.0.0` 发布后 180 天线性释放 | 改为 USDB 里程碑并确认 180 天是否合适；这段期限写在合约中，不是 bootstrap JSON 参数 |
| 手续费启用高度 | USDB block `8192` | 作为现有测试网硬截止保留并监控；正式网高度按出块实测、部署和验收预算冻结，不把 8192 解释为固定时长 |

还需要明确三个产品语义：

- 原生 USDB、矿工资格和 DAO ERC-20 治理代币是不同资产/资格。持有 USDB 或成为 miner 不会自动获得
  DevToken / NormalToken 的治理和质押分红权。
- 初始分配通过 `DevToken.initialize()` 直接 mint 给地址；仅配置 TokenLockup **不会自动锁住初始分配**。
  若有团队锁仓要求，需要显式分配/锁仓步骤和逐地址验收，不能只填写解锁项目名。
- Dividend 周期按交易触发推进；原生手续费到达合约后，还需 `updateTokenBalance(address(0))` 记账。
  应规划普通运维 keeper 的 gas、监控与重试，验证 `tryNewCycle`、质押和领取路径。
  该任务不应持有 bootstrap 权限。

DAO 和 Dividend 是 direct predeploy，不能通过 UUPS `onlyProxy` 入口升级；当前 Dividend 也没有修改
`cycleMinLength` 的公开 setter。周期和这两个合约的安全设计应在初始化前确定。其他六个模块使用 UUPS
代理，升级经 Committee 治理，并不意味着 bootstrap EOA 可以任意替换已接入的模块。

## 3. 已确认的缺口

### 3.1 P1：发布文档中的 bootstrap 与验收命令直接失败

public config 按设计不保存 `rpcUrl`，命令通过 `--rpc-url` 注入。但两份脚本都先执行
`resolveBootstrapConfig(sourceConfig)`，其中强制要求 `config.rpcUrl`，之后才应用 CLI override。

实测两条命令均在连接 RPC 和读取 signer 之前报：

```text
Error: Missing required bootstrap config field: rpcUrl
```

位置：SourceDAO `scripts/usdb_bootstrap_full.ts:468,1264`；
`scripts/usdb_validate_bootstrap.ts:411,791`。
开发容器入口先生成带 rpcUrl 的 runtime config，所以可以掩盖公开运维命令的缺口。

建议统一先解析有效 RPC 参数，并将其作为 runtime context；不为绕过此错误修改已经冻结的公开配置。
参数缺失、未知 CLI 参数、重复 JSON 字段等也应在发交易前失败。

### 3.2 P1：状态文件不具备部署恢复和交易证据保留能力

`usdb_bootstrap_full.ts:1267` 每次新建空 modules / operations，`1280` 在确认 RPC 网络和 signer 前
写状态；`writeState` 使用普通覆盖写，无原子替换或 fsync。脚本不读取旧 state 恢复进度。

本轮隔离函数探针复现：原 state 有 1 笔 completed 交易，错误 chain ID 导致预检失败，但 state 已被写成
0 笔 operations。即使完全成功后重跑，已接入模块会 skip，先前交易列表也不会被保留。

影响不只在显示层：`internal/usdbacceptance/acceptance.go:440` 要求 completed 交易证据非空；并要求
checkpoint 前全部交易与日志集合一致。重跑可能部署成功却无法生成验收凭证；在 implementation/proxy
部署后、wiring 前中断，也可能重复部署、留下不在新日志中的交易。

建议：

- 单写者锁，先验证网络、配置 hash 和 signer，再创建或恢复同一份 journal。
- 发交易前持久记录步骤、nonce、to/data/value、预期地址和交易身份；receipt 后追加确认结果。
- 原子写入并 fsync，重跑从 journal 和 canonical receipt 对账；不只依据 DAO getter 判断已完成。
- 处理 pending、replacement、dropped、receipt reorg；所有历史成功操作保留，失败重试不覆写审计证据。

### 3.3 P1：strict 验收未精确验证分配，也未绑定受审代码

`usdb_validate_bootstrap.ts:633` 的 DevToken strict 检查比较 name/symbol、总量、总释放量，**没有查询
初始地址的 balanceOf**。本轮隔离 getter 替身给出正确总量、十个初始地址余额均为 0，仍通过该模块检查。
这是 validator 覆盖探针，不是对真实链已发生错误分配的判断。

其他覆盖缺口包括：

- `ensureCode` 只检查非空代码；未比较 proxy 和 implementation runtime hash、ERC-1967 implementation
  slot、受审 artifact/golden digest，以及完整 DAO 反向绑定。
- Acquired 检查几个 getter 能否调用，未精确证明私有 `investmentCount` 的初值。
- 多次 RPC 读取未固定同一 block tag；summary 没有观察区块 hash/state root。
- `ModuleIdentity` 只有 address/version。已有 acceptance 确实绑定 config/state、交易集合和 checkpoint
  block hash/state root，但没有证明该 checkpoint 上的模块就是经审阅的实现，不能把它说成完全没有状态承诺。

建议 strict 验收固定 H/hash/root，逐地址检查分配和储备余额、decimals、模块正反向绑定、所有初始 cursor、
里程碑状态、proxy/implementation/code identity 和 finalized slot；在结束前复核 H 未重组。
将这些规范化结果纳入 acceptance commitment，joiner 独立重算。
这与既有 `USDB-AUDIT-007` 的修复方向一致。

### 3.4 P1：公开首节点流程未收敛为完整 candidate ceremony

`Dao.initialize()` 将首次调用者设为 admin；`Dividend.initialize()` 的参数也由首次调用决定。
这属于 UIP-0010 v1 明确采用的受控初始化模型，不能只靠脚本检查 signer 来隔离公开网络中的其他交易。

当前 node-kit 的 `--first-node` 是首节点声明，不是入站隔离：Compose 默认映射公网 P2P，空 bootnodes
不能阻止别人主动连接并传播交易。当前发布手册中的独立签名机与 SSH tunnel 是可保留的措施，但还需要
显式的 candidate 网络隔离和公开启用门禁。

已有 Go `usdb-bootstrap-acceptance create/verify` 与签名发布演练工具应复用；当前 node-kit 的
`activate-release` 是软件镜像版本切换，不能当作 UIP-0010 的 public activation。两套流程需要明确接入。

### 3.5 P1：原有治理审计阻塞项仍存在

当前 SourceDAO 仍为安全基线 `66b2f46`，代码复核确认：

- `Committee.endFullPropose` / `_fullProposalVotingPower` 仍按结算时当前余额计算 full proposal 权重，
  对应 `USDB-AUDIT-005`。需要所有投票者共享同一历史快照及固定 denominator / devRatio。
- `Project.acceptProject` 的批准参数未绑定完整 result/contributions，`updateContribute` 仍可在 Accepting
  状态修改，对应 `USDB-AUDIT-006`。
- 本节 3.3 的代码身份问题对应 `USDB-AUDIT-007`。

现有发布安全基线将 005/006/007 列为 testnet/mainnet blocker。把参数改为独立 USDB、设置更长分红周期
或增加委员数量，都不能替代这些修复。受控部署演练与对外开放治理应分别验收。

### 3.6 P1：opcode audit 对当前 Hardhat 产物空跑

`scripts/audit_usdb_bytecode.mjs:77` 只读取 `artifact.deployedBytecode.object` 或 `artifact.bytecode.object`，
当前 `artifacts-usdb` 使用字符串 bytecode。审计循环遇到无法解析的 bytecode 会直接 continue，但成功日志
打印的是文件总数，不是实际检查的 runtime 数量。

本轮实测：42 个 JSON artifact，当前解析分支识别到 **0 个 bytecode**，命令仍返回成功。
负例中 `deployedBytecode: "0x5c00"` 含禁用 TLOAD，却返回 exit 0；同样内容改为 `{object: "0x5c00"}`
才返回 exit 1。已有“42 artifact passed”不能作为 opcode 兼容性验收证据。

建议兼容实际 Hardhat artifact 形状，区分接口空代码与解析失败，输出扫描/跳过数量；必需生产 runtime 未被
扫描时失败。对真实构建形状加入禁用 opcode 负例，并正确处理 Solidity metadata 与 PUSH 数据段。
8 个合约 golden 匹配仍是独立有效的完整性检查，但不替代修复后的 opcode audit。

### 3.7 部署预算与权限交接仍需显式实现

当前 fresh full bootstrap 通常涉及 22 笔顺序交易：DAO/Dividend 初始化与 wiring 3 笔，六个模块各
implementation/proxy/wiring 共 18 笔，finalize 1 笔。每笔默认等待 receipt，缺少显式的总预算、截止高度、
交易超时与确认深度策略。

建议在 miner 启用前生成完整计划，核对 genesis/hash、预置 code hash、signer 可用性和余额、gas limit、
手续费上限及距离 8192 的余量。运行中按真实出块速度和剩余步骤告警；finalize 必须在 8191 及以前入链，
不能等到 8192 再用补发交易恢复。

当前 executor 使用 `ethers.Wallet` 和环境变量原始私钥，不能仅把 admin 地址改为多签地址就获得多签支持。
应增加 signer 接口或离线签名流程，并分别定义执行 signer 与最终权限接收者。否则 transferBootstrapAdmin
之后，现有 strict validator 仍按单一 bootstrapAdminAddress 比较，会拒绝合法交接。

## 4. 建议部署流程

以下是待实现的职责划分，不表示现有 CLI 已经提供这些命令：

| 阶段 | 产物与通过条件 |
| --- | --- |
| 参数冻结 / prepare | USDB 独立参数、三仓 revision、完整 artifact hashes、签名职责、操作计划和预算；无需私钥即可 review |
| 隔离 candidate / mine | 只允许受控节点、矿工和审计端访问 P2P/RPC；验证网络身份、首块和上游依赖 |
| bootstrap / resume | 独立签名端执行唯一 journal；先检查全部 artifacts，再初始化与部署；不中途编译或重新 npm install |
| 严格复检 / verify | 固定完成高度 H，在同一 H 核对代码、storage、wiring、分配和 signer；保存规范化报告 |
| checkpoint / accept | H 后等待明确非零确认深度；检查 H 前交易集合；创建 acceptance 并纳入签名 release manifest |
| 独立复核 / joiner | 使用另一节点从 genesis 验证到 H，重算验收 identity，确认 checkpoint 和签名 release |
| 对外开放 / activate | 验收凭证齐备后才开放公共 P2P/RPC，记录长期权限归属 |
| 常态运维 | 普通 keeper 负责 Dividend 记账/周期监控；节点 Docker 重启不重跑部署；持续区分业务 readiness 与进程存活 |

复用“逐模块 UUPS 代理 + constructor 中携带 initializer calldata”的现有做法；该初始化是原子的，
不建议把所有模块塞进一笔巨型交易。改进重点是计划与 artifact 冻结、可恢复 journal、严格验收和开放门禁。

配置保持三层：公开不可变参数；本地 RPC/路径/超时；独立 signer。普通 node controller 不持有 bootstrap
私钥。可由 `usdb-node` 提供只读检查和进度入口，签名操作由受控部署端执行。

确认深度没有普遍适用的正式网常数，应按候选网络的算力、出块和重组风险冻结；公共网络不能沿用 CLI 默认
`min-confirmations=0`。创建 acceptance 时应显式指定 H，不能每次用 latest 又要求它立即拥有确认。

业务探针、管理员交接等交易必须安排清楚：H 前的交易全部进入授权 journal；质押、转账、领取等业务验收
优先在演练链执行，或放在 H 后执行，避免破坏“checkpoint 前交易集合精确匹配”的约束。

## 5. 需要补齐的验收用例

- 直接使用不含 rpcUrl 的公开 bundle config，CLI / 环境注入 RPC 后正常进入预检；缺失 RPC 则早期失败。
- 错误网络、signer、genesis、artifact、余额或截止高度：零交易，且已有 journal 字节不变。
- implementation/proxy/wiring/finalize 的发送前、广播后、receipt 后强制中断；重跑没有重复部署或交易证据缺失。
- 相同总量但错误接收地址、错误储备余额、错误 decimals / DAO 绑定 / cursor，strict 必须拒绝。
- 同 version 的错误实现、错误 proxy runtime 或 implementation slot，strict 与 acceptance 必须拒绝。
- Hardhat 字符串和 object 两种 artifact 形状中的禁用 opcode 均被拒绝；不能把零扫描当通过。
- 验收过程中 head 前进、checkpoint 重组、历史 state 不可用：不混用 latest，不接受拼接状态。
- 受控 candidate 混入未授权交易、首次初始化被其他地址抢占：拒绝接受该 candidate。
- final admin 交接、restart、fresh joiner、accepted checkpoint 签名及篡改拒绝。
- 真实 geth 上跨 fee gate 核对 60% miner fee / 40% DAO fee、原生账本同步和周期推进；完成质押/领取闭环。
- 现有治理审计修复的对抗性测试，以及合约变更后的 build/audit/golden/Slither 与完整部署验收。

## 6. 本轮验证证据与限制

- 两条公开配置命令已直接复现 `Missing required bootstrap config field: rpcUrl`；未提供私钥、未连接节点。
- 隔离函数探针复现错误网络预检覆盖旧 journal，以及 strict DevToken 不检查逐地址分配。
  探针执行实际 TS 脚本函数，替换文件/RPC边界，不能替代真实 EVM 部署验收。
- 8 个生产合约 golden 匹配。opcode audit 虽然返回成功，但已复现 42 个 artifact 实际零扫描，以及字符串
  形状的禁用 TLOAD 负例误通过；此项资格检查不能记为有效通过。
- 本轮未重编译合约，未执行完整 Hardhat/Slither 或真实 bootstrap/restart/joiner 验收。
- 本轮新增的只有本文和目录链接；没有修改业务合约、canonical config、miner 配置或运行中的服务。

本机复核证据保存在 `/tmp/sourcedao-bootstrap-config-preflight-review.log`、
`/tmp/sourcedao-validator-config-preflight-review.log`、`/tmp/sourcedao-bootstrap-review-probes.cjs` 和
`/tmp/sourcedao-bootstrap-review-probes.json`、`/tmp/sourcedao-opcode-audit-review.json`；这些临时文件不是发布 artifact。

## 7. 下一轮冻结所需输入

需要明确 USDB DAO 的代币名称/symbol、总量、初始分配表及锁仓规则、独立委员会地址、开发者投票倍率、
USDB 项目里程碑、测试网 bootstrap signer 是否保留、长期 admin custody 和权限交接方式。
上述业务决策不能由脚本根据旧 BuckyOS 配置自动推断。

参数变更需要生成新的候选 bundle 并更新 hash、兼容契约和发布输入，不能直接编辑已安装的冻结 JSON。
若测试机仍在 block 0 且尚未执行 bootstrap，可先规划候选更新；若已经出块或初始化，需按网络身份规则评估
新 generation。Bitcoin 与兼容的 balance-history / indexer 数据应继续保留，不能把 DAO 参数重设当作
重新同步 Bitcoin 的理由。具体动作要以操作前读取的真实状态和 compatibility contract 为准。

参考：[UIP-0010](../UIP/UIP-0010-source-dao-dividend-bootstrap.md)、
[参数冻结清单](./usdb-testnet-v0-parameter-freeze.md)、
[首节点运维](./usdb-testnet-v0-first-node-operations.md)、
[SourceDAO 安全基线](./security-findings/sourcedao-stage-b-baseline-2026-09-04.md)。

## 工具改进跟进

本批落实工具和验收改进；其后确认的测试网源链分配及委员会导入见下一节，其余独立 DAO 参数仍待冻结：

- bootstrap/validator 先解析 CLI/RPC override，拒绝重复 JSON key 和未知参数。
- bootstrap 必须提供 state file，交易广播前持久化 signed bytes/nonce/CREATE 地址；通过锁与原子 fsync
  写入保证可恢复。重跑保留成功交易，已完成 state 保持原字节；缺失旧 journal、未知替换或重组停止处理。
- strict validator 固定 H，核对每个初始 holder、reserve、decimals、反向绑定、计数器、proxy 与实际
  implementation runtime，并输出可重放的 v2 evidence。
- Go acceptance v2 绑定同一个 H、配置与 reviewed golden；要求本地 `--contract-golden`，独立比对
  runtime/immutable、重放 code/storage/call，保留精确 transaction-set 与确认深度约束。
- opcode audit 支持 Hardhat flat string / solc object 格式，区分 PUSH data / Solidity CBOR metadata；
  实际扫描 29 份 runtime，11 份空接口/抽象合约单独计数。Golden 覆盖 8 个业务合约及 ERC1967Proxy。
- 两节点验收脚本把 restart/joiner 读取固定到接受的 H，幂等重跑同时携带 state 与 journal。
- Docker bootstrap runner 的前置错误写到 runner-status 文件，避免覆盖 full-bootstrap 交易证据。

操作与恢复细节见 [SourceDAO bootstrap 工具说明](../../../SourceDAO/docs/usdb-bootstrap-tools.md) 和
[首节点操作流程](./usdb-testnet-v0-first-node-operations.md)。

已完成本地 contract fast gate：266 个合约测试、build、opcode audit 和 golden 校验通过；工具回归覆盖
5 个强杀恢复点、22 笔交易幂等、错误 chain ID、分配变更、历史 checkpoint、错误代码、丢包、nonce 替换
和重组。Go 单测覆盖证据绑定、独立 RPC 重放、pruned state、错误代码与 immutable。

这些改进不改变 Solidity 治理逻辑；USDB-AUDIT-005/006、正式参数冻结和受控 candidate ceremony
仍是独立的发布条件。真实 miner 吞吐和线上部署不在本批工具验收范围。


完整 geth 双节点演练也已通过（本地 fake PoW / mock indexer；非线上发布）：

| 项目 | 结果 |
| --- | --- |
| bootstrap | 22 笔原交易，幂等重跑不新增交易，完成 state 字节完全一致 |
| acceptance | v2，H=51，确认深度 3；创建、独立验证和篡改拒绝通过 |
| 固定 H 的证据 | 14 个 code hash、17 个 storage word、65 个 call；joiner 与首节点完全一致 |
| 生命周期 | 签名 manifest 验证、费用分配/Dividend 探针、首节点重启、新 archive joiner、历史 proof 全部通过 |

本地完整演练使用 gate=96、1 秒 fake-PoW 区块以覆盖激活边界；这些仅为临时测试参数，未修改发布配置。
快速 fake-PoW 会产生超前时间戳，不用于本轮最终 joiner 验收。固定 H 的复检要求保留历史 state，
所以完整双节点驱动已要求两节点使用 archive；普通 full 节点剪枝后的 `missing trie node` 会正确导致验收失败。

结果日志：`/tmp/usdb-bootstrap-tools-e2e.log`，证据目录：
`/tmp/usdb-bootstrap-tools-e2e-pn6cs2bg`。contract fast gate 日志为
`/tmp/sourcedao-bootstrap-tools-fast.log`，最终工具回归为
`/tmp/sourcedao-bootstrap-tools-regressions.log`。临时日志只作本地验收证据，不是签名发布 artifact。

## 测试网源链参数导入

用户已确认测试网复用源合约**最初初始化时的代币分配**，委员会读取源链当前成员。
本次公开 RPC 读取确认主合约 `0x2fc3186176B80EA829A7952b874F36f7cb8bd184` 位于
Optimism（chain ID 10），与提供的 Optimism 浏览器链接和本地 `opmain` profile 一致。
它不是历史脚本中另一组 X Layer 部署地址。

固定读取区块为 `156576688`，哈希
`0x0c328c37a16921cd83e24aac59687b236e177cff1628695ae110db76ddafb909`。
主合约的全部 7 个模块绑定已核对，委员会成员为：

- `0x2514d2FEAAC3bFD8361333d1341dC8823595f744`
- `0x2DFD1FCFC9601E7De871b0BbcBCbB6Cad6901697`
- `0xad82A5fb394a525835A3a6DC34C1843e19160CFA`
- `0xdc7dD66eafdBf4B2e40CbC7bEb93f732f8F86518`
- `0x0F56a6f7662B38506f7Ad0ad0cc952b79b8e90e7`

后两名是相对本地旧配置新增的成员。BDDT 原始分配来自区块 `138178247` 的部署交易
`0xf948fb6513f4ff6a00afe74670192de66078ab1a13ea6577f60b35043b3091fd`；BDT 来自区块
`138178256` 的部署交易 `0x03b970f46d0633d7b10e7d6a7d0c5764cca01e2b481a733954b2ad7f55725b19`。
初始化事件、铸币日志与部署区块历史余额对齐：BDDT 总量为 `2100000000`，10 个外部分配地址
合计 `146999999.999999999999999994`，自持储备为 `1953000000.000000000000000006`；
BDT 初始总量为 `0`。外部分配表与本地旧配置逐项一致。

自动导入工具及中文操作说明位于 SourceDAO 仓库的 `scripts/usdb_import_bootstrap_source.ts` 和
`docs/usdb-bootstrap-tools.md`。候选配置与来源报告分别为该仓库中的
`tools/config/imported/usdb-testnet-v0.opmain-156576688.json` 和同名 `.source.json` 文件。
候选仅更新委员会、DevToken 总量与分配表，保留基础配置中的其他字段；它未替换本仓库已发布的
`docker/networks/testnet-v0/artifacts/sourcedao-bootstrap-config.json`，也未修改测试机或发交易。

独立 DAO 命名、项目里程碑、治理倍率、周期、业务计数器与签名权限仍需按第 7 节冻结。
源链读取报告不是目标链初始化验收文件；采用新候选配置时需要重新构建发布输入，并在目标链完成
strict 检查和 v2 acceptance。源链持有人需要掌握相应地址的签名权限才能在测试网操作。

本次 TypeScript 检查与 `npm run test:usdb:tools` 的 7 组测试全部通过；最终版导入工具在相同源链高度
重复读取，候选配置和来源报告的字节内容均完全一致。此验证只覆盖导入工具和初始化工具回归，
不代表其余独立治理审计问题已经解决。
