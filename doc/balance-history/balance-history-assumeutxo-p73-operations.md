# P7.3：原生 node-kit、Compose 与 controller 接入

日期：2026-09-13。P7.2 分发兼容已提交为 `14248ce`。

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

## 3. 生成原生候选 bundle

以下准备步骤在源码目录执行，生成新目录，不修改已发布 bundle。当前测试网 G=963800。

```bash
USDB_REPO=/home/bucky/work/usdb
P73_BUNDLE=/tmp/usdb-testnet-v0-assumeutxo-candidate
python3 "$USDB_REPO/docker/scripts/tools/assumeutxo_deployment.py" \
  --source-bundle "$USDB_REPO/docker/networks/testnet-v0" \
  --output-dir "$P73_BUNDLE" \
  --origin-block-hash 000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551
python3 "$USDB_REPO/docker/scripts/tools/validate_network_bundle.py" \
  --bundle-dir "$P73_BUNDLE"
```

不传 `--source-url` 表示操作员提前放置已下载的原始文件；需要下载时，传入完整 HTTPS 文件地址。
自有签名分发同时提供 `--manifest-file`、`--trusted-keys`，材料由[分发手册](./balance-history-bitcoin-artifact-distribution.md)生成；
仍可传 `--source-url` 指定镜像站上同一原始文件。工具先验证用途为 `bitcoin-assumeutxo` 的 Ed25519 签名及编译内置快照身份。

生成的 `artifacts/assumeutxo-bootstrap.json` 固定以下内容，并被 `network.json` 和 release manifest 哈希绑定：

- B、base hash、文件 SHA-256、UTXO 承诺及 G/hash。
- `pinned` 或 `usdb-signed` 来源模式和文件 URL。
- 自有源的公开 manifest、分离签名及 trusted-keys 文件哈希。

签名材料采用固定包内路径 `artifacts/assumeutxo-distribution.json[.sig]` 和 `trust/bitcoin-artifacts.trusted-keys.json`；
不会把私钥装入 node-kit。离线重启验证包内签名，不重新访问 manifest 服务。
调整来源/公钥需生成新候选 release；它不改变 BH 数据身份。改变 B/G/hash 则产生不同 BH 数据目录。

将候选 bundle 传入现有 release manifest 生成和 `prepare_release_node_kit.py --bundle-dir` 流程；
manifest 必须由该候选 bundle 生成，不能复用旧 manifest 后手改 checksum。
镜像需包含本阶段代码并按既有流程固定 digest，尤其 Core 镜像需要新的 `--ensure-snapshot-file` 与 `--require-tip` 探针。
默认发布 workflow 的 bundle 选择尚未切换，属于 P7.4 发布准备。

## 4. 在验收节点配置与运行

以下在安排好的新验收节点执行，使用上一步生成且镜像已准备好的 node-kit；不直接套用到当前开发机正在运行的服务。
安装方式沿用现有 node-kit 安装手册，也可直接调用包内入口。将 `P73_KIT` 设为该包的绝对目录。

```bash
: "${P73_KIT:?Set the absolute path of the prepared native node kit}"
P73_TOOL="$P73_KIT/docker/scripts/tools/usdb_node.py"
python3 "$P73_TOOL" setup --resource-mode auto
python3 "$P73_TOOL" resources --json
python3 "$P73_TOOL" doctor
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
python3 "$P73_TOOL" up
python3 "$P73_TOOL" status --progress-json
python3 "$P73_TOOL" status --watch
```

默认 `up` 由已有 systemd controller 托管；退出观察窗口只停止观察。
需要终端内验证时使用 `up --foreground --sync-timeout-secs 604800`。观察超时不删除下载/导入数据，也不取消 Core 已提交的导入。
完整 blocks/undo、Core 双 chainstate、UTXO 原始文件、BH staging/live、indexer 和 chain 均需计入磁盘空间，沿用前述全节点空间预留要求。

## 5. 进度与恢复

`status --progress-json` 保留现有 schema，增加 `native_bootstrap`：

- `download`：下载/校验字节、阶段、开始/更新时间、实际 elapsed。
- `activation`：等待 RPC/headers、请求导入、Core 激活或已验证状态，以及来源校验记录。
- `core`：foreground 高度、headers、background 高度、`history_validated`、`tip_ready`。
- `balance_history`：imported coins、replay height/target、waiting/sealed、服务写出的实际 elapsed。

部分旧观察记录可能没有 elapsed，界面不会补造预计完成时间。`waiting_for_blocks` 是等待后续块或 undo，不代表需要后台验证结束。
旧 snapshot-loader/registry/paired-checkpoint marker 不参与新模式启动条件；反向 script 查询来自 BH 原生 observed-script registry。

若 observer 下载失败或退出非零，controller 停止推进并保留证据。先查看日志，再显式重试。
将下面两个变量设为此次 node-kit 的 bundle 和私有配置路径：

```bash
: "${P73_NODE_ENV:?Set the native node.env absolute path}"
export USDB_TESTNET_BUNDLE_DIR="$P73_KIT/docker/networks/testnet-v0"
export USDB_TESTNET_NODE_ENV="$P73_NODE_ENV"
P73_BITCOIN="$P73_KIT/docker/scripts/tools/run_testnet_bitcoin.sh"
"$P73_BITCOIN" logs btc-snapshot-bootstrap
# 退出日志观察后，重试下载/观察，并让 controller 继续。
"$P73_BITCOIN" bootstrap-start
python3 "$P73_TOOL" --node-env "$P73_NODE_ENV" up
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
新机器安装、离线重启、下载与导入中断、磁盘不足、旧 Core 数据升级和新旧 release 选择。
归档实际镜像 digest、release/bundle 身份、耗时、峰值资源和最终 RPC 状态后，再决定默认部署切换与正式发布。
