# `/home/bucky/work/usdb` 的 AGENTS 指南

## 作用范围与目标
- 本文件是本仓库内 AI 编码代理的通用工作指南。
- 优先保持改动最小化、局部化。
- 遵循现有项目风格与命名，尤其是 Rust crate 相关约定。
- 未经明确指示，不要删除或修改用户数据目录与服务状态。
- 主要目标 Rust workspace 位于 `src/btc`；仓库根目录本身没有 `Cargo.toml`。
- AI agent 与开发者沟通统一使用中文。

## 仓库结构
- Rust workspace 清单：`src/btc/Cargo.toml`
- Workspace 成员：
  - `src/btc/usdb-indexer`
  - `src/btc/balance-history`
  - `src/btc/balance-history-cli`
  - `src/btc/usdb-util`
- 主要二进制：
  - `usdb-indexer`（守护进程）
  - `balance-history`（服务）
  - `balance-history-cli`（CLI 客户端）
- 工具库：
  - `usdb-util`

## 已检查的指令文件
- `.cursor/rules/` 与 `.cursorrules`：不存在。
- `.github/copilot-instructions.md`：不存在。
- 根目录或更深层级 `AGENTS.md`：在本文件创建前不存在。
- 若后续新增上述文件，请更新本指南并合并更高优先级的本地规则。

## 环境与工具链假设
- 本项目基于 Rust 2024。
- 除非使用 `--manifest-path`，以下命令默认从仓库根目录执行。
- 优先使用稳定的 Cargo 工作流，除非本地初始化确有必要，避免一次性脚本。

## 常用命令模式

### 构建
- `cargo build --manifest-path src/btc/Cargo.toml --workspace`
  - 构建 workspace 内所有 crate。
- `cargo build --manifest-path src/btc/Cargo.toml -p <package>`
  - 仅构建单个 crate（`usdb-indexer`、`balance-history`、`balance-history-cli` 或 `usdb-util`）。

### 检查
- `cargo check --manifest-path src/btc/Cargo.toml --workspace`
  - 在深入验证前进行快速编译检查。
- `cargo check --manifest-path src/btc/Cargo.toml --all-targets --workspace`
  - 同时覆盖测试与集成构建产物。

### 运行二进制
- `cargo run --manifest-path src/btc/Cargo.toml -p usdb-indexer`
- `cargo run --manifest-path src/btc/Cargo.toml -p balance-history`
- `cargo run --manifest-path src/btc/Cargo.toml -p balance-history-cli -- --help`

### 测试
- 运行 workspace 全量测试：
  - `cargo test --manifest-path src/btc/Cargo.toml --workspace`
- 运行单个 crate 全量测试：
  - `cargo test --manifest-path src/btc/Cargo.toml -p usdb-util`
- 按精确名称运行单测：
  - `cargo test --manifest-path src/btc/Cargo.toml -p usdb-util test_script_hash -- --exact`
  - 将 `test_script_hash` 替换为精确测试名。
- 按包名与模块路径运行测试（如有需要）：
  - `cargo test --manifest-path src/btc/Cargo.toml -p balance-history test_script_hash`（当名称在该包内唯一时可用）。
- 若测试因外部重依赖失败，优先使用模块级过滤：`cargo test -p <package> <module>::<test> -- --exact`。

### 格式化与静态检查
- `cargo fmt --manifest-path src/btc/Cargo.toml --all`
- `cargo fmt --manifest-path src/btc/Cargo.toml --all -- --check`
- `cargo clippy --manifest-path src/btc/Cargo.toml --workspace --all-targets --all-features`
- 若 clippy 噪声来自历史代码，优先做针对性修复，并尽量将 lint 范围限制在相关 package。

### 文档与打包检查
- `cargo doc --manifest-path src/btc/Cargo.toml --workspace --no-deps`
- 修改公开 API 时，保持文档构建可通过。
- 若新增 CLI 参数或配置项，需在对应 `cmd` 模块更新帮助文案并验证解析路径。
- 文档默认优先中文；在可行场景下提供英文对照版本。

## 代码风格与约定
- 遵循 Rust 2024 习惯写法，保持代码地道（idiomatic）。
- 重排导入时，优先 `std`，再外部 crate，最后内部模块。
- `use` 列表保持显式分组；无充分理由避免通配符导入。
- 使用 4 空格缩进，并遵循现有大括号风格。
- Rust 标识符命名：值与变量优先 `snake_case`；类型/枚举/trait 用 `PascalCase`；常量用 `UPPER_SNAKE_CASE`。
- 保持模块结构（如 `mod.rs`、`config.rs`、`index.rs`、`status.rs`），非必要不移动文件。
- 错误处理：
  - 二进制中允许显式退出并输出清晰日志。
  - 库代码优先返回 `Result`，由调用方决定处理策略。
  - 在转换失败场景补充上下文（如网络/文件/路径解析）。
- `unsafe` 使用应尽量少且意图明确；每个 `unsafe` 块附近添加简短安全说明。
- 优先使用显式 `Arc`、`tokio::sync` 通道与清晰所有权边界，避免隐藏可变全局状态。
- 长生命周期异步循环需具备取消感知能力。
- 标志位与配置字段命名应直接表达命令语义（如 `max_block_height`、`daemon`）。
- 日志与错误信息尽量保持可预测格式；若无下游协同，不要随意修改既有 message key。
- 除短期迁移或兼容遗留代码外，避免引入 `#[allow(...)]`。
- 代码层面的日志与注释统一使用英文，便于统一处理与展示。

## 日志规范（基于 `balance-history` 实践）
- 覆盖关键链路：对关键输入（CLI 参数、环境变量、配置文件）、状态切换与错误路径提供可追踪日志。
- 记录输入上下文：启动阶段记录配置来源（默认值/配置文件/环境变量/命令行）与关键参数生效值；涉及敏感字段时必须脱敏（如密码、token、私钥）。
- 记录状态切换：在初始化、启动、同步、停止、关闭等节点记录 `from -> to`、触发原因与关键上下文（例如 `block_height`、`with_utxo`、监听端口、目录路径）。
- 记录长任务进度：参考 `balance-history` 的索引/快照流程，至少包含 start/progress/finish 三类日志，并带上可量化字段（总量、当前值、耗时或批次信息）。
- 错误日志必须带完整上下文：至少包含模块、动作、目标对象、关键参数与底层错误；优先使用统一格式构造 `msg` 后同时用于 `error!` 与 `Err(msg)` 返回，保证日志与返回信息一致。
- 降级与回退场景必须可见：例如策略切换、重试、跳过校验、使用默认配置等，使用 `warn!`/`info!` 明确记录原因与影响。
- 日志文本保持稳定可检索：避免无意义缩写，字段命名与消息结构尽量稳定，便于后续按关键字、阶段和参数检索定位问题。

### 日志模板示例（建议直接复用）
- 启动与配置输入（注意敏感字段脱敏）：
```rust
info!(
    "Service starting: module=main, cmd={}, config_source={}, rpc_url={}, data_dir={}",
    cli_command,
    config_source,
    masked_rpc_url,
    data_dir.display()
);
```
- 状态切换（统一 `from -> to`）：
```rust
info!(
    "State transition: module=indexer, from={}, to={}, reason={}, block_height={}",
    from_phase,
    to_phase,
    reason,
    block_height
);
```
- 长任务进度（start/progress/finish）：
```rust
info!(
    "Snapshot task started: module=snapshot, target_height={}, with_utxo={}, total={}",
    target_height,
    with_utxo,
    total
);
info!(
    "Snapshot task progress: module=snapshot, processed={}, total={}, percent={:.2}",
    processed,
    total,
    processed as f64 * 100.0 / total as f64
);
info!(
    "Snapshot task finished: module=snapshot, processed={}, total={}, elapsed_ms={}",
    processed,
    total,
    elapsed_ms
);
```
- 错误路径（日志与返回信息一致）：
```rust
let msg = format!(
    "DB write failed: module=db, action=put_utxos, table={}, batch_size={}, error={}",
    table_name,
    batch_size,
    e
);
error!("{}", msg);
return Err(msg);
```
- 降级与回退（可观测）：
```rust
warn!(
    "Fallback activated: module=indexer, from_strategy={}, to_strategy={}, reason={}",
    "rpc",
    "local_loader",
    reason
);
```
- 字段建议（按场景选取，避免无意义堆叠）：`module`、`action`、`target`、`phase`、`from`、`to`、`reason`、`block_height`、`batch_size`、`total`、`processed`、`elapsed_ms`、`error`。

## 导入、文件与组织
- 不要随意改变 crate 边界。
- 新增模块时，仅在 `mod` 文件暴露必要接口。
- 保持二进制入口（`main.rs`）轻量，将编排逻辑下沉到库模块。
- 测试辅助代码靠近被测模块并置于 `#[cfg(test)]` 下。
- 目录与配置命名沿用既有模式：
  - `service`、`index`、`db`、`cache`、`storage`、`btc`、`status`、`output`。
- 处理 RPC 与 IO 时，优先小而确定的辅助函数，避免单个超大 `async` 代码块。

## 变更测试策略
- 对解析器/配置改动：
  - 在对应模块新增或调整单元测试。
  - 使用 `cargo test -p <crate> <test_name> -- --exact` 验证。
- 对存储/数据库行为改动：
  - 优先在模块测试区编写有针对性的单测。
  - 确保测试清理临时数据，避免写入 `~/.bitcoin` 或服务目录。
- 对二进制行为改动：
  - 至少验证 `cargo run ... --help`，并尽量覆盖一个最小非破坏路径。

## 安全与运维指引
- 严禁提交密钥、API 凭据、RPC URL、钱包文件或私钥。
- 测试期间避免误写用户主目录下的服务数据目录。
- 新测试优先使用临时目录与 mock/fake 数据。
- 对外部输入（RPC/配置/用户路径）使用防御式解析。
- 除非有明确迁移计划，保持命令行向后兼容。

## 提交与分支流程
- 提交模式采用 `commit -> rebase -> push`，保证时间线更清晰。
- 保持提交范围聚焦，并在提交说明中记录改动动机。
- 提交说明需包含对应的英文描述（简洁即可，不必过长），且英文描述首字母大写。
- 优先增量式修改，避免与当前任务无关的大重构。

## 代理提交前的最低验证
- `cargo check --manifest-path src/btc/Cargo.toml --workspace`
- `cargo test --manifest-path src/btc/Cargo.toml -p usdb-util`（作为 crate 健康度的快速信号）
- 若变更涉及 CLI 行为，手动运行相关 `--help` 路径。

## 单行示例（可直接复制）
- 构建全部 crate：
  - `cargo build --manifest-path src/btc/Cargo.toml --workspace`
- 运行单个测试：
  - `cargo test --manifest-path src/btc/Cargo.toml -p usdb-util test_script_hash -- --exact`
- 运行格式化+lint 流水线：
  - `cargo fmt --manifest-path src/btc/Cargo.toml --all -- --check && cargo clippy --manifest-path src/btc/Cargo.toml --workspace --all-targets --all-features`
