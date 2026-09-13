# P7.4：复用一台测试节点的正式部署验收方案

日期：2026-09-13。基于 P7.3 提交 `3c71205`。本文是执行前评估，不是清理或重部署记录。

## 1. 目标与现场确认程度

采用用户指定的第一台测试节点，先归档旧状态的比较证据，再在同机按正式发布安装流程串行重建。
不要求同时存放两套完整 Bitcoin/BH/indexer 数据。开发机现有 P4/P5/P6 证据可复用，不重新生成旧 BH 大型快照。

本次只读 SSH 探测确认主机名 `bucky04`、操作员 `usdb`，用户属于 `sudo/docker` 组。
首个探测在返回主机身份后未继续返回结果，后续 SSH 连接超时，原因尚未确定。
没有取得该节点的 release、服务版本、数据根、挂载关系、占用和可用空间；不能据此给出可删除路径或可回收字节数。
本次没有停止服务、修改配置、导出数据库或清理数据。

连接恢复后，首先补齐以下只读清单：

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

1. **接入正式 release bundle。** `usdb-release-candidate.yml` 和 `usdb-release-publish.yml` 目前仍选源码内旧
   `docker/networks/testnet-v0`，并有旧 snapshot 发布记录校验。需要让已审查的原生输入进入这条既有链路，
   固定模式、B/G/hash、来源和可选公钥；candidate、manifest、publish 复核、node-kit 必须使用同一 bundle。
   可将原生配置作为受版本管理的发布输入，再确定生成/复核步骤；不能发布时临时替换 bundle 而沿用旧 manifest。
2. **定义同机旧配置重置。** `setup` 会拒绝已有 `node.env`；`activate-release` 只接受相同数据兼容契约。
   旧 BH 与原生 BH 的契约/目录不同，不能仅替换镜像、手改 compatibility ID 或 dataset marker。
   应先明确 controller、配置、RPC 凭据与服务目录的归档/重建范围，再给出实际主机的命令。
3. **补齐只读比较采集。** 现有 `check_assumeutxo_p65_mainnet.py` 面向已封存 G 的原生服务；
   其 `capture_anchor` 要求 BH/indexer 当前高度恰好等于目标高度。
   它不能直接充当仍在追块的旧节点的任意历史高度导出器。需补充固定历史锚点采集与比较，或安排受控的固定高度验收阶段。
4. **自有签名 UTXO 的实际发布。** 原 `create/finalize/publish` 的 UTXO 分支、旧密钥适配和上传复用已实现并完成本地验证。
   操作员按[发布手册](./balance-history-assumeutxo-snapshot-publish-operations.md)使用原 `snapshot-keys` 和对象存储发布，
   取得通过匿名下载核对的真实来源；不要求重新生成私钥或签发 Core 二进制。公开源 `pinned` 模式不依赖这项自有签发。

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
源码目前仍保留原始 `.dat` 供整套服务校验/恢复使用，可将已下载文件作为保留项；不是必删的旧 snapshot workspace。

尚无目标节点目录清单，当前只定义数据类别，不授权删除任何路径。
清理前应另列每个实际路径、数据身份、占用、保留/回收理由和预期释放空间；同盘重命名归档不会释放空间。

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
