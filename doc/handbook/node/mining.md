# CPU 挖矿启停与出块观察

[返回手册首页](../README.md) · [版本与验证范围](../networks/testnet.md#版本与验证范围)

本章介绍测试网发布包提供的 `usdb-node mining` 操作。默认使用本机 CPU、1 个挖矿工作线程。节点先以 `full` 角色启动，通过检查后再启用挖矿。

## 启用前准备

- 普通加入节点先按[安装与入网](install.md)完成同步和连接；网络首节点按本页的独立分支操作。
- 准备与已生效的 **Active Standard pass** 对应的 `usdb_main` 收益地址。工具自动选择符合条件的 pass，无需手填 pass ID。
- 原运维账号具有 Docker 权限，后台 controller 已安装，主机有可用 CPU 资源。
- 没有正在运行的 SourceDAO 初始化任务或尚未完成的节点配置任务。

启用挖矿只需要收益地址，不需要把该地址的私钥放到节点。收益地址与 SourceDAO Bootstrap Admin 分别管理。资格通过、有效工作和本机实际产块是不同结果；工具显示能量为零本身不等于资格无效。

## 普通加入节点启用挖矿

输入收益地址并检查：

```bash
read -r -p 'Miner reward address: ' USDB_MINER_ADDRESS
usdb-node mining check --address "$USDB_MINER_ADDRESS"
```

核对网络、收益地址、自动选中的 pass、线程数和错误提示。预检失败时先处理原因；失败不会替你保存一个等待未来自动启用的挖矿任务。

检查通过后执行：

```bash
usdb-node mining enable --address "$USDB_MINER_ADDRESS"
usdb-node mining status --watch
```

`enable` 会再次检查并展示变更计划，确认后只重建链服务，保留上游同步和已有数据。切换期间链 RPC/P2P 短暂中断。

若输出 `controller_submitted`，表示后台任务已接收，继续观察实际状态。SSH 中断或 Ctrl+C 退出显示不会取消任务。不要用 `set-role --role miner` 或手改配置替代这个入口。

## 网络首节点启用挖矿

**仅由确认正在创建该网络的初始化负责人执行。** 首节点没有其他 peers 时，先确认 Bitcoin、数据服务和 USDB 链已启动，链处于初始状态；此时普通总览可能显示 `AWAITING_PEERS`。

在当前终端输入收益地址，执行首节点预检：

```bash
read -r -p 'Miner reward address: ' USDB_MINER_ADDRESS
usdb-node mining check --address "$USDB_MINER_ADDRESS" --first-node
```

预检通过后再启用；失败时先按下方的[常见问题](#常见问题)处理：

```bash
usdb-node mining enable --address "$USDB_MINER_ADDRESS" --first-node
usdb-node mining status --watch
```

`--first-node` 明确声明网络冷启动，不是无连接节点的通用修复开关。工具仍会检查网络身份、上游状态和矿工资格。

确认链持续出块后，立即按[网络启动后初始化 SourceDAO](../network-admin/sourcedao.md)完成初始化和验证。初始化交易需要矿工持续提供区块。

## 读懂预检输出

`mining check` 显示 `state=READY`，表示本次预检通过，可以继续执行 `mining enable`。它不会自动开启挖矿，也不表示收益已经到账。`enable` 会重新检查当时的状态。

### 地址、资格和链状态

| 输出 | 面向使用者的含义 |
| --- | --- |
| `Address` / `CPU workers` | 挖矿收益地址和请求使用的 CPU 工作线程数 |
| `Pass` / `candidates` | 工具自动选中的 pass，以及与收益地址匹配的候选数量；多个候选不表示同时使用多个 pass 挖矿 |
| `state=active/standard` | 选中的 pass 当前有效，类型为可直接挖矿的 Standard pass |
| `energy` / `level` / `difficulty` | 有效能量、对应等级及难度系数；`10000 bps` 表示完整基础难度，`5000 bps` 表示基础难度的 50%，不是成功率 |
| `BTC height` | 本次矿工资格和经济数据使用的 BTC 稳定状态高度，可能落后于 Bitcoin 前台链尖端 |
| `Chain ... height` / `peers` | USDB 链当前高度和连接的节点数量；链高度为 `0` 表示尚未产生创世块之后的区块 |
| `impact` | 执行角色切换的影响范围；`Recreate only usdb-chain` 表示只重建链服务，保留数据和上游进程 |

零能量的有效 Active Standard pass 仍可参与挖矿。`energy=0、level=0、difficulty=10000 bps` 表示按完整基础难度挖矿。

### economics：余额、供应量和首块发行估算

金额单位：**1 USDB = 10¹⁸ atoms，1 BTC = 10⁸ sats**。`atoms` 是 USDB 的最小单位；输出中使用十进制字符串保存大整数，以免丢失精度。

| 字段 | 含义 | 示例换算 |
| --- | --- | --- |
| `total_miner_btc_sats` | 全网所有活跃 Standard / Collab pass 的不同 owner 的 BTC 余额合计，使用本次查询的稳定状态；不是单个收益地址的 USDB 余额 | `18894` = **0.00018894 BTC** |
| `unit_sats` | 能量增长的余额单位；每个 owner 按 `floor(BTC 余额 sats / unit_sats)` 得到整数单位数 | `100000` = **0.001 BTC** |
| `issued_usdb_atoms` | 全链累计已发行量，包含创世初始分配和此前各块新增发行；普通转账不改变该值，销毁也不从中扣除 | `10000000000000000000` = **10 USDB** |
| `target_usdb_atoms` | 按当前活跃矿工 BTC 总余额与协议价格计算的目标供应量，会随输入变化；不是本次到账奖励或固定供应上限 | `18894000000000000000` = **18.894 USDB** |
| `first_block_emission_atoms` | 满足 `estimate_assumption` 所列条件时，USDB 首个区块的新增发行估算，不含交易手续费；不是钱包当前余额或每块固定奖励 | `56405377980720` = **0.000056405377980720 USDB** |

例如，链高度为 `0`，累计已发行 `10 USDB`，活跃矿工 owner 总余额为 `18894 sats`。当前 v1 协议采用固定价格 **100000 USDB/BTC**；这是发行计算参数，不是市场报价。按首块 `K=1`、BTC 状态不变计算：

```text
目标供应量 = 0.00018894 BTC × 100000 USDB/BTC = 18.894 USDB
待发行差额 = max(18.894 − 10, 0) = 8.894 USDB
首块新增量 = 待发行差额 / 157680，按 atoms 向下取整
           = 0.000056405377980720 USDB
```

`157680` 是当前协议用于平滑发行的参数，不表示每隔这么多块才发一次奖励。后续每块会根据当时的余额总量、已发行量和 `K` 系数重新计算。目标供应量不高于已发行量时，新增发行量为零。

`unit_sats` **不是挖矿最低余额门槛**。余额不足 `100000 sats` 时没有新的余额单位用于增长能量，但仍按实际 sats 计入上述供应量计算；新增加的余额需要先进入稳定状态，再随 BTC 区块积累能量。因此零能量与正的发行估算可以同时出现。

当前工具只在链高度为 `0` 且使用受支持的 v1 策略时展示这组首块估算；开始出块后，相关估算字段可能不再出现。若显示 `estimate_unavailable`，表示估算信息暂不可用，不等于矿工资格无效。实际出块和收益以链上结果为准。

### bootstrap：网络初始化进度

`fee_split_block` 是手续费分成开始生效的 USDB 区块高度，`blocks_remaining` 是到该高度还剩多少块；例如链高为 `0`、`fee_split_block=8192` 时，剩余 `8192` 块。`bootstrap_finalized=false` 表示 SourceDAO 初始化尚未完成。网络初始化负责人应在链开始出块后按 [SourceDAO 章节](../network-admin/sourcedao.md)操作，不要等到第 `8192` 块才开始初始化；普通矿工无需执行该管理操作。

## 判断是否已经在挖矿

```bash
usdb-node mining status
```

| 状态 | 含义与下一步 |
| --- | --- |
| `DISABLED` | 已关闭本地挖矿；普通 full 节点的正常状态 |
| `SWITCHING` | 配置正在应用，继续观察后台任务 |
| `WARMING_UP` | 挖矿配置已生效，正在准备数据或等待有效工作 |
| `ACTIVE` | 已观察到本机挖矿开启且有有效工作 |
| `WAITING` | 上游、资格或观察暂不可用，读取具体原因 |
| `STALE` | 当前查询失败，显示近期旧观察；不能据此认定仍在正常挖矿 |
| `FAILED` / `BLOCKED` | 任务失败、配置不匹配或恢复条件阻断；先查日志 |

`ACTIVE` 不保证本机已经产块。链高度增加可能来自其他矿工。需要核对本机产块时，查看输出中的 `last_local_seal`；只有完整区块哈希已与当前链核对且 `canonical=true` 时，才有这次本地产块的主链证据。

该字段只检查近期本地日志，为空不证明本机从未产块，也不提供累计产块统计。PoW 出块耗时不固定，不根据一个短观察窗口承诺下一块的时间。

结构化采集使用：

```bash
usdb-node mining status --json
```

## 停止挖矿

```bash
usdb-node mining disable
usdb-node mining status --watch
```

任务完成、状态为 `DISABLED` 后，节点以 full 角色继续运行。这个选择会持久保存；上游暂不可用时工具也提供停矿路径，但 Docker 不可用或 SourceDAO 任务仍活动时需先处理对应错误。

`usdb-node down` 只是停止节点，不取消矿工角色。之后 `up` 会按原配置恢复，所以需要长期停矿时使用 `mining disable`。

初始化任务尚未结束时不要停掉唯一矿工。计划停节点、换地址或调整挖矿配置前，先确认 SourceDAO 和当前角色切换任务均已结束。

## 常见问题

| 现象 | 检查与处理 |
| --- | --- |
| 没有合格 pass 或收益地址不匹配 | 核对地址和 pass 的生效状态，等待上游索引追平后重新检查 |
| `PEER_SOURCE_REQUIRED`、无 peers 或仍在同步 | 普通加入节点先完成入网；确认负责创建网络的首节点使用本页的[首节点分支](#网络首节点启用挖矿) |
| `BITCOIN_NOT_READY` | 新版 AssumeUTXO 检查已取得 Bitcoin 状态，但基线或前台链尚未就绪；查看提示中的高度、headers、连接数和链头年龄 |
| `BITCOIN_RPC_TIMEOUT`、`BITCOIN_RPC_UNAVAILABLE` | 未取得可靠的 RPC 结果，不能据此判断追块进度；查看失败方法、尝试次数和 Bitcoin 日志，待 RPC 恢复后重试 |
| `BITCOIN_RPC_AUTH_FAILED` | 核对节点的 Bitcoin RPC 认证配置；等待同步不能解决认证错误 |
| `BITCOIN_RPC_INVALID_RESPONSE`、`BITCOIN_RPC_ERROR` | RPC 返回格式或错误码异常；核对工具与运行镜像版本，并保存脱敏诊断信息 |
| `BITCOIN_PROBE_TIMEOUT`、`BITCOIN_PROBE_FAILED` | 探测程序整体超时、未能执行或输出无法解析；核对 Docker 可用性、访问权限和工具/镜像版本 |
| `CHAIN_NOT_RUNNING` | 链容器或 geth 进程尚未就绪。若刚执行 `up`，运行 `usdb-node status --watch`，等待链服务启动后重试；若节点已停止，先执行 `usdb-node up`。持续无法启动时检查 `usdb-node logs usdb-chain` |
| `RESOURCE_LIMIT_REQUIRED` | 链已运行，但容器没有内存上限；按[调整资源预算](maintenance.md#调整资源预算)核对并应用资源配置，再重试预检 |
| `RESOURCE_TRANSITION_PENDING` | 自动资源配置仍在切换，等待状态面板显示 steady 阶段后重试 |
| 长期 `WARMING_UP` | 检查 `usdb-node logs usdb-chain`、CPU/内存及上游进度，不只凭哈希率为零判断失败 |
| 任务失败或配置漂移 | 保存 `mining status --json` 和链日志；解决具体错误后按同一目标重试 enable/disable |
| 上游回滚或停链标记阻断 | 保留现场，按网络事故流程处理，不清除标记强行恢复 |

在 AssumeUTXO 模式下，Bitcoin 基线和前台链尖端必须就绪，BH 与 indexer 也必须通过共识就绪检查。Bitcoin 后台历史验证可以继续运行，`history_validated=false` 本身不阻止挖矿预检。

旧版工具可能在链容器尚未创建时误报 `RESOURCE_LIMIT_REQUIRED`。若刚启动节点，先观察启动进度并在链就绪后重试；不能仅凭这条旧提示认定内存配置有误。包含启动诊断修复的版本会先检查链是否运行，再检查实际资源限制。

**r25 已知问题**：挖矿预检错误地使用旧版 Bitcoin 状态解析器，可能在状态面板显示 Bitcoin READY 时仍报 `BITCOIN_NOT_READY`。若 `native_bootstrap.core.bootstrap_ready` 和 `tip_ready` 都为 `true`，向网络运维方取得包含该修复的工具版本；单纯等待后台历史验证完成不能修复这个版本问题。不要修改快照模式或启用 txindex 来绕过。

包含 RPC 诊断改进的版本会将上述错误分别报告；r27 等旧版可能统一显示 `BITCOIN_NOT_READY: Bitcoin RPC is unavailable`，其中还可能包括探测输出解析失败。这条旧提示不表示后台历史区块必须全部追完。

新版每轮 Bitcoin 探测对短暂超时、连接失败或 Core 正在初始化等情况最多尝试 3 次，重试间隔 2 秒，共用 45 秒等待上限；这不是整个 mining 命令的总耗时上限。重试进度写入标准错误，不混入 JSON 输出。每次都重新查询，认证失败、响应格式异常和基线身份不匹配不会通过重试放行。

若当前报告是 `rpc_unavailable`，说明这次查询没有取得就绪证据。查看 `usdb-node logs --bitcoin`：Core 大规模写入 UTXO 数据或核对快照时可能暂时无法响应 RPC，应等待该操作结束后重试检查，不能仅凭容器仍在运行认定就绪。新版 JSON 的 `native_bootstrap.core.rpc_failure` 会提供失败方法、分类、错误码及是否可重试，不包含 RPC 密码或原始服务端错误内容；旧镜像没有这些字段时，新工具仍能给出通用的 RPC 不可用提示并有限重试。

`Artifact signature verified` 表示发布材料签名验证成功。旧版工具在多个检查入口重复打印相同提示，不表示重复下载或重新执行 BH 基线核对。

需要帮助时提供状态、操作目标、网络与版本、脱敏日志及相关时间。不要提供收益地址私钥或 Bootstrap Admin 私钥。
