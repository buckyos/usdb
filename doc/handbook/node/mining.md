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
| `BITCOIN_NOT_READY` | 查看 `usdb-node status --progress-json` 的 Bitcoin 状态；AssumeUTXO 模式及 r25 已知问题见下方说明 |
| `CHAIN_NOT_RUNNING` | 先启动 full 节点并检查链及上游状态 |
| 长期 `WARMING_UP` | 检查 `usdb-node logs usdb-chain`、CPU/内存及上游进度，不只凭哈希率为零判断失败 |
| 任务失败或配置漂移 | 保存 `mining status --json` 和链日志；解决具体错误后按同一目标重试 enable/disable |
| 上游回滚或停链标记阻断 | 保留现场，按网络事故流程处理，不清除标记强行恢复 |

在 AssumeUTXO 模式下，Bitcoin 基线和前台链尖端必须就绪，BH 与 indexer 也必须通过共识就绪检查。Bitcoin 后台历史验证可以继续运行，`history_validated=false` 本身不阻止挖矿预检。

**r25 已知问题**：挖矿预检错误地使用旧版 Bitcoin 状态解析器，可能在状态面板显示 Bitcoin READY 时仍报 `BITCOIN_NOT_READY`。若 `native_bootstrap.core.bootstrap_ready` 和 `tip_ready` 都为 `true`，向网络运维方取得包含该修复的工具版本；单纯等待后台历史验证完成不能修复这个版本问题。不要修改快照模式或启用 txindex 来绕过。

若当前报告是 `rpc_unavailable`，则这次查询确实没有取得就绪证据。查看 `usdb-node logs --bitcoin`：Core 大规模写入 UTXO 数据时可能暂时无法响应 RPC，应等待该操作结束后重试检查，不能仅凭容器仍在运行认定就绪。

`Artifact signature verified` 表示发布材料签名验证成功。旧版工具在多个检查入口重复打印相同提示，不表示重复下载或重新执行 BH 基线核对。

需要帮助时提供状态、操作目标、网络与版本、脱敏日志及相关时间。不要提供收益地址私钥或 Bootstrap Admin 私钥。
