# SourceDAO Bootstrap Job Plan

## 1. 背景

在当前 USDB × ETHW 的冷启动设计里：

- `Dao`
- `Dividend`

需要因为链级固定地址和手续费分账约束，走 **genesis 预置 code + 运行后初始化** 的路径。

但 SourceDAO 的其余主模块：

- `Committee`
- `Project`
- `DevToken`
- `NormalToken`
- `TokenLockup`
- `Acquired`

并不要求在 genesis 中就固定地址，也不要求在 block 0 就具备被动收款能力。

因此，当前更合理的整体启动模型应拆成两层：

1. **链级 cold-start**
- ETHW 网络启动
- `Dao` / `Dividend` 在固定系统地址存在
- 最小 USDB / ETHW 联动服务启动

2. **应用级 SourceDAO bootstrap**
- 在链已启动、RPC ready 后
- 由管理员自动部署其余 SourceDAO 合约
- 完成必要初始化与 `Dao` 注册

这第二层就是本文件定义的 `sourcedao-bootstrap` one-shot job。

## 2. 目标与非目标

### 2.1 目标

`sourcedao-bootstrap` 的目标是：

- 连接已启动的最小 ETHW 网络
- 初始化 genesis 预置的 `Dao` / `Dividend`
- 部署 SourceDAO 其余主模块
- 调用各模块 `initialize(...)`
- 调用 `Dao.set*Address(...)` 完成主注册表 wiring
- 输出一份可审计的 bootstrap 状态结果

### 2.2 非目标

当前不把下面这些内容放进该 job：

- 生成 ETHW canonical genesis
- 在运行时编译 SourceDAO 合约
- 替代完整的 SourceDAO 治理或业务数据初始化
- 处理后续合约升级

正式环境下，合约编译产物应在镜像构建阶段固定，或以固定 artifact 的形式提供给该 job；不建议在启动时现场编译。

当前已经开始落地一个较小的 stage-one 开发路径，见：

- `doc/sourcedao-bootstrap-dev-artifacts-plan.md`

这个阶段只覆盖：

- `Dao.initialize()`
- `Dividend.initialize(...)`
- `Dao.setTokenDividendAddress(...)`

并复用 `SourceDAO/scripts/usdb_bootstrap_smoke.ts` 做链上验证。其余模块部署仍留在后续阶段。

开发期现在已经继续扩到一个 `full` scope：

- `SourceDAO/scripts/usdb_bootstrap_full.ts`

它会在 `Dao` / `Dividend` 初始化后继续部署并注册：

- `Committee`
- `DevToken`
- `NormalToken`
- `Project`
- `TokenLockup`
- `Acquired`

这仍然是开发期路径，正式发布仍建议消费固定 artifact bundle，而不是依赖工作区源码。

## 3. 与当前冷启动链路的关系

当前推荐顺序是：

1. `bootstrap-init`
- 准备 canonical ETHW genesis artifact
- 准备 `balance-history` snapshot 输入

2. `ethw-init`
- 对本地 `ethw-data` 执行一次 `geth init`

3. `ethw-node`
- 启动最小 ETHW 网络
- 等待 JSON-RPC 就绪

4. `sourcedao-bootstrap`
- 连接 ETHW RPC
- 完成 SourceDAO 的链上 bootstrap

也就是说：

- `bootstrap-init` 负责链级输入准备
- `ethw-init` 负责本地 datadir 初始化
- `sourcedao-bootstrap` 负责 SourceDAO 的链上部署与初始化

这三者不应混成一个容器或一个脚本。

## 4. 为什么只预置 Dao 和 Dividend

当前之所以只要求 `Dao` / `Dividend` 进入 genesis，是因为它们承担了系统级固定地址语义：

- `DividendAddress` 需要作为手续费分账目标地址
- `DaoAddress` 是 `Dividend` 和其余模块的主合约地址引用

而其余模块没有这个强约束：

- 它们可以在链启动后由管理员部署
- 地址可以在部署后再注册进 `Dao`
- 不要求在共识层提前知道地址

因此当前不需要扩展 bootstrap genesis 生成器，把整套 SourceDAO 合约都预置到 genesis 中。

## 5. 需要覆盖的模块

基于当前 `SourceDAO/contracts`，`sourcedao-bootstrap` 至少需要考虑下面这些模块：

- `Dao.sol`
- `Dividend.sol`
- `Committee.sol`
- `Project.sol`
- `DevToken.sol`
- `NormalToken.sol`
- `TokenLockup.sol`
- `Acquired.sol`

其中：

- `Dao` / `Dividend`
  - 地址固定
  - code 来自 genesis 预置
  - 在 bootstrap job 中执行 `initialize(...)`

- 其余模块
  - 由 bootstrap job 在链上部署
  - 再执行 `initialize(...)`
  - 最后注册进 `Dao`

## 6. 输入配置

`sourcedao-bootstrap` 应消费一份独立配置，例如：

- `ethw-bootstrap-config.json`
- `sourcedao-bootstrap-config.json`

建议至少包含：

- ETHW RPC URL
- `expected_chain_id`
- bootstrap admin signer 配置
- `dao_address`
- `dividend_address`
- SourceDAO artifact 根目录或镜像内 artifact 路径
- `dividend.cycle_min_length`
- `committee.initial_members`
- `committee.init_proposal_id`
- `committee.init_dev_ratio`
- `committee.main_project_name`
- `committee.final_version`
- `committee.final_dev_ratio`
- `project.init_project_id_counter`
- `dev_token.name`
- `dev_token.symbol`
- `dev_token.total_supply`
- `dev_token.init_addresses`
- `dev_token.init_amounts`
- `normal_token.name`
- `normal_token.symbol`
- `token_lockup.unlock_project_name`
- `token_lockup.unlock_version`
- `acquired.init_investment_count`
- 可选 `transfer_bootstrap_admin_to`

如果某些模块暂时不希望部署，也可以允许配置里显式关闭，例如：

- `deploy_committee = true|false`
- `deploy_acquired = true|false`

但默认建议是：

- 一次性把主模块 bootstrap 完成

## 7. 推荐执行顺序

推荐按下面顺序执行：

1. 基础链路检查
- 校验 RPC 可用
- 校验 `chain_id`
- 校验 `DaoAddress` / `DividendAddress` 上已有 code
- 校验 bootstrap admin 账户余额足够

2. 初始化 genesis 预置模块
- `Dao.initialize()`
- `Dividend.initialize(cycleMinLength, DaoAddress)`

3. 部署其余模块
- `DevToken`
- `NormalToken`
- `Committee`
- `Project`
- `TokenLockup`
- `Acquired`

4. 初始化其余模块
- `DevToken.initialize(...)`
- `NormalToken.initialize(...)`
- `Committee.initialize(...)`
- `Project.initialize(...)`
- `TokenLockup.initialize(...)`
- `Acquired.initialize(...)`

5. 注册到 `Dao`
- `Dao.setDevTokenAddress(...)`
- `Dao.setNormalTokenAddress(...)`
- `Dao.setCommitteeAddress(...)`
- `Dao.setProjectAddress(...)`
- `Dao.setTokenLockupAddress(...)`
- `Dao.setTokenDividendAddress(DividendAddress)`
- `Dao.setAcquiredAddress(...)`

6. 可选的 bootstrap admin 移交
- `Dao.transferBootstrapAdmin(...)`

7. 结果校验
- 读取 `Dao.devToken()`
- 读取 `Dao.normalToken()`
- 读取 `Dao.committee()`
- 读取 `Dao.project()`
- 读取 `Dao.lockup()`
- 读取 `Dao.dividend()`
- 读取 `Dao.acquired()`
- 校验与预期地址一致

## 8. 幂等与状态记录

`sourcedao-bootstrap` 不应只依赖本地 marker 判断是否完成，主判断依据应是链上状态。

建议分两层：

### 8.1 链上状态是第一信号

对于已完成步骤，优先依据链上状态跳过：

- `Dao` 是否已初始化
- `Dividend` 是否已初始化
- `Dao` 的各模块地址是否已设置

### 8.2 本地 state/marker 是辅助信号

job 完成后建议写一份结果文件，例如：

- `/bootstrap/sourcedao-bootstrap-state.json`

内容至少包括：

- `chain_id`
- `dao_address`
- `dividend_address`
- `dev_token_address`
- `normal_token_address`
- `committee_address`
- `project_address`
- `token_lockup_address`
- `acquired_address`
- 每一步的交易哈希
- `completed_at`

同时可以再写一份简单 marker：

- `/bootstrap/sourcedao-bootstrap.done.json`

但 marker 不是权威状态；它只是为了：

- 避免每次都完整重跑
- 方便运维快速判断已完成

对于 `full` scope，state 文件还应在运行过程中持续更新，而不是只在最后一次性输出。

建议至少补充：

- `status`
  - `running`
  - `completed`
  - `error`
- `current_step`
- 已完成或已跳过的 `operations`
- 当前已识别的模块地址

这样当 `TokenLockup` / `Project` / `Acquired` 这类后半段步骤中途失败时：

- 可以直接从 state 文件看出停在哪一步
- 下一次 resume 能结合链上状态继续收敛
- 外层 wrapper 不应再把更详细的错误状态覆盖成泛化的 `error`

### 8.3 部分失败处理

如果 job 在“合约已部署但尚未注册到 `Dao`”的中间状态失败，本地 marker 不能解决全部问题。

因此 v1 应明确：

- job 需要尽量原子化地完成整条 bootstrap 链路
- 若出现部分失败，应优先根据链上状态与本地 state 文件恢复
- 必要时允许人工介入

## 9. 产物与镜像策略

正式环境推荐：

- SourceDAO 编译产物在镜像构建阶段固定
- `sourcedao-bootstrap` 容器只负责：
  - 读取 ABI / bytecode
  - 发送部署与初始化交易

开发 / `dev-sim` 可以后续单独支持：

- 挂载本地 `SourceDAO` 仓库
- 运行前编译

但这不应成为正式 cold-start 的默认路径。

## 10. 与 Docker 的集成建议

后续 Docker 集成建议新增一个 one-shot service：

- `sourcedao-bootstrap`

它应：

- 依赖 `ethw-node` 已经 RPC ready
- 挂载 bootstrap 配置与固定 SourceDAO artifacts
- 成功后写 state/marker

而业务节点或 smoke 脚本如需依赖完整 SourceDAO，则应显式等待：

- `sourcedao-bootstrap: service_completed_successfully`

## 11. 当前建议结论

当前推荐路线是：

1. 保持 `Dao` / `Dividend` 的 genesis 预置策略不变
2. 不扩展 genesis 生成器去承载整套 SourceDAO 模块
3. 新增 `sourcedao-bootstrap` one-shot job
4. 让它在链启动后自动完成其余 SourceDAO 模块的部署、初始化与注册

这样可以把：

- 链级 cold-start
- SourceDAO 应用级 bootstrap

清晰分层，也更符合当前真实需求。
