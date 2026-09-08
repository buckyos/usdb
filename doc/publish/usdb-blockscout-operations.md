# USDB Blockscout 与公共 RPC 运维入口

浏览器与公共 RPC 已改为独立的 `usdb-public` 发布包，不再作为 `usdb-node` 的子命令或 node-kit 的组成部分。

完整的中文配置、Nginx、HTTPS、验收、独立升级及备份说明见：

- [独立部署操作手册](../../public-services/README.md)
- [配置样例](../../public-services/config.example.json)
- [公共测试网整体推进方案](usdb-public-testnet-services-plan.md)

独立机器只需准备合格的上游历史查询/tracing RPC，不需要先安装或启停 USDB 节点。
现有服务器可选择 external 模式对接已有 Nginx；专用服务器可选择 bundled 模式。
之前未发布的 `usdb-node public-services` 预览草案不再作为操作流程。

当前第三方镜像仍为私有兼容性预览，完整 archive 与公网验收尚未完成，具体边界见操作手册。
