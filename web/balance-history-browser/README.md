# balance-history-browser

Balance History Explorer 的 React/Vite 版本。

## 功能

- 查看 balance-history RPC 的网络、同步高度、同步阶段和延迟。
- 查询单个 `USDBScriptHash` 的余额历史，并展示余额趋势和 delta 分布。
- 批量查询多个 `USDBScriptHash`，汇总最新余额和区间净变化。
- 支持 English / 简体中文，并通过 `lang` URL 参数和 localStorage 记忆语言。

## 本地开发

```bash
npm ci
npm run dev
```

默认 Vite dev server 端口是 `5175`，并会把 `/api` 代理到：

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
web/balance-history-browser/dist
```

`usdb-control-plane` 默认会把该目录托管到：

```text
/explorers/balance-history/
```

在 control-plane 内打开时默认使用同源 RPC 代理：

```text
/api/services/balance-history/rpc
```

也可以通过 URL 指定 RPC：

```text
/explorers/balance-history/?rpc_url=/api/services/balance-history/rpc
```
