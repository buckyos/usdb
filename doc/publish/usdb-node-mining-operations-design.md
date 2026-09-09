# USDB 节点矿工切换与运维设计

状态：**脚本已实现，待发布后现场验收**，2026-09-06。`usdb-node mining ...` 已纳入 node-kit。
目标是在已启动的 full 节点上，以一次可恢复的运维操作完成矿工资格检查、角色切换和运行状态验证。
本文不修改共识规则。本次实现没有在测试机启用挖矿或执行 SourceDAO 交易。

## 1. 当前实现与现场依据

- `docker/scripts/tools/usdb_mining.py` 实现预检、角色转换日志和运行观测；入口由 `usdb_node.py` 提供。
- `setup/configure` 先创建 full / bootnode；需要挖矿时，在完整初始化后使用 `mining enable`。
- `set-role --role miner` 明确报错并指向 `mining enable`。非 miner 的配置编辑仍保留，`up` 会识别
  实际角色漂移并调用同一个 chain 转换引擎，避免 READY 提前返回。
- `run_testnet_runtime.sh stop-chain/recreate-chain` 只停止、重建 chain，后者固定使用 `--no-deps`；
  miner 的 `up-chain/recreate-chain` 都校验持久授权和新鲜上游资格。
- Go 仓库的 `scripts/usdb/docker/usdb_runtime_node.sh` 显式传递 bootnodes 和空 DNS discovery，
  拒绝覆盖身份、发现、矿工参数和 fake PoW 的 extra args，并启用包含完整 block hash 的 JSON 日志。
- `resolve_miner_candidate` 按固定高度和 state identity 自动选择 Active Standard pass。
  多张 pass 使用 effective energy 降序、pass ID 升序的协议规则。

2026-09-06 对测试机 r15 的只读观测：

| 项目 | 结果 |
| --- | --- |
| 矿工地址 | `0x4ddc71108239dbb30aa288b93ab1d18539ec863a` |
| 固定 BTC 高度 | `965775` |
| 匹配候选数 | `1` |
| pass ID | `1ff30e9a2393b6daf3e7a9fa0bc7f2ad1e16d1bf1c4c721fca5def215ef2e283i0` |
| pass 状态 / 类型 | `active` / `standard` |
| effective energy / level / difficulty factor | `0` / `0` / `10000 bps` |
| chain ID / 高度 / peers | `202608250` / `0` / `0` |
| `eth_mining` | `false` |

查询同时约束 readiness 返回的 snapshot ID 和 system-state ID。该记录只证明观测时的资格，
正式启用前必须重新检查；不能把这份历史记录当成永久授权。
零 energy 的 Active Standard pass 属于当前候选集合，不能增加「energy 必须大于零」的运维门禁。
`owner_btc_addr=null` 不影响以 script hash 和 `usdb_main` 为依据的资格检查；optional registry 未完成也不应阻塞。

### 1.1 零能量、出块和发行量

`UNIT_SATS=100_000` 只用于把 owner 的 BTC 余额离散为能量增长单位。低于该值时不新增 raw energy；
从低于阈值增加到恰好 `100_000 sats` 已有一个单位，不要求严格大于。新增加的余额需先进入
indexer 使用的 BTC 稳定状态，之后按 BTC block 的时间推进积累能量；不是到账即获得历史能量。

零能量 Active Standard pass 仍可参与 PoW，level=0 对应 difficulty factor=10000 bps，
即使用完整基础难度。只要其余共识条件成立，单节点不因全网 energy=0 而禁止出块。
现有 Go 用例 `TestVerifierResolveProfileAcceptsZeroEnergyActiveStandardCandidate` 明确覆盖此契约。

Coinbase 发行采用 UIP-0011 的 active owner BTC 余额总量，余额按 sat 求和，不按 UNIT_SATS 截断。
它没有以「全网总能量是否为零」作为发行开关：

```text
target = total_miner_btc_sats × price / 100_000_000
remaining = max(target - issued, 0)
emission = floor(remaining × k_bps / (157680 × 10000))  # 按 atoms 整数计算
```

能量影响难度以及 UIP-0012 的 K 动态系数；平均能量为零时 `k_bps=10000`（K=1），不会除零或令 K=0。
BTC 余额总量为零、目标不高于已发行量，或整数舍入结果为零时，新发行量才可能为零。
新发行量为零不使正常区块无效；交易费用另按激活的 fee policy 处理。

补充现场观测：BTC 高度 `965779`、chain 高度 0 时，active owner 总余额 `18894 sats`，
系统记录的累计已发行量为 `10 USDB`，来自当前 genesis 给 Bootstrap Admin 的初始分配。按当前 v1 固定价格 `100000 USDB/BTC`、K=1，
在状态不变的条件下首块目标供应量为 `18.894 USDB`，新发行约 `0.000056405377980720 USDB`，不包含费用。
这是条件计算而非出块保证；预检应分别展示资格、能量增长速率与发行估算，不能把它们混为一个 READY 条件。

## 2. 运维入口

以下入口共用角色转换逻辑：

```bash
# 可选：只读检查，无节点配置或服务变更
usdb-node mining check --address <usdb-main-address>
# 无 seed 的首节点预检需同样明确声明：
usdb-node mining check --address <usdb-main-address> --first-node

# 完整检查、展示计划、提交并等待生效；CPU 默认 1 线程
usdb-node mining enable --address <usdb-main-address>

# 首节点在 genesis 且没有 peers 时明确声明冷启动意图
usdb-node mining enable --address <usdb-main-address> --first-node

usdb-node mining status
usdb-node mining status --watch

# 持久切回 full，验证停止本地挖矿
usdb-node mining disable
```

`enable` 自带完整预检，不要求先执行 check，也不复用旧 check 报告来跳过新检查。
以安装节点的普通运维用户运行这些命令，该用户需具有现有 Docker 访问权限。
Geth 容器可能以 root 创建 `geth/`、`nodekey` 或恢复标记；宿主机读取遇到权限错误时，
脚本自动使用当前 release 固定的 chain 镜像启动临时只读探针，仅挂载链数据目录，
读取文件元数据、身份哈希和恢复标记。探针不启动 Geth，不打开数据库引擎，不输出 nodekey 内容，
chain 已停止时仍可执行。无需把整条命令改为 `sudo usdb-node ...`，也不要对数据目录执行递归 `chmod/chown`。
如果探针失败，`CHAIN_DATA_INSPECTION_FAILED` 会提示检查 Docker 权限和本机 release 镜像；
数据库身份、深重组停机标记及 epoch 校验继续生效，不能通过权限回退跳过这些检查。

交互终端提交后等待配置生效；`--json` 或非交互输出返回 operation ID 与 `controller_submitted`，
表示任务已提交，不能等同于已启用。使用 `mining status --watch` 继续观察。
交互模式只在检查通过后确认一次，显示完整地址、自动选中的 pass、网络身份、线程数和影响范围。
自动化可提供 `--yes` 与 `--json`；`--yes` 只跳过交互确认，不跳过验证。

测试网 v1 默认使用本地 CPU、1 个 PoW worker，无需运维人员设置线程数或选择算力模式。
高级显式线程数必须为正且不超过可用 CPU 配额；不把 `0` 当作自动线程数。
1 线程限制 sealing worker，不等同于整个 geth 的 CPU hard quota，DAG/验证仍可能使用额外 CPU。
正式网的 external miner、GPU、remote sealer 及算力调度另行设计，不在本次测试网 enable 中引入。
reward address 的私钥不需要放入节点、Compose 或命令行；SourceDAO Bootstrap Admin 另行保管。

### 2.1 首节点声明与 peer 发现

工具可以识别本机配置与连接状态，但不能从空 peer 列表证明整网不存在其他节点。
首次启用的默认规则收紧为：没有配置有效 USDB 引导来源且没有 `--first-node` 时，直接报
`PEER_SOURCE_REQUIRED`，不改配置、不停止 chain。引导来源包含 setup 保存的 `USDB_BOOTNODES`、
后续明确保存的 peer 配置，以及未来 release 显式提供且绑定同一网络的 seed；不包含其他链的隐式默认列表。

| 引导来源 / 声明 | 启用行为 |
| --- | --- |
| 无引导来源、无 first-node 声明 | 报错并提示配置 peer 或明确声明首节点 |
| 无引导来源、有 first-node 声明 | 首次要求预期 genesis、没有同步历史或已知网络冲突；确认后记录首节点身份 |
| 有引导来源、连接为零或尚未同步 | 等待连接并给出 P2P/NAT 排查信息；不降级为首节点 |
| 有引导来源、已连接同网且同步完成 | 走普通矿工启用 |
| 已有成功首节点记录、同一链目录后续重启/重复 enable | 验证记录后恢复，不能因本机已产块而要求重新满足高度 0 |
| first-node 与已知加入网络配置/观测相冲突 | 明确拒绝冲突，不把该参数作为绕过同步的开关 |

首节点记录绑定 chain ID、genesis、数据 identity 和持久 node identity；它是已执行操作的记录，
不替代重组、恢复或协议门禁。全新目录或新网络 generation 不继承旧声明。

发现流程应明确为：运维提供同网 seed/enode → 发现并建立连接 → 确认网络身份/同步状态 → 启用 miner。
持久 Seed 现可通过 `usdb-node peers add/remove` 管理，`peers status --watch` 分别观察配置应用与实际连接。
当前 enable 仍要求预检先通过；“提前提交 intent、先以 full 追平再自动挖矿”属于后续阶段，见
[节点入网与 Seed 管理计划](./usdb-node-peer-discovery-plan.md)。
种子发现不能保证与种子本身永久直连；也不能只凭 `eth_syncing=false` 或无 peers 的 genesis 声称已追平网络。
首节点上线后提供可分享的 enode，后续节点持久保存它作为引导来源。
node key 必须随数据目录保留，NAT 下要核对实际可达的外部地址以及 `31303/TCP+UDP` 映射，
不能直接把 Docker 内网 enode 或 SSH 的 `2224` 端口当作可用 P2P 地址。

此前 runtime 仅在 `USDB_BOOTNODES` 非空时传 `--bootnodes`，可能落回 Geth 的默认列表。
现在总是显式传递该参数；同一个参数同时约束 V4/V5 的默认 seeds，另传 `--discovery.dns ""`。
没有增加会改变 genesis/config 选择的 `--usdb` preset。

## 3. 启用前检查与展示

| 检查 | 失败时行为 |
| --- | --- |
| 地址长度、十六进制、非零；混合大小写地址校验 checksum | 指明输入错误，保留原配置和 full 容器 |
| release、chain ID、genesis、数据 identity 一致 | BLOCKED；不修改 genesis 或数据 |
| Bitcoin full-ready；BH / indexer consensus-ready | WAITING/BLOCKED，给出具体 blocker，不用旧缓存放行 |
| deep-reorg guard、恢复 latch、资源阶段已正常收敛 | 保留恢复门禁；不得删除 latch |
| 固定高度及完整 state identity 的候选查询 | 显示未索引、非 Active、非 Standard、地址不匹配或 RPC 错误 |
| RPC 返回 identity、`usdb_main`、选择规则与预期一致 | 拒绝错误响应或跨状态拼接 |
| chain 同步与首节点身份 | 无持久引导来源且无 first-node 时报错；有来源未连通时等待；首次声明要求 genesis |
| 可用 CPU、现有 chain 内存预算、启动参数无冲突覆盖 | 显示调整建议；不自动切换整机资源策略 |

优先使用一次候选 RPC 返回的完整 profile，不分别拼接 pass、energy 与 aggregate。
保留并复核 upstream reorg epoch；预检和实际应用之间状态变动时重新查询，而不是继续使用旧身份。
多张 pass 的默认选择遵循协议，展示 matching count；可另提供 `--expect-pass` 作为运维断言，
不改变 miner 之后按同一地址 consume/remint 自动跟随的协议行为。

输出包括 network、完整收益地址、pass ID、候选状态、level、BTC 高度、线程数和
「仅重建 chain 容器；保留全部数据与 upstream 进程」。详细 identity 放入 JSON 报告。
无需普通运维人员填写 RPC URL、DB 路径、snapshot ID 或 pass ID。

## 4. 可恢复的应用流程

实现复用 systemd bootstrap controller，由专门的角色转换任务完成：

1. 只读生成计划；提交时获得现有 node operation lock，重新核对配置摘要和实际角色。
2. 先原子落盘操作意图：operation ID、release/bundle identity、原角色配置、目标配置摘要、
   已验证身份与阶段。此后任何中断都可通过当前配置、容器状态和该日志恢复。
3. 在已有数据 identity 和恢复门禁通过后，优雅停止旧 chain，等待进程真正退出并释放数据库。
   Bitcoin、BH、indexer、registry installer 保持运行；control-plane 保持运行并显示角色切换进度。
4. 原子写入 role/address/threads，重新检查新鲜 readiness / candidate 与 reorg epoch，再以
   `--no-deps` 重建 chain。新旧 chain 不得并行打开同一数据目录，不重跑 snapshot 或 genesis init。
5. 比较容器的实际 image、角色配置与 geth 启动参数，查询 chain identity、`eth_mining` 和
   `eth_coinbase`；所有必要观测匹配后才标记配置已生效并清除待处理记录。
6. 继续展示 work/DAG/出块状态。首块等待不影响已完成的配置操作记录，也不能被误报为出块成功。

角色转换日志应独立于 `node.resources.json`，避免把矿工启动失败显示成资源策略重启。
`node.env` 同目录的 `node.mining.json` 保存不含 RPC 凭据、私钥的角色备份、身份与阶段记录。
写入使用原子替换并 fsync 文件与目录；转换日志与资源日志相互独立。SSH 断开只脱离显示；重复同一命令附加未完成任务，
目标已经生效时为 no-op。另一笔不同目标的并发操作应明确拒绝或显式取消前一任务。

失败时不删除任何链数据或回滚已经产生的区块。启用失败可恢复原 full 配置并尝试重建 full 容器；
恢复失败必须保留 FAILED 状态与原因，不能以配置文件恢复成功代替运行恢复成功。
尚未完成、但仍在准备 DAG/等待 work 的任务应报告 pending，不能循环重启或自动切回 full。

`disable` 不依赖矿工仍有资格或上游就绪。它先持久取消待启用意图，再停止本地 sealing、
将角色恢复 full 并重建 chain；RPC 不可达时也必须能通过停止 chain 容器落实禁用。
如果 deep-reorg guard 已停链，禁用不能恢复被 guard 禁止的 chain 运行。

角色切换完成后 controller 可以退出，保留 systemd unit 的开机启用状态。chain 容器自行处理临时上游故障：
启动时 guard RPC 失败会定时重试；运行中达到连续错误阈值后先优雅停止 geth，再等待上游恢复。
只有原有 baseline 校验通过才恢复原角色，保持 miner 地址和线程数，不通过重新执行 `mining enable` 恢复。
真实 epoch 变化及持久 `halted.json` 继续阻止自动恢复；不得把临时 RPC 故障与深重组事件合并为永久等待。

现有 `set-role` 与 `up` 必须纳入同一状态模型：配置角色与实际容器角色不一致时不能报告已完全就绪，
也不能返回「already ready」跳过应用。所有将配置应用为 miner 的入口都执行上述硬检查，不能由旧入口绕过。
`set-role` 如继续保留配置编辑语义，应明确报告尚未应用，并由 `up` 调用同一个角色转换引擎。

## 5. 状态与成功语义

必须分别展示配置意图、实际运行状态与出块观测：

| 状态 | 含义与证据 |
| --- | --- |
| DISABLED | full 角色，观测到本地挖矿关闭 |
| CHECKING / SWITCHING | 正在校验或执行有持久记录的角色转换 |
| WARMING_UP | miner 配置已生效，正在准备 DAG 或等待有效 work |
| ACTIVE | 实际 miner 地址匹配、`eth_mining=true`，存在当前有效 work |
| WAITING | 上游暂不可用、资格/工作暂缺失；显示具体原因与观测时间 |
| FAILED / BLOCKED | 执行失败、身份不匹配或恢复门禁阻止运行 |

检查使用现有 RPC、容器参数与阶段日志；必要的新结构化 runtime 状态属于后续明确实现项。
`eth_mining=true` 不能单独证明已有有效 work 或成功出块；`eth_hashrate=0` 在初始化窗口也不等于失败。
`eth_getWork` 可作为工作存在性的观测，不能用于改变 CPU/remote sealer 模式。

链高度增长只证明链推进。需要报告本机出块时，应把本地 sealed 事件与同 hash 的 canonical block 交叉确认；
仅按 coinbase 匹配或看到别的矿工出块，不能宣称本机已经成功挖到区块。
UI 显示 last local seal 与 chain head 两个独立观测，不承诺固定时间内必定出块。

状态查询超时保留带 STALE 的最后显示值，同时将本次观测标为不可用。旧观测不参与任何启用门禁。
正常 `status` 增加 mining 信息；full 节点的 mining=DISABLED 本身不降低运行状态。
入网状态独立判断：无 Seed、无连接时为 `AWAITING_PEERS`，进度面板会给出具体等待原因。

## 6. 首节点与 SourceDAO 边界

当前测试机属于「genesis 已初始化 → 首个矿工尚未启用」阶段。首个矿工成功出块后，再按既有手册
完成 SourceDAO bootstrap 和 strict validation，随后加入 full / late joiner / 第二矿工。

预检读取冻结网络中的 `dividendFeeSplitBlock`，并展示 Dividend 的 `bootstrapFinalized()` 状态。
当前 testnet-v0 的该高度为 8192；显示剩余区块数，不把 8192 写死为所有网络的常量。
SourceDAO 未完成不能阻止 genesis 冷启动挖出用于执行 bootstrap 交易的区块。
矿工启用不自动发送 SourceDAO 交易，不读取 Bootstrap Admin 私钥，也不宣称 bootstrap 已完成。

## 7. 实施与验收

已实现 `mining check/status`、地址资格检查、角色漂移检测、`enable/disable`、chain 独立重建、
持久转换日志、controller 恢复和运维面板。新 helper 与模块必须随 node-kit 发布，
发现参数和 JSON 日志的变化需要包含对应 Go runtime 脚本的新 chain 镜像。
基础验收完成后，再在测试机明确执行 first-node enable；本次代码工作保持测试机原有角色。

| 验收场景 | 通过条件 |
| --- | --- |
| 当前地址和 Active Standard pass | 自动解析资格；zero-energy、registry 未完成不被误拒 |
| 地址错误、缺失候选、inactive/collab、RPC 失败 | 没有配置或容器变更，错误可定位 |
| 多候选、consume/remint、预检后 reorg | 协议选择一致；重新检查 identity，不使用旧授权 |
| full→miner，现有服务全部 READY | 确实重建 chain；不因 READY 分支跳过 |
| 重复 enable 与 SSH 断开 | 同目标幂等；重连可观察原操作 |
| 在停止前后、配置写入前后、创建前后退出 controller | 恢复后收敛；同一目录从无两个 chain 并行写入 |
| 配置 miner 但运行 full / 地址或线程参数漂移 | 状态明确未应用，不能报 ACTIVE |
| OCI 创建失败或初始化 RPC 超时 | 真正失败与 pending 分开；不无限重启正常初始化 |
| 上游失败、pass 已失效时 disable | 仍能持久禁用，本地不再 sealing；不清除 recovery latch |
| chain 先于 indexer 启动、运行中 RPC 503/超时/响应损坏 | 自动等待与恢复，原 baseline 和 miner 参数不变；新旧 geth 不重叠 |
| RPC 故障期间 reorg epoch 前进或回退 | 恢复时记录 incident 并保持停机；重启不能绕过 |
| guard 检查、重试等待、运行或持久停机期间收到 SIGTERM | 正常退出并回收子进程；未知 guard 退出和 geth 崩溃交给 Docker 重启 |
| 无 peer 配置且未声明 first-node | PEER_SOURCE_REQUIRED，配置和容器不变 |
| 已配置 peer 但暂时不可达 | 等待/连接错误，不自动首节点，不启用独立挖矿 |
| 首节点首次启用与产块后再次 enable | 首次检查 genesis；后续校验绑定的首节点记录并幂等恢复 |
| peer 持久配置和默认发现来源 | 同网发现可用；空列表不回退 ETH/ETHW 默认 seed，重启 node identity 不变 |
| 测试网默认算力 | 无线程参数时实际为 1 个 CPU PoW worker；0 不被误作自动 |
| 零能量且有 BTC 余额 / 总 BTC 余额为零 | 都可验证合法 PoW；分别按发行公式计算非零/零 emission，不混同能量资格 |
| 等待 DAG、尚无首块、只有别人出块 | 不用 hashrate/链高度单一指标误报本机产块 |
| 回退失败或启动后已产块 | 保留数据库与链历史，正确报告阶段与失败 |

集成测试置于仓库根目录 `tests/`，共享 fixture 放 `tests/common/`；使用可控 RPC/Docker 替身。
发布前现场验收仍需在独立 regtest 部署完成启用、canonical 本机出块、重启保持 miner、禁用回 full；
比较上游四个容器的 ID/StartedAt 与持久数据，证明角色切换没有触发重导入或重启上游。
本地用例使用真实文件操作、实际 shell 启动路径和可控 RPC/Docker 边界，不能替代这项真实部署验收。

```bash
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=docker/scripts/tools:tests \
  python3 -m unittest discover -s tests -p test_node_mining.py -v
```

补充边界：RPC 是现有受控 operator 接口，管理员直接调用 `miner_start` 仍属于显式底层操作；
脚本的预检不是对节点管理员的权限隔离。面板会识别配置 full、实际 mining 的漂移。
`mining status --watch` 的旧观测最多保留 60 秒并标记 STALE；它从不用于 enable 门禁。

相关依据：

- [首节点操作手册](./usdb-testnet-v0-first-node-operations.md)
- [节点角色与 CPU 挖矿](./usdb-testnet-node-roles-and-cpu-mining.md)
- [Indexer RPC：候选选择与固定历史状态](../usdb-indexer/usdb-indexer-rpc-v1.md)
