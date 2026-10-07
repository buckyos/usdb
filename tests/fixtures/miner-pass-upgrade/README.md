# MinerPass 多区间查询联调向量

`economic-queries.json` 来自根目录 `tests/miner_pass_upgrade_queries.rs` 中的完整索引与 RPC 查询测试。
它包含专用 regtest registry、13 个高度的 profile 和协作明细；不是公开网络配置。
Go 副本位于 `go-ethereum/internal/usdb/testdata/miner_pass_upgrade_queries.json`。

常规 Rust 测试逐字段比较当前输出与此文件，Go 测试消费对应副本，Go fast 的 golden gate
检查两文件逐字节相同。禁止单独更新某一份或只为消除断言失败而重写预期。

有意调整测试契约时，在 USDB 仓库运行：

```bash
USDB_WRITE_UPGRADE_QUERY_FIXTURE=/tmp/miner-pass-economic-queries.json \
  cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer \
  every_economic_surface_uses_the_query_height_rules_without_writing_energy
```

审查输出中的 registry、每个高度的派生数值、舍入与身份差异后，同步替换两份文件。
取消生成环境变量，再运行 Rust 查询矩阵，以及 Go `internal/usdb` 的普通构建与
`-tags usdb_miner_pass_conformance` 测试。额外公式只存在于隔离测试构建；不要将该 tag 用于发布包。

`live-catalog.json` 是 F 批真实服务验收的同源时间表：raw=160/180、effective=163、
level=166、schema=170、state=172，origin=1、stable lag=10。只有显式启用
`miner-pass-conformance` feature 的 regtest indexer 与对应 Go tag 可以执行它。
普通发布构建拒绝这些规则；不能将此 catalog 用于测试网或主网。

有意调整时间表时生成并审查两份产物：

```bash
USDB_WRITE_LIVE_UPGRADE_CATALOG=/tmp/live-catalog.json \
  cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer \
  live_service_catalog_is_explicit_and_matches_the_shared_contract
# 审查后替换此目录下的 live-catalog.json，再生成 Go 产物。
cargo run --manifest-path src/btc/Cargo.toml -p usdb-util \
  --bin generate_go_btc_activation_golden -- \
  --catalog tests/fixtures/miner-pass-upgrade/live-catalog.json \
  ../go-ethereum/internal/usdb/testdata/miner_pass_live_activation_golden.json
```

随后取消生成变量，运行 Rust 测试、Go 的 golden gate、普通/带 tag 的测试以及
`nightly miner-pass-upgrade`。具体隔离环境与验收范围见
[实施计划 F 批](../../../doc/usdb-indexer/miner-pass-upgrade-implementation-plan.md#12-f-批次真实服务与-ci-验收)。
