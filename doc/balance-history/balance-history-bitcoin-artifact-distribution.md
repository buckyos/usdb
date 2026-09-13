# AssumeUTXO 快照分发与 Core 镜像构建说明

日期：2026-09-13。P7.2 基础编排已提交为 `eb6c8c2`；本文描述其后的分发兼容补充。

## 1. 本次部署采用的流程

本文的“Core 二进制发布包”指官方 `bitcoin-31.1-x86_64-linux-gnu.tar.gz`，包含 `bitcoind`、`bitcoin-cli` 等程序。
它在 **Docker 镜像构建阶段**下载、验证并安装。节点运行时拉取固定 digest 的 Core 镜像，不另行安装或签发这份二进制包。

本次 AssumeUTXO 改造保持现有 installer、setup、镜像交付及节点管理入口，主要变化是：

- Core 镜像固定升级到31.1，继续使用官方发布包的上游签名和 SHA-256 校验；无需 USDB Core 签名密钥。
- 节点新增原始 AssumeUTXO `.dat` 的下载/校验、Core 导入，以及 BH 从同一文件执行原生 bootstrap。
- 原生模式省去旧 BH core/registry 数据库快照的下载和安装，由已有 controller 处理新启动顺序和就绪条件。
- UTXO 采用 `pinned` 时无需签名；自有签名分发复用现有 `snapshot-keys` 和 `create -> finalize -> publish`，见第2节。

当前 `docker/bitcoin-source.json` 是 `upstream`，Dockerfile 固定31.1；旧 Dockerfile 同样在构建时验证上游签名和文件哈希。
这里的上游校验不要求我们生成密钥。UTXO 的分发模式也不会改变 Core 的构建来源。
普通部署按[P7.3 手册](./balance-history-assumeutxo-p73-operations.md)执行；只有自有签名 UTXO 的发布负责人需要第2、4节。
第3节说明镜像构建，Core 自有签名能力另列附录，不作为 P7.4 部署或发布前置条件。

| 对象 | 模式 | 验证规则 |
| --- | --- | --- |
| Core 二进制发布包 | `upstream`，当前采用 | 镜像构建时校验三份固定上游签名、归档大小和 SHA256，无需 USDB 密钥 |
| 原始 UTXO | `pinned`，默认 | 本地文件或任意合规 HTTPS 源；完整文件 SHA256 必须匹配内置检查点 |
| 原始 UTXO | `usdb-signed` | 先验证 USDB manifest/signature，再使用原有断点下载、文件校验和 Core 导入流程 |

模式在操作开始前确定，验签失败不会自动切换来源或降级。Core 二进制的下载校验发生在镜像构建时；
普通节点仍拉取发布的 OCI 镜像。镜像自身的 digest/provenance 与这里的二进制签名分属不同层次。

UTXO 签名绑定网络、B/hash、文件大小/SHA256、`hash_serialized_3`；接收端逐项匹配 Rust 使用的检查点目录。
它不能替换 Core 内置 UTXO 承诺，不能修改 BH 内置 C(B)/D(B)，也不会省略每个节点的官方后台历史验证。

## 2. 可选：UTXO 自有签名与现有 snapshot-keys 复用

UTXO 选择 `usdb-signed` 时，优先复用现有 snapshot signer、私钥文件和上传环境，不要求生成新 UTXO 密钥，更不要求生成 Core 密钥。
公开源或自有镜像使用 `pinned` 时跳过签发步骤。

### 2.1 已接入现有快照发布流程

现有 `src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh` 新增 `--snapshot-type assumeutxo`，
支持 `create -> finalize -> publish`，不传类型时继续执行原 BH 流程。
完整命令见[UTXO 发布操作手册](./balance-history-assumeutxo-snapshot-publish-operations.md)。

| 项目 | 已实现的复用方式 |
| --- | --- |
| 私钥 | 直接读取 `SNAPSHOT_KEY_ROOT` 下 `<SNAPSHOT_SIGNER_ID>.signing-key.json`；保留相同 Ed25519 seed、公钥和 signer ID |
| 公钥 | 从原可信 catalog 自动生成带 `bitcoin-assumeutxo` 用途的公开 catalog；不改写原 catalog 或私钥 |
| 命令入口 | 相同 wrapper/环境变量，UTXO 分支无需旧 builder、全量 BTC 节点或 `init/keygen` |
| 上传环境 | 复用 AWS CLI 客户端、原 profile、bucket、endpoint、公开域名和 multipart 参数 |
| 对象路径 | 相同 bucket 下使用 `bitcoin/utxo/935000/<finalization-sha256>/`，旧 BH 对象不变 |

旧 Rust 与新 Python 工具使用相同的32字节 Ed25519 seed/public key 编码。
`artifact_signing.py` 仅在显式 UTXO 复用途径中适配旧私钥格式，在内存中补充用途信息；先核对私钥导出的公钥与原可信目录一致，
再扫描文件并签发。原私钥、key ID 和 public catalog 原文件都保持不变。普通 Core 签发入口不接受这种旧格式适配。
旧 core/registry 快照共用 snapshot signer 并区分签名域，UTXO 同样使用独立的
`usdb-bitcoin-artifact:v1:bitcoin-assumeutxo` 签名域；旧签名不能用于新 manifest。

| 阶段 | UTXO 分支行为 |
| --- | --- |
| `create` | 校验已有或下载的 `.dat`，使用原 snapshot key 签发 manifest；以原子目录发布小型公开材料，不复制已有 `.dat` |
| `finalize` | 重新扫描文件并验签，冻结快照身份、签名/公钥目录哈希及发布工具 revision/source hashes；重复执行保留原记录 |
| `publish` | 复核 finalized 输入，幂等上传并逐个核验匿名 HTTPS 内容，最后发布不可变记录并核验该记录；成功后写 `publish-result.json` |

UTXO 使用单独的 `usdb-assumeutxo-release-record:v1`，记录位于 `snapshot-records/assumeutxo/v1/<sha256>.json`。
这是对原发布入口的类型扩展，底层沿用 `snapshot_distribution.AwsCliClient` 与不可变对象上传检查；旧 split v3 DB record 的校验语义不变。
发布结果提供 `source_url/manifest_url/manifest_file/trusted_keys_file`，可直接用于原生 bundle 装包。
节点继续验证现有的 UTXO manifest schema，不需要接收旧私钥或改变验签格式。

仓库默认上传配置仍是 bucket `usdb-snapshot`、AWS profile `usdb-snapshot-publisher`、公开域名 `usdb-snapshot.tbudr.top`。
本批没有读取线上私钥/AWS 凭据或上传线上对象；实际发布由操作员按手册执行。

### 2.2 已实现的独立签发工具与格式

以下保留现有工具的格式说明。单独新建 signer 的命令适用于没有可复用 snapshot key 的发布环境；
本项目已有密钥时，使用第2.1节兼容入口，不执行下面的 keygen，也不手工改写原私钥来迁就 schema。
入口：`docker/scripts/tools/bitcoin_release.py`。UTXO 签发需要 Python 3 和 OpenSSL。

- manifest 使用 `usdb-bitcoin-artifact:v1`，严格字段、无重复 JSON key、排序紧凑 JSON 和末尾 LF。
- Ed25519 签名对象为 `usdb-bitcoin-artifact:v1:<artifact_type>\0` 加 canonical manifest bytes。
- UTXO 的 `artifact_type` 固定为 `bitcoin-assumeutxo`；复用旧 snapshot signer 后也保持此签名用途。Core 自有签名属于另一权限域。
- manifest 文件名为 `<manifest-sha256>.json`，旁边是64字节原始签名 `<manifest-sha256>.json.sig`。
- manifest 包含文件名、大小、SHA256、身份、`signing_key_id`；UTXO 的 `upstream_verification` 为 `null`。
- 接收方可信目录使用独立 schema `usdb-bitcoin-artifact-trust:v1`，每个公钥绑定一个用途。
  当前新版 catalog 内禁止重复 key ID 或重复公钥条目；旧文件格式需要第2.1节的显式适配，这不禁止复用原 snapshot 密钥材料。

自有签名工具与独立 overlay 的默认 USDB 可信公钥文件为 `docker/trust/bitcoin-artifacts.trusted-keys.json`，随镜像复制到
`/opt/usdb/docker/scripts/data/trust/bitcoin-artifacts.trusted-keys.json`。
P7.3 原生 node-kit 的 UTXO observer 使用 bundle 内独立绑定的 public catalog，随 release 校验、挂载；不要求通过修改镜像内目录切换 UTXO 公钥。
**当前目录为空，不包含生产或测试签名者。** 空目录不影响默认 Core 上游构建或 UTXO `pinned` 模式。
选择 UTXO 自有签名后，由发布负责人核对复用或新建的 signer 公钥并将对应公钥目录纳入代码 review。
签名私钥只留在发布环境，工具要求文件权限排除 group/other，Docker build context 排除工具的私钥文件名。
公钥不能从待验证的同一下载源临时获取并直接信任。

仅在确实新建 signer 时使用以下示例（由发布负责人在受控环境执行，输出目录必须不存在）：

```bash
cd /home/bucky/work/usdb
DIST_TOOL="$PWD/docker/scripts/tools/bitcoin_release.py"
PUBLISH_ROOT=/data/usdb-bitcoin-artifacts
TRUST="$PWD/docker/trust/bitcoin-artifacts.trusted-keys.json"
umask 077
python3 "$DIST_TOOL" keygen --artifact-type bitcoin-assumeutxo \
  --key-id usdb-bitcoin-utxo-v1 --output-dir /secure/usdb-keys/bitcoin-utxo-v1
```

输出目录产生 `signing-key.json` 和 `trusted-keys.json`。将该 UTXO public catalog 的 `keys` 条目经审查后纳入
仓库可信目录，保留 schema；不要复制私钥。轮换时使用新 key ID、新密钥，先交付包含新旧公钥的可信目录，再切换签名者。
使用镜像内 catalog 的入口要更新镜像；原生 UTXO 使用 bundle catalog，要生成并交付新 release。
撤下旧公钥同样需要更新对应交付物，旧节点不会自动获知撤销。

## 3. Core 31.1 继续在 Docker 构建时固定

`docker/Dockerfile.bitcoin-core` 固定版本31.1和平台 `x86_64-linux-gnu`。
现有镜像 workflow 默认使用 `docker/bitcoin-source.json` 的 `upstream` 配置，自动获取官方归档、SHA256SUMS 和上游签名公钥，
验证通过后将二进制装入镜像；最终 release 继续绑定镜像 digest。

发布负责人使用既有镜像构建/release 流程即可，无需运行 `keygen --artifact-type bitcoin-core` 或 `prepare-core`。
Dockerfile 虽然向公共下载工具传递 `--trusted-keys`，但 `upstream` 分支不读取 USDB catalog、不验证 USDB 签名。
镜像构建完成后，普通节点安装流程不会重新下载或签发 Core 二进制。

如果只需将官方材料放到自有镜像站，也可以在 `upstream` 模式中配置相应 `release_base_url/keys_base_url`，
继续验证相同的官方签名与内置哈希，无需额外 USDB Core 密钥。

## 4. 可选：发布与消费自有签名 UTXO

本节仅供选用 `usdb-signed` 的 UTXO 发布方执行，沿用第2节变量及已审查的 UTXO 公钥目录。
`pinned` 模式直接在原生 bundle 中指定 `.dat` 来源或预置文件，无需执行本节签发命令。
下面是已实现工具的独立用法；原 snapshot key 与统一命令入口已按第2.1节接通，日常发布优先使用该入口。

签发原始935000快照会扫描完整9.39GB文件；保留原文件，不生成旧 BH DB 快照：

```bash
python3 "$DIST_TOOL" prepare-utxo \
  --artifact /data/btc/mainnet-935000-utxos.dat \
  --signing-key /secure/usdb-keys/bitcoin-utxo-v1/signing-key.json \
  --trusted-keys "$TRUST" --output-dir "$PUBLISH_ROOT/utxo-935000"
```

上传原始 `.dat`、manifest 和 signature 到同一专用目录。公开地址示例：
`https://downloads.example.org/bitcoin/utxo/935000/release-1/<sha>.json`。

下面是 P7.2 **独立 Core overlay** 的用法。P7.3 正式 node-kit 应使用下一节的包内材料及
[原生 bundle 生成流程](./balance-history-assumeutxo-p73-operations.md#32-自有签名--usdb-signed)，不能用 node.env 改写 release 已绑定的分发方式。

独立 overlay 的 node.env 增加：

```dotenv
BTC_ASSUMEUTXO_DISTRIBUTION_MODE=usdb-signed
BTC_ASSUMEUTXO_MANIFEST_URL=https://downloads.example.org/bitcoin/utxo/935000/release-1/<sha>.json
BTC_ASSUMEUTXO_MANIFEST_FILE=
BTC_ASSUMEUTXO_SOURCE_URL=
```

镜像需已包含经核对的 UTXO 公钥。工具从 manifest 同目录推导 `.dat` URL；也可以显式指定
`BTC_ASSUMEUTXO_SOURCE_URL` 指向另一镜像，下载内容仍必须匹配已签名和内置的同一文件身份。
快照仍使用原有 Range 断点恢复、全文件 SHA256、原子发布和 loadtxoutset 状态协调。
签名者、manifest hash、公钥目录 hash 会写入 activation journal；该记录不是 Core 链验证的替代。

手动准备文件并验收公开下载（不会调用 Core RPC）：

```bash
python3 docker/scripts/tools/bitcoin_assumeutxo.py download \
  --snapshot-file "$PUBLISH_ROOT/download/mainnet-935000-utxos.dat" \
  --distribution-mode usdb-signed \
  --manifest-url 'https://downloads.example.org/bitcoin/utxo/935000/release-1/<sha>.json' \
  --trusted-keys "$TRUST"
```

离线安装可将已取得的 manifest 和 `.json.sig` 放在 artifact 挂载目录，设置
`BTC_ASSUMEUTXO_MANIFEST_FILE=/data/assumeutxo/<sha>.json` 并清空 `BTC_ASSUMEUTXO_MANIFEST_URL`。
本地 manifest 同样必须验签；使用已有 `.dat` 时仍扫描完整文件。
`bootstrap` 若 Core 已激活相同基线，则继续按 Core 实际状态复用，不重新扫描 `.dat`；所选 signed 模式仍须先通过小型 manifest 验证。
使用远程 manifest 模式时，该步骤需要源可访问；离线环境应选择本地 manifest。
`status` 始终是只读 Core 探针，不访问分发源、不要求签名材料。

### 4.1 原生 bundle 中的公开材料格式

使用公开源的 `pinned` 模式不需要生成以下三份材料，也不需要准备 USDB UTXO 签名密钥。
自有镜像提供相同 `.dat` 时也可选 `pinned`；只有选择 `usdb-signed` 才执行密钥生成、`prepare-utxo` 和签名材料装包。
两种模式都需要发布端生成 `artifacts/assumeutxo-bootstrap.json`，并由安装包提供给节点。

`artifacts/assumeutxo-distribution.json` 的字段示意如下。数值来自当前内置935000检查点；
示意采用缩进便于阅读，**正式文件必须直接使用工具输出，不能在签名后重新排版**：

```json
{
  "schema_version": "usdb-bitcoin-artifact:v1",
  "artifact_type": "bitcoin-assumeutxo",
  "identity": {
    "network": "bitcoin",
    "base_height": 935000,
    "base_hash": "0000000000000000000147034958af1652b2b91bba607beacc5e72a56f0fb5ee",
    "file_sha256": "e572ddbe456d254f05fb004cebe225bdb3656074b66f0e9b1c7fa83e1301d486",
    "hash_serialized_3": "e4b90ef9eae834f56c4b64d2d50143cee10ad87994c614d7d04125e2a6025050"
  },
  "file": {
    "name": "mainnet-935000-utxos.dat",
    "sha256": "e572ddbe456d254f05fb004cebe225bdb3656074b66f0e9b1c7fa83e1301d486",
    "size_bytes": 9387990306
  },
  "upstream_verification": null,
  "signature_scheme": "ed25519",
  "signing_key_id": "usdb-bitcoin-utxo-v1"
}
```

UTXO 的 `upstream_verification` 为 `null`；Core 的三名 GPG 签名者证据属于另一用途的 manifest，不能填到这里。
快照 manifest 不包含私钥、G 或 BH commit。G/hash 由 bootstrap 配置绑定，C(B)/D(B) 由服务内置检查点取得。

`artifacts/assumeutxo-distribution.json.sig` 是64字节**原始二进制** Ed25519 签名，不是 JSON、Base64 文本或 GPG armor。
签名输入是 UTF-8 字符串 `usdb-bitcoin-artifact:v1:bitcoin-assumeutxo`、一个 NUL 字节、canonical manifest bytes 依次连接。
canonical JSON 使用字段排序、紧凑分隔符、ASCII 转义、禁止 NaN，文件末尾一个 LF；manifest 文件名不参与签名。
所以工具可以把发布端 `<sha>.json[.sig]` 原样复制并改名为上述固定路径，签名仍有效。

`trust/bitcoin-artifacts.trusted-keys.json` 示例：

```json
{
  "schema_version": "usdb-bitcoin-artifact-trust:v1",
  "keys": [
    {
      "key_id": "usdb-bitcoin-utxo-v1",
      "artifact_type": "bitcoin-assumeutxo",
      "public_key_base64": "<Base64 of 32 raw Ed25519 public key bytes>"
    }
  ]
}
```

公钥占位符应取自经审查的 signer 公钥。复用入口自动从原 snapshot catalog 转换公开格式；
新建 signer 时来自第2.2节 `keygen` 的输出。不能把示例占位符直接装包。
`key_id` 必须与 manifest 的 `signing_key_id` 一致，用途必须匹配。
`assumeutxo_deployment.py --manifest-file ... --trusted-keys ...` 负责实际验签、原样复制这三份文件，并计算它们在 bootstrap 配置中的 SHA-256 引用。
复用方案保留原 snapshot signer 的密钥材料，新 UTXO manifest/签名域保持独立；当前独立工具仍要求上述新版文件格式。

原生 node-kit 使用本地包内 manifest。自动下载时在生成 bundle 时显式传入完整 HTTPS `.dat` 地址；
留空只表示预置文件，不会通过固定包内文件名找到远程文件。已采用 P7.3 的 observer 还会确保原始 `.dat` 可供 BH 使用，
因此不能将独立 overlay 的“Core 已激活即可跳过原始文件扫描”行为套用到整套服务启动。

## 5. 验证与交付边界

```bash
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_bitcoin_artifact_distribution.py -v
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_bitcoin_assumeutxo.py -v
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_snapshot_range_download.py -v
PYTHONDONTWRITEBYTECODE=1 python3 tests/run_bitcoin_assumeutxo_container.py --image usdb-bitcoin-core:distribution-check
PYTHONDONTWRITEBYTECODE=1 python3 tests/run_bitcoin_distribution_acceptance.py \
  --image usdb-bitcoin-core:distribution-check \
  --core-archive /data/btc/bitcoin-31.1-x86_64-linux-gnu.tar.gz \
  --upstream-evidence /path/to/verified-upstream-files \
  --utxo-file /data/btc/mainnet-935000-utxos.dat
```

分发测试使用临时密钥、真实 GPG/Ed25519 签名及本机 HTTPS 服务，覆盖两条 Core 路径、完整 UTXO 签发/下载/本地复用、
签名/内容篡改、缺失上游签名者、未知/轮换/跨用途密钥、错误检查点，以及签名失败时 RPC/下载不得执行。
真实主网文件和隔离容器的操作结果另行记录，不能以小文件 fixture 的成功替代主网导入验收。

本次本机验收结果：

- 分发14项、bootstrap14项、Range下载8项、服务入口8项、旧部署14项，共58项Python测试通过。
- 默认上游模式完整镜像构建通过，仍实际验证三名固定上游签名者。
- 真实90,293,352字节Core归档通过上游验证、临时USDB签发、本机HTTPS分发和镜像内验签/完整下载校验。
- 真实9,387,990,306字节UTXO文件完整SHA256扫描、临时USDB签发、镜像内manifest验签与检查点匹配通过；没有执行loadtxoutset。
- 隔离禁网Core启动、版本、无txindex、readiness、入口参数及正常停止通过。
- 本地镜像：`usdb-bitcoin-core:p72-distribution`，image ID
  `sha256:44ecfffc10c04bf00b71f3c062b8631fdb0b9b6feef3fd7b67cd20759b5c5cb2`。
- 真实分发报告：`/tmp/usdb-p72-distribution-acceptance-r976swtk/result.json`；
  Core容器报告：`/tmp/usdb-p72-container-ylxiz5xy/result.json`；日志：`/tmp/usdb-p72-distribution-*.log`。

Fast CI 已加入上述Python回归入口；GitHub运行结果与远程正式源下载仍需实际发布流程验证。
测试公钥仅在临时验收目录中使用，临时分发私钥在验收退出时清理，仓库可信目录仍为空。

本批不生成或启用生产私钥、不上传自有存储、不发布镜像，也不修改已有节点。
公开下载闭环在发布环境落实；仅选用自有签名时才需要配置对应源和生产公钥。上面包含的 Core 自有签名测试是可选能力的覆盖记录，
不代表默认流程需要签发 Core。node-kit 整套配置/生命周期已在 P7.3 接入（`3c71205`），
见[P7.3 手册](./balance-history-assumeutxo-p73-operations.md)。正式发布 workflow 的原生 bundle 接入与单机主网交付验收仍属于 P7.4。

## 附录 A：可选 Core 自有签名能力（本次部署不采用）

此前的分发兼容实现还支持为官方 Core 二进制签发 USDB manifest。这是独立的可选发布能力，
目前默认源码、镜像 workflow 和节点部署均不要求启用，也不需要为它生成密钥或建立发布源。
仅托管官方文件可直接采用第3节的上游校验方式；只有明确选择 USDB 作为 Core 分发签名者时，才执行本附录。

自有 Core 发布端先通过三名固定上游签名者的验证，再签发 USDB manifest，构建端依据信任目录核对该声明。
本版本仅允许相同的官方31.1 x86_64归档，USDB 签名不能更改内置文件哈希或授权自行编译的替代程序。
此用途的密钥与 UTXO 密钥独立，是同时启用两种签发时的用途隔离要求，不是所有部署要生成两套密钥。

```bash
cd /home/bucky/work/usdb
DIST_TOOL="$PWD/docker/scripts/tools/bitcoin_release.py"
umask 077
python3 "$DIST_TOOL" keygen --artifact-type bitcoin-core \
  --key-id usdb-bitcoin-core-v1 --output-dir /secure/usdb-keys/bitcoin-core-v1
```

本附录还需要 GnuPG。经独立审查后，将输出的 public catalog 条目纳入仓库可信目录，私钥保留在发布环境。

以下目录变量是示例，按发布机器实际位置设置。准备步骤不上传任何远程对象。

```bash
PUBLISH_ROOT=/data/usdb-bitcoin-artifacts
TRUST="$PWD/docker/trust/bitcoin-artifacts.trusted-keys.json"
python3 "$DIST_TOOL" download-upstream --output-dir "$PUBLISH_ROOT/upstream-31.1"
python3 "$DIST_TOOL" prepare-core \
  --artifact "$PUBLISH_ROOT/upstream-31.1/bitcoin-31.1-x86_64-linux-gnu.tar.gz" \
  --upstream-evidence "$PUBLISH_ROOT/upstream-31.1" \
  --signing-key /secure/usdb-keys/bitcoin-core-v1/signing-key.json \
  --trusted-keys "$TRUST" --output-dir "$PUBLISH_ROOT/core-31.1"
```

已有官方归档和完整五份上游验证文件时，可以直接运行 `prepare-core`，它仍会重新验签和扫描归档，
不会把已有 `upstream-verification.json` 当成验证通过的凭据。
`download-upstream` 可显式指定 `--release-base-url`、`--keys-base-url` 使用上游材料的镜像，校验策略不变。

将以下文件上传到自有 HTTPS 源同一个专用目录，例如 `bitcoin/core/31.1/<release-id>/`：

1. `bitcoin-31.1-x86_64-linux-gnu.tar.gz`；
2. 工具输出的 `<sha>.json`；
3. 对应 `<sha>.json.sig`。

使用现有对象存储凭据和 `aws --endpoint-url ... s3 cp ...` 等发布工具即可；先上传归档和签名，最后上传 manifest。
只选择这三份公开文件上传，避免将私钥目录递归同步出去。上游验证材料另存到该发布的 `upstream/` 目录供审计。
每次发布使用新的不可变目录，不覆盖旧版本；URL 不含凭据、query、fragment，HTTPS 重定向仍须保持 HTTPS。

在仓库创建并提交 `docker/bitcoin-source-usdb.json`：

```json
{
  "mode": "usdb-signed",
  "manifest_url": "https://downloads.example.org/bitcoin/core/31.1/release-1/<sha>.json"
}
```

本地从公开入口验证实际文件并构建：

```bash
python3 "$DIST_TOOL" fetch-core \
  --source-config docker/bitcoin-source-usdb.json --trusted-keys "$TRUST" \
  --output-dir "$PUBLISH_ROOT/public-verification-31.1"
docker build -f docker/Dockerfile.bitcoin-core \
  --build-arg BITCOIN_SOURCE_CONFIG=docker/bitcoin-source-usdb.json \
  -t usdb-bitcoin-core:distribution-check .
```

`fetch-core` 会实际下载整个归档并校验，不仅检查公开链接或 manifest。
镜像包含 `/opt/bitcoin/share/core-artifact-provenance.json`，记录所选模式、manifest 身份/签名者或上游验证证据。
GitHub Core 镜像 workflow 的 `source_config` 输入接受已跟踪的 `docker/` JSON 路径；默认仍是
`docker/bitcoin-source.json` 的上游模式。整套 release workflow 也可以通过修改默认配置文件选择自有源。
公钥随源码固定，不能通过 workflow 输入从下载站获取或临时替换。
