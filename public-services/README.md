# USDB Explorer 已迁出

浏览器、公共 RPC gateway、部署工具及新版本 CI/Release 已迁到独立工程
[USDB Explorer](https://github.com/buckyos/usdb-explorer)。本地 checkout 位于相邻的 `usdb-explorer`。

新工程使用 `vX.Y.Z` tag 和 `usdb-explorer` 命令，并保留 `usdb-public` 兼容入口、原配置 schema、
`~/.config/usdb-public`、`~/.local/share/usdb-public`、deployment ID 和数据库卷身份。
节点仍独立维护，通过 RPC 为浏览器提供服务。

原仓库的 `usdb-public-v0.1.0` tag、未发布草稿及附件保留作为历史记录，不再安排发布。
旧 Public build/Publish workflow 已移除；后续版本只在 Explorer 仓库构建和发布。
草稿附件仍不能匿名下载，旧安装 URL 的 404 不会因为移除 workflow 而改变。
历史源码和 workflow 可通过迁移前的 `0b946e03c71f70e6dfe0bb1bc44d9e6b3ce803ec` 追溯。

网络资料由 USDB 导出，接口与刷新说明见
[Explorer 网络与 RPC 契约](../doc/interfaces/explorer-rpc-v1.md)。
