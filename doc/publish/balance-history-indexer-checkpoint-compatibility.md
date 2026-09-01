# Balance-history Snapshot 与 Indexer Checkpoint 兼容规则

## 1. 目标与边界

本文定义 USDB 节点使用 balance-history snapshot 时，BTC 历史保留下限、USDB index origin 和
usdb-indexer 本地状态之间的启动兼容关系。Snapshot 是可替换的部署 artifact，不进入 chain ID、genesis
或 network generation 身份。

`paired-checkpoint` 是一组不可拆分使用的部署 artifact：已签名的 balance-history snapshot 与绑定它的
已签名 usdb-indexer checkpoint。恢复流程会先离线校验并安装两侧数据，再在服务启动后按历史高度重算
完整 state-ref；任一步不一致都会阻止 USDB chain 启动。

## 2. 当前支持矩阵

设当前 network bundle 冻结的 BTC index origin 为 `O`，balance-history snapshot 高度为 `H`：

| 启动状态 | 条件 | 当前结果 | 原因 |
| --- | --- | --- | --- |
| Fresh indexer + full-sync balance-history | 无 snapshot | 支持 | balance-history 保留从 genesis 开始的完整查询历史 |
| Fresh indexer + snapshot | `H < O` | 支持 | balance-history 可从 `H` 增量同步到 `O`，之后 indexer 从 `O` 重放 |
| Fresh indexer + snapshot | `H = O` | 支持且推荐 | 最大化启动加速，并保留 indexer 从 origin 重放所需的点余额 |
| Fresh indexer + snapshot | `H > O` | 拒绝 | `balance_query_floor=H`，无法重放 `O..H-1` 的经济状态 |
| Paired indexer checkpoint + snapshot | `H >= O` | 支持 | indexer 已包含 `O..H` 状态，恢复后按 `H` 重算完整 state-ref |

`validate_network_bundle.py --require-runtime` 从当前 bundle 读取 `O`，从 signed snapshot manifest 读取
`H`。文件名不承载高度语义，也不使用 testnet-v0 的 `963800` 作为全局常量。

## 3. Paired-checkpoint Artifact 契约

允许 `H > O` 时，必须同时提供一份受信任的 indexer checkpoint manifest，至少绑定：

- manifest schema、network bundle ID、chain ID、BTC network 和 index origin；
- balance-history snapshot 文件 hash、signer、`block_height`、`stable_block_hash`、
  `latest_block_commit`、`snapshot_id` 和 retention floors；
- usdb-indexer checkpoint 文件 hash、signer、checkpoint height、activation registry ID、
  active version set ID、local state commit 和 system state ID；
- 两个 artifact 的创建时间、工具版本和原子 checkpoint operation ID。

兼容校验必须满足：

1. 两个 artifact 的 BTC 高度都等于 `H`；
2. `snapshot_id`、stable block hash 和 balance-history block commit 完全一致；
3. indexer checkpoint 的 local state commit、system state ID 和 active version set ID 可由恢复后的 DB
   在同一 external state 下重算；
4. registry、API、semantics、formula 和 storage schema 版本均被当前 binary 支持；
5. 目标 balance-history 与 usdb-indexer 数据目录为空，或已是同一 operation 的完整数据；安装器先在
   staging 目录验证，再分别原子发布；
6. 首次启动必须重新构造完整 external state，并与 paired manifest 逐字段比较后才进入 ready。

只匹配 `height` 不足以证明兼容。same-height BTC replacement 会保留相同高度但改变 block hash、snapshot ID
和后续 USDB 状态，因此必须按完整 state-ref fail closed。

## 4. 导出、签名与校验

导出前必须先在同一高度 `H` 创建并签名 balance-history snapshot。usdb-indexer 必须运行在 exact `H`，
且 readiness、upstream state-ref 和 snapshot manifest 完全一致。导出命令会请求 indexer 正常停止，取得
与服务相同的进程锁后复制数据，因此命令成功后 indexer 保持停止状态：

```bash
usdb-indexer-checkpoint-tool --json export \
  --indexer-root "${USDB_INDEXER_DATA_HOST_DIR}" \
  --indexer-rpc-url http://127.0.0.1:28020 \
  --height "${H}" \
  --network-bundle-id "${USDB_NETWORK_BUNDLE_ID}" \
  --chain-id "${USDB_CHAIN_ID}" \
  --index-origin-height "${USDB_GENESIS_BLOCK_HEIGHT}" \
  --balance-history-manifest "${BH_SNAPSHOT_MANIFEST}" \
  --trusted-keys "${SNAPSHOT_TRUSTED_KEYS}" \
  --signing-key "${SNAPSHOT_SIGNING_KEY}" \
  --output-root /home/usdb/.usdb/snapshots
```

导出器会离线打开 checkpoint 中的 SQLite/RocksDB，重算 activation active set、local state commit 和
system state ID。独立验收命令会再次验证双方签名、所有文件 hash 和 pair binding：

```bash
usdb-indexer-checkpoint-tool --json verify \
  --checkpoint-manifest "${USDB_INDEXER_CHECKPOINT_MANIFEST}" \
  --balance-history-manifest "${BH_SNAPSHOT_MANIFEST}" \
  --trusted-keys "${SNAPSHOT_TRUSTED_KEYS}"
```

签名私钥只存在于制作机。部署 bundle 只携带 trusted public-key catalog、两个 manifest、两个 detached
signature sidecar 和对应数据文件。

## 5. Staging Install 与故障恢复

离线安装命令必须在两个服务停止时执行：

```bash
usdb-indexer-checkpoint-tool --json install-pair \
  --checkpoint-manifest "${USDB_INDEXER_CHECKPOINT_MANIFEST}" \
  --balance-history-manifest "${BH_SNAPSHOT_MANIFEST}" \
  --trusted-keys "${SNAPSHOT_TRUSTED_KEYS}" \
  --indexer-root "${USDB_INDEXER_DATA_HOST_DIR}" \
  --balance-history-root "${BH_DATA_HOST_DIR}" \
  --network-bundle-id "${USDB_NETWORK_BUNDLE_ID}" \
  --chain-id "${USDB_CHAIN_ID}" \
  --index-origin-height "${USDB_GENESIS_BLOCK_HEIGHT}"
```

安装器同时取得 `usdb-indexer` 与 `balance-history` 进程锁，并在
`usdb-indexer/bootstrap/paired-checkpoint-install.journal.json` 记录 durable stage。indexer 与
balance-history 各自在自己的文件系统内 staging、校验、原子 rename；两次 rename 之间若断电，不启动
任何消费者，重新执行完全相同的命令即可识别已发布的一侧并继续。已有但不属于该 operation 的数据会
fail closed，不会被覆盖。

服务启动后必须执行：

```bash
usdb-indexer-checkpoint-tool --json verify-recovery \
  --checkpoint-manifest "${USDB_INDEXER_CHECKPOINT_MANIFEST}" \
  --trusted-keys "${SNAPSHOT_TRUSTED_KEYS}" \
  --indexer-root "${USDB_INDEXER_DATA_HOST_DIR}" \
  --indexer-rpc-url http://127.0.0.1:28020 \
  --balance-history-rpc-url http://127.0.0.1:28010 \
  --readiness-timeout-secs 300
```

该命令允许服务当前高度已经超过 `H`，但会分别查询历史高度 `H`，重算并逐字段比较 snapshot ID、
activation registry ID、active version set ID、local state commit 和 system state ID。成功后写入
`paired-checkpoint-recovery.done.json`。Compose 的 `paired-checkpoint-recovery` one-shot service 是
USDB chain 的启动依赖，因此未通过重算时不会开始组块或验块。

## 6. 测试矩阵

当前 release 必须保持：

- fresh indexer 接受 `H < O`；
- fresh indexer 接受 `H = O`；
- fresh indexer 拒绝 `H > O`；
- 动态 snapshot 文件名不改变上述判断；
- 完整 state-ref 匹配时接受 `H > O`；
- 高度、block hash、snapshot ID、block commit、local state commit、system state ID、registry 或 schema
  任一不匹配时拒绝；
- same-height replacement、multi-block reorg、安装中断、restart、joiner 和 artifact 篡改测试；
- 从 paired checkpoint 恢复后，与从 origin 完整重放得到的 profile、candidate、breakdown 和 system state
  逐项一致。

## 7. 当前验证边界

当前自动化已覆盖 manifest/signature/file tamper、离线 state-ref 重算、bundle binding、indexer staging、
indexer publish、balance-history staging、balance-history publish 四个中断窗口的幂等恢复，以及服务已继续
同步后的历史 state-ref 重算。发布共享 testnet 前仍需用真实导出的 artifact 完成一次跨进程演练：

1. 三节点 joiner 从 paired artifact 启动；
2. 与从 origin 完整重放得到的 profile、candidate、breakdown 和 system state 交叉比较；
3. 在 same-height replacement 与 multi-block reorg 后验证恢复节点继续得到相同 state-ref。
