# USDB Explorer 网络与 RPC 契约 v1

USDB 节点提供 RPC 与权威网络身份；独立的 `buckyos/usdb-explorer` 提供浏览器、公共接入和部署。
浏览器日常构建与测试不导入 USDB 源码，也不参与 USDB/go-ethereum/SourceDAO 的 release lock。

需要更新网络资料时，在 USDB checkout 从固定 commit 导出：

```bash
python3 tools/export_explorer_contract.py \
  --revision <40-character-USDB-commit> --output-dir /tmp/explorer-network-export
```

工具只读取指定 commit 的 `docker/networks/testnet-v1`，按 canonical bundle 校验，生成：

- `usdb-testnet-v1.json`：chain ID、network ID、genesis、BTC registry 和 rules scope 等网络身份。
- `usdb-testnet-v1.contract.json`：schema、来源仓库/commit、catalog SHA-256、RPC profile。

审阅导出内容后将两个文件一同更新到 Explorer 的 `explorer/networks/` 并提交该工程。
普通打包使用已提交的资料；不在 CI 自动跟随 USDB master，也不根据未知 RPC 自动替换网络身份。
本次 v1 同步固定 node1 的 r25 源码 `8d45b31f1bf43565579787069238d17c51e19aa1`：
chain ID `202610030`，genesis `0xb0b6ebc9a6c2e051855c2d61dff9a51635ea1db09c898a3c0126e27b9e779314`。
v0 到 v1 是网络重置；Explorer 从空数据库重新索引，不迁移或备份废弃的 v0 数据。
RPC profile 的 `v1` 是接口版本，与网络 bundle 的 `v1` 分别管理。

`usdb-explorer-rpc:v1` 要求历史状态查询、区块/交易/回执/日志、`callTracer` tracing 和
`eth_sendRawTransaction` 广播能力。读取、tracing、广播可以使用不同上游，但必须核对相同
chain ID、genesis 和 canonical checkpoint。浏览器内部所需 debug tracing 不等于公共 gateway
允许外部调用 debug 方法。

USDB 的 extraData 必须原样保留。尚未验收的 Ethereum 奖励、供应量和 fee distribution 假设
继续标记 `not_qualified`。网络字段或能力要求变化应有对应契约版本与兼容性测试；纯浏览器
UI/部署更新不改变 USDB 网络身份。

本机 RPC 样本检查继续由 node-kit 的 `check_explorer_rpc.py` 提供；节点资源预算、挖矿和运维
测试留在节点工程。真实 archive/reorg/wallet 验收按固定节点镜像版本在独立工程记录，
契约文件和 mock 测试不能代替这些验收。
