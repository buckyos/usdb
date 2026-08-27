# USDB 节点防火墙与端口暴露操作

## 1. 适用范围

本文定义 USDB 测试网和正式网节点的主机入站基线。具体网络的 P2P 端口来自 network bundle；
当前 `testnet-v0` 使用 `31303/TCP+UDP`。工具位于：

```text
docker/scripts/tools/prepare_usdb_firewall.sh
```

当前自动处理后端为 UFW，适用于项目优先验证的 Ubuntu/Debian 节点。云安全组、机房 ACL、路由器
端口转发和托管防火墙不由本工具修改，必须在部署记录中单独复核。

## 2. 端口语义

| 端口 | 入站策略 | 用途 |
| --- | --- | --- |
| SSH operator port | 必须按实际端口保留 | 节点运维；可进一步限制来源 IP |
| `31303/TCP` | testnet-v0 必须公开 | USDB devp2p 会话和区块/交易传播 |
| `31303/UDP` | testnet-v0 必须公开 | USDB devp2p 节点发现 |
| `8333/TCP` | 默认不公开，可选 | Bitcoin mainnet 入站 P2P；不是 USDB 服务依赖 |
| `8332/TCP` | 禁止公开 | Bitcoin JSON-RPC，只供 Docker 私网中的 USDB 服务使用 |
| `8545/8546` | 禁止公开 | USDB HTTP/WS operator RPC |
| `28010/28020/28040` | 禁止公开 | balance-history、indexer 和 control-plane RPC |

关闭 `8333/TCP` 入站不会阻止 Bitcoin Core 主动建立出站 peer，也不会阻止通过这些已建立连接同步
区块和交易。它只表示该节点不向 Bitcoin 网络提供公网入站连接容量。首个 USDB bootnode 的
`31303/TCP+UDP` 不同：其他 USDB 节点需要一个稳定可达的初始 peer，因此必须同时在主机防火墙和
上游网络防火墙中允许。

## 3. Docker 与 UFW 的边界

Docker 发布的容器端口可能绕过 UFW 的普通 `INPUT` 规则。因此安全边界不能只依靠一条 UFW
deny 规则：

1. `node.env` 必须把 operator API 固定到 `127.0.0.1`；
2. 默认私有 Bitcoin P2P 必须设置 `BTC_P2P_BIND_ADDRESS=127.0.0.1`；
3. USDB P2P 设置 `USDB_P2P_BIND_ADDRESS=0.0.0.0`；
4. UFW 使用默认拒绝入站，只放行 SSH、USDB P2P 和可选 Bitcoin P2P；
5. 云安全组或机房 ACL 使用相同允许列表。

`prepare_usdb_firewall.sh` 会先检查上述 bind 地址，再检查或修改 UFW。这样即使 Docker 的转发规则
不经过 UFW，敏感容器端口也只发布到 loopback。

## 4. 默认私有 Bitcoin P2P

先生成并编辑未提交的 `node.env`，保留：

```text
BTC_P2P_BIND_ADDRESS=127.0.0.1
BTC_P2P_BIND_PORT=8333
USDB_P2P_BIND_ADDRESS=0.0.0.0
USDB_P2P_BIND_PORT=31303
```

应用规则前必须明确填写当前 SSH 端口。以下示例采用 `22`：

```bash
sudo docker/scripts/tools/prepare_usdb_firewall.sh apply \
  --node-env docker/networks/testnet-v0/node.env \
  --ssh-port 22 \
  --bitcoin-p2p private \
  --confirm
```

`apply` 会在 APT 主机缺少 UFW 时安装 UFW，先允许 SSH，再设置默认拒绝入站、允许出站，最后放行
`31303/TCP+UDP`。它不会 reset 或删除无关规则；如果发现敏感 RPC/API 端口被显式允许，最终检查会
失败并要求人工清理。

只读复检：

```bash
sudo docker/scripts/tools/prepare_usdb_firewall.sh check \
  --node-env docker/networks/testnet-v0/node.env \
  --ssh-port 22 \
  --bitcoin-p2p private
```

## 5. 可选公开 Bitcoin P2P

只有明确希望该机器向 Bitcoin 网络提供入站 full-node 容量时，才将：

```text
BTC_P2P_BIND_ADDRESS=0.0.0.0
```

然后应用和检查 public profile：

```bash
sudo docker/scripts/tools/prepare_usdb_firewall.sh apply \
  --node-env docker/networks/testnet-v0/node.env \
  --ssh-port 22 \
  --bitcoin-p2p public \
  --confirm
```

该模式额外允许 `8333/TCP`。Bitcoin RPC `8332` 仍保持 Docker 私网可见，不得随 P2P 一起开放。

## 6. 验收与归档

每台节点至少归档以下非敏感信息：

```bash
sudo docker/scripts/tools/prepare_usdb_firewall.sh check \
  --node-env docker/networks/testnet-v0/node.env \
  --ssh-port <actual-ssh-port> \
  --bitcoin-p2p private

sudo ufw status verbose
docker compose --env-file docker/networks/testnet-v0/node.env \
  -f docker/compose.bitcoin.yml config
docker compose --env-file docker/networks/testnet-v0/node.env \
  -f docker/compose.runtime.yml config
```

不要归档完整 `node.env`，其中包含 Bitcoin RPC password。只保存检查输出、节点角色、公开端口、
上游防火墙规则摘要和操作时间。通过外部机器再次探测公网 IP，确认 `31303/TCP+UDP` 可达，并确认
`8332`、`8545`、`8546`、`28010`、`28020`、`28040` 不可达。
