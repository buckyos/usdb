# balance-history-electrs-audit

这是一个独立、只读的离线审计工具，用固定 seed 从 balance-history SQLite snapshot
抽取样本，并通过 electrs 历史交易重放计算目标高度余额。

工具不会打开或修改 balance-history RocksDB，也不会修改 snapshot。它只会写入明确指定的
JSON report 和 checkpoint。

## Electrs 保护

electrs 默认 `index_lookup_limit = 0`，即不限制热门地址查询。审计前应在运行实例使用的
配置文件中设置非零值，例如：

```toml
# /data/.electrs/config.toml
index_lookup_limit = 20000
```

修改后重启 electrs。不要修改 electrs 源码中的
`internal/config_specification.toml`；该文件定义程序参数及默认值，不是运行实例配置。

工具默认要求传入 `--electrs-config` 并验证：

- `index_lookup_limit` 已设置且不为零；
- 服务端 limit 不大于客户端 `--max-history-entries`；
- 超限 RPC 被记录为 `skipped_too_popular`，不会重试。

Electrum RPC 不暴露运行进程的 `index_lookup_limit`。因此完成重启后还必须传入
`--confirm-electrs-restarted-with-config`；这是明确的操作员确认，避免“文件已改但进程未重启”
时误以为保护已经生效。

对无法读取配置的远程 electrs，可显式使用 `--allow-unverified-electrs-limit`，但这会失去
服务端 OOM 保护证明，不建议用于未知服务。

## 黑名单

黑名单每行可以是 Electrum script hash 或当前 snapshot 网络的 BTC 地址，支持 `#` 注释：

```text
# known high-volume exchange scripts
<64-char-electrum-script-hash>
bc1q...
```

黑名单在 RPC 前生效，但只能保护已知地址。未知热门地址仍依赖 electrs 的
`index_lookup_limit`。

## 运行

首次建议只跑 32 条 smoke：

```bash
cargo run --release -p balance-history-electrs-audit -- \
  --snapshot-db /data/.usdb/balance-history-snapshot-mainnet/builder/snapshots/000000963800/000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551/snapshot_963800.db \
  --electrs-config /data/.electrs/config.toml \
  --confirm-electrs-restarted-with-config \
  --electrs-url tcp://127.0.0.1:50001 \
  --sample-count 32 \
  --seed usdb-mainnet-963800-smoke-v1 \
  --output-dir /data/.usdb/audit
```

默认 75% 样本来自 snapshot 正余额终态，25% 来自 `script_registry` 中不存在正余额终态的
script。抽样使用 SHA256 keyspace probe，不使用 `ORDER BY RANDOM()` 或大 OFFSET；同一
snapshot、seed、样本数、黑名单会生成同一 run ID 和样本集合。

未指定 `--output` 时，报告文件名自动包含 snapshot 高度、样本数、可读 seed 片段和 run ID
前 12 位，例如：

```text
balance-history-electrs-audit-h963800-n32-usdb-mainnet-963800-smoke-v1-<run-id>.json
```

因此保持 `--output-dir` 和 `--seed` 不变、只修改 `--sample-count` 就会创建独立报告及
checkpoint。`--output` 仍可用于需要固定文件名的自动化，但显式固定路径不会自动改名；改变
运行身份时复用该路径下的旧 checkpoint 会继续 fail closed。

共享交易缓存通过 `--transaction-cache-mib` 设置，默认 256 MiB。缓存按交易序列化大小加
固定对象开销计权，不使用容易被超大交易绕过的纯 entry-count 上限。

报告 checkpoint 默认写到 `<resolved-output>.checkpoint.json`。中断后使用完全相同参数重新
运行即可继续，参数或黑名单变化会导致 run ID 不匹配并 fail closed。

报告中的 RPC 请求数和 cache 统计带有 `this_process`/`at_completion` 后缀；断点续跑时它们只
描述本次进程，不伪装成中断前后累计值。

## 结果状态

- `matched`：snapshot 余额与 electrs 截止目标高度重放结果一致。
- `mismatch`：余额不一致，命令失败。
- `skipped_too_popular`：服务端或客户端历史条数上限触发。
- `skipped_bip30_ambiguous`：同 txid 出现在不同高度，txid-only electrs 无法独立验证。
- `error`：RPC、交易引用、顺序或算术错误。

默认任何 skipped、mismatch 或 error 都导致非零退出。`--allow-skipped` 只允许两类明确的
skipped，不允许 mismatch/error。

`--verify-file-hash` 会完整读取 snapshot 并校验 manifest SHA256。对于数百 GB snapshot，
首次 smoke 可以复用已经完成的 snapshot 发布校验；正式审计报告建议至少执行一次该选项。
