# Balance-History Snapshot 对象存储发布与安装

Status: first implementation complete; live R2 upload and target-host installation pending.

## 1. 边界

本文规定已完成 exact-height 构建、完整校验和轻量 release finalization 的 balance-history snapshot 如何
发布到 S3-compatible object storage，以及节点如何通过公开 HTTPS 下载并安装。Snapshot 不是 network
identity，也不绑定某一个 USDB `rN` release；fresh indexer 仍要求 snapshot 高度不高于该 network
bundle 的 `index_origin_height`。

当前对象存储参数为：

```text
bucket: usdb-snapshot
private S3 endpoint: https://87e0bdf811b13ee87fd0bcec7a4fd1e7.r2.cloudflarestorage.com
public HTTPS base: https://usdb-snapshot.tbudr.top
```

S3 access key/secret 只存在于 snapshot 发布机的 AWS CLI profile 或进程环境中。它们不得写入 Git、
release record、node kit、Compose env 或运维日志。节点只访问公开 HTTPS，不需要 AWS 凭证。

## 2. 发布模型

`snapshot_distribution.py` 发布原始 immutable artifact，而不是大 tar：

```text
snapshot_<H>.db
snapshot_<H>.manifest.json
snapshot_<H>.manifest.sig
complete.json
```

这样可以直接断点续传 DB，避免节点同时保留 tar 和解包后的 DB 所造成的接近双倍磁盘占用。`finalize`
只重算 DB SHA-256 并验证 marker/manifest/Ed25519 签名，不打开 SQLite 或恢复 RocksDB；完整恢复演练由
独立 `validate-install` 命令承担。只有明确需要离线归档时才执行 `archive`。

每个 release record 固定：

- network、height、BTC block hash 和 snapshot ID；
- 四个文件的 basename、size、SHA-256 和 object key；
- artifact producer USDB revision 与执行轻量 finalization 的 USDB revision；
- artifact-finalization marker 的 SHA-256 和时间；
- signer key ID、public trusted-key catalog basename 和 SHA-256；
- public HTTPS base。

四个文件的完整 inventory 生成 `artifact_set_id`。对象路径同时包含完整 `artifact_set_id`，因此重新
签名、manifest 时间或任一 sidecar 变化都会进入新目录，不覆盖旧对象。Release record 自身写入：

```text
snapshot-records/v2/<record-sha256>.json
```

上传顺序固定为“数据文件在前，release record 最后”。不发布可信语义不明确的 `latest.json`；运维或
USDB release 必须记录明确的 content-addressed record URL。

## 3. 发布机准备

要求：

- `mainnet_exact_height_snapshot.sh finalize --height H` 已成功；
- artifact 的 `complete.json`、DB、manifest 和 detached signature 相邻；
- 对应 release finalization 目录存在 `artifact-finalized.json`；
- public trusted-key catalog 已 review，且包含 manifest 的 signer；
- 已安装 AWS CLI，并通过 profile 或标准 `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` 提供凭证；
- Cloudflare R2 region 使用 `auto`；工具默认显式传递，也可用 `--aws-region` 覆盖。

AWS CLI 的源码、issue 和版本信息以 [AWS 官方仓库](https://github.com/aws/aws-cli) 为准；发布机安装
应使用 [AWS CLI v2 官方安装指南](https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html)，
不要把仓库默认分支中的 v1 开发说明或发行版中可能过期的 `awscli` 包当成生产安装基线。Linux x86_64
发布机可按官方流程安装并确认版本：

不要执行 `pip install awscli` 或使用 `--break-system-packages`。PyPI 的 `awscli` 属于 v1 安装路径，
Debian/Ubuntu 的 PEP 668 `EXTERNALLY-MANAGED` 保护也会拒绝它修改系统 Python；AWS CLI v2 官方
installer 自带运行时，不依赖系统 `pip`。

```bash
AWS_CLI_INSTALL_DIR="$(mktemp -d)"
curl -fsSLo "${AWS_CLI_INSTALL_DIR}/awscliv2.zip" https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip
unzip -q "${AWS_CLI_INSTALL_DIR}/awscliv2.zip" -d "${AWS_CLI_INSTALL_DIR}"
if command -v aws >/dev/null 2>&1; then
  sudo "${AWS_CLI_INSTALL_DIR}/aws/install" --update
else
  sudo "${AWS_CLI_INSTALL_DIR}/aws/install"
fi
aws --version
```

其他架构应使用官方文档对应的 installer。安装包签名验证可按同一文档的 PGP 验证步骤执行。

推荐为发布账户创建专用最小权限 profile：

```bash
aws configure --profile usdb-snapshot-publisher
```

该账户只需要 `usdb-snapshot` bucket 中约定 prefix 的 list/head/put 权限，不需要删除权限。对象存储的
PutObject 通常同时具备同 key 替换能力，因此发布工具仍会先 head 并拒绝身份不同的已有 key；发布账户应
独立保管，避免绕过工具直接上传。
Cloudflare 自定义域名应支持公开 GET/HEAD 和 HTTP Range；正式大文件发布前先用小对象验证 Range 响应。

## 4. 生成 Release Record

正常发布不需要重新填写 block hash、artifact 目录、finalization marker、trusted catalog 或 producer
revision。主网 snapshot 脚本会从首次 `create` 固定的 target record 和 `finalize` 目录推导并交叉检查：

- `targets/<height>.json`：BTC block hash 和生成任务固定的 USDB revision；
- `builder/snapshots/<height>/<hash>/`：immutable artifact 与 `complete.json`；
- `releases/finalized/<height>-<hash>/artifact-finalized.json`：轻量 release finalization 结果；
- `~/.usdb/secure/snapshot-keys/<signer>.trusted-keys.json`：公开 trusted catalog。

从 USDB 仓库执行：

```bash
cd /home/bucky/work/usdb

SNAPSHOT_SCRIPT=src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh
H=<snapshot-height>

bash "$SNAPSHOT_SCRIPT" prepare-release --height "$H"
```

`prepare-release` 会重新确认高度对应的 BTC canonical hash，并使用 finalize 已固定的 DB hash 构造
release record，不再重复扫描整个 SQLite。输出 JSON 中包含
record path、record SHA-256 和最终公开 URL，报告同时写入 snapshot release 目录。同一输入重跑是幂等
的；同名但内容不同的本地 record 会被拒绝覆盖。缺少 pinned target、`complete.json` 或 independent
artifact-finalization marker 时会失败关闭。

## 5. 上传 R2

日常发布只需执行：

```bash
bash "$SNAPSHOT_SCRIPT" publish --height "$H"
```

`publish` 会先幂等执行 `prepare-release`，再使用默认 profile `usdb-snapshot-publisher` 上传。bucket、S3
endpoint、region 和 public base 已使用本文第 1 节的固定默认值。大文件默认使用 AWS CLI classic transfer
client、`16` 个并发 multipart request 和 `64 MiB` part。工具把原 profile 复制到权限 `0600` 的临时
`AWS_CONFIG_FILE` 后注入本次参数，不修改 `~/.aws/config`。正常发布不再通过命令行重复传入。

镜像站、隔离演练或标准 AWS 环境凭证才使用高级环境覆盖：

| 环境变量 | 默认值 | 用途 |
| --- | --- | --- |
| `SNAPSHOT_AWS_PROFILE` | `usdb-snapshot-publisher` | AWS CLI profile；显式设为空字符串时使用标准 AWS 环境/role credential chain |
| `SNAPSHOT_S3_BUCKET` | `usdb-snapshot` | object storage bucket |
| `SNAPSHOT_S3_ENDPOINT_URL` | 当前 Cloudflare R2 S3 endpoint | 私有上传 endpoint |
| `SNAPSHOT_PUBLIC_BASE_URL` | `https://usdb-snapshot.tbudr.top` | 写入 release record 的公开下载 base |
| `SNAPSHOT_AWS_REGION` | `auto` | Cloudflare R2 region |
| `SNAPSHOT_RECORD_ROOT` | `<SNAPSHOT_ROOT>/releases/records` | 本地 content-addressed record 目录 |
| `SNAPSHOT_S3_UPLOAD_CONCURRENCY` | `16` | AWS CLI multipart 并发 request；允许 `1..64` |
| `SNAPSHOT_S3_CHUNK_SIZE_MIB` | `64` | multipart threshold/part size；允许 `5..1024` MiB |
| `SNAPSHOT_UPLOAD_PROGRESS` | `1` | wrapper 是否强制显示上传进度；自动化可设为 `0` |

这些值对应 AWS CLI 的 `s3.max_concurrent_requests`、`s3.multipart_threshold` 和
`s3.multipart_chunksize`，不是 `aws s3 cp` 自身的 `--s3-*` 参数。配置语义见
[AWS CLI S3 Configuration](https://docs.aws.amazon.com/cli/latest/topic/s3-config.html)。增加并发不能突破
宿主机上行、R2 endpoint 或中间网络的实际带宽，应先比较 `8/16/24` 三档吞吐和错误率再继续提高。

底层 `snapshot_distribution.py prepare/upload` 接口仍保留，用于审计、排障和其他 storage backend；日常
主网发布不应手工拼接这些路径和 provenance 参数。

对象存储发布不依赖 tar。只有需要离线介质、冷备或人工交付单文件包时才执行：

```bash
bash "$SNAPSHOT_SCRIPT" archive --height "$H"
```

该命令要求 `finalize` 已完成，并生成
`balance-history-mainnet-<height>-<block-hash>.tar` 及 `.tar.sha256`。不要在空间不足时执行；`publish`
不会隐式调用它，也不会自动删除历史 archive。

上传前工具会完整计算本地文件 SHA-256，防止 finalize/prepare 后文件变化。每个对象写入
`usdb-sha256` 和 `usdb-size` metadata，上传后使用 `head-object` 校验 metadata 与 ContentLength。
已存在且身份完全一致的对象会跳过；已存在但 metadata/size 不同会失败关闭。不要使用 multipart ETag
替代 SHA-256。

交互式发布时，本地 SHA-256 阶段显示已读/总字节、吞吐和 ETA；AWS CLI 上传阶段显示已上传字节
进度。进度统一写入 `stderr`，发布结果 JSON 仍保持在 `stdout`。wrapper 默认强制显示进度；自动化可设置
`SNAPSHOT_UPLOAD_PROGRESS=0`，底层工具也可通过 `USDB_SNAPSHOT_FORCE_PROGRESS=1` 强制打开。

AWS CLI 对单次大文件上传提供 multipart 和重试，但不保证进程退出后的跨进程 multipart resume。上传中断后
重跑同一命令是安全的，可能会重新发送该大文件并留下待生命周期规则清理的 incomplete multipart upload。
并发/chunk 参数只在新启动的 AWS CLI 进程生效，不能热更新当前上传；不要仅为切换参数中断已接近完成的上传。

发布结果也会保存到
`<SNAPSHOT_ROOT>/releases/reports/publish-<height>-<block-hash>.json`，脚本最后会输出该精确路径。从
命令输出或该报告读取并检查 `record_url`：

```bash
RECORD_URL=<record_url-from-publish-output>
curl -fsSI "${RECORD_URL}"
curl -fsSL "${RECORD_URL}" | sha256sum
```

第二条结果必须等于 URL basename 中的 SHA-256。还应对 DB URL 发起一个小范围 Range 请求，确认自定义
域名没有移除 byte-range 能力。

## 6. 节点下载和选择

从 release node kit 配置一个全新节点后、第一次 `usdb-node up` 前执行：

```bash
usdb-node setup
# setup 选择 snapshot 后会直接安装；中断时执行：
usdb-node snapshot install
usdb-node doctor
usdb-node up
```

标准节点命令不接受自由输入的 record URL；它从当前 release manifest 读取并复核已批准 URL/哈希。
`RECORD_URL` 仅用于本页前面的发布者审计。`snapshot install` 会：

1. 先下载较小的 content-addressed release record；
2. 在下载 DB 前校验 record schema、network、network bundle 的 index origin 和本地 trusted catalog；
3. 对大于等于 `128 MiB` 的 DB 默认使用 `8 x 64 MiB` 并行 HTTP Range；预分配 `.part`，将已 fsync 的
   chunk 记录到 `.ranges.json`，中断后只补缺失 chunk；小文件和旧连续 `.part` 继续单路续传；
4. 汇总分片后对完整 `.part` 校验 size 和 SHA-256；
5. 原子发布到 `<USDB_DATA_ROOT>/releases/balance-history/<snapshot-release-id>`；
6. 在下载前持久化 bundle-scoped `node.env` 中的批准 snapshot 选择，并在未完成时阻止 `up`；
7. 重新执行 runtime snapshot validator。

中断后重跑同一命令会复用 `.part` 和 staging 文件。已有同 ID 完整目录会逐文件复核；不同内容不会被
覆盖。若 `<BH_DATA_HOST_DIR>/db` 已初始化，命令会在下载前拒绝修改配置；应使用新的 data root，或执行
单独 review 的显式恢复流程。

默认参数适合普通 1 Gbps 级节点。确认 CDN、网络、磁盘和 CPU 有余量时可覆盖：

```bash
usdb-node snapshot install \
  --download-concurrency 16 \
  --download-chunk-size-mib 64
```

Range worker 只同时保留每个 worker 的一个临时 chunk，不额外保留完整分片副本；目标 DB 仍会在开始时
预分配全尺寸空间。调整 concurrency 不改变已有 `.ranges.json` 的 chunk 划分，调整 chunk size 只影响新任务。

安装命令只校验传输和 release identity。容器内 `snapshot-loader` 随后仍会使用 balance-history 原生
Ed25519 校验和 staging install；两层校验不能互相替代。

## 7. Paired Checkpoint 与后续工作

当前 `v1` release record 只接受 `artifact_type=balance-history`。当 `H > index_origin_height` 时，fresh
indexer 在大文件下载前拒绝该 record。不要通过修改 node.env 绕过这个限制。

`paired-checkpoint` 继续使用独立的严格双 artifact 工具链。后续对象存储 schema 应扩展为同时提交：

- signed balance-history artifact；
- signed usdb-indexer checkpoint 的 manifest/signature/data inventory；
- 两者 operation ID 和完整 state-ref binding。

在该 schema 和跨进程恢复测试完成前，不得把单侧 snapshot release record 标记成 paired checkpoint。

## 8. 当前验证边界

自动化已覆盖 release record 一致性、DB 篡改拒绝、S3 record-last 顺序、重复上传幂等、临时 AWS upload
配置、错误 catalog、高度提前拒绝、并行 Range 缺片恢复/HTTP 200 拒绝、断点 staging、原子安装、
node.env 选择和已有 DB 拒绝。仍需完成：

- 使用真实 R2 凭证的小文件 upload/head/download smoke；
- 最新主网大 DB 的中断上传和 HTTP Range 恢复；
- 空白目标机通过 `usdb-node snapshot install` 启动并核对 provenance/state-ref；
- 完成 manifest v4 已冻结 record URL/hash 的首个真实节点下载、安装和 state-ref 验收；
- paired-checkpoint 对象存储 schema 和真实 joiner 演练。
