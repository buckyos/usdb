# AssumeUTXO：复用 snapshot-keys 的 create / finalize / publish / deploy

日期：2026-09-13。本手册用于发布自有签名的原始935000 UTXO 文件，不生成 Docker 镜像或旧 BH 数据库快照。
Core 31.1 继续由既有镜像 workflow 固定；节点使用原生 bundle 后仍通过正式 `setup/up` 自动下载和导入。

## 1. 准备发布变量

在已有 snapshot 发布机执行，使用原签名私钥、公钥目录和 AWS profile。下面采用用户已下载的文件路径：

```bash
cd /home/bucky/work/usdb
set -o pipefail
export PYTHONDONTWRITEBYTECODE=1
SNAPSHOT_SCRIPT="$PWD/src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh"
export SNAPSHOT_TYPE=assumeutxo
export SNAPSHOT_ROOT=/data/btc/utxo-snapshot-release
export SNAPSHOT_UTXO_FILE=/data/btc/mainnet-935000-utxos.dat
export SNAPSHOT_KEY_ROOT="$HOME/.usdb/secure/snapshot-keys"
export SNAPSHOT_SIGNER_ID=usdb-mainnet-snapshot-v1
export SNAPSHOT_AWS_PROFILE=usdb-snapshot-publisher

test -r "$SNAPSHOT_UTXO_FILE"
test -r "$SNAPSHOT_KEY_ROOT/$SNAPSHOT_SIGNER_ID.signing-key.json"
test -r "$SNAPSHOT_KEY_ROOT/$SNAPSHOT_SIGNER_ID.trusted-keys.json"
bash "$SNAPSHOT_SCRIPT" paths
```

`SNAPSHOT_ROOT` 存放本轮小型发布材料，不需要恢复已删除的旧 builder workspace。
若发布机原来的路径、signer ID、AWS profile 有覆盖值，使用原值；不执行 `init` 或重新 `keygen`。
私钥权限须为仅 owner 可读写，例如 `0600`；工具不会替用户改权限、覆盖密钥或将其复制到发布目录。
Python 3、OpenSSL 为本地签发依赖；`publish` 还需要原有 AWS CLI 和已配置的上传权限。

默认 bucket/endpoint/public base 沿用原脚本配置，`paths` 会显示实际采用的值。
原有 `SNAPSHOT_S3_BUCKET/SNAPSHOT_S3_ENDPOINT_URL/SNAPSHOT_PUBLIC_BASE_URL/SNAPSHOT_AWS_REGION`
和 `SNAPSHOT_S3_UPLOAD_CONCURRENCY/SNAPSHOT_S3_CHUNK_SIZE_MIB/SNAPSHOT_UPLOAD_PROGRESS` 覆盖项继续有效。
也可以不设置 `SNAPSHOT_TYPE`，在每条命令中显式增加 `--snapshot-type assumeutxo`。

这里的 `--height` 指 **快照基线935000**，不是业务 G=963800。工具拒绝错误高度或 base hash；无需连接 bitcoind。

## 2. Create：核对原始文件并使用原密钥签名

```bash
bash "$SNAPSHOT_SCRIPT" create --height 935000
```

直接读取已有 `.dat`，重算完整9,387,990,306字节文件的 SHA-256，并核对代码内置935000身份。
使用原 snapshot 私钥对新 UTXO manifest 签名，再独立验签，成功后原子发布 `prepared/`。
不解析或重放全部 BTC 历史，不创建 BH core/registry 数据库，也不复制已有 `.dat`。
文件内容与 key ID 保持原样；私钥格式只在内存中适配，公开 catalog 自动转换为 UTXO 用途。

没有本地文件时，可另设 `SNAPSHOT_UTXO_SOURCE_URL` 为完整 HTTPS `.dat` 地址，并将 `SNAPSHOT_UTXO_FILE`
指向计划下载的位置。已有断点下载逻辑会先完成校验再签名，普通文件发布仍使用上述本地路径即可。

输出根默认是：

```text
<SNAPSHOT_ROOT>/releases/assumeutxo/mainnet-935000/
  prepared/<manifest-sha256>.json
  prepared/<manifest-sha256>.json.sig
  prepared/bitcoin-artifacts.trusted-keys.json
  prepared/prepared.json                 # 本地输入路径与生成代码信息，不上传
  artifact-finalized.json                # finalize 后生成
  records/<record-sha256>.json            # prepare-release / publish 后生成
  publish-result.json                    # publish 全部核验通过后生成
```

若配置过 `SNAPSHOT_RELEASE_ROOT`，以 `paths` 返回的 `root_dir` 为准。默认 `prepared.json` 绑定本地原始文件路径；
中断或重复执行使用相同输入与 signer。换签名者、源文件路径或公开 catalog 时使用新的发布根，避免覆盖已冻结材料。

## 3. Finalize：冻结可发布材料

```bash
bash "$SNAPSHOT_SCRIPT" finalize --height 935000
```

重新扫描原始文件，核对签名和原 public catalog，冻结文件/签名/公钥身份及生成、finalize 代码记录。
重复 finalize 会再次校验，但保留第一次的不可变记录；异常不覆盖原证据。
只有 `create` 需要读取私钥，`finalize/publish` 使用原 public catalog 复核即可。

可选：只生成发布记录供上传前查看，不进行上传：

```bash
bash "$SNAPSHOT_SCRIPT" prepare-release --height 935000
```

`prepare-release` 也会扫描文件。若不需要提前查看记录，直接执行下一步，`publish` 会准备同一记录。

## 4. Publish：复用服务器上传并核对公开下载

```bash
bash "$SNAPSHOT_SCRIPT" publish --height 935000
```

使用原 AWS profile/服务器，上传集合仅包含原始 `.dat`、UTXO manifest、分离签名、公开 catalog 和 finalization 记录。
S3 上传沿用原 multipart、metadata/大小检查与幂等规则：相同对象可复用，身份冲突时拒绝覆盖。
每个对象随后通过匿名 HTTPS 验证实际字节。数据和签名材料验证通过后才上传 content-addressed release record，并验证该记录。
公开大文件校验流式计算哈希，不在磁盘保存第二份9.39GB文件。

对象路径为 `bitcoin/utxo/935000/<finalization-sha256>/...`，最终记录位于
`snapshot-records/assumeutxo/v1/<record-sha256>.json`；同一 bucket 内的旧 BH 对象保持原样。
发布成功会输出 `status=published`，并写入 `publish-result.json`，包含公开 URL、本地公开材料路径和验证时间。
上传或公开验证失败会退出非零，保留已上传对象。排障后重复同一 `publish`；不要手改对象 metadata 或覆盖冲突对象。
S3 已存在的对象仍需通过实际 HTTPS 校验，不能只凭 upload 成功判断节点能够下载。

长任务有三次必要的本地文件扫描（create、finalize、publish），一次远程上传和一次完整公开下载。
扫描及下载显示开始、进度、完成和实际耗时；公开下载速度依赖发布机到下载源的网络，不使用本地扫描耗时估计网络耗时。

### 4.1 上传完成后，公开校验返回403

若日志停在 `Upload ...: complete` 后的 `Public verification started`，表示 S3 对象上传和 metadata/大小检查已通过，
失败发生在匿名 HTTPS 下载阶段；此时整套发布尚未完成。

2026-09-13 在现有公开域名复现：Python 默认 User-Agent 返回 `HTTP 403`、响应正文 `error code: 1010`；
同一 URL 使用原 snapshot 工具的 `usdb-snapshot-verifier/1` 返回 `HTTP 200`，Content-Length 为9,387,990,306。
这是 Cloudflare 根据客户端请求标识拒绝访问，参见[错误1010说明](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-1xxx-errors/error-1010/)。
发布验证器、manifest/签名下载器和节点 UTXO 断点下载器现已统一使用原请求标识；HTTP 校验失败日志会包含目标 URL 和状态码。

在原发布 shell 中保留第1节变量，使用修复后的脚本重跑：

```bash
bash "$SNAPSHOT_SCRIPT" publish --snapshot-type assumeutxo --height 935000
```

原 `.dat` 的对象身份一致时会显示 `already exists, skipping`，随后重新完整下载校验，并继续上传和校验剩余公开材料，
最后发布 release record。无需重新 create/finalize 或删除已上传对象；原 manifest、签名和 finalization 继续复用。
不能用 `verify-published` 代替这次恢复，因为剩余材料可能尚未上传。
后续节点部署使用的 Core 镜像也应包含此次下载器修复；本地脚本更新不会改变已有镜像。

## 5. 状态、Deploy 与节点装包

```bash
bash "$SNAPSHOT_SCRIPT" status --height 935000
bash "$SNAPSHOT_SCRIPT" verify --height 935000
bash "$SNAPSHOT_SCRIPT" verify-published --height 935000
```

`status` 只校验小型元数据，不扫描原文件或探测线上状态；`verify` 重扫本地文件；`verify-published` 重查完整公开发布，不上传。
上述命令无需私钥内容，但仍需要原可信公钥目录。多个修改发布状态的命令不能在同一 root 并发执行。

发布成功后，推荐直接使用同一脚本的 `deploy`，无需手工从 JSON 搬运 manifest、公钥与下载 URL：

```bash
SNAPSHOT_DEPLOY_BUNDLE="$SNAPSHOT_ROOT/deploy/usdb-testnet-v0-assumeutxo"
bash "$SNAPSHOT_SCRIPT" deploy --snapshot-type assumeutxo --height 935000 \
  --source-bundle /home/bucky/work/usdb/docker/networks/testnet-v0 \
  --output-dir "$SNAPSHOT_DEPLOY_BUNDLE" \
  --origin-block-hash 000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551
```

这里的 `deploy` 会**生成部署 bundle，并将打包需要的小型公开输入登记到源码目录**。它读取本轮 `publish-result.json` 和对应 record，
复核成功状态、record 哈希、finalization、签名及原可信公钥，再调用 `assumeutxo_deployment.prepare_bundle` 生成并校验候选。
URL 和签名材料以实际发布结果为准，不采用后来改变的 `SNAPSHOT_PUBLIC_BASE_URL/SNAPSHOT_UTXO_SOURCE_URL`。
没有成功发布记录、记录不匹配或签名/公钥无效时会拒绝生成。

`--source-bundle` 提供网络及业务起点 G（当前测试网为963800）；`--origin-block-hash` 必须对应该 G。
`--height 935000` 仍指快照基线 B，不能用它代替 G。`--output-dir` 必须在源 bundle 之外。
新目录正常生成；已有目录会与当前发布材料重建出的结果逐文件核对，一致时复用。
旧版 deploy 仅缺少发布 record 绑定时，会补入该小文件并更新 network.json；其他差异拒绝覆盖，需选择新的输出目录。
输出包含 `status=deployment_integrated`、`bundle_dir`、`release_input_file`、G/hash 和原 snapshot record URL/hash。
仅处理小型公开材料，不读取私钥或 AWS 凭据，不复制/扫描/下载 `.dat`，不访问或启动节点。
本轮原始文件可不在发布机上，但原公开 catalog、prepared、finalization、records 和 publish-result 必须保留。
该步骤复核本地已完成发布的证据，不重新确认当前线上可用性；需要再次全量公开复核时单独执行 `verify-published`。

源码中自动产生以下公开文件，应与工具和 workflow 改动一起进入版本控制：

```text
docker/networks/testnet-v0/release-bootstrap.json
docker/networks/testnet-v0/release-inputs/assumeutxo/<record-sha256>/
  assumeutxo-distribution.json
  assumeutxo-distribution.json.sig
  bitcoin-artifacts.trusted-keys.json
  <record-sha256>.json
```

这五个小文件固定基础 network.json 哈希、G/hash、下载 URL、签名、公钥和已发布 record；不含发布机绝对路径、私钥或 `.dat`。
如果只需导出、暂不登记源码，使用同一命令并增加 `--prepare-only`，此时状态为 `deployment_prepared`。

**登记后的输入已接入正式 candidate/publish workflow。** 后续按照原发布流程审查、提交这些文件及本批代码，并使用包含它们的 release tag：

1. candidate 通过 `release_bundle.py prepare` 从该 tag 的基础 bundle 和发布输入重建原生 bundle，并据此生成 manifest。
2. publish 从同一 tag 再次重建，复核 candidate manifest；网络 bundle 归档和 node-kit 都使用这个结果。
3. 两处公开校验按模式选择：原生模式核验 UTXO record、签名材料及 Range 下载；旧模式保留原 BH 快照校验。
   CI 不重新下载9.39GB；完整文件 SHA-256 仍由 snapshot publish 和节点下载执行。
4. 节点继续使用 `installer -> setup -> doctor -> up`。

GitHub CI 不需要发布机 `/data` 目录或 snapshot 私钥。登记文件损坏、与基础网络不匹配或签名无效会阻断打包，不回退到旧模式。
没有登记文件的历史源码仍使用原 bundle。`deploy` 不自动 commit、push、打 tag、触发 workflow 或发布安装器；未提交的登记不会影响已发布版本。
因此，使用这个命令后不必再重复[P7.3 第3.2节](./balance-history-assumeutxo-p73-operations.md#32-自有签名--usdb-signed)的手工装包操作。

保留独立工具入口供已有自动化调用；`publish-result.json` 的字段映射如下：

| 发布输出字段 | bundle 参数 |
| --- | --- |
| `manifest_file` | `--manifest-file` |
| `trusted_keys_file` | `--trusted-keys` |
| `source_url` | `--source-url` |

`record_url` 和 `manifest_url` 用于发布归档。节点只接收公开材料；原 snapshot 私钥不进入 bundle 或镜像。
本批已接通快照发布、源码登记、candidate/publish 和 node-kit；实际 GitHub release 与主网节点验收仍需按正式发布流程安排。

## 6. 本批验证记录

- 新发布生命周期18项测试通过，覆盖旧格式密钥复用、用途/签名域、错误文件和密钥、重复执行、中断恢复、不可变对象冲突、匿名 HTTPS 核验和原生 bundle 消费。
- Bitcoin 分发14项、Core bootstrap16项、原生 node13项、旧发布 wrapper13项、旧对象存储14项，共88项通过。
- ShellCheck、CLI 帮助、actionlint 和 diff 空白检查通过。
- 真实935000文件使用 Rust `snapshot-keygen` 生成的临时旧格式 signer，经原 shell 入口完成 create/finalize/prepare-release：
  本机分别约9.4秒、4.8秒、4.8秒，私钥文件未修改，已有 `.dat` 未复制。临时私钥在测试结束时删除。
- 现有 `usdb-bitcoin-core:p73-native` 镜像在禁网、只读临时容器内接受上述公开 catalog/manifest 并验签通过。
  没有启动 bitcoind，没有执行 Core 导入；本次无需为旧密钥复用重新构建节点镜像。
- 真实文件报告：`/tmp/usdb-utxo-real-file-spbwssfy/result.json`，镜像验签报告：同目录 `container-result.json`。
  这些报告来自临时验收 signer，不能作为正式发布材料。

本批没有使用生产私钥、读取 AWS 凭据或向线上服务器上传。实际自有源发布由操作员按第1至4节完成。

403修复验证：发布生命周期19项、Core bootstrap16项、Bitcoin 分发14项，共49项通过。
共用 HTTPS 测试服务器的旧 snapshot 下载8项也通过，总计57项；Python 语法、CLI 帮助与 diff 空白检查通过。
覆盖公开域名拒绝默认请求标识、失败后复用已上传对象、最后发布记录、manifest/签名下载，以及 HTTPS 重定向后的断点续传。
实际公开文件用修复后的验证器连续读取超过679MB后主动结束连通性检查，未完成本次完整远程 SHA-256 校验；
完整校验由操作员重跑 `publish` 完成。本次排障未上传对象、读取发布凭据或修改现有发布材料。

Deploy衔接验证：发布生命周期24项、旧发布 wrapper13项、原生 node-kit/controller13项，共50项通过。
新增覆盖成功发布自动装包、缺失/不完整发布阻断、结果路径与 URL 篡改、record/签名/公钥复核、无私钥及原始文件装包、
错误 G/hash 与已有输出保护；ShellCheck、CLI 帮助、Python/文档命令语法及 diff 空白检查通过。
这些验证使用临时发布材料与模拟下载源，没有生成或发布实际节点安装包。

发布链路接入验证：扩展后的发布/登记测试28项、原生 node13项、release manifest13项及旧打包/配置/发布记录回归96项，共150项通过。
两条 release workflow 与 Fast workflow 的 actionlint、shell 检查通过。
已复用操作员实际发布的 `77f1e50991528b053ad16c5abafafe09a305a62516804832747072ee5a3e1680` record，
将现有导出升级并登记到源码；未再次扫描/下载/上传原始 UTXO。
同一源码分别模拟 candidate/publish 重建，21个 bundle 文件逐字节一致；真实签名材料装入本地测试 node-kit 后，脱离源码目录加载为 `native`。
测试 node-kit 使用模拟的 revision、OCI digest 和 qualification，仅用于验证装包，不是可发布安装包。
新的 CI 公开验证已对真实发布源通过（小型材料完整哈希及 UTXO 一个字节 Range），报告位于
`/tmp/usdb-native-release-integration-s7r67pg9/result.json` 和同目录 `public-verification.json`。
实际源码登记结果为 `/tmp/usdb-native-release-integrate.json`。本次未 commit、push、触发 GitHub workflow 或切换节点。
