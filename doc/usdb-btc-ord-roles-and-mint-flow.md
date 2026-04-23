# USDB BTC `ord` 角色划分与 Mint 调用流备忘

## 1. 背景

最近在 `world-sim` 和控制台 `Mint` 流程联调时，一个容易混淆的问题是：

- `world-sim` 里已经有 `ord` binary
- 也已经有独立的 `ord-server`
- 当前又在 `usdb-control-plane` 里增加了 `ord`

表面上看像是“有三套 ord”，但实际不是。

当前本地开发栈里使用的是**同一个 `ord` 程序**，只是按不同容器职责和不同运行模式拆开：

- `ord-server` 负责长期索引和 HTTP 查询
- `world-sim` 负责自动模拟 agent 行为
- `usdb-control-plane` 负责响应控制台里一次用户触发的 `mint execute`

这份文档的目的，是把这几个 `ord` 的区别、联系和当前 `Mint` 调用流固定下来，后续遇到问题可以直接对照。

## 2. 一句话结论

当前并不存在三套独立的 `ord` 系统，而是：

- **同一个 `ord` binary**
- **同一条 BTC regtest 链**
- **同一份 `ord-data` 索引数据目录**

在不同容器里分别承担：

1. `server` 模式
2. `wallet` CLI 模式
3. 控制台触发的执行入口

## 3. 角色总览

| 位置 | 运行方式 | 主要职责 | 是否长驻 | 是否负责索引 | 是否负责执行 mint |
| --- | --- | --- | --- | --- | --- |
| `ord-server` | `ord ... server` | 跟踪 BTC 链并提供 ord HTTP 服务 | 是 | 是 | 否 |
| `world-sim-bootstrap` / `world-sim-runner` | `ord wallet ...` | 自动创建/驱动 deterministic agent 钱包与协议动作 | 否，按脚本执行 | 否，依赖 `ord-server` | 是，自动模拟用 |
| `usdb-control-plane` | `ord wallet ... inscribe` | 响应控制台内一次用户触发的 mint execute | 否，按请求执行 | 否，依赖 `ord-server` | 是，面向网页流程 |

## 4. 三处 `ord` 分别是什么

### 4.1 `ord-server`

定义位置：

- [docker/compose.ord.yml](/home/bucky/work/usdb/docker/compose.ord.yml)
- [docker/scripts/start_ord_server.sh](/home/bucky/work/usdb/docker/scripts/start_ord_server.sh)

它的启动方式是：

```bash
ord \
  --regtest \
  --bitcoin-rpc-url ... \
  --bitcoin-data-dir ... \
  --data-dir /data/ord \
  --index-addresses \
  --index-transactions \
  server \
  --http \
  --http-port ...
```

它的职责很明确：

- 跟踪 BTC 链
- 维护 ord 索引
- 暴露 HTTP 查询接口
- 被 `balance-history`、`usdb-indexer`、`world-sim`、控制平面等组件间接依赖

它更像“BTC inscription/indexing 查询层”，而不是用户交互入口。

### 4.2 `world-sim-bootstrap` / `world-sim-runner`

定义位置：

- [docker/compose.world-sim.yml](/home/bucky/work/usdb/docker/compose.world-sim.yml)
- [docker/scripts/start_world_sim.sh](/home/bucky/work/usdb/docker/scripts/start_world_sim.sh)

`world-sim` 本身并不启动第二个 `ord server`。  
它主要是通过脚本反复调用：

```bash
ord wallet \
  --no-sync \
  --server-url http://ord-server:28130 \
  --name <wallet_name> \
  ...
```

它做的事情包括：

- 恢复 deterministic ord wallet identity
- 为 agent 生成接收地址
- 模拟 mint / transfer / remint / send / spend 等动作
- 在 loop 模式下持续产生活动，给 `balance-history` 和 `usdb-indexer` 提供真实负载

所以 `world-sim` 里的 `ord` 本质上是“自动化 agent 的 CLI 执行器”。

### 4.3 `usdb-control-plane` 里的 `ord`

当前新增位置：

- [docker/compose.world-sim.yml](/home/bucky/work/usdb/docker/compose.world-sim.yml)
- [src/btc/usdb-control-plane/src/server.rs](/home/bucky/work/usdb/src/btc/usdb-control-plane/src/server.rs)

控制平面里新增的 `ord` 不是用来再起一个索引服务，而是用于在 `development/world-sim` runtime 下执行：

```bash
ord wallet \
  --no-sync \
  --server-url <ord-server-url> \
  --name <wallet_name> \
  inscribe \
  --fee-rate ... \
  --destination <owner_address> \
  --file <payload.json>
```

它承担的职责是：

- 接收控制台发起的 `mint execute`
- 使用当前 `wallet_name` 和 `owner_address`
- 调用 `ord wallet inscribe`
- 构造并广播 inscription 交易
- 把 `inscription_id`、`txid` 和执行输出回传给前端

因此，`usdb-control-plane` 里的 `ord` 更像“网页驱动的执行器”，不是新的索引层。

## 5. 它们之间的联系

### 5.1 同一个 `ord` binary

无论是在：

- `ord-server`
- `world-sim-bootstrap`
- `world-sim-runner`
- `usdb-control-plane`

本质上使用的都是 `ord` 程序，只是启动参数不同。

### 5.2 同一份 `ord-data`

当前 world-sim / development 栈会把 `ord-data` volume 挂给多个容器：

- `ord-server`
- `world-sim-bootstrap`
- `world-sim-runner`
- `usdb-control-plane`

这意味着它们共享的是同一个 ord 数据目录，而不是各自维护不同的链视图。

### 5.3 同一个 BTC 节点

这些组件最终都指向同一份 `bitcoind` / `regtest` RPC：

- `ord-server` 从它同步索引
- `world-sim` 用它确认交易、产块和驱动模拟
- `usdb-control-plane` 用它执行开发态 mint

### 5.4 索引来源仍然是 `ord-server`

即使 `usdb-control-plane` 内部调用了 `ord wallet inscribe`，它自己也**不是**新的索引服务。

真正维护 ord 索引、暴露 HTTP 查询的，仍然是独立的 `ord-server`。

## 6. 为什么控制台不能只靠 `ord-server`

因为 `ord-server` 解决的是：

- 跟链
- 索引
- 查询

而网页内的 `mint execute` 还需要：

- 选择具体 `wallet_name`
- 生成 inscription payload 文件
- 调用 `ord wallet inscribe`
- 处理执行结果和错误

这些都属于“主动执行”逻辑，不属于 `ord-server` 的职责。

所以控制平面里额外带一个 `ord`，不是重复建设，而是为了给控制台提供一个受控的执行入口。

## 7. 当前 Mint 调用流

下面是当前 `development/world-sim` 路径下的推荐理解方式。

### 7.1 Prepare 阶段

控制台页面：

- 用户选择 BTC 身份
- 输入 `ETH main` / 可选 `ETH collab`
- 点击 `Continue Mint`

控制平面：

- 调用 `POST /api/btc/mint/prepare`
- 校验 owner 地址、运行网络、runtime profile
- 查询 `balance-history` / `usdb-indexer` / active pass / readiness
- 生成：
  - `eligible`
  - `blockers`
  - `warnings`
  - `inscription_payload_json`
  - `prepare_request`

这个阶段的职责是：

- 校验
- 规范化
- 给出 draft
- 明确是否允许继续

### 7.2 Confirm / Sign 阶段

控制台页面：

- 进入 `Review`
- 展示本次 mint 的确认信息
- development runtime 下，使用 `Dev Signer` 对当前 payload 做本地确认签名

这里的签名目前主要承担：

- 让开发链流程更接近正式链的“确认后签名”
- 验证当前 world-sim 身份和 signer 材料匹配

它不是最终链上识别依据，链上真正识别的仍然是 inscription 交易本身。

### 7.3 Execute 阶段

控制平面收到 execute 请求后，会：

1. 再次收口 `prepare` 逻辑
2. 确认当前是 `development` runtime
3. 确认 `wallet_name` 存在于 world-sim bootstrap marker
4. 确认该 wallet 对应的 `owner_address` 与本次 mint 请求一致
5. 把 payload 写到运行时文件
6. 调 `ord wallet inscribe`

这里真正完成的是：

- inscription 交易构造
- 广播
- 获取 `inscription_id` / `txid`

### 7.4 Wait / Recognize 阶段

控制台拿到 execute 结果后，不会立即假设“协议已经完成”。

而是继续等待：

1. `ord-server` 跟上链
2. `usdb-indexer` 识别这条 inscription
3. `get_pass_snapshot` / `get_owner_active_pass_at_height` 能查到新的 pass 状态

只有走到这一步，控制台才会把流程推进到成功页。

## 8. 当前调用流图

### 8.1 Development / World-Sim

```text
MePage
  -> POST /api/btc/mint/prepare
     -> usdb-control-plane
        -> balance-history / usdb-indexer / ord readiness
        -> return draft + blockers + warnings
  -> Dev Signer confirm
  -> POST /api/btc/mint/execute
     -> usdb-control-plane
        -> ord wallet inscribe --server-url <ord-server>
        -> bitcoind mempool / block
     -> return inscription_id + txid
  -> poll pass snapshot / active pass
     -> usdb-indexer recognizes the inscription
  -> success
```

### 8.2 Public Runtime 的目标形态

正式链上的理想流程与上面类似，但会把签名与确认替换成浏览器 BTC 钱包：

```text
MePage
  -> prepare mint
  -> review
  -> browser wallet confirm + sign
  -> broadcast
  -> ord / usdb-indexer recognize
  -> success
```

两者应该尽量保持同一套产品步骤，只是底层执行器不同：

- development: `Dev Signer + world-sim wallet`
- public: `Browser Wallet`

## 9. 边界和注意事项

### 9.1 `usdb-control-plane` 中的 `ord` 不是第二个 `ord-server`

它不负责：

- 长驻索引
- 对外提供 ord HTTP API
- 独立维护一套链状态

它只在请求到来时临时执行 `ord wallet` 命令。

### 9.2 当前 execute 路径是 development-only

当前这条链路的定位是：

- world-sim
- regtest
- 内部开发 / 演示

不应直接外推成正式用户私钥托管方案。

### 9.3 正式链的目标不是把用户私钥放进 `ord wallet`

正式系统更合理的边界仍然是：

- 后端负责构造和协调
- 用户钱包负责确认和签名
- 链上结果再由 `ord-server` 和 `usdb-indexer` 识别

## 10. 推荐排查顺序

当后续再遇到“Mint 不通”或“为什么又有一个 ord”这类问题时，建议按这个顺序检查：

1. `ord-server` 是否正常运行并跟上 BTC 链
2. `world-sim` 的目标 `wallet_name` 是否存在且地址匹配
3. `usdb-control-plane` 是否能访问同一份 `ord-data`
4. `mint/prepare` 是否已经给出可执行的 `eligible=true`
5. `mint/execute` 是否成功调用 `ord wallet inscribe`
6. `usdb-indexer` 是否已经识别对应 inscription 并推进 active pass

## 11. 推荐一起阅读的文档

- [USDB BTC Mint Runtime Profiles](./usdb-btc-mint-runtime-profiles.md)
- [USDB Docker 部署说明](../docker/README.md)
- [dev-sim-world-sim-plan.md](./dev-sim-world-sim-plan.md)
