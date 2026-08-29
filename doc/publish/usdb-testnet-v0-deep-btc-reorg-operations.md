# USDB testnet-v0 深 BTC 重组停链与整网重置

Status: implemented for deterministic regtest rehearsal; public testnet execution remains manual.

本文冻结 `usdb-testnet-v0` 遇到 frontier-crossing BTC reorg 时的运维策略。它只适用于当前
`development-resettable` 测试网，不替代未来 deterministic rewind UIP，也不授权 mainnet
节点自动选择 USDB rewind target。

## 1. 冻结结论

testnet-v0 采用：

```text
detect stable-state rollback
  -> persist indexer upstream_reorg_epoch
  -> every chain node writes a local incident latch
  -> stop the old USDB network generation
  -> archive evidence and old chain data
  -> start a new generation from empty chain datadirs
```

本版本明确不执行：

- 自动 `debug_setHead`；
- 继续扩展引用 orphan BTC state 的旧 USDB chain；
- 在相同 chain/network ID 和 genesis 下清 latch 后恢复；
- 把 release revision `rN` 当成 network generation reset；
- 删除事故数据后再决定 safe head。

## 2. 触发条件与门禁

usdb-indexer 只索引 balance-history 已暴露的 stable state。它一旦检测已采用的 stable anchor/commit
发生 drift，会在 pass rollback 与 durable recovery marker 的同一个 SQLite 事务中递增
`upstream_reorg_epoch`。该 epoch 不因 replay、restart 或普通本地 rollback 清零。

每个 chain datadir 第一次启动时记录当前 epoch：

```text
<USDB_CHAIN_DATA_DIR>/recovery/deep-btc-reorg/baseline.json
```

运行中或 restart 前观察到 epoch 增长/回退时写入：

```text
<USDB_CHAIN_DATA_DIR>/recovery/deep-btc-reorg/halted.json
```

随后 supervisor 终止 geth 并保持容器 halted。indexer 可以继续在 replacement BTC branch 上完成
rollback/replay，供事故诊断和新 generation 使用；旧 chain node 不得恢复出块或导入。

短暂 indexer outage 不创建 reorg latch，但 guard 连续探测失败会停止 geth。服务恢复后，只有不存在
`halted.json` 且 epoch 仍等于基线的节点才可重新启动。

## 3. 事故操作

1. 停止所有节点的 miner、chain runtime 和 control-plane 写入口。
2. 保存三节点的 `halted.json`、最后 USDB block number/hash、genesis hash 和 peer 信息。
3. 保存 indexer readiness、reorg epoch、rollback target、旧/新 BTC stable hash 与 common ancestor。
4. 将旧 chain volume 和日志转为只读归档；不要执行 `down -v` 或直接删除。
5. 等待 Bitcoin Core、balance-history 和 usdb-indexer 在 replacement branch 上重新达到一致状态。
6. 对 replacement state 执行 state-ref、profile、candidate 和 snapshot 复核。
7. 准备新的 network generation bundle，至少更换 release `vN`、chain ID、network ID 和 genesis。
8. 三个 USDB chain 节点全部使用空 datadir；不得复制旧 chaindata、baseline 或 halted latch。
9. 先启动非矿工节点验证新 genesis 和 indexer，再启动一个 miner。
10. 重新执行 SourceDAO bootstrap，完成 restart 和 fresh joiner 验收后才恢复对外服务。

Bitcoin Core 数据和已经在 replacement branch 收敛的 BTC-side DB 不因 USDB network reset 自动删除。
若其一致性复核失败，则保持 halted 并从可信 snapshot/checkpoint 或 full sync 重建。

## 4. 分层演练断言

源码级自动演练使用当前 registry 的 `stable_lag=10`，构造 `lag+1=11` 深度 replacement，且不得
连接或修改宿主机 mainnet datadir。当前自动化断言：

1. depth 11 替换 stable block，indexer epoch 恰好递增一次。
2. 三份旧 generation guard state 全部生成 latch，重复 check 仍拒绝启动。
3. runtime supervisor 终止 chain process，使用旧 datadir restart 仍保持 halted。
4. indexer restart/replay 不重复递增同一次事件。
5. 新 generation 使用空 chain datadir 建立当前 epoch 基线并可启动 chain process。

候选 release 的三节点验收还必须额外断言：

1. BTC 双节点竞争分支在 depth 9/10 不改变 stable identity，也不递增 indexer epoch。
2. 三个真实 geth 节点全部停止，观察窗口内 USDB head 不再增长。
3. 新 miner 恢复出块，两个 validator 和 late joiner 收敛到同一 head/state root。
4. 旧 network ID/genesis/datadir 不能加入新 generation。
5. 报告记录旧/新 chain identity、BTC fork point、epoch、state-ref 和各进程 revision。

本地源码演练不依赖 release manifest。最终上线前还需使用 digest-pinned 三类镜像和候选 manifest
重跑同一停链/重置流程；真实 BTC mainnet 只验证监控和人工响应，不主动制造 reorg。

## 5. 长期边界

`upstream_reorg_epoch` 和本地 latch 是 testnet-v0 的保守运维门禁，不是新的 header 共识字段。
正式 deterministic rewind 仍需冻结 safe head、recovery artifact、签名授权、废弃分支拒绝和 fresh
joiner replay 规则，并通过新的 Standards Track UIP/activation 落地。
