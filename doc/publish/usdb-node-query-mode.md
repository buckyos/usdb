# 专用查询节点：full + archive + 私有 tracing

节点角色与状态保留策略相互独立。专用查询服务器可以使用 `full` 角色，不挖矿，
从同一网络同步并验证区块，同时保留历史 EVM 状态、为同机 USDB Explorer 提供 tracing。
普通 full/miner 节点继续使用默认的 `USDB_CHAIN_GCMODE=full`、`USDB_CHAIN_TRACING=0`。
这些选项不改变 chain ID、genesis、协议或矿工资格。

该配置必须使用包含本功能的 **node kit 和 usdb-chain 镜像配套 release**；
只更新 Python 工具或只改旧镜像的环境变量不会使旧入口支持这些选项。
当前发布网络仍是 `usdb-testnet-v0`。此部署结构可用于后续正式网，但不会启用尚未发布的主网
bundle，也不代表已完成主网镜像、历史回放、重组恢复和负载验收。

## 启用与检查

新服务器先按对应网络的部署文档完成配置，保持 `full` 角色，在首次同步 USDB 链之前启用：

`usdb-node setup` 会询问
`Provide full Explorer support (archive + private tracing) [y/N]`。
默认回车或选择 `n` 保持普通节点配置；选择 `y` 会在首次写入配置时同时设置
`USDB_CHAIN_GCMODE=archive` 和 `USDB_CHAIN_TRACING=1`，无需再单独执行 setter。
向导会在最终写入确认前显示选择结果；这只启用节点的 RPC 能力，Explorer 仍需独立部署。

已经配置过的节点使用原有入口：

```bash
# 已经启动过的节点须先停止；此命令保留所有数据。
usdb-node down
usdb-node set-query-mode --state-mode archive --tracing on
usdb-node query-mode
usdb-node up
```

命令只修改本地状态保留/tracing 配置，保留角色、网络身份、凭据和数据路径，拒绝运行中改配置。
不带 `--state-mode` 或 `--tracing` 时保留对应的原值，但至少需要提供一项。
`query-mode --json` 可用于自动化；`historical_coverage=unverified` 表示这里只读取配置，
没有证明运行中的镜像参数或完整历史覆盖。启动日志会记录实际入口的 `state_mode` 和
`private_http_tracing`。启用后应在宿主机回环 RPC 检查 `rpc_modules` 中的 `debug`，并在
同步完成后执行 `usdb-explorer preflight`；采样成功也不等于全历史验收。

已有矿机不会被该命令自动改成 full。专用查询服务器应从 full 角色开始；已有矿机切换职责时，
先通过 `usdb-node mining disable` 完成受管停矿，再配置查询模式。

旧配置中的 `USDB_CHAIN_EXTRA_ARGS=--gcmode archive` 或 `--gcmode=archive` 仍可识别；
执行 setter 时迁移到正式字段并保留其他额外参数。新字段与旧参数冲突或出现重复参数会报错，
需先消除冲突。HTTP/WS 参数由运行时管理，不能通过 EXTRA_ARGS 覆盖。

## 历史覆盖与回退

`archive` 只保证启用后保留所执行区块的状态，**不会补回已裁剪的旧状态**。
已有节点即使已经追平、当前余额可查，早期区块仍可能返回 `missing trie node`。
要覆盖从创世块开始的完整历史，应在独立的新数据目录从 genesis 完整执行，或恢复经过验证的
完整 archive 备份；保留原数据作为回退来源，不直接清理现有节点。普通 full 节点数据、
balance-history 快照或 indexer checkpoint 均不能单独证明 EVM archive 完整。

USDB 的历史执行还需要 indexer 提供相应 BTC anchor/profile 数据；必须按目标网络验证其
历史覆盖和重放结果，不能仅凭以太坊客户端支持 `--gcmode archive` 推定 USDB 全历史验收通过。
tracing 在缺少直接可用状态时可能触发历史重执行，耗时和成功率取决于可用状态与 indexer，
增大 reexec 不能替代完整 archive 数据。

关闭 tracing 或返回普通状态保留模式也须先 `down`，再执行：

```bash
usdb-node set-query-mode --state-mode full --tracing off
usdb-node up
```

这不会改变挖矿角色，也不负责回收已有 archive 的磁盘空间。返回 full 后不再保证后续的完整
历史状态保留；未来再次开启 archive 仍需重新检查覆盖范围。

## 私有 RPC 与同机 Explorer

启用 tracing 会在节点现有 HTTP RPC 上添加 **整个 `debug` namespace**，不只开放两个 trace 方法。
它是给可信本机运维和 Explorer backend 使用的接口，不是带鉴权的公共 tracing 产品。
node kit 继续要求宿主机 HTTP/WS 绑定 `127.0.0.1`，容器内可在节点私有网络访问；
WS 不增加 debug，自定义 WS namespace 中包含 debug 也会被拒绝。

同机 Explorer 使用 `rpc.mode=local-node`，经现有私有 relay 访问节点 loopback RPC。
公共网关仍按方法白名单服务，不转发 debug。公网只开放浏览器入口和需要的 P2P 端口，
不要把 8545/8546 映射到公网。若以后把 Explorer 和查询节点分机部署，应另行提供受控的私网
转发与访问控制，不能直接把本机模式的 RPC bind 改成 `0.0.0.0`。

## 同机资源预算

archive 增加持续磁盘写入与空间需求；本分支的 hash 状态存储还会保留交易索引并启用 preimages。
tracing 和 Blockscout 初始索引会争用 CPU、内存、磁盘 I/O，适合独立查询服务器，避免挤占矿机。
磁盘要同时计入 Bitcoin、balance-history、USDB indexer、EVM archive 和 Explorer PostgreSQL，
不能用现有快照大小推算 archive 容量。

节点自动资源策略不会自动发现尚未启动的 Explorer。为其显式保留预算，例如默认 Explorer
容器总预算为 6 GiB 时，在节点停止状态下执行：

```bash
usdb-node set-resource-policy --mode auto --external-memory-budget 6g
usdb-node resources
```

预算应匹配实际 Explorer 限额；节点各阶段仍须满足宿主机总内存约束。Explorer 的
`other_services_memory_gib=auto` 会反向计入其他运行中容器限额，但不会替节点预留未来服务的空间。
query-mode 不自动提高 chain 内存限额，不承诺高 tracing 并发；正式网需按目标负载测量
RPC 延迟、索引追赶速度、峰值内存和磁盘增长后制定容量与并发配置。

Blockscout 的上游能力要求见
[Node Tracing / JSON RPC Requirements](https://docs.blockscout.com/setup/requirements/node-tracing-json-rpc-requirements)。
