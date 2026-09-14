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

1. 记录当前版本、节点状态和目标版本，安排停机窗口及所需备份。
2. 使用当前工具执行 `usdb-node down`，等待整个节点停止。
3. 按[安装页第 2 步](install.md#2-下载并安装节点工具)安装目标版本工具。只执行工具安装，不继续首次 `setup`。
4. 用新工具执行下列命令，完成配置中的版本切换并刷新后台服务：

```bash
usdb-node activate-release
usdb-node controller install
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

恢复前核对原版本、网络、运行账号、挂载点、路径和文件权限。在独立恢复位置保留原备份，按备份方案完成恢复和验证后再投入使用，不直接覆盖仍在运行的节点。当前手册不提供跨版本数据库迁移或任意备份的一键恢复命令。

## 调整资源预算

默认使用自动内存策略。主机扩容或增加同机服务后，先查看 `usdb-node resources`。确需重新计算时，在停机窗口执行：

```bash
usdb-node down
usdb-node set-resource-policy --mode auto
usdb-node resources
usdb-node doctor
```

核对新的预算后执行 `usdb-node up`。只更新工具包不会自动重算已有预算；同机还有其他服务时，需要为它们预留实际额度，不能将节点预算之外的内存视作无限可用。

更多故障处理见[故障排查](../troubleshooting/README.md)。
