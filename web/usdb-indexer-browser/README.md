# usdb-indexer-browser

USDB Indexer Browser 的 React/Vite 版本。

## 功能

- 首页：自动刷新系统状态（时间、网络、本地同步高度、上游稳定高度、矿工证统计、活跃地址总余额）
- 矿工证：按 `inscription_id` 查询快照，并分页查看历史事件
- 能量：查看排行、查询单个矿工证当前状态/当前能量、按时间线倒序分页查看历史能量记录

## 运行依赖

- 已启动 `usdb-indexer` RPC 服务（默认 `http://127.0.0.1:28020`）
- 或通过 `usdb-control-plane` 同源代理访问：

```text
/api/services/usdb-indexer/rpc
```

注意：不要填 `bitcoind` RPC 端口（例如 `28032` 或 `28132`），浏览器会被 CORS 策略拦截。

## 本地开发

在当前目录执行：

```bash
npm ci
npm run dev
```

默认 Vite dev server 端口是 `5176`，并会把 `/api` 代理到：

```text
http://127.0.0.1:28140
```

可通过环境变量覆盖：

```bash
USDB_CONTROL_PLANE_TARGET=http://127.0.0.1:28140 npm run dev
```

## 构建与托管

```bash
npm run check
npm run build
```

构建产物输出到：

```text
web/usdb-indexer-browser/dist
```

`usdb-control-plane` 默认会把该目录托管到：

```text
/explorers/usdb-indexer/
```

在 control-plane 内打开时默认使用同源 RPC 代理：

```text
/api/services/usdb-indexer/rpc
```

## 已使用的 RPC 方法

- `get_rpc_info`
- `get_sync_status`
- `get_pass_stats_at_height`
- `get_latest_active_balance_snapshot`
- `get_pass_snapshot`
- `get_pass_history`
- `get_pass_energy_leaderboard`
- `get_pass_energy`
- `get_pass_energy_range`

## 说明

- 页面使用 React/Vite 构建，运行时由 `dist` 静态产物提供。
- 顶部 RPC 区域支持按网络预设（Mainnet/Regtest/Testnet/Signet/Testnet4）自动填充默认 endpoint。
- 首页默认每 5 秒刷新一次，可点击“刷新”立即拉取最新状态。
- 页面支持 English / 简体中文，并通过 `lang` URL 参数和 localStorage 记忆语言。
- 完整同步状态模型说明见：[../../doc/usdb-indexer/usdb-indexer-sync-status-model.md](../../doc/usdb-indexer/usdb-indexer-sync-status-model.md)。
- 首页中的“同步高度”表示 usdb-indexer 本地 durable 已提交高度，“稳定高度”表示 balance-history 当前稳定高度，也是 usdb-indexer 的同步 ceiling。
- 首页中的“当前进度 / 进度上限”对应 RPC 的 current / total，只用于进度展示，不应当解读成新的高度语义字段。
- 能量排行榜点击某一行会自动联动到“当前能量状态”查询。
- 能量详情查询默认使用最新同步高度，并自动加载 `[mint_block_height, current_height]` 区间记录（分页倒序展示）。
