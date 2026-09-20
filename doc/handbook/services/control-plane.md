# 私有节点控制台

[返回手册首页](../README.md)

Control-plane 随 USDB 节点发布，供节点运维人员查看本机同步、服务健康和挖矿状态。
默认入口是节点本机 `127.0.0.1:28040`，通过 SSH 隧道访问。公开区块查询仍使用
[USDB Explorer](explorer.md)，不要把私有控制台接到 Explorer 的公网反向代理。

**本页的令牌登录、独立监控进程和提前启动属于当前源码改进，尚未包含在 r30 中。**
需要同时升级 node kit 和 `usdb-services` 镜像；仅升级网页或 CLI 不能获得完整功能。
本阶段提供监控底座，正式网络的钱包签名、矿工证铸造和交易操作尚未完成验收。

## 启用与访问

新安装使用 `usdb-node setup` 的默认 systemd 模式，会安装启动控制器和独立的 console monitor。
`usdb-node up` 先准备 USDB 服务镜像并启动控制台，再继续 Bitcoin 镜像准备、快照和同步。
首次 USDB 镜像尚未下载完成时，网页暂不可用，使用 `usdb-node status --watch` 观察。

已有节点完成[正常升级](../node/maintenance.md)后，使用同一运维账号执行：

```bash
usdb-node controller install
usdb-node console start
usdb-node console token
```

`controller install` 安装或更新两个 systemd unit，不会启动或重启链；
`console start` 只启动／更新控制台容器及已安装的监控进程，不等待 BTC、BH、Indexer 或 Chain 就绪。
它不会启动挖矿，也不清理数据。端口被占用、Docker 不可用或镜像尚不可获取时，需要先处理对应错误。
`console token` 会在当前终端显示私有令牌，只复制到登录表单，不放入 URL、工单或截图。

在自己的电脑上打开 SSH 隧道（将 `NODE_IP` 换成节点地址）：

```bash
ssh -N -L 28040:127.0.0.1:28040 usdb@NODE_IP
```

浏览器访问 `http://127.0.0.1:28040/`，输入令牌。如果 SSH 使用其他端口，在命令中增加 `-p`。
本地端口冲突时，可改为 `-L 38040:127.0.0.1:28040`，浏览器对应访问本机 `38040`。
无需开放服务器的公网 `28040`。登录会话最多持续 12 小时；退出登录或控制台重启后需重新登录。
访问令牌不保存在浏览器 localStorage 中。

使用 `setup --no-controller`，或使用前台启动而未启动 systemd 监控进程时，另开一个终端运行：

```bash
usdb-node console monitor
```

这个命令只持续采集状态，不控制节点启动。结束该进程后网页会显示数据过期；
也可用 `usdb-node console export` 生成一次观测。需要断开终端后继续监控时，安装 systemd 控制器。

## 怎样阅读状态

首页和 Bootstrap 页直接使用与 `usdb-node status --progress-json` 相同的宿主机观测模型。
监控进程在每轮采集后间隔约 10 秒刷新，网页约每 8 秒读取一次；RPC 超时时单轮采集可能更长。

| 信息 | 含义与边界 |
| --- | --- |
| 网络、Chain ID、Genesis、版本与角色 | 来自本机发布配置，可在 Chain RPC 尚未就绪时显示；不是网络同步证明 |
| 快照文件完成 | 下载／SHA-256 校验完成是独立里程碑，不会因等待区块头或进入导入阶段而归零 |
| Bitcoin 前台同步 | 展示当前前台链的高度与状态 |
| Bitcoin 后台历史校验 | 独立显示高度和完成情况，不阻塞已满足条件的前台启动 |
| Balance history / USDB indexer | 使用各自的导入、回放和就绪观测，不借用 Bitcoin 进度代替 |
| 资源阶段与容器内存上限 | 显示配置和资源交接状态；内存上限是预算，不是实时 RSS |
| 挖矿状态 | 与节点角色、同步和 controller 状态分别显示；full 节点不挖矿是正常情况 |
| 私有控制台 | 独立显示容器健康，控制台故障不会被写成 Chain 共识故障 |
| 服务 RPC 探测 | 各服务并发探测、缓存约 5 秒；显示这一批探测的实际采集时间 |

“采集正常”表示监控数据新鲜，节点仍可能处于 `SYNCING`、`FAILED` 或 `BLOCKED`。
`READY` 不代表矿工已经产块，也不代表钱包操作已经验收。节点没有安装 Ord 时，Ord 不可用本身不等于节点同步失败。

超过 120 秒未得到新的宿主机观测，页面显示“数据已过期／当前状态未知”。最后一次观测仅供参考。
监控缺失、采集失败、文件格式无效会分别显示，不会以旧的 READY 代替当前状态。
控制器完成启动后，独立监控进程仍继续运行；`usdb-node down` 会停止两个宿主机进程和节点服务。

## 排错

| 现象 | 检查与处理 |
| --- | --- |
| 网页无法连接 | 先确认 SSH 隧道、本机端口，再执行 `usdb-node console start`；查看 `usdb-node logs` 中 control-plane 的启动错误 |
| `Host is not allowed` / `Cross-origin` | 使用隧道的实际本机地址，同一地址完成登录和后续访问；不要通过任意域名或修改 Origin 绕过检查 |
| HTTP 401 / 登录失效 | 重新运行 `usdb-node console token` 并登录；控制台重启会清除会话 |
| 监控进程未启用 / 数据过期 | `systemctl status usdb-console-monitor-usdb-testnet-v0.service`；用 `journalctl -u usdb-console-monitor-usdb-testnet-v0.service -n 100 --no-pager` 查看采集错误 |
| 状态文件无效 / 采集不可用 | 执行 `usdb-node status --progress-json` 对照，检查配置目录权限、Docker 和本机 RPC；不要删除节点数据来“恢复进度” |
| 网页可用，但某个 RPC 不可达 | 查看对应服务日志和 `usdb-node doctor`；监控页面可以在上游未启动时正常运行 |

上面的 unit 名使用 `usdb-testnet-v0` bundle；其他网络替换为相应 bundle 名。
状态文件和令牌位于当前 `node.env` 同级的 `console/` 目录，权限仅供节点用户读取，
容器以只读目录方式挂载；Web 服务不挂载 Docker socket，也不读取完整 `node.env`。

普通运维优先使用 SSH 隧道。确需已有私有 HTTPS 入口时，管理员需同时配置
`CONTROL_PLANE_BIND_ADDRESS`、`CONTROL_PLANE_BIND_PORT` 和 `CONTROL_PLANE_ALLOWED_ORIGINS`。
后者使用 JSON/TOML 数组，例如 `["https://console.internal.example"]`，不能包含路径；
反向代理保留原始 Host/Origin。更改后运行 `usdb-node console start` 应用容器配置。
这不会自动配置 TLS，也不会使控制台适合公开访问。
