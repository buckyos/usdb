# Bitcoin Core 与 AssumeUTXO 自有源分发

日期：2026-09-13。P7.2 基础编排已提交为 `eb6c8c2`；本文描述其后的分发兼容补充。

## 1. 两条明确的信任路径

| 对象 | 模式 | 验证规则 |
| --- | --- | --- |
| Core 发布包 | `upstream`，默认 | 下载发布包、SHA256SUMS、签名和公钥；校验三份固定上游签名、归档大小和 SHA256 |
| Core 发布包 | `usdb-signed` | 从自有源下载 USDB manifest/signature，使用独立固定的公钥验签，再下载并校验发布包 |
| 原始 UTXO | `pinned`，默认 | 本地文件或任意合规 HTTPS 源；完整文件 SHA256 必须匹配内置检查点 |
| 原始 UTXO | `usdb-signed` | 先验证 USDB manifest/signature，再使用原有断点下载、文件校验和 Core 导入流程 |

模式在操作开始前确定，验签失败不会自动切换来源或降级。Core 二进制的下载校验发生在镜像构建时；
普通节点仍拉取发布的 OCI 镜像。镜像自身的 digest/provenance 与这里的二进制签名分属不同层次。

自有 Core 发布端必须先通过当前三名上游签名者的验证，才能签发 USDB manifest；接收端信任这个发布声明，
不必再访问 bitcoincore.org 或 GitHub。发布端归档上游证据，支持离线复核。
**本版本仅支持原封不动的官方 Core 31.1 x86_64 包**，自有签名不能授权更换其固定 hash。
自行编译/修改 Core 需要另行定义构建身份和验收策略，不属于本批。

UTXO 签名绑定网络、B/hash、文件大小/SHA256、`hash_serialized_3`；接收端逐项匹配 Rust 使用的检查点目录。
它不能替换 Core 内置 UTXO 承诺，不能修改 BH 内置 C(B)/D(B)，也不会省略每个节点的官方后台历史验证。

## 2. Manifest 与密钥

入口：`docker/scripts/tools/bitcoin_release.py`。运行需要 Python 3 和 OpenSSL；接收/复核上游 Core 还需要 GnuPG。

- manifest 使用 `usdb-bitcoin-artifact:v1`，严格字段、无重复 JSON key、排序紧凑 JSON 和末尾 LF。
- Ed25519 签名对象为 `usdb-bitcoin-artifact:v1:<artifact_type>\0` 加 canonical manifest bytes。
- `artifact_type` 分为 `bitcoin-core`、`bitcoin-assumeutxo`；两种用途使用独立密钥。
- manifest 文件名为 `<manifest-sha256>.json`，旁边是64字节原始签名 `<manifest-sha256>.json.sig`。
- manifest 包含文件名、大小、SHA256、身份、`signing_key_id` 和上游验证证据摘要。
- Core 上游证据包括 SHA256SUMS、SHA256SUMS.asc 和三个固定公钥文件的 hash/大小。
- 接收方可信目录使用独立 schema `usdb-bitcoin-artifact-trust:v1`，每个公钥绑定一个用途。
  禁止重复 key ID 或复用同一公钥材料；旧 BH snapshot 的 catalog/私钥不能直接用于这里。

可信公钥文件为 `docker/trust/bitcoin-artifacts.trusted-keys.json`，随镜像复制到
`/opt/usdb/docker/scripts/data/trust/bitcoin-artifacts.trusted-keys.json`。
**当前目录为空，不包含生产或测试签名者。** 切换自有签名源前，由发布负责人生成正式密钥，核对公钥并将公钥目录纳入代码 review。
签名私钥只留在发布环境，工具要求文件权限排除 group/other，Docker build context 排除工具的私钥文件名。
公钥不能从待验证的同一下载源临时获取并直接信任。

生成密钥的操作示例（这些命令由发布负责人在受控环境执行，输出目录必须不存在）：

```bash
cd /home/bucky/work/usdb
DIST_TOOL="$PWD/docker/scripts/tools/bitcoin_release.py"
umask 077
python3 "$DIST_TOOL" keygen --artifact-type bitcoin-core \
  --key-id usdb-bitcoin-core-v1 --output-dir /secure/usdb-keys/bitcoin-core-v1
python3 "$DIST_TOOL" keygen --artifact-type bitcoin-assumeutxo \
  --key-id usdb-bitcoin-utxo-v1 --output-dir /secure/usdb-keys/bitcoin-utxo-v1
```

每个目录产生 `signing-key.json` 和 `trusted-keys.json`。仅将两个 public catalog 的 `keys` 条目合并到
仓库可信目录，保留 schema；不要复制私钥。轮换时使用新 key ID、新密钥，先发布包含新旧公钥的镜像，再切换签名者。
撤下旧公钥需要再次更新发布目录/镜像；旧镜像不会自动获知撤销。

## 3. 发布 Core 31.1

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

## 4. 发布与消费原始 UTXO

签发原始935000快照会扫描完整9.39GB文件；保留原文件，不生成旧 BH DB 快照：

```bash
python3 "$DIST_TOOL" prepare-utxo \
  --artifact /data/btc/mainnet-935000-utxos.dat \
  --signing-key /secure/usdb-keys/bitcoin-utxo-v1/signing-key.json \
  --trusted-keys "$TRUST" --output-dir "$PUBLISH_ROOT/utxo-935000"
```

上传原始 `.dat`、manifest 和 signature 到同一专用目录。公开地址示例：
`https://downloads.example.org/bitcoin/utxo/935000/release-1/<sha>.json`。

独立 Core overlay 的 node.env 增加：

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
正式源地址、公钥和公开下载闭环在发布环境落实；node-kit 整套配置/生命周期继续属于 P7.3。
