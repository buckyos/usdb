# 私有节点控制台

[返回手册首页](../README.md)

Control-plane 随 USDB 节点发布，供节点运维人员查看本机同步、服务健康和挖矿状态。
默认入口是节点本机 `127.0.0.1:28040`，通过 SSH 隧道访问。公开区块查询仍使用
[USDB Explorer](explorer.md)，不要把私有控制台接到 Explorer 的公网反向代理。

**本页的令牌登录、独立监控进程和提前启动属于当前源码改进，尚未包含在 r30 中。**
需要同时升级 node kit 和 `usdb-services` 镜像；仅升级网页或 CLI 不能获得完整功能。
当前源码还提供“钱包与身份”只读页面；正式网络的钱包签名、矿工证铸造和交易操作尚未完成验收。

## 启用与访问

新安装使用 `usdb-node setup` 的默认 systemd 模式，会安装启动控制器和默认启用的[核心 node monitor](../node/monitor.md)。
`usdb-node up` 先准备 USDB 服务镜像并启动控制台，再继续 Bitcoin 镜像准备、快照和同步。
首次 USDB 镜像尚未下载完成时，网页暂不可用，使用 `usdb-node status --watch` 观察。

已有节点完成[正常升级](../node/maintenance.md)后，使用同一运维账号执行：

```bash
usdb-node up --no-watch
usdb-node console token
```

包含后台服务自动检查改进的 node kit 会补齐缺失的标准 monitor，并在升级后重启旧版采集进程。
核心节点已经 `READY` 时也会执行这些检查，无需额外记住一次 `controller install`。
如果工具提示 controller 从未安装，执行 `usdb-node controller install` 后再 `up`；
有意使用前台模式的节点仍保持前台方式。自定义 unit、覆盖配置或 mask 按提示核对，不自动覆盖。
尚未包含此改进的旧工具，仍使用 `controller install` 后 `console start` 的方式补齐监控。

`controller install` 安装或更新 controller 与核心 monitor 的 systemd unit，不会启动或重启链。首次 setup 可关闭监控，后续操作保留该选择；
`console start` 只启动／更新控制台容器及已安装的监控进程，不等待 BTC、BH、Indexer 或 Chain 就绪。
如果旧节点只有启动控制器而没有 console monitor，单独启动控制台容器不会自动安装采集服务；
命令会提示监控未安装，先补做 `controller install`，再执行 `console start`。安装时可能需要输入 sudo 密码。
它不会启动挖矿，也不清理数据。端口被占用、Docker 不可用或镜像尚不可获取时，需要先处理对应错误。
`console token` 会输出 `Console access token: <令牌>`，只复制冒号后的令牌到登录表单，
不要复制前一行的 `manifest_sha256`（它是安装包校验摘要）。不要将令牌放入 URL、工单或截图。
脚本需要无标签输出时，使用 `usdb-node console token --raw`；stdout 只有令牌，校验提示仍写入 stderr。

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
usdb-node monitor run
```

这个命令持续采集状态并保存本地事件，不控制节点启动。结束该进程后历史记录仍保留，当前观测不可用；
也可用 `usdb-node console export` 生成一次观测；核心 monitor 运行时一次性导出不会覆盖它的快照。需要断开终端后继续监控时，安装 systemd 控制器。
一次性导出只适合诊断：没有持续采集时，网页会在观测过期后再次显示状态未知。

### Ord 容器运行，但尚未启动 HTTP 服务

启用可选铸造后端后，`ord-server` 容器先运行监督进程。它等待 Bitcoin 前台同步、历史校验和
txindex 全部满足条件后，才启动 Ord 本体与 HTTP 服务。容器显示 `Up` 不能证明 Ord 已开始索引。
在控制台的“矿工证铸造依赖（可选）”中查看具体状态（旧版名称为“本机铸造后端”）：

| 显示 | 含义与处理 |
| --- | --- |
| 等待交易索引追平 | 对比交易索引高度与 Bitcoin 前台高度；等待完成后自动启动 Ord，无需反复重启 |
| Ord 索引／规范链校验中 | Ord 已启动，继续等待它自己的索引；txindex 完成不代表 Ord 索引也完成 |
| 监控未启用／当前状态未知 | 无法判定 Ord 当前是否正常；先恢复核心 monitor，不能当作 Ord 离线 |
| Ord 运行失败／磁盘余量不足 | 执行 `usdb-node minting-status --json` 并查看 `usdb-node logs ord-server`，按具体原因处理 |

正式节点的首页和 Ord 服务页使用相同的宿主机观测；等待依赖不会影响 USDB 节点同步、挖矿或已有矿工证查询。
后端就绪也不表示正式钱包签名与广播已经开放。

## 怎样阅读状态

首页和 Bootstrap 页直接使用与 `usdb-node status --progress-json` 相同的宿主机观测模型。
监控进程在每轮采集后间隔约 10 秒刷新，网页约每 8 秒读取一次；RPC 超时时单轮采集可能更长。

控制台按服务职责命名；服务页保留命令和日志使用的技术标识。包含术语改进的版本使用以下名称：

| 页面名称 | 技术标识与含义 |
| --- | --- |
| Bitcoin 最新区块同步 | `btc-node` 当前活动链的区块同步；与历史区块验证分别展示 |
| Bitcoin 历史区块验证 | AssumeUTXO 后台补齐并验证较早区块，不阻塞已满足条件的最新区块同步就绪 |
| Bitcoin 历史余额索引 | `balance-history`；按 Bitcoin 区块高度记录地址或脚本的余额与变动，并提供查询 |
| USDB 协议索引 | `usdb-indexer`；处理 USDB 协议数据，包括矿工证及其状态 |
| USDB 链节点 | `usdb-chain`；显示 USDB 链的区块高度与运行状态 |
| Bitcoin 脚本注册表 | 脚本与脚本哈希的映射；是否需要独立准备取决于节点配置 |
| Ord 铭文索引 | `ord-server`；可选的铭文索引服务，是本地矿工证铸造的依赖之一 |
| 私有节点控制台 | `usdb-control-plane`；本机运维入口 |

“已就绪”对应服务状态 `READY`；“构建并更新索引”对应处理阶段 `Indexing`。索引服务会持续处理新增区块，
所以这两项可以同时出现。中文界面显示“已同步 / 目标高度”或“区块高度”，不再把高度笼统标为 `blocks`。
未知的新状态保留原始代码供诊断，不会当作已就绪。

“观测已过期”对应 `STALE`，表示不能据此确认当前状态，不等于服务已停止。
过期组件保留上次阶段和数值，并用中性色进度条标记历史记录；即使显示 100%，也不代表当前就绪。

| 信息 | 含义与边界 |
| --- | --- |
| 网络、Chain ID、Genesis、版本与角色 | 来自本机发布配置，可在 Chain RPC 尚未就绪时显示；不是网络同步证明 |
| 快照文件完成 | 下载／SHA-256 校验完成是独立里程碑，不会因等待区块头或进入导入阶段而归零 |
| Bitcoin 最新区块同步 | 展示当前活动链的高度与状态 |
| Bitcoin 历史区块验证 | 独立显示高度和完成情况，不阻塞已满足条件的前台启动 |
| Bitcoin 历史余额索引 / USDB 协议索引 | 使用各自的导入、回放和就绪观测，不借用 Bitcoin 进度代替 |
| 资源阶段与容器内存上限 | 显示配置和资源交接状态；内存上限是预算，不是实时 RSS |
| 挖矿状态 | 与节点角色、同步和 controller 状态分别显示；full 节点不挖矿是正常情况 |
| 私有节点控制台 | 独立显示容器健康，控制台故障不会被写成 Chain 共识故障 |
| 服务 RPC 探测 | 各服务并发探测、缓存约 5 秒；显示这一批探测的实际采集时间 |

“采集正常”表示监控数据新鲜，节点仍可能处于 `SYNCING`、`FAILED` 或 `BLOCKED`。
`READY` 不代表矿工已经产块，也不代表钱包操作已经验收。节点没有安装 Ord 时，Ord 不可用本身不等于节点同步失败。

超过 120 秒未得到新的宿主机观测，页面显示“数据已过期／当前状态未知”。最后一次观测仅供参考。
监控缺失、采集失败、文件格式无效会分别显示，不会以旧的 READY 代替当前状态。
控制器完成启动后，独立监控进程仍继续运行；`usdb-node down` 会停止两个宿主机进程和节点服务。
如果页面能打开而这里一直未知，先检查采集进程；浏览器刷新只能重读快照，不能启动缺失的 monitor。
旧工具升级后可能只有启动控制器，没有持续采集服务；新版后台 `up` 会检查并修复可识别的标准部署。

## 主机资源与数据磁盘

总览的“节点监控”展示主机 CPU、内存、Swap，当前节点容器的 CPU / 内存占用，以及关键服务数据目录和文件系统余量。
这些是带采集时间的采样值；浏览器刷新读取最新观测，不会直接执行宿主机命令。
使用新功能需要一起升级 node kit 和控制台镜像。按正常升级流程执行后台 `up`，使控制台和采集进程使用新版代码。
新版工具通过 monitor 的版本标记识别升级；同版本且健康的进程不会因重复 `up` 被重启。
使用旧工具时，如只更新工具而采集进程一直运行，可执行
`sudo systemctl restart usdb-node-monitor-usdb-testnet-v0.service`；其他网络替换对应 bundle 名。
旧工具尚未安装该服务时，先执行 `usdb-node controller install` 和 `usdb-node console start`。

| 指标 | 口径 |
| --- | --- |
| 主机 CPU | 两次计数器采样之间的平均占用，按全核归一化为 0–100%；首次采样约 150 ms，后续为实际监控间隔 |
| 主机内存 | 已用 = `MemTotal - MemAvailable`，可用内存包括内核估计可回收的缓存；与各容器工作集之和不相等 |
| 容器 CPU / 内存 | 仅当前节点的两个 Compose 项目；CPU 的 100% 代表一个逻辑核，可超过 100%。内存是 Docker 报告的约数和实际运行上限，与配置预算分开 |
| 数据目录 | Bitcoin、BH、Indexer、Chain、Control-plane、已启用的 Ord 和已配置的 UTXO 快照文件目录；显示宿主机路径及已分配磁盘空间 |
| 文件系统余量 | 同一文件系统只列一次，包含同盘其他应用的占用；目录可能共享、重叠，不可直接相加推算磁盘占用 |

CPU、内存、文件系统余量随监控周期采集；目录遍历每 5 分钟触发一次，在后台低优先级执行，
不会阻塞后续节点状态心跳。目录统计不跨文件系统，单次扫描有超时限制。
目录受容器用户权限保护时，采集器校验正在运行的服务及其 bind mount 与配置路径一致后，
才尝试在该容器内执行限时只读统计，使用容器本身的用户。它不改变目录权限，不读取文件内容。

容量低于 **10% 或 100 GiB** 时提醒，低于 **5% 或 50 GiB** 时标红。这只是运维提醒，
不会停止服务或替代 Ord 自身的磁盘保留阈值。容量不足时安排扩容或按手册清理无关文件。

“正在统计”表示首次后台扫描尚未完成；“目录读取权限不足”“统计超时”“无法采集”都不等于零占用。
失败时保留上次成功统计值和原时间，超过 10 分钟的目录统计会明确标记为历史值。
宿主机快照过期时，整个资源面板显示历史观测；容器采样失败不会抹去节点同步状态。
`usdb-node console export` 可诊断一次完整采集，但不替代持续运行的 monitor。

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

本节点监控缺失或过期时暂停查询，先恢复 `usdb-node monitor run`。
BTC 查询还要求 indexer 的查询能力就绪、网络匹配。数据约每 15 秒刷新，可能落后于网络；
“未查到有效矿工证”只表示当前本机索引高度的查询结果。查询超时、RPC 故障和索引未就绪会单独提示，
不会显示为余额零或没有矿工证。矿工证查询也不等于当前已满足全部挖矿条件。

开发模拟器的 signer、PSBT 和铸造工具移到独立的“开发工具”入口，只有后端显式开启开发能力时显示。
开发 WIF 仅在页面会话内存中保留，离开开发页、退出登录或刷新后需重新导入；旧版 localStorage
中的开发 WIF 记录会在加载控制台时移除。正式节点保持开发能力关闭。

## 可选的本机铸造后端

此选项属于 r30 之后的源码改进，需要同时更新 node kit 和 `usdb-services` 镜像。
新节点在 `usdb-node setup` 中回答 `Enable local minting backend (txindex + private Ord)`，默认 `n`。
选择 `y` 后直接使用下文的推荐内存、缓存和磁盘保护余量，不再逐项询问 Ord 数字参数；编辑已有配置时保留先前的自定义值。
新建 Ord 索引前要求额外 **300 GiB 可用空间**；全新节点首次同时启用时，叠加基础部署容量，门槛为 **1.5 TiB + 300 GiB**。
非交互配置使用 `usdb-node configure --minting on`，并提供该节点所需的其他参数。

开启后从首次 Bitcoin 启动起配置 `BTC_TXINDEX=1`。一个轻量监督进程先等待依赖；
只有 Bitcoin 前台追平、历史链已完整校验、txindex 覆盖观测到的前台高度后，才启动 Ord 索引进程。
前台追平时不会为了启用 txindex 自动再重启 Bitcoin。**BH、Indexer、Chain 的启动条件不依赖 Ord**，
AssumeUTXO 的前台启动仍可早于历史校验完成。关闭本机铸造后端也不影响已有矿工证查询和正常挖矿。

已有节点需由运维人员安排一次正常停机来改变配置：

```bash
usdb-node down
usdb-node set-minting --enabled on
usdb-node up
usdb-node minting-status
usdb-node status --watch
```

`set-minting` 拒绝修改正在运行的节点，自动重算各阶段内存预算；已有自动模式保留当前资源阶段，不会因补建 txindex 回退到 Bitcoin 独占预算。
不会删除 Bitcoin、txindex 或 Ord 数据。
已有未剪枝 Bitcoin 数据可用于补建 txindex，无需重新导入 UTXO 快照或执行全量 `-reindex`。
关闭时执行同样的 `down → set-minting --enabled off → up`，在 AssumeUTXO 部署中将 txindex 设为 0、停止运行 Ord，但保留索引文件供以后复用。
旧式非 AssumeUTXO 部署仍保留其原有的 `txindex=1` 就绪要求，关闭 Ord 不会移除这一历史依赖。
支持编辑的新版 node kit 也可在停机后运行 `usdb-node setup`，在本机铸造后端问题中选择 `y`；
其他选项直接回车会保留当前配置。保存后执行 `doctor → up`，无需 `prepare --replace`。
`set-minting` 继续作为独立入口，详见[编辑已有配置](../node/maintenance.md#编辑已有配置)。

本阶段完成的是索引后端和能力检查。**正式钱包签名、PSBT 交易构建、广播和铸造仍未开放**。
`backend_ready=true` 只表示本机依赖满足；`transactions_enabled` 保持 `false`。未来交易流程还需重新校验
钱包网络、地址权限、UTXO 资产和费用。无需导入私钥，也不为此选项开启 Bitcoin Core 钱包。

### 资源与网络

- Ord 默认容器上限 **4 GiB 内存、2 个 CPU 核当量**，索引缓存 **1 GiB**，不借用额外 swap。
  自动资源策略在 Bitcoin、overlap、steady 三阶段都预留这笔内存，避免索引就绪后突然超配。
  即使 Ord 正在等待，也保留预算；txindex 位于 Bitcoin 进程内，会增加磁盘与 I/O 开销，不是独立容器。
- 新版 Ord 0.29.0 数据存放在 `USDB_DATA_ROOT/datasets/ord/btc-mainnet/ord-0.29.0`，包含版本、数据库格式与索引配置标识。
  不接管已有未标识的非空目录。页面显示数据库文件大小和文件系统可用空间，不通过遍历全盘计算容量。
- 新建索引的容量门槛为 **300 GiB 空闲**。`setup`、`configure --minting on`、`set-minting --enabled on`
  和需要新索引的版本激活都会检查；容量不足时保留原配置和数据，先扩容或保持 Ord 关闭。
  仅有目录、身份标识或空索引文件仍视为新建；复用已存在且有匹配身份标识的索引时不重复要求另一份完整预算。
- 默认保留 **50 GiB 可用磁盘**。不足时暂停 Ord，保留数据；恢复容量后自动继续。
  这只是运行保护阈值，**不是整个 Ord 或 txindex 索引的容量估计**，也无法保证 Bitcoin 等其他服务停止写盘。
  300 GiB 的启用预算已包含保护和增长余量，不再叠加 50 GiB；Bitcoin `txindex` 和其他服务占用需另外计算。
  容量检查不会实际锁定或分配磁盘。初次完整索引需要额外磁盘和时间，应按实际增长扩容；尚未提供主网全量索引的容量或耗时保证。
- 本版本索引铭文和地址，不额外保存整套交易副本、不启用全量 sat 或 Rune 索引。
  Ord 仅在 Docker 私网监听，**不发布主机端口或公网入口**；控制台只消费经过筛选的只读状态。

确需调优时，在 `down` 后修改私有 `node.env` 中的 `ORD_MEMORY_LIMIT`、`ORD_INDEX_CACHE_BYTES`
和 `ORD_MIN_FREE_BYTES`。内存使用 Docker 整数字节或 `g/m` 单位，后两个参数使用正整数字节；
内存至少 2 GiB，缓存不得超过容器内存一半。自动模式随后执行
`usdb-node set-resource-policy --mode auto`，用 `usdb-node resources` 检查所有阶段，再 `up`。
增大 Ord 预算会压缩其他服务预算；工具拒绝总量超出主机容量的配置。

### Ord 版本升级

Ord 随 `usdb-services` 镜像发布，从官方固定 tag/commit 编译，使用仓库维护的依赖锁文件；
无需另外安装宿主机 Ord 或下载第三方 Ord 镜像。

本次从 0.23.3 升级到 0.29.0，Ord 数据库格式从 30 变为 34，**已有 Ord 索引需要单独重建**。
正常停机并安装新版 node kit 后，执行 `usdb-node activate-release`，工具会备份原配置为
`node.env.ord-upgrade-backup`，将 Ord 配置切换到新的版本目录；旧目录及其索引保留。
已开启 Ord 时，激活新索引前检查目标磁盘至少还有 **300 GiB 空闲**；旧索引的占用不计入这笔可用空间。
检查失败保留原配置、旧索引和已有备份。
然后执行 `doctor → up → minting-status`。新版从已有 Bitcoin 数据补建自己的索引，Bitcoin、
BH 和 USDB 数据无需因此重建；尚未开启 Ord 的节点不会创建索引或启动后台索引任务。

默认仍为 4 GiB 内存、2 核上限和 1 GiB 索引缓存。升级的主要成本是 Ord 重新索引所需的时间、I/O，
以及旧新索引并存期间的磁盘空间；具体主网耗时和峰值尚未测量。保留原 Ord 数据，待新版本稳定后再按实际容量安排清理。
需要回退时，先停机，恢复升级前 release/node kit 和对应的 `node.env.ord-upgrade-backup` 配置，
核对旧 Ord 目录后再启动；不要让旧二进制直接打开新索引。

### 进度与处理方式

首页、“钱包与身份 → BTC 矿工证身份”和 Ord 服务页分别展示可选后端状态。
CLI 可使用 `usdb-node minting-status --json`，`usdb-node status --progress-json` 的 `minting` 字段使用同一份观测。
Bitcoin 前台、历史校验、txindex、Ord 高度分别显示；Ord 还显示落后区块、磁盘余量与数据库文件大小。

| 状态 | 含义与操作 |
| --- | --- |
| `DISABLED` | 未选择本机 Ord；属于正常配置 |
| `WAITING_CORE` | Bitcoin 尚未追平、链尖过旧或连接不足；看 Bitcoin 同步及连接状态 |
| `WAITING_HISTORY` | 等待历史链完整校验；不要重启或重复导入快照来“加速” |
| `WAITING_TXINDEX` | 等待交易索引覆盖前台高度；不能只看 `getindexinfo.synced=true`。索引始终不存在时核对 Core 是否采用 `BTC_TXINDEX=1` |
| `STARTING` / `INDEXING` | Ord 正在启动、补建索引或处理重组；查看高度与差距 |
| `READY` | txindex 已覆盖采样高度，Ord 包含对应区块且哈希与 Core 规范链一致；不是正式交易功能已开放 |
| `BLOCKED_DISK` | 增加容量或清理无关文件，保留节点及索引数据 |
| `BLOCKED_CONFIG` | 需要未剪枝的 Bitcoin mainnet 节点；检查配置 |
| `UNAVAILABLE` / `FAILED` | 观测失败、过期或 Ord 退出；执行 `usdb-node logs ord-server`，同时检查 Bitcoin 日志、内存和磁盘 |
| `STOPPED` | 监督进程已停止；需要运行已配置服务时执行 `up` |

监督进程每轮完成后间隔约 10 秒检查。Ord 观测超过 60 秒即失效，网页不会借用新鲜的控制台心跳
维持旧的 READY。RPC 超时期间能力关闭；依赖再次满足后可恢复。Ord 故障单独展示，不改变节点的整体共识就绪状态。
Ord 启动后遇到短暂 RPC 故障或 txindex 落后时会继续运行，只撤销就绪能力，避免反复中断长时间索引；
低磁盘或不兼容的 Bitcoin 配置会触发停止，恢复条件后再启动。

## 排错

| 现象 | 检查与处理 |
| --- | --- |
| 网页无法连接 | 先确认 SSH 隧道、本机端口，再执行 `usdb-node console start`；查看 `usdb-node logs` 中 control-plane 的启动错误 |
| `Host is not allowed` / `Cross-origin` | 使用隧道的实际本机地址，同一地址完成登录和后续访问；不要通过任意域名或修改 Origin 绕过检查 |
| HTTP 401 / 登录失效 | 重新运行 `usdb-node console token` 并登录；控制台重启会清除会话 |
| 监控进程未启用 / 数据过期 | `systemctl status usdb-node-monitor-usdb-testnet-v0.service`；用 `journalctl -u usdb-node-monitor-usdb-testnet-v0.service -n 100 --no-pager` 查看采集错误 |
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

## 通知配置

“节点监控 → 通知配置与投递”支持编辑 Webhook/SMTP 渠道、通知级别和频率，并发送测试通知。
本地文件和页面共用同一份配置，配置有效时 monitor 在线应用；没有 ack 或人工消警操作。
控制台仅获得专用通知配置目录的写权限，事件库和通知队列仍由宿主机 monitor 管理。
需配套更新 node-kit 与服务镜像，详细参数、投递结果和排障见[通知配置与投递](../node/notifications.md)。
