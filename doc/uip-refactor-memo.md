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

状态：UIP-0005 usdb-indexer core 暂时收尾。公式层 helper、纯单元测试、`get_pass_energy` 查询派生字段、candidate set view 派生字段、状态边界和服务层交叉验证均已对接；版本绑定和 validator / USDB-chain policy 闭环由 UIP-0006、UIP-0008、UIP-0009 承接。

### 已对接内容

- `src/btc/usdb-indexer/src/index/energy_formula.rs`
  - 新增 UIP-0005 level / difficulty 参数常量、整数阈值表和关键常量注释。
  - 新增 `calc_level_from_effective_energy(effective_energy)`，按整数阈值表计算 level，运行时不使用浮点、`log` 或 `pow`。
  - 新增 `calc_difficulty_factor_bps(level)`，按每级 100 bps 折扣计算并 clamp 到 `MIN_DIFFICULTY_FACTOR_BPS = 5000`。
  - 新增 `calc_real_difficulty(base_difficulty, difficulty_factor_bps)` 纯公式 helper，用于 USDB chain 侧 ceil 规则测试；usdb-indexer 不查询、不持久化 USDB-chain `base_difficulty` / `real_difficulty`。
- `src/btc/usdb-indexer/src/service/rpc.rs`
  - `PassEnergySnapshot` 增加 `level` 和 `difficulty_factor_bps`。
  - `CandidateSetViewItem` 增加 `level` 和 `difficulty_factor_bps`。
- `src/btc/usdb-indexer/src/service/server.rs`
  - `get_pass_energy` 从 UIP-0004 `effective_energy` 运行时派生 `level` / `difficulty_factor_bps`，不写入 energy DB。
  - `get_candidate_set_view` item 增加 `level` / `difficulty_factor_bps`，同样从每个 candidate 的 `effective_energy` 运行时派生；排序仍保持 `effective_energy DESC, pass_id ASC`。
  - 补充状态边界测试：active standard 使用 collab 聚合后的 `effective_energy` 计算 level；active collab、non-active standard、dormant / consumed / burned / invalid Leader 的 `effective_energy = 0` 时均返回 `level = 0`、`difficulty_factor_bps = 10000`。
  - 增加服务层交叉验证：同一 leader/collab 场景下，`get_pass_energy` 与 `get_candidate_set_view` 的 leader energy / level / factor 完全一致；collab 自身返回 `level = 0`、`difficulty_factor_bps = 10000` 且不进入 candidate set。
- `doc/usdb-indexer/usdb-indexer-rpc-v1.md`
  - 更新 `PassEnergySnapshot` 和 `get_pass_energy` 字段说明，明确 UIP-0005 派生字段不依赖 USDB-chain difficulty。
  - 更新 `get_candidate_set_view` 返回字段说明，明确 UIP-0005 字段不改变 candidate 排序口径。

### 当时后续项（当前状态）

- UIP-0006 economic state view、candidate/profile 版本字段和审计查询口径已完成，详见后续章节。
- UIP-0008：统一 formula version、历史高度 activation matrix 与参数版本重放。
- UIP-0009：明确 USDB-chain `base_difficulty` / `real_difficulty` 类型、来源和重算 policy。
- UIP-0005 / UIP-0006 已对齐，现进入集中 live/regtest 复核和大数据性能阶段。
- UIP-0014 的 quote activity / candidate energy 口径：quote stale 时 USDB chain 侧 candidate energy 回落为 raw/self energy；不反向修改 USDB indexer 的 effective energy。

## UIP-0006 USDB Economic State View

状态：任务 1（v1 查询/响应契约收敛）、任务 2（共享历史 context / version mismatch / external state 基础）、任务 3（单 Pass economic profile）和任务 4（state-ref 二次复验与 cursor 稳定分页）已对接。当前开发阶段不保留省略 `view_version`、旧 `page/page_size`、缺少 formula selector 或旧 protocol mismatch 名称的兼容入口。

### 已对接内容

- `doc/UIP/UIP-0006-usdb-economic-state-view.md`
  - 固定 `view_version = uip-0006-usdb-economic-state-view:v1`，所有 UIP-0006 请求顶层必填；该字段不进入 `ConsensusStateReference`。
  - 固定 `EconomicExternalState` 的 9 个必填字段，补齐 `balance_history_api_version`，并要求 protocol/formula 来自目标高度历史 identity。
  - 固定 invalid profile 的 raw/contribution/effective canonical 零值和 `level=0 / factor=10000` 查询语义，不要求伪造 energy DB row。
  - 固定 candidate/breakdown 的 `cursor + limit` 契约、cursor 绑定范围和错误边界；实现已直接替换数字分页，不保留双栈。
  - 明确 breakdown pass-id 排序以 canonical RPC inscription-id 文本为准，禁止使用内部 txid byte order。
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
  - collab breakdown 的 pass-id 排序改为 canonical RPC inscription-id 文本顺序，不再使用内部 txid 字节序；默认排序和 contribution tie-break 共用同一口径。
  - 服务层测试补充非法 limit、cursor 篡改、跨资源复用、查询条件变化、same-height reorg、旧分页字段拒绝和派生期间 state-ref 变化。
  - 增加非对称 txid 回归测试，明确覆盖 native `InscriptionId` 顺序与 canonical 文本顺序相反的情况。
- `src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh`
  - validator payload / context 强制携带 API、semantics、protocol、formula 四类版本字段，删除 protocol fallback 和可空 `.get(...)` 兼容逻辑。
  - protocol mismatch 断言改为 `PROTOCOL_VERSION_MISMATCH(-32051)`。
  - 新增 UIP-0006 profile、candidate set 和 collab breakdown 请求 helper；candidate/breakdown 自动遍历 opaque cursor，并校验分页期间 `external_state`、query 条件、`limit/max_limit`、total、去重和稳定排序不变。
  - collab breakdown 全量收集后按 `u128` 饱和规则独立重算 aggregate；candidate set 全量收集后独立检查 `effective_energy DESC, pass_id ASC`。
  - validator payload 成功校验统一交叉检查 `get_state_ref_at_height`、`get_pass_economic_profile`、完整 `get_candidate_set_view` 和完整 `get_collab_breakdown`；same-height reorg 失败校验要求四个入口返回同一 consensus mismatch。
  - candidate-set payload 改为直接使用 canonical candidate view 构造并强制 winner 为首项，不再逐个组合旧 snapshot/energy RPC；传入测试 pass 集合必须与服务端完整 candidate set 一致。
- `src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_{e2e,reorg}.sh`
  - 单 Pass validator payload 改为从 `get_pass_economic_profile` 构造，不再依赖旧 snapshot + energy 拼装路径。
  - happy-path 在原历史高度及 current head 前进后重复交叉验证 UIP-0006 economic views。
  - same-height reorg 在替换历史锚点后验证旧 payload 被 state-ref/profile/candidate/breakdown 四个入口统一拒绝。
- `src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_three_collab_breakdown.sh`
  - 新增 1 张 active standard Leader + 3 张 `leader_pass_id` collab pass 的真实 ord/live 场景，并为四个 owner 注入余额、累积正 raw energy。
  - 以 `limit=2` 分别遍历 `collab_pass_id_asc` 和 `contribution_desc_pass_id_asc` 两页 cursor，验证稳定排序、三项正 contribution、5000 bps 权重、集合一致和 aggregate 重算。
  - 交叉验证 Leader profile/candidate/payload 的 raw、collab contribution、effective energy、level/factor；验证 collab 自身 effective energy 为 0 且不进入 candidate set。
  - current head 前进后重放原历史 payload，验证 cursor 与完整 external state 仍冻结在原查询高度。
- `src/btc/usdb-util/src/constants.rs`
  - 集中定义 indexer API version、UIP-0006 view version、candidate selection rule、economic page max limit 和四个必需 feature，供 server/client/CLI/control-plane 共用，避免字符串漂移。
- `src/btc/usdb-indexer/src/service/{rpc,client,server}.rs`
  - `RpcInfo` 增加 `economic_state_view_version`、`candidate_set_selection_rule`、`economic_page_max_limit`，features 补齐 `historical_state_ref`。
  - typed Rust client 增加 `get_pass_economic_profile`、`get_candidate_set_view`、`get_collab_breakdown`；历史 `get_state_ref_at_height` 入口保持可用。
- `src/btc/usdb-indexer-cli/src/{cmd,usdb_indexer_service}.rs`
  - 新增 `state-ref`、`pass-economic-profile`、`candidate-set-view`、`collab-breakdown` 一等命令。
  - `--context` 按 `ConsensusQueryContext` 解析；cursor query 的 `--limit` 使用共享 `1..=max_limit` 校验，version/rule 默认值复用共享常量。
- `src/btc/usdb-control-plane/src/{models,rpc_client,server}.rs`
  - proxy allowlist 放行四个 historical/economic RPC，且不重写其 params。
  - indexer probe 并行读取 network/readiness/RpcInfo，在 service summary 暴露原始 capability metadata。
  - overview 增加 `usdb_economic_state_view` 能力判断；只有 service/API/view/rule/features/max-limit 均有效时才返回 `available=true`。
- `web/usdb-console-app/src/lib/types.ts`
  - 同步 control-plane service/capability 与 indexer RpcInfo 类型字段。

### 跨 UIP client surface 收口

- `src/btc/usdb-control-plane/src/{models,server}.rs`
  - mint prepare/execute 请求直接切换到 UIP-0001 v1 strict schema：standard 仅接受 `usdb_main`，collab 仅接受 `leader_pass_id` / `leader_btc_addr` 二选一，并拒绝未知字段。
  - `leader_btc_addr` 按当前 BTC network 校验；payload 固定写入整数 `v: 1`，不再生成旧协作地址字段。
  - prepare response 和 active-pass summary 增加 `pass_kind`、`mint_version`、Leader 绑定字段，不保留兼容响应。
  - 纯函数测试覆盖三种合法身份形态、字段冲突、BTC network mismatch、v1 payload 键和旧字段拒绝。
- `web/usdb-console-app/src/{lib,pages,i18n}`
  - mint 编辑器改为 standard/collab 与 fixed-pass/BTC-address 两级模式选择，并用互斥 TypeScript union 构造请求；session storage key 直接升级，不读取旧 mint 表单状态。
  - pass 类型同步 UIP-0001 字段；`PassEnergySnapshot` 同步 raw/collab/effective decimal string、level 和 factor。
  - raw energy range / leaderboard 继续按 UIP-0003 RPC 的 `items[].energy` 命名，但类型改为 decimal string；所有能量使用 `BigInt` 格式化，避免 `u128` 转 JavaScript `number` 丢失精度。
- `web/usdb-indexer-browser/src/main.tsx`
  - pass/owner/recent 类型和详情改为 `mint_version`、`pass_kind`、Leader 绑定字段。
  - 单 pass 能量详情改为 raw/collab/effective/level/factor，range / leaderboard 明确标注 raw energy 并按 decimal string 展示。
- `doc/矿工证铭文协议{,_en}.md`
  - 旧 issue 初稿不再维护第二套 schema，改为 UIP-0001 至 UIP-0006 的入口摘要，并明确 UIP 文本是唯一规范来源。
- `doc/usdb-btc-ord-roles-and-mint-flow.md`
  - control-plane mint 流程说明同步 standard/collab 选择和 UIP-0001 字段互斥校验。

### 本轮补充测试

- CLI candidate 默认 view/rule/limit 和非法 limit 拒绝。
- CLI `ConsensusQueryContext` JSON object 解析与非 object 拒绝。
- control-plane 四个 UIP-0006 RPC allowlist/params 原样转发。
- control-plane capability 完整声明通过，缺少 feature、错误 view version 或错误 selection rule 时 fail closed。
- indexer `RpcInfo` 的 API/view/rule/max-limit 和四个必需 feature 与 profile/breakdown 服务测试交叉验证。
- control-plane strict mint schema / payload 定向测试和 Rust workspace 全量测试。
- console 与独立 indexer browser 的 TypeScript check 和 production build。

### 已验证

- `cargo fmt --all`
- `cargo test --workspace`
- `cargo test -p usdb-indexer-cli -p usdb-control-plane`
- `cargo test -p usdb-util`
- `cargo test -p balance-history`
- `cargo test -p usdb-indexer`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `npm run build`（`web/usdb-console-app`）
- `npm run check`（`web/usdb-console-app`、`web/usdb-indexer-browser`）
- `npm run build`（`web/usdb-indexer-browser`）
- `cargo test -p usdb-indexer test_collab_breakdown_sort_uses_canonical_pass_id_text_order -- --nocapture`
- 全部 57 个 shell 脚本 `bash -n`
- `git diff --check`
- `bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_e2e.sh`
- `bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_reorg.sh`
- `bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_three_pass_candidate_set.sh`
- `bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_three_collab_breakdown.sh`

上述 targeted live smoke 均使用独立 regtest bitcoind/ord/balance-history/usdb-indexer 数据目录和端口执行成功；后续完整确定性矩阵执行结果见“UIP-0001 至 UIP-0006 确定性 Live/Regtest 矩阵”。

### 待继续对齐

- 在大规模数据集上评估 `contribution_desc_pass_id_asc` continuation 的查询/索引成本。
- 执行 300-block 随机 world-sim、扩大 candidate set 规模并进行长时 soak；这些非确定性/性能项目不计入本轮协议矩阵完成结论。

## UIP-0001 至 UIP-0006 测试入口与文档状态收口

### 测试入口

- `scripts/run_regression.sh` 更新两个已改名的 UIP-0002/UIP-0003 Rust 状态机测试，避免 core protocol suite 在执行场景前因 test filter 失效。
- `scripts/run_regression.sh` 显式通过 `bash` 调用 reorg 总入口，避免 `run_reorg_regression.sh` 的文件 mode 导致完整 suite 无法启动。
- `regtest_live_ord_e2e.sh` 的 transfer/remint 场景改为断言旧 prev `Consumed / raw_energy = 0`，并独立验证新 pass raw energy 为 `floor(prev_raw * 9500 / 10000)`。
- `regtest_live_ord_e2e.sh` 和 `regtest_world_sim.sh` 的默认 `ORD_BIN` 与其余 live/reorg 入口统一为当前仓库使用的 release ord 路径，避免总 runner 依赖调用者额外修改 `PATH`。
- scenario runner 新增通用非负整数 `assert_mul_div_floor`，供 live 场景表达协议 rounding，而不使用浮点数。
- world-sim validator sample 改从 UIP-0006 `get_candidate_set_view` 获取 canonical active standard candidate，延迟回放使用 `get_pass_economic_profile` 交叉验证，不再从全部 active pass 手工拼装 candidate。
- `regtest_world_sim.sh` 显式向 simulator 传递临时 regtest 节点的 cookie 文件，修复 wrapper 已启用 cookie auth、Python 入口却缺少认证参数而无法启动的问题。
- world-sim context 固定 API/semantics/protocol/formula version，并记录 `pass_kind`、level、factor；recovery state 直接升级为 v2，不兼容旧采样文件。
- world-sim 独立 energy oracle 从旧 u64/sat 级增长切换到 UIP-0003 unit、age penalty 和 `u128` 饱和算法。
- 新增 formula-version mismatch live wrapper，要求 state ref/profile/candidate/breakdown 统一返回 `FORMULA_VERSION_MISMATCH (-32052)`。
- `run_reorg_regression.sh` 纳入 formula-version mismatch 和已有 three-collab breakdown 场景。
- 修正后的 transfer/remint live smoke 已通过真实 ord 链路，验证 exact owner script、prev `Consumed / 0` 和 child 95% inherit rounding；新增 formula-version mismatch live smoke 也已验证四个 UIP-0006 view 统一返回 `FORMULA_VERSION_MISMATCH (-32052)`。
- 缩短版 candidate-set world-sim 已通过真实服务链路：canonical candidate capture、profile 交叉验证、delayed replay、tamper detection、global cross-check 和 UIP-0003 oracle self-check 均为零失败；完整 300-block/reorg 压测仍留在集中测试阶段。

### 文档状态

- UIP-0001 至 UIP-0006 保持 `Draft`，但实现状态、后续依赖和测试说明已按当前代码更新。
- UIP-0001/UIP-0002 移除已经完成的 parser/state-machine 未决项，只保留 UIP-0008 activation 和可选审计 API。
- UIP-0003/UIP-0004/UIP-0005/UIP-0006 移除“后续再实现 UIP-0004/0005/0006”等过期描述，明确 BTC/indexer core 已完成及 USDB-chain/UIP-0008/UIP-0009 边界。
- economic issue tracker 将 ECO-002 至 ECO-007、ECO-012 标为 `Done`；ECO-008/ECO-011 保持 `In Progress`，并明确只剩 USDB-chain policy/payload 或集中测试与性能部分。

## UIP-0001 至 UIP-0006 确定性 Live/Regtest 矩阵

状态：已完成。共执行 58 个确定性入口，包括 7 个 Rust core protocol test filter 和 51 个独立 shell/live-regtest 场景；所有入口最终均通过。

### 执行范围

- core protocol：7 个 UIP-0001/UIP-0002/UIP-0003 parser、状态机和公式测试。
- 基础 scenario：4 个非 ord smoke，以及 5 个真实 ord mint/transfer/remint/invalid/duplicate-prev 场景。
- reorg/recovery：6 个 smoke reorg、3 个真实 ord reorg、2 个 pending recovery 场景。
- historical validation：4 个 state-ref、retention floor、history gap 和 validator historical-context 场景。
- validator block-body：27 个 happy-path、state advance、multi-pass/collab、tamper、reorg、版本矩阵、payload upgrade、restart/not-ready/crash-recovery 场景。

### 矩阵发现与修正

- `balance-history` JSON-RPC client 现在区分“缺少 result 字段”和合法 `result: null`；reorg 后查询已回滚 block commit 时，`Option<T>` 可正确解码为空，不再中止 indexer 扫块。新增 present-null 和 missing-result 单元测试。
- duplicate-prev live 场景修正 JSON 断言传参方式，并按 UIP-0002 收敛预期：首次 child 保持 Active，第二次复用已 Consumed prev 的 mint 为 `Invalid / INVALID_PREV_ID`，且 invalid pass 不生成 energy row。
- transfer/remint reorg 场景显式把 child mint 到 prev 当前 owner 的同一 script，满足 exact owner 约束；旧 prev 断言统一为 `Consumed / raw_energy=0`。
- multi-block reorg 场景移除“重复 prev 可再次继承”的旧预期，改为校验第二 child invalid、leaderboard 仅包含有效 Active pass，并覆盖 rollback 后 invalid pass 移除。
- energy-state helper 增加可选 raw energy 断言，用于交叉验证 Consumed 终态固定为 0。
- validator version-matrix 在 head advance 后改为等待实际新高度 `current_tip_height + 1`；避免仍等待旧高度而提前通过，随后在 `CatchingUp` 窗口误收到 `SNAPSHOT_NOT_READY`。

### 环境说明

- validator aggregate runner 在 protocol-version mismatch slot 首次遇到一个残留 ord 进程占用端口；确认非协议失败后，使用空闲独立端口重跑该项，并按原顺序完成其余场景。
- 本轮没有运行完整 300-block 随机 world-sim 或长时 soak；这两项属于后续规模/稳定性评估，不影响 UIP-0001 至 UIP-0006 确定性协议矩阵结论。

## UIP-0001 至 UIP-0006 Reorg/Cursor/Historical Context 集中复核

状态：已完成。2026-07-18 在确定性矩阵基础上聚焦重跑 80 个 service 测试和 19 个独立 live/regtest 场景，全部通过；本轮未发现新的协议或实现偏差。

### 执行范围与结论

- service 层 80 个测试全部通过，覆盖 cursor 篡改/绑定变化/same-height reorg、旧 external state 回放、派生期间二次 state-ref 校验，以及 profile/candidate/breakdown 的历史查询。
- 11 个 reorg/restart/recovery 场景全部通过：height-regression、same-height、三轮 multi-reorg、hybrid reorg、三类真实 ord transfer/remint 回滚，以及 energy/transfer reload 故障注入恢复。
- pending recovery marker 在故障后可持久化、自动重试清理，并可跨 usdb-indexer 进程重启继续恢复；未来高度的 pass commit 与 balance snapshot 均被清理。
- 4 个 historical context 场景全部通过：head advance 后旧 context 可重放；历史锚点被替换后返回 `SNAPSHOT_ID_MISMATCH (-32042)`；提高 retention floor 后返回 `STATE_NOT_RETAINED (-32048)`；保留窗口内辅助历史缺失时返回 `HISTORY_NOT_AVAILABLE (-32049)`。
- candidate set 和 collab breakdown 均以 `limit=2` 遍历 3 项、强制跨两页 opaque cursor；排序、去重、total、external state 冻结和 collab aggregate 独立复算全部通过，head advance 后仍可重放原历史结果。
- validator payload restart consistency 通过；payload version upgrade restart 同时验证两个历史高度的 payload 在服务重启后仍保持各自 snapshot/system-state identity 并可重放。

### 后续边界

- 本轮是确定性功能复核，没有重复执行完整 300-block 随机 world-sim、扩大 candidate set 的性能评估或长时 soak；这些继续作为规模与稳定性阶段任务。

## World-Sim UIP-0001 至 UIP-0006 经济机制扩展

状态：核心实现和聚焦 live smoke 已完成；未执行 300-block 随机 soak。

### 改动事项

- world-sim 动作模型由旧 `mint/remint` 拆为 `standard`、`fixed collab`、`address collab` 三类 mint 和三类 remint；payload 直接使用 UIP-0001 v1 严格字段，不保留旧 action/参数兼容入口。
- remint prev 收敛为 actor-owned `Active / Dormant` pass，transfer 同样排除 terminal pass；collab 动作会锁定本次使用的 active standard Leader。
- recovery state 升级为 v3，持久化 pass kind 与原始 Leader 引用；reorg rebuild 从 canonical pass snapshot 重建这些身份字段。
- 全局检查新增 UIP-0004 至 UIP-0006 独立审计：candidate 必须等于 active standard 集合，active collab 必须被排除；fixed/address Leader 在目标高度独立解析；两种 breakdown 排序均完整遍历 cursor，并从 raw energy 重算 contribution、饱和 aggregate、effective energy 与 profile count。
- validator historical replay 在原 candidate/profile 对比基础上增加 breakdown aggregate 重算，并校验历史 external state 冻结。
- 新增 deterministic economic bootstrap，覆盖 Leader remint 后 fixed 不跟随/address 自动跟随，以及 standard/fixed/address remint 的 `Consumed / raw=0` 和新 pass raw-only inheritance 路径。
- 新增纯 Python 测试与 `regtest_world_sim_economic_views.sh` 聚焦入口；后者以 `limit=2` 强制 cursor continuation，并从 JSONL 报告检查必需动作和零失败指标。

### 已验证

- `python3 src/btc/usdb-indexer/scripts/test_regtest_world_simulator.py`：6 个纯测试通过。
- 新增/调整 shell 脚本全部通过 `bash -n`，Python 入口通过 `py_compile`，`git diff --check` 通过。
- 独立 regtest smoke 于 2026-07-18 通过：bootstrap 完成 8 个业务步骤和 1 个 energy-growth block，随后运行 3 个 tick；最终 2 张 active standard、2 张 active collab，完成 2 次 historical candidate/profile/breakdown replay。
- smoke 最终指标：`verify_fail=0`、`global_cross_check_fail=0`、`validator_sample_fail=0`、`global_cross_check_ok=11`、`validator_sample_ok=2`。
- 同入口追加 `depth=3` deterministic reorg 组合 smoke 通过：回滚区间覆盖最终 address-collab remint，本地 pass identity/ownership 从 canonical history 重建后继续收敛；最终 `reorg_ok=1`、`validator_sample_ok=3`，三类 fail 指标仍为 0。

### 后续边界

- 继续保留 300-block 随机 world-sim 和更长 reorg 组合作为后续 soak；candidate/breakdown 首轮大数据量评估见下一节，后续只保留 cold cache、并发和更高容量点。

## UIP-0004 至 UIP-0006 Economic View 规模评估

状态：首轮 `100 / 1K / 10K` deterministic release 矩阵已完成；改动尚未提交，等待 review。

### 实现与测试入口

- 新增 ignored release scale test 和 `run_economic_scale_eval.sh`，每档构造等量 active standard/collab；collab 一半固定 Leader、一半 address Leader，并集中到同一 Leader 形成 breakdown 大集合。
- 使用真实 SQLite/RocksDB，记录 latency、RSS/VmHWM、DB size、SQLite statement/result-row/VM-step/fullscan/sort，以及 RocksDB get/seek/decode 逻辑操作。
- 硬断言 candidate 排除 collab、两种 breakdown 排序集合/aggregate 一致、profile/candidate/breakdown 交叉一致、冻结 external state 重放和重启重放摘要一致。
- 原始 JSON 默认写入 `src/btc/target/economic-scale/`；详细方法和结果见 `doc/usdb-indexer/usdb-indexer-economic-view-scale-evaluation.md`。

### 查询路径修正

- candidate 派生改为一次加载 active standard/collab 历史 snapshot，并建立 `leader_pass_id` / `leader_btc_owner` map；每个 pass 只读取一次 raw energy。
- address Leader 在批量路径继续按当前 BTC network 规范化，并校验存储的 owner relation，避免性能优化绕过 UIP-0004 network 规则。
- breakdown 直接信任按 relation 索引筛出的 active collab 行，并执行字段 invariant 校验，不再逐 collab 重复解析 Leader。
- candidate 和按 sort 排好的 breakdown 增加最多 2 项的有界缓存；key 包含完整 `EconomicExternalState` 及 resource 参数，只在派生后 state-ref 二次校验通过时写入。
- continuation page 仍执行完整 external-state 前后校验；same-height reorg 会因 state identity 变化 fail closed，不复用旧 cache。

### 结果摘要

- 1K 原始 candidate 全分页约 `16.16s / 40,288 SQLite reads / 20K RocksDB seeks`；优化后约 `13.02ms / 270 statements / 2K seeks`。
- 10K standard + 10K collab、`limit=100`：candidate 首次全分页约 `323.55ms`、cache replay `23.29ms`；breakdown 首次约 `247.51ms`、cache replay `147.04ms`；profile `95.45ms`；peak RSS 约 `50.94 MiB`，fixture DB 约 `27.96 MiB`。
- 10K 的 `limit=20/100/500` 均通过相同 digest/aggregate；页数越多，逐页 external-state 校验成本按页数增长，但 RocksDB cache replay 读取保持为 0。

### 后续边界

- 仍需 cold cache/物理 I/O、并发与 cache eviction、collab 多 Leader 分布、100K 以上容量点和长时 soak。
- 这些属于容量与运维评估，不阻塞 UIP-0001 至 UIP-0006 当前协议行为对齐。

## 跨 UIP 术语收口

状态：已完成文档和 parser 对齐，改动尚未提交，等待 review。

### 规范组织

- `doc/UIP/README.md` 增加非规范性的跨 UIP 术语索引，只负责把术语指向唯一的定义 UIP。
- UIP-0000 增加术语所有权规则：下游 UIP 必须引用定义 UIP；增加过滤、状态或计算层时必须使用新的限定名称，禁止用同名词覆盖原语义。
- `uip-split-design.md` 明确为非规范路线图，所有规划性简写均服从 README 索引和正式 UIP。

### 关键边界

- UIP-0001 固定 `pass_id` canonical encoding、`pass_kind`、`owner_script_hash` / `owner_btc_addr`；所有 lexical ordering 均按 canonical ASCII 文本。
- UIP-0004 区分 `leader_ref`、`resolved_leader` 和 USDB-chain `leader_eligible`，并把 `effective_energy` 固定为 BTC-side nominal 派生值。
- UIP-0006 将 `candidate_pass` 定义为同一 `external_state` 下的 Active standard pass，成员资格与 energy 是否为零无关；`candidate_set_view` 只提供确定性审计排序。
- 排序第一项统一称为 `top_ranked_candidate`；UIP-0007 区块头显式携带的是 `selected_pass`，两者不要求相同，也都不能简写成 PoW `winner`。
- UIP-0014 的 `candidate_energy`、`candidate_level` 和 `candidate_difficulty_factor_bps` 是 USDB-chain policy 对 `selected_pass` 的实际输入，不回写 UIP-0004 nominal 字段，也不改变 UIP-0006 集合成员资格。
- UIP-0007 `ProfileSelectorPayload` 是链上最小二进制 selector；validator block-body JSON 明确称为链外 `validator test envelope`，不再把完整审计字段误称为链上 payload。

### 实现与测试

- mint parser 对 `leader_pass_id` 和 `prev[]` 执行 canonical inscription id 校验，拒绝 uppercase txid、index 前导零等文本别名；开发期不保留兼容解析。
- duplicate `prev` 在解析后的 pass identity 上判断，storage/RPC 继续只接收和输出 canonical `pass_id`。
- 增加 non-canonical `leader_pass_id` 与 `prev` invalid 单元测试。
- RPC 注释和 v1 文档明确 `selection_rule` 只是 `candidate_set_view` ordering contract；raw energy leaderboard 不是 candidate view。

## UIP-0007 Profile Selector 与 Economic Profile 基础对齐

状态：前三项基础重构已完成，尚未提交，等待 review；chain config 激活和最终 reward/difficulty policy 不在本批范围。

### Wire Contract

- go-ethereum 删除旧 105-byte `RewardPayloadV1`，直接替换为 UIP-0007 107-byte `ProfileSelectorPayload`，加入 big-endian `difficulty_policy_version` 并按固定 offset 编解码，不保留旧格式兼容入口。
- `pass_id`、`snapshot_id` 和 `system_state_id` 文本入口改为严格 canonical 校验；拒绝 `0x`、uppercase txid/hash、非零 inscription index 前导零和其它文本别名。
- 新增独立 golden binary vector、精确 size/version、canonical parser 和 roundtrip 测试，避免仅靠自洽 roundtrip 掩盖 offset 或 endian 错误。

### UIP-0006 Profile Client

- Go client 删除 `get_pass_snapshot + get_pass_energy` 组合接口，统一调用冻结版本 `get_pass_economic_profile`。
- client 类型完整承接 `view_version`、`external_state`、pass state/kind、三种 decimal-string energy、level/factor 和 breakdown count；新增无网络 fake transport 测试，直接校验 RPC 方法名、参数对象和当前返回结构。

### Resolved Consensus Profile

- 新增 `ResolvedConsensusProfile`，miner 和 validator 共用同一只读解析/校验路径。
- 校验 payload 与 profile 的 BTC height、snapshot id、system state id、pass id、view version 和完整 external-state identity；`selected_pass` 必须为 `Active / standard`，不要求是 `top_ranked_candidate`。
- 三种 energy 按 canonical `uint128` decimal 解析；Go 端重算 `raw + collab contribution` 的 u128 饱和值，并使用 UIP-0005 固定阈值表重算 level 和 difficulty factor，任何不一致均 fail closed。
- 现有 development reward adapter 改为只消费已验证 profile；最终 reward 和 real difficulty 公式继续由 UIP-0009/UIP-0011/UIP-0014 承接。

### 已验证与后续边界

- `go test -cover ./internal/usdb` 通过，statement coverage 为 `78.9%`；`go test ./miner`、USDB ethash 定向测试和 `go vet ./internal/usdb ./miner ./consensus/ethash` 通过。
- 完整 `go test -count=1 ./consensus/ethash` 在允许临时 loopback `httptest` listener 的环境下通过；沙箱内的首次阻断仅来自 listener 权限限制。
- 后续仍需：由 UIP-0008/UIP-0009 chain config 提供按 USDB block height 的 expected versions；在 header validation 中执行精确 107-byte/version 校验；移除本地开关决定共识语义；更新旧 105-byte reward live/E2E 脚本和集成文档。

## UIP-0007 Chain Config、Header Validation 与 CLI 边界

状态：UIP-0007 chain-config/CLI、UIP-0008 初始 activation ownership 和 USDB 命名批次已提交；本节保留实现边界记录。

### Chain Config 与 Miner

- `params.ChainConfig.usdb` 已由固定两个字段替换为按 USDB block 严格排序的 `activations[]`；每条记录携带 UIP-0008/UIP-0009 完整 version set。
- `USDBConsensusAt(usdb_block)` 返回目标高度最近生效版本；首次激活前返回 inactive，激活边界及之后按对应完整版本集合解释。
- registry 增加空记录、乱序/同高冲突、必需版本和 reward 依赖校验；`CheckCompatible` 只允许修改尚未到达的未来激活，已生效差异返回最早安全回退高度。
- built-in development chain 从 genesis 激活 payload/difficulty v1；UIP-0011 至 UIP-0015 尚未实现的版本字段显式为 `0`，非零未知 policy fail closed。
- payload builder 删除构造期固定 policy version，`BuildCurrentPayload(ctx, blockNumber)` 从 chain config 取得 expected version；不支持的 payload version、无效共识配置、current system state/profile 不可用或 selected pass 非 candidate 均停止组块。
- worker 在 USDB chain 上即使调用方请求 `noExtra` 也必须生成 selector；active chain 缺少 builder 或初始化失败时禁止回退到静态/空 extra-data。

### Validator 与运行参数

- `VerifyHeader` 先执行精确 107 bytes 与 expected version 的纯二进制 guard，再解析 historical profile 并按 UIP-0005 `ceil(base_difficulty * factor / 10000)` 重算 difficulty；畸形字段不会触发 RPC。
- miner `Prepare` 使用相同 chain-config policy 和 profile resolver 计算 difficulty；miner 生成、validator 验证的交叉测试会拒绝未折算的 base difficulty。
- reward state transition 重复 binary guard 并消费同一 selector-bound profile；旧 `0.5..2.0` level multiplier 和 mock reward policy 已删除，UIP-0011 未激活前保留既有 Ethash 静态奖励。
- BTC-side `usdb-indexer` 服务不可用、historical state 不可保留、same-height state replacement 或 profile 字段不一致时均 fail closed；非 USDB chain 继续使用 legacy 行为。
- 删除 legacy `--ethash.usdb`、`--miner.usdb` 及两个 runtime `Enabled` 字段。CLI 只保留显式的 `--ethash.usdb-indexer.*` / `--miner.usdb-indexer.*` 访问参数、`--miner.usdb.passid` 和 query timeout，不能启用、关闭或改写共识版本。

### 测试与后续

- 新增完整 activation/version lookup、`CheckCompatible`、genesis JSON roundtrip、RPC error mapping、candidate boundary、codec、historical replay、same-height replacement、服务不可用、selector/profile 篡改和 miner/validator 交叉测试。
- `params`、`internal/usdb`、`miner`、`core`、`consensus/ethash`、`cmd/geth` 全包测试及受影响包 `go vet` 已通过。当前环境为 Go 1.26，旧 `fjl/memsize` linkname 检查仍需对 CLI 测试使用 `-ldflags=-checklinkname=0`。
- 三条 live runner 已统一迁移到 107-byte `ProfileSelectorPayload`、UIP-0001 `v=1/usdb_main` mint 和 `get_pass_economic_profile`。basic、energy growth（含 factor `9900`）和 BTC head 前进后的 fresh-validator historical replay 均已实跑通过。

## UIP-0008 Activation Registry 与 State Identity

状态：机器可读 registry、canonical identity、Rust 历史解析链路和初始 Go codec 已提交；Go 跨语言 registry binding/height lookup 批次已实现，尚未提交，等待 review。

### Registry Contract

- 原先同时包含 BTC/USDB-chain 和多个 network 的全局 `activation-registry.json` 已拆分为 `activation-registry/btc-mainnet.json` 与 `btc-regtest.json`；每个 artifact 在顶层固定一个 BTC network scope，record 只允许 BTC-side family 和 `activation_height`。
- Rust API 改为先按配置的 `bitcoin::Network` 选择唯一内嵌 registry，再按目标高度 lookup；testnet3/testnet4/signet 在没有独立 artifact 时 fail closed，不能回退或混用其他 network。
- `activation_registry_id` 改为 network-scoped canonical encoding，固定 golden：`btc-mainnet=bb751626...a7d39e`、`btc-regtest=22d820e6...aaf83d`；两者当前 active set 相同，继续使用跨 Rust/Go golden set ID `01d1d45f...f94691`。
- Rust registry 类型不再表达 USDB-chain activation record，并显式拒绝 USDB-chain family；USDB expected versions 只由 go-ethereum `ChainConfig.usdb.activations[]` 按 USDB block number 解析。`ChainConfig.usdb.btcActivationRegistryId` 仅绑定可接受的 BTC historical-profile registry identity。
- audit-only `release-manifest.json` 关联 BTC registry IDs 与 USDB `chain_id/genesis_hash/chain-config authority/btc registry binding`；manifest 自身不参与 BTC identity、USDB header validation 或 runtime activation lookup。
- 当前 `btc-mainnet` height 0 只定义 BTC source history 的 v1 解释，不代表 USDB public mainnet 已发布；正式网络的 indexing origin 和 USDB genesis 仍需分别冻结。

### State Identity Boundary

- `ConsensusSnapshotIdentity` 收敛为纯 balance-history identity，不再承诺 USDB indexer protocol/formula；balance-history RPC/state ref 也不再声明下游 activation identity。
- indexer 的 `LocalStateCommitIdentity` 增加 `commit_protocol_version` 和 `active_version_set_id`，使 `local_state_commit` 与 `system_state_id` 承诺目标高度实际生效的完整版本集合。
- UIP-0006 external state、economic cursor、profile、candidate 和 breakdown 统一暴露并冻结 `activation_registry_id + active_version_set + active_version_set_id`；不保留旧全局 protocol/formula 字段双栈。

### Runtime Validation

- indexer 启动时同时校验配置 genesis height 和 DB durable synced height 的 registry/v1 支持面，每次 `sync_block` 在写入前再按该 BTC 高度解析；未知、缺失、冲突、不支持版本以及降级二进制跨 activation 重启均不会产生部分状态。
- balance-history 在 indexer/RPC 启动时校验当前 durable height，并在每个 batch 写入前逐高度校验 semantics activation；历史 state-ref lookup 将 activation 失败映射为结构化共识错误。
- historical RPC 以请求冻结高度解析 active set，重算 set ID 后再构造 local/system state；expected state 与 cursor 对 registry/set identity 做精确匹配。
- 新增 activation record、supported version、active set identity 和 commit protocol 的结构化 RPC 错误码；formula member 不支持继续映射到专用 formula mismatch。旧 `PROTOCOL_VERSION_MISMATCH/-32051` 随全局 protocol 字段删除，不保留 client 兼容映射。
- Rust 新增确定性 `generate_go_btc_activation_golden` 工具，将两个 registry 的 active 高度边界展开为 Go 内嵌 artifact；`--check` 可以在 CI/release 中直接拒绝跨仓库 artifact 漂移。
- `ChainConfig.usdb.btcActivationRegistryId` 进入 genesis JSON 与 `CheckCompatible`；built-in development chain 固定绑定 `btc-regtest` registry。Go builder/verifier 对未知 binding 启动失败，并使用 `binding + payload BTC height` 解析本地 expected set。
- historical profile query 现在主动携带 expected registry/set ID；返回值必须同时通过 chain-config registry equality、canonical set ID 重算及本地 height lookup。builder 继续检查 current state 与 historical profile 两次读取之间的 identity 漂移。
- verifier 从 expected active set 解析 `energy_formula_version`、`effective_energy_formula_version`、`level_formula_version` 后分别分派公式；当前只实现 v1，任何未知版本均 fail closed，而不是隐式复用最新公式。

### 已验证与后续边界

- Go 聚焦测试已覆盖 generated registry/set golden、payload-height boundary lookup、unknown/tampered registry、chain-config binding compatibility、historical query pinning和未知 formula dispatch；`internal/usdb`、`params`、`consensus/ethash`、`miner`、`core` 当前测试通过。

## USDB Chain 命名与系统边界收口

状态：协议、实现和开发入口已按当前链身份收敛；改动尚未提交，等待 review。

### 统一口径

- `USDB protocol` 表示横跨 BTC 铭文、BTC-side 派生状态和 USDB chain 共识的协议族；具体对象不得仅依赖裸 `USDB` 推断所属系统。
- 当前 EVM-compatible PoW 链统一称为 `USDB chain`；其 block、validator、miner、reward、difficulty、genesis、chain config 和 activation anchor 使用 USDB 命名。
- BTC 索引服务统一称为 `BTC-side USDB services/state view`；USDB chain 的 EVM 地址称为 `USDB-chain account address`，BTC owner 继续使用 `owner_btc_addr` / `owner_script_hash`。
- 原生资产在余额、发行和手续费语境中称为 `USDB native currency` 或 `USDB atom`，避免与协议族和链实现混淆。
- `ETHW` 只保留在上游 fork 历史、继承专名/配置（例如 `Ethash`、`EthPoWForkBlock`、`ETHWStartDifficulty`）以及仍实际存在的运维标识（例如 `ethw-init`、`ethw-node`、`ETHW_COMMAND`）。

### 实现与文档

- UIP-0007/UIP-0009 文件名、标题、激活锚点和 network id 改为 USDB chain；release manifest 使用 `usdb_chain_configs`，USDB activation 继续只由本地 genesis/chain config 决定，不依赖 BTC RPC。
- BTC Electrum-compatible script hash 类型从易歧义的 `USDBScriptHash` 改为 `BtcScriptHash`；字节/数据库编码不变，RPC 文档明确其为反转字节序的 `SHA-256(scriptPubKey)`。
- control-plane 链 RPC 配置使用 `usdb_chain_url`，链账户接口使用 `/api/usdb-chain/...` 和 `UsdbChain*` 类型；console 展示区分 USDB 链账户地址、BTC owner 与 USDB 原生资产余额，原生最小单位字段使用 `balance_atoms_hex`。Docker 生成的 TOML 和保留的静态 console 基线同步使用新 schema，不保留 `ethw_*` API 字段。
- 消除裸 `USDB_RPC_*` 歧义：BTC-side 测试框架和 world simulator 改为 `USDB_INDEXER_RPC_PORT` / `USDB_INDEXER_RPC_URL` / `--usdb-indexer-rpc-url`，USDB chain 节点使用 `USDB_CHAIN_RPC_URL`。geth 访问 indexer 的运行参数改为 `--miner.usdb-indexer.*` / `--ethash.usdb-indexer.*`，而 `--miner.usdb.passid` 仍表示 USDB protocol selector。
- world-sim/geth 中原先可能与 BTC owner 混淆的 `USDB_MINER_ADDRESS` 改为 `USDB_CHAIN_MINER_ADDRESS`；对应 CLI、identity marker 和 alignment 字段统一使用 `usdb_chain_miner_*`。这些值始终表示 USDB-chain account/coinbase，不表示 BTC pass owner。
- go-ethereum 的新增集成文档和 E2E 入口从 `usdb-ethw-*` / `usdb_ethw_*` 改为 `usdb-*` / `usdb_profile_*`；JSON-RPC `eth_*`、`Ethash` 和底层 geth 运维名称继续保留。
- 本批为开发期直接 schema/API 改名，不保留旧字段、旧 route、旧脚本名或兼容双栈。
- `balance-history` 排除手工 snapshot fixture 后为 101 passed、0 failed；未过滤执行时唯一失败仍是既有 `db::snapshot::tests::test_load` 依赖本机预置 height 900000 snapshot，与本批 identity 改动无关。
- `cargo fmt --check`、`usdb-control-plane` test/clippy 和 `usdb-util` activation/hash test 通过；Go `params`、`internal/usdb`、`consensus/ethash`、`miner`、`cmd/utils`、`cmd/geth`、`eth/ethconfig` 测试与 vet 通过，Rust/Go canonical active-set golden vector 一致。console type-check/build、Python compile/help、regtest shell syntax和 dev/bootstrap Compose config 均已复核。
- 正式网络的具体 activation 高度、签名/发布流程以及未来 v2 dispatch 仍需随 UIP-0008/UIP-0009 冻结；开发期数据库不做迁移兼容，测试时删除旧库重建。
