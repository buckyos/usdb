# 升级预检与保留数据重建

[返回手册首页](../README.md) · [日常维护](maintenance.md#升级节点)

适用范围：目标 node kit 包含 `upgrade-plan` / `upgrade-release` 命令，且已有节点使用 v2 数据布局。
它们用于已经配置的节点，不替代首次 `setup`。更新 release 不意味着每次都要卸载；实际动作取决于新旧网络身份和各服务的数据契约。
旧工具不认识这些命令时，先安装包含该功能的目标版本。始终使用原运维账号，保留原数据根和旧安装包。

## 1. 查看升级计划

从网络运维方取得目标版本与升级通知。需要停机时先用原版本 `usdb-node down` 停止整个节点，
再按[安装说明](install.md#2-下载并安装节点工具)安装目标工具，暂不执行 `setup` 或启动服务。
下面的预检本身允许在节点运行时执行，只读取本地 manifest、配置和数据身份标记，不创建目录、停止服务或改写数据：

```bash
usdb-node upgrade-plan
usdb-node upgrade-plan --details
usdb-node upgrade-plan --json
```

工具根据配置中的三个镜像 digest 和 runtime compatibility ID 寻找准确的旧安装包。
默认搜索目标包的同级目录及原账号的 `~/.local/share/usdb/releases/`。无法识别或存在歧义时，指定仍保留的旧包：

```bash
usdb-node upgrade-plan --from-kit /absolute/path/to/old-kit
```

默认输出先显示兼容结论、各组件的 `REUSE` / `REBUILD` 表格及主要变化，再列出本次计划对应的下一步命令。
相同目录不重复打印，重复的 ID 差异不会占据摘要。`--details` 展示完整路径、runtime ID 和字段变化；
`--json` 保持完整计划结构，便于自动化采集。`upgrade-release` 的预览同样支持这两个选项。
输出不包含 RPC 密码或私钥；预检和建议命令都不会自动执行升级。

兼容计划提示 `activate-release`；可执行的重建计划提示 `down` 和带 `--backup-dir`、`--execute` 的 `upgrade-release`，
占位备份路径必须替换为实际的新私有目录。存在阻断项时只提示先解决问题，不给出执行命令。
恢复预览会显示保存的操作状态，未完成时提示对应的 `--resume`，已经完成或回退时给出相应后续步骤。
不要用发布号大小、同一个 Chain ID，或相同 genesis block hash 来代替这项检查：genesis 配置及 BTC 规则历史也可能改变。

| 结果 | 下一步 |
| --- | --- |
| `compatible` 且 `executable=true` | 按[日常维护](maintenance.md#升级节点)执行 `activate-release → doctor → up`；数据保留 |
| `data_rebuild` 且 `executable=true` | 按下节显式重建不兼容的派生数据 |
| `network_reset` 且 `executable=true` | 仅在网络运维方已协调重置开发网络后执行下节；USDB 链从头开始 |
| `executable=false` 或校验错误 | 保留现场，先处理阻断原因；不能手改 ID 或 marker 绕过校验 |

`activate-release` 同样核对旧包与目标包的完整共识身份；可使用 `activate-release --from-kit /absolute/path/to/old-kit`。
缺少准确旧包时先恢复该包，不猜测兼容性。

当前自动执行范围限于同一 bundle、Bitcoin Core/BH 契约不变、启动模式不变的重建。
跨 bundle、旧全量同步切到 AssumeUTXO、paired checkpoint、同时需要迁移 Ord 的组合升级，以及正式网的规则迁移，需独立方案。
链重置还要求目标网络明确标记 `development-resettable`。本机工具不能决定其他节点何时升级，也不能替代网络重置公告。

## 2. 显式重建不兼容部分

确认 SourceDAO、Mining、Peer 和资源切换任务均已完成，安排备份和停机窗口。执行时必须停止整个节点，不能使用 `down --keep-bitcoin`。
工具会核对 systemd、容器、共享数据挂载及数据库锁，拒绝在仍有使用者时移动数据。

```bash
usdb-node down
usdb-node upgrade-release --from-kit /absolute/path/to/old-kit
usdb-node upgrade-release --from-kit /absolute/path/to/old-kit \
  --backup-dir /absolute/path/to/new-private-upgrade-backup --execute
```

前一个 `upgrade-release` 只预览。执行命令需要交互终端、sudo 和包含网络名/主机名的确认短语。
`--backup-dir` 必须是全新目录，不能与节点数据根、配置目录或新旧安装包重叠。工具先保存私有配置和凭据，
再执行数据切换；旧数据库在原路径保留，或在同一目录改名为 `*.before-upgrade-<操作ID>`，不会删除。
备份目录包含敏感配置，必须保持私有，不上传工单。

以此次 MinerPass 规则历史改变的开发网重建为例，预检确认契约相同后：

| 内容 | 处理 |
| --- | --- |
| Bitcoin Core、balance-history | 原目录复用，保留同步和索引进度 |
| Ord、UTXO 下载文件、启动材料 | 保留；若需要额外迁移则阻断组合执行 |
| USDB indexer、USDB chain、control-plane | 隔离旧数据，准备新数据集，不让新规则读取旧结果 |
| RPC 凭据、资源/端口配置、Geth nodekey 和 keystore | 保留；钱包仍须独立备份 |
| 矿工角色、旧 Mining/Peer 操作记录、SourceDAO 本地状态 | 链重置时停用矿工并隔离旧记录；旧 SourceDAO 签名材料保留在隔离目录，由管理员按新链流程处理 |
| 监控配置与通知配置 | 保留；旧链事件和待发送通知隔离，避免混入新链 |

这不是数据库格式迁移，也不是独立灾备副本：大数据库只在原盘保留，不复制到 `--backup-dir`。
旧数据仍占磁盘空间，新索引和新链会继续增加占用，执行前应预留容量。工具不自动清理这些归档。
更改本地数据库不会移动或销毁 BTC；USDB 链重置意味着旧链余额、合约状态不会在新链自动延续。

执行成功后服务保持停止，旧 systemd 自启暂时关闭，避免重启主机时由旧安装包拉起新数据。
使用目标工具刷新 controller（重新安装时沿用原先的超时/拉取选项；有意不启用后台服务的节点使用前台流程），然后检查与启动：

```bash
usdb-node controller install
usdb-node doctor
usdb-node up
usdb-node status --watch
usdb-node peers status
```

`up` 会按当前工具检查和刷新其余后台服务。确认新链完成必要的 SourceDAO 初始化、MinerPass 有效、节点同步和资格检查通过后，
再按[矿工指引](mining.md)重新授权挖矿。该工具不会自动恢复旧链的矿工资格。

## 3. 中断恢复与回退

保留新旧安装包、升级备份目录、数据旁的隔离目录，以及 `.usdb-upgrade-pending.json`。
出现 `UPGRADE_PENDING` 时，新工具会阻止启动或改变节点配置；不要删除标记绕过它。
用**执行该次升级的同一个目标包**恢复，不能换成另一个 release 继续操作：

```bash
usdb-node upgrade-release --resume /absolute/path/to/upgrade-backup
usdb-node upgrade-release --resume /absolute/path/to/upgrade-backup --execute
```

第一条只显示保存的计划。恢复执行会重新核对 manifest、配置、文件身份和停止状态；已完成步骤不会重复覆盖数据。
磁盘不足、权限或外部文件变更导致中断时，先处理具体错误，再恢复同一操作。不要对它重新执行 `setup` 或卸载。

在新服务尚未启动、数据和配置未被外部修改的情况下，可以恢复原来的停止状态：

```bash
usdb-node upgrade-release --resume /absolute/path/to/upgrade-backup --rollback --execute
```

回退恢复旧配置、旧目录和原来的自启开关；新建数据也会隔离保留。随后必须使用原安装包，不能让目标包打开旧数据。
一旦新服务已运行并写入数据，自动回退会拒绝丢弃这些新数据；需另行安排恢复。启动后不要把切回旧工具入口当作回滚。
