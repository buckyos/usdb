# 浏览器与公共 RPC

[返回 USDB 手册首页](../README.md)

USDB Explorer 已独立为 [buckyos/usdb-explorer](https://github.com/buckyos/usdb-explorer)，负责浏览器、
公共 RPC 网关、PostgreSQL 和可选 Nginx。它使用独立的 `vX.Y.Z` 版本，不随节点 `rN` 自动安装或升级。

**完整用户与运维指南由 [Explorer handbook](https://github.com/buckyos/usdb-explorer/blob/main/docs/handbook/README.md)
维护。** 本页保留 USDB 节点侧的准备要求和阅读入口，避免两套安装、升级和排错命令随版本分叉。
Explorer 手册会区分已发布接口和尚未发布的改进；本页不改变节点手册中既有 r25 的验证范围。

## 选择使用方式

| 目标 | 操作 |
| --- | --- |
| 普通使用者查询区块/交易 | 向网络运维方取得实际浏览器 URL，无需安装节点 |
| 专用查询服务器 | 使用 full 节点，开启 archive 和私有 tracing，再单独安装 Explorer |
| 首节点尚未 mining | 支持 genesis 启动的 Explorer 版本可先运行，等待后续区块和交易 |
| 现有矿机同机运行 Explorer | 先安排资源预算并核对历史覆盖；不要默认修改 mining 角色 |

## 节点侧准备

配套 node kit 和 chain 镜像需支持查询模式。新节点 `usdb-node setup` 中的
`Provide full Explorer support (archive + private tracing)` 选择 `y`，会开启 archive 和私有 tracing；
默认 `n` 适合不承担完整浏览器查询的普通节点。

已有节点先查看 `usdb-node query-mode`。需要改配置时按
[查询模式指南](../../publish/usdb-node-query-mode.md)在维护窗口操作；运行中不能直接切换。
archive 不会自动恢复已裁剪的历史。保留原目录，安排独立重放或完整归档恢复，不能以重建/清空现有数据作为默认步骤。

专用 full 查询节点无需自行挖矿；普通加入节点需要正常同步。首节点尚未开始出块时，零 peer、
genesis 高度和空交易列表可以是预期状态，但无法因此证明历史和真实交易追踪已完成验收。
相反，RPC 超时、身份不符或未开放 tracing 都不是“还未 mining”的正常表现。

同机部署还需为 Explorer 预留内存和磁盘。默认 Explorer 容器预算为 6 GiB，节点查询模式指南
说明如何使用通用资源预算为外部服务留出容量；不要只比较当前 RSS。

## Nginx 与访问入口

默认 `bundled` 模式仍内置 Nginx，统一入口通常为宿主机 `28080`。另有 `external` 模式，用于接入
管理员已有的 Nginx；并非拆仓后移除了 Nginx。
这里描述的是 `ingress.mode`；`rpc.mode=local-node` 则表示连接同机节点，二者独立组合。
公布 URL、宿主机监听端口、域名和 HTTPS 的具体配置由下方 Explorer 手册统一维护。

局域网访问填写实际服务器局域网 IP 和端口；公网访问填写实际公网域名/IP 和外部端口，并核对
转发与防火墙。`192.0.2.10` 是示例地址，不能作为实际入口。不要直接公开节点 `8545/8546`、
debug namespace 或数据库端口。

## 对应章节

- [使用浏览器](https://github.com/buckyos/usdb-explorer/blob/main/docs/handbook/using-explorer.md)
- [安装、同机部署与访问地址](https://github.com/buckyos/usdb-explorer/blob/main/docs/handbook/installation.md)
- [RPC/入口模式、端口映射、域名和 HTTPS](https://github.com/buckyos/usdb-explorer/blob/main/docs/handbook/networking.md)
- [已有证书启用内置 Nginx HTTPS、入口验证与续期](https://github.com/buckyos/usdb-explorer/blob/main/docs/handbook/https.md)
- [preflight/check 结果、升级与备份](https://github.com/buckyos/usdb-explorer/blob/main/docs/handbook/operations.md)
- [超时、旧采样、archive/tracing 和页面排错](https://github.com/buckyos/usdb-explorer/blob/main/docs/handbook/troubleshooting.md)

`usdb-node down` 管理节点，`usdb-explorer down` 管理 Explorer；两者不能互相替代。
Explorer 的 preflight 成功只说明上游检查允许启动，仍需在服务启动后通过 check 和实际访问者网络确认入口。
