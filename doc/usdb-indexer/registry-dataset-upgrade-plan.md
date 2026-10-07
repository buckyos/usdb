# Registry 数据集接管方案与验收

状态：按 2026-10-07 讨论确定实施边界。G1 已提交为 `073f13d`；G2 已提交为 USDB `a3dd955` / Go `44ae78a51`；G3 已完成本地真实容器和服务矩阵验收、尚未提交，结果见第 5 节。对应 UIP-0017 实施计划 G；不改变网络参数或分配新的业务规则版本。

## 1. 约束

- 协议按约定高度激活；软件安装和本地数据库格式变更不改变历史规则。
- 新版接管旧库时检查已执行历史，而不是等到激活高度才检查，也不按新准入条件重新审核旧证。
- 同 source、scope、origin 的 catalog 只能沿不可改写的 revision 序列前进。历史前缀一致才允许接管。
- 已按过期规则越过分歧点的库明确拒绝复用，给出来源、目标、已处理高度和分歧上下文，要求操作者重建受影响的数据。
- 不提供针对过期协议分支的局部回退修复工具。既有 Bitcoin reorg / 区块发布中断恢复仍保留。
- 不把配置错误、缺失 registry、I/O 错误或程序异常统一归类为需要重建；不自动删除数据。
- 数据库重建仍按各历史区间重放，不改变 genesis、链身份和已经成立的规范历史。

## 2. 实施顺序

### G1：索引器双库接管

在服务启动、任何索引写入和 RPC 发布之前处理。精确匹配仍走原有启动路径；发生 revision 变化时：

1. 验证双库原绑定完整、一致，且原 revision 存在于目标 catalog。拒绝跨域、降级、未知来源和无恢复记录的混合双库。
2. 要求 SQLite 与 RocksDB 处于已完成区块的相同边界，无 pending energy 或 upstream reorg。未完成的旧操作须先用原配置完成正常恢复。
3. 按已提交高度比较完整历史区间，并验证该历史的执行能力。空库也必须验证来源和接管方向。
4. 将来源、目标、双库高度、末尾 block commit 和 reorg epoch 写入 SQLite 接管记录；同步写入 RocksDB 新绑定；在一个 SQLite 事务中更新绑定并清除接管记录。
5. 重启重新验证接管记录、配置和高度，只允许完成同一目标。不得仅凭两库恰好不同就猜测一次升级。

接管不修改 pass、能量、区块承诺、历史 snapshot 或旧 registry。未知未来规则仍提前报告能力缺口，并在对应高度写入前停止。

### G2：节点工具和操作文档

升级预览区分网络重置、可验证的 registry 接管、存储重建和阻止操作。数据目录身份、冻结配置、双库身份的责任不能互相替代；只有完整接入通过验收后才能放宽现有 `upgrade-release` 的重置/绑定保护。

G2 按以下顺序实现，不能仅将 registry ID 从 `CHAIN_FIELDS` 中删除：

1. 将 registry 追加、USDB checkpoint 配置更新、存储 schema 变化与真正的网络重置分别分类。静态 manifest 相容只表示可继续预检，不代替本机数据库校验。
2. 在停机后调用实际服务的离线检查，读取各自已提交高度。BTC 与 USDB 高度不可互换；主链配置须通过 Geth 对已生效 checkpoint 的兼容检查。
3. 保留 genesis block identity，只允许经校验的未来 chain-config 更新。现有 `ethw_init.sh` 对 genesis 文件摘要变化直接拒绝；必须通过受控配置更新后同步 init marker，不能直接修改 marker 绕过检查。
4. 将目录路径/宿主 identity marker、双库接管和链配置更新纳入可恢复会话。旧版本回退只在明确安全边界允许；不得让配置文件先声称升级完成而实际数据库仍不可使用。
5. 主网不能把一个未支持的运维迁移路径解释为授权重置网络。拒绝路径保留现场并提供准确的下一步。

明确前进接管与重建建议；保留钱包、节点身份及未受影响的 BTC/BH 数据。接管路径只移动活跃 indexer 目录而不复制数据库；重建路径继续保留旧目录和显式清理，不增加自动删除或故障重建循环。USDB chain 的 checkpoint 配置兼容性必须独立验证，不能以 BTC 历史前缀一致代替。

### G3：完整验收

以隔离数据库和既有真实服务夹具验证升级前后、历史查询、从 origin 重放等价；将关键边界与拒绝路径纳入常规测试，长时真实服务覆盖进入 nightly/weekly。

## 3. 测试矩阵

| 场景 | 预期 |
| --- | --- |
| 相同 revision 重启 | 原恢复行为不变，不生成接管记录 |
| H 前切换到追加未来规则的 revision | 原库继续使用，旧历史与承诺不变，H 时使用新规则 |
| 旧规则执行到 H 或 H 后 | 在任何接管写入前拒绝，要求显式重建 |
| 多个区间中间分歧但末尾一致 | 拒绝，不能只比较 tip 的版本集合 |
| 未知来源、跨 source/scope/origin、降级 | 拒绝，不能通过清空或换标签隐式解决 |
| 一库缺失或双库混合 | 拒绝，不认领另一套数据 |
| 接管记录写入后 / RocksDB 切换后中断 | 同配置重启完成接管；高度、来源或目标变化时拒绝 |
| SQLite 最终提交后重启 | 幂等，无待恢复接管记录 |
| pending block / reorg / 双库高度不一致 | 要求先完成原有恢复，不能借接管绕过 |
| 未知未来规则 | 已完成历史可复用；提前提示，激活点写入前失败 |
| 升级续写与新库重放 | 相同 registry 历史上下文下的状态、承诺、能量与查询一致 |

## 4. 交付记录

G1 实现位置：`storage/rules_upgrade.rs`、双库私有元数据 API 与 `InscriptionIndexer::new`。SQLite 使用 FULL 同步，RocksDB 绑定写使用同步 WAL；接管意图先于两库切换持久化，最终 SQLite 绑定与意图删除在同一事务提交。

`tests/miner_pass_registry_adoption.rs` 使用真实 SQLite/RocksDB 和 ordered pipeline，覆盖 prefix 保留、旧 registry 固定上下文、续写与新库重放、H/H+ 拒绝、中间分歧、空库、未知未来规则、混库/降级、未完成区块、恢复记录变更及三个持久化边界的真实进程退出。

离线 checkpoint 的导出、验证与安装共用校验入口，遇到尚未完成的接管意图即拒绝；即使当时双库仍同绑旧 revision，也不能将其视为已完成的快照。接管 key 在 `usdb-util` 共享，不新增业务规则版本。

本地验证（2026-10-07）：

- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer --offline`：393 通过、12 忽略；另有普通生产二进制隔离测试 1 通过。新增接管模块 10 个父测试通过，真实进程退出入口为 ignored 子测试，由父测试显式运行。
- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer-checkpoint-tool --offline`：18 通过。
- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-util --offline`：87 通过、2 忽略。
- workspace `cargo check`、`cargo clippy --workspace --all-targets --all-features --offline -- -D warnings`、`cargo fmt --all -- --check` 与 `git diff --check` 通过。
- 现有 fast CI 的 `cargo test --workspace` 自动发现这些测试；尚未触发远端 CI。G1 交付时，G3 的真实 Core/Ord/BH/indexer/Geth 接管联调尚待实施；现已补齐的结果见第 5 节。

测试通过不代表已发布、已部署或目标网络已激活。


### G2：停机工具接入

- `upgrade-plan` 根据经校验的旧/新 artifact 区分 `protocol_upgrade`，原 checkpoints/catalog revision 必须完整保留，source/scope/origin/genesis block/storage schema 必须相同。仅追加 USDB checkpoint 也可走此路径；未知格式与存储迁移不由此工具处理。
- `usdb-indexer registry-upgrade` 复用 G1 的历史校验，先只读打开真实双库，再输出来源/目标绑定、双库高度、block commit、reorg epoch。`--apply --expected-report` 再次比对边界，禁止隐式建库和 schema 初始化。RocksDB 只读日志输出到 stderr，预检通过只读挂载读取 SQLite WAL，不使用会忽略 WAL 的 immutable 模式。
- `geth usdb-upgrade-config` 读取已有 LevelDB/ancient 数据，验证源配置、目标配置、genesis 和三类 head。兼容错误直接拒绝，不调用 rewind；同一目标重试允许幂等确认。显式 metadata 写采用同步写入，不调用自动损坏库修复。
- `node_protocol_upgrade.py` 在同一升级会话中保存两项预检，再同步 Geth config、indexer bindings、目录身份、初始化标记和 node.env。开始写入的意图先落盘；从此拒绝 rollback，恢复必须沿相同目标向前完成。源/目标 release、预检文件或原目录 inode 改变时拒绝。
- 接管保留业务数据、钱包和矿工配置，无旧数据库归档可清理。普通开发网 reset/rebuild 路径继续保持原行为。节点包增加模块，fast CI 增加相关用例；未触发远端 CI、未发布或操作在线节点。

本地覆盖包括真实 SQLite/RocksDB 的只读 WAL 查询、全部接管中断点、late upgrade 与源缺失；真实 LevelDB 的 header-only 越界、genesis/配置错误、目录只读、损坏拒绝和 CLI apply；节点工具则验证发布 bundle、双检查顺序、两次 apply/目录移动/初始化标记/node.env 各阶段恢复、保存凭证变更与目录替换拒绝、仅 chain checkpoint 升级和清理保护。节点协调测试注入服务调用结果，不能代替 G3 的真实容器和跨链运行验收。

G2 的 Go CI 依赖锁已推进到 USDB `a3dd9553f0d2ef274018f8ff93b79a7c43c137d4`。G3 提交后仍需按 USDB → Go 依赖锁的顺序冻结，才可进行成对发布资格验证。

G2 本地验证：indexer 单元 395 通过 / 12 原有忽略，生产分派集成 1 通过；checkpoint 18 通过；util 87 通过 / 2 忽略；升级/清理 Python 74 通过，节点管理 127 通过，网络/配置 68 通过，节点打包 1 通过。工作区 check、Clippy（all targets/all features、warnings as errors）、格式检查通过。Go 1.26 兼容工具链下 `internal/usdbupgrade`、LevelDB、USDB verifier、params、fork ID 和相关 Geth CLI 测试以及 vet 通过；G2 交付时尚未验证 Go 1.18 发布工具链；G3 已补齐该工具链验证，远端 CI 仍待成对提交。


## 5. G3：真实容器与跨服务验收

### 5.1 执行路径

Go 的 `scripts/usdb/run_miner_pass_upgrade_services.sh` 继续承载 nightly
`miner-pass-upgrade` 和 weekly `miner-pass-upgrade-soak`（seed 41/42/43）。准备阶段编译
普通/合成规则 indexer、普通/合成规则 Geth 和 BH，并以已编译二进制及宿主动态库构建
只用于测试的 scratch 镜像；不下载基础镜像、不推送镜像。执行前验证二进制摘要、镜像
ID 与构建凭证，避免在服务启动后才发现工具缺失或被替换。

`live-source-catalog.json` 只包含原规则；`live-catalog.json` 保留原 revision 并追加原有
多区间合成规则。两者仅适用于 BTC regtest 与专用 conformance scope。公开 catalog、
网络参数、JSON schema 和生产规则版本均不改变。

真实 Core/Ord 铸造 Leader 与两类协作 pass，BH/indexer 按旧 registry 索引至 BTC 159，
Geth 挖出一段旧 checkpoint 下的历史。停机后，以当时 USDB head + 1 作为追加 checkpoint，
调用生产 `node_protocol_upgrade.execute`，其两个离线服务命令均在真实 Docker 容器中
执行，使用只读根目录、隔离网络和预检只读数据卷；不注入服务结果。

`tests/common/protocol_upgrade_live.py` 提供校验过的 regtest 包和持久化会话适配器。
它不放宽公开网络 allowlist，也不模拟宿主 systemd、sudo 或安装流程。宿主会话边界继续
由 G2 节点工具测试覆盖；已发布包在真实运维主机上的安装、停机、升级和观察验收仍是发布前
单独步骤，不能用此 regtest 验收替代。

### 5.2 新增矩阵与证据

- BTC 159 原库接管后跨 160/163/166/170/172/180 续写；目录 inode 不变、原目录名消失，
  证明移动活跃数据集而非复制或重建。Geth 三类 head 与 genesis 保持不变。
- 真实子进程分别在 Geth apply、indexer apply、目录发布、初始化标记发布后立即退出。
  每次从磁盘日志重新加载，沿相同目标恢复；完成后重复执行仍幂等。
- BTC 旧规则已经执行至 160 的 indexer，以及 USDB 已经达到目标 checkpoint 的旧链，
  分别在两个预检阶段拒绝。逐文件摘要证明两个数据库均未变化，未开始任何元数据写入。
- 所有历史查询固定旧 registry 的 BTC 159 上下文，升级前后逐字段相等，包括审计、能量、
  profile、候选集、协作、聚合和 state ref。独立 validator 验证新旧 checkpoint 的完整历史。
- 独立奖励参考按 USDB checkpoint 选 registry，并按 BTC anchor 选公式；价格区间身份同样
  按完整 chain checkpoint 切换，即使价格公式本身没有改变。
- 保留已有 schema/继承/协作/转移/burn、普通构建拒绝合成规则、进程重启、旧/中段 checkpoint
  追赶、从 origin 重放、跨多区间重组和旧分支 header 拒绝；weekly 三个 seed 分别执行
  3/2/3 次深重组。

报告升级为 `miner-pass-live-upgrade:v2`，包含源码/构建/镜像身份、两个服务的预检凭证、
会话事件、各拒绝原因及文件摘要、前后查询、奖励与重放对比证据。
长时 CI 原有 diagnostics 会保留这些 JSON/JSONL/log 文件。

### 5.3 联调发现的只读 freezer 缺口

G2 的 LevelDB 夹具未创建实际运行节点的 `ancient/chain`。真实只读容器暴露了 Geth
仍以读写方式打开 `FLOCK` 和 table `.meta` 的问题。本批改为在既有锁文件上获取互斥锁、
只读打开元数据，并在只读模式明确拒绝不完整 index、metadata 或 content；不创建目录、
不初始化旧 metadata、不截断或清理残留文件。读锁仍排斥运行中的 writer。

回归同时覆盖空 freezer 的真实节点、已写入 ancient 的 genesis、只读文件权限、锁互斥、
缺失/空 metadata、损坏 index 和 dangling content。缺失旧 metadata 属于需要用原节点
诊断/初始化的情况，不自动归类为重建或网络重置。

### 5.4 本地验证（2026-10-07）

- 完整真实服务场景 seed 0 已通过；weekly 41/42/43 通过原 CI 入口全部通过，分别完成 3/2/3 次深重组。
- 使用发布 Go 1.18.5、Bitcoin Core 28.1、Ord 0.23.3 和当前 Rust 构建。
- indexer 395 通过 / 12 原有忽略，普通生产分派集成 1 通过；util 87 通过 / 2 原有忽略。
- Go rawdb、离线接管、LevelDB、USDB verifier、params、fork ID、合成规则 tag、Geth CLI
  与 vet 通过；节点协调 13 个测试、CI 分派 17 个、查询辅助 9 个、奖励参考 5 个通过。
- Rust workspace check、严格 Clippy、格式、Rust/Go golden 一致性、ShellCheck、发布片段
  和 diff 检查通过。

以上为本地隔离验收，未触发远端 CI，未推送、发布或修改在线节点。
