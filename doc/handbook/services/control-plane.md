# 私有节点控制台

[返回手册首页](../README.md)

Control-plane 随 USDB 节点发布，供节点运维人员查看本机同步、服务健康和挖矿状态。
默认入口是节点本机 `127.0.0.1:28040`，通过 SSH 隧道访问。公开区块查询仍使用
[USDB Explorer](explorer.md)，不要把私有控制台接到 Explorer 的公网反向代理。

**本页的令牌登录、独立监控进程和提前启动属于当前源码改进，尚未包含在 r30 中。**
需要同时升级 node kit 和 `usdb-services` 镜像；仅升级网页或 CLI 不能获得完整功能。
当前源码还提供“钱包与身份”只读页面；正式网络的钱包签名、矿工证铸造和交易操作尚未完成验收。

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

## 钱包与身份

登录后打开顶部“钱包与身份”，选择 **USDB 账户** 或 **BTC 矿工证身份**。
也可以使用本机入口 `http://127.0.0.1:28040/#/me/usdb` 和 `/#/me/btc`。
钱包扩展安装在访问控制台的电脑浏览器中，不安装在节点服务器上。

| 身份 | 来源与用途 |
| --- | --- |
| USDB 钱包账户 | 从选定的 EVM 钱包读取；通过本节点查询 USDB 余额，按 18 位小数精确显示 |
| BTC 矿工证身份 | 从 UniSat / OKX Bitcoin 扩展读取；按 BTC 持有人地址查询本节点索引中的有效矿工证及其关联 USDB 地址 |
| 只读地址 | 手动输入后查询；不表示已连接钱包，也不证明拥有该地址 |
| 节点配置矿工地址 | 从节点配置只读展示；不是当前浏览器账户，也不是运行时 coinbase 或已经挖矿的证明 |

在“浏览器钱包”中选择扩展，点击“连接钱包”，在扩展里确认授权。页面不会自动弹出连接请求，
也不会要求输入私钥。连接只读取账户和网络，不签名、不发送交易，也不建立密码学所有权证明。
切换账户、钱包网络或身份页会清除旧结果；页面刷新、退出登录后需要重新连接。
“断开本页连接”清除当前页面状态；需要撤销站点授权时，在钱包扩展中执行。

USDB 账户必须同时匹配本节点配置的 **Chain ID 和 genesis**。本机 Chain RPC 的身份也必须匹配，
才展示余额。钱包 Chain ID 不同时可点击“请求钱包切换至本节点 Chain ID”，由钱包确认。
如果钱包未添加网络，使用该网络运营方提供的浏览器可达 RPC 手动添加；控制台不会把节点内部
Docker RPC 地址自动写入钱包。相同 Chain ID、不同 genesis 仍然是网络不匹配。

BTC 网络单独校验。**当前 USDB testnet-v0 使用 Bitcoin mainnet，BTC 钱包应选择 Bitcoin 主网**，
不能因为 USDB 名称中有 testnet 就选择 Bitcoin testnet。地址会检查格式、校验和及网络；
Bitcoin testnet / signet 等共享地址格式时，以钱包报告的链为准，不靠地址前缀猜测。
钱包不能报告网络时，页面显示“网络身份尚未确认”，可改用只读地址查询本节点数据。

本节点监控缺失或过期时暂停查询，先恢复 `usdb-node console monitor`。
BTC 查询还要求 indexer 的查询能力就绪、网络匹配。数据约每 15 秒刷新，可能落后于网络；
“未查到有效矿工证”只表示当前本机索引高度的查询结果。查询超时、RPC 故障和索引未就绪会单独提示，
不会显示为余额零或没有矿工证。矿工证查询也不等于当前已满足全部挖矿条件。

开发模拟器的 signer、PSBT 和铸造工具移到独立的“开发工具”入口，只有后端显式开启开发能力时显示。
开发 WIF 仅在页面会话内存中保留，离开开发页、退出登录或刷新后需重新导入；旧版 localStorage
中的开发 WIF 记录会在加载控制台时移除。正式节点保持开发能力关闭。

## 排错

| 现象 | 检查与处理 |
| --- | --- |
| 网页无法连接 | 先确认 SSH 隧道、本机端口，再执行 `usdb-node console start`；查看 `usdb-node logs` 中 control-plane 的启动错误 |
| `Host is not allowed` / `Cross-origin` | 使用隧道的实际本机地址，同一地址完成登录和后续访问；不要通过任意域名或修改 Origin 绕过检查 |
| HTTP 401 / 登录失效 | 重新运行 `usdb-node console token` 并登录；控制台重启会清除会话 |
| 监控进程未启用 / 数据过期 | `systemctl status usdb-console-monitor-usdb-testnet-v0.service`；用 `journalctl -u usdb-console-monitor-usdb-testnet-v0.service -n 100 --no-pager` 查看采集错误 |
| 状态文件无效 / 采集不可用 | 执行 `usdb-node status --progress-json` 对照，检查配置目录权限、Docker 和本机 RPC；不要删除节点数据来“恢复进度” |
| 网页可用，但某个 RPC 不可达 | 查看对应服务日志和 `usdb-node doctor`；监控页面可以在上游未启动时正常运行 |
| 未检测到钱包 | 在当前浏览器启用扩展并刷新；可先使用只读地址查询。BTC 当前适配 UniSat 和具备账户读取／事件接口的 OKX Bitcoin |
| 钱包拒绝授权／响应超时 | 解锁扩展、处理已有弹窗，再点击连接；拒绝授权不会切换到其他身份 |
| 网络不匹配 | 对照页面中的节点 Chain ID、genesis 或 Bitcoin 网络；手动修正钱包 RPC。Chain ID 相同仍可能连接了另一条链 |
| 本节点查询不可用／失败 | 先看节点监控、Chain / Indexer 服务状态；恢复服务后点击刷新查询。不需要重新导入钱包或清理数据 |

上面的 unit 名使用 `usdb-testnet-v0` bundle；其他网络替换为相应 bundle 名。
状态文件和令牌位于当前 `node.env` 同级的 `console/` 目录，权限仅供节点用户读取，
容器以只读目录方式挂载；Web 服务不挂载 Docker socket，也不读取完整 `node.env`。

普通运维优先使用 SSH 隧道。确需已有私有 HTTPS 入口时，管理员需同时配置
`CONTROL_PLANE_BIND_ADDRESS`、`CONTROL_PLANE_BIND_PORT` 和 `CONTROL_PLANE_ALLOWED_ORIGINS`。
后者使用 JSON/TOML 数组，例如 `["https://console.internal.example"]`，不能包含路径；
反向代理保留原始 Host/Origin。更改后运行 `usdb-node console start` 应用容器配置。
这不会自动配置 TLS，也不会使控制台适合公开访问。
