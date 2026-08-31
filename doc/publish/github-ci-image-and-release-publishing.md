# GitHub CI 镜像与跨仓 Release 发布

## 1. 目标与边界

本文定义 GitHub Actions、GHCR 和 USDB 跨仓 release manifest 的职责。发布流水线分为普通分支
校验和显式 release tag 构建两层：

- `pull_request` / `master` push 只运行 Fast CI，用于合并前后反馈；
- 两仓同名、不可移动的 annotated release tag 重新运行 Fast gate，并发布 `linux/amd64` 候选镜像；
- 每个镜像绑定 source commit、OCI digest 和 GitHub provenance attestation；
- manifest workflow 只接收 release ID，从两仓同名 tag、compatibility lock、tag build 和 network bundle
  派生 revisions、三个镜像 digest 与 genesis identity；
- candidate manifest 从 bundle 派生并冻结 release-approved snapshot record、可信公钥目录及公开下载身份；
- candidate 与 publish workflow 都必须确认 record 和全部对象已在公开 HTTPS 端点完整可用，节点是否采用该
  snapshot 仍由 `usdb-node setup` 显式选择。

Snapshot 大文件由独立对象存储分发，不进入 GitHub Release；最终 GitHub Release 和节点部署批准仍由
受保护的 publish workflow 负责。
SourceDAO 当前没有独立运行镜像，但其 commit 和 CI check 是 release manifest 的必要输入。

GitHub 官方参考：

- [Publishing Docker images](https://docs.github.com/en/actions/tutorials/publish-packages/publish-docker-images)
- [Artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)
- [GitHub Container Registry permissions](https://docs.github.com/en/packages/learn-github-packages/about-permissions-for-github-packages)
- [Deployment environments](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)

## 2. 三层身份

不要用同一个 version 字符串表达所有层级：

| 层级 | 示例 | 用途 |
| --- | --- | --- |
| 源码候选 | Git commit `0123...` | 三仓独立开发和普通 CI 身份 |
| 发布选择器 | 两仓同名 tag `usdb-testnet-v0-r1` | 冻结本次 USDB / Go commit 组合 |
| OCI artifact | `ghcr.io/buckyos/usdb-chain@sha256:...` | 节点实际拉取和执行的不可变字节 |
| 部署 release | `usdb-testnet-v0-r1` | 一次跨仓、跨 artifact 的部署集合 |
| 网络 bundle | `usdb-testnet-v0` | chain ID、genesis、BTC source 和公共网络身份 |

`deployment release` 更新不一定重置网络。仅替换经过兼容性验证的 binary/image 时可发布 `r2` 并
滚动重启；改变 genesis、链身份或 block-0 activation 时必须生成新 network bundle 和空数据目录。

## 3. Release Tag 与候选镜像流水线

### 3.1 Release tag 规则

`release_id` 同时作为 `buckyos/usdb` 和 `buckyos/go-ethereum` 的 annotated Git tag。v1 命名规则为：

```text
usdb-testnet-v<network-version>-r<release-sequence>
usdb-mainnet-v<network-version>-r<release-sequence>
```

例如 `usdb-testnet-v0-r1`。这里的 `vN` 是 network generation，不是 UIP 或软件大版本；`rN` 是同一
network generation 上不可移动的 deployment release sequence。兼容 binary/image、运维配置、部署
artifact 或支持未来 activation 的代码发生变化时递增 `rN`。genesis、chain ID 或其他 block-0
identity 变化时必须创建新的 network generation，例如从 `usdb-testnet-v0-r3` 转为
`usdb-testnet-v1-r1`，不能继续沿用 `v0`。在线协议升级仍由 activation matrix 的 policy version 表达。

release tag 必须满足：

- 两仓 tag 名完全相同，但分别指向各自被选择的 commit；
- tag target 必须属于仓库 `master` 历史；
- tag 必须是 annotated tag，不能使用 lightweight tag；
- GitHub tag ruleset 必须禁止更新和删除 `usdb-testnet-*` / `usdb-mainnet-*`；
- tag build 因 runner、网络等临时故障失败时，重跑同一个 workflow run；
- 需要修改源码、compatibility lock 或构建输入时，保留旧 tag 并创建下一个 `rN`，不能移动 tag。

两仓 tag push 不是原子操作。只推送成功一个 tag 时允许对应仓库完成独立 release build，但 manifest
必须等两个同名 tag 和两个成功 tag build 都存在后才能生成。SourceDAO 不创建同名 tag，其 revision
由 Go tag commit 内的 compatibility lock 唯一确定。

推荐冻结顺序：

1. push 目标 USDB commit，记录明确的 40 字符 commit `U`，等待普通 Fast CI 通过；
2. 在 Go `scripts/usdb/ci-revisions.json` 中锁定 `U` 和目标 SourceDAO commit；
3. push 目标 Go commit，记录明确的 40 字符 commit `G`，等待普通 Fast / cross-repository golden 通过；
4. 人工确定尚未使用的 `release_id`；
5. 分别在 `U`、`G` 上创建同名 annotated tag，不能隐式使用可能继续变化的本地 `HEAD`；
6. push 两个 tag，等待两仓 `USDB Release Build` 成功；
7. 从同名 release tag 手工运行 manifest workflow；release ID 自动取自该 tag，不再重复输入。

协调工具位于 Go 仓库 `scripts/usdb/prepare_release.py`。默认从脚本所在 `go-ethereum` checkout
推导其上级 workspace，因此标准 sibling 布局不需要传 `--workspace-root`；非标准布局可显式覆盖。
推荐命令：

```bash
cd /path/to/go-ethereum

# 预检，不修改 lock。
python3 scripts/usdb/prepare_release.py sync-lock

# USDB Fast CI 通过后更新 lock，提交并 push Go master。
python3 scripts/usdb/prepare_release.py sync-lock --commit --push

# Go Fast CI 通过后预检 tag，再显式创建和 push。
python3 scripts/usdb/prepare_release.py tag --release-id usdb-testnet-v0-r1
python3 scripts/usdb/prepare_release.py tag \
  --release-id usdb-testnet-v0-r1 --create --push
```

工具只接受 clean、已发布且等于对应远端主分支的 HEAD。两个远端之间无法原子 push；如果 USDB tag
已成功而 Go tag push 失败，必须修复问题后继续 push 已创建的同一 Go tag，不能删除、移动或重建
已经发布的 release tag。`--no-fetch` 仅供明确需要使用现有 remote-tracking refs 的离线检查。

### 3.2 USDB services

入口：

- [USDB Fast](../../.github/workflows/usdb-fast.yml)
- [USDB Release Build](../../.github/workflows/usdb-release-build.yml)
- [USDB Services Image](../../.github/workflows/usdb-services-image.yml)
- [services Dockerfile](../../docker/Dockerfile.usdb-services)

`master` push 的 Rust Fast CI 只执行代码校验。`USDB Release Build` 收到合法 release tag 后重新执行
同一 Fast gate，再调用 image workflow。image workflow 也支持人工诊断，但只有 release-tag build
产生的 run 才能进入跨仓 manifest。选中的 commit 必须属于 `master` 历史。发布名格式为：

```text
ghcr.io/buckyos/usdb-services:git-<40-char-sha>-run-<run-id>-<attempt>
```

### 3.3 USDB chain

入口位于 `go-ethereum`：

```text
.github/workflows/usdb-fast.yml
.github/workflows/usdb-release-build.yml
.github/workflows/usdb-chain-image.yml
Dockerfile
```

普通 `master` Fast 不发布 release image。Go `USDB Release Build` 必须在 release tag 上重新通过 Go Fast
和 cross-repository golden jobs，才调用 chain image workflow：

```text
ghcr.io/buckyos/usdb-chain:git-<40-char-sha>-run-<run-id>-<attempt>
```

### 3.4 Bitcoin Core

入口：

- [Bitcoin image workflow](../../.github/workflows/usdb-bitcoin-image.yml)
- [Bitcoin release Dockerfile](../../docker/Dockerfile.bitcoin-core)
- [Bitcoin image 与同步手册](./bitcoin-core-release-and-sync-operations.md)

该 workflow 由 USDB release-tag build 调用，从同一 `usdb` commit 构建经过上游签名校验的
Bitcoin Core 28.1 image：

```text
ghcr.io/buckyos/usdb-bitcoin-core:bitcoin-28.1-git-<40-char-sha>-run-<run-id>-<attempt>
```

### 3.5 OCI candidate tag 与 digest

不要混淆 Git release tag 和 OCI candidate tag。Git release tag 冻结源码组合；OCI candidate tag
只用于定位每次 workflow run/attempt 的镜像，避免静默覆盖前一次候选。节点和 release manifest
一律使用 digest：

```text
ghcr.io/buckyos/usdb-services@sha256:<64-char-digest>
ghcr.io/buckyos/usdb-chain@sha256:<64-char-digest>
ghcr.io/buckyos/usdb-bitcoin-core@sha256:<64-char-digest>
```

三个 workflow 都写入 OCI source/revision/version labels，生成 BuildKit provenance/SBOM，并通过
`actions/attest` 生成 GitHub attestation。跨仓 coordinator 会同时校验：

- digest 在 GHCR 中存在；
- attestation 来自规定的 signer workflow；
- attestation source digest 等于 manifest 中冻结的 Git commit；
- runner 不是 self-hosted runner。

## 4. 一次性 GitHub 配置

在第一次发布前完成：

1. 允许仓库 workflow 使用 `packages: write`、`attestations: write` 和 `id-token: write`。当前 OCI
   candidate 不创建可选的 GitHub artifact storage record，因此不需要 `artifact-metadata: write`；
   provenance 仍同时写入 GitHub attestation 和 OCI registry。
2. 首次创建 `usdb-services`、`usdb-chain`、`usdb-bitcoin-core` package 后，将其设为 public，便于节点匿名拉取；如果保持
   private，则必须给 `buckyos/usdb` coordinator 仓库授予三个 package 的 Actions read access。
3. 创建 `testnet-release-candidate` Environment，配置 required reviewer、禁止发起人自审，并把允许部署的
   tag 限制为 `usdb-testnet-*`。
4. 创建 `usdb-release` Environment，配置独立 required reviewer、禁止发起人自审、禁止管理员绕过，并只
   允许 `usdb-testnet-*` / `usdb-mainnet-*` tag。
5. Environment 名称被 workflow 引用时 GitHub 可以自动创建无保护 environment，因此首次运行前必须在
   repository settings 中人工确认上述 protection rules 已真实生效。
6. 为 `master/main` 保持 branch protection，确保 release tag 只能指向已进入主线的 commit。
7. 为 `usdb-testnet-*` / `usdb-mainnet-*` 配置 tag ruleset，限制创建者并禁止 update/delete。
8. 不把 snapshot signing key、SourceDAO bootstrap private key 或 BTC RPC secret 放入这些 workflow。

GitHub-hosted runner 是当前 provenance 信任边界。后续若使用 self-hosted release runner，必须重新定义
runner hardening 和 attestation policy；当前 coordinator 会明确拒绝 self-hosted provenance。

## 5. 跨仓 Candidate Manifest

入口：

- [candidate workflow](../../.github/workflows/usdb-release-candidate.yml)
- [manifest tool](../../docker/scripts/tools/release_manifest.py)
- [manifest tests](../../docker/scripts/tools/test_release_manifest.py)

完成第 3 节的 tag build 后，从同一个 release tag 手工运行 `USDB Release Candidate Manifest`。
workflow 不再接受 `release_id` input，而是直接把所选 tag 的 `github.ref_name` 作为 release ID：

在 GitHub Actions 页面点击 `Run workflow` 时，必须先在 `Use workflow from` 下拉框选择同名
release tag，不要保留默认的 `master`；选定后不再需要填写其他字段。命令行只需要提供 `--ref`：

```bash
gh workflow run usdb-release-candidate.yml \
  --repo buckyos/usdb \
  --ref usdb-testnet-v0-r1
```

workflow 会：

1. 解析两仓同名 annotated tag 并取得 USDB / Go revision；
2. 检查两个 tag target 都属于各自 `master` 历史；
3. 从 Go tag commit 的 compatibility lock 取得 SourceDAO revision，并要求其中 USDB revision 等于
   USDB tag target；
4. 在两个仓库分别查找该 tag 上唯一成功的 `USDB Release Build` push run；
5. 从 run ID、attempt 和 source commit 构造唯一 candidate tag，再把 tag 解析为不可变 OCI digest；
6. 严格校验 `testnet-v0` network bundle，并用选中的 Go revision 重算 genesis block hash；
7. 验证三个 OCI artifact 的 digest、signer workflow 和 source commit；
8. 使用 USDB annotated tag 的固定 tagger timestamp 生成确定性 manifest；
9. 再次读取验证 `usdb-release-manifest.json`，上传 manifest 和 SHA-256，保留 30 天供 review；
10. 拒绝已经存在未过期同名 candidate artifact 的第二次独立 dispatch，避免 publish 阶段自动选择“最新”。

不存在 tag、tag 不是 annotated tag、tag target 不在主线、成功 release build 缺失或存在歧义、
compatibility lock 不匹配、genesis hash 漂移或 attestation 不匹配时都会 fail closed。自动解析只用于
消除人工复制错误；最终 manifest 仍记录完整 revisions 和 digest-only image reference。

本地创建同一 schema 的示例：

```bash
python3 docker/scripts/tools/release_manifest.py create \
  --bundle-dir docker/networks/testnet-v0 \
  --output /tmp/usdb-release-manifest.json \
  --release-id usdb-testnet-v0-r1 \
  --created-at-utc <USDB_ANNOTATED_TAG_UTC_TIMESTAMP> \
  --compatibility-lock /path/to/go-ethereum/scripts/usdb/ci-revisions.json \
  --usdb-revision <40-char-sha> \
  --go-ethereum-revision <40-char-sha> \
  --source-dao-revision <40-char-sha> \
  --services-image ghcr.io/buckyos/usdb-services@sha256:<digest> \
  --chain-image ghcr.io/buckyos/usdb-chain@sha256:<digest> \
  --bitcoin-image ghcr.io/buckyos/usdb-bitcoin-core@sha256:<digest>
```

当前 v4 candidate manifest 固定以下边界：

- 只接受 canonical `buckyos` repositories 和 GHCR image names；
- 只接受完整 lowercase Git SHA 和 digest-only image reference；
- 固定平台为 `linux/amd64`；
- 绑定 `network.json`、genesis、BTC origin/registry 和 snapshot trusted-key catalog hash；
- 绑定 Go commit 内的 compatibility lock hash，并拒绝混搭其他 `usdb/SourceDAO` revision；
- 可选 snapshot record、record SHA-256/URL、height、BTC block hash、snapshot ID、下载规模、signer 和
  trusted-key catalog 必须与 tagged bundle 完全一致，且 height 不得超过 BTC index origin；
- candidate 和 publish 都会精确比对公开 content-addressed record，对四个对象执行 HTTPS/Content-Length
  availability 检查，并对 DB 执行 1-byte Range 探测。record 由 uploader 最后发布，因此未完成的大文件
  上传不能进入 candidate；远端整文件 SHA-256 不在 CI 中重复下载计算，节点安装时仍会逐文件完整校验。

## 6. 从 Candidate 到正式 Release

入口：

- [publish workflow](../../.github/workflows/usdb-release-publish.yml)
- [publish resolver](../../docker/scripts/tools/release_publish_resolver.py)
- [publish resolver tests](../../docker/scripts/tools/test_release_publish_resolver.py)

Actions artifact 会过期，因此不能作为节点长期信任入口。candidate review 完成后，从同一 release tag
手工启动 publish workflow：

在网页操作时同样只需要把 `Use workflow from` 设为该 release tag，不再填写重复的 release ID。
publish 直接从 tag 推导 ID；preflight 会重新校验唯一 candidate artifact、tag target 和 release identity，之后才进入
`usdb-release` Environment 审批；审批通过才会创建或验证 GitHub Release。

```bash
gh workflow run usdb-release-publish.yml \
  --repo buckyos/usdb \
  --ref usdb-testnet-v0-r1
```

preflight job 自动定位唯一成功且未过期的 candidate run/artifact，只拥有 read permission。`publish` job
引用固定 `usdb-release` Environment；通过 required reviewer 审批后才取得 `contents:write`，然后：

1. 再次验证两仓 annotated tag target 和主线 ancestry；
2. 下载原 candidate archive，并校验 GitHub Actions artifact digest、archive 文件集合和 manifest checksum；
3. 使用 tagged USDB/Go source 重新校验 v2 compatibility lock、manifest 和 network bundle；
4. 再次验证三张 digest-pinned OCI image 的 signer workflow、source revision 和 provenance；
5. 从 tagged source 生成稳定的 public network bundle、self-contained node kit 及 SHA-256；
6. GitHub Release title 直接使用完整 `release_id`，确保窄列表也优先显示网络和版本；testnet 创建
   prerelease，mainnet 创建普通 release，且两者都不更新 mutable `latest`；
7. release 已存在时只允许 title、notes、flags、完整 asset 集合和每个 SHA-256 全部一致，否则 fail closed。

当前 Release assets 固定为：

- `usdb-release-manifest.json`；
- `usdb-release-manifest.json.sha256`；
- `<release_id>-network-bundle.tar.gz`；
- `<release_id>-network-bundle.tar.gz.sha256`；
- `<release_id>-node-kit.tar.gz`；
- `<release_id>-node-kit.tar.gz.sha256`；
- `install-<release_id>.sh`；
- `install-<release_id>.sh.sha256`；
- `install-usdb-node.sh`；
- `install-usdb-node.sh.sha256`。

node kit 内含 release manifest、network bundle、批准的 snapshot record、部署 Compose 和 `usdb-node`
控制器。它将 image digest、snapshot 来源和服务顺序从人工输入改为 release-owned 输入，但仍不包含
secret、snapshot DB 或节点数据。安装和运行
流程见 [Release Node Kit 与简化部署](./usdb-release-node-kit-and-deployment.md)。

正式节点只按 release ID、manifest hash 和 digest 安装，不能自动追踪 `latest`。private key、RPC credential、
节点本地 `node.env` 和实际 snapshot 大文件不得进入 GitHub Release。v4 manifest 已冻结 snapshot release
record、URL、大小、SHA-256、signer 和 catalog hash；节点仍可在 `setup` 中选择 full sync。PoW 校准、
E2E 与人工批准证据继续保留在对应 workflow/deployment record。

## 7. 当前限制

- 当前 Dockerfile 的所有 base image 尚未固定 digest；候选 artifact 可审计，但未达到完全可重复构建。
- services image 仍包含当前 testnet 不使用的 ord binary 和 Web assets，后续可按发布频率拆分。
- 当前 workflow 只发布 `linux/amd64`，增加架构必须分别完成容量和共识一致性测试。
- publish workflow 当前只接受已冻结的 `usdb-testnet-v0` bundle；未来 mainnet bundle 冻结后需显式加入映射。
- GitHub Environment protection rules、tag ruleset 和 release immutability 是 repository settings，不能只靠
  workflow 文件声明，首次发布前必须人工复核。
- publish workflow 不更新节点 `node.env`，也不上传实际 snapshot 大文件。
- `scripts/usdb/ci-revisions.json` 仍是联合 CI baseline，不是 release manifest。
