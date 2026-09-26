# 卸载节点与清空后重新同步

[返回手册首页](../README.md) · [日常维护](maintenance.md)

`usdb-node uninstall` 是面向运维人员的固定入口，在 `usdb-node --help` 中可见。
本文命令需要包含此功能的新版 node kit；旧版没有该入口时，先取得新版工具包。
普通升级、RPC 暂时超时、补充 Seed 都不需要清空节点。

## 选择需要的操作

| 目的 | 操作 | 保留内容 |
| --- | --- | --- |
| 暂停，稍后继续 | `usdb-node down` | 程序、配置及全部数据 |
| 卸载程序，保留同步成果 | `usdb-node uninstall --execute` | 配置、钱包、节点身份、监控历史及全部链/索引数据 |
| 清空当前节点，准备重新安装同步 | `usdb-node uninstall --purge-data --execute` | 先备份配置、钱包、节点身份及监控历史；区块、BH/indexer/chain 数据库和下载快照不备份 |

**不加 `--execute` 时只预览路径，不停止服务，也不删除文件。** 执行时还需要交互确认，
没有 `--yes` 或通过管道跳过确认的入口。只运行 `controller stop` 不能代替停止整个节点。

## 先预览，再停机执行

以原安装账号运行；`--backup-dir` 使用一个尚不存在的绝对路径，位于当前节点数据、配置和 release
目录之外。可选择其他数据盘，下面的 `$HOME/usdb-uninstall-backup` 只是示例。

```bash
# 只卸载程序的预览：
usdb-node uninstall

# 清空节点的预览：
usdb-node uninstall --purge-data --backup-dir "$HOME/usdb-uninstall-backup"
```

检查输出中的主机名、网络、删除路径、保留路径及备份目录。工具只处理选定网络和配置所引用的
已知数据目录，不会递归删除整个数据盘。路径、数据布局或安装方式无法确认时，会拒绝执行。

确认范围后，停止服务并关闭其开机启动：

```bash
usdb-node down
usdb-node controller disable
```

SourceDAO 任务活动时先等待或按 [SourceDAO 运维](../network-admin/sourcedao.md)处理。
前台 `monitor run` 也需要正常退出。旧版遗留的 console monitor、活动容器或引用这些目录的其他容器
会阻止卸载；按错误指出的具体服务检查。若旧 console monitor 仍启用，可核对后使用
`sudo systemctl disable --now usdb-console-monitor-<bundle-id>.service`，将占位符换成预览里的网络 ID。

随后只选择下面一个命令执行：

```bash
# 卸载程序，保留数据和配置：
usdb-node uninstall --backup-dir "$HOME/usdb-uninstall-backup" --execute

# 或：清空当前节点，并保留私有状态备份：
usdb-node uninstall --purge-data --backup-dir "$HOME/usdb-uninstall-backup" --execute
```

命令可能要求 sudo 密码，以处理容器所属文件和 systemd 定义。真正执行前再次展示清单，要求输入包含
网络和主机名的完整确认短语；输入不匹配即取消。备份和删除过程中有进度输出。

软件卸载范围包括当前网络的 release 工具包、指向该网络的 `usdb-node` 启动链接、controller、monitor
及旧 console monitor 服务定义，以及该节点已停止的容器。清空模式还删除预览中列出的数据和配置目录，
包括配置引用的 Ord 数据集，即使当前没有启用 Ord。

## 私有备份与中断恢复

执行前会把独立恢复脚本和操作记录放入备份目录。清空模式在删除任何目录前复制并以 SHA-256 校验：

- 当前网络的配置目录，包括监控事件、资源历史、通知配置及 SourceDAO 操作记录。
- 节点身份和钱包私有状态，包括 chain nodekey/keystore、Bitcoin 钱包和自定义钱包目录。
- secure、control-plane 数据及服务定义；Bitcoin/chain/Ord 数据目录中不属于已知可重建大数据库的文件也会保留。

`private/` 下是真正的私有备份；`uninstall.json` 记录进度、文件校验信息及原文件 UID/GID；`runner/` 是独立执行入口。
备份目录权限为 `0700`，部分文件可能由 root 持有，读取或恢复时需要 sudo。
不要公开上传此目录，其中可能包含钱包、签名凭据和通知密钥。它不是完整链数据备份；如需保留 BH 数据作
比对，或希望以后直接恢复同步成果，请事先另做离线数据库备份，或选择默认的保留数据模式。

SSH 中断、空间不足或权限错误后，保留同一个备份目录，用命令输出中打印的恢复命令继续，例如：

```bash
# 只查看已保存的卸载计划：
sudo python3 "$HOME/usdb-uninstall-backup/runner/node_uninstall.py" \
  --resume "$HOME/usdb-uninstall-backup" --plan

# 继续执行，仍会要求确认：
sudo python3 "$HOME/usdb-uninstall-backup/runner/node_uninstall.py" \
  --resume "$HOME/usdb-uninstall-backup"
```

恢复不依赖原来的 `usdb-node` 或 release 目录。发现配置改变、目标被替换、新钱包出现或备份校验失败时，
会停止，避免删除新数据。不要在未完成卸载时先重新安装、重新 setup 或启动节点；先处理原操作记录。

备份出现空间不足时，应给所选文件系统留出空间后继续；工具另要求至少约 1 GiB 的备份安全余量。
自定义符号链接、挂载点/嵌套挂载、共享目录、service drop-in 等情况会要求人工检查，不自动扩大删除范围。

## 重新安装与剩余内容

默认卸载保留配置和数据。重新安装同一网络的 release 后，按 [升级流程](maintenance.md#升级节点)执行
`activate-release → doctor → up`，继续使用已有数据。

清空模式完成后，按 [首次安装流程](install.md)安装、`setup → doctor → up`，重新下载/同步。
需要沿用原 enode、恢复钱包或 SourceDAO 身份时，在启动相应服务前从私有备份恢复对应文件，并核对运行账号、
路径及权限；不要直接把整个旧配置目录覆盖到新的 setup 结果上。

工具保留 Docker 镜像/命名卷、Docker 本身及主机依赖、防火墙规则、其他网络、未引用的历史数据集和已有备份。
父目录可能作为空目录留下。它不会执行全局 Docker prune，也不会自动关闭 SSH 端口或改动其他容器。
需要回收这些剩余内容时，先单独盘点其用途；“卸载完成”不表示整台主机恢复成空白系统。

目前固定入口支持标准账号安装路径、已配置的 v2 数据布局和可检查的 Linux/systemd/Docker 环境。
`--node-env` 指向自定义位置、未知数据布局或服务状态无法读取时，需要按实际部署另行检查。
旧 `node_rebuild.py` 仍保留给历史测试归档流程使用；它默认只归档 BH，并不提供这里的私有状态保留策略，
日常卸载优先使用本页的新命令。
