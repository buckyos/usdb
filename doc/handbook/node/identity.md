# 管理 P2P 节点身份

[返回手册首页](../README.md) · [安装节点](install.md) · [升级节点](upgrade.md)

`nodekey` 是决定 enode 公钥的私钥，独立于矿工地址、钱包和网络版本。更换它会让其他节点保存的旧 Seed 公钥失效。
包含身份保护功能的版本将它视为持久数据：`setup`、`up`、重新初始化和普通升级都不会创建或替换它。
现有有效身份可直接继续使用；缺失或损坏会阻断启动，要求操作者明确处理。

## 首次创建或沿用旧身份

先完成 `setup`，然后只读检查：

```bash
usdb-node identity status
usdb-node identity status --json
```

输出文件位置、公钥和文件 SHA-256，不显示私钥，也不启动服务。活动文件位于配置中的
`USDB_CHAIN_DATA_HOST_DIR/geth/nodekey`。容器所属文件通过缓存的 chain 镜像只读检查；无法读取时不会猜测为“缺失”。
升级后若当前账号不能直接读取文件，且目标 chain 镜像尚未缓存，`identity status` 显示 `PENDING`，
表示身份尚未核验，不代表文件丢失。`doctor` 将其作为待完成检查，继续其他预检，不因缺少检查镜像报 `FAIL`。
正常继续执行 `usdb-node up`：工具会下载镜像，再严格核验身份，通过后才启动服务。
若只想完成检查、不启动服务，可按提示执行 `docker pull <提示中的完整镜像引用>`，然后重试 `identity status` 和 `doctor`。
使用 `up --skip-pull` 时不会下载镜像，检查仍无法完成则停止；应先准备镜像或取消该选项。

镜像已缓存时会立即检查。实际 nodekey 缺失、损坏、仍无法读取，或 Docker 不可访问，均继续阻断；
不会自动生成、替换 nodekey 或修改目录权限。`identity status` 只有确认身份有效才返回成功退出码，`PENDING` 返回非零。

全新节点明确选择新身份：

```bash
usdb-node identity create
usdb-node identity create --execute
```

第一个命令只预览。第二个要求交互终端并输入 `CREATE IDENTITY <网络名>`，确认后以 `0600` 权限创建。
随后运行 `doctor` 和 `up`。`configure` 的非交互配置流程也不代为生成身份。

跨网络切换或从卸载备份恢复时，如果希望沿用原公钥，应选择恢复而非创建：

```bash
usdb-node identity restore --from-key /absolute/private-backup/geth/nodekey
usdb-node identity restore --from-key /absolute/private-backup/geth/nodekey --execute
```

核对显示的公钥和指纹，再输入 `RESTORE IDENTITY <完整文件SHA-256>`。恢复保持文件内容不变，源文件保留。
旧网络尚在运行时先用旧工具执行 `down`；**同一私钥不能同时用于两个活动节点**。工具会检查本账号可发现的旧网络 controller
及挂载相关路径的活动容器；其他主机上的副本需由操作者确认已停用。跨网络 `setup` 本身不会迁移身份。

## 明确更换身份

已有文件时，`create` 和 `restore` 都拒绝覆盖，包括无效私钥。确实要更换时先停机并归档：

```bash
usdb-node down
usdb-node identity archive
usdb-node identity archive --execute
```

输入 `ARCHIVE IDENTITY <完整文件SHA-256>` 后，工具先把原文件保存到
`~/.local/state/usdb/identities/<网络名>/<文件SHA-256>/nodekey`，核验一致后才移除活动文件。
然后单独执行创建或恢复步骤。归档可保留损坏文件用于排查；不能将损坏备份恢复为可用身份。
容器所属目录可能需要 sudo，取消或失败后先执行 `identity status` 检查结果，不重复清空数据。

## 升级、卸载和清理

同网络升级/重建保留原 `nodekey`。保留身份的空链目录可以重新初始化，创世块或 Chain ID 改变不授权身份轮换。
普通 `uninstall` 保留所有数据。`uninstall --purge-data`、旧版重建工具和 `upgrade-cleanup` 若涉及 nodekey，
除原操作确认外，还必须对每个相关文件输入 `REMOVE IDENTITY <完整文件SHA-256>`；取消身份确认会在删除前停止。
卸载清空仍会先校验私有备份，但重新安装不会自动恢复备份或生成替代身份。

nodekey 及上述备份均为敏感文件，不要贴到日志、工单或公开仓库。删除备份也可能永久失去原节点身份。
旧版本工具不具备这些保护；升级后的操作应使用新版入口及其生成的恢复脚本。
