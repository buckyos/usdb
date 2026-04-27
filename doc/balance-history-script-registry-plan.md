# Balance-History Script Registry Plan

## 1. 背景

当前 `balance-history` 和 `usdb-indexer` 以 `script_hash` 作为 BTC 地址相关
查询和状态索引的核心 key。这个选择适合底层索引和协议一致性：

- BTC 链上真实锁定对象是 `scriptPubKey`，address 是部分标准脚本的网络相关展示编码。
- Electrum/electrs 体系也普遍以 script hash 做余额和 history 索引。
- `script_hash` 作为固定长度 key 更适合 DB 索引、快照排序和协议查询。

但这个模式存在一个产品和运维层面的缺口：从 BTC address 正向查询时可以得到
`script_hash`，但从 overview、排行榜、矿工证 owner 等查询结果里只有
`script_hash` 时，无法直接反向还原出用户可读的 BTC address。

`script_hash -> address` 不是数学反解。只有索引器曾经看到过对应的
`scriptPubKey`，才能根据当前 BTC 网络尝试派生标准 address。因此需要增加一个
辅助 registry，而不是改变现有主索引 key。

## 2. 方案结论

采用：

```text
辅助 cache，不参与共识 commit，但 snapshot 可导入导出。
```

核心原则：

- 内部索引和协议状态继续使用 `script_hash`。
- 链上可验证原始材料使用 `script_pubkey`。
- 用户展示和前端输入优先使用 BTC address。
- `script_registry` 作为辅助解析索引，不影响 USDB 共识状态。
- `script_registry` 是 `balance-history` 主索引流程的原子副产物，不是独立索引。
- snapshot 需要携带 `script_registry`，保证从 snapshot 恢复后的节点仍然具备
  address 展示能力。

## 3. 目标

第一阶段目标：

- 在 `balance-history` indexing 过程中收集所有已见过的 output script。
- 在主 block batch flush 时和 UTXO、balance history、block commit 一起原子写入。
- 提供 `script_hash -> scriptPubKey -> address?` 的批量解析能力。
- 在 snapshot export/import 中包含 `script_registry`。
- 让 `usdb-indexer` 和 web browser 的 overview、owner、排行榜等页面可以优先展示
  BTC address。

非目标：

- 不改变现有 balance/history/UTXO 的主索引 key。
- 不把 address 文本纳入共识 commit。
- 不要求所有 script 都能解析成 address。
- 不维护 per-script `first_seen_height` / `last_seen_height`。
- 不维护独立的 `script_registry_indexed_height`。
- 不在第一阶段实现严格 reorg 删除语义。

## 4. 数据模型

建议在 `balance-history` 主 RocksDB 中新增 `script_registry` column family。

```text
script_hash    BLOB PRIMARY KEY
script_pubkey  BLOB NOT NULL
```

字段说明：

| 字段 | 说明 |
| --- | --- |
| `script_hash` | 当前系统使用的 canonical script hash。 |
| `script_pubkey` | BTC tx output 中的原始 locking script。 |

不建议持久化 `address` 字符串作为主字段：

- address 是网络相关编码，mainnet/testnet/regtest 输出不同。
- 对非标准 script 可能无法派生 address。
- `script_pubkey` 才是链上事实，address 可以在 RPC 层动态派生。

后续如有性能需要，可以增加可清理的 address display cache，但不作为协议或 snapshot
的权威字段。

也不建议在第一阶段维护 per-script height：

- `first_seen_height` 需要保留旧值，写入时会引入 read-modify-write。
- `last_seen_height` 会让热 script 频繁重写 registry。
- 并行批处理内需要额外聚合 min/max height。
- 在第一阶段 append-like auxiliary cache 语义下，reorg 后这些 height 也不是严格
  canonical 值。
- address 解析只需要 `script_hash + script_pubkey + network`。

`script_registry` 也不需要独立 indexed height。它跟随 `balance-history` 主索引
一起写入，状态边界由现有 `META_KEY_BTC_BLOCK_HEIGHT`、block commit 和 readiness
表达。

### 4.1 与现有 AddressIndexer 的关系

当前代码里已有独立的 `AddressIndexer` 工具项：

- `index/address.rs`
- `db/address.rs`
- `balance-history index-address`

这个工具可以由使用者主动运行，也可以拥有自己的状态和进度管理。它更适合作为
离线 rebuild / verify / debug 工具。

本方案中的 `script_registry` 不复用该独立状态模型。正式 registry 应该是
`balance-history` 主索引的副产物：

- 不需要单独启动。
- 不需要单独进度。
- 不需要独立 indexed height。
- snapshot 跟随主 snapshot 导出和导入。
- crash consistency 跟随主 block batch `WriteBatch`。

## 5. 索引写入流程

在 `balance-history` index block / tx output 时同步收集：

1. 读取每个 tx output 的 `script_pubkey`。
2. 计算现有规则下的 `script_hash`。
3. 将 `(script_hash, script_pubkey)` 加入当前 block batch 的 registry collection。
4. flush 前按 `script_hash` 批内去重。
5. 在 `BalanceHistoryDB::update_block_state_with_undo_async` 的同一个 RocksDB
   `WriteBatch` 中写入 registry。

写入语义建议：

```text
batch.put_cf(SCRIPT_REGISTRY_CF, script_hash, script_pubkey)
```

重复写入相同 key 是可接受的。为减少写放大，建议批内先去重，但第一阶段不需要
为了判断 key 是否已存在而额外读 DB。

如果在同一批内发现相同 `script_hash` 对应不同 `script_pubkey`，应记录 error 并
中止。这属于编码不变量错误或理论 hash 碰撞，不应静默覆盖。

## 6. Reorg 策略

第一阶段推荐采用 append-like auxiliary cache：

- `script_registry` 只保证“本节点曾经从索引输入看到过这个 script”。
- 不参与共识 commit。
- reorg 时不强制删除只在孤块中出现过的 script。
- 对 UI 展示来说，额外存在的 script 映射通常无害，因为业务查询仍然由 canonical
  balance/history/pass 状态决定。

这个策略的优点：

- 实现简单。
- 不需要为 registry 增加复杂 undo log。
- 不影响现有 reorg correctness。
- 不会阻塞 address 展示能力的第一阶段落地。

后续如果需要更严格语义，可以增加：

- `script_registry_history` 或 per-height reference count。
- reorg rollback 时回滚只在孤块中出现的 script。
- snapshot manifest 标记 registry policy。

但这不建议作为第一阶段需求。

## 7. Snapshot 设计

snapshot 需要支持 `script_registry` 的导出和导入，否则 snapshot 恢复节点会出现：

- balance/history 查询可用。
- usdb-indexer 状态可用。
- overview 里 owner 只能显示 script hash，无法显示 address。

建议新增独立 snapshot section：

```text
balance_entries
utxo_entries
block_commits
script_registry
```

导出规则：

- 按 `script_hash` 字典序分页导出。
- 每条记录包含 `script_hash`, `script_pubkey`。
- manifest 增加 registry 元数据。

manifest 建议字段：

```json
{
  "script_registry": {
    "included": true,
    "count": 123456,
    "root": "hex...",
    "hash_algo": "sha256",
    "policy": "auxiliary_seen_scripts_v1"
  }
}
```

`root` 用途：

- 用于 snapshot 文件完整性校验。
- 用于导入后自检。
- 不纳入现有 balance-history block commit 或 USDB consensus commit。

导入规则：

- 先导入主 snapshot 数据。
- 再批量导入 `script_registry`。
- 导入完成后校验 count/root。
- 如果 registry section 缺失，主 snapshot 仍可安装，但 readiness/UI 应标记
  `script_registry_available=false` 或 `partial`。

## 8. RPC 设计

### 8.1 balance-history RPC

新增批量解析 RPC：

```text
resolve_script_hashes(script_hashes[], include_script_pubkey?)
```

响应示例：

```json
{
  "network": "regtest",
  "items": [
    {
      "script_hash": "...",
      "script_pubkey": "5120...",
      "address": "bcrt1p...",
      "address_type": "p2tr",
      "standard": true
    },
    {
      "script_hash": "...",
      "script_pubkey": "6a...",
      "address": null,
      "address_type": "non_standard",
      "standard": false
    }
  ],
  "missing": ["..."]
}
```

参数保护：

- 单次最多解析数量，例如 `max_items=500` 或 `1000`。
- `script_hash` 必须格式合法。
- 默认不返回 `script_pubkey`，除非 `include_script_pubkey=true`。

### 8.2 usdb-indexer RPC

`usdb-indexer` 可以先不复制 registry 数据，而是通过 balance-history RPC 批量补全。

对外响应建议逐步从：

```json
{
  "owner": "script_hash..."
}
```

扩展为：

```json
{
  "owner": "script_hash...",
  "owner_script_hash": "script_hash...",
  "owner_address": "bcrt1p...",
  "owner_address_type": "p2tr"
}
```

兼容策略：

- 保留旧字段 `owner`，含义仍然是 script hash。
- 新增明确字段 `owner_script_hash` 和 `owner_address`。
- 前端优先展示 `owner_address`，技术详情里保留 `owner_script_hash`。

## 9. Web 展示策略

浏览器和 console 页面展示规则：

1. 如果 `owner_address` 存在，主展示使用 address。
2. 鼠标悬停或详情区展示完整 `script_hash`。
3. 如果 address 不可用，展示 `script_hash` 并标注 `script hash only`。
4. 列表页批量解析当前页所需 owner，不做全量扫描。

适用页面：

- balance-history-browser 地址详情和 overview。
- usdb-indexer-browser 最近铸造矿工证。
- usdb-indexer-browser 能量 Top。
- usdb-console 我的页面 / 钱包状态 / 矿工证状态。

## 10. 数据规模评估

`script_registry` 不应被假设为小表。

单条记录粗略组成：

| 项 | 典型大小 |
| --- | ---: |
| `script_hash` | 32 bytes |
| 常见 `script_pubkey` | 22-34 bytes，Taproot 34 bytes |
| DB/index overhead | 可能大于原始字段本身 |

在 mainnet 上，如果唯一 script 数达到千万到上亿级，实际占用可能达到数 GB 到数十 GB。

结论：

- 相比 `.bitcoin` blocks、ord index、balance-history 主 history，registry 小很多。
- 但它不是可以全量内存加载的小表。
- 从第一版开始就必须分页、批量、按 key 排序导出。

## 11. 开发阶段

### Phase 1: balance-history 本地 registry

- 新增 `SCRIPT_REGISTRY_CF`。
- 在 `index/block.rs` 的 batch data 中收集 `(script_hash, script_pubkey)`。
- 在 block batch flush 时把 registry 和 UTXO、balance history、block commit 放进同一个
  RocksDB `WriteBatch`。
- 新增 storage 查询接口：
  - `resolve_script_hashes`
  - `get_script_registry_entries`
  - `get_script_registry_count`
- 增加单元测试：
  - 标准 P2TR/P2WPKH/P2SH script 可解析 address。
  - non-standard script 返回 `address=null`。
  - 重复 script 批内去重。
  - 同一批内 script hash 对应不同 script pubkey 时返回错误。

### Phase 2: snapshot export/import

- snapshot manifest 增加 `script_registry` section。
- 导出时按 `script_hash` 分页写入。
- 导入时批量恢复。
- 增加 corrupt/missing/retry 测试。
- readiness 增加 registry 可用性状态。

### Phase 3: RPC 暴露

- 增加 `resolve_script_hashes` RPC。
- 参数保护和错误码补齐。
- 更新 `balance-history-rpc.md`。
- 控制台 control-plane proxy 白名单增加该方法。

### Phase 4: usdb-indexer 集成

- indexer 对 owner/pass/energy overview 批量补全 address。
- RPC 响应新增 `owner_script_hash`, `owner_address`, `owner_address_type`。
- 保留旧 `owner` 字段兼容现有前端。

### Phase 5: Web UX

- 列表主字段显示 address。
- 详情区保留 script hash、scriptPubKey、address type。
- 对不可解析地址的 script 显示 `script hash only`。
- 对当前页 owner 批量解析，避免逐行 RPC。

## 12. 风险和注意事项

| 风险 | 处理建议 |
| --- | --- |
| Registry 数据量被低估 | 禁止全量加载，所有导出/导入/RPC 都分页。 |
| address 被误当成共识字段 | address 仅展示，commit 和协议状态仍使用 script hash/scriptPubKey。 |
| snapshot 兼容性 | manifest 标记 `script_registry.included`，旧 snapshot 可安装但显示 partial。 |
| reorg 严格性争议 | 第一阶段声明为 auxiliary seen-script cache，不参与 canonical state。 |
| 独立索引状态造成不一致 | registry 跟随主 balance index 原子写入，不保存独立 indexed height。 |
| 非标准 script | `address=null`，前端显示 script hash only。 |
| 网络不匹配 | address 派生必须使用当前 BTC network。 |

## 13. 推荐实施顺序

推荐从 `balance-history` 开始，而不是先改 UI：

1. 先完成 registry 写入和 storage 查询。
2. 再完成 snapshot export/import。
3. 然后暴露 `resolve_script_hashes` RPC。
4. 最后让 `usdb-indexer` 和 web 页面消费该解析能力。

这样可以保证恢复节点、独立部署节点和 console 托管模式都有一致的 address 展示能力。
