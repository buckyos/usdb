# GitHub CI 镜像与跨仓 Release 发布

## 1. 目标与边界

本文定义 GitHub Actions、GHCR 和 USDB 跨仓 release manifest 的职责。当前实现覆盖：

- `usdb` 和 `go-ethereum` Fast CI 成功后发布 `linux/amd64` 候选镜像；
- 每个镜像绑定 source commit、OCI digest 和 GitHub provenance attestation；
- 手工选择 services、chain、Bitcoin Core 三个镜像 digest 与三仓 commit，生成严格校验的跨仓 candidate manifest；
- candidate manifest 从 bundle 派生 `not_used/full-sync` 或 `pending/signed-snapshot`，且不能直接作为 public release。

Snapshot 大文件分发、最终 GitHub Release 和节点部署批准属于后续批次。
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
| 源码候选 | Git commit `0123...` | 三仓独立开发和 CI 身份 |
| OCI artifact | `ghcr.io/buckyos/usdb-chain@sha256:...` | 节点实际拉取和执行的不可变字节 |
| 部署 release | `usdb-testnet-v0-r1` | 一次跨仓、跨 artifact 的部署集合 |
| 网络 bundle | `usdb-testnet-v0` | chain ID、genesis、BTC source 和公共网络身份 |

`deployment release` 更新不一定重置网络。仅替换经过兼容性验证的 binary/image 时可发布 `r2` 并
滚动重启；改变 genesis、链身份或 block-0 activation 时必须生成新 network bundle 和空数据目录。

## 3. 候选镜像流水线

### 3.1 USDB services

入口：

- [USDB Fast](../../.github/workflows/usdb-fast.yml)
- [USDB Services Image](../../.github/workflows/usdb-services-image.yml)
- [services Dockerfile](../../docker/Dockerfile.usdb-services)

`master` push 的 Rust Fast CI 成功后调用 image workflow。workflow 也支持人工触发，但选中的 commit
必须属于 `master` 历史。发布名格式为：

```text
ghcr.io/buckyos/usdb-services:git-<40-char-sha>-run-<run-id>-<attempt>
```

### 3.2 USDB chain

入口位于 `go-ethereum`：

```text
.github/workflows/usdb-fast.yml
.github/workflows/usdb-chain-image.yml
Dockerfile
```

只有 Go Fast 和 cross-repository golden jobs 都成功，才调用 chain image workflow：

```text
ghcr.io/buckyos/usdb-chain:git-<40-char-sha>-run-<run-id>-<attempt>
```

### 3.3 Bitcoin Core

入口：

- [Bitcoin image workflow](../../.github/workflows/usdb-bitcoin-image.yml)
- [Bitcoin release Dockerfile](../../docker/Dockerfile.bitcoin-core)
- [Bitcoin image 与同步手册](./bitcoin-core-release-and-sync-operations.md)

该 workflow 从同一 `usdb` commit 构建经过上游签名校验的 Bitcoin Core 28.1 image：

```text
ghcr.io/buckyos/usdb-bitcoin-core:bitcoin-28.1-git-<40-char-sha>-run-<run-id>-<attempt>
```

### 3.4 Tag 与 digest

候选 tag 用于人工查找，不是部署身份。每次 workflow run/attempt 使用唯一 tag，避免静默覆盖前一次
候选。节点和 release manifest 一律使用：

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

1. 允许仓库 workflow 使用 `packages: write`、`attestations: write` 和 `id-token: write`。
2. 首次创建 `usdb-services`、`usdb-chain`、`usdb-bitcoin-core` package 后，将其设为 public，便于节点匿名拉取；如果保持
   private，则必须给 `buckyos/usdb` coordinator 仓库授予三个 package 的 Actions read access。
3. 创建 `testnet-release-candidate` Environment，配置 required reviewer 并禁止发起人自审。
4. 为 `master/main` 保持 branch protection，确保 image workflow 只能消费已进入主线的 commit。
5. 不把 snapshot signing key、SourceDAO bootstrap private key 或 BTC RPC secret 放入这些 workflow。

GitHub-hosted runner 是当前 provenance 信任边界。后续若使用 self-hosted release runner，必须重新定义
runner hardening 和 attestation policy；当前 coordinator 会明确拒绝 self-hosted provenance。

## 5. 跨仓 Candidate Manifest

入口：

- [candidate workflow](../../.github/workflows/usdb-release-candidate.yml)
- [manifest tool](../../docker/scripts/tools/release_manifest.py)
- [manifest tests](../../docker/scripts/tools/test_release_manifest.py)

候选组合的推荐冻结顺序是：

1. 合并并通过目标 `usdb`、`SourceDAO` commit 的 Fast CI；等待 services 和 Bitcoin candidate image 完成。
2. 在 `go-ethereum/scripts/usdb/ci-revisions.json` 中锁定上述两个 commit，完成 cross-repository golden
   验证并等待 chain candidate image 完成。
3. 从被锁定的 `usdb` commit 运行 manifest workflow，选择三个实际 OCI digest。

这个顺序避免循环依赖：Go lock 中的自身 revision 仍是联合 CI baseline，release 的最终 Go revision
由跨仓 manifest 冻结；但 lock 中的 `usdb`、`SourceDAO` revision 必须与 release 选择完全一致。

在 GitHub Actions 中手工运行 `USDB Release Candidate Manifest`，输入：

- `release_id`，例如 `usdb-testnet-v0-r1`；
- 完整 `go-ethereum` 和 `SourceDAO` commit；
- services/chain/Bitcoin Core 三个 GHCR digest reference；
- 冻结的 genesis block hash。

`usdb` revision 取 workflow 自身的 `github.sha`，不能由字符串输入替换。workflow 会：

1. 检查三仓 revision 都属于各自主分支历史；
2. 要求所选 `usdb`、`SourceDAO` revision 与 Go commit 内 `ci-revisions.json` 的兼容性锁一致；
3. 检查该 commit 上规定的 Fast CI jobs 已成功；
4. 严格校验 `testnet-v0` network bundle；
5. 验证三个 OCI artifact 的 digest、signer workflow 和 source commit；
6. 生成并再次读取验证 `usdb-release-manifest.json`；
7. 上传 manifest 和 SHA-256，保留 30 天供 review。

本地创建同一 schema 的示例：

```bash
python3 docker/scripts/tools/release_manifest.py create \
  --bundle-dir docker/networks/testnet-v0 \
  --output /tmp/usdb-release-manifest.json \
  --release-id usdb-testnet-v0-r1 \
  --genesis-block-hash 0xac89ddec1c12efa4173c67e70772861def1e121c387b612e702805161970e560 \
  --compatibility-lock /path/to/go-ethereum/scripts/usdb/ci-revisions.json \
  --usdb-revision <40-char-sha> \
  --go-ethereum-revision <40-char-sha> \
  --source-dao-revision <40-char-sha> \
  --services-image ghcr.io/buckyos/usdb-services@sha256:<digest> \
  --chain-image ghcr.io/buckyos/usdb-chain@sha256:<digest> \
  --bitcoin-image ghcr.io/buckyos/usdb-bitcoin-core@sha256:<digest>
```

当前 v2 candidate manifest 固定以下边界：

- 只接受 canonical `buckyos` repositories 和 GHCR image names；
- 只接受完整 lowercase Git SHA 和 digest-only image reference；
- 固定平台为 `linux/amd64`；
- 绑定 `network.json`、genesis、BTC origin/registry 和 snapshot trusted-key catalog hash；
- 绑定 Go commit 内的 compatibility lock hash，并拒绝混搭其他 `usdb/SourceDAO` revision；
- snapshot 状态必须与 bundle 一致；当前 full-sync testnet-v0 为
  `{"status":"not_used","bootstrap_mode":"full-sync"}`。

## 6. 从 Candidate 到正式 Release

Actions artifact 会过期，因此不能作为节点长期信任入口。后续 promote 流程必须使用同一份 candidate
manifest，并补齐实际采用的发布证据：

- 使用 snapshot 时补 snapshot release record、URL、大小、SHA-256、signer 和 catalog hash；
- full-sync 时保留 `snapshot.status=not_used` 并归档数据层 readiness/state-ref；
- PoW 校准报告、完整 E2E 报告与人工批准记录；
- 最终 manifest 签名/attestation。

完成后创建不可变 GitHub Release，并把最终 manifest、checksum、public catalog 和小型报告作为 release
assets。正式节点只按 release ID 和 digest 安装，不能自动追踪 `latest`。

## 7. 当前限制

- 当前 Dockerfile 的所有 base image 尚未固定 digest；候选 artifact 可审计，但未达到完全可重复构建。
- services image 仍包含当前 testnet 不使用的 ord binary 和 Web assets，后续可按发布频率拆分。
- 当前 workflow 只发布 `linux/amd64`，增加架构必须分别完成容量和共识一致性测试。
- candidate workflow 尚不创建 GitHub Release，也不更新节点 `node.env`。
- `scripts/usdb/ci-revisions.json` 仍是联合 CI baseline，不是 release manifest。
