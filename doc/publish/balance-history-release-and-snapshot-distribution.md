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

可选 snapshot 独立分发。节点下载路径发布原始 immutable files，tar 仅保留为离线归档：

```text
snapshot_<H>.db
snapshot_<H>.manifest.json
snapshot_<H>.manifest.sig
complete.json
snapshot-records/v2/<record-sha256>.json
```

发布对象不包含 signing private key、builder workspace、job state、接收方 trusted-key catalog 或
validation DB。对象存储具体契约和命令见
[Snapshot 对象存储发布与安装](./balance-history-snapshot-object-storage.md)。

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

状态：`implemented`；真实 R2 大文件发布验收 pending。

在生成机执行：

```bash
SCRIPT=src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh

bash "$SCRIPT" init
bash "$SCRIPT" preflight --height "$H"
bash "$SCRIPT" create --height "$H"
bash "$SCRIPT" resume-verify --height "$H" # interrupted verifying jobs only
bash "$SCRIPT" verify --height "$H" # optional full re-verification
bash "$SCRIPT" finalize --height "$H"
bash "$SCRIPT" validate-install --height "$H" # optional independent restore drill
bash "$SCRIPT" prepare-release --height "$H" # optional review gate
bash "$SCRIPT" publish --height "$H"
bash "$SCRIPT" archive --height "$H" # optional offline archive only
```

`create` 和 `resume-verify` 在验证期间每 30 秒向终端 `stderr` 输出一次带本地时间的进度，
包括当前 phase、phase elapsed 和本轮 verify elapsed；phase 开始、完成或失败时也会立即输出。
机器可读的最终 JSON 仍独占 `stdout`。终端断开或需要从另一个会话观察时，可运行：

```bash
bash "$SCRIPT" status --height "$H"
tail -F ~/.usdb/balance-history-snapshot-mainnet/builder/logs/balance-history-snapshot-tool_rCURRENT.log
```

`finalize` 的整文件 SHA-256 扫描会在交互式终端显示已读取字节、文件总量、吞吐、百分比和 ETA。
`publish` 会先显示上传前本地 SHA-256 扫描进度，再显示 AWS CLI 的实际上传字节进度。两者都写入
`stderr`，不会混入最终 JSON/report；非交互式 CI 默认关闭动态进度，必要时可设置
`USDB_SNAPSHOT_FORCE_PROGRESS=1` 强制打开 publish 侧进度。

`create` 已包含完整 SQLite integrity/count 验证。`finalize` 只重新核对 artifact/manifest/complete
identity、整文件 SHA-256 和 Ed25519 签名，不打开 SQLite，也不创建 RocksDB；因此生产机上传不再
需要额外预留一份恢复后 RocksDB 的磁盘空间。`validate-install` 才执行完整的接收方视角恢复，并写入
独立 validation report；首次主网发布、snapshot schema/installer 升级和周期性恢复演练建议执行，
但它不是每次对象存储上传的硬门槛。

日常对象存储发布只需要高度参数。脚本从 pinned target 和 finalize 结果推导 BTC hash、producer
revision、artifact、artifact-finalization marker 和 trusted catalog；对象存储默认值与高级覆盖项见
[Snapshot 对象存储发布与安装](./balance-history-snapshot-object-storage.md)。
`publish` 不依赖且不会生成 tar；离线介质或冷备明确需要单文件归档时才执行 `archive`。

发布前必须确认：

- target `H/hash`、确认深度和代码 revision 已固定；
- create/verify 的 snapshot ID、state-ref、计数和 file SHA-256 一致；
- artifact-finalization marker 与 complete/manifest、producer revision 和 trusted catalog 一致；
- 首次正式发布或 installer/schema 变更时，独立 `validate-install` 恢复演练成功；
- direct-file release record 可重算；若选择生成离线 archive，其 tar checksum 可在 release 目录中复核；
- manifest signer 存在于即将随 binary bundle 发布的 trusted-key catalog；
- snapshot release record 分别引用 artifact producer/finalizer revision 和 catalog hash；接收方 binary/network compatibility
  由 release manifest 与 runtime validator 独立校验；
- 生成机私钥和 mutable builder 内容不在待发布目录中。

## 6. 接收方安装流程

状态：`implemented`，目标机 live 验收 pending。

正确顺序是：

1. 安装或解包 balance-history release bundle；
2. 通过独立渠道核对 release manifest 和 trusted-key catalog hash；
3. 安装 catalog 并生成 `trust_mode = "signed"` 配置；
4. 通过 content-addressed release record 断点下载 snapshot files；
5. 校验逐文件 size/SHA-256，并保持 DB、manifest、signature 相邻；
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
- 最终 published release manifest 的 snapshot record URL/hash 扩展尚未冻结；
- binary/OCI signing、SBOM 和正式 CI 尚未落地；
- catalog rotation/revocation 的公开渠道和演练尚未完成。

在这些事项完成前，可以通过 R2 工具链生成和验证正式候选 snapshot，但面向第三方的 public
distribution 仍应标记为 candidate，而不是最终 promoted release。
