# balance-history 快照信任模型与安装校验备忘

## 1. 背景

随着 `balance-history` 引入：

- `block commit`
- `snapshot_id`
- 历史 `state ref`

快照能力的语义已经不再只是“加速同步”，还开始影响：

- 历史一致性
- 下游 validator 查询
- ETHW / USDB 双链共识校验

因此需要明确：

1. 当前快照安装流程到底校验了什么
2. 当前没有校验什么
3. 如果快照被篡改，系统会怎样表现
4. 后续应该如何增强安装期与运行期的保护

## 2. 当前实现的实际行为

当前 `balance-history` 的快照安装逻辑集中在：

- [snapshot.rs](/home/bucky/work/usdb/src/btc/balance-history/src/index/snapshot.rs)

安装入口使用：

- `SnapshotInstaller::install(SnapshotData)`

其中 `SnapshotData` 当前只有两个输入：

- `file`
- `hash: Option<String>`

## 2.1 当前已经做的校验

当前安装时已经做的检查主要有：

1. 快照文件存在检查
2. 如果传入 `hash`：
   - 计算整个快照文件的 SHA256
   - 与输入 hash 对比
3. 从快照 DB 读取 `SnapshotMeta`
4. 安装过程中检查导入数量是否与 `SnapshotMeta` 中记录一致
   - `balance_history_count`
   - `utxo_count`
   - `block_commit_count`

也就是说，当前有：

- **文件级完整性检查**
- **记录数级别的一致性检查**

## 2.2 当前没有做的校验

当前安装流程默认没有自动做以下验证：

1. 不会重算安装后 stable state 的 `snapshot_id`
2. 不会把安装后的结果和某个可信 manifest 中的 `snapshot_id` / `state ref` 对比
3. 不会重算 `block_commit` 链的正确性
4. 不会在安装完成后自动运行 [verify.rs](/home/bucky/work/usdb/src/btc/balance-history/src/index/verify.rs) 中已有的校验器
5. 如果 `hash` 没提供：
   - 当前就是直接跳过内容校验

因此当前行为更接近：

- **默认信任快照**
- `hash` 只是可选保护

而不是：

- 默认安装后必做语义级自校验

## 3. 当前系统能发现什么篡改

## 3.1 能发现的情况

### 情况 A：快照文件被改动，但外部 hash 没改

当前可以发现。

因为：

- installer 会重新计算快照文件 hash
- 与外部提供的 hash 不一致就直接失败

这属于：

- **文件级篡改检测**

## 3.2 不能直接发现的情况

### 情况 B：快照文件和外部 hash 一起被改

当前安装流程发现不了。

原因：

- 当前没有独立信任根
- 安装流程只会看到：
  - 文件本身
  - 调用方提供的 hash

如果这两者一起被替换成一个伪造但自洽的版本，安装流程会认为它是合法的。

### 情况 C：伪造一个内部自洽、可通过自校验的错误快照

这同样是当前安装阶段无法直接阻止的。

因为攻击者完全可以构造一个：

- 记录数正确
- 元数据自洽
- block commit 自洽
- 并且文件 hash 也匹配其伪造版本

的错误快照。

这说明一个根本事实：

- **只依赖快照文件自身的信息，无法建立强信任**

## 4. `snapshot_id` 引入后，当前获得了什么保护

引入 `snapshot_id` 和 `state ref` 后，系统获得的保护主要是：

- **错误状态更容易在运行期暴露**

但这并不等于：

- 安装期已经具备完整防篡改能力

原因是：

- 当前快照安装时会直接把 `block_commit` 一并导入
- 后续 `snapshot_id` / `state ref` 也是基于这些已导入状态构造

所以如果导入的是一个伪造但内部自洽的快照，那么：

- 节点仍然可以先启动起来
- 并继续基于错误历史生成自己的 `snapshot_id`

因此 `snapshot_id` 当前更像：

- **后续运行期的一致性暴露机制**

而不是：

- **安装阶段的完整真实性校验机制**

## 5. 错误快照当前的影响边界

这个问题需要特别说明。

错误快照的影响通常不会直接扩散为“全网一起坏掉”，但会导致当前节点进入错误状态。

可能表现为：

1. 节点对下游返回错误的历史查询结果
2. 节点生成与 honest world 不一致的历史 `state ref`
3. 节点给 ETHW miner / validator 提供错误的历史输入
4. 节点无法正确出块
5. 节点无法正确验证某些块或某些历史状态

更准确地说：

- 影响通常主要局限在：
  - **这个节点自己**
  - **以及依赖它的本地下游服务**

而不是：

- 直接让其他 honest 节点被污染

原因是：

- honest 节点不会接受它错误历史派生出来的共识结果

所以这更像：

- **单节点被污染**
- 而不是“全网立即被污染”

## 6. 当前已有但未接入安装流程的能力

`balance-history` 本身其实已经有校验器：

- [verify.rs](/home/bucky/work/usdb/src/btc/balance-history/src/index/verify.rs)

它可以通过 electrs / BTC 数据源做：

- latest 验证
- 指定高度验证
- 地址级验证

这说明当前系统并不是完全没有语义校验能力，而是：

- **安装流程默认没有把这层校验串进去**

所以当前更准确的状态是：

- 系统具备“可验证能力”
- 但当前安装流程仍然是“默认信任 + 可选 hash”

## 7. 风险分层

建议把这块的风险分成三层理解。

## 7.1 文件级风险

快照文件传输过程中损坏、被部分篡改。

当前可通过：

- 文件 hash

来处理。

## 7.2 快照来源风险

快照文件和其 hash / manifest 一起被替换。

当前无内建防护。

必须依赖：

- 可信分发渠道
- 签名 manifest
- 外部可信根

## 7.3 语义级风险

安装的快照在文件上完整，但其内容代表的是错误历史。

当前安装阶段无自动防护。

只能在后续运行中通过：

- 与 honest world 的 `state ref` 不一致
- 历史查询不一致
- validator 回放失败

等方式间接暴露。

## 8. 建议的改造方向

建议后续把快照信任模型增强分成三层推进。

## 8.1 第一层：基础安装约束

目标：

- 提高“默认安装安全性”

建议：

1. 生产模式下 `hash` 改成必填
2. 没有 `hash` 时只允许 dev / debug 安装
3. 安装日志中明确打印：
   - snapshot hash
   - target height
   - source manifest 信息

## 8.2 第二层：manifest + expected state ref

目标：

- 增加“安装后立即一致性比对”

第一阶段建议把 manifest 设计成：

- **快照数据库外部的 sidecar 文件**
- 而不是直接放回同一个 sqlite 文件里

推荐形式：

- `snapshot_<height>.db`
- `snapshot_<height>.manifest.json`
- 后续如需增强分发信任，再扩展：
  - `snapshot_<height>.manifest.sig`

这样做的原因是：

1. 信任边界更清楚
   - 快照 DB 是被安装的数据本体
   - manifest 是安装期的外部预期描述
2. 更适合后续 Docker / 对象存储 / 离线分发
3. 便于以后增加签名文件，而不需要修改快照 sqlite 结构
4. 避免把“发布元数据”和“快照数据本体”耦合在一起

建议引入 snapshot manifest，至少包含：

- target block height
- stable block hash
- latest block commit
- expected `snapshot_id`
- 版本信息

安装完成后：

- 直接在本地重算 `snapshot_id`
- 和 manifest 中的 expected value 对比

这样可以提升：

- 安装结果与预期 state ref 的一致性确认

但仍要注意：

- 如果 manifest 本身也被攻击者伪造，这一层仍不足以建立最终信任

## 8.3 第三层：post-install verify

目标：

- 在安装后做真正的语义级验证

建议：

- 提供显式 `post-install verify` 模式
- 安装完成后自动调用 [verify.rs](/home/bucky/work/usdb/src/btc/balance-history/src/index/verify.rs)
- 可分为：
  - 抽样验证
  - 全量验证

这一步虽然更慢，但可以显著提高：

- 对错误快照的发现能力

## 9. 对 Docker / 部署层的含义

这对后续 Docker 部署也有直接影响。

如果后续在 Docker 中使用 `balance-history` 快照：

- 快照恢复不应只是“把文件 mount 进去然后 install”
- 还应配套：
  - hash / manifest
  - 可选 post-install verify
  - readiness gate

也就是说，一个更完整的 Docker 快照流程应是：

1. 恢复快照文件
2. 校验 hash / manifest
3. 安装到 staging
4. 可选做 post-install verify
5. 通过后才进入服务 ready

## 10. 当前结论

当前可以明确记录的结论是：

1. `balance-history` 当前快照安装流程默认仍然是“信任快照内容”
2. 当前唯一自动接入的内容校验是：
   - 可选文件 hash
3. `snapshot_id` 当前更多是运行期一致性暴露机制，而不是安装期完整防篡改机制
4. 即使错误快照被装入，影响通常主要局限在本节点及其下游依赖，而不是立即污染全网
5. 后续若要把快照正式引入 Docker / 节点部署流程，应优先补：
   - manifest + expected state ref
   - post-install verify
   - readiness gate
6. 第一阶段 manifest 推荐采用独立 sidecar 文件：
   - `snapshot_<height>.db`
   - `snapshot_<height>.manifest.json`
