# USDB 整机内存预算与自动资源切换

## 1. 生效范围与主机基线

新 `setup/configure` 的自动策略和停机后的显式重算使用资源策略 v2，保存
`USDB_RESOURCE_POLICY_VERSION=2`。未包含该字段的运行中节点继续使用原策略；安装工具包
不会在线改写它们的额度。manual 模式保留原有配置和 32 GB 基线。

有效主机容量取 `MemTotal` 与 controller 所在 cgroup 及父级内存限制的较小值。
不使用随负载变化的 `MemAvailable`，swap 不计入 RAM。GB 门槛按十进制计算，GiB 和配置
中的 `g` 按 1024³ 字节计算。

| 场景 | 最低有效容量 | 说明 |
| --- | ---: | --- |
| 全新部署、基线导入、`bitcoin/overlap` | 32,000,000,000 字节 | 保留初始同步预算 |
| 已进入运行阶段的节点恢复，`catchup/steady` | 16,000,000,000 字节 | 使用较低的运行预算；实际服务最低额度及外部预留仍须全部容纳 |

16 GB 是本轮资源配置的准入边界，预算试算和故障注入测试不等同于主网长期峰值验收。
特别是 Ord、archive/tracing、大量查询或重新建索引，仍需观察实际内存压力与吞吐。
缩容应在全部同步完成后进行；中途缩容到小预算虽然可以续跑已有数据，但不保证追赶速度。

## 2. 阶段与完成判定

自动阶段为 `bitcoin → overlap → catchup → steady`。

| 阶段 | 含义 | 下游行为 |
| --- | --- | --- |
| `bitcoin` | Bitcoin 独立同步、快照准备 | 保留独占同步限制；Ord 仅运行等待监督进程 |
| `overlap` | Bitcoin 同步与 BH 导入、重放和核验交叠 | 按数据锚点启动 BH/indexer |
| `catchup` | 前台可用，仍可能有后台验证、索引或链同步 | chain 按既有共识与身份检查启动；Ord 获得追赶预算后等待自己的依赖 |
| `steady` | 当前启用的同步任务持续完成 | 使用较低的运行预算 |

原生 AssumeUTXO 的前台就绪不要求 Bitcoin 历史或 txindex 完成。chain/mining 可以在
`catchup` 运行；不开启 Ord 时不等待 Ord。资源控制器在前台启动结束后仍驻留，约每 15 秒观察：

- Bitcoin 前台就绪、历史验证完成；开启 txindex 时，它必须已追到当前前台高度。
- BH/indexer 的实时共识就绪和进度完成，USDB chain 已完成同步并满足既有入网检查。
- 开启 Ord 时，它的当前容器启动后生成的观测仍在有效期内，且历史、txindex、canonical 检查均通过。
- 没有正在执行的快照导入、registry 安装、chain 初始化或 checkpoint 恢复作业。

以上条件连续成立 60 秒才进入 `steady`。缺失、过期观测、容器重启和采样长时间中断都会
重新计时，旧的 phase、历史报告或缓存提示不能充当当前完成证据。
`getindexinfo` 查询失败只使索引完成情况未知，不撤销已取得的 Bitcoin 前台就绪状态。

Core 历史和索引先完成、Ord 仍追赶时，也会经 60 秒窗口先释放 Core 预算，保存
`USDB_RESOURCE_RUNTIME_BUDGET=core-ready`。旧 Ord 策略保持其余服务额度；新 `adaptive-v2`
先分配各服务运行额度，再按整机余量扩大 Ord 追赶预算（见下节）。
所有任务完成后保存 `compact`。普通新块或短暂 RPC 故障不切回追赶阶段；持续 5 分钟落后
至少 1,000 个 BTC/BH/indexer/Ord 进度单位才从 `steady` 回到 `catchup`。
回退采用 `core-catchup`，在现有运行服务预算之外将空余额度分给 Core；仍受整机内存池约束，
不重新启用独占 Bitcoin 档。

## 3. 预算与缓存

初始同步沿用 `balanced/slow-disk` 的内存池与封顶算法：扣除外部服务预留后，节点可用
80%～90%（默认 90%），系统至少保留 4 GiB。`bitcoin` 将剩余池优先给 Core；`overlap`
为 BH 保留至少 8 GiB，慢盘档将更多余量给 Core。32 GB 及以上的首次 `catchup` 保留原来
运行阶段的后台验证/Ord 追赶预算。用户已有的自定义封顶继续生效。

compact 运行预算系统至少保留 2 GiB，并继续遵守所选百分比；它按各服务最低额度计算，
然后在上限以内增配，不为了占满 RAM 而分配。下表是**有效容量 16 GB、原生 AssumeUTXO、
默认 90%、无外部服务预留**的预算，不是实测进程占用：

| 服务/用途 | 容器上限或预留 |
| --- | ---: |
| Bitcoin | 2 GiB，dbcache 512 MiB |
| balance-history | 4 GiB，应用缓存合计 1.5 GiB |
| USDB indexer | 1 GiB |
| USDB chain / 顺序运行的 chain-init | 2 GiB |
| control-plane | 512 MiB |
| registry installer | 512 MiB |
| checkpoint verification | 256 MiB |
| Bitcoin bootstrap helper | 512 MiB |
| 可选 Ord | 2 GiB，索引缓存 512 MiB |
| 系统预留 | 2 GiB |

含所有辅助预算、开启 Ord 时总额 14.75 GiB，低于十进制 16 GB；不开启 Ord 时总额
12.75 GiB。更大的运行主机可逐渐增至 Core 4 GiB、BH 8 GiB、chain 4 GiB、indexer
2 GiB、Ord 4 GiB。compact dbcache 不超过 1 GiB，BH 应用缓存为容器额度的 37.5%，
Ord 缓存不超过 1 GiB 且不超过其额度的四分之一。系统百分比与外部预留仍优先保留；
无法容纳所有服务最低额度时明确拒绝配置。

小内存恢复和 compact 冷启动先使用 `core-catchup`。以 compact 为基础，只将未分配额度
增加到 Core 容器上限，最高 8 GiB，仍遵守显式 Core steady cap；dbcache、BH/Ord 和其他
服务预算均保持不变。有效容量 20.49 GiB、90%、Ord 关闭时 Core 为 8 GiB、dbcache 655 MiB。
Core 持续完成 60 秒后采用 `runtime-ready`，释放额外 Core 额度；等所有服务完成再进入
`steady / compact`。`core-ready` 或 `runtime-ready` 若持续 5 分钟存在 Core 未完成且进度落后至少 1,000，
也可回到 `core-catchup`。观测未知时不降额。

新 setup 或停机后显式重算将 `USDB_ORD_RESOURCE_POLICY` 设为 `adaptive-v2`，保留旧版本公式，
避免已有 `node.env` 因算法变化而校验失败。初始同步及 Core 尚未完成时的分配不变；
`catchup/core-ready` 与 `catchup/runtime-ready` 为其他服务保留上述运行目标后，Ord 上限为：

```text
min(Ord cap, (effective RAM - external reserve) / 2, remaining pool after other services)
```

Ord 最低额度仍为 2 GiB，整机必须先满足所有服务最低额度。追赶缓存为 Ord 上限的三分之一，
最高 16 GiB，按 MiB 向下取整；其余空间用于 UTXO 批次、地址表及 cgroup 计费的文件缓存。
默认 Ord cap 仍为 32 GiB，自定义 cap、系统预留和外部服务预留继续生效。
在 64 GiB / 90% / 无外部预留时为 32 GiB 容器额度和约 10.7 GiB 缓存；20 GiB 同条件下约为
5.5 GiB / 1.83 GiB。所有服务持续完成后回到原 compact 预算。
容器额度/缓存变更使用下述正常停止与恢复流程，冷启动和缩容保留已选 Ord 策略版本。
`resources --json` 的 `phases` 可能包含多个 `catchup` 条目，需结合 `runtime_budget` 识别；
预览不会将 Core 已完成当作当前事实，也不会修改配置或容器。

BH 的所有配置仍校验：

```text
UTXO cache + balance cache <= BH memory limit × (pressure threshold - 10) / 100
```

snapshot-loader 与 BH、chain-init 与 chain 分别按顺序复用预算槽位；实际容器检查仍逐个
计算并发实例，不因逻辑上应当顺序运行而漏算异常并发。后台资源切换会等待一次性作业退出。

## 4. 安全切换与中断恢复

每次切换先把目标 phase、预算、计划摘要和待恢复服务原子写入 `node.resources.json` 并刷盘。
仅停止需要改变容器额度/应用缓存的服务及其写入依赖：Core 改动会先停 chain、indexer、BH；
BH 改动先停 indexer、chain；仅 Ord 改动时不会停止 Core/BH。

使用正常退出或 SIGTERM，等待数据库刷盘，不加入超时 SIGKILL，也不直接降低运行中数据库
的硬内存上限。写入新配置后重建必要容器，核对 Docker 的实际额度、swap 和相关应用缓存，
再按原有 readiness、数据身份和恢复门禁启动下游。不会重新构建或删除已有数据。

`pending` 表示预算交接尚未完成；`recover_services` 单独追踪已计划停止但尚待恢复的服务。
新 Core 已采用目标配置时，重试不会重复重启它。切换期间更换 release/策略会因计划不匹配
被拒绝。controller 轮询期间释放操作锁，实际切换期间持锁；前台可用后继续管理资源，
queued mining/peers 操作完成后也回到资源观察。`status/doctor/resources` 都是只读入口。

## 5. 缩容、升级与观察

推荐先确认新版 `steady`、没有 pending 切换，再完整停机和调整 VM：

```bash
usdb-node down
# 在虚拟化平台调整内存，启动 VM 后：
usdb-node up --no-watch
usdb-node resources
usdb-node doctor
usdb-node status --watch
```

停机的旧自动 `steady` 节点或容量变化后的运行阶段节点，在实际 `up` 时先按当前主机容量
重算，再进入常规预检；旧 `steady` 重新归为 `catchup` 接受实时观察。已经完成降档的节点
冷启动采用 `core-catchup`，确认同步后再释放额度。原有 release 激活、数据身份和
快照兼容检查仍然执行。容量变化且容器运行、未知 Docker 状态或未完成的切换不会被擅自重算。
旧版 `catchup / compact` 在显式 `up` 时迁移到 `core-catchup`；容量不变时可通过已有的
切换日志正常停止 Core 及其消费者后恢复，无需操作者手工修改容器限额。
因此缩容后直接运行只读 `doctor` 可能仍看到旧预算不符，可先显式重算再检查：

```bash
usdb-node set-resource-policy --mode auto
usdb-node resources
usdb-node doctor
usdb-node up
```

改变磁盘档位、封顶或外部预留也用停机后的 `set-resource-policy`。升级只安装工具包时
保留运行中节点的旧策略；停机后的运行阶段 `up` 或显式重算才采用 v2。尚处初始同步的旧节点
需显式重算，仍要求 32 GB。旧 manual 节点不会因安装新版本而自动切换模式。

`resources` 预览当前阶段可到达的计划，小内存运行节点不必通过首次导入的 32 GB 校验。
`status --watch` 区分 `CATCHUP`、具体 runtime budget、容器上限和资源切换等待，资源历史记录策略版本、phase、runtime budget
以及实际额度。停止 controller 会暂停自动资源管理，已运行容器继续运行；重新 `up` 可恢复。

发布前仍需使用可维护的测试节点验证 16/20 GiB VM 的冷启动、持续追块/出块、Ord 可选路径、
实际峰值内存和一次正常降档。当前本地回归覆盖预算、真实 shell 编排与模拟容器故障续接，
不能替代 Docker/PVE 上的运行验收。
