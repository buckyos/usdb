# 安装普通节点并加入测试网

[返回手册首页](../README.md) · 上一步：[环境与主机准备](requirements.md)

适用范围：全新 Linux 主机上的 AssumeUTXO 发布包、`full` 节点、加入已有测试网。已有配置的节点按[日常维护](maintenance.md)操作，不重复执行 `setup`。

**先检查[版本页](../networks/testnet.md#版本与验证范围)，使用网络运维方指定版本。** r28 已确认安装和首次配置通过；同步和入网仍按本页逐项确认。r28 的 IPv6 配置方式与包含新向导提示的版本有所不同，见第 4 步。

## 1. 确认安装输入

准备好满足环境要求的机器、普通运维账号、可写的数据盘，以及同一测试网的 Seed 地址。整个过程保持使用这个账号。

从[测试网资料](../networks/testnet.md)取得指定安装版本及其 Release 页面。该版本应明确支持本页的启动方式，并已解决首次配置问题。

## 2. 下载并安装节点工具

打开第 1 步选定的 Release 页面，找到 **Release-bound installer**，复制其中的完整命令，在节点的普通运维账号下执行即可。

例如，[r28 Release 页面](https://github.com/buckyos/usdb/releases/tag/usdb-testnet-v0-r28)提供的命令为：

```bash
bash <(curl -fsSL https://github.com/buckyos/usdb/releases/download/usdb-testnet-v0-r28/install-usdb-testnet-v0-r28.sh)
```

每个 Release 的命令都已绑定对应版本，安装器自动完成下载和校验。安装其他版本时，直接复制该版本页面中的命令。下载失败或校验不通过时，按[下载故障](../troubleshooting/README.md#下载失败或校验不通过)处理。

安装器提示安装成功后，在当前终端执行：

```bash
export PATH="${HOME}/.local/bin:${PATH}"
usdb-node --help
```

**完成标志**：能看到 `usdb-node` 命令帮助。此时只安装了节点工具，还没有生成节点配置或启动同步。若新登录终端找不到命令，见[命令找不到](../troubleshooting/README.md#找不到-usdb-node-命令)。

## 3. 准备主机软件

```bash
usdb-node prepare-host
```

工具先检查依赖和 Docker 权限；有缺失时才询问是否安装。需要主机权限时按提示使用 sudo。安装有冲突或失败时，先处理报错，再继续。

如果提示已添加 Docker 组、当前终端权限尚未生效，退出 SSH，以同一账号重新登录，重新设置 PATH 后检查：

```bash
export PATH="${HOME}/.local/bin:${PATH}"
usdb-node host check
```

**完成标志**：主机检查通过，当前账号可以访问 Docker。新会话仍提示权限问题时，见[Docker 权限](../troubleshooting/README.md#docker-权限或主机检查失败)。

需要通过 IPv6 入网时，还要核对[双栈准备检查](requirements.md#双栈准备检查)。`P2P_IPV4_FALLBACK` 表示自动配置会使用 IPv4；基础依赖通过后仍需先处理这项网络问题。

## 4. 配置节点

```bash
usdb-node setup
```

按向导逐项确认：

| 向导项目 | 普通加入节点的选择 |
| --- | --- |
| `Host data root` | 填写已准备的数据目录，例如 `/data/usdb`；确认显示的是正确磁盘 |
| `Node role` | `full` |
| `Seed enode(s)` | 填写同网 Seed 完整地址；多条以逗号分隔。暂缺时可留空，但之后必须补充才能入网 |
| `USDB P2P address family`（包含向导改进的版本） | 默认 `auto`；明确需要 IPv6 时选 `dual`（双栈）或 `ipv6`。所选模式不满足主机条件时，按提示修复 |
| `Provide full Explorer support` | 普通节点选 `n`；专用查询节点应在首次同步前另行规划 |
| `Accept inbound Bitcoin peers` | 默认 `n`，不影响 Bitcoin 出站同步 |
| `Manage this host firewall ... UFW` | 已有云或主机规则选 `n`；需要工具管理 UFW 时选 `y`，并确认实际 SSH 服务端口 |
| 原生启动提示 | 显示 `Native AssumeUTXO bootstrap`，无需选择旧 BH 快照或填写下载地址 |
| `Write this node configuration` | 核对目录、角色及网络暴露后确认写入 |

工具会生成私有凭据、保存配置，并安装后台启动管理服务。sudo 提示要求当前运维账号的密码。首次部署采用默认后台方式，不使用 `--no-controller`。

**r28 及此前的向导没有地址族交互项，摘要中的 `USDB P2P: public TCP/UDP 31303` 只说明端口。** 首次部署且需要双栈时，使用以下命令代替上面的普通 `setup`；主机检查应先通过：

```bash
read -r -p 'This node stable IPv6 address: ' USDB_NODE_IPV6
usdb-node setup --p2p-ip-family dual --advertise-ipv6 "$USDB_NODE_IPV6"
```

包含向导改进的版本会在确认写入前列出 `requested`、最终 `family`、公告地址及自动回退原因。看到 `P2P_IPV6_SEED_UNREACHABLE` 时，说明当前 IPv4 链容器无法连接填写的 IPv6 Seed；取消写入并修复，或取得可达的 IPv4 Seed。`auto` 只选择并保存一次，主机 IPv6 后来恢复也不会自动切换。

已经完成 `setup` 的节点按[已部署节点修改地址族](peers.md#已部署节点修改地址族)处理，不重新执行配置向导。

**完成标志**：向导正常结束，显示后续检查和启动命令。出现 `node is already configured` 时，不删除配置重新开始；按[已有配置](../troubleshooting/README.md#提示已有配置)处理。

## 5. 检查并启动

先执行：

```bash
usdb-node doctor
```

这是启动前检查。没有下载镜像或原生启动文件在此阶段可以是正常情况，后续 `up` 会准备。存在失败项时先处理；使用 external 防火墙的节点还需自行确认端口规则。

检查通过后执行：

```bash
usdb-node up
```

后台任务会拉取镜像、准备启动数据并按条件启动各服务。终端显示进度，首次同步可能持续很久。**Ctrl+C 只退出观察，SSH 中断也不会取消后台任务。** 不需要一直保持当前连接。

重新连接后观察：

```bash
usdb-node status --watch
```

遇到等待或失败时，先按[状态与同步进度](status.md)判断当前阶段，不连续重启节点。

## 6. 补充 Seed 并确认入网

如果向导已填 Seed，先查看连接状态：

```bash
usdb-node peers status --watch
```

如果还没有 Seed，向网络运维方取得地址，然后退出观察并执行：

```bash
read -r -p 'Seed enode: ' USDB_SEED_ENODE
usdb-node peers add "$USDB_SEED_ENODE"
usdb-node peers status --watch
```

通过 IPv6 入网时，先按[节点地址与连接管理](peers.md)核对双方链容器的 IPv6 能力。首节点使用 `usdb-node peers enode --family ipv6` 导出可分享地址；不要直接复制带 `127.0.0.1` 的原始 enode。

可以在上游同步期间补充 Seed。运行中的链应用新配置时会有短暂 RPC/P2P 中断；节点同步数据保留。命令返回任务已提交或配置 `APPLIED`，还需要继续观察实际连接。

`AWAITING_PEERS` 表示数据服务已经运行但尚未确认入网；处理连接问题即可，不需要重下启动文件。普通加入节点不使用 `--first-node`。

## 7. 确认部署结果

退出观察后分别执行：

```bash
usdb-node status
usdb-node peers status
```

基本完成条件：

- `status` 的 `Overall` 为 `READY`，没有未处理的核心服务阻断。
- `peers status` 显示实际连接，加入状态为 `READY / CONNECTED`，当前没有报告链同步。
- 安装的网络与指定测试网一致；链高度继续跟随网络。若需正式接入验收，再与网络运维方核对共同高度的区块哈希。

`CONNECTED` 是基本连接观测，不单独证明已经追到全网最新高度。Bitcoin 前台就绪后，后台历史验证仍可能继续；保持节点运行并在进度面板观察它。

此时节点角色仍为 `full`，不会自动挖矿，也不需要重复初始化已有网络的 SourceDAO。需要参与挖矿时继续阅读[CPU 挖矿](mining.md)；网络初始化负责人在链开始出块后执行[SourceDAO 初始化与验证](../network-admin/sourcedao.md)。日常操作见[日常维护](maintenance.md)。
