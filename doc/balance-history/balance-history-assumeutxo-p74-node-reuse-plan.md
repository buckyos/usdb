# P7.4：复用一台测试节点的正式部署验收方案

日期：2026-09-13。发布输入已提交为 `8bd2bbd`。本文是执行前评估，不是清理或重部署记录。

## 1. 目标与现场确认程度

采用用户指定的第一台测试节点，先归档旧状态的比较证据，再在同机按正式发布安装流程串行重建。
不要求同时存放两套完整 Bitcoin/BH/indexer 数据。开发机现有 P4/P5/P6 证据可复用，不重新生成旧 BH 大型快照。

本次通过 `ssh usdb@99.47.70.238 -p 2224` 只读核实：主机名 `bucky04`，操作员属于 `sudo/docker` 组，
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
| 需要保留的节点身份 | chain keystore、矿工/钱包密钥、controller 配置、RPC 凭据等是否存在及其备份位置 |

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
   应先明确 controller、配置、RPC 凭据与服务目录的归档/重建范围，再给出实际主机的命令。
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

至少保留业务起点 G 与一个较新的固定高度 H。G 用于复用既有主网全量投影证据，H 用于覆盖实际矿工证和下游服务。
H 必须落在旧 BH/indexer 均保留的历史区间，并固定对应 canonical BTC hash；不是随采样变化的“最新高度”。

| 证据 | 留存内容及比较范围 |
| --- | --- |
| 运行身份 | release/bundle 文件及哈希、三仓库源码 revision、镜像 digest、网络/G/hash、协议/算法/registry 身份、脱敏配置 |
| BTC 锚点 | G/H 的 canonical block hash；采集前后核对 H 未被重组，必要时重采 |
| BH | G/H 的 block commit、可取得的历史 state-ref、固定地址/脚本的高度余额；归档样本键和请求参数 |
| indexer | H 的历史 state-ref、pass commit，以及该历史 state-ref 内嵌的 local/system 信息；代表性矿工证 owner/状态/能量 |
| control-plane | 同一批矿工证对应的脚本反查结果、地址及所依赖的 BTC 锚点；动态展示/时间字段单独记录 |
| chain | genesis/链身份、选定旧链块及其实际消费的 BTC 锚点和承诺输入；如计划续跑，还需保护相应链数据和身份密钥 |
| 运行与故障 | 当前 readiness、版本和必要日志；用于解释行为，不作为数据一致性的替代 |

历史 H 的比较不得混入只返回当前头部的 `get_local_state_commit_info/get_system_state_info`。
优先取 `get_state_ref_at_height` 的历史内嵌结果，采集前后重复核对锚点；若旧版本缺少历史接口，应明确限制或安排停在 H 的窗口，不能拼接不同时点结果。
样本覆盖已有非零余额、之后归零、B 后首次出现脚本、实际铸造/转移矿工证的地址。
对 B 前已归零且之后未观察到的脚本，新 registry 不承诺反查；不能把这种覆盖差异记为余额错误。

相同 G、BTC 链、协议和领域身份下，比较 BH commit/snapshot identity、pass/local/system 承诺及业务结果。
历史保留下界、registry coverage、bootstrap 来源、进度计数和时间字段可以不同。
重新启动测试链后产生的新 chain tip/hash/state root 不能直接与旧链 tip 比较；只有相同区块和共识输入才适合做这些比较。

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
私有配置、身份密钥另做受限备份，不混入可公开的验收报告。无需为此生成新的旧式 BH 数据库 snapshot。

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

### 4.2 清理和保留清单（已确认路径；尚未执行）

下面 `BH_OLD` 指：
`/data/.usdb/datasets/balance-history/btc-mainnet/09ece7f367d1b0f266adcf9f999be5ca1cfb9cadf13fe66c1ddc907c3b0ca156`。
`INDEXER_OLD` 指：
`/data/.usdb/datasets/usdb-indexer/5754a8f5af6839a0710aef7289c81514923634185873a26e4d133d8290095ead`。

| 对象 | 计划处理 | 先决条件/理由 |
| --- | --- | --- |
| `BH_OLD` | 停服后一致性备份整个服务根；备份核验后清除旧活动路径 | 包含完整 `db`、`auxiliary` registry、bootstrap、配置、dataset marker 和必要日志；不能只复制几个 SST |
| `BH_OLD/db_backup_snapshot_install_1788678082866748700` | 单独归类旧安装回退副本 | 本次仅测得274432 bytes；不是当前数据库备份，不替代上一项 |
| `/data/.usdb/datasets/bitcoin/btc-mainnet` | 整个清除并由 setup 重建 | 包括 blocks/undo、indexes、chainstate、日志、peers 和 dataset marker；不保留旧块，不只删 chainstate |
| `INDEXER_OLD` | 留存 H 锚点与样本后清除 | 新 indexer 从相同业务 G 重建；必要的历史 state-ref 先导出 |
| `/data/.usdb/networks/usdb-testnet-v0/usdb-chain` | 保护密钥/身份及旧链证据后清除并重同步 | 已发现 `keystore`、`geth`、`bootstrap`、`recovery`；保护 keystore、geth/nodekey 和 SourceDAO/bootstrap 相关私有材料。仍使用相同 v0 genesis，不能误当新链创世节点 |
| `/data/.usdb/networks/usdb-testnet-v0/control-plane` | 归档配置/日志后清除 | 已确认目录存在；按新 release 重建 |
| `/data/.usdb/artifacts/balance-history` | 旧 core/registry 下载包与安装状态可清除 | 当前含 `balance-history-bitcoin-h963800-59e54b88ef118294`、`balance-history-bitcoin-h963800-99ef3fb1f13b8609`；先确认完整 BH 备份包含所需 registry，且无其他节点引用 |
| `/home/usdb/.config/usdb/usdb-testnet-v0` | 私密归档后移出活动位置，再走 setup | 已发现 node.env、node.resources.json、node.mining.json、sourcedao 和操作锁；不要单删 node.env 留下旧控制状态 |
| `/data/.usdb/networks/usdb-testnet-v0/secure` | 先私密备份，重建时生成新 RPC 凭据 | 这是代码约定路径，现场尚未单独核实；如有其他签名材料则保护，不能整段无条件删除 |
| `/etc/systemd/system/usdb-node-bootstrap-usdb-testnet-v0.service` | 先 disable/停服；归档后移除旧 unit，由新 setup 安装 | 旧 unit 可能指向旧 release；保留日志证据 |
| `/home/usdb/.local/share/usdb/releases/` 下的旧 r4/r5/r7/r10/r11/r12/r14/r15/r17 | 归档至少 r17 的完整 kit/manifest 后，按明确 release 列表移除 | 离线查询旧 BH 可能需要旧版本工具；不能先丢掉唯一兼容工具 |
| `/home/usdb/.local/bin/usdb-node` | 停服并归档后移除旧链接 | 新 installer 重建；不清整个 `.local/bin` |
| USDB runtime/Bitcoin 两个 Compose 项目的容器与镜像 | 正式 down 后清容器；仅移除核实过、未被其他容器引用的镜像 ID/digest | 已观察旧 snapshot-loader/registry-installer 和独立 sourcedao 退出容器，清理前补齐精确项目/mount/引用清单；不要 `docker system prune -a --volumes`，不清整个 Docker 根目录 |
| `/data/.usdb/artifacts/assumeutxo/mainnet-935000`、`/data/.usdb/networks/usdb-testnet-v0/assumeutxo` | 新部署前应为空或不存在 | 新代码约定路径，未发现旧配置使用；若实际存在，先核实来源再清下载片段和 activation journal |
| `/data/usdb-reference/node1-pre-assumeutxo-20260913` | 建议的独立保留目录；永不纳入此次清理 | 尚未创建；位于活动数据根外，同盘保存仍受370.05 GiB总保留量约束 |
| SSH/sudo、Docker Engine/Compose、`/data` 挂载、其他服务和其他网络数据 | 保留 | 不需要卸载宿主机 Docker 或格式化磁盘；`/data/usdb` 也未确认用途，不列入清理 |
| `/home/usdb/.usdb` | 先核实旧目录的用途，暂不删除 | 现场存在，当前 node.env 使用的是 `/data/.usdb`；不根据目录名直接推断可回收 |

### 4.3 BH 备份方式与操作顺序

1. 先保存旧 Core 故障日志和磁盘/文件系统健康观察；新部署前排除仍在发生的 I/O 问题并验证 BH 备份。
   采集第3节的固定高度 H 锚点/样本和版本。旧 Core 重启循环无法提供稳定 RPC 时，可用另一台经过验证的同链 Core
   核实 G/H canonical hash；不要把 BH 的健康检查或不断变化的“最新高度”当作对比基线。
2. 使用旧 `r17` 正式入口执行 `controller disable`，再 `down`（不带 `--keep-bitcoin`）；
   检查 controller inactive、两个项目容器退出、无进程持有 BH DB。数据库必须干净关闭后再作物理备份。
3. 首选复制整个 `BH_OLD` 到独立盘/主机上的受限目录，并生成文件清单、大小和 SHA-256；
   同时保存 r17 kit、私有配置和上述身份材料；确认对应旧服务镜像可再次按digest取得，否则先离线保存该镜像。
   不要对运行中的 RocksDB 使用普通 `cp/rsync` 并称为一致性备份。
4. 如果只能同盘保存，可在干净停服后把整个 BH 根移动至上述保留目录，作为冻结的**唯一旧副本**。
   这避免临时存两份 BH 的空间峰值，但不等于另一介质上的备份，不释放 BH 占用；不得把新服务指向此目录。
5. 校验副本清单/哈希；用匹配旧版本工具对工作副本进行 DB 打开和 H 锚点检查，避免恢复检查改写唯一保留副本。
   registry 若含外部路径引用，先核实并保存对应文件。完成后才执行清理表中的回收动作。
6. `df` 复核至少1.5 TiB可用，确认新 BTC 目录无旧 blocks/chainstate/indexes，确认新 UTXO artifact 无预置文件，
   然后执行新 installer、setup（选择 `/data/.usdb`）、doctor、up。

本节是排期/审批清单，没有执行停服、复制、移动或删除，也没有生成可无条件运行的递归删除命令。

## 5. 后续执行顺序与完成条件

1. 补齐只读现场清单和比较采集工具；发布 workflow 原生输入接入已实现，待审查提交后生成经验证的原生 release/安装包。
2. 从旧节点取得 G/H 参考材料、必要身份备份并移出目标节点，验证完整性；确定 chain 是同网络重同步还是保留旧链续跑。
3. 明确重建窗口和实际路径清单后，使用旧节点正式入口停止 controller/服务并确认退出，再执行约定的配置重置与数据回收。
4. 安装原生 release，正式 `setup/doctor/up`。只运行这一套主网服务；记录下载、Core 导入、BH 导入/重放和资源阶段切换。
5. 核对 G 封存摘要及 H 的历史锚点/业务样本，再验收 chain/control-plane readiness 与其实际消费的承诺。
   如果接口保留窗口不足，提前安排官方配置支持的固定高度阶段，不能等 H 已被回收后才取样。
6. 在同一部署验证重启与恢复，记录峰值磁盘/内存、各阶段实际耗时、前台就绪和后台历史验证进度。
   下载中断、错误签名/文件等优先复用小型自动化覆盖；需真实主网窗口的中断场景单独排期，避免反复重导入。

长任务包括主网下载/后台验证、BH 全量导入与 B+1…G/H 重放，以及按需的旧节点全量参考扫描。
不预估尚未测量的完成时间；一次正式部署尽量合并 P6 主网与 P7 交付证据。
前台全套服务通过可以先记录；后台未完成时必须保留实际状态，不能把它标成完成，也不需阻塞 BH 的正常使用。

下一次执行前的交付物应是：实际空间/回收清单、已校验的比较基线、选定原生 release、可逐条执行的同机重建手册。
本文没有执行上述停服、归档、删除、发布或重部署步骤。
