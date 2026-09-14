# P7.3：原生 node-kit、Compose 与 controller 接入

日期：2026-09-13。P7.2 分发兼容已提交为 `14248ce`，P7.3 编排已提交为 `3c71205`。

## 1. 实现边界

本阶段完成候选 bundle、release manifest、node-kit、配置校验、服务依赖图、controller 资源阶段与状态展示的原生接入。
现有 `docker/networks/testnet-v0` 没有切换默认模式；原生模式由新候选 bundle 的 `assumeutxo_bootstrap` artifact 选择。
安装器携带该 bundle，`setup/configure` 自动采用原生模式，不能用 `node.env` 改回旧快照或改写 B/G/hash。

本阶段的自动化验收使用临时 bundle、真实签名、真实 Compose 配置展开、模拟 RPC/容器状态及隔离 Core 容器。
它不等于主网全量 BH 重导入、真实 Go chain/control-plane 同锚点验收或旧28.1数据目录升级。
这些长任务继续归入 P6/P3 与 P7.4，不在本阶段自动执行。

## 2. 新启动顺序

| 阶段 | 启动条件与行为 | 资源 |
| --- | --- | --- |
| `bitcoin` | Core `prune=0` 启动；observer 下载/校验原始 UTXO，等待 headers，执行或恢复 `loadtxoutset` | Core 启动期额度 + 128 MiB observer；下游保持停止 |
| `overlap` | Core canonical B/hash 匹配且 UTXO 文件准备成功后，先安全停止 Core、提交新额度，再启动 Core、BH、indexer | Core/BH/indexer 并行预算；BH 导入和 B+1…G 重放可在前台未到 G 时开始 |
| `steady` | Core 前台追平、G/hash 匹配、tip 时间和 peers 合格，且 BH/indexer 实际 `consensus_ready=true` | 安全切换稳定额度，恢复服务，重新核对 readiness 后执行 chain-init 并启动 chain/control-plane |

Core 后台历史验证高度单独报告。后台未完成、txindex 未建立均不阻止原生前台服务；最终链启动仍要求实际 RPC 就绪。
`usdb-node` 的最终状态还保留原有链身份、网络成员和运行状态检查。
BH 的 `bootstrap-progress.json` 只用于显示；即使文件记录 `sealed`，也不能代替服务 RPC 就绪。

两次资源切换沿用持久化 `pending/recover_services` 记录。先停止依赖方，等待 Core 正常落盘退出，才应用新额度。
中断后依据实际容器状态继续；不会叠加 Bitcoin 启动期高额度与 BH 导入额度。
后台验证继续使用 Core 的稳定额度，可能影响后台完成速度，但不影响上述启动条件。
原生预算也暂时保留旧辅助任务的保守预留，不会实际启动这些任务。

## 3. 发布端准备原生 bundle

这里生成的是小型部署配置包：继承网络/genesis 配置，增加 `assumeutxo-bootstrap.json`，并更新相关身份和哈希。
**不会生成、改写或复制原始 UTXO 文件，也不会生成旧 BH core/registry 数据库快照。**
UTXO 自有签名是另外一个可选的发布步骤，只为同一原始 `.dat` 生成 manifest 和签名。

| 原始 UTXO 分发选择 | 原生 bootstrap 配置 | UTXO manifest/signature/public catalog |
| --- | --- | --- |
| `pinned`：公开源、自有镜像或预置本地文件 | 必须，由发布端生成并装入 release | 不需要，不生成；三个引用均为 `null` |
| `usdb-signed`：经 USDB 签名的同一原始文件 | 必须，由发布端生成并装入 release | 必须，由发布端签发、核对并装入 bundle |

模式决定信任方式，与网站归属不等价：自有镜像也可以使用 `pinned`。
`pinned` 核对内置文件 SHA-256，后续 Core/BH 继续核对快照承诺；没有额外要求上游 UTXO 发布者签名。
“三名固定上游签名者”是 Core 二进制发布包的验证规则，不是 UTXO 文件的签名格式。
Core 31.1 已在 Docker 构建时固定并采用上游校验；这两种 UTXO 模式均不要求 USDB 为 Core 二进制签名或生成 Core 密钥。
自有签名 UTXO 已接入原 snapshot signer 和 `create/finalize/publish`，直接复用旧密钥文件，普通节点不执行密钥生成。
两种模式均保留原有 release/OCI 交付校验和每台 Core 的官方后台历史验证。

以下命令由**发布端**在源码目录执行一次，生成不存在的新目录。当前测试网 G=963800。
普通节点拿到已包含这些配置的安装包后，直接执行第4节，无需源码、签名私钥或再次生成 bundle。

### 3.1 公开源 / pinned

```bash
USDB_REPO=/home/bucky/work/usdb
P73_BUNDLE=/tmp/usdb-testnet-v0-assumeutxo-candidate
# 空值表示节点提前放置文件；自动下载时填已选定的完整 HTTPS .dat 地址。
P73_SOURCE_URL=
python3 "$USDB_REPO/docker/scripts/tools/assumeutxo_deployment.py" \
  --source-bundle "$USDB_REPO/docker/networks/testnet-v0" \
  --output-dir "$P73_BUNDLE" \
  --source-url "$P73_SOURCE_URL" \
  --origin-block-hash 000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551
python3 "$USDB_REPO/docker/scripts/tools/validate_network_bundle.py" \
  --bundle-dir "$P73_BUNDLE"
```

不提供 `--manifest-file/--trusted-keys` 即选择 `pinned`。此步骤只生成配置，不下载或扫描 `.dat`；节点使用文件时再验证。

### 3.2 自有签名 / usdb-signed

先按[分发手册第2节](./balance-history-bitcoin-artifact-distribution.md)准备 UTXO manifest/signature 和公开 catalog。
已有 `snapshot-keys` 时按[UTXO 发布手册](./balance-history-assumeutxo-snapshot-publish-operations.md)执行原脚本的
`create/finalize/publish --snapshot-type assumeutxo` 分支，无需重新生成密钥。
成功后的 `publish-result.json` 提供下列参数需要的 `manifest_file/trusted_keys_file/source_url`。
**推荐继续用同一 `SNAPSHOT_SCRIPT deploy` 生成候选并登记源码发布输入**，见[发布手册第5节](./balance-history-assumeutxo-snapshot-publish-operations.md#5-状态deploy-与节点装包)。
该入口自动读取并复核成功发布记录，将三个公开输出传给本脚本；只需指定基础 bundle、新输出目录和 G 的 block hash。
它默认写入基础 bundle 下的 `release-bootstrap.json` 和 `release-inputs/`，供正式 candidate/publish 自动重建；
`--prepare-only` 只导出候选。它不部署节点或发布安装器，也不重复下载/扫描原始 UTXO。
使用该入口后，不必再执行下面的等价底层命令。
独立 `bitcoin_release.py prepare-utxo` 仍可使用；显式 `--reuse-snapshot-key` 接受旧私钥格式，但公钥目录需要先转为 UTXO 用途，统一 wrapper 会自动完成。
以下装包步骤消费这些公开输出，与私钥是否复用无关，也不重新生成 UTXO。
以下变量分别指向准备好的公开 manifest、经审查的公钥目录、未使用的新 bundle 输出目录；`P73_SOURCE_URL` 与上一节含义相同：

```bash
: "${P73_UTXO_MANIFEST:?Set the generated manifest absolute path}"
: "${P73_UTXO_TRUST:?Set the approved public key catalog absolute path}"
: "${P73_SIGNED_BUNDLE:?Set a new signed bundle output directory}"
python3 "$USDB_REPO/docker/scripts/tools/assumeutxo_deployment.py" \
  --source-bundle "$USDB_REPO/docker/networks/testnet-v0" \
  --output-dir "$P73_SIGNED_BUNDLE" \
  --origin-block-hash 000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551 \
  --source-url "$P73_SOURCE_URL" \
  --manifest-file "$P73_UTXO_MANIFEST" \
  --trusted-keys "$P73_UTXO_TRUST"
python3 "$USDB_REPO/docker/scripts/tools/validate_network_bundle.py" \
  --bundle-dir "$P73_SIGNED_BUNDLE"
```

工具先验签并核对内置快照身份，再将三份公开材料原样复制到固定包内路径，不在此处重新签名。
节点使用的是包内本地 manifest；需要自动下载 `.dat` 时必须提供完整 `--source-url`，不能从这个本地路径推导远程源。
地址为空时仍要求节点预置文件。上述两个生成流程都不上传文件或发布 release。

### 3.3 配置与固定包内路径

生成的 `artifacts/assumeutxo-bootstrap.json` 固定以下内容，并被 `network.json` 和 release manifest 哈希绑定：

- B、base hash、文件 SHA-256、UTXO 承诺及 G/hash。
- `pinned` 或 `usdb-signed` 来源模式和文件 URL。
- 自有源的公开 manifest、分离签名及 trusted-keys 文件哈希。

仅 `usdb-signed` 需要以下三个文件，格式和签名编码见[分发材料格式](./balance-history-bitcoin-artifact-distribution.md#41-原生-bundle-中的公开材料格式)：

| 包内路径 | 内容与生成来源 |
| --- | --- |
| `artifacts/assumeutxo-distribution.json` | `prepare-utxo` 输出的 canonical manifest，原样改名复制 |
| `artifacts/assumeutxo-distribution.json.sig` | 相邻的64字节 Ed25519 原始二进制签名，原样复制 |
| `trust/bitcoin-artifacts.trusted-keys.json` | 经审查的 UTXO public catalog；原 snapshot 公钥由发布 wrapper 自动转换，新 signer 也可直接生成 |

`pinned` 的 `distribution` 配置如下；`snapshot`、`origin_height`、`origin_block_hash` 在同一个 bootstrap JSON 中由工具填入：

```json
{
  "mode": "pinned",
  "source_url": "",
  "manifest": null,
  "signature": null,
  "trusted_keys": null
}
```

`usdb-signed` 将 `mode` 设为该值，并将三个 `null` 替换为对应文件的 `{"path":"包内路径","sha256":"文件SHA256"}`。
`assumeutxo-bootstrap.json` 的 schema 是 `usdb-assumeutxo-deployment:v1`；外部配置不能注入 C(B)/D(B)，它们仍由服务代码内置。
不会把私钥装入 node-kit。离线重启验证包内签名，不重新访问 manifest 服务。
调整来源/公钥需生成新候选 release；它不改变 BH 数据身份。改变 B/G/hash 则产生不同 BH 数据目录。

将候选 bundle 传入现有 release manifest 生成和 `prepare_release_node_kit.py --bundle-dir` 流程；
manifest 必须由该候选 bundle 生成，不能复用旧 manifest 后手改 checksum。
镜像需包含本阶段代码并按既有流程固定 digest，尤其 Core 镜像需要新的 `--ensure-snapshot-file` 与 `--require-tip` 探针。
**正式 candidate/publish workflow 已通过 `release_bundle.py` 消费源码中的发布输入。**
`SNAPSHOT_SCRIPT deploy` 登记的小型配置和公开材料随代码提交、进入 release tag 后，两条 workflow 从同一源码重建原生 bundle，
并用于 manifest、发布复核、归档和 node-kit。原生公开校验核对 UTXO record/签名材料和 Range 下载，旧模式保留原 BH record 校验。
登记缺失时使用历史基础 bundle；登记存在但损坏时阻断，不回退。
仅使用本节底层命令生成 `/tmp` 候选仍属于独立导出；自有签名正式打包应使用 `deploy` 完成源码登记。
随后按原发布流程生成用于节点验收的真实安装包，不在节点手改 bundle、旧 release manifest 或 compatibility ID。

## 4. 在验收节点配置与运行

验收使用正式的 `installer -> setup -> doctor -> up -> status` 流程，不另建一套临时节点部署入口。
可以在同一台测试机归档旧基线、按确认的范围重置后串行执行，具体见[单机复用评估](./balance-history-assumeutxo-p74-node-reuse-plan.md)。
以下是第3节发布链路接通、原生安装包就绪之后的操作；目前不应直接在运行中的旧节点执行。
先按[正式安装手册](../publish/usdb-release-node-kit-and-deployment.md)安装选定 release，随后使用安装器提供的入口：

```bash
usdb-node setup --resource-mode auto
usdb-node resources --json
usdb-node doctor
```

`setup` 不再询问是否使用旧 BH 数据库快照。配置生成两个独立路径：

| node.env 字段 | 默认布局 | 用途 |
| --- | --- | --- |
| `BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR` | `<data-root>/artifacts/assumeutxo/mainnet-935000` | 原始 `.dat`、下载 partial/校验进度；Core/BH 只读，observer 可写 |
| `BTC_ASSUMEUTXO_STATE_HOST_DIR` | `<data-root>/networks/<bundle-id>/assumeutxo` | Core 导入请求、恢复与来源记录 |
| `BH_DATA_HOST_DIR` | `<data-root>/datasets/balance-history/btc-mainnet/<native-contract-id>` | 原生 BH 导入、重放、发布状态及在线数据库 |

若候选未配置下载 URL，应在启动前将已有 `/data/btc/mainnet-935000-utxos.dat` 放入生成的 artifact 目录，保持同名且容器用户可读。
无需解包或转成旧 BH/core/registry 数据库。observer 会检查文件大小和完整 SHA-256，BH 导入还会独立验证状态承诺。
本阶段保留原始 `.dat`：即使 Core 已完成历史验证，observer 仍确保 BH 的原始导入输入可用。
重启后的文件校验可能扫描约9.39GB，但不会因此重复发送 `loadtxoutset`；已 sealed 的 BH 自身直接恢复在线状态。

```bash
usdb-node up
usdb-node status --progress-json
usdb-node status --watch
```

默认 `up` 由已有 systemd controller 托管；退出观察窗口只停止观察。
需要终端内验证时使用 `up --foreground --sync-timeout-secs 604800`。观察超时不删除下载/导入数据，也不取消 Core 已提交的导入。
完整 blocks/undo、Core 双 chainstate、UTXO 原始文件、BH staging/live、indexer 和 chain 均需计入磁盘空间，沿用前述全节点空间预留要求。
当前 `setup/configure` 要求数据根所在文件系统**总容量和可用空间均至少1.5TiB**，推荐2TiB；
复用磁盘时不能只看删除 BH 后的空闲量或磁盘标称容量。已有 `node.env` 时 `setup` 会拒绝，旧模式到原生模式也不是同契约的 `activate-release`。
配置、凭据、controller 和数据目录的重置范围需要在单机复用步骤中明确。

## 5. 进度与恢复

`status --progress-json` 保留现有 schema，增加 `native_bootstrap`：

- `download`：下载/校验字节、阶段、开始/更新时间、实际 elapsed。
- `activation`：等待 RPC/headers、请求导入、Core 激活或已验证状态，以及来源校验记录。
- `import_progress`：从本机 Core `debug.log` 读取本次进程/导入阶段的 UTXO 数量、读取百分比、落盘与校验阶段；仅供展示，不作为就绪依据。
- `core`：foreground 高度、headers、background 高度、`history_validated`、`tip_ready`。
- `balance_history`：imported coins、replay height/target、waiting/sealed、服务写出的实际 elapsed。

部分旧观察记录可能没有 elapsed，界面不会补造预计完成时间。`waiting_for_blocks` 是等待后续块或 undo，不代表需要后台验证结束。
下载完成后，UTXO 行切换为 Core 导入阶段：读取时显示 Core 报告的 UTXO 数量和百分比，
批量落盘、最终落盘和 UTXO 哈希校验时显示 `in progress` 及阶段耗时，不把读取完成的 100% 当作快照就绪。
只有 Core RPC 确认基线可用且准备容器正常退出后才显示 `READY`。日志缺失、权限不足或最近日志中没有导入记录时，
显示详细进度不可用，保留 RPC/observer 的真实状态；不会阻止服务启动或重试导入。
观察器按增量读取日志，首次/落后时最多读取末尾 8 MiB，日志轮转或 Core 重启会重置观测。
快照激活前 Bitcoin 行标为 `Bitcoin (IBD)`，并注明尚未激活快照，表示普通区块同步；其区块百分比独立于 UTXO 导入进度。
激活后恢复前台追平和后台历史验证的独立显示。
Core RPC 临时不可用时，已经完成的导入不会退回 `WAITING`：该次观察显示 `UNKNOWN`，
连续 watch 在有效期内保留同一 Core 进程/导入尝试最近确认的百分比并标注 `STALE`；
无历史观察或超出有效期则显示 `unavailable`。这不构成就绪证据，controller 仍使用实时 RPC 判定。

BH 区块进度统一为 `(已处理高度 - 935000) / (BTC 最新稳定高度 - 935000)`，
稳定目标按 release activation registry 的确认滞后从 BTC headers 高度扣除；界面同时显示起点和确认块数。
配置了同步高度上限时，目标仍受该上限约束。
963800 是单独的 genesis 里程碑，依次显示重放、校验、封存等待服务 RPC、可查询。
到基线后仍使用同一个区间追平，不重新从零计算；新增 BTC 区块会使目标小幅移动。
RPC 暂不可用时可使用标注为“上次观测”的 headers 目标，未知目标不伪造为 genesis 或 `0/0`。
基线已封存后，BH RPC 超时会保留有效期内最后观测的高度并标注 `STALE`，不把旧 bootstrap journal 的高度当作当前高度。

indexer 进程已启动但尚无上游可查询状态时显示 `WAITING`，隐藏无意义的 `0/0` 和同步 ETA。
BH 完成基线校验、发布并开放查询后，indexer 可从 genesis 开始，跟随 BH 已发布的稳定高度并行追赶。
`UpstreamSnapshotMissing` 指 BH 的稳定状态信息，不是需要重新下载历史数据库快照。
`Process elapsed` 是进程运行时间；等待上游也计入，不能理解为已执行索引的时长。
旧 snapshot-loader/registry/paired-checkpoint marker 不参与新模式启动条件；反向 script 查询来自 BH 原生 observed-script registry。

若 observer 下载失败或退出非零，controller 停止推进并保留证据。先查看日志，再显式重试。
将下面两个变量设为此次 node-kit 的 bundle 和私有配置路径：

```bash
: "${P73_KIT:?Set the installed native node kit absolute path}"
: "${P73_NODE_ENV:?Set the native node.env absolute path}"
export USDB_TESTNET_BUNDLE_DIR="$P73_KIT/docker/networks/testnet-v0"
export USDB_TESTNET_NODE_ENV="$P73_NODE_ENV"
P73_BITCOIN="$P73_KIT/docker/scripts/tools/run_testnet_bitcoin.sh"
"$P73_BITCOIN" logs btc-snapshot-bootstrap
# 退出日志观察后，重试下载/观察，并让 controller 继续。
"$P73_BITCOIN" bootstrap-start
usdb-node --node-env "$P73_NODE_ENV" up
```

若 activation 记录 `load_uncertain`，先按 P7.2 手册核对 Core 的 `getchainstates/getrpcinfo` 和日志。
普通重试不会再次提交不确定的 load 请求；确认没有活动导入且确需重新提交后，使用
`"$P73_BITCOIN" bootstrap-start --retry-interrupted-load`，再继续 `up`。
不用删除 activation 记录或 BH staging 来“解锁”。错误 B/hash、坏签名、坏文件、错误资源额度均在相应阶段阻止继续。

## 6. 本阶段验证与下一步

自动化覆盖：原生候选/manifest/node-kit 的完整生成、包外独立加载、私有配置与身份篡改拒绝、真实签名打包、
真实 Compose 依赖图及只读挂载、真实 shell helper 的三重启动条件、资源阶段切换和中断恢复、Core 前后台独立就绪、进度记录不能代替服务就绪。
现有部署工具回归也覆盖旧模式，检查命令已接入 Fast CI。

本轮本地结果：部署工具394项、AssumeUTXO/原生编排37项、分发14项，共445项通过；Shell 语法、
actionlint、旧默认 bundle 校验和 `git diff --check` 通过。
本地 `usdb-bitcoin-core:p73-native` 镜像 ID 为
`sha256:40ad0feb4bfe90ead8e79fcd461b13058bb1067d07ff667ec0704babb85cfc60`。
禁网临时 Core 容器验证31.1版本、`prune=0`、无 txindex、原生 baseline/foreground 探针及安全退出，
报告在 `/tmp/usdb-p72-container-ftw6an29/result.json`（复用 P7.2 验收脚本的目录前缀）。
镜像只保存在本地，没有发布；该容器停留在主网 genesis，没有导入真实935000快照。

P7.4 再安排主网长任务：实际 Core 下载/导入与 BH 全量重放、同一锚点的 BH/indexer/Go chain/control-plane 验收、
正式安装流程的冷启动、离线重启、下载与导入中断、磁盘不足、旧 Core 数据升级和新旧 release 选择。
主网验收优先采用一台现有测试机串行重建，旧节点比较证据先归档，不要求同时保留两套主网服务。
归档实际镜像 digest、release/bundle 身份、耗时、峰值资源和最终 RPC 状态后，再决定默认部署切换与正式发布。
