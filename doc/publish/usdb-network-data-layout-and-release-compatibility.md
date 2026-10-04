# USDB Network 数据布局与 Release 兼容契约

Status: implemented for new node configurations; legacy data migration remains explicit/manual.

本文定义 network generation、deployment release、协议激活和宿主机数据目录之间的边界。目标是允许同一
网络的 `rN` 升级安全复用数据，也允许新 network generation 在来源和 schema 完全一致时复用 BTC 派生数据，
同时禁止把不同 genesis 或不同索引语义的数据静默混用。

## 1. 四类身份

| 身份 | 示例 | 变化含义 | 默认数据动作 |
| --- | --- | --- | --- |
| network generation | `usdb-testnet-v0` | chain ID、genesis 或 block-0 identity 改变 | 新建 network state |
| deployment release | `usdb-testnet-v0-r4` | 同一网络的软件和发布 artifact 更新 | contract 相同才原地升级 |
| protocol activation | 某高度启用 policy v2 | 同一网络内按高度切换共识规则 | 由冻结 activation history 决定 |
| dataset contract | balance-history schema/source identity | 数据编码或派生语义改变 | mismatch 时 rebuild |

`vN` 不是普通软件大版本，`rN` 也不表达数据库 schema。数据库是否可复用只由 release manifest 中的
runtime compatibility contract 和数据目录内的 service marker 决定。

### 1.1 字段与 ID 速查

| 名称 | 含义 | 由什么组成或约束 | 变化后果 |
| --- | --- | --- | --- |
| `USDB_DATA_ROOT` | 单台宿主机的 USDB 持久化根目录 | 节点本地配置，默认 `~/.usdb`，也可使用专用数据盘 | 只改变宿主机位置，不改变任何网络或 dataset 身份 |
| `network-bundle-id` / `bundle-id` | 一套 USDB 网络代际的人类可读标签，例如 `usdb-testnet-v0` | 对应 bundle 冻结 chain/network ID、genesis、BTC source/origin/registry、bootstrap 与 trust artifacts | 改变 chain/genesis/block-0 identity 或执行网络重置时使用新 `vN` |
| `release-id` | 某 network bundle 的不可移动发布编号，例如 `usdb-testnet-v0-r4` | `<bundle-id>-rN`，并绑定代码 revision、image digest、bundle 和 runtime compatibility | 同一网络兼容升级时递增 `rN`，不因此新建链数据 |
| `chain_id` / `network_id` | EVM 交易签名域与 devp2p 网络标识 | 由 network bundle 冻结；当前通常取相同数值，但语义不同 | 任一变化都按新 network generation 处理 |
| `genesis_block_hash` | USDB 链 block 0 的确定性身份 | 由完整 genesis 内容计算 | 不同 hash 的节点不属于同一条链 |
| `btc-network-id` | 上游 Bitcoin 网络身份 | 例如 `btc-mainnet` 或隔离测试使用的 regtest identity | 不同 BTC 网络的数据绝不能复用 |
| `btc_rules_scope` / `rules_scope` | USDB 对 BTC 数据的独立规则域 | 不同 USDB 网络可以共享 `btc-mainnet` source，但分别冻结 catalog 和激活日程；旧格式隐式 `legacy` | 不同 scope 使用独立 indexer dataset 与 state identity |
| `storage-schema` | 单个服务持久化格式的显式版本 | 由该服务维护，例如 RocksDB column/layout 或 geth DB version | 不兼容变化必须升级 contract 并重建或执行已审核 migration |
| `source identity` | 生成 dataset 所依赖的不可忽略来源参数 | 例如 BTC network；indexer 还包括 rules scope、index origin 和精确 activation registry ID | 任一字段变化均不得静默复用旧 dataset |
| `service-contract-id` | 单个服务完整数据兼容契约的 SHA-256 | 规范化哈希 `service name + storage schema + optional data model + source/network identity` | ID 相同才允许该服务复用既有可写目录 |
| `balance-history-contract-id` | balance-history 的 `service-contract-id` | BTC network、RocksDB schema 与 balance-history data model | 用于 balance-history 数据目录和 marker |
| `indexer-contract-id` | usdb-indexer 的 `service-contract-id` | BTC network、rules scope、index origin、activation registry ID 与 indexer storage schema | 用于 indexer 数据目录和 marker；它不只是 derivation/source ID |
| `runtime-compatibility-id` | 一次 release 的全局运行时数据兼容契约 SHA-256 | data layout、全部 service contracts、mismatch/migration policy | `activate-release` 只有在该 ID 与各 marker 一致时才允许原地切换 image |
| `dataset identity marker` | 数据目录内的公开身份声明 | service、service contract ID 及完整 contract，文件名为 `.usdb-dataset-identity.json` | `doctor`、`up` 和 release 激活时用于拒绝错目录或错数据 |
| `snapshot-release-id` | 一份不可变 balance-history snapshot artifact 的发布标识 | snapshot manifest、目标 BTC state、格式和签名共同约束 | 只用于 artifact 缓存目录；安装前仍必须验证 record、签名和消费者 contract |

这里的 `contract` 是部署层的“持久化数据兼容契约”，不是 SourceDAO 智能合约、RPC 接口或某一份 UIP。
`service-contract-id` 也不是目录内容哈希；它回答的是“按当前服务规则，这个目录是否允许被复用”。

`bundle-id` 本身是稳定标签，不是由 USDB 协议参数计算出的 content hash，也不存在一个被它直接编码的单一
“USDB chain protocol ID”。它通过所对应的冻结 network bundle 和 release manifest 间接绑定初始链规则：

- `chain_id`、`network_id`、genesis block/hash 及 genesis 内冻结的初始 chain config；
- BTC network、USDB rules scope、`index_origin_height` 和 BTC activation registry ID；
- SourceDAO/bootstrap、snapshot trust keys、network environment 与其他 bundle artifacts；
- 上述文件的 SHA-256，以及 release 的 runtime compatibility contract。

同一网络中按已冻结 activation history 在未来高度启用新的 policy/UIP，不改变 `bundle-id`；若改变 genesis、
block-0 identity 或采用需要重置整条网络的不兼容规则，则创建新的 `vN` bundle。`release-id` 的 `rN` 只表示
同一 bundle 的部署发布迭代，不能用来承载网络重置。

## 2. 数据所有权

| 数据 | scope | 跨 `rN` | 跨 `vN` | 约束 |
| --- | --- | --- | --- | --- |
| Bitcoin Core datadir | BTC source | 是 | 可 | BTC network 与 storage contract 相同，且只有一个 writer |
| balance-history | BTC source + service model | 是 | 可 | RocksDB schema、data model、BTC network 相同 |
| usdb-indexer | BTC source + USDB rules scope | 条件可 | 条件可 | source、scope、index origin、精确 current registry ID、schema 全相同 |
| USDB chain DB | genesis | 是 | 否 | chain ID、genesis hash、geth DB contract 必须相同 |
| control-plane state | network bundle | 是 | 否 | bundle identity 必须相同 |
| snapshot/checkpoint | immutable artifact | 是 | 条件可 | signature、manifest 和消费者 contract 均通过校验 |
| RPC secret/private key | node/operator | 是 | 人工决定 | 不属于可重建 dataset，不得被 reset 自动删除 |

同一可变目录任何时刻只允许一个 writer。需要并行运行旧、新网络时，应使用独立目录或经过校验的
reflink/copy；不能让两个服务实例共享同一个可写 RocksDB、SQLite、LevelDB 或 Bitcoin datadir。

## 3. 默认宿主机布局

新配置使用 `usdb-node-data-layout:v2`：

```text
<USDB_DATA_ROOT>/
  datasets/
    bitcoin/<btc-network-id>/
    balance-history/<btc-network-id>/<balance-history-contract-id>/
    usdb-indexer/<indexer-contract-id>/
  artifacts/
    balance-history/<snapshot-release-id>/
  networks/
    <bundle-id>/
      usdb-chain/
      control-plane/
      secure/
```

具体绝对路径始终以 bundle-scoped `node.env` 为准。两个目录 ID 都是对应服务完整兼容契约的 SHA-256；
运维人员不应手工拼接或缩短它们。source/derivation identity 是 service contract 的组成部分，而不是这里
单独使用的目录 ID。

旧布局 `bitcoin/mainnet`、`balance-history`、`usdb-indexer`、`usdb-chain`、`control-plane` 仍可由原
release 读取，但新工具不会自动移动、复制或认领这些目录。升级到新 contract 时使用新 data root；旧数据的
迁移或归档必须在服务停止后按单独审核的操作执行。

## 4. Release compatibility contract

`usdb-release-manifest:v8` 保留 `runtime_compatibility`、不可变 CI qualification 证据及 v7 的 split v3
snapshot 的 required core / optional script-registry component binding：

v8 还绑定一次性 SourceDAO 工具镜像的 digest、源码 revision 和 provenance；该工具镜像不属于
Compose 常驻服务，也不改变既有数据目录身份。

- 每个服务的 storage schema；
- 影响数据语义的 BTC network、rules scope、index origin、精确 registry ID、chain ID 和 genesis hash；
- 默认 data layout version；
- `compatibility_id`，即上述规范化内容的 SHA-256；
- 当前开发阶段固定 `migration_support=none`、`mismatch_action=rebuild`。

`setup/configure` 将全局 `compatibility_id` 写入 `node.env`，并在每个服务目录创建
`.usdb-dataset-identity.json`。marker 只包含公开 schema/source identity，不包含 secret。

`doctor`、`up` 和 `activate-release` 必须同时校验：

1. release manifest 与 bundled network identity 一致；
2. `node.env` 的 layout/compatibility ID 与目标 release 一致；
3. 每个绝对路径等于目标 contract 推导结果；
4. 每个 dataset marker 与对应服务 contract 完全一致；
5. image 使用目标 release 冻结的 digest。

任一项不一致均失败关闭。`activate-release` 还要求可识别的旧安装包与目标包完整共识身份不变，
包括 genesis 文件 checksum、BTC origin、scope 和 registry；相同 genesis block hash 不充分。
可通过 `--from-kit` 指定准确旧包。它不执行 DB migration、不移动既有数据库、不启动服务；已有的显式 Ord 版本切换仍按其独立规则选择新目录。

`upgrade-plan` 提供只读差异与组件复用/重建计划；`upgrade-release` 默认预览，显式 `--execute` 才执行。
当前执行器只支持同 bundle、Bitcoin/BH 与启动契约不变的重建，链重置仅允许 `development-resettable`。
共识历史变化时必须同时重建 indexer、chain 和 control-plane，不能仅依据 chain service contract 判断链库可复用。
旧数据库改名或原地保留，私有配置先备份，持久日志支持恢复；新服务启动或新数据变化后拒绝自动回退。
执行器不会放宽 `migration_support=none`、修改数据库 marker 来假装兼容，也不提供正式网按高度升级迁移。
操作顺序和边界见[升级手册](../handbook/node/upgrade.md)。

### 4.1 规则域与冻结 Catalog

新 scope 使用 `btc_source.rules_scope` 与 `btc_source.activation_registry_id` 绑定规则历史；release identity 以 `btc_rules_scope` 记录非 legacy scope。对应的 indexer service contract 也纳入该字段。旧 bundle 缺省 scope 按 `legacy` 解释，序列化时不补入新字段，因此旧 runtime compatibility ID、dataset 路径和冻结文件哈希保持不变。

发布容器的选择参数是：

| 冻结来源 | 环境变量 | indexer `usdb` 配置 |
| --- | --- | --- |
| `btc_source.rules_scope` | `USDB_RULES_SCOPE` | `rules_scope` |
| `btc_source.activation_registry_id` | `BTC_ACTIVATION_REGISTRY_ID` | `activation_registry_id` |
| `artifacts.btc_activation_registry_catalog` | `BTC_ACTIVATION_REGISTRY_CATALOG_FILE` | `activation_registry_catalog_file` |

新 scope 的三项必须齐备。catalog artifact 声明 `path: artifacts/<名称>.json` 和完整 `sha256`；冻结 `network.env` 使用容器路径 `/network/<名称>.json`，由 indexer、snapshot loader 与 checkpoint recovery 只读挂载同一 bundle artifacts 目录。宿主机路径不能写入冻结容器配置。catalog 文件内的 `current_registry_id` 必须与网络 pin 精确一致；文件内容完整性哈希是发布约束，不额外成为 dataset 语义身份。

两个配置 renderer 在写入前验证选择条件。`node.env` 不得覆盖冻结的 scope、ID 或 catalog 路径，官方 launcher 清除调用者 shell 对这三个变量的覆盖。开发 Compose 使用外部 catalog 时需在开发 overlay 显式提供只读挂载；未挂载文件必须拒绝启动。

本批仅提供选择和隔离能力，发布 validator 仍固定现有 `testnet-v0` 的 legacy binding；未新增可发布的 v1 bundle。不得修改 v0 的冻结文件来试用新 scope。

### 4.2 DB 内部绑定与升级限制

indexer 除 host marker 外，在 pass SQLite `state_text` 和 energy RocksDB `meta` 内绑定 BTC source、rules scope、index origin 和精确 current registry ID。两库都必须与配置一致；只修改目录名、host marker 或配置不能授权复用另一 scope 的状态。

空数据集可首次绑定。非空且无绑定的旧数据只允许 legacy 兼容接管，该路径依赖旧部署可信的 source/origin provenance，不能把首次写入绑定当作旧数据来源的追溯证明。新 scope 必须重建独立数据集。若 current registry ID 变化，本批采取保守的独立 dataset / rebuild 策略；即便新 catalog 只追加未来记录，也没有因此获得在线 DB migration 能力。catalog 历史追加校验与已有数据可否复用是两项不同的检查。

scoped checkpoint 在 `data/rules-catalog.json` 携带导出时的完整 catalog，文件受现有 inventory、operation ID 和 manifest 签名保护。standalone verify 使用该文件复原 catalog，并验证 manifest registry ID 与 BTC source；恢复目标仍以本地冻结配置和双库绑定为准，不接受 artifact 替目标选择 scope。legacy checkpoint 不新增此文件，manifest schema 不变。

checkpoint 预检要求 manifest registry ID 与目标网络 pin 一致，安装与离线验证继续校验双库绑定和重算 state-ref。旧 legacy checkpoint 允许按兼容规则读取缺少绑定的旧数据，新 scoped checkpoint 必须带有完整绑定。未通过验证时不得把恢复结果当作可用索引状态。

### 4.3 按高度升级时的 Peer 兼容识别

USDB 的 fork ID 高度列表包含 `ChainConfig.usdb.activations[].block`，与顶层 EVM fork 高度统一排序、去重和去除 0。Status 握手与 ENR 使用相同计算；只改变 registry binding 的非零 checkpoint 也会进入该列表。BTC registry 的 BTC 激活高度不会被混入 USDB 高度列表。

对旧列表确实缺失的新增高度 H，升级节点在 H 前仍可能接受旧 peer；到 H 后，新握手会拒绝报告旧 checksum、`Next=0` 的节点。已升级但尚未同步到 H 的节点可以用 `Next=H` 继续追块。已经建立的连接不会仅因越过 H 自动断开，因此运维不能把连接数或握手成功当作全部 peer 已升级的证明。

fork ID 是 4 字节高度历史 checksum 加下一高度，不是 registry/版本内容身份。同高不同规则，或新增 checkpoint 与已有顶层 fork 同高，仍须由链配置和区块共识验证发现。不能据此承诺旧链自动停止，也不能跳过升级前的版本和激活配置核验。

这项软件改造本身不要求重置网络、重建数据或修改现有配置；安装对应链节点版本需要正常替换运行时并重启。当前 testnet-v0 只有 block 0 checkpoint，因此既有 fork ID 保持不变；本批没有修改或发布网络包。未来引入新的非零激活高度时，应在该高度前按已审议发布计划升级 miner 和 follower，网络身份与数据动作仍由该次变更的独立 compatibility contract 决定。

## 5. Reset、重建与恢复

从空机器部署在数据可用性上是成立的，但恢复来源不同：

- Bitcoin、balance-history、indexer 可由上游 BTC 数据或受信 snapshot/checkpoint 重建；
- 同一 USDB 网络的空 chain node 只能从仍存活的 peer 或受信 chain checkpoint 同步；如果所有节点同时清空，
  历史状态不会凭 BTC 自动恢复；
- 新 `vN` 从新 genesis 启动，不会自动继承旧网络余额、合约和 SourceDAO 状态；如需继承必须另行定义迁移；
- signer、keystore、RPC secret、审批记录和审计日志不可由链数据重建，reset 流程不得连带删除。

同文件系统且确认旧 writer 已停止时，人工迁移优先使用原子 rename。跨文件系统或需要并行验证时使用
reflink/copy，并在切换前后复核 marker、权限、大小和服务 state-ref。当前工具不自动执行这些动作。

## 6. 当前限制与后续条件

- balance-history 已在 DB 内部校验 schema、data model、BTC network 和 genesis；geth 校验 chain DB version
  与 genesis；Bitcoin Core 仍由自身 network/datadir 检查负责。
- usdb-indexer 由 host marker、确定性路径、双库 rules binding 和 checkpoint 验证共同约束；
  本批未实现 registry current revision 的在线数据迁移，也未重置或升级运行中的网络。
- 若未来支持在线 DB migration，必须增加独立 migration version、前后 state-ref、可中断恢复和 rollback
  矩阵；不能把 `migration_support=none` 直接改成宽松兼容。
