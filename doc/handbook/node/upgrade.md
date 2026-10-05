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

## 4. 查看旧数据与显式清理

升级默认保留旧数据，不按时间或 release 数量自动删除。安装包含 `upgrade-status` / `upgrade-cleanup`
的工具后，可以管理已有 r7 升级记录；不需要重新升级或重新创建备份目录。
查看和清理均需保留记录对应的新旧安装包，工具会据此核对数据身份和路径。

以原来指定的 `/data/usdb/r5` 为例：

```bash
usdb-node upgrade-status --backup-dir /data/usdb/r5
usdb-node upgrade-status --backup-dir /data/usdb/r5 --json
usdb-node upgrade-cleanup --backup-dir /data/usdb/r5
```

这些命令只读，显示每个旧目录的真实位置、文件逻辑大小和磁盘分配量、引用情况，以及备份目录的占用。
读取容器所属目录或其他账号的标准节点配置时可能需要 sudo；提权后的预览仍不改动数据。
目录分配量不等于最终能释放的空间：私密/未知文件要先复制保存，文件系统快照也可能继续占用物理空间。

`--backup-dir` 是恢复记录和私密资料的存放位置，不是所有旧数据库的集中存放位置：

| 位置 | 内容与用途 |
| --- | --- |
| `backup-dir/upgrade.json` | 新旧版本身份、数据路径、升级阶段和隔离记录 |
| `backup-dir/private/node.env`、`private/config`、`private/secure` | 原配置、运维状态、RPC 凭据等校验备份，含敏感信息 |
| 数据根中的旧 indexer 身份目录 | 原索引数据库；新版本使用另一个身份目录 |
| `*.before-upgrade-<操作ID>` | 原 chain、control-plane、监控及旧链操作状态；在原文件系统改名保留 |
| 清理后的 `backup-dir/cleanup.json`、`private/retained/` | 清理进度和追加的私密文件备份；日志记录原路径与备份编号的对应关系 |

确认新节点同步、业务及挖矿配置符合预期，且不再需要旧数据回退后，安排一次停机并显式清理：

```bash
usdb-node down
usdb-node upgrade-cleanup --backup-dir /data/usdb/r5 --execute
usdb-node up
usdb-node status --watch
```

执行要求交互终端、sudo 和包含网络名、升级操作 ID、主机名的确认短语。必须停止整个节点，不能只停 USDB chain
或使用 `down --keep-bitcoin`。清理不会改变自启开关，也不会删除 Bitcoin Core、balance-history、Ord、快照材料、
当前数据目录或备份目录本身。

删除前会先复制并 SHA-256 校验旧 chain 中的 keystore、nodekey，以及已知数据库位置以外的文件。
旧 control-plane、监控和操作记录整体保存在 `private/retained/`；未知文件默认保留。
标准 indexer 数据库及 Geth 的派生链数据库允许删除；不要把私密文件放进这些数据库内部。
自定义 indexer 数据布局不会被猜测为可重建数据，可能需要复制较多内容，应预留备份空间。

**确认执行、进入 `cleanup_started` 后，本次升级永久放弃自动回退**，即使随后备份失败、尚未删除数据库也如此。
旧 r7 恢复程序也会拒绝这个阶段。清理中断时保留全部记录，排除报错后重复同一条 `upgrade-cleanup ... --execute`，
不能改用 `upgrade-release --rollback`。已删除目录若被重新创建，工具会拒绝再次删除。
`cleaned` 表示本次旧数据已清理；恢复记录和私密备份仍需妥善保管，它们不再构成完整旧节点恢复材料。

引用检查覆盖本机各账号标准路径 `~/.config/usdb/*/node.env`、全部 Docker 挂载（包括已停止容器）、
已登记的升级记录，以及指定备份目录同级的其他升级记录；执行时还检查数据库锁和本机进程的打开文件、映射及工作目录。
发现引用或无法完成检查时会阻断，不会把“没有检查到”当成“可以删除”。

新版升级会把记录位置登记在数据根的 `.usdb-upgrades/`。r7 没有这个登记，因此位于其他目录的历史记录需显式补充，
预览和执行都要带上相同参数，可重复使用：

```bash
usdb-node upgrade-cleanup --backup-dir /data/usdb/r5 \
  --other-backup-dir /another/location/older-upgrade
```

其他未登记、已移动的历史记录，以及自定义配置路径、离线消费者仍需要管理员核对；工具不会遍历所有磁盘来证明不存在任何引用。
不要删除登记文件来绕过冲突。多次升级通常从较早的已验收记录开始清理；记录缺失或共享使用关系不明确时，先恢复记录并厘清引用。
当前命令只处理成功应用的升级；未完成、已回退操作留下的数据继续保留，需单独复核。
