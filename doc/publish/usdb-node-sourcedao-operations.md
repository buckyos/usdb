# 使用 usdb-node 初始化 SourceDAO

目标节点使用已安装 release 中的冻结 bundle 和按 digest 固定的 SourceDAO 工具镜像。
不需要 clone SourceDAO、安装 Node/npm、编译合约或设置 `SOURCE_DAO_RELEASE_DIR`。
配置准备、参数审核、`freeze --apply` 在发布前完成；部署时不再修改委员会或初始分配。

## 首节点操作顺序

先按常规流程更新 node kit、激活 release 并启动节点，启用矿工且确认开始出块，再执行：

```bash
usdb-node sourcedao check

# 该文件应由安全渠道放到节点；内容是一行十六进制 Bootstrap Admin 私钥。
# 必须使用冻结配置中的 Bootstrap Admin，不能用矿工地址的私钥替代。
chmod 600 /secure/bootstrap-admin.key
usdb-node sourcedao bootstrap --key-file /secure/bootstrap-admin.key
usdb-node sourcedao status --watch

usdb-node sourcedao export
usdb-node sourcedao validate
```

当前 `usdb-testnet-v0` 的 Bootstrap Admin 为
`0x0b5223FD31cDc1536f31b3627e6D7025b52310c9`。命令使用 bundle 中的值核对签名人，
同时验证连接的 chain ID、genesis、已审核工具 artifact 和链上预部署代码。
RPC 自动取 `node.env` 的 `USDB_HTTP_BIND_ADDRESS` / `USDB_HTTP_BIND_PORT`，通配绑定转换为
主机回环地址；无需单独配置 URL。

`check` 不加载私钥、不创建部署状态；输出管理员余额、初始化状态、最新观察区块和 fee split
激活高度。未出首块、仍在同步或管理员没有 gas 余额时，bootstrap 会拒绝启动。
当前网络必须在高度 **8192** 前完成 `Dividend.finalizeBootstrap()`，应在出块后尽快部署。
余额为正只是一项预检；每笔实际交易还会核对 gas 预算和账户 nonce。
启动命令还会根据 journal 已确认的交易估算剩余区块需求，空间不足时拒绝继续。

`bootstrap` 创建独立的一次性 Docker 任务并立即返回。SSH 断开不会停止部署；
`--wait` 可以在当前终端观察到结束。`status --watch` 只观察任务，Ctrl-C 不终止它。
`export` 和 `validate` 也由 Docker 执行，命令默认等到结果出来；SSH 断开后仍可用 `status` 观察。
这不是主服务 controller 的阶段，`up` 不会自动签名或部署 SourceDAO。

所有子命令支持 `--json`。`status` 分别报告容器结果、私有 bootstrap 阶段和新读取的链上
`finalized`，不会仅凭状态文件存在显示成功。RPC 观察失败会明确显示，正在运行的任务仍继续观察。
`SUCCEEDED` 表示最近一次任务完成；`validate` 的报告另行记录固定区块的严格验证证据。

## 怎样判断部署进度

`check` 和 `status` 的文本输出分别列出任务结果、`Deployment` 阶段、DAO 初始化标记和
Dividend 完成标记。`finalized=False` 本身只说明 Dividend 尚未完成 bootstrap：

| Deployment | 含义 |
| --- | --- |
| `NOT_STARTED` | DAO 尚未初始化，本机没有已记录的部署进度，也没有运行中的 bootstrap 任务 |
| `STARTING` | 本机已启动 bootstrap，正在做预检或准备首笔交易 |
| `DEPLOYING` | bootstrap 正在运行，DAO 已初始化或已有交易记录 |
| `FINALIZING` | journal 已记录 `Dividend.finalizeBootstrap` 交易，正在等待回执 |
| `INCOMPLETE` | DAO 已初始化或存在恢复记录，但没有运行中的本机 bootstrap 任务；结合任务错误检查原记录 |
| `FINALIZED` | 最新链上观察显示 Dividend 完成标记已设置；仍需确认 bootstrap 任务成功，再执行 export 和 validate |
| `UNKNOWN` | 缺少新鲜链上观察，或本机记录不可读／身份不匹配；不能据此判断未开始或已完成 |

例如，`SourceDAO | bootstrap | RUNNING` 和 `Deployment: FINALIZED` 可以同时出现：完成交易
已上链，但工具还在做最后核对、落盘或退出。`SourceDAO | validate | SUCCEEDED` 则表示最近的
严格验证任务成功，不是另一次部署。`check` 的 `CHECKED` 表示预检已执行，需继续看阶段和阻塞项。

有匹配的私有 journal 时，面板显示已确认交易数、待回执交易的具体操作名、nonce、交易哈希，
以及最近一次确认的操作和区块高度。比如 `Step: Acquired` 表示模块阶段，
`Waiting: Acquired.deployProxy` 才是当前待确认的具体交易。这些信息只从 journal 中提取白名单
元数据，不展示已签名交易字节。`confirmed` 指 journal 已记录回执，观察命令不会重新验证每笔
历史交易的主链归属；`awaiting_receipt` 也不证明交易已进入矿工的交易池，记录可能刚刚持久化、
尚未广播。断线恢复和严格验收仍由原工具处理。

支持 ANSI 的终端中，`status --watch` 原地刷新固定面板，长文件路径在结束后显示。
重定向到文件或不支持刷新的终端中，每次观察都有时间戳和分隔线，路径只在首末帧打印。
`--json` 保留结构化输出，新增 `deployment` 和经过白名单筛选的交易进度，不插入终端控制符。
Ctrl-C 会恢复终端并保留最后一次观察结果，后台任务继续执行。

节点总览 `usdb-node status --watch` 另列最新 USDB 区块高度和哈希。高度与哈希取自同一次
`eth_getBlockByNumber("latest")` 响应，同高度发生重组时也会更新哈希。宽度不足时仅缩短文本
面板中的哈希，`status --progress-json` 的 chain 组件 `head` 保留完整值；它不是最终性证明。

## 部署交易与出块的关系

当前完整初始化共 **22 笔交易**：DAO 与 Dividend 初始化 2 笔，DAO 绑定 Dividend 1 笔，
6 个模块各自部署实现合约、部署并初始化代理、写入 DAO 绑定，共 18 笔，最后
`Dividend.finalizeBootstrap` 1 笔。

当前工具串行提交：先持久化已签交易，广播并等待 1 个确认的成功回执，核对回执所在区块，
然后才开始下一笔。因而新部署通常至少跨 22 个出块高度；合约读取、代码核对、状态落盘等
只读或本地步骤不需要新区块，一次模块阶段也可能包含 3 笔交易。重试会复用已完成的交易。

链本身支持一个区块容纳多笔交易，但当前工具没有批量流水线。未来可以按依赖关系批量提交
独立模块的部署，并在初始化、模块绑定和最终确认之间保留核对点；这需要配套处理连续 nonce、
交易未确认或失败后的恢复，以及区块 gas 容量。不要在现有串行任务运行时另起脚本并发使用
同一个 Bootstrap Admin，以免触发未知 pending nonce 或交易替换保护。

## 各命令的用途

| 命令 | 解决的问题 | 主要输入和输出 | 是否使用私钥／写链 |
| --- | --- | --- | --- |
| `check` | 当前 release 是否连到正确链，预部署代码和启动条件是否符合预期 | 冻结 bundle、工具 artifact、RPC → 只读预检结果 | 否／否 |
| `bootstrap` | 按冻结配置完成部署、初始化、模块绑定和 `finalizeBootstrap` | bundle、管理员私钥、已有恢复记录 → 链上交易、私有 state 和 journal | 是／是 |
| `status` | 最近一次受管任务是否运行、失败或完成，当前链上是否有完成标记 | 任务容器、私有状态、RPC → 任务结果、链上观察和文件路径 | 否／否 |
| `export` | 从私有恢复材料生成可公开的部署记录 | bundle、已完成 state、签名交易 journal → public state | 否／否 |
| `validate` | 在同一个区块核对实际部署是否符合冻结配置和审核过的代码 | bundle、public state、工具 artifact、RPC → strict validation 报告 | 否／否 |

`export`、`validate` 不会再次部署、花费 gas、修改委员会或激活分红。链上初始化由 `bootstrap`
中的交易完成；这两个命令用于保存和核对验收证据。`check` 和 `status` 中的 `finalized=true`
表示 Dividend 合约的 bootstrap 完成标记，不代表 PoW 区块已经不可重组，也不代替完整验证。

## export：生成可公开的部署记录

在 bootstrap 任务成功、私有 `state.json` 为 `completed` 后执行：

```bash
usdb-node sourcedao export
```

命令自动定位当前链身份下的 `state.json` 及同目录 `state.json.transactions.json`，不需要复制文件
或重新指定配置。它在独占锁下核对完整的已完成操作集合、链／配置／golden 身份，以及 journal
内签名交易的签名人、交易哈希、连续 nonce 和已记录的区块信息。只有这些本地材料
一致时，才生成 `sourcedao-bootstrap-public-state.json`。

输出只保留链与配置身份、管理员和模块地址、操作名称、交易哈希、区块高度等白名单字段。
RPC URL、本机路径、错误文本、私钥和原始签名交易字节均不导出。这份 public state 供后续
`validate` 和发布 acceptance 使用，也可以提交公开 Git；它不能替代私有 state/journal 来恢复部署。

需要注意：

- export 不访问 USDB RPC，不重新查询交易回执或判断重组；成功表示本地材料自洽，链上现状需要
  后续验证。受管命令仍需要 Docker；镜像尚未缓存时还需要拉取镜像。
- 导出需要私有目录写权限来创建和释放锁，但不会改写原 state/journal 的内容。
- 相同输入重复执行会保留完全相同的公开文件字节；已有输出不同则报错，不覆盖旧证据。
- 若报告未完成、缺少 journal 或身份不一致，应核对 bootstrap 原任务和备份。不要手工把 status
  改成 `completed`，也不要用 public state 冒充私有恢复文件。
- 文件名中的 `public` 表示内容可以公开；命令只在本地生成文件，不会自动上传 GitHub 或发布 release。

## validate：核对链上初始化结果

export 成功后，应尽快在治理操作、代币转移、兑换或锁仓等业务活动开始前执行首次验证：

```bash
usdb-node sourcedao validate
```

受管入口固定使用当前 release 的 bundle、工具镜像和导出的 public state，并默认启用 **strict**。
它不读取私钥或私有 journal，通过 RPC 检查以下内容：

- chain ID、genesis、配置及 golden 身份与发布输入一致，检查点不早于记录中的 bootstrap 完成高度。
- DAO/Dividend 预部署代码、各模块代理及实现代码与审核过的 artifact 一致，ERC1967 实现绑定正确。
- 管理员、模块地址、模块到 DAO 的反向绑定、委员会参数与冻结配置一致。
- DevToken 初始分配、合约自持储备、代币精度，以及其他模块的初始化计数器、锁仓状态和
  `bootstrapFinalized` 等检查项符合预期。

首次默认报告不存在时，验证器在开始时选取一个 `latest` 区块作为检查点，随后所有合约调用、
代码和存储读取都固定在该高度，结束前再次确认区块哈希和状态根未变化。成功生成
`sourcedao-bootstrap-validation.json`，其中应有 `status: "ok"`、`mode: "strict"`，
`evidence.checkpoint` 记录高度、哈希和状态根；同时保存配置／golden 摘要与代码、存储、调用证据。
`usdb-node` 终端或 `--json` 输出描述的是受管任务，完整合约验证结果以这个报告文件为准。

检查点和重复执行的规则：

- 第一次验证核对的是初始化基线。若首次运行前已经转账或进行治理，最新状态可能合理地偏离
  初始分配等预期；strict 失败需要核对业务时间线，不应直接认定合约损坏或修改冻结配置来通过检查。
- 默认报告已经存在时，重复 `validate` 会重新读取**原检查点**并验证，保持报告字节不变。
  它不会随链增长改为检查最新状态，因此不能当作持续的 DAO 运行健康监控。
- 原检查点发生重组、历史状态不可读，或复验结果不同，命令会失败。不会回退到最新高度或覆盖
  已接受报告。节点必须保留该高度的历史状态；需要跨重启／新节点重放时，应按验收方案保留 archive 数据。
- 新检查点报告需要维护者使用完整工具的 `--block H --output <新文件>`，当前受管入口不提供这些
  覆盖参数。保留旧报告，单独审核新报告；完整工具的显式输出参数具有覆盖语义，不能指向验收原件。

strict 报告证明该 RPC 所提供的固定检查点状态通过了这些检查；它不等待发布要求的确认深度，
也不独立核验全部初始化交易历史。正式发布还需等待规定的非零确认深度，执行 geth 的
`usdb-bootstrap-acceptance create/verify`，核对交易历史并独立重放报告证据。
这一步尚未封装为 `usdb-node sourcedao` 子命令，参数说明见 SourceDAO
[`docs/usdb-bootstrap-tools.md`](../../../SourceDAO/docs/usdb-bootstrap-tools.md#创建验收文件与独立重放)。
因此 public state、strict validation 和最终 acceptance 是配套材料，各有不同作用。

## 结果检查与常见失败

```bash
usdb-node sourcedao status --json
```

通过输出中的 `action` 确认最近一次执行的是 bootstrap、export 还是 validate，结合 `outcome` 判断
任务结果，并从 `state_file`、`public_state`、`validation` 取得实际路径。这些路径可能在文件创建前
就已显示，不能仅凭路径被打印判断成功。export／validate 正常完成返回 0，失败返回非零；
Ctrl-C 停止观察返回 130，后台任务继续。SSH 断开后先查看 `status --watch`，避免并发重复提交任务。

| 现象 | 处理方式 |
| --- | --- |
| export 找不到输入、state 未完成或 journal 不完整 | 先检查 bootstrap 任务；修复条件后用原私有记录恢复 bootstrap，再导出 |
| validate 找不到 public state | 先成功执行 export，不要直接拷贝私有 state 到公开目录 |
| RPC 超时或连接错误 | 核对 chain 容器和本机 RPC；恢复后重试，保留已有证据 |
| 配置、genesis、golden 或代码不匹配 | 核对已激活 release 和连接的链；不要编辑摘要或放宽 strict 来绕过 |
| 旧检查点不可读、发生重组或已有输出不同 | 保留文件和私有日志，核对历史节点、主链与原验收记录；由维护者决定是否重做验收 |

## 恢复与互斥

- 部署运行期间，重复 bootstrap、切换 release、down 和 mining 配置变更会报错。
- 主机或 Docker 重启后，一次性任务不自动重新使用私钥。确认链服务恢复后，重新执行相同
  `bootstrap --key-file ...`，工具使用原 state 和交易 journal 恢复。
- 失败容器保留。只有确认锁由这个已停止的受管任务创建，命令才会自动清除遗留锁。
  容器丢失或锁身份不明时会拒绝恢复，保留文件供管理员核查。
- 不要删除 state/journal 来“重新开始”。已签交易必须恢复原 nonce、交易哈希和部署地址。
- 私钥文件只以只读挂载提供给 bootstrap。导出和验证不接收私钥；验证也不挂载私有 journal。
  失败细节可通过 `status` 给出的 `docker logs <容器名>` 查看，按私有运维日志管理。

## 文件位置与版本管理

受管命令将文件放在 `node.env` 同级的 `sourcedao/` 中，与 release 安装目录分离：

```text
~/.config/usdb/usdb-testnet-v0/sourcedao/
├── task.json
├── logs/<taskId>.log
├── private/<chainId>/<genesisHash>/<configDigest>/
│   ├── state.json
│   └── state.json.transactions.json
└── public/<chainId>/<genesisHash>/<configDigest>/
    ├── sourcedao-bootstrap-public-state.json
    └── sourcedao-bootstrap-validation.json
```

`status` 输出实际完整路径，自定义 `--node-env` 时目录随之变化。私有 state、journal、task 和日志
进入私有备份，不能上传公开 Git。私钥由独立密钥托管方案保管。`public/` 中的两个文件可以公开
提交，导出工具仅允许明确列举的字段，不包含私钥、原始签名交易、RPC URL 或本机路径。

已存在的公开记录默认不可覆盖；重复导出/验证复用同一身份和验证 checkpoint，内容不同会拒绝。
维护新的验证 checkpoint 时，使用完整工具的显式输出选项并单独审核新报告。

发布输入的权威副本是 USDB `docker/networks/testnet-v0` 中的 bundle，可提交 Git。
SourceDAO 中保留经过审核的 final 配置和来源记录；重复生成的 `frozen-network-bundle` 预览和
`.sourcedao-freeze-backup-*` 回滚目录不用提交。受管部署结果也不会自动改写冻结 bundle。

## 发布与验收

release manifest v8 必须包含第四个镜像
`ghcr.io/buckyos/sourcedao-bootstrap-tools@sha256:<digest>`，绑定 SourceDAO revision 及
`usdb-tools-image.yml` 的 provenance。该镜像包含工具与审核过的 artifact，bundle 在执行时只读挂载。
SourceDAO `main` push 的 Fast workflow 成功后构建镜像；candidate 根据锁定 revision 对应的成功 run
派生镜像标签，再冻结 digest。publish 再核验 provenance，目标节点执行时不接受可变标签替代。
首次启用该 GHCR package 时应按现有发布规范确认它可被目标节点拉取。

本地验收包括 Python 运维测试、TypeScript 临时链测试，以及显式选镜像的 Docker 完整流程：

```bash
# usdb 仓库
python3 tests/test_node_sourcedao.py

# SourceDAO 仓库，使用仓库固定的 Node 版本
npm run test:usdb:tools
docker build -f Dockerfile.usdb-tools -t sourcedao-bootstrap-tools:acceptance .
SOURCE_DAO_MANAGED_TEST_IMAGE=sourcedao-bootstrap-tools:acceptance \
  node --import tsx --test tests/bootstrap_managed.test.ts
```

Docker 验收只创建临时 Hardhat 链，使用公开测试私钥，覆盖运行中断、journal 恢复、22 笔交易无重复、
公开导出和严格验证。它不连接测试机，也不执行生产 bootstrap；发布仍需通过实际 CI 与 release 审批。
