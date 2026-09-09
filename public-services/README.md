# USDB Explorer 已迁出

浏览器、公共 RPC gateway、部署工具及新版本 CI/Release 已迁到独立工程
[USDB Explorer](https://github.com/buckyos/usdb-explorer)。本地 checkout 位于相邻的 `usdb-explorer`。

新工程使用 `vX.Y.Z` tag 和 `usdb-explorer` 命令，并保留 `usdb-public` 兼容入口、原配置 schema、
`~/.config/usdb-public`、`~/.local/share/usdb-public`、deployment ID 和数据库卷身份。
节点仍独立维护，通过 RPC 为浏览器提供服务。

原仓库的 `usdb-public-v0.1.0` tag、草稿及附件继续保留。过渡期间可从当前 `master` 运行
`usdb-public-publish.yml`，只允许选择 `usdb-public-v0.1.0`；该入口使用迁移前固定的
`0b946e03c71f70e6dfe0bb1bc44d9e6b3ce803ec` publisher 实现，检查原资产后发布，不再构建新版本。
原有 release URL 不重写、不转移。

网络资料由 USDB 导出，接口与刷新说明见
[Explorer 网络与 RPC 契约](../doc/interfaces/explorer-rpc-v1.md)。
