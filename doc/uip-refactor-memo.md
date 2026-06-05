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

状态：第一轮 review 已开始，已先对齐 `prev` strict invalid 路径。

### 已对接内容

- `src/btc/usdb-indexer/src/index/pass.rs`
  - `on_mint_pass` 在写入任何状态前完整校验 `prev` 状态前置条件。
  - `prev` 缺失、owner 不一致、非 Dormant 或重复引用会将本次 mint 记录为 `Invalid`，不再 warn/skip。
  - 同 owner 当前 active pass 可在同一次 mint 中作为虚拟 Dormant `prev` 被原子消费；若同次 mint 还有其他 invalid `prev`，旧 active pass 保持原状态。
- `src/btc/usdb-indexer/src/index/test/pass_scenario.rs`
  - 更新 missing referenced `prev` 与重复继承已 consumed `prev` 的测试期望为 invalid mint。
- `src/btc/usdb-indexer/src/index/test/indexer_behavior.rs`
  - 更新 burned `prev` remint 和已 consumed `prev` 二次继承的 block-level 行为期望。
- `doc/UIP/UIP-0002-pass-state-machine.md`
  - 将测试要求中的 missing `prev` 表述明确为 missing referenced `prev`，避免与 UIP-0001 的 `prev` 缺省等价空数组冲突。

### 待继续对齐

- `leader_pass_id` collab mint 需要校验 Leader pass 存在、是 active standard pass，且不是本次 mint 创建的新 pass。
- burn 需要同步写入 energy 终态；当前 pass state 会转 `Burned`，但 energy 终态仍需继续对齐 UIP-0002 / UIP-0003。
- `Consumed` / `Burned` transfer 后是否继续更新 owner/satpoint 需要按非共识审计口径收敛。
- active pass 离开 Active 前的 block-level balance settlement 已有实现和测试，但还需要按 UIP-0002 逐项复核幂等和同高度多事件 replay。
