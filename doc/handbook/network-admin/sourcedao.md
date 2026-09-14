# 网络启动后初始化 SourceDAO

[返回手册首页](../README.md) · [版本与验证范围](../networks/testnet.md#版本与验证范围)

本章面向持有 **Bootstrap Admin** 签名权限的网络初始化负责人。在 USDB 链已经启动、能够持续出块后，通过 `usdb-node sourcedao` 完成 SourceDAO 初始化、查看进度并保存验证结果。

这是每个网络的一次初始化任务。普通节点加入已有网络时不重复执行；`usdb-node up` 也不会自动代替管理员签名。这里使用安装包提供的完整工具，无需安装 SourceDAO 源码、Node/npm 或手工配置 RPC。

## 1. 执行前确认

| 条件 | 如何确认 |
| --- | --- |
| 网络正确，链服务已启动 | `usdb-node status` 没有上游或链身份阻断 |
| 已有矿工持续出块 | 链高度持续增长；需要启用矿工时先阅读[CPU 挖矿](../node/mining.md) |
| 管理员身份正确 | 使用本网络配置指定的 Bootstrap Admin 私钥，地址以 `sourcedao check` 输出为准 |
| 管理员有支付交易费用的余额 | `check` 中管理员余额和阻塞项满足要求；余额为正不代表一定足够支付全部交易 |
| 初始化期限有余量 | 检查 `Dividend fee split` 的高度及剩余区块；当前 testnet-v0 必须在高度 **8192** 前完成初始化 |
| 没有并发管理操作 | 同一管理员不同时在其他脚本或钱包提交交易；本机没有正在进行的版本、矿工或节点配置切换 |

bootstrap 会逐笔提交交易、等待成功回执后继续，需要多个区块。首次出块后应及时开始，不要等到期限前最后几个区块。工具会检查剩余交易所需的区块余量，余量不足时拒绝执行。

任务期间保留持续出块的矿工。执行本章的节点可以是普通 full 节点，只要同网有其他矿工提供区块；不要求本机同时挖矿。

## 2. 检查初始化状态

```bash
usdb-node sourcedao check
```

此命令不使用私钥、不发送交易，也不创建部署恢复记录。首次使用时可能下载工具镜像并启动短时检查容器，因此需要正常 Docker 权限和网络。

检查输出中的网络、管理员、当前区块、`dao_initialized`、`dividend_finalized` 和 `Blocked`：

- **尚未开始且没有阻塞**：可以准备私钥文件并启动任务。
- **已经完成初始化**：不再启动新的 bootstrap；由原部署负责人核对已有导出和验证材料。
- **初始化未完成但有历史记录**：先按本章[中断与失败恢复](#中断与失败恢复)确认原任务。
- **有阻塞或观察失败**：先处理错误，不因为命令输出了 `CHECKED` 就认为可以部署。

普通加入节点可以使用 `check` 只读核对链上初始化状态；本机没有任务记录不代表整个网络从未初始化。

## 3. 提供管理员私钥文件并启动

由管理员通过受控方式将私钥文件放到执行节点。文件内容是一行十六进制 Bootstrap Admin 私钥，文件必须属于当前运维账号、为普通文件，并仅允许所有者访问；不能放在 SourceDAO 的公开导出目录或可写恢复目录里。

在原节点运维账号下输入**文件路径**：

```bash
read -r -p 'Bootstrap Admin key file: ' USDB_BOOTSTRAP_KEY_FILE
chmod 600 "$USDB_BOOTSTRAP_KEY_FILE"
usdb-node sourcedao bootstrap --key-file "$USDB_BOOTSTRAP_KEY_FILE"
```

命令只接受文件路径，私钥内容不要写入命令行、环境变量或聊天记录。Bootstrap Admin 与矿工收益地址是不同职责，不能用矿工地址的私钥替代。

命令返回 `STARTED` 和任务标识后，立即观察：

```bash
usdb-node sourcedao status --watch
```

`STARTED` 只表示任务已提交，不代表链上初始化成功。任务在独立容器中运行，SSH 中断或 Ctrl+C 只结束当前观察。重新连接后执行同一条 `status --watch` 即可。

## 4. 如何看进度

面板第一行显示**最近任务的动作和结果**，`Deployment` 显示**当前部署阶段**。例如 `bootstrap / RUNNING` 与 `Deployment: FINALIZED` 可以同时出现：最后一笔交易已上链，任务仍在核对和保存结果。

| `Deployment` | 含义与操作 |
| --- | --- |
| `NOT_STARTED` | 链上尚未初始化，本机也没有部署进度 |
| `STARTING` | 任务已启动，正在检查或准备交易 |
| `DEPLOYING` | 正在部署、初始化或绑定各模块 |
| `FINALIZING` | 已记录完成初始化的交易，正在等待回执 |
| `INCOMPLETE` | 已有部分部署或恢复记录，本机没有运行中的 bootstrap；先查原任务错误 |
| `FINALIZED` | 最新链上观察显示初始化完成标记；还需确认任务成功并完成导出、验证 |
| `UNKNOWN` | 当前观察或记录不足，不能判断未开始或已完成 |

面板还会显示当前步骤、已记录确认的交易数、待回执的操作和交易哈希。等待回执时先确认链是否继续出块；进度计数不替代最终验证，也不保证交易已被矿工接收。

**bootstrap 完成标志**：最近任务为 `bootstrap / SUCCEEDED`，且新鲜链上观察中的 `dividend_finalized=True`。需要详细核对时使用 `usdb-node sourcedao status --json`，其中 `bootstrap_status` 应为 `completed`。出现观察错误时先恢复连接，不把旧的完成状态当作当前结论。

## 5. 导出并验证初始化结果

bootstrap 成功后执行：

```bash
usdb-node sourcedao export
```

等待 `export / SUCCEEDED`，再执行：

```bash
usdb-node sourcedao validate
```

这两个命令都不使用私钥、不发送交易；默认等待后台任务结束。SSH 中断后仍用 `sourcedao status --watch` 查看结果。

首次 `validate` 应在治理、代币转移、兑换或锁仓等业务开始前尽快执行。它核对初始化时应有的状态，若业务已改变初始分配，验证失败需要结合业务时间线判断。

最后查看输出文件位置：

```bash
usdb-node sourcedao status --json
```

| 输出 | 用途与完成标志 |
| --- | --- |
| `public_state` 指向的 `sourcedao-bootstrap-public-state.json` | 导出的公开部署记录；须确认 export 成功，不是仅看路径已显示 |
| `validation` 指向的 `sourcedao-bootstrap-validation.json` | 完整验证报告；须确认 validate 成功，报告为 `status: "ok"`、`mode: "strict"`，并保留 `evidence.checkpoint` 中的区块信息 |

`export` 只验证本地恢复材料的一致性，`validate` 才检查指定区块的链上初始化结果。文件在本机生成，不自动发布到 GitHub 或其他服务。

重复 `validate` 会复验原报告的检查点，不会自动改为最新区块。因此它不是日常健康监控，也不等于 PoW 区块已不可重组。若网络交付另有确认深度或独立验收要求，还需按网络交付流程完成，不能仅凭 `SUCCEEDED` 宣布全部验收完成。

## 中断与失败恢复

先执行：

```bash
usdb-node sourcedao status
```

若输出提示工具容器失败，按错误中给出的 `docker logs <容器名>` 查看**该 SourceDAO 任务**的日志。`usdb-node logs usdb-chain` 是链日志，不能替代初始化任务日志。

| 现象 | 处理方式 |
| --- | --- |
| SSH 断开或退出观察窗口 | 用 `status --watch` 重连观察，不重复提交任务 |
| 主机/Docker 重启，任务中断 | 一次性签名任务不自动重新运行；先确认链恢复和原任务已停止，再使用同一私钥文件及原记录重试 |
| `RUNNING` 但长期等待回执 | 检查链是否继续出块、任务日志和待确认交易；不要停掉唯一矿工或并发使用同一管理员发交易 |
| 管理员无余额、链仍在同步或尚未出首块 | 修复 `check` 的具体阻塞项，再检查；不要更换为另一个管理员身份 |
| 剩余区块不足 | 联系网络初始化负责人制定恢复方案，不改写期限或重复提交 |
| 私钥文件权限/所有者错误 | 确认文件属于当前账号且为普通文件，权限为 `0600` 或 `0400`；保持原运维账号 |
| export 找不到已完成输入 | 确认 bootstrap 成功及私有记录完整，先恢复原任务再导出 |
| validate 找不到公开记录 | 先完成 export |
| 原检查点不可读或发生重组 | 保留原报告，检查节点历史数据和链状态；需要新检查点时由维护者另行处理 |
| 配置、网络、工具版本或恢复锁不匹配 | 保留现场并核对原版本与记录，不删锁或修改摘要 |

原任务已停止、报错原因已解决且原私有记录完整时，重新指定同一私钥文件并执行：

```bash
usdb-node sourcedao bootstrap --key-file "$USDB_BOOTSTRAP_KEY_FILE"
usdb-node sourcedao status --watch
```

重新登录后需先按第 3 节设置路径变量。工具依据原状态和交易记录恢复，不要删除它们来“从头开始”。容器丢失且锁归属不明等情况仍可能拒绝恢复，应保存错误后交由维护者处理。

## 维护期间的互斥与备份

SourceDAO 任务运行期间，重复初始化、切换版本、停节点和修改矿工配置会被拒绝。先观察到任务结束再安排维护。`controller stop` 或 `controller disable` 不会终止这个独立任务，也不能用来绕过互斥。

默认记录位于原账号的 `~/.config/usdb/usdb-testnet-v0/sourcedao/`，自定义配置路径时以 `status` 输出为准：

| 材料 | 保存要求 |
| --- | --- |
| 私有 `state.json` 与 `state.json.transactions.json` | 配套保存；用于恢复已签交易和部署进度，不能公开 |
| `task.json`、任务日志及原版本资料 | 与私有记录一起备份，保留任务归属和失败现场 |
| 公开部署记录、strict 验证报告 | 可以对外提供的交付材料；不能代替私有恢复记录 |
| Bootstrap Admin 私钥 | 使用独立密钥保管方案；只读提供给任务，不与公开文件一起分发 |

一致备份在任务结束后进行。不要将仍在运行时随手复制的两份私有文件当作已验证的恢复备份。

日常观察使用 `sourcedao check/status`；需要协助时提供动作、结果、部署阶段、区块高度、待确认交易哈希和脱敏日志，不提交私钥、原始签名交易或完整私有状态文件。
