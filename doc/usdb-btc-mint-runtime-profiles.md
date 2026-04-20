# USDB BTC Mint Runtime Profiles

## 1. 背景

随着 `usdb-control-plane` 和统一控制台逐步成为本地与正式部署的统一入口，BTC 侧矿工证
(`pass`) mint 的正式运行模式需要单独收口。

当前已经明确的前提是：

- `world-sim` 是开发 / 演示环境
- `world-sim` 中大量依赖 `ord-server + ord wallet`
- 正式系统不应把这种“内置 agent / 托管钱包”模型直接照搬到用户场景

因此需要区分：

1. 开发/模拟所需的 `ord wallet` 便利能力
2. 正式系统向用户提供的 mint / inscription 能力

## 2. 结论

正式系统建议提供两种运行档位：

### 2.1 `full` 档位：带 `ord`

当前推荐的部署落点是：

- `compose.base.yml`
- 可选 `compose.dev-sim.yml` / `compose.joiner.yml`
- `compose.ord.yml`

此档位包含：

- `bitcoind`
- `ord-server`
- `balance-history`
- `usdb-indexer`
- `usdb-control-plane`

能力：

- 官方网页内直接发起矿工证 mint
- `control-plane` 为前端提供 mint 协调能力
- 对 inscription 相关状态提供更完整的查询和追踪能力
- 更适合：
  - 官方控制台
  - 演示环境
  - 面向最终用户的完整节点分发

### 2.2 `slim` 档位：不带 `ord`

当前推荐的部署落点是：

- `compose.base.yml`
- 可选 `compose.dev-sim.yml` / `compose.joiner.yml`
- 不叠加 `compose.ord.yml`

此档位包含：

- `bitcoind`
- `balance-history`
- `usdb-indexer`
- `usdb-control-plane`

不包含：

- `ord-server`
- 官方网页内直接发起 inscription mint 的能力

能力边界：

- 保留协议状态查询、链状态、基础聚合控制台能力
- 用户仍可使用第三方支持 inscription 的 BTC 钱包自行发起 mint
- 节点本身只负责：
  - 索引
  - 查询
  - 协议状态解释

适合：

- 精简部署
- 降低运行资源开销
- 不需要官方网页内直接 mint 的场景

## 3. 为什么正式系统仍建议保留一个“带 ord”的版本

因为如果要在官方网页里直接提供矿工证 mint，系统仍然需要一层 inscription 能力。

当前最现实、工程风险最低的方式，是继续依赖 `ord`：

- 由 `ord` 提供 inscription 相关协议构造与索引语义
- `control-plane` 提供统一的网页接口和状态协调
- 用户钱包负责签名和确认广播

这比立即完全自研一套 inscription 构造器更稳妥。

## 4. 私钥管理边界

这里需要非常明确：

- 正式系统 **不应该** 使用 `bitcoind wallet` 托管真实用户私钥
- 正式系统 **不应该** 使用 `ord wallet` 托管真实用户私钥
- `control-plane` 也 **不应该** 直接持有用户私钥

这三者只应管理：

- 系统自身的服务状态
- 开发环境的测试钱包
- `world-sim` 的内部 agent identity

而不应管理最终用户钱包。

## 5. 正式 mint 的推荐路径

正式网页内的 mint 能力建议定义成：

### 5.1 官方网页 mint 路径

1. 用户在网页内连接 BTC 钱包
2. `control-plane` 根据协议生成标准 mint 请求
3. 后端提供：
   - inscription payload
   - 交易参数 / 上下文
   - mint 状态跟踪
4. 用户钱包签名
5. 用户确认广播
6. `ord-server` / `usdb-indexer` 跟踪链上结果

这里的关键点是：

- 后端负责“构造和协调”
- 钱包负责“签名和确认”

### 5.2 第三方钱包直发路径

协议也应允许用户完全不经过官方网页。

也就是说：

- 用户可以自行构造符合矿工证协议的 inscription 内容
- 使用支持 inscription 的 BTC 钱包独立发起 inscribe
- 只要最终上链内容符合协议，`usdb-indexer` 就应正确识别和索引

这保证了协议不会被官方前端绑定。

## 6. `ord` 在正式系统里的推荐角色

正式系统中，`ord` 更适合作为：

- inscription/indexing 能力层
- 官方网页 mint 的辅助构造与追踪层

而不适合作为：

- 用户钱包托管层
- 用户私钥管理层

换句话说：

- `ord` 仍可以保留
- 但 `ord wallet custody` 不应成为正式产品形态

## 7. 关于“是否自己实现 mint”

长期来看，可以评估自研最小 mint builder。

但当前阶段不建议立即替代 `ord`，原因是：

- inscription 交易构造本身复杂
- 需要处理 fee / postage / UTXO / change / 广播 / 恢复
- 自研会显著增加正式系统风险

因此更推荐的阶段性路线是：

### 阶段 1

- 保留 `ord`
- 去掉正式用户的 `ord wallet custody`
- 先实现“浏览器钱包签名 + 官方 mint flow”

### 阶段 2

- 再评估是否把 mint builder 最小化自研
- 如果自研成熟，再考虑进一步弱化 `ord` 依赖

## 8. 钱包类型边界

还需要明确：

- `MetaMask` 只适用于 ETH / ETHW / EVM 合约交互
- BTC 矿工证 mint 需要的是 BTC 钱包能力，而不是 MetaMask

因此控制台后续的“钱包接入”应分成两条：

### 8.1 ETH / ETHW 钱包

- 用于：
  - ETHW / SourceDAO 合约操作
  - 地址身份展示

### 8.2 BTC 钱包

- 用于：
  - 矿工证 mint
  - inscription 相关签名
  - BTC 地址身份展示

## 9. 对控制台的影响

后续控制台建议基于这两种运行档位暴露不同能力：

### 9.1 `full` 档位控制台

- 显示 `ord-server` 状态
- 提供官方 mint 入口
- 提供 mint 任务状态追踪

### 9.2 `slim` 档位控制台

- 不显示官方 mint 按钮
- 只显示协议查询和节点状态
- 可提示用户：
  - 该节点未启用官方 mint 服务
  - 可使用外部 inscription 钱包完成 mint

## 10. 推荐发布策略

推荐在后续发布中明确两套 profile：

1. `usdb-full`
   - 带 `compose.ord.yml`
   - 带 `ord`
   - 带官方网页 mint 能力

2. `usdb-slim`
   - 不带 `compose.ord.yml`
   - 不带 `ord`
   - 更轻量
   - 不提供网页内直接 mint

这样既保留正式产品能力，也能给资源敏感场景一个精简版本。

## 11. 当前建议

当前建议明确为：

- 正式系统可以同时支持 `full` 和 `slim`
- 官方网页直接 mint 的能力只放在 `full`
- 用户私钥始终留在浏览器钱包
- 协议本身继续允许第三方钱包独立发起 mint
- `world-sim` 中的 `ord wallet` 继续只作为开发期便利能力，不代表正式用户钱包模型
