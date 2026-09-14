# 故障排查

[返回手册首页](../README.md) · [版本与验证范围](../networks/testnet.md#版本与验证范围)

先执行 `usdb-node status`，记录版本、`Overall` 和第一项明确失败。正常同步或等待入网不需要重新安装。以下命令均在原运维账号下执行；跟随日志和进度的命令用 Ctrl+C 退出。

## 按现象查找

| 现象 | 处理入口 |
| --- | --- |
| `usdb-node: command not found` | [命令找不到](#找不到-usdb-node-命令) |
| 404、下载失败、校验不通过、拉取镜像被拒绝 | [下载问题](#下载失败或校验不通过) |
| `node.env template is missing keys`，包含 P2P 字段 | [r25 首次配置问题](#首次配置缺少-p2p-字段) |
| Docker 无权限、版本太低、sudo 失败 | [主机和权限](#docker-权限或主机检查失败) |
| `node is already configured` | [已有配置](#提示已有配置) |
| 磁盘不足、目录容量不符合要求 | [磁盘问题](#磁盘空间不足) |
| 下载、导入或同步看起来长时间不动 | [同步等待](#同步或导入看起来卡住) |
| `status --watch` 报 `KeyError: 'sync_start_height'` | [进度面板退出](#进度面板报-keyerror-后退出) |
| `AWAITING_PEERS`、没有实际连接 | [入网问题](#等待-peers-或一直没有连接) |
| 后台任务失败、服务不健康、`BLOCKED` | [后台或服务失败](#后台任务或服务失败) |
| 更新工具后要求激活或提示不兼容 | [升级问题](#升级后要求激活或兼容检查失败) |
| 矿工预检失败、长期等待工作、无法确认本机产块 | [矿工常见问题](../node/mining.md#常见问题) |
| SourceDAO 初始化失败、等待回执或导出/验证失败 | [SourceDAO 中断与失败恢复](../network-admin/sourcedao.md#中断与失败恢复) |
| `A SourceDAO task is active`，停机或升级被拒绝 | [SourceDAO 任务互斥](../network-admin/sourcedao.md#维护期间的互斥与备份) |

## 找不到 usdb-node 命令

**适用范围**：安装器曾成功完成，当前终端找不到命令。

检查当前账号及安装入口：

```bash
id -un
ls -l "${HOME}/.local/bin/usdb-node"
```

如果文件存在，先在当前终端恢复 PATH：

```bash
export PATH="${HOME}/.local/bin:${PATH}"
usdb-node --help
```

希望以后 Bash 登录也能找到命令，可以在自己使用的登录配置（通常为 `~/.profile`）中加入上面的 `export PATH` 一行，保存后重新登录。不要重复追加，也不要改写系统级 PATH。

如果入口不存在或是失效的链接，检查是否切换了账号、安装是否失败、工具目录是否仍存在。不要切到 root 重新配置节点。

**恢复标志**：同一账号能正常显示命令帮助。仍失败时提供账号是否变化、安装器末尾错误及该入口的存在情况。

## 下载失败或校验不通过

**适用范围**：下载安装脚本、启动数据或拉取镜像失败。

先记录失败 URL 所属域名、HTTP 状态或完整错误码，以及失败发生在安装器、镜像拉取还是同步阶段。涉及私有访问参数时只保留脱敏后的地址。

| 现象 | 判断与处理 |
| --- | --- |
| GitHub 下载返回 404 | 检查指定版本的 Release 页面是否公开、是否有准确同名附件。不要把源码压缩包或旧版本附件当作替代；附件未公开时联系网络运维方 |
| DNS、超时、连接重置 | 检查主机 DNS、出站 HTTPS、代理和网络连通性。不要因为连接重置就关闭 TLS 校验 |
| Docker 镜像 `denied` / `unauthorized` | 保留具体镜像及错误，确认目标版本所需镜像是否已公开。公共节点流程不应要求自行寻找内部 token |
| `sha256sum`、签名或文件完整性错误 | 停止该步骤。核对版本、下载来源及磁盘状态；不跳过校验，也不修改校验值 |

安装脚本的下载失败，可以在网络恢复后重新执行同一安装步骤。后台启动文件下载失败，先看 `usdb-node controller logs --follow`；只有状态允许继续时才重新 `up`。明确 `BLOCKED` 或不确定导入请求按[后台或服务失败](#后台任务或服务失败)处理。

保留未完成下载和已有数据，不能把删除目录作为续传的前提。

**恢复标志**：下载及校验完成，或者相应下载量持续增长并进入下一阶段。仍失败时提供版本、阶段、时间、状态码和相关日志片段。

## 首次配置缺少 P2P 字段

**适用范围**：公开的 `usdb-testnet-v0-r25`，首次 `setup/configure` 确认后报错：

```text
node.env template is missing keys: ['USDB_P2P_…', …]
```

这是 r25 工具与随包配置模板之间的问题，不是 Seed 填写错误或 IPv6 不可达。选择 IPv4 仍会失败。查看 `usdb-node status` 开头的版本号，确认是否为 r25。

向网络运维方取得包含修复的发布版本及其安装说明。不要编辑发布包中的模板、填写虚假的网络字段或清空数据来绕过检查。当前[版本页](../networks/testnet.md#版本与验证范围)尚未确认替代安装版本。

该失败路径会撤回本次新建的 `node.env` 和 Bitcoin RPC 凭据，但可能已经创建数据目录。取得修复版本后，按其说明重试配置；不要额外清理这些目录。

**恢复标志**：新版本向导成功结束，随后 `doctor` 通过。仍失败时提供安装版本及缺失字段列表，不提供完整私有配置。

## Docker 权限或主机检查失败

**适用范围**：`prepare-host`、`host check`、`setup` 或 `doctor` 提示主机依赖、Docker 或 sudo 问题。

先检查：

```bash
id
docker version
docker compose version
usdb-node host check
```

如果账号已加入 Docker 组但当前终端还未生效，退出并以原账号重新登录。希望保留 SSH 连接时，也可以执行：

```bash
newgrp docker
```

在新 shell 中重新运行 `usdb-node host check`；退出这个 shell 后，原终端不会因此获得新权限。

如果账号尚未授权，重新按 `prepare-host` 的提示完成准备。组权限已生效但 Docker 仍无法访问时，检查 Docker 服务和 socket；版本过低时更新实际 Docker Engine 和 Compose，不能只更新 CLI。

sudo 验证当前运维账号的权限和密码。若 `setup` 已保存配置、只在安装 controller 时失败，解决 sudo 或 systemd 问题后执行：

```bash
usdb-node controller install
usdb-node doctor
```

不需要再次 `setup`。不支持 systemd 的主机不属于本批默认后台部署路径。

**恢复标志**：原运维账号的主机检查通过；已有配置的节点 `doctor` 通过。仍失败时提供主机系统、内核、Docker/Compose 版本及第一项失败信息。

## 提示已有配置

**适用范围**：重复 `setup` 提示 `node is already configured`。

```bash
usdb-node status
usdb-node up --dry-run
```

如果是原节点继续运行，使用现有配置，按状态恢复启动。如果是升级，按[升级节点](../node/maintenance.md#升级节点)执行。如果显示 `UNCONFIGURED` 却确定以前配置过，先核对是否使用了不同账号或不同网络的安装器。

工具拒绝重复配置是为了保留已有账号凭据和数据关系，不应通过删除 `node.env` 解决。更换数据盘、换网络或旧模式重建，需要单独安排。

**恢复标志**：工具识别原配置，并给出与实际场景一致的下一步。仍失败时提供原/当前版本、账号是否变化和状态摘要。

## 磁盘空间不足

**适用范围**：首次配置未通过容量检查，或运行中出现空间不足。

检查实际数据目录所在文件系统，例如：

```bash
findmnt --target /data
df -h /data
df -i /data
```

换成自己的挂载点。首次配置要求总容量和当前可用空间都至少 1.5 TiB；磁盘标称容量、目录名称和是否刚创建目录都不能替代这个检查。`df -i` 用于查看是否耗尽文件节点。

先核对数据盘是否已挂载、备份是否占用同一磁盘，以及其他服务是否在增长。尚未配置的新节点可以选择满足要求的磁盘；已有节点需按维护窗口扩容或迁移，不能只修改数据路径。

不要删除 RocksDB/WAL、Bitcoin 数据或原生 UTXO 启动文件。r25 原生运行仍需要保留启动文件，导入完成不表示可以回收它。也不要套用旧快照 GC 清理原生数据。

**恢复标志**：容量满足当前操作要求，空间错误消失，相应工作恢复推进。仍失败时提供挂载点、总量、可用量、inode 余量及失败阶段。

## 同步或导入看起来卡住

**适用范围**：原生启动中的下载、导入、Bitcoin 或数据服务同步。

```bash
usdb-node status --watch
```

记下发生等待的组件和阶段，退出面板后查看相应日志：

```bash
usdb-node controller logs --follow
```

再按服务选择 `usdb-node logs --bitcoin`、`usdb-node logs balance-history` 或 `usdb-node logs usdb-indexer`。每条日志命令分别运行。

| 现象 | 如何判断 |
| --- | --- |
| 下载 100%，仍未就绪 | 后面还有文件校验和导入，查看当前阶段 |
| 导入阶段没有百分比 | r25 不一定提供详细导入进度；看阶段、耗时和 Bitcoin 日志 |
| BH 重放结束后仍等待 | 可能在校验、落盘或等待服务开放查询，看最新日志 |
| `waiting_for_blocks` | 在等待所需区块或相关数据；同时观察 Bitcoin 是否继续推进 |
| indexer 提示 `UpstreamSnapshotMissing` | 指上游尚未提供可查询的稳定状态；检查 BH 进度，不据此重新下载旧 BH 快照 |
| `Connection refused/reset` | 上游服务可能尚在打开或恢复数据库，也可能失败；结合容器状态、重复退出和日志判断 |
| 前台 `READY`，后台历史验证还未完成 | 两者独立，继续运行并观察后台验证 |

最近日志、处理量或高度持续变化时继续等待。RPC 不可用期间面板可能暂时缺少可靠进度，单次显示变化不证明数据丢失。长时间没有新进度且重复报错时，转到[后台或服务失败](#后台任务或服务失败)。

**恢复标志**：组件进入后续阶段、处理量继续增长，最后通过整体状态和入网检查。仍失败时提供两次带时间的观察结果、阶段及同一时间段日志。

## 进度面板报 KeyError 后退出

**适用范围**：r25 的 `usdb-node status --watch` 报 `KeyError: 'sync_start_height'`，也可能涉及 `stable_lag_blocks`。

这是连续刷新时的显示问题：面板已经观察到 BH 基线可用，随后容器重建或启动状态使部分进度字段暂时缺失，旧版面板可能退出。单凭该报错不能判断基线核对失败，也不需要为此重建数据或重启服务。

先用单次查询确认实际状态；需要继续观察时重复执行：

```bash
usdb-node status
usdb-node status --progress-json
```

`native_bootstrap.balance_history.phase=sealed` 表示基线已封存；是否恢复正常服务还要看 `components` 中 balance-history 和 indexer 的当前状态。若 `Overall` 是 `AWAITING_PEERS`，按[入网问题](#等待-peers-或一直没有连接)处理；服务明确失败时按[后台或服务失败](#后台任务或服务失败)处理。

保留版本和报错信息，向网络运维方取得包含面板修复的工具版本。**恢复标志**：连续刷新跨过容器启动或短暂观测不可用阶段后，面板继续显示；旧进度标记为 `STALE`，实时观测恢复后正常更新。

## 等待 peers 或一直没有连接

**适用范围**：普通加入节点显示 `AWAITING_PEERS`、`SEED_REQUIRED` 或 `WAITING_FOR_PEERS`。

```bash
usdb-node peers list
usdb-node peers status
usdb-node peers network --json
```

`SEED_REQUIRED` 表示缺少引导来源，按[安装页第 6 步](../node/install.md#6-补充-seed-并确认入网)添加。`WAITING_FOR_PEERS` 表示已有来源但尚无连接，逐项确认：

1. Seed 来自同一网络，地址和 P2P 端口正确，对端正在运行。
2. 本机和对端的云安全组、主机规则及 NAT 转发允许实际 TCP/UDP 端口。
3. IPv6 Seed 有对应的主机和容器 IPv6 连通性；只保存 IPv6 地址不等于网络已可用。

先在节点主机查询地址和路由；缺少 IPv6 路由或报 `P2P_IPV6_RA_REQUIRED` 时，由主机网络管理员修复报错中指定的接口和路由。不要在未知接口上修改系统网络参数。

如果是配置应用任务失败，修复报告中的原因后执行：

```bash
usdb-node peers apply
usdb-node peers status --watch
```

只在任务失败且条件已修复时重试；纯粹等待对端连通不需要反复重建链。应用运行中配置会短暂中断链 RPC/P2P。

**恢复标志**：出现实际连接，加入状态为 `READY / CONNECTED`，整体 `status` 通过；不要只看 `APPLIED`。仍失败时提供状态原因、所用地址族、实际端口及脱敏连接信息。普通加入节点不能用 `--first-node` 消除等待。

## 后台任务或服务失败

**适用范围**：controller `failed`，总体 `DEGRADED/BLOCKED`，或组件明确 `FAILED`。

先分别检查：

```bash
usdb-node status
usdb-node controller status
usdb-node doctor
usdb-node controller logs --follow
```

退出后按状态选择故障服务的日志。读取失败项时区分以下情况：

- 主机权限、下载连通性或磁盘问题：先修复具体条件，再用 `up --dry-run` 判断是否可以继续。工具仍阻断时保留输出，不循环重试。
- 配置应用失败：按该任务的恢复入口处理，例如 `peers apply`。
- SourceDAO 任务活动导致操作被拒绝：使用 `usdb-node sourcedao status --watch` 观察独立任务；不能通过停止 controller 或删除锁来绕过。具体见[SourceDAO 章节](../network-admin/sourcedao.md)。
- 原生导入显示 `load_uncertain`、`load_failed`，或准备任务退出失败：保留导入记录和日志，交由网络运维人员核对是否仍有活动导入，并按对应版本的恢复说明操作。普通 `up` 不一定能恢复这种情况。
- 数据库损坏、网络身份不匹配或深度重组导致停链：保存现场并联系网络运维方。不要删除锁、修改身份、清除停链记录或丢弃数据库来强行启动。

如果 controller 只是 `inactive (dead)`，先核对节点是否已 `READY` 或正在等待 peers；这可能是本轮编排正常结束。

**恢复标志**：明确失败已消除，服务继续推进并最终恢复整体运行；只提交成功或容器重新出现不算恢复完成。仍失败时按下面的求助清单提供信息。

## 升级后要求激活或兼容检查失败

**适用范围**：安装了新工具，但 `status` 显示 `ACTIVATION_REQUIRED`，或者 `activate-release/doctor` 拒绝配置或数据。

```bash
usdb-node status
usdb-node doctor
```

若目标版本确认可以保留现有数据，按[完整升级顺序](../node/maintenance.md#升级节点)完成停机、安装、激活、controller 刷新、检查和启动。不重复 `setup`。

若错误涉及网络、创世块、数据兼容或已有数据目录，停止升级并确认目标版本；旧 BH 模式到 AssumeUTXO 不能直接原地激活。保留原工具包、配置备份和数据，不修改检查值，也不默认用旧二进制打开新数据。

**恢复标志**：版本激活检查通过，节点实际恢复同步和连接。仍失败时提供升级前后版本、第一条不兼容错误和是否曾更换数据目录。

## 需要协助时提供什么

通过网络运维方提供的支持渠道提交以下信息：

1. 安装版本、网络名称、机器系统及 Docker/Compose 版本。
2. 出错时间和时区、正在执行的命令、最近是否升级或重启。
3. `status` 的总体状态与第一项失败；同步问题补充两次带时间的进度观察。
4. 对应阶段附近的日志片段；连接问题补充地址族和实际端口；容量问题补充 `df` 结果。

矿工问题另提供 `mining status` 的状态及操作目标。SourceDAO 问题另提供 `action/outcome`、部署阶段、当前区块和待确认交易哈希；核对最近任务是 bootstrap、export 还是 validate。原始签名交易和私有部署恢复文件不作为公开附件。

不要上传完整 `node.env`、私钥、助记词、钱包文件、RPC 密码或 token。日志和 JSON 输出也要检查并移除带凭据的 URL、私有访问参数及不需要公开的主机信息。配置和数据库需要保留在本机或受控备份中，不作为公开工单附件。

新增故障条目时沿用“现象、适用版本、检查、判断与处理、恢复标志、求助信息”的结构。特定版本问题标明受影响版本和已验证的修复版本。
