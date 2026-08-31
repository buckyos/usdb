# Balance-History Snapshot 签名与信任说明

## 1. 文档定位

本文说明 balance-history snapshot 的签名对象、密钥边界、接收方信任配置、发布包集成和密钥
轮换原则。它不描述 exact-height builder 的完整运行步骤，也不定义整个 USDB 的发布顺序。

相关文档：

- [Exact-Height Snapshot Tool Design](./balance-history-exact-height-snapshot-tool-design.md)
- [主网 Exact-Height Snapshot 操作指南](./balance-history-mainnet-exact-height-snapshot-operations.md)
- [Balance-History 发布与 Snapshot 分发](../publish/balance-history-release-and-snapshot-distribution.md)

## 2. 签名保护的对象

正式 snapshot artifact 包含：

```text
snapshot_<height>.db
snapshot_<height>.manifest.json
snapshot_<height>.manifest.sig
complete.json
```

当前使用 Ed25519 私钥对 canonical manifest bytes 生成 detached signature。签名不直接覆盖 tar
或 balance-history 二进制，但 manifest 会固定：

- snapshot DB 文件名和 SHA-256；
- exact BTC height、canonical block hash 和 state-ref；
- snapshot ID、manifest version、signature scheme 和 signer key ID；
- manifest 生成时间。

合法签名能够证明受信发布方认可了该 manifest，而 manifest 再约束 snapshot DB 内容和状态
身份。它不能替代 BTC canonical hash 检查、snapshot DB 重开校验、state-ref 校验或独立安装
验证。

## 3. 与其他发布签名的边界

Snapshot signer 只用于 snapshot manifest，不用于：

- Git commit/tag 签名；
- balance-history 或其他 USDB 二进制签名；
- Docker/OCI image 签名；
- release tar、系统安装包或 CI provenance 签名；
- TLS、Windows Authenticode 或通用 X.509 身份。

这些用途必须使用不同密钥和权限域。普通 CI runner 不应读取 snapshot signing key。未来如果
由发布流水线签发 snapshot，应使用隔离 runner 或 secret manager，并只向该签名步骤授予最小
权限。

## 4. 密钥文件

`balance-history snapshot-keygen` 生成三个文件：

```text
<key-id>.signing-key.json
<key-id>.public-key.json
<key-id>.trusted-keys.json
```

### 4.1 私钥

```json
{
  "key_id": "usdb-mainnet-snapshot-v1",
  "secret_key_base64": "<base64 raw 32-byte ed25519 seed>"
}
```

`signing-key.json` 是私密材料：

- 只保留在受控 snapshot 生成环境；
- 不提交 Git，不进入普通节点镜像，不随 snapshot 或 release bundle 分发；
- 应进行离线加密备份，并记录恢复演练结果；
- 文件权限应限制为生成用户可读。

### 4.2 公钥与 trusted-key catalog

```json
{
  "keys": [
    {
      "key_id": "usdb-mainnet-snapshot-v1",
      "public_key_base64": "<base64 raw 32-byte ed25519 public key>"
    }
  ]
}
```

`public-key.json` 和 `trusted-keys.json` 不包含秘密。接收方实际使用的是 trusted-key catalog，
其中可以同时放置多个 signer，支持密钥轮换过渡期。

## 5. 发布方配置

生成端配置只引用私钥：

```toml
[snapshot]
trust_mode = "signed"
signing_key_file = "/home/usdb/.usdb/secure/snapshot-keys/usdb-mainnet-snapshot-v1.signing-key.json"
```

首次正式生成可使用：

```bash
balance-history \
  --root-dir /path/to/keygen-workspace \
  snapshot-keygen \
  --key-id usdb-mainnet-snapshot-v1 \
  --out-dir /path/to/private-key-root
```

同一个逻辑 signer ID 不得静默替换成另一把公钥。需要轮换时必须创建新的 key ID。

## 6. 接收方配置

接收方不能获得 `signing_key_file`，只配置 trusted-key catalog：

```toml
[snapshot]
trust_mode = "signed"
trusted_keys_file = "/etc/usdb/snapshot-keys/usdb-mainnet-snapshot-v1.trusted-keys.json"
```

无 root 部署可以使用：

```text
~/.usdb/trust/snapshot-keys/usdb-mainnet-snapshot-v1.trusted-keys.json
```

Signed install 会依次验证 manifest、signature scheme、signer ID、trusted public key、detached
signature、snapshot DB hash 和 staged state-ref。验证结果及 signer ID 会写入 snapshot install
provenance。trusted-key 文件仍应长期保留，以支持后续 snapshot 更新、重装和审计。

## 7. 发布包中的 trusted-key

首版采用“外部 catalog、随 balance-history release bundle 安装”的方式，不把完整公钥列表硬
编码进 Rust 二进制。推荐 bundle 布局：

```text
bin/balance-history
bin/balance-history-snapshot-tool
etc/balance-history/config.toml.example
share/usdb/trust/usdb-mainnet-snapshot-v1.trusted-keys.json
release-manifest.json
install.sh
```

安装器负责：

1. 校验 release manifest 和 bundle 文件 hash；
2. 把 trusted-key catalog 安装到 `/etc/usdb/snapshot-keys/` 或用户 trust 目录；
3. 生成或检查 `trust_mode = "signed"` 配置；
4. 确认接收方配置中不存在 `signing_key_file`；
5. 执行 snapshot install 前输出 signer ID 和 catalog SHA-256 供审计。

不能只把 trusted-key 放入 snapshot tar 并依赖同一下载渠道，因为攻击者可能同时替换 snapshot、
签名和公钥。初始信任至少应由以下一种独立来源固定：

- 经过 review 的代码 revision 中的 public catalog；
- 正式 release manifest 和独立公布的 manifest hash；
- 签名的系统包或 OCI image；
- 官方文档或其他独立可信渠道公布的 key ID、公钥和 catalog SHA-256。

当前项目尚未建立正式 CI 和通用二进制签名，因此首次 public release 必须人工交叉核对代码
revision、release bundle hash 和 trusted-key catalog hash。

## 8. 密钥轮换与失陷处理

正常轮换建议：

1. 生成新的唯一 key ID；
2. 发布同时包含旧、新 public key 的 catalog；
3. 使用新 key 签发一轮 snapshot，并完成接收方交叉验证；
4. 经过明确过渡期后，在后续 release catalog 中移除旧 key；
5. 保留旧 public key 和历史 release manifest 供历史 artifact 审计。

私钥疑似失陷时应立即停止签发，公布受影响 key ID 和最后可信 snapshot，并发布新的 trusted-key
catalog。当前实现没有在线撤销服务；仍使用旧 catalog 的节点不会自动获知撤销，因此 public
release 前必须冻结 catalog 更新和安全公告渠道。

## 9. 当前实现状态

已经具备：

- Ed25519 keygen、manifest signing 和 detached signature；
- signed install、未知 signer/错误签名/篡改拒绝测试；
- snapshot install provenance；
- 主网 wrapper 的生成端 key/config 初始化、轻量 artifact finalization，以及显式独立
  `validate-install` 恢复演练。

仍需在发布阶段完成：

- 生成并 review 正式 mainnet snapshot public catalog；
- balance-history release bundle 和 installer 自动携带 catalog；
- release manifest、bundle hash 和可信公布渠道；
- key rotation、失陷公告和恢复演练的正式操作记录。
