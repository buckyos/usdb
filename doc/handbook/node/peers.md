# 节点地址、IPv6 与连接管理

[返回手册首页](../README.md) · [安装与入网](install.md) · [状态与同步进度](status.md)

本章介绍如何取得本机的 enode、让另一台节点通过 IPv6 接入，以及确认真实连接。适用于提供 `usdb-node peers` 命令并配套 IPv6 链镜像的发布包。**r20 不提供这些命令和配套的双栈部署能力**，先阅读本页的[旧版本与升级边界](#旧版本与升级边界)。

## 先分清本机地址与种子地址

**本机 enode** 是给其他节点连接自己的地址；**Seed / bootnode** 是自己用来发现并连接已有网络的节点地址。

首节点即使尚无 Seed、`Connected: 0`，也可以公布自己的 enode。已确认的首节点显示 `READY / FIRST_NODE` 时，不需要为了取得本机地址添加一个虚假的 Seed。普通加入节点则需要有效的种子和真实连接。

| 目的 | 命令 |
| --- | --- |
| 查看本机 IPv4/IPv6 enode 候选地址 | `usdb-node peers enode` |
| 只取得本机 IPv6 地址 | `usdb-node peers enode --family ipv6` |
| 查看地址族、宿主机路由和实际容器端口 | `usdb-node peers network --json` |
| 查看实际连接、链高度和入网状态 | `usdb-node peers status` 或 `usdb-node peers status --watch` |
| 查看配置的 Seed 与尚未应用的 Seed | `usdb-node peers list` |

包含本机 enode 展示改进的版本，还会在 `peers status` / `--watch` 中显示 `Local P2P` 和各地址族的 enode，JSON 中对应 `local` 字段。旧版如 r27 需要额外执行 `peers enode`；并不是首节点没有自己的地址。

## 为什么原始 enode 会显示 127.0.0.1

Geth 的原始 `admin_nodeInfo.enode` 可能包含 `127.0.0.1`，也可能显示容器地址或其中一个地址族。它表示 Geth 当前生成的公告记录，**不能据此判断宿主机只监听回环地址，或双栈配置没有生效**。

节点工具使用同一节点的公钥，加上配置的对外 IPv4/IPv6 地址及端口，生成供其他机器使用的 enode。查询入口是：

```bash
usdb-node peers enode --family ipv6
usdb-node peers network --json
```

IPv6 地址的形式如下；这里的公钥和地址只是格式占位，不能直接用于入网：

```text
enode://<节点公钥>@[<节点IPv6地址>]:31303
```

IPv6 地址必须保留方括号，复制命令参数时给整个 enode 加引号。同一节点的 IPv4 与 IPv6 enode 使用相同公钥，但地址不同。`?discport=...` 表示 UDP 发现端口与 TCP 端口不同，有这个后缀时完整保留。

对外分享工具生成的地址，不直接复制带 `127.0.0.1` 或 Docker 内部地址的原始 enode。工具显示 `family=dual` 也不保证能列出两个地址：例如只有可确认的 IPv6 对外地址时，会只列出 IPv6 候选。

## 查看首节点当前可分享的地址

在首节点原运维账号下执行：

```bash
usdb-node peers enode --family ipv6
```

输出 `P2P | CONFIGURED` 且列出 `ipv6: enode://...`，表示本机配置和所需容器网络已经通过检查。复制完整的 `enode://...` 部分给加入节点。

`public reachability unverified` 表示尚未通过另一台机器验证公网路径。它不是配置失败；需要完成下方的真实连接检查。若已经显示正确地址和 `CONFIGURED`，无需因为原始 enode 中出现回环地址而重新配置。

若输出 `WAITING`、`BLOCKED` 或没有地址，先查看 `peers network --json` 中的错误。`enode --family ipv6` 不会在缺少 IPv6 时悄悄返回 IPv4。

## 配置 IPv6 或双栈

### 准备条件

- 使用支持 IPv6 的配套节点工具与链镜像；完成[主机检查](requirements.md)，Docker Engine 至少 `28.0.0`，Compose 至少 `2.33.1`。
- 宿主机具有已分配、稳定的 IPv6 地址和默认路由。不要使用 `fe80::` 链路本地地址、临时地址或 Docker 内部地址作为公网入口。
- 需要被其他节点连接时，主机防火墙、云安全组或路由器放行实际 P2P 端口，默认 **TCP 与 UDP 31303**。IPv6 不使用 IPv4 的端口转发规则。

先在宿主机查看：

```bash
ip -6 address show scope global
ip -6 route show default
usdb-node host check
```

宿主机能使用 IPv6，不等于链容器也能使用 IPv6。配置完成后还必须查看 `peers network --json`：链容器应有 IPv6 地址、IPv6 网关及所选地址族的 TCP/UDP 发布记录。

如果工具报告 `P2P_IPV6_RA_REQUIRED`，按下方[IPv6 诊断与恢复](#ipv6-诊断与恢复)检查实际接口并持久设置，再恢复和核对地址与路由。

### 首次配置选择地址族

`setup` 默认使用 `auto`：找到稳定 IPv6 地址和默认路由，并通过 RA 和 Docker 检查后选择 `dual`；检查不满足时可以回退为 `ipv4`。明确选择 `dual` 或 `ipv6` 时，失败会报错，不自动改成 IPv4。

包含准备提示和向导改进的版本增加地址族选择项，并在最终确认前显示实际 `family`、公告地址及回退原因。`prepare-host` / `host check` 的 `P2P host check` 是当时的自动选择预览；已经安装的节点仍以 `peers network --json` 显示的保存配置为准。

**r28 及此前的向导没有这个交互项。** 首次配置可显式传入参数：

```bash
read -r -p 'This node stable IPv6 address: ' USDB_NODE_IPV6
usdb-node setup --p2p-ip-family dual --advertise-ipv6 "$USDB_NODE_IPV6"
```

输入本机地址，不带方括号。有多块网卡或多个稳定地址时，用参数明确指定要公告的地址。需要公告 NAT 后的 IPv4 地址时，同时传入 `--advertise-ipv4`。`Explorer support`、Bitcoin 入站公开与 USDB 地址族是分别配置的项目。

### 已部署节点修改地址族

在需要配置的那台节点上输入**它自己的**稳定 IPv6 地址，不带方括号：

```bash
read -r -p 'This node IPv6 address: ' USDB_NODE_IPV6
usdb-node peers configure --ip-family dual --advertise-ipv6 "$USDB_NODE_IPV6"
usdb-node peers status --watch
```

如果只需要 IPv6 P2P 发布，把 `dual` 换成 `ipv6`。节点在 IPv6 模式下仍会保留与本机数据服务之间的内部网络。

需要同时公布可达的 IPv4 地址时，在同一次 `configure` 中加 `--advertise-ipv4`；经过路由器映射时填写对外 IPv4。对外端口不是默认值时，同时提供 `--advertise-port`，TCP/UDP 端口不同还需提供 `--advertise-discovery-port`。

**`configure` 会设置完整的一组 P2P 参数。** 省略地址会重新选择，省略对外端口会采用默认 `31303`；已有 NAT 地址或自定义端口时，每次修改都保留相应参数。

等待任务 `APPLIED`，再执行：

```bash
usdb-node peers network --json
usdb-node peers enode --family ipv6
```

运行中的节点会只重建链服务，短暂中断 USDB P2P/RPC，保留链数据、节点身份、Seed 和已授权矿工配置，上游同步继续。原本停止的节点只保存配置，之后仍需 `usdb-node up`。这一网络配置操作不需要清空数据，也无需额外执行整套 `down → up`。

`auto` 只在执行配置时选择并保存一次地址族；后来增加 IPv6 地址，不会自动把已保存的 IPv4 配置切成双栈。明确要求 IPv6 时选择 `ipv6` 或 `dual`，并检查实际结果。

## IPv6 诊断与恢复

### 1. 分别检查宿主机与保存配置

```bash
ip -6 address show scope global
ip -6 route show default
usdb-node host check
```

已有配置时另执行：

```bash
usdb-node peers network --json
```

| 检查结果 | 操作 |
| --- | --- |
| 没有可用稳定 IPv6 地址 | 检查主机、上级路由器或云网络的 IPv6 配置；不要使用临时地址、链路本地地址或容器地址作为公告地址 |
| 有 IPv6 地址，但没有默认路由 | 检查路由器公告或网络管理员提供的静态路由；填一个 IPv6 地址不能替代路由 |
| `P2P_IPV6_RA_REQUIRED` | 按下一步检查错误指出的实际接口 |
| Docker 版本、rootless 或访问权限检查失败 | 按[环境与主机准备](requirements.md)修复，再用原运维账号检查 |
| `host inspection failed` | 根据附带原因处理地址/路由读取失败；探测失败不能视作 IPv6 已就绪 |
| 主机检查可选 `dual`，但 `peers network` 的 `family` 仍为 `ipv4` | 旧配置不会自动改变；修复后显式执行 `peers configure` |
| `P2P_IPV6_SEED_UNREACHABLE` | 当前保存/预览的 IPv4 链网络无法使用 IPv6 Seed；修复并切换地址族，或换用可达的 IPv4 Seed |

### 2. 修复 RA 接收策略

只对使用路由器公告（RA）获取默认路由的接口执行这一步。`ip -6 route show default` 通常显示 `proto ra`，工具错误也会给出接口名。接口可能是 `enp170s0`、`eth0` 或 `vmbr0`，以本机为准。

Linux 的 `accept_ra=1` 仅在未开启转发时接收 RA，`2` 允许在转发开启时继续接收。USDB 双栈检查要求 RA 接口为 `2`，避免 Docker 启用 IPv6 转发后影响路由续期。[Linux 内核说明](https://docs.kernel.org/networking/ip-sysctl.html)。

先输入实际接口并检查当前值；若错误涉及多个上联网卡，应分别处理：

```bash
read -r -p 'IPv6 default-route interface: ' USDB_IPV6_INTERFACE
ip link show dev "$USDB_IPV6_INTERFACE"
sysctl "net.ipv6.conf.${USDB_IPV6_INTERFACE}.accept_ra"
```

临时应用并核对：

```bash
sudo sysctl -w "net.ipv6.conf.${USDB_IPV6_INTERFACE}.accept_ra=2"
ip -6 address show dev "$USDB_IPV6_INTERFACE" scope global
ip -6 route show default
```

临时值重启后可能丢失。由管理员检查已有 `/etc/sysctl.d/` 和网络管理器配置，将以下示例中的 `实际接口` 替换后写入适用配置，避免留下互相覆盖的旧值：

```text
# /etc/sysctl.d/90-usdb-ipv6-ra.conf
net.ipv6.conf.实际接口.accept_ra=2
```

使用上述文件时，应用它并再次核对：

```bash
sudo sysctl -p /etc/sysctl.d/90-usdb-ipv6-ra.conf
sysctl "net.ipv6.conf.${USDB_IPV6_INTERFACE}.accept_ra"
```

如果网络管理器会重设这个值，还需在其实际接口配置中保持一致。已有地址或默认路由已经过期时，仅修改 sysctl 不保证立即恢复；检查上级路由器是否提供 RA。已安装 `ndisc6` 的主机可执行 `sudo rdisc6 -1 "$USDB_IPV6_INTERFACE"` 请求一次 RA，之后重新检查地址和路由。

### 3. 应用到节点并确认恢复

已有节点使用本页的 `peers configure --ip-family dual --advertise-ipv6 ...`；未配置节点使用带相应参数的 `setup`。这一步不会要求重新下载 Bitcoin 或 BH 数据。

等待配置操作 `APPLIED`。节点尚未启动时，`peers network` 可以显示 `family=dual` 但状态仍为 `WAITING`；此时还不能验证容器的 IPv6。按安装流程完成 `doctor` 和 `up`，链服务启动后检查：

```bash
usdb-node peers network --json
usdb-node peers status --json
```

**恢复标志**：本机地址和默认路由正常、所选模式已应用、链容器具有 IPv6 地址及网关、TCP/UDP 发布符合所选模式，并且实际连接的 `remoteAddress` 使用目标 IPv6。只看到 `family=dual` 或宿主机 TCP 测试成功，还不能认定已经完成 IPv6 入网。

## 第二台通过 IPv6 加入

先确保第二台也使用上述受支持的 IPv6/双栈部署，链容器具有 IPv6 出站能力。首节点支持双栈，并不能替另一台的 IPv4 容器增加 IPv6 路由。

在第二台原运维账号下粘贴首节点导出的完整 IPv6 enode：

```bash
read -r -p 'Seed IPv6 enode: ' USDB_SEED_ENODE
usdb-node peers add "$USDB_SEED_ENODE"
usdb-node peers status --watch
```

`controller_submitted` 表示任务已提交，`APPLIED` 表示配置已应用；两者都不证明已经连通。普通加入节点应继续观察实际连接数大于零、USDB 链同步推进，最后达到 `READY / CONNECTED`。

核对 IPv6 是否真正用于连接：

```bash
usdb-node peers status --json
```

查看 `connected[].network.remoteAddress`，应为目标节点的 IPv6 地址和连接端口。包含显示改进的版本会在文本中直接显示 `remote=...`。节点公告的 enode 可能偏向另一个地址族，以真实远端连接地址判断本次连接。

如果两台本来就配置过 IPv4 Seed，本次新增 IPv6 地址不会自动删除它们；需要专门验证 IPv6 时，核对真实 `remoteAddress`，不要仅凭连接数认定使用了 IPv6。

追平后核对双方相同 USDB 高度的区块哈希，再按 [CPU 挖矿](mining.md)执行 `mining check` 和 `mining enable`。普通加入节点不使用 `--first-node`。

## 旧版本与升级边界

先执行 `usdb-node peers --help` 确认是否有本章命令。r20 的节点工具没有 `peers` 子命令，原链容器也可能只有 IPv4 网络；仅把 IPv6 enode 填进旧 `USDB_BOOTNODES` 配置，不能解决容器没有 IPv6 路由的问题。

应先取得支持 IPv6 的配套节点工具、运行配置和链镜像，再按目标版本的升级说明操作。仅更新工具或宿主机 IPv6 设置不足以证明运行中的链容器已经更新。

需要分别判断两件事：

- **P2P 地址族切换**本身保留已有数据，不要求重新同步 Bitcoin/BH。
- **跨发布版本升级**还要检查数据契约。旧 BH 快照模式切到原生 AssumeUTXO 时，不能直接套用兼容升级的 `activate-release`，应按[升级与恢复边界](maintenance.md#升级节点)单独安排。

## 常见问题

| 现象 | 处理 |
| --- | --- |
| 原始 enode 是 `127.0.0.1`，但网络已经是双栈 | 使用 `peers enode --family ipv6` 查询对外候选，并核对 `peers network --json`；不凭原始公告值判断监听失败 |
| `peers status` 没显示本机 enode | 旧版需额外使用 `peers enode`；包含本机地址展示改进的版本会直接显示 `Local P2P` |
| `dual` 模式只列出 IPv6 | 检查是否有可确认的对外 IPv4；双栈端口发布和对外地址生成是两项检查 |
| `P2P_IPV6_HOST_REQUIRED` / `P2P_IPV6_HOST_CHANGED` | 检查本机配置的 IPv6 是否仍存在、默认路由是否正常；地址变化后重新配置并重新导出 enode |
| `P2P_IPV6_RA_REQUIRED` | 由管理员检查实际接口的 RA 接收配置；恢复路由后重试 |
| `P2P_TRANSPORT_MISMATCH` | 实际容器的地址、网关或 TCP/UDP 发布与配置不符；核对配套版本与网络配置，修复后用 `peers apply` 重试 |
| 本机 enode 检查失败，但已有连接仍正常 | 分别查看 `local` 和 `membership`；本机探测失败不等于已有连接断开，也不能据此确认新入站可用 |
| Seed 已保存但 `WAITING_FOR_PEERS` | 核对地址、端口、防火墙和双方链容器的 IPv6 路由；宿主机 TCP 连通仅证明该 TCP 路径，不证明容器出站、UDP 发现或 P2P 握手 |
| 只在 IPv4/IPv6 之间摇摆 | 明确使用带方括号的 IPv6 地址，检查实际 `remoteAddress`；同时升级到包含地址族发现修复的配套链镜像 |

排查时保留双方版本、`peers status --json`、`peers network --json` 和链日志。分享公网节点地址不需要开放管理 RPC 或公开节点私钥。
