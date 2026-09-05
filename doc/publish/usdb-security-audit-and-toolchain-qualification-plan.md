# USDB 安全审计与 Go 工具链资格验证计划

## 1. 文档目的

本文定义 USDB 在测试网上线、运行和后续主网上线前的分阶段安全工作路线。它覆盖：

- USDB 自主实现的 Rust 服务、工具和发布链；
- SourceDAO 合约、构建链和 bootstrap 流程；
- `go-ethereum` 中 USDB 新增的共识集成；
- 从 ETHW 继承的代码、依赖和 Go 发布工具链。

本计划不把“发现旧依赖”直接等同于“立即整体升级”。安全修复必须同时考虑漏洞可达性、
网络暴露面、共识兼容性、历史重放结果和部署风险。

## 2. 已冻结的决策

1. 先审计项目完全可控的非 ETHW 代码，再处理 `go-ethereum` 的 USDB 增量和 ETHW
   继承基线。
2. Go 工具链升级与 Go module 依赖升级分开进行，首轮工具链资格验证不得主动修改
   `go.mod` 或 `go.sum`。
3. 可重置的 `testnet-v0` 可以在明确风险接受、限制网络暴露面和设置失效日期后，暂时使用
   Go 1.18.5 发布工具链。
4. Go 1.18.5 对 public mainnet 仍是发布阻断项；除非完成受支持工具链升级，或维护并验证
   一套具有等价安全修复的显式 backport 工具链。
5. 已确认可从公网利用的 Critical/High 问题，无论位于哪个阶段，立即成为 testnet 和
   mainnet 发布阻断项。
6. 任何共识、数据库或网络兼容性结论都必须由测试证明，不能只从“编译成功”或 ETHW
   历史运行时间推断。

## 3. 审计域与执行顺序

| 阶段 | 审计域 | 主要内容 | 升级策略 |
| --- | --- | --- | --- |
| A | USDB Rust 服务和工具 | `balance-history`、`usdb-indexer`、control-plane、CLI、snapshot/checkpoint、RPC 和发布脚本 | 可直接修复；按功能小批次升级 |
| B | SourceDAO | Solidity 合约、权限、bootstrap、artifact、Hardhat 和脚本依赖 | 源码审计与构建链升级分开 |
| C | Go 中的 USDB 增量 | `internal/usdb`、`core/usdbstate`、USDB reward/fee/genesis/miner/verifier 和发布脚本 | 以共识差分测试为门禁 |
| D | ETHW 继承基线和 Go 工具链 | 继承的 P2P、数据库、RPC、EVM、Ethash、Go 标准库和 modules | 双工具链、双二进制资格验证 |
| E | Release artifact | 三类镜像、SBOM、操作系统包、签名、manifest 和节点部署暴露面 | 对最终 digest 重新扫描 |

阶段 A 和 B 可以并行。阶段 C 应在 A/B 的高风险问题收敛后开始。阶段 D 必须保留当前
ETHW/Go 1.18.5 二进制作为差分基线，不得与大批依赖升级同时进行。阶段 E 在每个 release
candidate 上执行，而不是只在源码层执行一次。

## 4. 统一发现记录

每个发现至少记录以下字段：

| 字段 | 说明 |
| --- | --- |
| `finding_id` | 稳定的项目内编号 |
| `source` | 人工审计、CodeQL、govulncheck、cargo-audit、npm audit、fuzz 或外部报告 |
| `component` | 受影响服务、合约、二进制或镜像 |
| `introduced_by` | USDB 自主代码、USDB Go 增量、ETHW 继承代码或第三方依赖 |
| `reachability` | confirmed、likely、unknown 或 unreachable |
| `exposure` | public network、operator RPC、local input、build input 或 test-only |
| `impact` | 共识分叉、资产/权限、远程执行、DoS、数据损坏、信息泄露或供应链 |
| `decision` | fix、upgrade、replace、mitigate、accept-temporarily 或 false-positive |
| `evidence` | 调用链、复现、测试、配置边界或最终二进制扫描结果 |
| `owner` | 负责人 |
| `expires_at` | 临时接受的强制失效日期 |
| `release_gate` | testnet、mainnet、both 或 none |

禁止使用永久的 package-wide ignore。`unreachable` 和 `false-positive` 必须附证据，并在输入、
feature、target 或构建方式变化后重新检查。

## 5. 阶段 A：USDB Rust 服务与工具

### 5.1 依赖与构建面

- 以 `src/btc/Cargo.lock` 为唯一 Rust workspace inventory；
- 使用 target/feature-aware dependency tree 判断 advisory 是否进入 Linux release binary；
- 优先处理处于 HTTP/RPC 活跃路径的 `h2`、`bytes` 等可达问题；
- 分别检查服务 binary、CLI、浏览器应用和 release image，不能用 workspace 扫描代替最终
  artifact 扫描；
- 每批只升级一个直接依赖或一组强耦合依赖，并重新运行 workspace、live/regtest 和容量测试。

### 5.2 `balance-history`

重点审计：

- Bitcoin RPC 返回值、区块/交易解析和异常资源消耗；
- reorg、stable lag、精确高度和 state-ref 不变量；
- snapshot/checkpoint 的签名、哈希、网络身份、高度和 schema 校验；
- 下载断点续传、HTTP Range、staging install、路径穿越、符号链接和故障恢复；
- 数据库损坏、磁盘耗尽、OOM、重启和只读预检边界；
- RPC limit、cursor、历史 context 和大查询的内存上限。

最低测试包括 parser/fuzz corpus、签名与 manifest 篡改、截断文件、错误网络/高度、重组恢复、
资源上限和 snapshot 安装故障注入。

### 5.3 `usdb-indexer` 与 control-plane

重点审计：

- inscription 严格 JSON 解析、重复 key、字段大小和嵌套深度；
- pass/energy/effective-energy 的整数边界、饱和计算和状态机不变量；
- leader 解析、candidate/breakdown/profile 的历史高度一致性；
- same-height 与 multi-block reorg、cursor replay 和 external state 校验；
- RPC 认证边界、分页上限、批量查询和拒绝服务风险；
- control-plane 的命令构造、密钥输入、mint payload 和错误日志脱敏。

最低测试包括属性测试、跨实现 golden vector、无效输入 corpus、100K 数据容量、并发分页、
reorg/restart 和真实服务链路回归。

### 5.4 发布和节点工具

重点审计 installer、`usdb-node`、Compose、release manifest 和 GitHub Actions：

- 所有下载内容在执行或安装前验证 digest/签名；
- archive 解包和 bind-mount 不能逃逸目标目录；
- shell 参数不能形成命令注入；
- secret 不写入日志、artifact、image layer 或 release bundle；
- operator API 默认 loopback-only，公开端口采用 allowlist；
- workflow/action、基础镜像和发布镜像使用不可移动 revision 或 digest。

## 6. 阶段 B：SourceDAO

### 6.1 合约源码

重点审计：

- initializer、bootstrap admin、委员会和 module address 的权限边界；
- genesis 预置身份、accepted bootstrap hash、runtime code hash 和 marker；
- 重入、外部调用顺序、授权绕过、整数/舍入、重复初始化和拒绝服务；
- Dividend fee 分配、未初始化窗口和错误 module wiring；
- 固定 system address、storage slot 和 bytecode golden vector。

### 6.2 构建与 bootstrap 供应链

- 完整 npm tree 属于发布构建链，即使生产 runtime dependency 为零也不能忽略；
- Hardhat 或编译器升级必须比较 bytecode、ABI、storage layout 和 bootstrap transcript；
- 升级前后必须从相同输入生成可解释的 artifact 差异；
- bootstrap restart/joiner、错误管理员、抢跑模拟和最终链上状态验收必须自动化。

合约源码问题与 Node/Hardhat 依赖问题分别修复，避免无法判断 bytecode 变化来源。

当前首轮基线见 [SourceDAO Stage B 安全基线](./security-findings/sourcedao-stage-b-baseline-2026-09-04.md)。
Slither 处于 report-only 阶段；USDB production contract 的 compiler settings、ABI、method selectors、
bytecode 和 storage layout 已进入确定性 golden gate。已确认的治理与资产记账问题必须按独立
artifact-changing batch 修复，不能与 Hardhat/npm 升级合并。

## 7. 阶段 C：Go 中的 USDB 增量

先建立相对于选定 ETHW upstream revision 的 delta inventory，再集中审计：

- BTC profile payload 的 codec、版本、chain scope、state-ref 和字段篡改；
- miner 与 validator 对同一 resolved profile、difficulty、reward 和 fee 的消费一致性；
- 外部 BTC 服务不可用、超时、stale state 和 same-height replacement 时的 fail-closed 行为；
- genesis/bootstrap 配置、system storage、activation registry 和历史 revision replay；
- miner address 到 active pass 的动态解析，以及 consume/remint/outage recovery；
- block import、reorg、restart、joiner 和历史区块重放。

本阶段允许修复 USDB 增量，但不得顺带大规模重构继承的 ETHW 子系统。

## 8. 阶段 D：ETHW 基线和 Go 工具链

### 8.1 双轨策略

`baseline` 轨继续使用冻结的 ETHW/Go 1.18.5 源码、工具链和镜像 digest。`candidate` 轨使用
当前受支持 Go major line 的最新安全 patch，并在第一轮保持 `go.mod`/`go.sum` 不变。

现有 compatibility lane 只能作为起点。候选 release 必须：

1. 移除或替换依赖私有 runtime symbol 的 `github.com/fjl/memsize` 调试能力；
2. 不使用 `-checklinkname=0` 绕过现代 linker 校验；
3. 从同一 source commit 分别生成 baseline/candidate 二进制；
4. 保存工具链版本、源码 revision、build flags、SBOM 和二进制 digest。

### 8.2 资格验证矩阵

- genesis、chain config 和 activation registry roundtrip；
- 历史区块导入及 state root、receipt root、reward、system storage 逐块一致；
- old miner -> new validator 与 new miner -> old validator 双向接受；
- same-height/multi-block reorg、restart、late joiner 和数据库重开；
- profile、difficulty、reward、fee、bootstrap 和 quote/aux inactive/fake activation；
- P2P/RPC 错误边界、畸形输入和连接压力；
- CPU、内存、GC、数据库 I/O、PoW 性能和长时间 soak；
- release manifest 驱动的三节点 digest-pinned E2E。

“可以编译”“单元测试通过”或“单节点继续同步”均不足以完成资格验证。

### 8.3 Canary 顺序

1. candidate 先作为不出块 validator 加入 baseline 网络；
2. 对比 head、state root、USDB system state 和历史查询；
3. 再启用一个 candidate miner，由 baseline validator 验证其区块；
4. 观察完整 reorg/restart/soak 周期后，才允许替换 canonical release toolchain。

若发现共识、数据库或网络语义差异，停止晋级。测试网可以重置，但不能把重置能力当成忽略差异的
理由；public mainnet 不允许依赖重置恢复。

### 8.4 继承依赖升级

工具链资格验证完成后，再按网络、加密、数据库、RPC、EVM 和构建工具分组升级 Go modules。
每组必须单独提交、扫描和运行相关兼容矩阵。不得使用一次性全量升级作为安全修复策略。

## 9. Release 门禁

安全扫描的 CI 状态和 release 决策必须分离。一个独立安全扫描任务显示 warning 或 failure，
不应在基线阶段自动中断 Fast、Nightly、Weekly 或普通功能开发；但 release reviewer 仍需根据
本节规则审查可达性和例外记录。

### 9.1 CI 门禁演进

采用三个阶段逐步收紧：

| 阶段 | CI 行为 | 分支/发布行为 |
| --- | --- | --- |
| Baseline/report-only | 独立 workflow 在依赖清单变化、定时或手工触发时生成报告；发现漏洞不使任务失败，扫描器、网络、报告格式或 artifact 失败仍应报错 | 不设为 Fast/Nightly 或普通 PR 的 required check；不自动合并 Dependabot PR |
| Incremental | 对现有已分类基线做差分，只拒绝新增的 Critical/High；先观察运行，再限制到依赖或安全相关 PR | 现有接受项按 owner/expiry 管理，不因为历史数量阻塞并行功能开发 |
| Strict release | 检查源码、最终 binary/image、SBOM、例外到期和 release manifest | public-mainnet candidate 必须通过；根据稳定性再纳入受保护分支 required checks |

从 Baseline 进入 Incremental 前，必须完成初始 Critical/High 分类并稳定运行至少两个定时周期。
从 Incremental 进入 Strict release 前，必须完成阶段 A-C 的高风险收敛、最终镜像扫描和例外审批
自动校验。不得仅因为打开了 GitHub 安全功能，就一次性把全部历史告警配置成合并阻断。

### 9.2 `testnet-v0`

允许带限期例外上线，但必须满足：

- 无已确认公网可达的 Critical/High 未修复问题；
- operator HTTP/WS、debug、metrics 和 Engine API 不对公网开放；
- 使用 immutable source revision、toolchain 和 image digest；
- 不承载不可恢复的真实资产或正式 signer；
- Go 1.18.5 例外记录 owner、理由、补偿措施、到期日和退出计划；
- reset/rollback 操作手册经过演练。

### 9.3 Public mainnet

除满足 testnet 条件外，还必须：

- 使用受支持且完成资格验证的 Go patch release，或经过等价验证的 backport 工具链；
- 清零所有公网可达和共识/资产相关的 Critical/High；
- 所有暂时接受项未过期并经过发布审批；
- 最终镜像扫描、SBOM、provenance、签名和 release manifest 完整；
- 至少完成一次真实 release artifact 的多节点升级、回退和历史重放演练。

## 10. 输出物

每个阶段应产生：

- scope/threat-boundary 清单；
- 机器可读扫描报告和人工 finding registry；
- 修复或限期接受记录；
- 新增的回归、fuzz、容量或差分测试；
- 测试报告及对应 source/image digest；
- release-note fragment 或明确的 `Release-Note: none`；
- 阶段完成报告和剩余风险。

安全报告不得只记录 advisory 数量；必须能回答“哪个发布 artifact、通过什么输入、是否可达、
由谁在何时处理”。

## 11. 首批执行清单

1. 修正当前依赖安全基线中的 testnet/mainnet 工具链风险口径。
2. 为阶段 A/B 建立 finding registry 模板和组件 owner。
3. 对 Rust workspace 生成 target/feature-aware 依赖路径，优先验证并修复活跃 HTTP/RPC 路径。
4. 分别审计 balance-history snapshot/checkpoint、indexer parser/state machine 和节点发布工具。
5. 对 SourceDAO 执行合约权限/初始化审计，并建立 Hardhat 升级前 bytecode/storage golden baseline。
6. 阶段 A/B 的 Critical/High 收敛后，生成 Go USDB delta inventory，进入阶段 C。
7. 最后扩展现代 Go compatibility lane，执行阶段 D 的完整资格验证。

## 12. 关联文档

- [USDB 依赖安全策略](./usdb-dependency-security-policy.md)
- [USDB 依赖安全基线（2026-09-04）](./usdb-dependency-security-baseline-2026-09-04.md)
- [USDB Release 总体流程](./usdb-release-process.md)
- [USDB Release 变更记录与 Changelog 管理](./usdb-release-change-management.md)
