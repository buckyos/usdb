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
| Docker | Engine daemon 可访问 | 节点全部通过 digest-pinned Linux containers 运行 |
| Compose | `docker compose` plugin | runtime 使用 Compose overlay 和分阶段服务编排 |
| 运维命令 | Git、Python 3、curl、jq | checkout、bundle 校验、readiness 与 RPC 检查所需 |

发行版名称不参与共识，也不是硬门槛。`check` 可在任何满足上述条件并提供 `/etc/os-release`
的 Linux 发行版运行。

当前自动安装只覆盖以下经过明确编码的 APT 系发行版：

- Ubuntu 22.04、24.04；
- Debian 12、13。

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
- Docker daemon、Linux engine 类型和 cgroup v1/v2；
- 可选运行用户的 `docker` 组成员关系。

底层自动安装：

```bash
sudo docker/scripts/tools/prepare_usdb_host.sh install --docker-user usdb
```

安装器使用 Docker 官方 APT repository。若 Docker Engine 和 Compose 已经完整存在，则保留现有
安装；若发现 `docker.io`、`podman-docker`、`containerd` 或 `runc` 等冲突包，则在任何包安装前
停止并给出人工处理提示。它不会自动卸载容器软件，也不会删除 `/var/lib/docker` 或节点数据。

`install` 安装仓库当时提供的 stable Docker 版本，不把具体 Docker patch version 写入网络身份。
每次 release 应归档 `check` 输出；正式网上线可在 release checklist 中进一步冻结已验证版本。

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
