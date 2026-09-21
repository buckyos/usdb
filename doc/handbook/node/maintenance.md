# 日常维护

[返回手册首页](../README.md) · [版本与验证范围](../networks/testnet.md#版本与验证范围)

适用范围：已经配置的节点。始终使用原安装账号和原数据目录。普通重启、补充 Seed 和同契约升级均不需要重新执行 `setup`。

## 日常检查

```bash
usdb-node status
usdb-node peers status
usdb-node resources
```

`status` 检查整体运行，`peers status` 检查实际入网，`resources` 展示配置的内存预算。预算不是实时内存用量；应结合主机监控观察 CPU、内存、磁盘余量和 I/O。

磁盘检查使用实际数据路径，例如：

```bash
df -h /data/usdb
free -h
```

不要只检查系统盘。发现空间持续减少时提前扩容或调整部署；不要删除数据库文件、仍在使用的启动文件或未完成的下载来临时绕过错误。

矿工节点另查 `usdb-node mining status`；执行过网络初始化的节点另查 `usdb-node sourcedao status`。主节点 `READY` 不替代这两类任务的结果。

## 编辑已有配置

新版 `usdb-node setup` 会识别已有 `node.env` 并进入编辑模式，以当前配置为默认值。旧版若仍提示
`node is already configured`，先升级 node kit，或继续使用下表中的独立命令。

```bash
usdb-node down
usdb-node setup
usdb-node doctor
usdb-node up
usdb-node status --watch
```

必须停止整个节点，不能使用 `down --keep-bitcoin` 代替。向导不会自动停止服务；容器仍在运行、
状态无法确认，或存在未完成的 Peer、Mining、SourceDAO 操作时，会拒绝修改并提示处理方法。

向导支持以下配置；原有独立命令继续可用：

| 配置 | 向导行为 | 独立入口 |
| --- | --- | --- |
| 完整 Explorer 支持 | 同时启用 archive 和私有 tracing；已有独立组合默认保留 | `set-query-mode` |
| 本机铸造后端 | 开关 txindex + Ord，直接使用推荐资源值，已有自定义值保留 | `set-minting --enabled on/off`、`minting-status` |
| 整机资源 | 保留当前 auto/manual，按需调整预算或显式重算 | `set-resource-policy`、`set-bitcoin-profile`、`resources` |
| Bitcoin 入站连接 | 选择本机访问或公开 P2P 监听，保留已有端口 | 原有配置文件中的 Bitcoin P2P 设置 |
| 主机防火墙 | 修改 external/managed 策略和运维 SSH 端口 | `set-firewall-mode`、`firewall apply/check` |

直接回车保留当前值；全程不修改选项时，不改写文件、不重算资源、不重置进度记录。启用/关闭 Ord、
修改资源输入或明确选择重算时，才重新计算预算并清理旧资源切换记录。已有自动模式节点保留当前
`bitcoin / overlap / steady` 阶段，不会仅因开启 txindex 回退到 Bitcoin 独占阶段；启动时仍检查实际就绪状态，
按原策略推进阶段。新建配置或从 manual 切换至 auto 时从 `bitcoin` 阶段开始。已有链数据和索引保留。
开启 Ord 会在各阶段预留其内存，因此 BTC、BH 等服务的预算可能相应缩小；这些是自动策略的计算结果，
不是新增的手动配置。修改摘要和容量提示使用 MiB/GiB/TiB（1024 进制），配置文件仍保存精确数值，回车不会按显示精度取整。
开启 archive 不会恢复已裁剪历史；开启 Ord 后仍需等待历史验证、txindex 和 Ord 索引完成，详见[本机铸造后端](../services/control-plane.md#可选的本机铸造后端)。
已有节点首次启用 Ord 时，`setup` 和 `set-minting --enabled on` 都要求目标磁盘至少有 **300 GiB 可用空间**。
容量不足会拒绝保存，保留原配置和索引；复用同一版本的已有索引不重复要求一份新的 300 GiB 预算。
这与运行时默认 **50 GiB 最低剩余空间**是两项独立检查，Bitcoin `txindex` 的新增占用还需另外考虑。

保存前显示修改摘要并校验完整候选配置。取消或校验失败保留原配置；成功保存时，将此前配置备份为同目录的
`node.env.setup-backup`，权限为 `0600`，下一次保存会替换这份备份。数据目录、RPC 凭据、矿工地址、节点身份、
Seed、P2P 地址族、快照选择和 release 镜像保持原值。角色/挖矿变更仍使用 `set-role` 或 `mining enable/disable`，
Seed/P2P 变更使用 `peers` 命令；编辑已有节点时不要给 `setup` 传首次安装的 P2P 参数。

**Node 的修改在下一次 `up` 时使用，不需要 Explorer 的 `prepare --replace`。** 编辑不会重装 controller，也不会直接修改系统防火墙。
若改动影响 managed 防火墙，先按向导提示执行 `usdb-node firewall apply --confirm` 并用 `usdb-node firewall check` 验证，再运行 `doctor/up`。
改成 external 不会移除已经存在的 UFW 规则，需要按实际主机策略维护。原本未安装 controller 的节点继续使用 `up --foreground`，或显式执行 `controller install` 后使用后台启动。

升级 release 与编辑配置是两件事：安装新工具后，先按[升级步骤](#升级节点)激活目标 release，再按需运行 `setup`。
编辑向导不会替你切换镜像；发现仍引用旧 release 时，会提示 `activate-release`。

## 持续观察与告警

无人值守节点应将[状态采集](status.md#自动采集)接入自己的监控系统。至少关注以下变化：

| 观察项 | 需要处理的情况 |
| --- | --- |
| 服务和后台任务 | 新出现 `BLOCKED`、`DEGRADED`、`FAILED`，或进程反复退出 |
| 同步和入网 | 持续没有新进度且日志重复报错；原本有连接的节点持续无 peers；网络在出块而本节点长期不跟随 |
| 主机资源 | 数据盘余量持续下降、内存不足/OOM、磁盘错误 |
| 矿工 | 原本 `ACTIVE` 的节点持续等待或失败；结合资格、上游和日志判断 |
| SourceDAO 初始化 | 任务失败、长时间等待回执，或未完成时逐渐接近初始化期限 |

采集频率和持续时间阈值按主机同步速度及网络实际出块情况设置。保留带时间的连续观察；不要对每次正常等待或单次 RPC 超时自动重启服务。`doctor` 是启动前检查，重复 `sourcedao validate` 是旧检查点复验，都不能代替日常监控。

## SSH 中断后继续观察

以原账号重新登录，确认命令可用后执行：

```bash
usdb-node status --watch
```

默认后台方式下，SSH 中断或 Ctrl+C 不取消启动和同步任务。如果节点已处于 `READY`，继续日常观察即可。若节点尚未就绪且后台任务异常，先查看：

```bash
usdb-node controller status
usdb-node controller logs --follow
```

正常停止后需要继续运行时，先查看可执行的下一步：

```bash
usdb-node up --dry-run
```

确认允许继续启动后执行 `usdb-node up`。`BLOCKED`、`DEGRADED` 或明确导入失败时，先按[故障排查](../troubleshooting/README.md)处理，不能把重复 `up` 当作通用修复。

## 停止与重新启动

**先确认本机没有运行中的 SourceDAO 任务。** 执行过初始化、导出或验证的节点，先运行 `usdb-node sourcedao status`；任务活动时，停机、升级和矿工配置变更会被拒绝。等待任务结束；异常时按[SourceDAO 恢复步骤](../network-admin/sourcedao.md#中断与失败恢复)处理。停止 controller 不会终止独立的 SourceDAO 任务。

| 操作 | 命令 | 实际影响 |
| --- | --- | --- |
| 停止整个节点 | `usdb-node down` | 停止后台编排、USDB 服务和 Bitcoin，保留配置及持久数据 |
| 暂停 USDB、保留 Bitcoin | `usdb-node down --keep-bitcoin` | 停止 USDB 服务，Bitcoin 继续运行 |
| 恢复整个节点 | `usdb-node up` | 使用现有配置和数据继续启动 |
| 只停止后台编排 | `usdb-node controller stop` | 已经启动的容器继续运行；不是停机命令 |
| 关闭编排的开机自动启动 | `usdb-node controller disable` | 停止并禁用编排；已运行容器仍需用 `down` 停止 |

计划重启节点时执行：

```bash
usdb-node down
```

等待命令正常完成，再执行：

```bash
usdb-node up
```

Bitcoin 停止时可能需要较长时间完成写盘，终端会显示停机耗时。看到输出仍在推进时继续等待，不用强杀代替正常停止。如果命令报错，先处理错误，不马上关机。

`down --keep-bitcoin` 不适合整机断电、完整备份或需要停止全部数据服务的配置变更。启停会中断 RPC/P2P；矿工节点停链期间也无法继续本地挖矿。

`down` 保留矿工角色，后续 `up` 会按原角色恢复。若希望持久停止挖矿而保持节点服务，使用[矿工停用命令](mining.md#停止挖矿)。

## 主机维护和开机恢复

正常主机维护前先执行 `usdb-node down` 并等待成功，再通过主机管理流程重启。默认配置的后台编排已启用开机启动，开机后会继续检查和启动节点；重新登录后确认 `status` 和 `peers status`。

如果希望主机重启后**仍保持节点停止**，先按顺序执行：

```bash
usdb-node down
usdb-node controller disable
```

之后需要恢复默认行为时执行：

```bash
usdb-node controller install
usdb-node doctor
usdb-node up
```

节点已经就绪时，controller 显示 `inactive (dead)` 不需要重新安装或反复重启；以节点状态为准。

## 升级节点

先从网络运维方取得目标版本及其升级说明。确认它允许保留现有数据，并检查[版本页](../networks/testnet.md)中的已知问题。

| 变更 | 处理方式 |
| --- | --- |
| 同一网络，目标版本明确支持现有数据，工具兼容检查通过 | 可以按下面的原地升级顺序执行 |
| 旧 BH 快照/全量同步切到 AssumeUTXO | 单独安排数据保留和重建，不套用原地升级 |
| 网络重置、更换网络标识或创世块 | 按新网络部署通知执行，保留旧配置和数据供核查 |
| 版本或数据兼容检查失败 | 停在失败处，保留原版本和数据，联系网络运维方 |

**`rN` 数字增加不保证数据兼容。** 同样的网络名也不保证新旧数据库可以直接复用。

原地升级顺序如下，每一步成功后再继续：

1. 记录当前版本、节点状态和目标版本，确认 SourceDAO 任务已结束，安排停机窗口及所需备份。
2. 使用当前工具执行 `usdb-node down`，等待整个节点停止。
3. 按[安装页第 2 步](install.md#2-下载并安装节点工具)安装目标版本工具。只执行工具安装，不继续首次 `setup`。
4. 用新工具完成配置中的版本切换，并查看 controller 是否需要额外操作：

```bash
usdb-node activate-release
usdb-node status
```

正常 `setup` 已安装并启用 controller，普通换版不必重复安装。包含[controller 诊断](status.md#后台任务状态)的工具会在缺失或配置需要刷新时给出具体命令；按提示处理后再执行下面的检查。若目标版本未提供该诊断，按该版本升级说明判断是否需要刷新 unit。自定义 unit 或 systemd 覆盖配置先核对，不直接覆盖。

```bash
usdb-node doctor
```

5. 检查通过后恢复运行：

```bash
usdb-node up
```

6. 用 `status` 和 `peers status` 确认恢复到 `READY`，实际连接和同步正常。

安装新工具不等于运行中的服务已经升级。`ACTIVATION_REQUIRED` 应在完成切换后消失；兼容检查失败时，不手改配置中的镜像、数据身份或网络参数。

升级失败时保留原工具包、配置备份及日志。**重新指向旧工具不等于完成回滚**，运行镜像和数据也必须匹配；具体回退按目标版本的说明执行。

## 备份与恢复边界

备份应覆盖以下内容，并保存到节点数据盘之外的受控位置：

| 内容 | 需要保留的原因 |
| --- | --- |
| 原运行账号下 `~/.config/usdb/<网络标识>/` | 私有配置、凭据引用、节点任务和本地运维状态 |
| 实际数据根下的持久数据、节点身份和启动状态 | 恢复同步进度、链数据及节点身份；路径以配置为准 |
| 当前版本号、对应工具包与校验材料 | 恢复匹配的运行环境 |
| 故障或升级前后的相关日志 | 解释恢复时的数据和任务状态 |

配置目录和数据根需要作为同一次停机的一致备份保存。先执行 `usdb-node down`，等所有服务正常停止后，再用现有备份系统复制或快照这些目录。包含 Bitcoin 的完整备份不能使用 `--keep-bitcoin`。

私钥、钱包、管理员签名材料按独立的密钥保管方案备份，不上传公开工单。只复制 `node.env` 不能恢复整套节点；启动快照文件也不能代替运行中所有服务的完整备份。

执行过 SourceDAO 初始化的节点还需配套保留私有 `state.json`、`state.json.transactions.json`、任务记录和日志，公开导出不能替代这些恢复材料。任务结束后再做一致备份，具体见[SourceDAO 备份说明](../network-admin/sourcedao.md#维护期间的互斥与备份)。

恢复前核对原版本、网络、运行账号、挂载点、路径和文件权限。在独立恢复位置保留原备份，按备份方案完成恢复和验证后再投入使用，不直接覆盖仍在运行的节点。当前手册不提供跨版本数据库迁移或任意备份的一键恢复命令。

## 调整资源预算

默认使用自动内存策略。主机扩容或增加同机服务后，先查看 `usdb-node resources`。确需重新计算时，在停机窗口执行：

```bash
usdb-node down
usdb-node set-resource-policy --mode auto
usdb-node resources
usdb-node doctor
```

核对新的预算后执行 `usdb-node up`。已有自动模式节点重算时保留当前资源阶段。
只更新工具包不会自动重算已有预算；同机还有其他服务时，需要为它们预留实际额度，不能将节点预算之外的内存视作无限可用。

更多故障处理见[故障排查](../troubleshooting/README.md)。
