# USDB 测试网节点角色与 CPU 挖矿方案

Status: implemented configuration; staged testnet rollout remains manual.

本文规定首个 USDB 三节点测试网的角色演进、矿工身份隔离和 CPU-only Ethash 运行基线。它适用于
`testnet-v0` 启动及早期 soak，不改变 PoW 共识规则、难度参数或网络身份。

## 1. 角色语义

当前 chain runtime 支持：

- `miner`：完整验证并导入区块，同时构造候选区块并执行本地 Ethash sealing；
- `full`：完整验证、同步和转发区块，不执行本地 sealing；
- `bootnode`：用于节点发现和稳定 P2P 入口，不拥有额外共识权力。

USDB PoW 没有独立投票型 validator 角色。`miner` 和 `full` 都必须执行相同的 header、selector、
difficulty、reward 和 state transition 校验；区别只是是否参与本地挖矿。因此增加 full 节点会增加
独立验证副本和可观测性，不会形成 BFT quorum。

## 2. 首轮三节点推进顺序

使用节点 A、B、C，避免把机器编号、miner 编号和 release generation 混在一起：

| 阶段 | 节点 A | 节点 B | 节点 C | 验收重点 |
| --- | --- | --- | --- | --- |
| upstream preflight | `full` | 离线 | 离线 | BTC、balance-history、indexer、genesis 和 bundle identity |
| cold start | `miner` | 离线 | 离线 | 单矿工连续出块并完成 SourceDAO bootstrap |
| online validator | `miner` | `full` | 离线 | B 从 genesis 追平并独立验证 A 的区块 |
| late joiner | `miner` | `full` | `full` + 空 chain datadir | C 完成历史 replay，三节点 head/state root 一致 |
| multi-miner | `miner` | `miner` | `full` | 双矿工竞争、退出容错及永久非矿工验证路径 |
| stress window | `miner` | `miner` | 可临时切换 `miner` | 三矿工 stale/reorg 与难度收敛；结束后 C 可恢复 `full` |

冷启动阶段建议先由 A 产生至少 20 个 USDB 区块，再启动 B；A/B 同步后继续产生一批区块，再以
空 chain datadir 启动 C。首轮稳定拓扑冻结为 `2 miner + 1 full`。这已经覆盖多矿工和非矿工验证，
不要求在首次上线前增加到 `3 miner + 2 full`。

后续扩展到五节点时，新增 full 节点只有在使用独立 BTC、balance-history 和 usdb-indexer 时，才能
验证 external state 的独立收敛。共享同一 indexer 的多个 chain 节点仍有价值，但不能发现上游的
相关性故障。

## 3. 矿工身份

每个长期 miner 应使用不同的：

- `USDB_MINER_ADDRESS`；
- Active Standard pass；
- inscription `usdb_main`；
- 收益地址和私钥负责人。

多个进程共用同一个 `usdb_main` 不能覆盖独立 candidate、奖励归属、consume/remint 和 pass 失效行为。
Bootstrap Admin 必须与矿工身份分离；同一负责人可以保管两类 signer，但地址、用途和审计记录不能混用。

## 4. CPU-only Ethash 配置

测试网机器没有 GPU 时，geth 使用内置 CPU Ethash sealer。规范配置为：

```text
USDB_NODE_ROLE=miner
USDB_MINER_ADDRESS=<active-standard-pass-usdb-main>
USDB_MINER_THREADS=1
```

runtime 会转换为：

```text
--mine --miner.threads 1 --miner.etherbase <address>
```

`miner.threads=1` 表示每份 sealing work 只启动一个 nonce-search worker，稳态挖矿通常消耗约一个逻辑
核。不要省略该值，也不要用 `0` 表示一个线程；当前 geth CLI mining startup 会把 `0` 转成禁用本地
CPU sealing。

该参数只限制 Ethash sealing worker，不是整个 geth 进程的 cgroup 限额。以下任务仍可能短时使用额外 CPU：

- Ethash cache/DAG 初始化；
- chain sync、区块导入和状态提交；
- RPC、P2P 和数据库压缩；
- Go runtime 的其他后台任务。

当前 runtime Compose 对 chain 容器设置 `USDB_CHAIN_MEMORY_LIMIT`，但没有 CPU hard quota。因此首轮
以 `USDB_MINER_THREADS=1` 作为挖矿约束，同时要求宿主机至少 8 个逻辑核，不在同步/挖矿期间执行构建、
压缩或容量测试。若目标机器仍出现资源竞争，应增加可选的 Compose CPU quota；quota 会限制整个 geth，
不能只限制 Ethash，建议从 2 CPU 开始验证，而不是直接把整个 chain 容器压到 1 CPU。

## 5. 后续接入专用 GPU 矿机

当前 Ethash engine 保留标准 remote sealer RPC：

- `eth_getWork`：取得 seal hash、DAG seed、target 和 block number；
- `eth_submitWork`：提交 nonce、seal hash 和 mix digest；
- `eth_submitHashrate`：可选上报 worker hashrate；
- `eth_hashrate`：查询节点已知的本地与已上报远程算力。

专用 GPU 矿机加入后应拆成两层：

```text
USDB chain mining coordinator
  - 查询 indexer，解析 pass/profile
  - 构造完整 USDB block candidate
  - 发布 Ethash work
  - 验证并提交 GPU 返回的 PoW

GPU worker / mining farm
  - 只生成 DAG 并搜索 nonce
  - 不直接查询 BTC/indexer
  - 不选择 pass、coinbase 或 USDB policy
```

coordinator 使用：

```text
USDB_NODE_ROLE=miner
USDB_MINER_ADDRESS=<mining-identity-usdb-main>
USDB_MINER_THREADS=0
```

当前 geth mining startup 会把 `miner.threads=0` 转为禁用本地 CPU sealing，但仍启动 block builder 和
remote sealer。这样 geth 继续产生绑定该 `USDB_MINER_ADDRESS` 的 work，实际 Ethash 计算全部交给 GPU。

同一 coordinator 后面的多台 GPU worker 共享同一个 pass、coinbase 和收益身份。若矿场需要多个独立
USDB 矿工身份，应为每个 `USDB_MINER_ADDRESS` 使用独立 coordinator；不能让 worker 在收到 work 后自行
替换 coinbase 或 pass。

### 5.1 RPC 与网络边界

geth 原生提供 HTTP JSON-RPC remote mining 协议，不直接提供矿池常用的 Stratum listener。只支持
Stratum 的 GPU miner 需要经过兼容性验证的 mining proxy/pool gateway，将 Stratum work 映射到
`eth_getWork/eth_submitWork`。

不得把当前 operator `8545` 直接开放到公网。GPU 接入至少需要：

- 独立 mining VLAN、VPN 或私网接口；
- 防火墙只允许登记过的 gateway/worker 地址；
- 对外 mining endpoint 只允许 `eth_getWork`、`eth_submitWork`、`eth_submitHashrate` 等必要方法；
- operator `admin/miner/txpool` RPC 继续保持 localhost、SSH tunnel 或独立受控入口；
- 保持 PoW solution verification，禁止启用 `--miner.noverify`。

当前 runtime 只有一个 HTTP API listener，默认同时启用 operator API。正式接入远程 GPU 前，应增加受控
mining gateway 或 RPC method allowlist，不能仅把 `USDB_HTTP_BIND_ADDRESS` 改为公网地址。

### 5.2 GPU 上线前门禁

GPU 算力可能比单线程 CPU 高多个数量级。不能只把 `USDB_MINER_THREADS` 改成 `0` 就直接接入现有网络，
还必须：

1. 使用目标 GPU 型号、数量、功耗限制和矿工软件运行 PoW calibration；
2. 验证 DAG 显存、epoch 切换、worker restart 和 stale-work 处理；
3. 以单 GPU、部分矿机、全矿场和 miner-loss 四档验证 retarget；
4. 检查 CPU 候选难度下是否产生一秒时间戳饱和、大量 stale block 或连续短分叉；
5. 冻结适合新算力分布的 difficulty 参数和 activation/new-generation 策略；
6. 用选定的 GPU miner 或 Stratum gateway 完成 invalid/stale solution、断线和重连 E2E。

GPU worker 本身不需要运行 Bitcoin、balance-history 或 usdb-indexer；但 coordinator 是共识入口，必须运行
完整受控 chain/upstream stack。若矿机运营方希望独立验证而非信任共享 coordinator，则应自行运行完整
节点和 coordinator。

## 6. 运行与应急控制

长期配置通过私有 `node.env` 管理。修改 `USDB_NODE_ROLE` 或 `USDB_MINER_THREADS` 后，使用标准 runtime
helper 重新创建 chain 容器；不要修改 network bundle。

本地 operator RPC 保留 `miner` API 时，可以临时执行：

```bash
curl -fsS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"miner_stop","params":[]}' \
  http://127.0.0.1:8545

curl -fsS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"miner_start","params":[1]}' \
  http://127.0.0.1:8545
```

RPC 只用于受控应急和测试；持久角色仍以 `node.env` 为准。HTTP/WS 不得暴露到公网。

## 7. 观测与通过条件

每个阶段至少记录：

- `docker stats` 中 chain 容器 CPU、内存和是否持续 throttling；
- `eth_hashrate`、USDB block interval 和连续不出块时长；
- 三节点 head height、block hash、state root、genesis hash 和 chain ID；
- 每台 indexer 在同一 BTC stable height 的 snapshot/system-state identity；
- peer count、节点重启追平时间、stale block 和 reorg 数量；
- miner 退出后剩余 miner 是否继续出块。

一个 mining thread 的目标是保护同机 Bitcoin 和索引服务，不是保证固定出块时间。PoW
`GenesisDifficulty` 和 `MinimumDifficulty` 必须根据目标硬件的可重放校准报告冻结，不能根据节点本地
启动基准动态改变。当前候选值在上线前仍需完成 clean hardware calibration。

## 8. 推荐结论

首轮共享测试网采用 `A miner + B miner + C full`。完成基本验收后，再安排短时三矿工压力窗口；只有在
需要跨地域、独立 upstream 或公共 RPC 观察节点时，再扩展为 `3 miner + 2 full`。
