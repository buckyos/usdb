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

状态：公式层 helper、leader resolver、storage 查询、只读 effective energy resolver 和 `get_pass_energy` 三字段聚合已开始对接；breakdown、candidate set 和 leaderboard 待继续实现。

### 已对接内容

- `doc/UIP/UIP-0004-collab-leader-effective-energy.md`
  - 明确 `collab_contribution` 使用 UIP-0003 `energy_uint`，bps 乘除按整数 floor 计算并 saturate 到 `ENERGY_MAX`。
  - 明确 `raw_energy + Σ collab_contribution` 使用 `energy_uint` 饱和加法，超过 `ENERGY_MAX` 时 effective energy 固定为 `ENERGY_MAX`，且不得写回 raw energy ledger。
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
- `src/btc/usdb-indexer/src/service/rpc.rs`
  - 更新 `PassEnergySnapshot` 字段注释，移除 UIP-0004 未实现的旧说明。

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
- `get_pass_energy` active collab pass 的 `raw_energy` 保留、`effective_energy` 为 0。
- `get_pass_energy` non-active standard pass 的 `raw_energy` 保留、`effective_energy` 为 0。

### 待继续对齐

- candidate set / leaderboard 排除 collab pass 并按 standard effective energy 排序。
- collab breakdown 审计查询与 validator payload 三字段对齐。
