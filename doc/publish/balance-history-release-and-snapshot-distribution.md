# Balance-History 发布与 Snapshot 分发

## 1. 目标

本文规定 balance-history binary、public trusted-key catalog 和可选 exact-height snapshot 如何
进入 USDB release。Snapshot 的格式、签名算法和 builder 操作分别见：

- [Snapshot 签名与信任说明](../balance-history/balance-history-snapshot-signing.md)
- [Exact-Height Snapshot Tool Design](../balance-history/balance-history-exact-height-snapshot-tool-design.md)
- [主网 Exact-Height Snapshot 操作指南](../balance-history/balance-history-mainnet-exact-height-snapshot-operations.md)

## 2. 发布物边界

Balance-history release bundle 至少包含：

```text
bin/balance-history
bin/balance-history-snapshot-tool
etc/balance-history/config.toml.example
share/usdb/trust/<snapshot-catalog>.trusted-keys.json
release-manifest.json
install.sh
```

可选 snapshot 独立分发：

```text
balance-history-mainnet-<H>-<BTC_HASH>.tar
balance-history-mainnet-<H>-<BTC_HASH>.tar.sha256
snapshot-release-record.json
```

Snapshot tar 内只包含 immutable artifact，不包含 signing private key、builder workspace、job
state、接收方 trusted-key catalog 或 validation DB。

## 3. Public trusted-key catalog

生成机器上的默认目录 `~/.usdb/secure/snapshot-keys` 同时包含 private/public material，不能整体
复制到 release。发布步骤只提取经过 review 的 `*.trusted-keys.json`，并记录：

- signer key ID；
- public key；
- catalog schema/version；
- catalog SHA-256；
- 生效 release 和轮换状态；
- review/approval 记录。

首版 catalog 保持外部文件，不硬编码进 balance-history。正式 bundle 的安装脚本将它部署到：

```text
/etc/usdb/snapshot-keys/<snapshot-catalog>.trusted-keys.json
```

无 root 安装则使用：

```text
~/.usdb/trust/snapshot-keys/<snapshot-catalog>.trusted-keys.json
```

Catalog 不能仅依赖 snapshot 下载站点建立信任。其 hash 必须同时固定在 release manifest 和一个
独立可信发布渠道中。当前尚无正式 binary/image signing，首版需要从冻结代码 revision、人工
review 记录和公布 hash 三方交叉确认。

## 4. Binary bundle 构建

状态：`planned`

当前已有 release binary 构建和 Docker 开发镜像，但尚无完整 balance-history public release
installer。实现后至少执行：

1. 从冻结 commit 构建 release binaries；
2. 记录 Rust toolchain、features 和目标平台；
3. 复制 config template 和 public trusted-key catalog；
4. 生成逐文件 SHA-256 和 release manifest；
5. 扫描 bundle，确保不存在 `secret_key_base64`、`signing-key.json`、私有 env 或 BTC cookie；
6. 在空目录/空容器执行安装 smoke；
7. 校验默认配置为 `trust_mode = "signed"` 且 trusted key 路径存在；
8. 记录 binary `--version`、commit 和 bundle hash。

## 5. Snapshot 生成和发布

状态：`implemented/manual`

在生成机执行：

```bash
SCRIPT=src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh

bash "$SCRIPT" init
bash "$SCRIPT" preflight --height "$H"
bash "$SCRIPT" create --height "$H"
bash "$SCRIPT" verify --height "$H"
bash "$SCRIPT" finalize --height "$H"
```

发布前必须确认：

- target `H/hash`、确认深度和代码 revision 已固定；
- create/verify 的 snapshot ID、state-ref、计数和 file SHA-256 一致；
- 独立 validation root 的 signed install 成功；
- tar checksum 可在 release 目录中复核；
- manifest signer 存在于即将随 binary bundle 发布的 trusted-key catalog；
- snapshot release record 引用 binary bundle 和 catalog hash；
- 生成机私钥和 mutable builder 内容不在待发布目录中。

## 6. 接收方安装流程

状态：`manual`

正确顺序是：

1. 安装或解包 balance-history release bundle；
2. 通过独立渠道核对 release manifest 和 trusted-key catalog hash；
3. 安装 catalog 并生成 `trust_mode = "signed"` 配置；
4. 下载 snapshot tar/checksum 和 snapshot release record；
5. 校验 tar hash 并保持 DB、manifest、signature 相邻；
6. 停止现有 balance-history，或使用全新 root；
7. 执行 `install-snapshot`；
8. 检查 snapshot provenance 后启动服务；
9. 等待服务从 `H` 同步到当前 stable height 并检查 readiness。

接收方配置示例：

```toml
[snapshot]
trust_mode = "signed"
trusted_keys_file = "/etc/usdb/snapshot-keys/usdb-mainnet-snapshot-v1.trusted-keys.json"
```

接收方不得配置或持有 `signing_key_file`。

## 7. 发布验收矩阵

| 检查 | 预期 |
| --- | --- |
| release bundle 不含私钥 | pass |
| trusted catalog hash 与 release record 一致 | pass |
| 正确 signer snapshot 安装 | pass |
| 未知 signer、错误签名、篡改 manifest/DB | 全部拒绝 |
| snapshot provenance | `signature_verified=true`，key ID/hash/height 正确 |
| restart | 保持安装 provenance 并继续同步 |
| joiner | 仅使用公开 bundle、catalog 和 snapshot 即可启动 |
| 无 snapshot | 仍可从创世同步，snapshot 不是共识前提 |

## 8. 尚未完成

- 正式 mainnet signer 和 public catalog 尚未冻结；
- binary bundle/install script 尚未实现；
- 最终 published release manifest 的 snapshot/promotion 扩展尚未冻结；
- binary/OCI signing、SBOM 和正式 CI 尚未落地；
- catalog rotation/revocation 的公开渠道和演练尚未完成。

在这些事项完成前，可以生成和验证正式候选 snapshot，但面向第三方的 public distribution 仍应
标记为 manual candidate，而不是最终自动化 release。
