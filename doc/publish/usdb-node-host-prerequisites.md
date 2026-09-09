# USDB 节点主机软件基线与准备工具

## 1. 适用范围

本文定义 USDB 节点通用的主机软件边界，适用于开发网、测试网和正式网。网络身份、genesis、
BTC registry、PoW 参数、端口暴露和节点角色由各自 network bundle 决定，不应进入软件准备工具。

Ubuntu 24.04 是当前优先验证的运维基线，但不是协议或运行时硬要求。容器内已经固定用户态依赖，
宿主机只需提供满足要求的 Linux kernel、amd64 架构、Docker runtime 和少量运维命令。

## 2. 硬性运行边界

| 项目 | 当前要求 | 原因 |
| --- | --- | --- |
| Kernel | Linux 5.10 或更高 | 采用仍广泛使用的 LTS 内核代际作为项目运维下限 |
| 架构 | x86-64/amd64 | 当前 services、chain、Bitcoin 三张发布镜像只构建 `linux/amd64` |
| Cgroup | v1 或 v2 | Docker memory limit 和 balance-history cgroup-aware cache 都需要有效层级 |
| Docker | Linux rootful Engine **28.0.0 或更高**，daemon 可访问 | 所有节点统一采用支持 IPv6 bridge/NAT 的运行时基线，不支持 rootless 或 Docker Desktop |
| Compose | `docker compose` plugin **2.33.1 或更高** | runtime 使用 Compose overlay；双栈网络需要 `gw_priority` |
| 运维命令 | Git、Python 3、curl、jq | checkout、bundle 校验、readiness 与 RPC 检查所需 |

发行版名称不参与共识，也不是硬门槛。`check` 可在任何满足上述条件并提供 `/etc/os-release`
的 Linux 发行版运行。

以上版本下限适用于所有节点角色、网络和 P2P 地址族，包括仅使用 IPv4 的现有节点。
Engine 按实际 daemon/server 版本检查，仅升级 Docker CLI 不满足要求；Compose 按数字版本比较，
例如 Compose 5.x 也高于此下限。采用稳定版本，预发布版本不作为部署基线。
`host check`、`doctor` 和 `up` 的 controller 预检共享该检查，低版本不能通过选择 IPv4 绕过。
后续发布文档引用本表作为最低要求；具体 release 仍需记录实际验收版本及结果。

当前自动安装只覆盖以下经过明确编码的 APT 系发行版：

- Ubuntu 22.04、24.04、26.04；
- Debian 12、13。

Ubuntu 26.04（包括 26.04.1）使用 Docker 仓库的 `resolute` suite（官方源或其镜像）；
[Docker 官方安装文档](https://docs.docker.com/engine/install/ubuntu/)已列出该 LTS 版本。
自动安装支持不等同于完整节点部署验收，Ubuntu 24.04 仍是优先验证的运维基线。

其他发行版先使用原生包管理器安装 Docker Engine、Compose plugin、Git、Python 3、curl 和 jq，
再运行同一个 `check`。不要为了通过检查而修改 `/etc/os-release` 或伪装发行版。

> 当前硬性保留 amd64，是 release artifact 能力限制，不是 Rust、Bitcoin 或 USDB 协议限制。
> 只有在三张镜像、CI 和目标硬件测试全部增加 arm64 后，才能放宽这一项。

## 3. 工具接口

Release node kit 安装后，首选统一入口：

```bash
usdb-node prepare-host
usdb-node host check
usdb-node host install
```

`prepare-host` 先运行只读检查，仅在失败时询问是否安装。`host check/install` 是无人值守和故障排查入口；
非 root 运行时默认检查当前用户的 Docker group membership。底层实现仍是 node kit 内的
`docker/scripts/tools/prepare_usdb_host.sh`。

源码 checkout 或 node kit 尚不可用时，可以直接执行底层只读检查：

```bash
docker/scripts/tools/prepare_usdb_host.sh check
docker/scripts/tools/prepare_usdb_host.sh check --docker-user usdb
```

检查会聚合输出以下结果并以非零状态拒绝不合格主机：

- distribution 信息、Linux kernel 和架构；
- Docker CLI、Compose、Git、Python、curl、jq 的实际版本；
- Docker daemon 实际版本、rootful Linux engine 类型和 cgroup v1/v2；
- 可选运行用户的 `docker` 组成员关系。

底层自动安装：

```bash
sudo docker/scripts/tools/prepare_usdb_host.sh install --docker-user usdb
```

安装器默认先使用 Docker 官方 APT repository，下载失败后回退到清华 Docker CE 镜像。若 Docker Engine 和 Compose 已经完整存在，则保留现有
安装；若发现 `docker.io`、`podman-docker`、`containerd` 或 `runc` 等冲突包，则在任何包安装前
停止并给出人工处理提示。它不会自动卸载容器软件，也不会删除 `/var/lib/docker` 或节点数据。
保留现有安装不等于版本合格：现有 Docker/Compose 低于下限时，最终检查仍会失败，并提示管理员
主动升级后复检。升级前评估正在运行的容器和 Docker 重启影响；工具不会隐式升级现有 Docker。

`install` 安装仓库当时提供的 stable Docker 版本，不把具体 Docker patch version 写入网络身份。
每次 release 应归档 `check` 输出；正式网上线可在 release checklist 中进一步冻结已验证版本。

### 3.1 Docker 软件源与网络重试

三个安装入口都接受 `--docker-mirror auto|official|tuna`，默认 `auto`：

```bash
usdb-node prepare-host                         # 官方源失败后自动回退清华源
usdb-node prepare-host --docker-mirror tuna     # 直接使用清华源
usdb-node host install --docker-mirror official # 只使用官方源，失败即停止
docker/scripts/tools/prepare_usdb_host.sh install --docker-user bucky --docker-mirror tuna
```

| 选择 | Docker CE 仓库地址 | 行为 |
| --- | --- | --- |
| `auto` | 先 `official`，再 `tuna` | 下载阶段失败后打印原因并切换，两个来源都失败则退出 |
| `official` | `https://download.docker.com/linux/<ubuntu或debian>` | 重试后仍失败即退出，不切换来源 |
| `tuna` | `https://mirrors.tuna.tsinghua.edu.cn/docker-ce/linux/<ubuntu或debian>` | 直接使用清华源，重试后仍失败即退出 |

公钥与软件包使用同一选定来源，Ubuntu 26.04 始终使用 `resolute` suite。
仅修改 Ubuntu 自身的 APT 镜像不会改变 Docker CE 来源；Docker CE 软件仓库也不等同于
Docker Hub/GHCR 容器镜像加速。清华仓库说明见[官方帮助](https://mirrors.tuna.tsinghua.edu.cn/help/docker-ce/)。

重试策略：公钥下载最多尝试 3 次，包括连接重置等 curl 错误；连接超时 10 秒，单次请求最多
30 秒，两次重试之间等待 2 秒，重试窗口 90 秒（窗口内启动的最后一次请求仍受单次超时限制）。
APT 对下载失败的文件最多重试 2 次，HTTP/HTTPS 连接及数据等待超时均为 30 秒；这是每次网络等待的限制，
不是整个包下载或安装流程的总时限。

官方源的公钥、仓库索引或安装包下载失败，都可触发 `auto` 回退。Docker 仓库更新使用
`--error-on=any`，不将失败后复用旧索引视为成功。全部包先以 `--download-only` 下载，再以
`--no-download` 安装；本地写入或实际 APT/dpkg 安装错误会直接停止，不通过切源反复执行安装。
所有来源失败时不启动 Docker；如果已经写入仓库配置，会保留最后一次尝试的 Docker 来源，供排查和重跑。
基础依赖更新使用临时的 APT 来源列表，排除工具管理的 `docker.sources`，避免上次失败残留的 Docker
来源阻塞修复；其他系统来源保持有效且更新必须成功。手工添加在其他 `.list` 或 `.sources` 中的
重复 Docker 来源仍需运维者自行处理。

HTTPS 证书验证与 APT 的 `Signed-By` 签名验证保持启用。下载的公钥必须匹配脚本内固定的
Docker 官方公钥 SHA-256；公钥不匹配时立即停止，不自动接受镜像站提供的新密钥。
该公钥于 2026-09-08 对照 Docker 官方 Ubuntu/Debian 下载地址及清华镜像核对一致。
若 Docker 更新公钥文件，维护者须重新核验官方来源，并同步更新脚本中的摘要和
`tests/common/docker-signing-key.asc` 测试样本。

## 4. 全新机器引导

完全空白机器可能尚未安装 Git/curl，不能先 clone 仓库。应从发布协调机传入 candidate revision
中的脚本：

```bash
scp docker/scripts/tools/prepare_usdb_host.sh root@<node-ip>:/tmp/
ssh root@<node-ip> 'chmod 0755 /tmp/prepare_usdb_host.sh'
```

创建专用运行用户并安装：

```bash
ssh root@<node-ip>
id usdb >/dev/null 2>&1 || useradd --create-home --shell /bin/bash usdb
/tmp/prepare_usdb_host.sh install --docker-user usdb
```

退出当前 SSH session，以 `usdb` 重新登录，然后复检：

```bash
/tmp/prepare_usdb_host.sh check --docker-user usdb
```

`usdb` 是推荐的专用账号名称，也可以使用 `bucky` 等已有普通账号；`usdb-node prepare-host`
默认使用当前用户。加入 Docker 组只会更新账号配置，已经打开的终端不会自动获得该组权限。
`prepare-host` 和 `setup` 完成时会检查当前进程的实际用户组；账号已加入但当前终端尚未生效时，
会显示 `WARN Docker session`。此时 `doctor` 仍会失败，并明确提示 Docker 组权限尚未生效。

如果希望保留当前 SSH 连接，可以在该普通用户的终端执行：

```bash
newgrp docker
usdb-node doctor
usdb-node up
```

`newgrp docker` 打开一个取得 Docker 组权限的新 shell，后续命令需在其中执行；`exit` 会返回原 shell，
其他已打开的会话也不会被刷新。脚本不能修改调用它的父 shell 的用户组，因此不会自动替用户切换 shell。
如果账号尚未加入 Docker 组，应先完成 `prepare-host` 安装；如果组权限已经生效但 daemon 仍不可访问，
则需要检查 Docker 服务和 socket。

复检通过后才能 clone 固定 revision、写入 node-local secret，并进入对应网络的部署手册。

## 5. 防火墙准备

软件准备工具不会修改防火墙。Release node kit 的 `setup` 生成私有 `node.env` 并询问是否让 `usdb-node`
管理 UFW，默认选择 `external`，即不安装、不读取也不修改 UFW。只有 managed 模式才确认 SSH port。云安全组、
虚拟化平台防火墙、已有宿主机规则或隔离 VM 都可以采用 external 模式；容器 bind address 仍会被校验。

需要使用项目 UFW profile 时显式切换：

```bash
usdb-node set-firewall-mode --mode managed
usdb-node firewall apply --confirm
usdb-node firewall check
```

`doctor` 仅在 managed 模式执行 UFW 只读检查；external 模式会报告跳过 UFW。源码 checkout 的直接脚本
接口保留为手工回退路径。

测试网和正式网共用该工具，但具体端口和 public/private P2P 决策必须服从对应 network bundle 与
发布手册。完整边界见 [USDB 节点防火墙与端口暴露操作](./usdb-node-firewall-operations.md)。

## 6. 安全与运维边界

- `docker` 组拥有 root 级主机权限，只允许专用运维用户加入；
- Docker 发布端口可能绕过 UFW；容器 bind address 始终校验，external 模式还必须独立复核上游防火墙；
- 已有容器工作负载的主机不能直接删除冲突包，应先评估迁移和数据保留；
- 自动安装不修改 Docker daemon storage driver、data root、日志策略或防火墙；
- 正式网可复用同一工具，但必须使用正式网单独冻结的 network bundle 和 release manifest。
