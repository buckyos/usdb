# testnet-v1 网络包与首版发布准备

本批准备 `usdb-testnet-v1-r1`。v1 是开发测试网的一次重置，USDB 链从 block 0 开始；
不迁移 v0 的区块、挖矿余额、运行期 SourceDAO 状态和 indexer 数据。实际重置在发布后执行。
源码中的 v0 包保留作为历史身份和可复用输入，不代表新的 MinerPass 程序继续支持 V1 规则。

## r4 后的 schema 分层修正

后续开发发布使用 JSON schema v1 + 状态机 v2；r1–r4 的 schema/state 双 v2 配置撤回。
新 registry 和 active-version-set 身份已重新计算，并同步到 Rust、Go 和网络包。
chain ID、network ID、BTC origin 和创世分配保持不变；genesis 配置中的 registry 绑定、文件校验和、
network bundle 与运行数据 compatibility ID 更新。创世 block hash 不包含 chain config，
因此相同 genesis hash 不能替代精确 registry/config 一致性检查。

此调整仅用于已允许重置的开发网，不提供旧 schema v2 铭文或旧索引状态的兼容迁移。
miner、follower 与 indexer 必须一起切换到同一新发布。新包会派生新的 indexer 数据路径；
USDB chain 的目录名仍为 `networks/usdb-testnet-v1/usdb-chain`，不会仅因更新包而自动隔离旧状态。
部署时必须使用空的 USDB chain 数据目录，不复用旧 indexer/USDB chain 状态；
Bitcoin/BH 基础数据仍按原兼容合同复用。
不能将此做法用于正式网：正式网应按网络 scope、激活高度与历史规则设计升级。

## 固定身份

| 项目 | v1 值 |
| --- | --- |
| bundle | `usdb-testnet-v1` |
| chain ID / network ID | `202610030`（2026-10-03 + 当日序号 0，与 v0 编号方式一致） |
| genesis block hash | `0xb0b6ebc9a6c2e051855c2d61dff9a51635ea1db09c898a3c0126e27b9e779314` |
| genesis 文件 SHA-256 | `3417be9f2a9614d330fa68caa298b2fa6ef3876a1db6f8c2b56cce8d32500fba` |
| BTC 数据源 / origin | `btc-mainnet` / `963800` |
| origin block hash | `000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551` |
| rules scope | `usdb-testnet-v1` |
| registry ID | `53b4bfed53b55a4accbd947d04d113a684f1af896d39d2d9bd984e07ad543f32` |
| 稳定滞后 | 10 BTC blocks |

规则从索引 origin 即使用 MinerPass V2；registry 的全域生效点为 BTC height 0，
USDB genesis 的 activation 从 USDB block 0 绑定此 registry。两个高度轴不可混淆。
独立 scope 不改变 BTC 主网本身或其他 USDB 网络的规则。

Geth bootstrap 生成器根据新的 chain ID 重新计算 `0x1000` 系统账户中的固定价格 range ID，
因此创世 state root 和 block hash 都与 v0 不同。不能只编辑 genesis.config.chainId 并保留旧 alloc。
`extraData` 等不相关参数保持原值。
## 复用与重新生成

保留 BTC origin、P2P/RPC 端口、PoW 初始及最低难度、anchor 最大年龄、fee split block，
以及 SourceDAO 合约预部署代码、admin 和初始分配/委员会配置。复用的是冻结初始配置，
不是旧网当前余额或合约存储。SourceDAO ceremony 必须在新链重新执行并验收。

`usdb-genesis.manifest.json` 的 derivation 记录复用的 v0 冻结 genesis 来源；
`sourcedao_revision` 指向当前锁定合约版本。生成器比较新旧预部署代码及 admin 分配，
只允许系统账户因网络身份改变而重建。当前 SourceDAO artifact JSON 的元数据与原冻结构建不同，
bootstrap config 更新为当前 artifact SHA-256，但 Dao/Dividend runtime bytecode 必须与 v0 完全一致。
实际发布使用的三仓 revision 由最终 release manifest 冻结，不把生成时未提交的工作区误记为已提交版本。

Bitcoin 的 height 935000 签名 UTXO 文件、签名、信任公钥及 publication record 可复用，
因为其身份绑定 BTC 主网。`release-bootstrap.json` 重新绑定 v1 的 network.json SHA-256。
发布流程仍验证签名，并从该输入生成 AssumeUTXO 运行包。

数据路径遵循现有 compatibility 合同：Bitcoin 数据可复用；balance-history 仅在相同
bootstrap 模式、origin/hash 和存储合同下复用；v1 indexer、USDB 链、控制面及私有配置使用独立目录。
不能把旧 indexer checkpoint、运行期 SourceDAO 记录或 accepted-bootstrap 状态复制到 v1。

bootnodes 复用原公开端点配置，但须在节点运营方实际重置后验收。v0 节点不会因公告自动消失，
也不会自动升级；其 network ID 和 genesis 不匹配，不能作为 v1 peer。原端口由 v1 接管时先停止旧服务。

## 复现与检查

在 USDB 仓库根目录执行，使用发布规定的 Go / Rust 工具链：

```bash
cargo build --manifest-path src/btc/Cargo.toml -p usdb-util --bin generate_go_btc_activation_golden
(cd ../go-ethereum && go build -o /tmp/usdb-v1-genesis-hash ./cmd/usdb-genesis-hash)
(cd ../go-ethereum && go build -o /tmp/usdb-v1-geth ./cmd/geth)
python3 docker/scripts/tools/generate_testnet_v1_bundle.py \
  --activation-generator src/btc/target/debug/generate_go_btc_activation_golden \
  --genesis-hash-tool /tmp/usdb-v1-genesis-hash \
  --geth-tool /tmp/usdb-v1-geth --check
python3 -m unittest discover -s docker/scripts/tools -p 'test_testnet_v1_bundle.py'
```

生成器只写新的 `--output-dir`；`--check` 在临时目录重建并逐字节比较整个包。
SourceDAO artifacts 需由兼容锁指定 revision 的正式构建生成；可通过 `--sourcedao-artifacts` 指定目录。
本地现代 Go 构建 Geth 时采用仓库 `go_toolchain.sh` 的 compatibility 入口，发布仍使用固定 Go 工具链。
若设置了 `CARGO_TARGET_DIR`，相应调整 activation-generator 路径。
registry 的权威输入为 `src/btc/usdb-util/activation-registry/usdb-testnet-v1.json`；
Rust 生成包内 catalog 身份和 Go golden，fast CI 同时检查原始 registry、包内 catalog 与 Go artifact 一致。

在不存在的输出目录重建发布包：

```bash
python3 docker/scripts/tools/release_bundle.py prepare \
  --release-id usdb-testnet-v1-r1 \
  --output-dir /tmp/usdb-testnet-v1-release
```

Candidate 和 Publish 都按 tag 选择网络代际；Publish 额外比较 candidate manifest 中的 bundle ID。
不存在的代际、v1 tag 搭配 v0 bundle、旧 registry 或错误 scope 均拒绝，不能回退到 v0。

## 正式发布前剩余验收

1. 评审并提交两仓变更，更新兼容锁，完成最终 revision 的 fast/nightly/weekly 资格。
2. 冻结同名 annotated tags，执行 Candidate；使用发布 Go 工具链再次计算真实 genesis hash。
3. 验收实际候选安装包：首次安装、完整 Core AssumeUTXO/BH/indexer 就绪、重启、follower 加入及与旧网隔离。
4. 新链重新执行 SourceDAO bootstrap；按 MinerPass V2 流程铸造并核验新的有效证，再启用 miner。
5. Publish 后发布重置说明并协调原 Seed/miner/follower 切换。wallet、bootstrap signer 等私有数据不属于可清除的旧链数据。

本批没有发布标签、上传产物、触发远端 CI、运行 SourceDAO 交易或重置现有节点。
