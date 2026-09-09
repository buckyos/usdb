# USDB 节点入网、Seed 管理与延后启用矿工

状态：第一阶段已实现，本地回归覆盖配置与故障恢复；真实多机验收待部署。第二、三阶段仍是计划。

## 1. 实施顺序

| 阶段 | 目标 | 交付与验收 |
| --- | --- | --- |
| 1：持久 Seed 管理与入网状态 | 通过命令补充引导来源，空 peer 的 genesis 不再误报已入网 | `peers list/add/remove/status/apply`、可恢复的 chain 配置应用、未配置 Seed / 等待连接 / 同步中 / 已连接状态；本地故障注入回归 |
| 2：IPv6 与双栈 | IPv6-only 加入节点可以连接首节点，双栈节点正确对外提供地址 | 宿主机和容器能力检查、setup 地址族选择、TCP/UDP 映射与防火墙、可分享 enode；IPv4-only / IPv6-only / 双栈真实双机验收 |
| 3：提前提交挖矿意图 | 补充 Seed 后立即提交 `mining enable`，先以 full 同步，再自动启用 miner | 分离期望与实际角色、等待上游与链同步、共同高度区块 hash 检查、切换前重新验证矿工资格；中断、断连、重组和取消验收 |

先完成配置入口与任务恢复，再解决地址可达性，最后让 controller 自动串联入网和挖矿。
第一阶段不会把一次 `eth_syncing=false` 观测升级为自动启用矿工的授权。

## 2. 第一阶段操作

以安装节点的普通运维用户执行，要求已完成 `setup`，拥有现有 Docker 权限并安装 systemd controller。
采用 `setup --no-controller` 的部署需先运行 `usdb-node controller install`，才能提交这些持久任务。
Seed 来自同一网络的可信节点运维方；工具不会从空配置推断“本机是整个网络的首节点”。

```bash
# 只读：持久配置中的目标列表；不要求 chain RPC 已启动
usdb-node peers list

# 使用首节点提供的完整、可达 enode；不要填 SSH 端口
usdb-node peers add "$SEED_ENODE"

# 观察配置应用、chain 高度与当前实际连接
usdb-node peers status --watch

# 删除这一条引导地址；重复 add/remove 不生成重复配置
usdb-node peers remove "$SEED_ENODE"

# 修复报错原因后，显式重试未完成的应用任务
usdb-node peers apply
```

各子命令均支持 `--json`；`status` 支持 `--refresh-secs`，默认每 5 秒刷新。
`controller_submitted` 表示意图已经保存和提交，不表示已经连通。
`APPLIED` 表示配置已写入；对于本来在运行的 chain，还会核对新容器和 geth 实际参数。
之后仍需看独立的入网状态。

Seed 是引导来源，不保证始终与其直连，也不是 peer 白名单。`remove` 删除持久引导地址，不封禁该节点；
它仍可能通过发现协议或入站连接重新成为 peer。链数据库、node key 与发现数据库均保留。

### 2.1 地址与校验

输入使用 `enode://<128 位十六进制公钥>@<地址>:<TCP 端口>`，IPv6 地址用方括号；
可附带 `?discport=<UDP 端口>` 指定不同的发现端口。校验公钥、主机名/IP 和端口，规范化大小写与 IPv6 写法，
去除完全重复的地址，最多保存 64 条。同一公钥的 IPv4、IPv6 和域名端点分别保留，删除时匹配完整地址。
域名在配置阶段不会触发 DNS 查询。

**保存了 IPv6 格式的 enode，不等于当前 Docker 网络已经具备 IPv6 连通性。** 端口、容器网络与双栈验收
在第二阶段补齐；第一阶段不承诺同一公钥多个地址之间的自动故障切换。
首节点地址必须是另一台机器可以访问的地址及实际 P2P 映射端口。默认 P2P 为 TCP/UDP `31303`；
RPC 继续采用现有本地访问方式，不因添加 Seed 而开放公网 RPC。

### 2.2 状态含义

| 状态 / 原因 | 含义与操作 |
| --- | --- |
| `STARTING / APPLYING_SEEDS` | 操作未完成；查看阶段与 controller 日志 |
| `BLOCKED / PEER_OPERATION_BLOCKED` | 应用失败，保留具体错误；排除原因后执行 `peers apply` |
| `WAITING / SEED_REQUIRED` | 无引导来源且无有效首节点记录；加入节点应 `peers add` |
| `WAITING / WAITING_FOR_PEERS` | 有引导来源但当前无连接；检查地址、P2P 映射和 TCP/UDP 防火墙 |
| `SYNCING` | chain RPC 正在报告链同步 |
| `READY / CONNECTED` | 有引导来源、有 peer，且当前未报告同步；这是基本连接观测 |
| `READY / FIRST_NODE` | 已明确确认并绑定当前网络、数据与 node identity 的首节点；允许零 peer |
| `WAITING / OBSERVATION_UNAVAILABLE` | RPC 尚未可用或读取失败；不宣称已经入网 |
| `BLOCKED / CHAIN_IDENTITY_MISMATCH` | RPC 的 chain ID、network ID 或 genesis 不符合当前 release |

`peers status --json` 分别提供目标 `configured`、已写入 `applied_config`、实际 `connected` 与 `operation`。
应用中保留独立的 `membership` 观测，避免将旧连接误认为新配置已经生效。
普通 `status` 在数据服务就绪但尚未入网时显示 `AWAITING_PEERS`，进度面板的 chain 行说明具体原因。
controller 可以结束此次启动并保留数据服务运行，等待运维补充 Seed；不靠循环重启解决 P2P 不通。
`CONNECTED` 不证明已核对共同高度区块 hash，也不保证当前高度就是全网最新高度。这些证据与自动挖矿授权
属于第三阶段。

### 2.3 应用、并发与恢复

操作保存在 `node.env` 同目录的 `node.peers.json`，使用权限 `0600`、原子替换及 fsync。
记录网络/数据路径绑定、目标 Seed、配置指纹与应用检查点，不保存 `node.env` 的 RPC 密码。
阶段为 `QUEUED → CONFIGURING → STOPPING → STARTING → APPLIED`；chain 未启动时跳过停止和启动阶段。

- bootstrap 同步期间可以提交 Seed；只写入任务，待 controller 取得生命周期锁后应用。
- chain 运行时仅停止、重建 chain。Bitcoin、balance-history、indexer 的进程和数据不受此操作重建；
  chain RPC/P2P 会短暂中断。节点已停止时仅写配置，之后用 `up` 启动。
- 已有矿工切换任务优先完成，Seed 变更随后执行。peer 应用未完成时不能创建新的 mining enable；
  重复附加已有矿工任务仍可执行。显式 `mining disable` 可优先取消挖矿并保留 Seed 意图。
- 修改运行矿工的 Seed 必须有匹配当前角色、网络及数据身份的成功授权；只更新其 Seed 绑定，保留
  首节点记录、地址和线程，切换前仍执行现有启动门禁。加入矿工移除最后一条 Seed 会报错，
  应先 `mining disable`，或先添加替代 Seed。
- SourceDAO 活动任务受现有生命周期互斥保护；等待其结束后再 `peers apply`。
- 配置与矿工授权之间发生写入中断，可以按检查点续跑。外部修改导致配置或授权不匹配时明确报错，
  不覆盖现场。`QUEUED` 阶段可继续增删目标；进入应用阶段后先完成或重试当前任务。
- 自动重试只观察已启动的新容器，不重复创建仍在启动的 chain。新容器失败时保留错误；修复后
  `peers apply` 可显式重新停止/重建失败的 chain。
- `down` 保留 Seed 意图，但取消该任务继续 bootstrap 的意图。后续 `peers apply` 完成配置，`up` 恢复服务。

### 2.4 目前加入第二节点的顺序

```bash
# setup 可先省略 Seed，以 full 模式同步上游
usdb-node peers add "$SEED_ENODE"
usdb-node peers status --watch

# 如果之前停止了节点，用 up 启动；已有同步任务会继续
usdb-node up

# 等待入网、同步和矿工资格检查通过后启用加入节点挖矿
usdb-node mining check --address "$MINER_ADDRESS"
usdb-node mining enable --address "$MINER_ADDRESS"
usdb-node mining status --watch
```

加入节点不加 `--first-node`；首节点 genesis 冷启动仍需明确 `mining enable --first-node`。
此阶段“添加 Seed 后立刻 enable 并等待自动追平”尚未实现，不能把预检报错当成任务已保存。

## 3. 后续实现约束

第二阶段分别确认宿主机全局 IPv6、容器出站 IPv6、geth 监听、Docker 发布地址及上游路由/防火墙。
自动模式按真实能力选择，显式 IPv6 模式失败时给出缺失项，不默默改用 IPv4 后宣称 IPv6 成功。
同一 node key 对应同一节点身份；对外地址展示区分地址族、公布端口和容器内地址，避免复制不可达地址。

第三阶段将期望 `miner` 与实际运行 `full` 分开保存。收到 intent 后依次等待上游、Seed 连通、网络身份
和链同步证据；切换前重新验证共同高度 canonical hash、矿工资格、资源和上游 epoch。
无 peer、RPC 超时、Seed 修改、同步中、观察窗口内重组应保持等待或阻塞，不隐式变成首节点。
`disable` 必须能取消等待意图。controller 完成切换后退出，长期进程仍由 Docker 管理；持续断连时矿工
是否暂停，以及链运行时如何保持这条门禁，需要在该阶段一起明确并验收。

## 4. 第一阶段验收

```bash
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_node_peers.py
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_node_mining.py
PYTHONDONTWRITEBYTECODE=1 python3 docker/scripts/tools/test_usdb_node.py
PYTHONDONTWRITEBYTECODE=1 python3 docker/scripts/tools/test_prepare_release_node_kit.py
```

测试使用临时数据、可控 RPC 与 Docker 边界，覆盖地址规范化、并发排队、只重建 chain、授权写入中断、
配置漂移、失败容器显式重试、down/disable、首节点身份及错误展示。Fast CI 接入 peer 验收，包含与 mining
任务的串行化和授权更新；完整 mining 测试还需 sibling go-ethereum checkout 中的运行脚本。
本地测试不代替真实两机的连接与同步测试，也不作为 IPv6/双栈已经可用的证据。
