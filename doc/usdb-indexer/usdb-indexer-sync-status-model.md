# usdb-indexer 同步状态模型说明

本文档用于完整解释 `usdb-indexer` 的 `get_sync_status` 语义边界，统一 RPC、CLI、浏览器和回归脚本对同步状态的理解。

## 1. 背景

`usdb-indexer` 并不是直接追逐 BTC tip，而是消费 `balance-history` 已经对外承诺的稳定快照。因此，`get_sync_status` 里至少有两类不同语义的高度：

- 本地已经 durable 提交的高度。
- 上游 `balance-history` 当前暴露出来、允许 `usdb-indexer` 追赶的稳定高度。

历史上字段名 `latest_depend_synced_block_height` 容易把上游稳定高度误读成“依赖服务当前同步到了哪里”。Phase 1/2 的收敛目标就是把这一层语义改成更直接的 `balance_history_stable_height`，并明确 `current` / `total` 只是进度展示值。

## 2. 字段语义

`get_sync_status` 的核心字段如下：

| 字段 | 类型 | 语义 | 应如何消费 |
| --- | --- | --- | --- |
| `genesis_block_height` | `u32` | 协议索引起点高度 | 只作为协议范围基线展示或校验 |
| `synced_block_height` | `Option<u32>` | `usdb-indexer` 本地 durable 已提交高度 | 任何需要判断“本地已经真正提交到哪里”的逻辑，都应使用它 |
| `balance_history_stable_height` | `Option<u32>` | `balance-history` 当前稳定高度，也是 `usdb-indexer` 的同步 ceiling | 任何需要判断“上游允许追到哪里”的逻辑，都应使用它 |
| `current` | `u32` | 当前进度位置 | 仅用于进度条、watch 输出、页面展示 |
| `total` | `u32` | 当前进度上限 | 仅用于进度条、watch 输出、页面展示 |
| `message` | `Option<String>` | 人类可读状态文本 | 仅展示，不参与语义判断 |

重点：

- `synced_block_height` 不等价于 `current`。
- `balance_history_stable_height` 不等价于 `total`。
- `current` / `total` 可以参与 UI 进度展示，但不应升级为新的“状态高度字段”。

## 3. 三种高度职责

### 3.1 本地 durable 高度：`synced_block_height`

这是 `usdb-indexer` 已经写入本地存储并完成提交后的高度。对外如果要回答“本机已经真正落盘到哪一块”，只能看它。

适用场景：

- CLI 判断本地是否已经追上某个目标高度。
- 浏览器默认查询“最新同步高度”相关的历史区间。
- 回归脚本断言 `usdb-indexer` 至少已经 durable 到某个高度。

### 3.2 上游稳定高度：`balance_history_stable_height`

这是 `balance-history` 当前对外承诺的稳定高度。它代表 `usdb-indexer` 的同步 ceiling，而不是 `balance-history` 的所有内部进度细节。

适用场景：

- 判断 `usdb-indexer` 是否仍然落后于上游稳定快照。
- 对外解释“为什么本地还没继续前进”，因为上游稳定高度还没推进。
- 在 CLI / 浏览器首页展示上游稳定状态。

### 3.3 进度展示高度：`current` / `total`

这两个字段只解决“当前进度条怎么画”这个问题。

它们适用的场景只有：

- CLI `watch_sync_status` 的进度条长度与当前位置。
- 浏览器首页同步进度条、`当前进度 / 进度上限` 文案。

它们不适用的场景：

- 不能代替 `synced_block_height` 判断 durable 提交完成。
- 不能代替 `balance_history_stable_height` 判断上游 ceiling。
- 不能被脚本或下游协议解释成新的业务高度语义。

## 4. 推荐不变量

在正常同步过程中，应按下面的关系理解状态：

- 当 `synced_block_height` 和 `balance_history_stable_height` 都存在时，通常有 `synced_block_height <= balance_history_stable_height`。
- 当本地完全追平上游稳定高度时，可以达到 `synced_block_height == balance_history_stable_height`。
- CLI 当前的完成判定采用：
  - `synced_block_height >= balance_history_stable_height`
  - 且 `current >= total`

注意：

- 启动早期或上游快照尚未拿到时，`synced_block_height`、`balance_history_stable_height` 允许为 `null`。
- `current` / `total` 即使数值上看起来接近某个高度，也不应该替代上面两个可空字段。

## 5. 典型阶段

### 5.1 启动期

服务刚启动、快照还未刷新完成时，状态可能类似：

```json
{
  "genesis_block_height": 900000,
  "synced_block_height": null,
  "balance_history_stable_height": null,
  "current": 0,
  "total": 0,
  "message": "Starting"
}
```

此时只能说明服务正在初始化，不能从 `current` / `total` 推断 durable 高度。

### 5.2 追赶期

```json
{
  "genesis_block_height": 900000,
  "synced_block_height": 900123,
  "balance_history_stable_height": 900130,
  "current": 900123,
  "total": 900130,
  "message": "Syncing block 900124"
}
```

这里的正确解释是：

- 本地 durable 已提交到 `900123`。
- 上游稳定 ceiling 是 `900130`。
- 页面/CLI 可以把 `900123 / 900130` 画成进度条，但语义判断仍应分别读取前两个字段。

### 5.3 追平期

```json
{
  "genesis_block_height": 900000,
  "synced_block_height": 900130,
  "balance_history_stable_height": 900130,
  "current": 900130,
  "total": 900130,
  "message": "Idle"
}
```

此时可解释为：本地 durable 高度已经追平上游稳定高度。

## 6. 各消费端应该怎么用

### 6.1 CLI

CLI watch 模式应：

- 展示 `synced=<synced_block_height>`。
- 展示 `stable=<balance_history_stable_height>`。
- 用 `current / total` 驱动进度条。
- 不再把上游字段称为 depend height。

### 6.2 浏览器

浏览器首页应：

- 把 `synced_block_height` 展示为“同步高度”。
- 把 `balance_history_stable_height` 展示为“稳定高度”。
- 把 `current / total` 展示为“当前进度 / 进度上限”。
- 明示 `current / total` 仅用于进度展示。

### 6.3 回归脚本 / 自动化

回归脚本判断服务是否“到位”时应至少区分：

- `synced_block_height` 是否达到预期。
- `balance_history_stable_height` 是否达到预期。

不要把 `total` 直接当成上游稳定高度，也不要把 `current` 直接当成 durable 高度。

## 7. 兼容策略

服务端对外的规范字段已经是 `balance_history_stable_height`。

为了兼容旧响应格式，CLI 反序列化层暂时保留：

- `#[serde(alias = "latest_depend_synced_block_height")]`

这只是过渡兼容，不代表旧字段仍然是推荐语义。新增消费者、文档、示例和断言都应只使用 `balance_history_stable_height`。

## 8. 关联文档

- RPC 规范：[usdb-indexer-rpc-v1.md](./usdb-indexer-rpc-v1.md)
- 浏览器说明：[../../web/usdb-indexer-browser/README.md](../../web/usdb-indexer-browser/README.md)
- Regtest E2E：[usdb-indexer-regtest-e2e-smoke.md](./usdb-indexer-regtest-e2e-smoke.md)
