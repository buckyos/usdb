# UIP 重构备忘

本文记录基于 `doc/UIP/` 草案逐步对接现有实现时的改动事项，方便按 UIP 顺序 review。当前 UIP 仍处于 Draft，但已作为实现工作合同使用；实现过程中发现的缺口会继续回填到本备忘和对应 UIP。

## UIP-0001 Miner Pass Inscription Schema

状态：当前 dev schema/storage/RPC/test/script surface 已对齐；activation matrix 和后续 collab/effective energy 语义转入后续 UIP。

### 已对接内容

- `doc/UIP/UIP-0001-miner-pass-inscription.md`
  - 开发期实现策略收紧为 dev 直接使用 v1 strict schema。
  - 旧 payload、旧数据库和临时 parser 字段不作为兼容对象；测试需要时删除旧 DB 并按 v1 schema 重建。
  - 移除 duplicate key 检测未决项；当前 schema 已要求 top-level duplicate JSON key invalid。
- `src/btc/usdb-indexer/src/index/content.rs`
  - 增加 v1 strict schema 解析。
  - 要求 `p == "usdb"`、`op == "mint"`、`v == 1`。
  - 顶部注释示例已对齐 v1 schema，包含 `v` 且不包含 `usdb_collab`。
  - 增加 `standard` / `collab` 两类 pass 推导。
  - `standard` 要求 `usdb_main`，禁止 leader 绑定字段。
  - `collab` 要求 `leader_pass_id` / `leader_btc_addr` 二选一，禁止 `usdb_main`。
  - v1 禁止 `usdb_collab`。
  - `prev` 缺省等价于空数组，并校验 inscription id 与重复项。
  - unknown field 判 invalid。
  - string parser 增加 top-level duplicate JSON key 检测。
  - content-type 接受 `application/json;charset=utf-8`，并保留 text/plain UTF-8 兼容。
- `src/btc/usdb-indexer/src/index/indexer.rs`
  - 将 `mint_version`、`pass_kind`、`leader_pass_id`、`leader_btc_addr` 从解析结果传递到 pass manager。
  - `inscription_source.load_block_mint_batch` 调用传入当前 BTC network，保证 `leader_btc_addr` 按配置网络校验。
- `src/btc/usdb-indexer/src/index/pass.rs`
  - `PassMintInscriptionInfo` 与 mint mutation 记录新增 UIP-0001 字段。
  - 状态面不再携带 `usdb_collab`；该字段仅作为 v1 payload invalid 条件保留在 parser。
- `src/btc/usdb-indexer/src/storage/pass.rs`
  - `MinerPassInfo` 新增 `mint_version`、`pass_kind`、`leader_pass_id`、`leader_btc_addr`。
  - `miner_passes` 建库 schema 直接使用 v1 字段并移除 `usdb_collab` 列；dev 阶段不保留旧数据库兼容迁移，测试需要时删除旧 DB 重建。
  - rollback 重建、history owner/recent 查询保留新增字段。
- `src/btc/usdb-indexer/src/service/rpc.rs`
  - pass snapshot、owner pass、recent pass、invalid pass 响应新增 UIP-0001 字段。
  - pass 响应不再暴露 `usdb_collab`。
- `src/btc/usdb-indexer/scripts`
  - regtest/live mint payload 直接补齐 `"v": 1`，不保留缺失版本字段的开发期格式。

### 已补测试

- v1 standard mint valid。
- v1 collab mint with `leader_pass_id` valid。
- v1 collab mint with `leader_btc_addr` valid。
- v1 missing `prev` 等价于空数组。
- v1 invalid `usdb_main`。
- v1 invalid `leader_pass_id`。
- v1 invalid `leader_btc_addr` for active BTC network。
- v1 同时包含 `usdb_main` 和 leader 绑定字段 invalid。
- v1 同时包含 `leader_pass_id` 和 `leader_btc_addr` invalid。
- v1 同时缺失 `usdb_main`、`leader_pass_id` 和 `leader_btc_addr` invalid。
- v1 包含 `usdb_collab` invalid。
- v1 unknown field invalid。
- v1 duplicate key invalid。
- pre-standard payload 不作为正式 v1 解析。
- source 层分类使用传入的 BTC network 校验 `leader_btc_addr`，覆盖 regtest 地址路径。
- regtest/live 脚本中的 USDB mint payload 覆盖 v1 `v` 字段。
- collab pass 的新增字段可写入并从 storage/history 查询读回。

### 已验证

- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer content`
- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer pass_storage_persists_uip0001_leader_fields -- --nocapture`
- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer classify_usdb_mints_uses_supplied_network_for_leader_btc_addr -- --nocapture`
- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer`
- `cargo check --manifest-path src/btc/Cargo.toml --workspace`
- `bash -n` for touched regtest/live shell scripts
- `python3 -m py_compile src/btc/usdb-indexer/scripts/regtest_world_simulator.py`
- `bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_e2e.sh`

### 暂缓事项

- activation matrix 尚未接入；当前实现直接按 v1 strict schema 解析 USDB mint。
- `leader_pass_id` 引用对象是否必须在 mint 时已存在，留给 UIP-0002 状态机处理。
- `leader_btc_addr` 在历史高度解析 active standard leader 的逻辑，留给 UIP-0002 / UIP-0004。
- collab pass 的 raw energy、effective energy、防双计数和 candidate set 过滤，留给 UIP-0003 / UIP-0004。

## UIP-0002 Miner Pass State Machine

状态：第一轮 review 进行中，已对齐 `prev` strict invalid、leader pass、burn 终态、terminal transfer 与 block-level balance settlement 复核项。

### 已对接内容

- `src/btc/usdb-indexer/src/index/pass.rs`
  - `on_mint_pass` 在写入任何状态前完整校验 `prev` 状态前置条件。
  - `prev` 缺失、owner 不一致、非 Dormant 或重复引用会将本次 mint 记录为 `Invalid`，不再 warn/skip。
  - 同 owner 当前 active pass 可在同一次 mint 中作为虚拟 Dormant `prev` 被原子消费；若同次 mint 还有其他 invalid `prev`，旧 active pass 保持原状态。
  - `leader_pass_id` collab mint 校验 Leader pass 存在、为 active standard pass，且不是本次 mint 自身；同 block 前序 canonical event 创建的 standard Leader 可被后序 collab mint 引用。
  - same-owner mint supersede 时先完成 old active 的 event-height energy settlement，再写 pass `Active -> Dormant` 状态。
- `src/btc/usdb-indexer/src/index/test/pass_scenario.rs`
  - 更新 missing referenced `prev` 与重复继承已 consumed `prev` 的测试期望为 invalid mint。
- `src/btc/usdb-indexer/src/index/test/indexer_behavior.rs`
  - 更新 burned `prev` remint 和已 consumed `prev` 二次继承的 block-level 行为期望。
  - 增加同 block 前序 standard Leader mint + 后序 `leader_pass_id` collab mint 的 canonical ordering 测试。
  - 更新 mint/transfer/burn/remint timeline，断言 burn 高度及后续 energy 为 `Burned/0`。
  - 增加 same-owner transfer exact-height active settlement 后 block-end settlement 幂等的集成测试。
  - 增加 same-owner remint 在同高度负 delta 下先扣 penalty、再继承扣罚后 energy，且新 pass 不额外承担 pre-mint penalty 的集成测试。
- `src/btc/usdb-indexer/src/index/energy.rs`
  - 增加 burn 终态写入：`Active` / `Dormant` burn 在 event height 写入 `Burned/0` energy record，并按 burn 前 pass state 精确校验 energy state。
- `src/btc/usdb-indexer/src/index/pass.rs`
  - burn 仅允许 `Active` / `Dormant` 转 `Burned`；`Consumed` / `Burned` / `Invalid` burn 保持当前经济状态。
  - 增加 active、dormant、consumed burn，以及 pass/energy 状态不一致时 fail-fast 的单元测试覆盖。
  - `Consumed` / `Burned` / `Invalid` transfer 作为非共识 physical transfer 处理，不更新 owner/satpoint，不写 history 或 pass commit mutation。
- `src/btc/usdb-indexer/src/storage/pass.rs`
  - transfer-trackable pass 查询收敛为仅返回 `Active` / `Dormant`，避免 terminal pass 后续物理流转进入共识状态。
- `doc/UIP/UIP-0002-pass-state-machine.md`
  - 将测试要求中的 missing `prev` 表述明确为 missing referenced `prev`，避免与 UIP-0001 的 `prev` 缺省等价空数组冲突。
  - 移除 `leader_pass_id` 同 block canonical ordering 未决项，当前实现按 ordered event context 校验。
  - 去掉 burn energy 终态仍未封口的过期描述；明确当前 BTC indexer 不保留 terminal pass 后续 physical transfer 审计记录。

### 待继续对齐

- 同一 height 下是否需要非共识审计 API 暴露 event index；协议状态查询暂不需要。

## UIP-0003 Pass Raw Energy Formula and Inheritance

状态：公式层 helper、纯单元测试、energy manager settlement/projection、pass inheritance、storage `u128` 与 RPC decimal string 已对接。

### 已对接内容

- `src/btc/usdb-indexer/src/index/energy_formula.rs`
  - 新增 `Energy = u128` 与 UIP-0003 常量：`UNIT_SATS`、`ENERGY_PER_UNIT_BLOCK`、`PENALTY_LAMBDA_NUM`、`PENALTY_LAMBDA_DEN`、`INHERIT_DISCOUNT_BPS`、`BPS_DENOMINATOR`、`ENERGY_MAX`。
  - 新增 `balance_units` 与 `calc_unit_delta`，按 before/after unit 快照计算 `gained_units` / `lost_units`。
  - 新增 `calc_growth_delta_energy`，按 `balance_units(owner_balance_sats) * ENERGY_PER_UNIT_BLOCK * block_delta` 计算 raw energy 增长。
  - 新增 `calc_penalty_energy` 与 `calc_balance_penalty_energy`，按 lost units、active age 和 `3/2` penalty lambda 计算扣罚。
  - 新增 `calc_next_active_block_height`，封装 `0 unit -> positive units` 与 `lost_units > 0 && units_after == 0` 的 active height 更新边界。
  - 新增 `calc_inheritable_energy`，按 per-prev `floor(raw_energy * 9500 / 10000)` 计算 5% 继承折损。
  - 新增 `mul_div_floor_saturating`，避免 `u128::MAX` 参与 bps 乘除时先乘溢出。
  - `calc_growth_delta` 统一返回 `Energy`，公式层、manager、storage 与测试侧不再把 raw energy 截断为 `u64`。
  - 删除旧 `calc_penalty_from_delta` 入口，settlement 不再按 signed satoshi delta 计算 penalty。
- `src/btc/usdb-indexer/src/index/energy.rs`
  - projection 改为按 `last_settlement_height = record.block_height` 计算增长窗口，不再以 `active_block_height` 作为增长窗口输入。
  - range settlement 与 block-end settlement 统一使用 `last_record.owner_balance -> next_owner_balance` 的 unit delta 计算 penalty。
  - active height 更新改为 `0 unit -> positive units` 与 `lost_units > 0 && units_after == 0` 两个边界。
  - balance-history range 记录按 height 排序后结算，避免 provider 返回顺序影响 deterministic settlement。
- `src/btc/usdb-indexer/src/index/pass.rs`
  - `prev` consume 成功后写入同高度 `Consumed/0` energy record。
  - consume 返回值改为 `floor(raw_energy * 9500 / 10000)` 的 inheritable energy。
  - 多 `prev` mint 按每个 prev 单独折损后再求和，不再无损继承 raw energy。
- `src/btc/usdb-indexer/src/storage/energy.rs`
  - `PassEnergyValue` / `PassEnergyRecord` 的 `energy` 字段改为 `Energy = u128`；dev 阶段不做旧 DB 迁移兼容。
- `src/btc/usdb-indexer/src/service/rpc.rs`
  - `PassEnergySnapshot` 删除单一 `energy` 字段，改为 `raw_energy`、`collab_contribution`、`effective_energy` 三个 canonical decimal string。
  - `PassEnergyRangeItem`、`PassEnergyLeaderboardItem` 的 `energy` 字段保持 raw energy，输出 canonical decimal string。
- `src/btc/usdb-indexer/src/service/server.rs`
  - RPC 单 pass energy 先按 UIP-0003 raw-only 暴露三字段：`collab_contribution = "0"`，`effective_energy = raw_energy`。
  - range 直接将 `u128` raw energy 编码为 decimal string。
  - leaderboard 先保留 `u128` ranking key 排序，再把 item energy 编码为 decimal string，避免按字符串排序。

### 已补测试

- `balance_units` threshold floor。
- unit delta 使用 before/after 快照而非 `abs(delta)` floor。
- below threshold / threshold / multi-unit growth。
- `u128` 饱和乘除与 transitional `u64` clamp helper。
- penalty 使用 lost units、age 和 `3/2` lambda。
- 非 unit loss 不触发 penalty。
- active height 仅在 UIP-0003 指定边界更新。
- energy manager 覆盖 partial unit loss 保持 active height、full unit loss 重置 active height、`0 unit -> positive units` 重置 active height。
- inheritable energy 5% discount floor。
- single prev remint 继承折损后的 raw energy，且 prev 同高度 query 为 `Consumed/0`。
- multi prev remint 逐项先折损再求和，并覆盖与“先求和再折损”不同的 rounding 场景。
- multi prev 继承求和超过 `ENERGY_MAX` 时 saturate 到 `u128::MAX`。
- storage `u128` energy roundtrip，覆盖 `u64::MAX + 1`、`u128::MAX - 1`、`u128::MAX`。
- energy manager active projection 在接近 `u128::MAX` 时 saturating 到 `ENERGY_MAX`。
- RPC `get_pass_energy` 覆盖 exact、projection saturation、三字段 decimal string 输出，并断言旧 `energy` 字段不再序列化。
- RPC range / leaderboard 覆盖 raw `items[].energy` decimal string 输出和 cache 场景。
- leaderboard 覆盖先按 `u128` 数值排序再编码，避免 decimal string 字典序排序；并覆盖同 energy 下 `record_block_height DESC` / `inscription_id ASC` tie-breaker。
- `u128::MAX` 继承折损不发生先乘溢出。
- penalty 饱和到 `ENERGY_MAX`。

### 已验证

- `cargo fmt`
- `cargo test energy_formula`
- `cargo test energy`
- `cargo test energy_timeline`
- `cargo test pass_scenario`
- `cargo test`
- `cargo check`
- `cargo clippy --all-targets -- -D warnings`

### 待继续对齐

- 继续对照 UIP-0003 审核 collab effective energy / raw energy 聚合和后续 UIP-0004 candidate set 使用方式。

## UIP-0004 Collab Leader and Effective Energy

状态：公式层 helper、leader resolver、storage 查询、只读 effective energy resolver、`get_pass_energy` 三字段聚合、collab breakdown 审计查询、candidate set view、validator payload 三字段和核心测试覆盖已对接；UIP-0004 core 基本收尾。后续 quote activity / candidate energy 口径留到 UIP-0014。

### 已对接内容

- `doc/UIP/UIP-0004-collab-leader-effective-energy.md`
  - 明确 `collab_contribution` 使用 UIP-0003 `energy_uint`，bps 乘除按整数 floor 计算并 saturate 到 `ENERGY_MAX`。
  - 明确 `raw_energy + Σ collab_contribution` 使用 `energy_uint` 饱和加法，超过 `ENERGY_MAX` 时 effective energy 固定为 `ENERGY_MAX`，且不得写回 raw energy ledger。
  - 补充当前 `usdb-indexer` 实现接口、validator payload 三字段、后续 UIP 边界和 live/regtest 集中复核策略。
- `src/btc/usdb-indexer/src/index/energy_formula.rs`
  - 新增 `COLLAB_WEIGHT_BPS = 5_000`。
  - 新增 `calc_collab_contribution(raw_energy)`，按 `floor(raw_energy * 5000 / 10000)` 计算单张 active collab pass 的折算贡献。
  - 新增 `calc_standard_effective_energy(raw_energy, collab_contribution)`，统一封装 standard pass 的 `raw + aggregate contribution` 饱和加法。
- `src/btc/usdb-indexer/src/storage/pass.rs`
  - 新增按 inscription id 查询指定高度 pass snapshot 的 helper，返回历史 owner/state/satpoint 与不可变 mint 字段。
  - 新增按 owner 查询指定高度唯一 active standard pass 的 helper，用于 `leader_btc_addr` 解析。
  - 新增按高度、state、pass kind 过滤 pass snapshot 的 count/page helper，并封装 active standard / active collab 专用查询。
  - 新增 active standard owner 分页 helper，供后续 candidate set / leaderboard 只枚举 standard pass。
  - 新增 `pass_kind` 与历史 owner/state/height 相关索引，支撑 UIP0004 kind-aware 查询。
  - 新增内部字段 `leader_btc_owner`，在写入 collab mint 时将 `leader_btc_addr` 按当前 BTC network 规范化为 script hash，用于 runtime 查询优化；该字段不作为 RPC/protocol surface 暴露。
  - 新增 active collab by `leader_pass_id` / `leader_btc_owner` 查询和索引，避免 effective energy resolver 全量扫描 active collab。
- `src/btc/usdb-indexer/src/index/pass.rs`
  - 新增 `resolve_leader_pass_id_at_height`，固定 pass 绑定只在目标高度存在 active standard snapshot 时解析成功。
  - 新增 `resolve_leader_btc_addr_at_height`，按当前 BTC network 将 Leader address 转为 owner script hash，再解析该 owner 在目标高度的 active standard pass。
  - 新增 `resolve_collab_leader_at_height`，按 collab mint payload 的 leader ref kind 统一返回 resolved Leader snapshot。
- `src/btc/usdb-indexer/src/index/effective_energy.rs`
  - 新增只读派生层，`raw_energy` 直接来自 UIP-0003 raw energy ledger/projection。
  - active standard pass 在查询时枚举 active collab pass、解析 Leader，并聚合 `calc_collab_contribution(raw_energy)`。
  - active collab pass 与 non-active pass 的 `effective_energy` 均派生为 0。
  - `raw + Σ collab_contribution` 使用 `energy_uint` 饱和加法，且不写回 raw energy DB。
- `src/btc/usdb-indexer/src/service/server.rs`
  - `get_pass_energy` 三字段接入 UIP-0004 派生结果：`raw_energy` 保持 UIP-0003 原值，`collab_contribution` / `effective_energy` 运行时计算。
  - 新增 `get_collab_breakdown`，按 `leader_pass_id + height/context + cursor/limit + sort` 返回稳定分页审计明细，并暴露全量 `aggregate_collab_contribution`。
  - 新增 `get_candidate_set_view`，只枚举 active standard pass，排除 collab pass，并按 `effective_energy DESC, pass_id ASC` 返回 UIP-0006 candidate set audit view。
  - `get_pass_energy_leaderboard` 保留为前端/浏览器 raw energy 展示榜单，不改造成 validator candidate set 口径。
- `src/btc/usdb-indexer/src/service/rpc.rs`
  - 更新 `PassEnergySnapshot` 字段注释，移除 UIP-0004 未实现的旧说明。
  - 新增 `GetCollabBreakdownParams`、`CollabBreakdownItem` 和 `CollabBreakdownPage`。
  - 新增 `GetCandidateSetViewParams`、`CandidateSetViewItem` 和 `CandidateSetViewPage`，并声明 UIP-0006 view version / selection rule。
- `doc/usdb-indexer/usdb-indexer-rpc-v1.md`
  - 更新 `get_pass_energy` 的 UIP0004 三字段语义。
  - 明确 `get_pass_energy_leaderboard` 不是 validator candidate set。
  - 记录 `get_candidate_set_view` 参数、返回字段、selection rule 和 fail-closed 语义。
  - 记录 `get_collab_breakdown` 参数、返回字段、sort 规则和 aggregate 审计口径。
- `src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh`
  - validator block-body JSON payload 删除单一 `energy` 字段，`miner_selection` / `candidate_passes` 统一携带 `raw_energy`、`collab_contribution`、`effective_energy`。
  - candidate-set winner 选择规则收敛为 `uip-0006:effective-energy-desc-pass-id-asc:v1`，本地重算按 `effective_energy DESC, inscription_id ASC`。
  - validator payload 校验会逐项重查并比对三字段，保证 aggregate 可审计。
- `src/btc/usdb-indexer/scripts/regtest_world_simulator.py`
  - validator sampled validation 的 single / candidate-set 样本改为保存并校验 `raw_energy`、`collab_contribution`、`effective_energy`。
  - world-sim candidate-set winner 重算显式按 `effective_energy DESC, inscription_id ASC`。
- `doc/usdb-indexer/*validator*`
  - 更新 validator block-body 与 world-sim candidate-set 文档，移除旧 `max_energy` / 单 `energy` payload 口径。

### 已补测试

- collab contribution 50% 权重与 floor rounding。
- `u128::MAX` collab contribution 不发生先乘溢出。
- standard effective energy 的 raw + contribution 饱和到 `ENERGY_MAX`。
- `leader_pass_id` 只解析 active standard pass；dormant 或 collab pass 不解析。
- `leader_btc_addr` 在无 active pass 时不解析，并在 Leader 同 owner remint 后自动跟随新 active standard pass。
- `leader_btc_addr` 使用当前 BTC network 解析，错误网络地址会拒绝解析。
- storage kind-aware 查询覆盖 active standard / active collab 计数、active collab 枚举和 active standard owner 分页。
- storage leader-ref 查询覆盖 active collab by `leader_pass_id` 和 by normalized `leader_btc_owner`，并排除非 active collab。
- collab mint 写入路径覆盖 `leader_btc_addr -> leader_btc_owner` 规范化落库。
- `get_pass_energy` active standard 聚合多个 active collab contribution，并断言派生 effective 不写回 raw energy storage。
- `get_pass_energy` 覆盖 `leader_btc_addr` 动态 Leader 绑定的 collab contribution。
- `get_pass_energy` 覆盖 dormant / consumed / burned / invalid Leader 不接收 collab contribution，breakdown 返回空 aggregate。
- `get_pass_energy` 覆盖 `leader_btc_addr` 在 Leader remint 后自动跟随新 active standard pass，同时 fixed `leader_pass_id` 不跟随旧 Leader remint。
- `get_pass_energy` 覆盖 old collab consumed 后不再向旧 Leader 贡献 contribution，breakdown aggregate 归零。
- `get_pass_energy` active collab pass 的 `raw_energy` 保留、`effective_energy` 为 0。
- `get_pass_energy` non-active standard pass 的 `raw_energy` 保留、`effective_energy` 为 0。
- `get_collab_breakdown` 覆盖 contribution desc + pass id tie-break 的稳定分页、全量 aggregate、`leader_pass_id` 与 `leader_btc_addr` 两种 ref kind。
- `get_collab_breakdown` 覆盖全量明细可重算 aggregate。
- `get_collab_breakdown` 覆盖 non-active Leader 返回空 breakdown。
- `get_collab_breakdown` 覆盖 context height mismatch。
- `get_candidate_set_view` 覆盖旧 raw leaderboard 仍展示 active collab，但 candidate set 排除 active collab / dormant standard。
- `get_candidate_set_view` 覆盖按 standard `effective_energy` 排序，并在相同 effective energy 下按 pass id 升序打平。
- `get_candidate_set_view` 覆盖 explicit selection rule、context height mismatch、invalid selection rule 和 active standard 缺 raw energy 时 fail-closed。
- pass manager 覆盖 old collab remint 为 standard / collab 时，new pass 只继承 UIP-0003 折损后的 raw energy，old collab 进入 Consumed 且终态 raw energy 为 0。

## UIP-0005 Level and Real Difficulty

状态：UIP-0005 usdb-indexer core 暂时收尾。公式层 helper、纯单元测试、`get_pass_energy` 查询派生字段、candidate set view 派生字段、状态边界和服务层交叉验证均已对接；版本绑定和 validator / ETHW policy 闭环由 UIP-0006、UIP-0008、UIP-0009 承接。

### 已对接内容

- `src/btc/usdb-indexer/src/index/energy_formula.rs`
  - 新增 UIP-0005 level / difficulty 参数常量、整数阈值表和关键常量注释。
  - 新增 `calc_level_from_effective_energy(effective_energy)`，按整数阈值表计算 level，运行时不使用浮点、`log` 或 `pow`。
  - 新增 `calc_difficulty_factor_bps(level)`，按每级 100 bps 折扣计算并 clamp 到 `MIN_DIFFICULTY_FACTOR_BPS = 5000`。
  - 新增 `calc_real_difficulty(base_difficulty, difficulty_factor_bps)` 纯公式 helper，用于 ETHW 侧 ceil 规则测试；usdb-indexer 不查询、不持久化 ETHW `base_difficulty` / `real_difficulty`。
- `src/btc/usdb-indexer/src/service/rpc.rs`
  - `PassEnergySnapshot` 增加 `level` 和 `difficulty_factor_bps`。
  - `CandidateSetViewItem` 增加 `level` 和 `difficulty_factor_bps`。
- `src/btc/usdb-indexer/src/service/server.rs`
  - `get_pass_energy` 从 UIP-0004 `effective_energy` 运行时派生 `level` / `difficulty_factor_bps`，不写入 energy DB。
  - `get_candidate_set_view` item 增加 `level` / `difficulty_factor_bps`，同样从每个 candidate 的 `effective_energy` 运行时派生；排序仍保持 `effective_energy DESC, pass_id ASC`。
  - 补充状态边界测试：active standard 使用 collab 聚合后的 `effective_energy` 计算 level；active collab、non-active standard、dormant / consumed / burned / invalid Leader 的 `effective_energy = 0` 时均返回 `level = 0`、`difficulty_factor_bps = 10000`。
  - 增加服务层交叉验证：同一 leader/collab 场景下，`get_pass_energy` 与 `get_candidate_set_view` 的 leader energy / level / factor 完全一致；collab 自身返回 `level = 0`、`difficulty_factor_bps = 10000` 且不进入 candidate set。
- `doc/usdb-indexer/usdb-indexer-rpc-v1.md`
  - 更新 `PassEnergySnapshot` 和 `get_pass_energy` 字段说明，明确 UIP-0005 派生字段不依赖 ETHW difficulty。
  - 更新 `get_candidate_set_view` 返回字段说明，明确 UIP-0005 字段不改变 candidate 排序口径。

### 待继续对齐

- UIP-0006：统一 economic state view、candidate set view/profile 版本字段和审计查询口径。
- UIP-0008：统一 formula version、历史高度 activation matrix 与参数版本重放。
- UIP-0009：明确 ETHW `base_difficulty` / `real_difficulty` 类型、来源和重算 policy。
- 大规模 live/regtest 场景在 UIP-0005 / UIP-0006 对齐后集中复核和重构，避免中间字段反复调整。
- UIP-0014 的 quote activity / candidate energy 口径：quote stale 时 ETHW 侧 candidate energy 回落为 raw/self energy；不反向修改 USDB indexer 的 effective energy。

## UIP-0006 USDB Economic State View

状态：任务 1（v1 查询/响应契约收敛）、任务 2（共享历史 context / version mismatch / external state 基础）、任务 3（单 Pass economic profile）和任务 4（state-ref 二次复验与 cursor 稳定分页）已对接。当前开发阶段不保留省略 `view_version`、旧 `page/page_size`、缺少 formula selector 或旧 protocol mismatch 名称的兼容入口。

### 已对接内容

- `doc/UIP/UIP-0006-usdb-economic-state-view.md`
  - 固定 `view_version = uip-0006-usdb-economic-state-view:v1`，所有 UIP-0006 请求顶层必填；该字段不进入 `ConsensusStateReference`。
  - 固定 `EconomicExternalState` 的 9 个必填字段，补齐 `balance_history_api_version`，并要求 protocol/formula 来自目标高度历史 identity。
  - 固定 invalid profile 的 raw/contribution/effective canonical 零值和 `level=0 / factor=10000` 查询语义，不要求伪造 energy DB row。
  - 固定 candidate/breakdown 的 `cursor + limit` 契约、cursor 绑定范围和错误边界；实现已直接替换数字分页，不保留双栈。
  - 明确 `ECONOMIC_FIELD_MISMATCH` 属于下游 verifier 重算结果，不是无 expected economic fields 的查询 RPC 错误；服务内部矛盾使用 `INTERNAL_INVARIANT_BROKEN`。
- `src/btc/usdb-util/src/types.rs`
  - `ConsensusStateReference` 增加 `usdb_index_formula_version`。
  - 新增 `VIEW_VERSION_MISMATCH(-32050)`、`PROTOCOL_VERSION_MISMATCH(-32051)`、`FORMULA_VERSION_MISMATCH(-32052)`；balance-history API/semantics 继续使用通用 `VERSION_MISMATCH(-32044)`。
  - `ConsensusRpcErrorData` 增加 canonical `mismatch_field`。
- `src/btc/balance-history/src/service/{rpc,server}.rs`
  - current/historical state reference 均暴露 formula version。
  - protocol/formula selector 按目标高度 `ConsensusSnapshotIdentity` 校验并返回独立错误；所有 mismatch 返回具体字段名。
- `src/btc/usdb-indexer/src/service/rpc.rs`
  - 新增强类型 `EconomicExternalState`，支持从 `HistoricalStateRefInfo` 无损构造，并可反向生成完整 `ConsensusStateReference / ConsensusQueryContext`。
  - `get_candidate_set_view` / `get_collab_breakdown` 请求增加必填 `view_version`，响应统一增加 `view_version + external_state`。
  - 新增 `get_pass_economic_profile` 强类型请求/响应，固定 `pass_id`、owner、state/kind、三类 energy、level/factor 和 breakdown count 字段。
  - candidate/breakdown 请求改为 `cursor + limit` 并拒绝未知字段；响应改为 `limit + max_limit + next_cursor`，不再返回裸 `resolved_height`。
- `src/btc/usdb-indexer/src/service/economic_cursor.rs`
  - 新增 UIP-0006 versioned opaque cursor codec；绑定 view version、完整 external state、resource/query 条件、limit 和确定性 continuation key。
  - 使用 base64url envelope 与 domain-separated SHA-256 checksum 检测损坏/schema drift；实现级 `max_limit` 固定为 500。
- `src/btc/usdb-indexer/src/service/server.rs`
  - 新增统一 economic query context resolver；即使调用方不传 context，也必须重建目标高度完整历史 identity。
  - historical protocol/formula 校验改为读取目标高度 identity，不再与当前进程常量比较。
  - 现有 candidate/breakdown 测试补齐历史 snapshot/local/system state 夹具，并覆盖 view/protocol/formula mismatch 和 external state 字段。
  - `get_pass_economic_profile` 对 non-invalid pass 复用 UIP-0003/0004/0005 派生层；invalid pass 从 history 合成 canonical 零值，不伪造 energy DB row。
  - derived energy snapshot 同步返回 `collab_breakdown_count`，profile 无需为计数重复扫描 collab 集合。
  - non-invalid pass 缺 raw energy 按内部状态损坏返回 `INTERNAL_INVARIANT_BROKEN`，不存在的 pass 返回 `PASS_NOT_FOUND`。
  - 服务层测试覆盖 active standard 多 collab 聚合与 breakdown 交叉重算、active collab/non-active/invalid 边界、view/formula mismatch、缺 pass/缺 energy、head 前进后的旧 external state 重放，以及 same-height anchor 替换后的 snapshot mismatch。
  - profile/candidate/breakdown 在业务派生前后重建并比较完整 historical state ref；查询期间发生 reorg 时返回对应 mismatch，不组合跨状态响应。
  - candidate/breakdown 改用 keyset cursor continuation；cursor 自带历史 context，current head 前进后续页仍固定首包 external state。
  - 服务层测试补充非法 limit、cursor 篡改、跨资源复用、查询条件变化、same-height reorg、旧分页字段拒绝和派生期间 state-ref 变化。
- `src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh`
  - validator payload / context 强制携带 API、semantics、protocol、formula 四类版本字段，删除 protocol fallback 和可空 `.get(...)` 兼容逻辑。
  - protocol mismatch 断言改为 `PROTOCOL_VERSION_MISMATCH(-32051)`。

### 已验证

- `cargo test -p usdb-util`
- `cargo test -p balance-history`
- `cargo test -p usdb-indexer`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 受影响 shell 脚本 `bash -n`
- `git diff --check`

本轮未启动 bitcoind/ord live regtest；按既定安排在 UIP-0006 其余 RPC 完成后集中执行和重构。

### 待继续对齐

- 补 formula mismatch 的 live/regtest 场景，并在全部 USDB indexer UIP 对齐后集中复核现有 live/ord 场景。
- 在大规模数据集上评估 `contribution_desc_pass_id_asc` continuation 的查询/索引成本。
