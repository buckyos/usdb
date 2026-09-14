# P7.4：复用一台测试节点的正式部署验收方案

日期：2026-09-13。发布输入已提交为 `8bd2bbd`。本文是执行前评估，不是清理或重部署记录。

## 1. 目标与现场确认程度

采用用户指定的第一台测试节点，先归档旧状态的比较证据，再在同机按正式发布安装流程串行重建。
不要求同时存放两套完整 Bitcoin/BH/indexer 数据。开发机现有 P4/P5/P6 证据可复用，不重新生成旧 BH 大型快照。

**当前确定范围：全清测试节点后重新安装，只归档 BH RocksDB 用于后续核对。**
用户已确认旧节点没有恢复需求，也不保留旧账户、密钥或 P2P 身份；矿工 address 在新 `setup` 时重新输入。
需要保留原节点数据/身份的场景应走单独的升级流程，不纳入这次全清重装。
第4.2—4.4节为已精简的脚本和当前续跑命令；同盘归档完成后可以安排重装，不再等待小型导出工具。
本文不代表已执行远程移动或删除；旧 BH 在归档验证完成前仍需保留。

本次通过 `ssh usdb@192.168.1.119` 只读核实：主机名 `bucky04`，操作员属于 `sudo/docker` 组，
launcher 指向 `usdb-testnet-v0-r17`，数据根 `/data/.usdb`。配置仍为 `SNAPSHOT_MODE=balance-history`、
`BH_SCRIPT_REGISTRY_ENABLED=1`、自动资源阶段 `steady`。controller unit 为
`usdb-node-bootstrap-usdb-testnet-v0.service`，本次观察是 `inactive/dead`；这不代表 Docker 服务已经停止。

SSH 间歇失去响应；最初的查询稍后返回 Docker 状态，其中旧 BH 容器为 healthy，旧 Bitcoin 容器为
`Restarting (1)`。这不是本轮操作引起的停服/重启，本轮没有执行任何服务变更。
随后只读 inspect 返回 `exit=1, oom=false, restarts=1410`；日志在 `2026-09-13T19:06:06Z` 报告
`LevelDB read failure: Corruption: block checksum mismatch: /data/bitcoin/chainstate/331207.ldb`，
随后退出。容器内文件对应宿主机 BTC 根下的 `chainstate/331207.ldb`。
这是旧 Core chainstate 校验失败的直接证据，尚不能判断是磁盘/I/O、异常掉电还是其他原因；未执行 reindex 或修复。
BH 目录统计只返回部分子目录，未取得完整大小。用户报告 BH 已追平，本轮尚未用 RPC 独立确认其最新高度。
healthy 不能替代固定高度的共识就绪/锚点检查，不得把未返回的统计当成零。
已返回的完整配置路径和磁盘 JSON 保存在开发机 `/tmp/usdb-node1-readonly-inventory.json`，
BTC 占用结果在 `/tmp/usdb-node1-readonly-sizes.jsonl`；这些临时文件应随验收材料归档。
Core 故障观察在 `/tmp/usdb-node1-bitcoin-observation.txt`。
所有配置输出使用字段白名单，未输出 RPC 凭据。本次没有停止服务、修改配置、导出数据库或清理数据。

连接稳定后补齐以下清单中尚未取得的项目：

| 清单 | 需确认的事实 |
| --- | --- |
| 发布和服务 | 已安装 release/bundle、image digest、Core/BH/indexer/chain 版本、容器和 controller 状态 |
| 身份和路径 | 数据根与实际挂载、各服务目录及 marker、B/G/hash、网络/chain ID、activation registry 和算法版本 |
| 空间 | 数据根文件系统总容量/可用空间，Core blocks/indexes/chainstate、BH、旧下载包、indexer、chain 分项占用 |
| 可比状态 | Core canonical 高度/hash，BH/indexer 稳定高度与历史保留下界，当前 chain 引用的 BTC 锚点 |
| 新安装参数 | 矿工 address 在 setup 时重新输入；不采集或备份旧身份材料 |

只读清单不输出凭据内容。大目录统计需单独计时，避免把尚未完成的统计误记为零占用。

## 2. 正式安装前的交付缺口

节点端使用既有 `installer -> usdb-node setup -> doctor -> up -> status`。
P7.3 的临时 bundle 用于发布端准备和本地自动化验证，不是另一个节点部署系统。

当前执行前置项：

1. **生成真实原生 release。** 正式 bundle 接入已完成：源码 `release-bootstrap.json` 固定公开材料，
   candidate/publish 从同一 tag 重建并验证原生 bundle。下一步按既有发布流程更新三仓 revision lock、
   构建新镜像、生成 candidate、批准 publish；不能沿用旧镜像或旧 manifest。
2. **定义同机旧配置重置。** `setup` 会拒绝已有 `node.env`；`activate-release` 只接受相同数据兼容契约。
   旧 BH 与原生 BH 的契约/目录不同，不能仅替换镜像、手改 compatibility ID 或 dataset marker。
   本次清理旧 controller、配置、RPC 凭据和服务目录，由新 setup 重新生成；不要求先做恢复备份。
3. **补齐只读比较采集。** 现有 `check_assumeutxo_p65_mainnet.py` 面向已封存 G 的原生服务；
   其 `capture_anchor` 要求 BH/indexer 当前高度恰好等于目标高度。
   它不能直接充当仍在追块的旧节点的任意历史高度导出器。需补充固定历史锚点采集与比较，或安排受控的固定高度验收阶段。
4. **自有签名 UTXO 已发布并登记。** 记录 SHA-256 为
   `77f1e50991528b053ad16c5abafafe09a305a62516804832747072ee5a3e1680`，
   来源为 `usdb-snapshot.tbudr.top`，继续复用原 `snapshot-keys`；新节点仍独立完整验签/验文件。
   不要求重新生成私钥或签发 Core 二进制。

这是 P7.4 对正式发布、迁移操作和验证工具的补齐，不新增临时部署流程。
在发布输入和安装包完成前，本次不启动主网重导入。

## 3. 清理前留存的比较基线

旧数据只用于 BH 一致性验证。至少保留业务起点 G 与一个较新的固定高度 H；G 用于复用既有主网全量投影证据，
H 用于核对原生导入后的余额与 commit。H 必须落在旧 BH 保留的历史区间，并固定对应 canonical BTC hash。

| 证据 | 留存内容及比较范围 |
| --- | --- |
| 比对元数据 | BH 版本、导出工具版本、网络、G/H 和 commit 协议版本；无需完整配置或所有旧 kit |
| BTC 锚点 | G/H 的 canonical block hash；采集前后核对 H 未被重组，必要时重采 |
| BH | G/H 的 block commit、可取得的历史 state-ref、固定地址/脚本的高度余额；归档样本键和请求参数 |
| 脚本反查样本 | 若验收需要，保存同一批 BH 样本的 scriptPubKey/address 映射；不保存完整 registry |

样本覆盖已有非零余额、之后归零、B 后首次出现脚本、实际铸造/转移矿工证的地址。
对 B 前已归零且之后未观察到的脚本，新 registry 不承诺反查；不能把这种覆盖差异记为余额错误。

Indexer、chain、control-plane 在新部署中验收就绪、连通及正常业务调用，不要求与旧节点做历史数据对照。
这些运行验收仍需完成，但不增加旧节点数据库、身份或配置的保留要求。

### 3.0 BH 核对范围与本次归档方式

本次先用第4节脚本同盘归档完整BH RocksDB，以支持重装后的固定核对及事后补查。无需保留整个BH服务根或其余服务备份。
旧身份材料也不保留，矿工address在setup时重新输入。后续从离线旧库读取下列参考数据；也可以导出小型参考包后释放旧库。

参考包至少保存：

1. 网络、G=963800、固定稳定高度 H、G/H 对应的 BTC block hash、commit 协议版本和导出工具版本。
2. 旧 BH 在 `[G,H]` 内已持久化的逐块 commit。若旧库缺少某段历史，明确记录范围，不把缺失当作匹配。
3. 固定 script_hash 清单及 G/H 的逻辑余额（satoshi），覆盖非零余额、后续归零、新出现脚本和实际矿工证地址。
   需要反查验收的少量脚本另导出 scriptPubKey/address 映射，不为这些样本保留完整 registry。
4. 导出完成状态、样本数量和参考包 SHA-256；保留请求高度/样本键，使新服务可以逐项复查。

新 BH 同步到至少 H 后，在相同 BTC 锚点和协议版本下比较这些记录。余额按指定查询高度的逻辑值比较；
bootstrap 前的最后变化高度、初始化 delta、registry 覆盖范围和运行进度不能混作同一项业务结果。
已有第3.1节 G 的全量投影结果继续作为原生导入的参考。样本匹配加 commit 匹配支持本次既定验收，
不等于重新扫描了 H 的全部数据库，也不支持删除旧库后任意新增历史查询。

当前旧服务已经停止时，优先使用 `BalanceHistoryDB::open_read_only` 从现有目录导出，
无需为了取样重启 Bitcoin 或搬动大库。现有离线工具提供 checkpoint/origin 检查，但**尚无上述参考包的一键导出/比对命令**；
小型导出/比对命令仍可后续补充；本次已经选择归档完整RocksDB，因此不以该命令实现为清理前置条件。

如果仍需要事后任意补查，才选择保存完整 `db/balance_history` RocksDB 目录，并保留对应读取工具和最小身份元数据。
“只取部分数据”应通过逻辑查询导出记录，不能从 RocksDB 随意挑几个 SST 当作可读数据库。

### 3.1 复用已有 G=963800 主网证据

[P4 全量投影结果](./balance-history-assumeutxo-p4-validation-2026-09-12.md)和
[P5 结果](./balance-history-assumeutxo-p5-validation-2026-09-12.md)已经记录：

| 字段 | 已归档参考值 |
| --- | --- |
| G hash | `000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551` |
| C(G) | `d5981b06db4e1f9d12f3a9e8b66da45001d0b38d3e2a689de3251542922f1e75` |
| 活 UTXO 行数 / 投影 SHA-256 | 165748439 / `86cba93334b2a6b6862a00d070617a0bdfded56b8eec2a25c41c2dcff82faedd` |
| 非零余额行数 / 投影 SHA-256 | 59356343 / `7e9e427332cf8bf95a52cd8689e2b17c732593b69a3f7f062c03dd93ab92449b` |

这些是此前实验的参考值，**不是本次从目标节点读取的结果**。
新原生入口在 G 封存时使用相同排序/编码的全量投影摘要复核，不必为了验收重新保留37GB旧 core 快照和 registry 快照。
归档原始报告、投影算法/schema 和代码 revision，不能仅抄表格后丢弃证据来源。

H 的样本和承诺可保存在小型 JSON/报告包内。若还要求 H 的独立全量状态核对，需要在旧数据库删除前完成一致读视图下的
规范投影扫描，保存总行数、顺序/编码和全量 SHA-256；总余额或几条样本不能替代全量核对。
小型参考包支持既定比较，不能恢复已删数据库，也不能满足事后任意补查。

比较材料计算文件哈希并复制到目标节点以外，验证副本可读后再进入重建安排。
本次不另行归档私有配置或身份密钥，也无需生成新的旧式 BH 数据库 snapshot。

## 4. 两种复用范围与磁盘约束

| 方案 | 验证覆盖 | 代价与限制 |
| --- | --- | --- |
| 同机清空已确认的旧大库，Core/BH/indexer 从新目录开始 | 完整冷启动：真实下载/`loadtxoutset`、Core 前后台、BH 原生导入及渐进追块、全套正式编排 | 需重新下载/验证旧 BTC 历史；同一时刻只保留一套大库。最符合本次 P7.4 冷启动目标 |
| 保留已有完整 Core 数据，重建 BH/indexer | 旧 Core 目录升级/复用，以及原生 BH 和下游接入 | 若 Core 已完全验证，observer 复用现有链状态，不执行快照导入；不能作为新 Core AssumeUTXO 冷启动验收 |

第一种方案作为冷启动验收首选；第二种可作为独立升级场景，具体是否先做取决于实际空间和安排。
不通过仅删除 chainstate 或手动拼接 blk/index 文件来制造快照导入条件。

当前 `setup/configure` 硬性要求数据根文件系统**总容量与当前可用空间都至少1.5TiB**，长期建议2TiB。
因此保留大体积 Core 后“只清 BH”未必能通过正式 setup；必须以现场可用字节复核，不在验收时临时绕过该检查。
运行峰值还要计入完整 blocks/undo、双 chainstate、原始约9.39GB UTXO、BH staging/live、indexer、chain、镜像和日志。
本次完整冷启动同时验证自有源下载，因此新 UTXO artifact 目录也应为空；后续运行时保留 `.dat` 供恢复使用。

### 4.1 node1 已测空间与备份约束

| 项目 | 实测值 |
| --- | --- |
| `/data` 文件系统 | `/dev/sda1`，ext4，2046609936384 bytes（1906.05 GiB） |
| `/data` 当前可用 | 502493724672 bytes（467.98 GiB） |
| 当前 BTC 整个目录 | 1021281103872 bytes（951.14 GiB） |
| BTC blocks / chainstate / indexes | 815.53 / 10.61 / 124.99 GiB（目录总量还包括日志等） |
| 根分区 | `/dev/mapper/pve-root`，总量约65 GiB、可用约42 GiB，不适合作为 BH 大库备份盘 |
| BH / 旧快照 / indexer / chain 分项 | 尚未完成统计，执行前补齐 |

仅删除 BTC 后，理论可用约1419.13 GiB，距离 setup 的1536 GiB仍差 **116.87 GiB**。
如果 BH 备份留在 `/data`，该盘所有保留数据合计须低于约 **370.05 GiB** 才能满足硬门槛，实际还应留出余量。
该上限不是 BH 已测大小，也不保证完整运行峰值足够；以停服归档和清理后的 `df` 实测为准。
无法满足时，将 BH 备份转移到独立磁盘/其他主机。把目录改名不会释放其占用。

### 4.2 当前脚本的归档和清理范围

[node_rebuild.py](../../docker/scripts/tools/node_rebuild.py) 已调整为 v2：只保留 BH RocksDB，其他旧数据逐项确认清理。
不要求恢复旧节点，也不备份旧身份。脚本仍是可独立复制的 Python 3/64位Linux 文件，进入现有 node-kit 打包链路。

| 对象 | 当前行为 |
| --- | --- |
| `BH_OLD/db/balance_history` | 完整归档 RocksDB，含全部列族和数据库文件；只处理这个目录 |
| BH 根下其他文件、日志、auxiliary registry、旧安装回退目录 | RocksDB 已归档且移出活动位置后，单独确认删除 BH 根；不作全量备份 |
| BTC、indexer、chain、control-plane、secure、UTXO状态、旧 snapshot 下载目录 | 逐项确认删除，不要求身份或恢复备份 |
| 旧 release kit、launcher、node 配置、controller unit | 逐项确认删除；会话只留不含凭据的路径布局以支持续跑 |
| 本节点已退出的容器（含 SourceDAO helper） | 显示容器名、ID和挂载路径后逐项确认，使用 `docker rm`，不带 force/volume 删除参数 |
| 镜像 | 仍按既有清单核对精确 ID 后单独处理，不执行全局 prune |
| `--backup-dir` 和其内已有旧档案 | 不作为清理对象；v1留下的其他备份不再要求重复验证 |
| 其他节点、宿主机工具、SSH/sudo、Docker、挂载 | 不属于清理范围 |

跳过 BH 归档时，BH数据库和其父服务目录都会保留，其余路径仍可分别处理。
运行中的相关容器会阻止清理；拒绝移除的退出容器若仍引用目标路径，该路径也不会被强删。

### 4.3 BH move/copy 与旧会话迁移

- `--bh-backup-mode move`：同一文件系统内 rename，保留文件 inode，并核对条目、大小、mtime和权限。
  不复制数据库、不反复读取全库做SHA-256；这验证移动边界和元数据，不证明旧数据库无损坏或业务结果正确。
  清理旧快照硬链接导致的ctime变化不算数据库内容变化。`verify` 对这种归档也只检查上述身份与元数据。
- `--bh-backup-mode copy`：复制到独立目录，按文件SHA-256校验，可跨盘，需要同时容纳原库和副本；
  `verify` 重新读取副本核对哈希。复制失败保留部分文件以便续跑。
- 两种模式都先要求服务停止并检查数据库锁；每次移动/复制/删除都有独立确认，回车跳过，`q`退出。
- 备份目录必须位于活动数据根之外。同盘移动不释放数据库占用；新setup仍需实测至少1.5TiB可用空间。
- 移动前先记录意图，中断后可以核对归档续跑。不要先手工移动活动数据库再让脚本猜测其来源。

已有v1会话可以续用。`old_backup`中的`session.json`、`.lock`和`objects/`一起迁移后，传入新路径即可，
无需编辑记录中的源路径或改变`copy/move`模式。尚未开始的BH归档自动缩小为RocksDB目录；
已开始或完成的v1 BH归档保持原来的根目录边界并沿用原校验。

最近只读观察，用户已迁移出`/home`，实际有会话文件的目录是：

```text
/data/usdb/old_backup/old_backup/session.json
/data/usdb/old_backup/old_backup/objects/
```

外层`/data/usdb/old_backup`还有另一份`objects/`，但没有`session.json`。续跑应选内层会话目录，
不合并两份对象，也不再次移动数据库。旧BH仍位于`/data/.usdb/datasets/balance-history/btc-mainnet/09ece7f367d1b0f266adcf9f999be5ca1cfb9cadf13fe66c1ddc907c3b0ca156`。
源与内层会话均在`/data`文件系统，满足rename要求。脚本会在任何容器删除或归档扫描前检查这一条件。

若另一节点尚未迁移，先确保目标目录不存在，再把整个旧会话移到同盘、活动数据根之外；
尚无BH归档时迁移小型旧会话即可。已完成的v2 move归档依赖原inode，不能再用跨盘copy替换它并期待元数据验证通过。

### 4.4 node1 续跑命令

先在开发机更新独立脚本：

```bash
scp /home/bucky/work/usdb/docker/scripts/tools/node_rebuild.py \
  usdb@192.168.1.119:/home/usdb/node_rebuild.py
ssh usdb@192.168.1.119
```

在node1预览实际会话。旧文件和会话有root所有的私有条目，所以独立工具使用sudo，操作员home仍显式指定：

```bash
NODE_REBUILD_BACKUP=/data/usdb/old_backup/old_backup

sudo python3 /home/usdb/node_rebuild.py plan \
  --operator-home /home/usdb --backup-dir "$NODE_REBUILD_BACKUP" \
  --bh-backup-mode move
```

服务已经停止时直接续跑；若仍有服务，先用原操作员的正式`controller disable`和`down`停止。
脚本自身不会启动、停止或强制删除运行中的服务：

```bash
sudo python3 /home/usdb/node_rebuild.py run \
  --operator-home /home/usdb --backup-dir "$NODE_REBUILD_BACKUP" \
  --bh-backup-mode move
```

依次确认归档目录、已退出的SourceDAO容器、BH RocksDB移动以及各清理路径。
不再出现keystore、钱包、旧kit的BACKUP提示。默认目标库为：

```text
/data/usdb/old_backup/old_backup/objects/balance-history/db/balance_history
```

这是旧库的唯一离线保留副本，新服务不要指向这里。后续只读工具可将`objects/balance-history`作为服务根进行余额/commit查询；
外部registry sidecar不在本次归档中，反查样本需使用已有映射或另行导出，不能声称保留了完整旧registry。
如需重新检查归档，使用相同参数运行`verify`：

```bash
sudo python3 /home/usdb/node_rebuild.py verify \
  --operator-home /home/usdb --backup-dir "$NODE_REBUILD_BACKUP" \
  --bh-backup-mode move
```

节点本地的`session.json`会自动升级为v2，保存路径、原文件身份及完成状态，不保存新的RPC凭据或账户密钥。
已有旧档案留在会话目录；本工具不会为了精简流程自动删除它们。
本地测试覆盖真实临时文件的copy/move、旧会话迁移、删除后续跑、同盘预检、锁保护、逐容器确认及拒绝清理其他节点。
本节命令由操作员执行，文档更新和本地测试不代表已在node1移动或删除数据。

## 5. 后续执行顺序与完成条件

1. 补齐只读现场清单和比较采集工具；发布 workflow 原生输入接入已实现，待审查提交后生成经验证的原生 release/安装包。
2. 按第4节归档完整BH RocksDB并验证，后续从离线旧库读取G/H参考材料；不做旧节点身份和恢复备份。
3. 确认 controller/服务停止；逐项确认移除本节点遗留容器，再逐项清理相应旧目录/镜像。
   清理BH活动目录以RocksDB归档验证通过为前提，其余测试节点数据不以完整备份为前提。
   已有 `old_backup` 中的配置/kit 仅在导出工具仍依赖它们时暂留，导出完成后无需为恢复旧节点继续保留。
4. 安装原生 release，正式 `setup/doctor/up`，重新输入矿工 address。只运行这一套主网服务；
   记录下载、Core 导入、BH 导入/重放和资源阶段切换。
5. 核对 G 封存摘要、`[G,H]` 的 BH commit 和固定高度余额/反查样本；新 indexer、chain、control-plane 验收正常运行及业务调用。
   不要求保存或恢复这些下游服务的旧数据库；如果 BH 接口保留窗口不足，提前安排固定高度的比对阶段。
6. 在同一部署验证重启与恢复，记录峰值磁盘/内存、各阶段实际耗时、前台就绪和后台历史验证进度。
   下载中断、错误签名/文件等优先复用小型自动化覆盖；需真实主网窗口的中断场景单独排期，避免反复重导入。

长任务包括主网下载/后台验证、BH 全量导入与 B+1…G/H 重放，以及按需的旧节点全量参考扫描。
不预估尚未测量的完成时间；一次正式部署尽量合并 P6 主网与 P7 交付证据。
前台全套服务通过可以先记录；后台未完成时必须保留实际状态，不能把它标成完成，也不需阻塞 BH 的正常使用。

下一次执行前的交付物应是：实际空间/回收清单、已校验的比较基线、选定原生 release、可逐条执行的同机重建手册。
本文没有执行上述停服、归档、删除、发布或重部署步骤。
