# USDB 节点 IPv6 与双栈运维

状态：第二阶段代码、本地回归和 Docker 29.5.3 同机跨网络验收已完成；公网 IPv6 出站等待开发机 RA
配置修复，真实两机公网入站验收仍待部署验证。
本次需配套发布 node kit 和 go-ethereum chain 镜像。旧节点保留 IPv4 配置，升级后通过命令选择新模式。

## 1. 地址族与部署前提

| 选择 | 行为 |
| --- | --- |
| `auto` | 新 `setup/configure` 默认值。发现稳定全局 IPv6、默认路由及受支持 Docker 时选择 `dual`，否则选择 `ipv4` 并说明原因 |
| `ipv4` | 维持现有 IPv4 容器网络和 P2P 发布方式 |
| `dual` | chain 增加双栈 bridge，发布 IPv4 和 IPv6 的 TCP/UDP `31303` |
| `ipv6` | chain 增加双栈 bridge，P2P 仅发布到 IPv6；内部数据服务和本地 RPC 仍通过 IPv4 访问 |

`auto` 的结果写入 `node.env`，重启时不重新选择。显式 `ipv6/dual` 缺少条件时直接报错，不降级为 IPv4。
`ipv6` 控制 P2P 端口的发布方式，并提供 IPv6 出站路径；它不禁止所有 IPv4 出站连接。仅提供 IPv6
上游网络的主机仍需能够下载 release、镜像和 snapshot，并访问 Bitcoin 网络，这不由 P2P 设置保证。

IPv6 模式的要求：

- Linux rootful Docker Engine **28.0.0 或更高**，Compose **2.33.1 或更高**。这是本方案的最低版本检查；
  不会自动升级 Docker，也不支持 rootless/Desktop 网络替代此配置。
- 宿主机已分配稳定 IPv6 地址并有默认路由。自动选择排除临时、过期、尚在 DAD 检查中及 Docker bridge
  地址。多地址主机建议显式指定 `--advertise-ipv6`。
- Docker 允许 IPv6 bridge 和相应的转发/NAT 规则；主机、路由器及云安全组允许所需 TCP/UDP 流量。
- 默认路由由 IPv6 Router Advertisement（RA）提供时，对应外网接口必须设置 `accept_ra=2`。
  Docker 创建 IPv6 bridge 会开启转发；`accept_ra=1` 此时不再接受 RA，现有默认路由会到期消失。
  预检会以 `P2P_IPV6_RA_REQUIRED` 拒绝显式 IPv6，自动模式则说明原因并选择 IPv4。
  由管理员针对实际外网接口设置，例如 `net.ipv6.conf.eth0.accept_ra=2`，并持久化到 sysctl 配置；
  工具不会自动修改主机 sysctl。默认路由已经消失时，还需等待或主动请求新的 RA。
  行为依据见 [Linux IPv6 accept_ra 定义](https://www.kernel.org/doc/html/latest/networking/ip-sysctl.html)。

chain 单独加入 `${USDB_DOCKER_NETWORK}-p2p-v6`，该 bridge 启用 IPv6/NAT，并以 `gw_priority: 1`
选择默认出口。Bitcoin、balance-history、indexer 等仍使用原来的共享 bridge。这里采用
[Docker bridge 网络](https://docs.docker.com/engine/network/drivers/bridge/)与
[Compose 的 gateway priority](https://docs.docker.com/reference/compose-file/services/#gw_priority)；
不能只在 IPv4 bridge 上添加一个 `[::]` 端口映射，就认为容器具有 IPv6 出站能力。

## 2. 新节点与现有节点

新节点在交互式 setup 时选择：

```bash
usdb-node setup --p2p-ip-family auto

# 明确要求 IPv6；NODE_IPV6 为本机已分配的稳定地址
usdb-node setup --p2p-ip-family ipv6 --advertise-ipv6 "$NODE_IPV6"
```

非交互式 `configure` 同样支持 `--p2p-ip-family` 和这些地址参数。两条 setup 示例是替代选择；
已经存在配置时，使用下面的 `peers configure`。

```bash
# 首节点提供双栈入口；PUBLIC_IPV4 可以是路由器映射的公网 IPv4
usdb-node peers configure --ip-family dual \
  --advertise-ipv4 "$PUBLIC_IPV4" --advertise-ipv6 "$NODE_IPV6"
usdb-node peers status --watch

# 查看宿主机地址、默认路由、实际容器 IPv6 地址和 TCP/UDP 发布情况
usdb-node peers network --json

# 分享同一 node key 对应的不同地址
usdb-node peers enode
usdb-node peers enode --family ipv6
```

`peers configure` 设置一组完整的 P2P 参数；省略地址时重新从本机选择，省略端口时采用 `31303`。
若公网 IPv4 位于路由器上，或使用自定义外部端口，重新配置时需要带上这些参数。
TCP 和 UDP 对外端口不同的场景使用 `--advertise-port 41303 --advertise-discovery-port 41304`。
它们分别映射到宿主机 TCP/UDP `31303`；该命令不修改路由器端口转发规则。

操作沿用 `node.peers.json` 持久任务与现有 controller，要求已安装 controller unit。上游同步期间可排队，
正在运行的 chain 会短暂停止并重建，保留链数据、node key、Seed 列表和已授权矿工身份。
chain 尚未运行时只保存配置，之后 `up` 启动。`APPLIED` 不表示已经连接到其他节点。

公布地址仅用于生成可分享的 enode，不覆盖 `USDB_NAT`，也不保证 Geth 的 ENR 同时公布两个地址族。
Geth 本次修复会在收到不同地址族的 ENR 时保留已验证的联系地址，避免 IPv6 Seed 被 IPv4 地址替换；
仍执行记录签名、节点身份和地址校验。同一公钥多个端点之间的自动故障切换尚未实现。
域名解析仍沿用当前 Geth 逻辑，不保证在 A/AAAA 之间选择所需地址族；明确需要 IPv6 时使用带方括号的 IP enode。

## 3. 防火墙与恢复

双栈需要 IPv4、IPv6 两侧的 TCP/UDP `31303` 都可达；IPv6 入站通常配置路由器防火墙放行，不需要 IPv4
式的端口映射。若另外做了端口转换，分享实际外部端口。RPC、WS 和内部服务继续仅发布到 `127.0.0.1`。

托管 UFW 模式会检查 `/etc/default/ufw` 中的 `IPV6=yes`、SSH/P2P 的 IPv6 allow 规则，并拒绝敏感 RPC
端口的 IPv6 allow 规则。切换前缺少规则时会报错，并在停止旧 chain 前中止。排除原因后：

```bash
usdb-node firewall apply --confirm
usdb-node peers apply
usdb-node peers status --watch
```

UFW 检查不代替 Docker 转发规则、云安全组和路由器的外部连通验证。external 防火墙模式由运维方管理规则。
配置保存后若 IPv6 地址或默认路由消失，`up/doctor` 会报错；应用中的任务也会保留错误。
恢复原地址/路由后使用 `peers apply` 重试。需要回到 IPv4 时可以重新提交：

```bash
usdb-node peers configure --ip-family ipv4 --advertise-ipv4 "$PUBLIC_IPV4"
usdb-node peers status --watch
```

当旧操作已经报错并停在停止/启动检查点时，允许上述修正；外部配置漂移或未完成的授权写入仍会阻止覆盖。
切回 IPv4 会让 chain 离开新增 bridge；可能遗留未被使用的 Compose 网络，不影响原共享网络和数据。

## 4. 地址展示与两机验收

`peers network/enode` 的 `CONFIGURED` 表示已观察到运行容器、匹配的 TCP/UDP 发布和所需容器 IPv6 端点。
`WAITING` 表示容器/RPC或地址尚不可用，`BLOCKED` 表示主机条件或网络身份不匹配；后两者返回非零退出码。
显式 `enode --family ipv6` 缺少该地址时返回非零，不输出 IPv4 作为替代。
地址族切换尚未应用时显示 `desired_family` 和操作阶段，并暂停分享地址，避免把旧配置当作新配置已生效。

所有候选地址均标注 `reachability=unverified`。公网 NAT 地址不会通过第三方服务推断，Docker 私网 IPv4
不会被当作可分享的公网地址。需要来自另一台机器的证据才能确认公网可达。

在第二台节点执行：

```bash
usdb-node peers configure --ip-family ipv6 --advertise-ipv6 "$JOINER_IPV6"
usdb-node peers status --watch
usdb-node peers add "$SEED_IPV6_ENODE"
usdb-node peers status --watch
usdb-node peers network --json
```

停止中的节点仍需 `up`。验收时记录双方的 chain ID、genesis、node ID、候选 enode 和真实 `connected`
的 remoteAddress。确认连接通过 IPv6、链高度持续同步，并核对共同高度的区块 hash。
分别覆盖 IPv4 加入、IPv6 加入、双栈首节点被两种地址族连接，以及切换/重启后仍使用同一 node key。
单次 TCP connect 或 `nc -u` 成功不能证明发现协议可用；隔离的新发现数据库加 Seed 后成功完成
UDP 发现和 RLPx TCP 连接，才是相应协议链路的验收。不要清理现有节点发现数据库来制造测试条件。

本地验收命令：

```bash
# usdb 仓库；Compose 客户端需满足上述版本，渲染测试不启动容器
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_node_p2p.py
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_node_peers.py
PYTHONDONTWRITEBYTECODE=1 python3 docker/scripts/tools/test_prepare_usdb_firewall.py

# go-ethereum 仓库；按仓库 Go toolchain 要求执行，需要本机 socket 权限
go test ./p2p/discover -run '^TestUDPv4_(ENRPreservesContactFamily|EIP868)$'
go test ./tests -run '^TestP2PTransportBootstrap$' -count=1 -timeout=150s
```

本地覆盖配置/重试/矿工授权、错误映射拒绝、防火墙、真实 Compose 合并，以及真实回环 UDP 发现和 TCP
握手。回环测试不覆盖 Docker NAT、宿主机外部 IPv6 路由和公网防火墙；这些仍须在符合版本要求的两机上验收。
第三阶段“提前 mining enable，自动等待同网同步后再挖矿”仍待实现，当前继续遵循现有 mining 预检。

可选的真实 Docker 验收使用实际 Compose 网络/端口配置，以测试探针替换 chain 进程和数据目录：

```bash
# go-ethereum 仓库：生成无外部动态库依赖的测试探针
CGO_ENABLED=0 go build -o /tmp/usdb-p2p-probe ./tests/common/p2pprobe

# usdb 仓库：需要本机 Docker/网络权限、空闲 TCP/UDP 31303 和本地 alpine:3.20 镜像
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_node_p2p_docker.py \
  --probe-binary /tmp/usdb-p2p-probe
```

该命令创建独立 Compose 项目，依次验证 IPv4 → 双栈 → IPv6 → IPv4，检查实际映射、独立网络中的
客户端经宿主机地址收到签名 UDP Pong 并完成 RLPx TCP 握手、内部数据服务 DNS/IPv4 连通及 node key
保持。IPv6/IPv4 出站通过 Debian 网站 HTTPS HEAD 验证；主机本身不可达时在报告中单独记录。
不运行区块链或上游索引服务，不挂载现有节点目录。结束后清理本次容器和网络，保留临时目录中的 JSON
报告及日志。测试能验证同一主机上的 Docker 转发链路，不能替代异地主机的公网入站防火墙验收。
仅验证同机 Docker 链路可显式加 `--local-only`，其报告状态为 `PASS_LOCAL`，不表示主机启动资格或公网
出站通过。完整模式如果主机 HTTPS 探测失败，则报告 `PARTIAL`，不能当作全部通过。

开发机本次 Docker 29.5.3 / Compose 5.5.1 实测：四次模式切换与五次指定地址族的签名 UDP/RLPx TCP
连接通过；内部数据服务保持 IPv4 连通，重建后 node ID 一致，非选定地址族不暴露 TCP，HTTP/WS 仍只绑定
loopback，测试资源清理后容器/网络清单与开始时相同。验收还复现了 RA 默认路由到期问题，已补入预检及回归。
