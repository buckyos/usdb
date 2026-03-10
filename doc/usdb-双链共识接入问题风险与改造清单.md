# USDB 双链共识接入问题、风险与改造清单

## 1. 背景与目标

当前方案的基本方向是：

- ETHW 链上的部分共识或收益逻辑依赖 BTC 链衍生状态。
- BTC 侧状态由 bitcoind、balance-history、usdb-index 共同提供。
- 为降低 BTC 短重组影响，balance-history 和 usdb-index 均计划落后 BTC tip N 个块运行，N 目前大致在 2 到 5 之间。
- ETHW 节点后续可能基于 usdb-index 提供的矿工等级、pass 状态、能量值等外部状态进行奖励判定或其他共识相关决策。

本文目标不是给出最终实现方案，而是先形成一份可执行的总清单，明确：

- 当前设计里已经识别的问题和风险
- 哪些风险属于工程风险，哪些已经触及共识安全
- 后续建议的改造顺序
- 每一阶段完成后应该达到什么验收标准

## 2. 当前架构简述

当前链路可抽象为：

1. bitcoind 提供 BTC 主链区块与链状态。
2. balance-history 从 bitcoind 建立地址余额历史视图。
3. usdb-index 基于 BTC 数据和 balance-history 结果计算 pass 状态、历史、活跃余额快照、能量等派生状态。
4. ETHW 节点在出块或验块时，消费 usdb-index 提供的结果。

这条链路天然带来一个问题：

- ETHW 共识需要的是跨节点严格一致的确定性输入。
- 现有两个索引服务更接近高可用、可查询、历史可回放的业务索引服务。

两者目标并不完全一致，所以必须在共识化约束上补足若干关键能力。

## 3. 结论先行

### 3.1 可以肯定的结论

- 落后 BTC tip N 个块是有意义的，它能显著降低短重组对服务层的冲击。
- usdb-index 和 balance-history 目前都已经具备一定的历史查询、恢复、自检和索引一致性基础，不是从零开始。
- 现有系统适合作为业务层稳定查询系统的基础。

### 3.2 不能高估的地方

- 落后 N 个块只能降低风险，不能替代区块哈希锚定。
- 按高度查询不能直接等价为共识可判定输入。
- 只要仍然存在同一高度、不同 BTC 链视图、可能得到不同结果的可能，ETHW 节点之间就可能产生共识分歧。
- 只要 RPC 语义里仍然把未同步、无记录、真实为零混在一起，外部调用方就可能把不完整数据误当成真实状态。

## 4. 问题清单

### 4.1 共识级问题

#### P1. 仅按 BTC 高度建模，不足以形成确定性锚点

当前讨论与实现倾向主要围绕 BTC 高度组织查询与状态，但对共识来说，仅有高度不够。

原因：

- 高度 H 对应的 BTC 区块并不天然唯一，重组时同一高度可能对应不同区块哈希。
- 两个节点即使都落后了 5 个块，仍然可能站在不同的 BTC 分叉上。
- 如果 usdb-index 最终对外只说这是高度 H 的状态，ETHW 节点拿到的并不是一个严格可验证的锚点。

直接风险：

- 同一 ETHW 区块在不同节点上可能读到不同外部状态。
- 导致奖励计算不一致、区块合法性判定不一致，最终触发分叉。

#### P2. balance-history 的按高度查询语义不是 exact-height，而是 at-or-before

balance-history 当前对按高度查询的语义更接近：查询高度 H 时，返回该地址在不晚于 H 的最近一条记录。

这对于钱包、浏览器、业务查询通常是合理的，但对共识输入并不严格。

问题在于：

- exact-height 和 at-or-before 是两种不同语义。
- 如果上游没有在高度 H 产生记录，返回的是 H 之前最近一条记录。
- 对共识侧来说，这意味着没有变化和没有该高度记录被合并了。

直接风险：

- 下游误把最近状态当成该高度严格状态。
- 导致能量、余额、活跃态等派生结果与预期快照不一致。

#### P3. 无记录默认返回零值，容易掩盖未同步和数据缺口

从当前接口与代码行为看，某些查询在没有记录时会回退为零值结果，例如：

- block_height = 0
- balance = 0
- delta = 0

这在业务层是宽容设计，但在共识侧是危险设计。

问题在于以下几种状态可能被混淆：

- 服务尚未同步到请求高度。
- 该地址从未出现过记录。
- 该地址在该时刻真实余额为零。
- 索引异常或局部缺失。

直接风险：

- 下游组件把缺数据解释为真实零值。
- 共识在数据不完整时继续前进，且错误不可见。

#### P4. 服务 ready 语义不严格，RPC 可用不等于快照可用

当前 balance-history 的 RPC 服务生命周期与索引循环并不是强绑定的。

这会带来一个典型问题：

- 端口已经起来了。
- get_block_height 或 get_sync_status 也许能返回内容。
- 但并不等于当前 stable snapshot 已经构建完成且可供共识使用。

直接风险：

- usdb-index 在上游半同步状态下开始取数。
- ETHW 节点在下游半同步状态下开始取数。
- 结果是链路上每一层都服务存活，但没有任何一层对快照完整性作出严格承诺。

#### P5. 外部状态缺少统一 snapshot identity

当前设计更多是在传递某个高度的概念，但没有形成统一的快照身份。

共识可消费的快照身份至少应包含：

- BTC block height
- BTC block hash
- 外部状态公式版本
- 外部状态协议版本
- 必要时还应包含上游数据源版本或快照哈希

如果缺少这层统一 snapshot identity，则：

- 上下游之间无法证明自己使用的是同一份外部视图。
- 故障时很难审计。
- 不同版本程序可能在不知情情况下输出不同结果。

#### P6. 版本治理不够系统，后续升级容易引入静默分歧

这里的版本问题不只是数据库 schema version。

至少还存在以下几个独立版本面：

- RocksDB 或 SQLite 的 schema/storage version
- balance-history 查询语义 version
- usdb-index 派生公式 version
- pass 状态机规则 version
- energy 计算公式 version
- ETHW 节点认可的外部状态协议 version

如果这些版本没有被统一纳入：

- 启动校验
- RPC 输出
- 持久化元信息
- 共识侧兼容性检查

那么一次看起来只是正常升级的动作，就可能在不同节点上产出不同结果。

### 4.2 系统级问题

#### S1. lag N 是设计意图，但还没有上升为协议约束

目前已经明确有落后 N 个块的设计思路，但从系统语义看，还需要进一步回答：

- N 是固定配置还是运行时可变。
- N 是 balance-history 和 usdb-index 各自独立决定，还是统一由系统配置决定。
- 下游查询是否只允许访问 stable_height 以内的数据。
- 服务是否保证对外只暴露稳定窗口内的状态。

如果这些没有协议化，最终就会出现：

- 有的节点 N=2，有的节点 N=5。
- 有的节点能查到 tip 附近数据，有的节点只能查稳定高度。
- 外部状态虽然名义上稳定，但节点间实际口径不一致。

#### S2. 跨服务缺少上游锚点透传

usdb-index 依赖 balance-history，而 balance-history 依赖 bitcoind。

但如果 usdb-index 的输出结果没有把自己的依赖锚点完整带出来，那么就会出现：

- usdb-index 说自己同步到高度 H。
- balance-history 也说自己同步到高度 H。
- 但它们所引用的 BTC block hash 未必一致。

因此，单独传 height 没有意义，必须传完整锚点。

#### S3. 审计与排障证据链不足

当前系统已经有一定的一致性检查和恢复机制，但若未来进入共识场景，还需要更强的审计能力。

否则一旦出现：

- 某节点验块失败。
- 某节点奖励值不同。
- 某节点对 pass 状态判定不同。

将很难快速回答：

- 当时使用的是哪个 BTC 区块哈希。
- 上下游服务各自用了哪个稳定高度。
- 使用的公式版本是什么。
- 查询得到的原始输入是什么。

### 4.3 工程级问题

#### E1. balance-history 更偏业务查询友好，不偏共识查询友好

当前 balance-history 的接口设计对业务足够友好，但对共识缺少明确边界，例如：

- at-or-before 语义默认暴露。
- 零值默认回退。
- 缺少 exact-height exact-hash 的强约束查询入口。
- 缺少 HEIGHT_NOT_SYNCED、BLOCK_HASH_MISMATCH 之类错误模型。

#### E2. usdb-index 的历史语义不错，但还没到共识输入模块的标准

usdb-index 已经具备：

- 历史高度查询语义。
- savepoint 和 pending marker。
- 一致性检查。
- 活跃余额快照和能量存储分层。

但要作为 ETHW 共识输入，还缺少：

- 显式 BTC 区块哈希锚定。
- 上游 snapshot identity 透传。
- 公式版本固定与版本校验。
- 对上游未 ready 状态的严格拒绝。
- 面向共识的 deterministic RPC 契约。

## 5. 风险清单

### 5.1 高优先级风险

#### R1. 共识分叉风险

触发条件：

- 不同节点使用了不同 BTC 视图。
- 或不同节点使用了不同版本的公式或查询语义。
- 或某些节点在半同步状态下参与了验块。

后果：

- 奖励计算不同。
- 区块合法性不同。
- 链分叉。

#### R2. 静默错误风险

触发条件：

- 未同步、无记录、真实零值被混淆。
- 上游返回默认值而不是显式错误。
- 下游缺少足够的输入校验。

后果：

- 系统继续运行。
- 结果错误但不报警。
- 问题只会在后续对账或链分歧中暴露。

#### R3. 升级漂移风险

触发条件：

- 某些节点升级了 balance-history。
- 某些节点升级了 usdb-index。
- 公式、查询语义、schema 或默认参数发生变化。
- ETHW 节点没有做版本匹配校验。

后果：

- 相同 BTC 高度得到不同衍生结果。
- 旧节点和新节点对同一区块作出不同判定。

### 5.2 中优先级风险

#### R4. 运维配置漂移风险

例如：

- 不同节点设置了不同的 N。
- 不同节点设置了不同的最大同步高度。
- local loader 切换阈值不同导致行为差异。
- 上下游服务未统一发布同一组配置。

#### R5. 审计困难风险

一旦出问题，无法快速重建：

- 当时具体使用了哪个 BTC 块。
- 该块对应的上游查询结果。
- 该结果的公式版本和中间过程。

## 6. 改造目标

后续所有改造可以统一围绕以下目标推进：

1. 让所有跨服务状态都能锚定到唯一 BTC 区块。
2. 让所有共识相关查询都具备 deterministic 语义。
3. 让所有未就绪或数据不充分状态显式报错。
4. 让升级行为可验证、可审计、可兼容治理。
5. 让 ETHW 节点只消费稳定且可证明一致的外部快照。

## 7. 分阶段改造清单

以下顺序建议按优先级推进。

### Phase 0. 先冻结语义，不急着改实现

目标：

- 先把共识侧到底认什么说清楚，避免一边开发一边改语义。

要做的事：

- 明确定义 stable_height 的概念。
- 明确定义 lag N 的来源、默认值、允许范围和治理方式。
- 明确定义所有共识相关查询都必须带 snapshot identity。
- 明确定义 exact-height 和 at-or-before 是两套不同接口语义。
- 明确定义未同步、无记录、哈希不匹配等错误码。

验收标准：

- 形成一版统一协议草案。
- balance-history、usdb-index、ETHW 三方对术语和边界一致。

建议在 Phase 0 先补充以下冻结定义，后续实现不得偏离：

#### P0-1. stable_height 的定义

- stable_height 定义为 balance-history 当前已经完整提交、并且按固定 lag N 规则对外承诺可用的 BTC 高度。
- 这里的 N 不是运行时可协商参数，而是 balance-history 内部固定配置；同一网络、同一协议版本下必须保持一致。
- usdb-index 不自行定义另一套 stable_height，而是直接依赖 balance-history 对外暴露的 stable_height，并保持一致。
- stable_height 必须与对应的 stable BTC block hash 成对出现，单独的高度不构成共识锚点。
- stable_height 与固定 lag N 都应计入 snapshot identity 的原始字段中，以保证不同节点在相同输入下产生同一 snapshot id。

建议补充一个术语区分：

- btc_tip_height：bitcoind 当前观察到的链尖高度。
- stable_height：balance-history 按固定 lag N 向外承诺的稳定高度。
- synced_height：服务内部已经处理完成的最高高度。对当前设计而言，balance-history 对外的 synced height 应等同于 stable_height，而不是 btc_tip_height。

#### P0-2. snapshot identity 的全局定义

- 所有共识相关查询都必须显式带 snapshot identity，不能再仅靠 at_height 或 block_height 隐式解析。
- snapshot identity 应作为全局结构定义，并在 balance-history、usdb-index、ETHW 节点三侧复用。

建议最小字段集：

- source_chain：固定为 BTC。
- network：mainnet、testnet、signet、regtest 之一。
- stable_height：稳定高度。
- stable_block_hash：该稳定高度对应的 BTC 区块哈希。
- stable_lag：固定 lag N。
- balance_history_api_version：balance-history 对外协议版本。
- balance_history_semantics_version：balance-history 查询语义版本。
- usdb_index_formula_version：usdb-index 派生公式版本。
- usdb_index_protocol_version：usdb-index 对外协议版本。

建议 snapshot_id 的生成规则也在 Phase 0 一起冻结，例如：

- 对上述字段按固定顺序做 canonical serialization。
- 再计算单一哈希作为 snapshot_id。
- 所有服务只接受这一种生成规则，避免不同实现序列化差异导致同字段不同 id。

#### P0-3. exact-height 与 at-or-before 的语义边界

- exact-height：只接受目标高度 H 的精确结果；如果 H 没有对应记录，必须显式返回无记录或未同步，不允许回退到 H 之前的记录。
- at-or-before：接受目标高度 H 之前最近一条记录，用于业务查询、统计展示或派生计算中的宽松读取。
- 这两者必须被定义为两套不同接口语义，不能只靠注释或调用方约定来区分。

需要额外明确 balance-history 当前余额存储模型的约束：

- balance-history 对地址余额采用稀疏存储，只在余额发生变动的区块写入一条 record。
- 因此，查询某地址在指定高度 H 的余额快照时，天然应该使用 at-or-before 语义；因为若高度 H 没有余额变动记录，正确余额本来就需要从 H 之前最近一次变动继承得到。
- 这意味着“指定高度余额查询”本身不应被定义成 exact-height 余额 record 查询，否则会把稀疏存储模型误判为数据缺失。
- 真正需要 strict exact 语义的，是“该高度是否存在变动记录”这类事件或 delta 查询，而不是余额快照查询本身。

基于当前代码 review，现状需要明确记录如下：

- balance-history 的 get_address_balance(block_height) 当前实现是 at-or-before 语义，而不是 exact-height。
- balance-history 的 get_address_balance_delta(block_height) 是 exact-height 的“该高度是否有变更记录”语义，但它返回的是 delta 记录，不等价于“精确高度余额快照接口”。
- usdb-index 的 energy 相关逻辑当前通过 balance-history 的 get_address_balance(block_height) 读取余额，因此它现在依赖的是 at-or-before 语义，而不是 exact-height。

这意味着 Phase 0 需要先把“希望使用 exact”与“当前实际使用 at-or-before”分开记录，后续在 Phase 2 再决定：

- 是否为 balance-history 新增“严格区分无记录与零余额”的余额快照接口返回结构。
- usdb-index 哪些路径必须改成 strict exact 依赖。
- 哪些路径可以继续保留 at-or-before 作为业务或投影接口。

当前更准确的改进方向应是：

- 保留 at-or-before 作为余额快照查询的核心语义。
- 增加结果状态位或错误码，区分地址不存在、历史上无记录、余额真实为零、请求高度未同步等不同情况。
- 对需要判断“该高度是否发生变化”的调用方，继续提供或增强 exact-height delta 查询接口。

#### P0-4. 全局统一错误码

- 错误码应做成跨服务统一规范，而不是 balance-history、usdb-index 各自定义一套相似但不完全一致的错误名。
- 错误码应覆盖至少三类场景：
  - 快照状态错误
  - 查询语义错误
  - 数据缺失或锚点不匹配错误

建议 Phase 0 至少冻结以下错误名与含义：

- HEIGHT_NOT_SYNCED：请求高度高于当前 stable_height，或服务尚未完成该高度快照。
- SNAPSHOT_NOT_READY：服务已启动，但当前 snapshot identity 尚未准备完成。
- SNAPSHOT_ID_MISMATCH：调用方提供的 snapshot id 与服务本地解析结果不一致。
- BLOCK_HASH_MISMATCH：请求高度对应的 block hash 与本地稳定视图不一致。
- ADDRESS_NO_RECORD：该地址在该语义约束下不存在记录。
- PASS_NO_RECORD：该 pass 在该语义约束下不存在记录。
- INVALID_QUERY_SEMANTICS：调用方请求的 exact 或 at-or-before 模式与接口能力不匹配。
- VERSION_MISMATCH：调用方要求的协议版本、语义版本或公式版本与服务当前版本不兼容。

建议统一约束：

- 同一类错误在三个服务中使用相同 message key。
- RPC 响应中的 data 字段携带 snapshot identity、requested_height、resolved_height、expected_hash、actual_hash 等上下文。
- 不允许用默认零值替代以上错误。

### Phase 1. 先把高度升级成锚点

目标：

- 把所有关键状态从 height-only 升级成 height + block_hash。

要做的事：

- balance-history 持久化并对外暴露当前 stable BTC block hash。
- usdb-index 持久化其所依赖的上游 BTC block hash。
- 所有对外响应结果增加：
  - btc_block_height
  - btc_block_hash
  - snapshot_resolved_height
  - snapshot_resolved_hash
- 所有跨服务调用必须校验锚点一致性。

验收标准：

- 任意一次状态查询都能回答这是基于哪个 BTC 区块得出的结果。
- usdb-index 无法在上游锚点未知或不一致时继续产出共识结果。

### Phase 2. 拆分业务查询语义与共识查询语义

目标：

- 保留业务友好接口，同时新增严格的共识接口。

要做的事：

- balance-history 保留现有业务查询接口。
- 新增共识专用查询接口：
  - exact-height
  - exact-block-hash
  - exact-snapshot
- 对共识接口禁止默认零值回退。
- 对共识接口返回显式错误码：
  - HEIGHT_NOT_SYNCED
  - BLOCK_HASH_MISMATCH
  - SNAPSHOT_NOT_READY
  - ADDRESS_NO_RECORD

验收标准：

- 共识调用方不再依赖 at-or-before 语义。
- 缺数据和真实零值完全可区分。

### Phase 3. 建立统一 snapshot identity

目标：

- 让 balance-history 和 usdb-index 都围绕同一个快照标识工作。

定义建议：

- source_chain = BTC
- block_height
- block_hash
- stable_lag
- protocol_version
- formula_version
- optional snapshot_hash

要求：

- balance-history 的响应包含该结构。
- usdb-index 的输入校验该结构。
- usdb-index 的输出继续透传该结构。
- ETHW 节点验块时记录并校验该结构。

验收标准：

- 任意派生值都能追溯到唯一 snapshot identity。
- 跨服务日志、RPC、数据库元信息全部能串起来。

### Phase 4. 严格化 ready 语义

目标：

- 让服务活着和快照可用于共识彻底分开。

要做的事：

- balance-history 增加 ready 状态：
  - 服务启动完成。
  - 当前 stable_height 已构建完成。
  - 当前 stable_hash 已确认。
  - 所有必要列族和元信息一致。
- usdb-index 只在上游 ready 后继续推进。
- ETHW 节点只在 usdb-index ready 且快照匹配时使用外部状态。
- 增加显式的健康检查接口和 readiness 接口。

验收标准：

- 任意层处于半同步状态时，下游都不会误用数据。
- ready 条件具有明确、可测试的判定标准。

### Phase 5. 建立版本治理体系

目标：

- 避免后续升级产生静默分歧。

要做的事：

为以下对象建立独立版本并纳入治理：

- DB schema version
- storage format version
- query semantics version
- pass state machine version
- energy formula version
- external consensus protocol version

并要求：

- 持久化到数据库元信息。
- 在 RPC 响应中返回。
- 在启动时做兼容性校验。
- 在 ETHW 节点做版本白名单或硬校验。

验收标准：

- 任意节点启动时即可发现版本不兼容。
- 不允许服务正常但输出口径已漂移的状态存在。

### Phase 6. 做端到端审计与重放能力

目标：

- 确保未来出了问题能重建现场。

要做的事：

- 为关键查询记录审计日志：
  - 请求锚点
  - 上游锚点
  - 返回值
  - 公式版本
  - 中间关键输入
- 为 usdb-index 生成可重放的派生证据。
- 增加按 snapshot identity 重放计算的离线工具。
- 增加 BTC 重组、服务重启、跨版本升级的回归测试场景。

验收标准：

- 任意一次异常结果都可以离线重放。
- 可以在测试环境完整重演一笔历史争议案例。

## 8. 推荐实施顺序

建议严格按下面顺序推进，不要跳步：

1. 先定统一语义和错误模型。
2. 再补 BTC block hash 锚定。
3. 再拆业务接口与共识接口。
4. 再引入统一 snapshot identity。
5. 再做 ready 语义和上游依赖强约束。
6. 再做版本治理。
7. 最后做审计、回放和大规模回归测试。
8. 在上述工作完成前，不建议把该链路直接接入 ETHW 共识判定。

## 9. 建议近期优先讨论的 5 个议题

为了后续讨论更聚焦，建议先从下面 5 个问题开始：

### 议题 1. ETHW 共识最终消费的最小外部状态单元是什么

例如到底是：

- pass level
- energy
- active balance snapshot
- 或者一个统一的 eligibility/result object

这个问题不先定，后面的接口都会反复改。

### 议题 2. stable_height 的精确定义是什么

需要明确：

- stable_height = btc_tip - N 是否固定。
- N 是否写入协议。
- N 是否允许动态调整。
- 调整后如何保证全网一致。

### 议题 3. 共识接口是否必须强制 exact-height + exact-hash

这是 balance-history 需要最早回答的问题。

### 议题 4. usdb-index 的输出结果最小锚点字段集是什么

建议至少明确：

- btc_block_height
- btc_block_hash
- snapshot_protocol_version
- formula_version

### 议题 5. ETHW 区块中需要记录哪些外部状态承诺

如果未来真的接入共识，需要决定：

- 只在本地验，不上链记录。
- 还是把某种 snapshot identity 或结果承诺写入区块头或区块体。
- 不同方案对可审计性和分叉处理影响很大。

## 10. 当前建议

在现阶段，建议把 usdb-index 和 balance-history 定位为：

- 向共识能力演进中的索引系统。

而不是：

- 已经可以直接作为共识输入的稳定状态机。

短期最重要的不是继续堆功能，而是先补确定性边界。

只有先把：

- BTC 锚点
- 查询语义
- ready 语义
- 版本治理

这四件事做实，后面讨论 ETHW 收益规则、等级规则、能量公式、区块承诺格式才有意义。

## 11. 里程碑建议

### M1. 语义冻结里程碑

完成后应产出：

- 统一术语表
- 错误码表
- snapshot identity 草案
- 共识接口与业务接口边界说明

### M2. 锚点化里程碑

完成后应产出：

- 全链路 block hash 锚定
- 上下游锚点一致性校验
- 以 snapshot identity 为核心的接口草案

### M3. 共识接口里程碑

完成后应产出：

- exact-hash exact-height 查询能力
- 严格错误语义
- ready 语义与健康检查

### M4. 接入评估里程碑

完成后再评估是否允许进入 ETHW 共识侧灰度接入。

## 12. 最后一条原则

只要一个外部状态不能回答下面这句话，它就还不能进入共识：

这个结果是由哪个 BTC 区块、用哪个版本的规则、在什么稳定窗口内、基于哪份完整快照算出来的，并且所有节点都能得到同样结果。

这应该作为后续所有改造的最高约束。

## 13. balance-history 块级提交哈希方案分析

### 13.1 目标边界

对于 balance-history，当前更合理的目标不是给整个数据库文件做物理 hash，也不是一开始就引入完整的默尔克树证明系统，而是先给逻辑状态引入可重放、可校验的块级提交哈希。

这里需要先明确状态边界：

- balance-history 的 UTXO 存储主要用于本地快速索引和回填缺失输入。
- 当 UTXO 缺失时，系统允许从 BTC RPC 服务重新加载。
- 因此，UTXO 缓存不构成 balance-history 对外承诺的核心状态，不应纳入共识快照的状态根。
- 需要纳入状态承诺范围的，应是余额历史结果本身，也就是 balance history 的逻辑结果。

这意味着 balance-history 的提交哈希应只覆盖余额历史逻辑状态，而不覆盖 UTXO 本地缓存状态。

### 13.2 为什么该方案可行

从当前实现看，balance-history 虽然前半段使用并行预处理和批量写库，但最终影响余额状态的逻辑顺序仍然是确定的。

关键观察如下：

- block preload 阶段会并行拉取多个 block，并并行预处理 vin/vout。
- 预处理完成后，所有 block 会重新按 block height 排序。
- 余额变化的逻辑计算虽然会先并行生成每个 block 的 delta 集，但最终会按排序后的 block 顺序依次应用到批次内余额状态。
- 最终的数据库更新虽然是按 batch 一次性写入，但该 batch 内每个 block 的逻辑结果边界实际上已经在内存中形成。

因此：

- block 预处理顺序可以乱。
- 逻辑状态转移顺序不能乱。
- 当前实现满足“预处理可乱、状态推进有序”的条件。

这使得 balance-history 可以在 batch 内为每个 block 生成稳定的逻辑 root，然后在 batch flush 成功后再顺序落库对应的 block commit。

### 13.3 为什么不应该直接 hash 整个数据库文件

直接对 RocksDB 或 SQLite 文件做 hash 不可靠，原因包括：

- 底层文件布局受 compaction、页分裂、写入顺序、数据库版本影响。
- 两个节点即使逻辑状态完全相同，物理文件字节也可能不同。
- 文件级 hash 无法天然表达 block 边界，也不利于重放与审计。

因此必须 hash 规范化后的逻辑数据，而不是物理存储文件。

### 13.4 推荐的状态单元

对于 block h，建议只对“该 block 对余额历史产生的逻辑结果”做 hash。

该逻辑结果可以定义为每个地址在 block h 结束后的规范化余额条目集合，条目最小字段建议包括：

- script_hash
- block_height
- delta
- balance_after_block

这些条目在 block 内按 script_hash 升序编码后，可得到：

$$
balance\_delta\_root(h) = H(\text{sorted balance entries at block } h)
$$

然后再结合链锚点和前一提交，得到块级提交：

$$
block\_commit(h) = H(
service\_id \parallel
protocol\_version \parallel
btc\_height \parallel
btc\_block\_hash \parallel
balance\_delta\_root(h) \parallel
block\_commit(h-1)
)
$$

这样得到的是一条逻辑提交链，而不是物理数据库文件的 hash 链。

### 13.5 最合适的计算时机

基于当前实现，最合适的计算时机不是：

- preload 阶段，因为此时 block 顺序仍可能无序；
- 全量同步结束后统一重算，因为成本高且丢失了批次提交边界；
- flush 完成之后重新扫描数据库，因为那样又回到物理存储视角。

最合适的时机是：

- 在 batch 内各 block 的逻辑 balance delta 已经计算完成；
- 且这些 block 已经按 height 排序；
- 但当前 batch 尚未被宣布为已提交 stable 状态。

也就是当前实现中 block.rs 的余额计算结果已经形成、而 flusher 尚未最终提交数据库的那一段边界。

这时可以：

1. 为 batch 中每个 block 计算 balance_delta_root。
2. 以上一已提交 block_commit 为 prev，依次滚动计算本 batch 的 block_commit。
3. 在 batch flush 成功时，把余额历史更新和本 batch commit 元信息一起提交。
4. 最后再更新最新已提交高度和最新 commit。

### 13.6 推荐的第一阶段方案

对 balance-history，建议第一阶段只做以下能力：

- 每个 block 的 balance_delta_root。
- 每个 block 的 block_commit。
- 当前 stable_height 对应的 latest_block_commit。

第一阶段不建议引入：

- UTXO 状态根。
- 全量数据库状态根。
- 查询 proof。
- 完整默尔克树或稀疏默尔克树。

这样能在较低改造成本下获得：

- 稳定的逻辑提交标识。
- 与 BTC block hash 绑定的快照承诺。
- 更强的跨节点一致性校验能力。
- 后续进一步扩展到更强状态根或证明系统的基础。

### 13.7 与 snapshot identity 的关系

如果该方案落地，balance-history 的 snapshot identity 不应只包含：

- stable_height
- stable_block_hash

还应进一步包含：

- latest_block_commit
- commit_protocol_version
- commit_hash_algo

这样下游服务在依赖 balance-history 时，校验的就不再只是“是否对齐到同一高度”，而是“是否对齐到同一份逻辑提交状态”。

### 13.8 后续扩展方向

在该方案稳定之后，再考虑是否继续增加：

- 周期性 full state root，用于审计和 checkpoint。
- 更细粒度的 per-query proof。
- 面向共识消费的可验证状态树结构。

当前阶段不建议直接跳到完整默尔克证明系统，优先应把块级逻辑提交链做稳定。