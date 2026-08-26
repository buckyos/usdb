# USDB testnet-v0 参数冻结清单

本文定义第一个可重置三节点测试网 `usdb-testnet-v0` 的参数基线、权威文件和上线前待办。
它是运维 review 入口，不替代 UIP、genesis、SourceDAO bootstrap config 或 release manifest。

`testnet-v0` 当前状态为 `development-resettable`。第一次成功出块后，任何会改变 genesis、链身份或
accepted-bootstrap 身份的修改都必须发布新的 bundle，并使用新的 chain/network ID 从空数据目录重启。

## 本轮结论

以下两个值继续采用当前 bundle，不能回退到早期开发值：

- Chain ID 和 devp2p network ID 使用 `202608250`。`20260323` 是内置开发链和本地 regtest
  基线，测试网复用它会增加误连、错误数据目录复用和交易 replay domain 混淆风险。
- BTC index origin 使用 `963800`。它必须与 exact-height full-UTXO snapshot 高度一致；不能把
  origin 设置成 `900000` 后直接安装 `963800` snapshot。

首次测试网不做在线修补。block-0 policy、genesis、SourceDAO accepted-bootstrap 参数如需修改，
优先重置网络。registry revision 和 policy version 的未来升级仍可在新的 USDB 高度加入 activation，
但不作为 testnet-v0 首次启动的补救手段。

## 网络与 BTC 输入

| 参数 | testnet-v0 | 身份类别 | 权威来源 |
| --- | --- | --- | --- |
| Release / bundle ID | `usdb-testnet-v0` | release | `network.json` |
| 状态 | `development-resettable` | release | `network.json` |
| Chain ID | `202608250` | genesis / replay domain | chain bootstrap config |
| devp2p network ID | `202608250` | P2P network | `network.json` / `network.env` |
| 默认 P2P 端口 | `31303/TCP+UDP` | 运维约定 | `network.env` |
| BTC source | `btc-mainnet` | consensus input domain | chain bootstrap config |
| Bitcoin Core | `28.1`、非裁剪、`txindex=1` | 发布兼容基线 | release record |
| BTC index origin | `963800` | BTC 索引身份 | chain bootstrap config |
| BTC registry revision | mainnet revision 1 | consensus | activation registry artifact |
| BTC registry ID | `cc47923f4cdff1875f89771d08e1b89fa22295c92bb816073c3271dc53c54c1c` | consensus | activation registry artifact |
| BTC stable lag | `5` blocks | BTC state view identity | activation registry artifact |
| BTC anchor max age | `6650` USDB blocks | consensus | chain bootstrap config |

Bitcoin Core 的 image digest、数据目录、RPC userpass 和 P2P bind 是节点本地配置，不进入 genesis。
release profile 固定使用独立 Compose project 和 `http://btc-node:8332` 私有 endpoint；RPC 不映射
宿主机端口，每台机器使用独立 rpcauth。操作流程见
[Bitcoin Core Release Image 与同步操作](./bitcoin-core-release-and-sync-operations.md)。

## Genesis 与共识参数

| 参数 | testnet-v0 |
| --- | --- |
| activation 起点 | USDB block `0` |
| payload / BTC anchor / difficulty | `1 / 1 / 1` |
| reward / emission / fee split | `1 / 1 / 1` |
| collaboration efficiency / price | `1 / 1` |
| quote / aux pool | `0 / 0`，完全禁用 |
| genesis / minimum difficulty | `0x180000 / 0x100000`，上线前仍需目标硬件校准 |
| fee gate | block `8192` |
| genesis gas limit | `30000000` |
| genesis extra data | ASCII `USDB genesis` |
| UIP-0007 selector | v1 exact `111` bytes；USDB outer limit `160` bytes |
| uncle reward | reward v1 下禁用 |
| fixed price v1 | `100000000000000000000000` atoms/BTC |
| fee split v1 | miner `6000` bps，DAO `4000` bps |
| K range/window | `8001..20000` bps，`50400` blocks |
| level difficulty factor | 每级 `100` bps，最低 `5000` bps |
| genesis SHA-256 | `c40bc1f7e907701d8fe61c25d0386bce86db6768ca1f583614781a732c45ea3e` |
| genesis block hash | `0xac89ddec1c12efa4173c67e70772861def1e121c387b612e702805161970e560` |

所有 EVM fork block、genesis nonce/timestamp、alloc、system storage 和代码字节不再逐项复制到本文；
它们统一由冻结的 genesis 文件及其 SHA-256/block hash 约束。上述公式常量由 policy version 和
冻结的 `go-ethereum` revision 共同确定，不能只更新文档或 JSON 数字。

## System State 与 SourceDAO

### Genesis 预置

| 项目 | testnet-v0 |
| --- | --- |
| USDB reserved system account | `0x0000000000000000000000000000000000001000` |
| system schema / nonce | `1 / 1` |
| Dao address | `0x0000000000000000000000000000000000001001` |
| Dao runtime code hash | `0xc6549189a694031f7d94e6b1b8b11b2bc43d1753cdaa7ac885a97c5e6d462593` |
| Dividend address | `0x0000000000000000000000000000000000001002` |
| Dividend runtime code hash | `0x24859958e81d46d4a3df2de8b346600d05ed8a3e683ad4c2117fe9e73804144e` |
| bootstrap admin | `0xabCd35AfbB4561213fEAfF01B5F91e18F8Df7c37` |
| bootstrap admin genesis balance | `10000000000000000000` wei |

合约 artifact SHA-256、runtime bytecode 和 genesis alloc 由
`artifacts/usdb-chain-bootstrap-config.json` 与冻结 genesis 共同约束。

### Accepted bootstrap

SourceDAO bootstrap 的完整权威输入是
`artifacts/sourcedao-bootstrap-config.json`，当前 SHA-256 为
`5339986ce8cbee70c381dab388345571a28f6e8988958bc98b120c36b572d37e`。上线前必须人工复核：

- bootstrap admin 地址与实际 signer 一致；private key 不进入 Git、镜像或 bundle；
- `cycleMinLength=60`、transaction gas limit `8000000`；
- BDDT/BDT 名称、symbol、总量、10 组初始地址和数量；
- 3 个初始委员会成员、proposal cursor `7`、dev ratio `400 -> 120`；
- project counter `4`、investment count `4`、token lockup 项目和版本；
- 所有模块地址、owner/main address、Dividend marker 和代码哈希通过只读 bootstrap validator。

在 block `8192` 之前必须完成 `Dividend.finalizeBootstrap()`。accepted-bootstrap record 需要记录交易
哈希、完成区块、区块哈希、state root、validator report SHA-256 和确认深度；仅有“脚本执行成功”
不能作为网络启动成功的证据。

## 上线前必须补齐

以下内容尚未全部冻结，不应伪装成 genesis 参数：

| 项目 | 上线前输出 | 是否导致重置 |
| --- | --- | --- |
| PoW 校准 | 目标硬件报告、最终 genesis/minimum difficulty | 若改 genesis 则是 |
| 三仓 revision | `go-ethereum`、`usdb`、`SourceDAO` commit | 未启动前重新生成 bundle |
| 发布镜像 | services/chain/Bitcoin Core 三个 OCI digest、构建日志/SBOM | 仅共识兼容替换可不重置 |
| Snapshot | height/hash、snapshot ID、文件 SHA、manifest/signature、signer key ID | snapshot 本身否；origin 改变则是 |
| BTC origin anchor | BTC block `963800` 的 canonical block hash | origin/hash 身份变化则是 |
| Bootnodes | enode、外部 IP、端口和节点所有者 | 否 |
| 初始 miner | active standard pass ID、owner、`usdb_main`、首次 profile/state-ref | 否，但未满足则不能开挖 |
| SourceDAO bootstrap | signer 可用性、执行/复检报告、accepted checkpoint | 参数变化则是 |
| Release manifest | artifact hashes、image digests、revision、测试报告 | 否；身份 artifact 变化则是 |

当前 `usdb-genesis.manifest.json` 中记录的是生成 genesis 时的旧 Go revision。由于随后还有运行时改动，
正式 testnet-v0 release manifest 必须重新记录最终三仓 revision；不能把 genesis manifest 当成完整的
跨仓 release manifest。

## 参数所有权

当前设计不是单一巨型配置文件，而是一个集中 bundle 下的分层权威模型：

| 文件 | 负责内容 |
| --- | --- |
| `docker/networks/testnet-v0/network.json` | bundle 身份、状态、公共网络参数和 artifact hash 索引 |
| `network.env` | Compose 消费的公共、非秘密运行参数 |
| `artifacts/usdb-chain-bootstrap-config.json` | genesis 可配置输入和 block-0 activation |
| `artifacts/usdb-genesis.json` | 最终不可变 genesis/chain config/alloc |
| `artifacts/usdb-genesis.manifest.json` | genesis 文件与生成输入 hash |
| `artifacts/sourcedao-bootstrap-config.json` | accepted SourceDAO 初始化参数 |
| `artifacts/bootstrap-manifest.json` | control-plane 启动门禁输入 |
| `trust/*.trusted-keys.json` | snapshot public trust catalog；不含 private key |
| 未提交的 `node.env` | 三个 image digest、Bitcoin 数据/RPC、节点角色、bootnodes、miner、资源限制 |

`validate_network_bundle.py` 对重复字段执行 fail-closed 解析，并交叉校验 chain ID、BTC source、origin、
registry、genesis、SourceDAO 地址和 artifact hashes。脚本中的 `EXPECTED_*` 是 testnet-v0 的 validator
pin，不是另一套可独立修改的配置源。

这种分层比把所有参数塞入一个文件更合适：genesis、SourceDAO、snapshot 和节点秘密有不同生命周期。
集中性由 `network.json` 的 artifact hash 索引、release manifest 和 validator 提供，而不是通过复制所有
字段实现。本文只用于 review 和运维导航，机器执行时仍以 bundle artifact 为准。

## 修改规则

- 修改 chain/network ID、genesis、BTC origin、block-0 activation、system address/code、genesis alloc、
  bootstrap admin 或 accepted-bootstrap 参数：发布新 bundle，用空数据目录重置。
- 新 registry revision 或 policy version：只允许作为未来高度 activation，并先完成跨激活 reorg/restart
  测试；testnet-v0 首次启动阶段仍优先重置。
- 修改 bootnodes、RPC endpoint、凭据、cache、cgroup、日志、miner threads：属于节点运行参数，
  不改变链身份。
- 替换 binary/image：必须证明与冻结 genesis、activation 和 payload codec 兼容，并更新 release manifest。

## 启动签字清单

1. `validate_network_bundle.py` 和 genesis roundtrip 通过。
2. PoW 校准值已决定；如有变化，重新生成 genesis 和所有 hash。
3. snapshot 在独立空目录安装/继续同步验证通过。
4. 三仓 revision、镜像 digest、trusted catalog 和所有 artifact hash 已写入 release manifest。
5. SourceDAO 配置、管理员、token 分配和委员会完成双人 review。
6. 初始 miner pass 在选定 BTC state-ref 下为 active standard，recipient 正确。
7. 三节点先以非矿工角色验证 genesis、peer 和 BTC-side state，再开启一个 miner。
8. SourceDAO 在 fee gate 前完成 bootstrap，并冻结 accepted checkpoint。
