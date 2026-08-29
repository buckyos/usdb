# USDB 进程日志配置与运维

本文说明 `src/btc` Rust workspace 的统一进程日志基础设施。该机制覆盖
`balance-history`、`usdb-indexer`、control-plane、snapshot tool 和配套 CLI，
其中前两个索引服务是生产排障的重点对象。

## 默认行为

守护进程默认写入各自 service root 下的 `logs/`：

- `balance-history`: `${root_dir}/logs/balance-history_rCURRENT.log`
- `usdb-indexer`: `${root_dir}/logs/usdb-indexer_rCURRENT.log`
- 子命令和工具可能使用独立 basename，例如 `balance-history_snapshot`
- CLI 默认只输出到 stderr，不创建日志文件

默认参数如下：

| 参数 | 默认值 | 说明 |
| --- | ---: | --- |
| log level/spec | `info` | 支持 `flexi_logger` target 过滤语法 |
| 单文件轮转阈值 | `100000000` bytes | 约 100 MB |
| 保留文件数 | `20` | 按 basename 分别保留 |
| 文件命名 | timestamp rotation | 当前文件后缀为 `_rCURRENT.log` |

轮转和保留参数只控制 USDB 进程自己写入的文件。Docker 配置中的
`USDB_LOG_MAX_SIZE`、`USDB_LOG_MAX_FILES` 控制的是 container runtime 日志，
两者不是同一层配置。

## 运行时配置

在启动进程前设置以下环境变量：

| 环境变量 | 示例 | 约束 |
| --- | --- | --- |
| `USDB_PROCESS_LOG_LEVEL` | `info,balance_history::index=debug` | 必须是合法的 `flexi_logger` spec |
| `USDB_PROCESS_LOG_MAX_FILE_BYTES` | `200000000` | 必须大于 0 |
| `USDB_PROCESS_LOG_KEEP_FILES` | `30` | 必须大于 0 |

例如，仅临时提高 `balance-history` 索引模块的详细度：

```bash
USDB_PROCESS_LOG_LEVEL='info,balance_history::index=debug' \
  ./target/release/balance-history --root-dir /path/to/root
```

环境变量非法时，服务在打开数据库和启动 RPC 之前 fail closed，并把原因输出到
stderr。代码显式设置的 logger builder 参数优先于环境变量，环境变量优先于默认值。

## 启动与异常记录

每个进程初始化 logger 后都会写入统一的 `Process started` 记录，包括：

- service、binary、binary version 和 build revision
- PID、可执行文件路径和 service root
- 文件日志目的地、console 开关、level、轮转阈值和保留数

记录不包含 RPC 凭据、签名密钥、token 或完整业务配置。release 构建应在编译时设置
`USDB_BUILD_REVISION`；GitHub 构建可以使用编译时 `GITHUB_SHA`，未提供时记录为
`unknown`。

共享 panic hook 会记录 service、thread、源码位置、panic message 和 backtrace，随后
立即 flush 日志并继续调用 Rust 默认 panic hook。正常关闭时，`balance-history` 和
`usdb-indexer` 会显式 flush 并 shutdown logger，不再依赖固定 sleep 等待落盘。

## 连续故障采样

`balance-history` 和 `usdb-indexer` 对 Bitcoin RPC、上游服务和同步循环的连续失败使用
统一采样规则：记录第 1、2、3 次失败、2 的幂次以及固定周期次数。首次失败使用
`error`，后续样本使用 `warn`；未命中采样的重试仍会更新进度状态，但不会重复写入相同
日志。不同调用链的固定周期如下：

| 调用链 | 重试间隔 | 固定周期样本 |
| --- | ---: | ---: |
| `balance-history` 同步或 BTC tip 轮询 | 10 秒 | 每 30 次，约 5 分钟 |
| `usdb-indexer` 同步或新区块轮询 | 5 秒 | 每 60 次，约 5 分钟 |
| `usdb-indexer` status monitor | 1 秒 | 每 60 次，约 1 分钟 |

依赖恢复后会写入一条 `info` 汇总，包含 `failed_attempts`、`outage_elapsed_ms` 和可用时的
恢复高度。连续失败计数按调用链独立维护，避免一个上游恢复错误地清除另一个上游的故障
状态。

## 核心同步阶段

`balance-history` 的关键记录包括：

- RocksDB 成功打开后的 mode、path 和 elapsed；失败仍由启动调用链记录并终止启动
- 每个 block batch 的高度范围、块数、总耗时、各 preload/process/write 阶段耗时、变更数和 cache 数量
- 每轮同步的范围、batch 数、最终 flush 耗时和总耗时
- reorg rollback 的起止高度、深度、耗时和失败原因；失败时 readiness 保持 recovery pending

`usdb-indexer` 的关键记录包括：

- 每段 block range 的起止高度、块数和耗时
- 每个 block 的 mint、invalid mint、transfer、active balance、energy update 数量及阶段耗时
- snapshot anchor 持久化、历史 snapshot backfill 和整轮同步耗时
- block transaction 任一阶段失败后的 energy/transfer 即时恢复结果；日志同时保留原始错误和恢复错误

无铭文、无 transfer 和逐块开始提示属于正常高频事件，默认降为 `debug`。每块最终状态提交和
durable savepoint 进度仍保留为 `info`，用于定位跨存储一致性边界。

## 排障建议

1. 先检查每次启动的 `Process started`，确认二进制 revision、root 和日志策略正确。
2. 使用 target 级过滤临时扩大局部日志，不建议在长时间同步中全局启用 `debug`。
3. 对 crash 同时保留 panic 日志和进程管理器的 stderr；后者包含 Rust 默认 panic 输出。
4. 修改保留策略前确认磁盘预算。默认上限按单个 basename 约为 2 GB，不包含当前文件、
   其他子命令 basename 和 Docker runtime 日志。
5. 日志初始化失败属于启动阻断错误，不应通过禁用文件和控制台两种目的地来绕过。

日志是诊断证据，不参与共识或恢复决策。错误分支仍必须向调用方返回失败；日志写入成功
不能把失败降级为成功，恢复日志也不能替代 readiness、pending marker 和 durable
savepoint 的状态校验。
