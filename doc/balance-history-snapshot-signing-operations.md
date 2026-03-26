# balance-history 快照签名配置与运维说明

## 1. 目标

`balance-history` 当前支持：

- 生成带 sidecar manifest 的快照
- 使用 Ed25519 对 manifest 做 detached signature
- 在 snapshot install 时校验受信签名

这份文档记录：

- 配置项如何使用
- 私钥、公钥文件格式
- 发布方和消费方各自应如何管理密钥
- 当前是否适合直接依赖系统自带或第三方通用工具

## 2. 当前设计

快照发布物现在推荐采用三件套：

- `snapshot_<height>.db`
- `snapshot_<height>.manifest.json`
- `snapshot_<height>.manifest.sig`

其中：

- `manifest.json` 包含：
  - `file_sha256`
  - `state_ref`
  - `manifest_version`
  - `signature_scheme`
  - `signing_key_id`
  - `generated_at`
- `manifest.sig` 是对 manifest 的 detached signature

当前签名算法固定为：

- `ed25519`

## 3. 配置项

相关配置位于：

- [config.rs](/home/bucky/work/usdb/src/btc/balance-history/src/config.rs)

`config.toml` 示例：

```toml
[snapshot]
trust_mode = "signed"
signing_key_file = "snapshot_signing_key.json"
trusted_keys_file = "trusted_snapshot_keys.json"
```

说明：

- `trust_mode`
  - `dev`
    - 允许无 manifest / 无签名
  - `manifest`
    - 要求 manifest，并验证 staged state-ref
  - `signed`
    - 要求 manifest + `manifest.sig` + trusted signer
- `signing_key_file`
  - 仅发布方需要
  - 用于创建 snapshot 时给 manifest 签名
- `trusted_keys_file`
  - 安装方需要
  - 用于 install 时验证签名

路径规则：

- 绝对路径：直接使用
- 相对路径：相对 `balance-history` 的 `root_dir`

## 4. 私钥文件格式

私钥文件当前格式是 JSON，对应代码中的：

- `SnapshotSigningKeyFile`

文件示例：

```json
{
  "key_id": "snapshot-signer-1",
  "secret_key_base64": "<base64 of raw 32-byte ed25519 seed>"
}
```

字段说明：

- `key_id`
  - 逻辑 signer 标识
  - 会进入 manifest 的 `signing_key_id`
- `secret_key_base64`
  - Ed25519 原始 32-byte seed
  - 再做 base64 编码

注意：

- 这里不是 PKCS#8 PEM
- 也不是 OpenSSH private key
- 也不是 age/SSH 常见文本格式

## 5. 公钥信任集格式

公钥信任集当前格式是 JSON，对应代码中的：

- `SnapshotTrustedKeySet`
- `SnapshotTrustedPublicKey`

文件示例：

```json
{
  "keys": [
    {
      "key_id": "snapshot-signer-1",
      "public_key_base64": "<base64 of raw 32-byte ed25519 public key>"
    }
  ]
}
```

字段说明：

- `key_id`
  - 必须与 manifest 中的 `signing_key_id` 一致
- `public_key_base64`
  - Ed25519 原始 32-byte public key
  - 再做 base64 编码

## 6. 发布方如何使用

发布方节点需要：

1. 配置 `signing_key_file`
2. 创建 snapshot
3. 产出：
   - snapshot DB
   - manifest
   - manifest signature

建议发布方模型：

- 私钥只保留在 snapshot 发布机
- 不进入普通节点镜像
- 不放进仓库
- 不随 Docker 镜像分发

## 7. 安装方如何使用

安装方节点需要：

1. 配置 `trusted_keys_file`
2. 将 `trust_mode` 设为：
   - `signed`
3. 安装 snapshot

安装时会：

1. 读取 manifest
2. 从 manifest 中读取：
   - `signing_key_id`
   - `signature_scheme`
3. 加载 `manifest.sig`
4. 在本地 trusted key set 中按 `key_id` 查找公钥
5. 校验签名
6. 再继续 staged install 和 state-ref 验证

## 8. 推荐的运维边界

建议把角色分成两类：

### 8.1 发布方

持有：

- `signing_key_file`

职责：

- 生成和签发 snapshot

### 8.2 消费方 / 普通节点

持有：

- `trusted_keys_file`

职责：

- 验证 snapshot 来源是否可信

这样可以避免：

- 每个节点都持有发布私钥
- 私钥进入 Docker 镜像
- 私钥进入自动化部署模板

## 9. 当前是否适合直接使用系统自带或通用第三方工具

结论：

- **可以借助第三方工具生成 Ed25519 密钥**
- 但**当前并没有一个“即拿即用”的系统默认工具，能直接输出本项目要求的 JSON 格式**

原因是当前项目使用的格式是：

- 原始 32-byte seed
- 原始 32-byte public key
- 再做 base64

而常见工具通常输出的是：

- PEM / PKCS#8
- OpenSSH private key
- OpenSSH public key
- 其他带封装的文本格式

这意味着：

- 即使使用 `openssl`、`ssh-keygen` 或其他通用工具生成 Ed25519 密钥
- 仍然需要额外做一次格式转换

所以从运维角度看：

- **现在不是不能用第三方工具**
- 而是**转换步骤不够直接，容易出错**

## 10. 当前建议

当前更推荐的做法是：

1. 先把 snapshot signing 的文件格式固定下来
2. 后续补一个 repo 内置的小工具，例如：
   - keygen
   - public-key export
   - trusted key set append/update
3. 再把这套流程接进 Docker / 发布脚本

原因是：

- 可以保证生成格式和 install 逻辑完全一致
- 可以减少人工转换错误
- 可以把 key rotation / signer onboarding 做成标准流程

## 11. 当前阶段的实际建议

如果现在就要使用这套功能，建议：

- 正式/准正式环境：
  - 使用固定的手工生成密钥文件
  - 将 trusted key set 作为节点配置的一部分分发
- 开发环境：
  - 可先用 `manifest` 模式
  - 签名能力用于联调和发布流程验证

## 12. 后续建议补充

这条能力后续最值得补的是：

1. 内置 keygen 工具
2. trusted key set 管理工具
3. signer rotation / revoke 文档
4. Docker / snapshot-loader 接入说明
