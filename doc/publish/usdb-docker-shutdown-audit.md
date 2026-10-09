# Docker 服务退出检查与验证

本次检查针对 Node1 在 `usdb-control-plane` 阶段无法完成 `down` 的问题。
控制台入口脚本通过 `exec` 将 Rust 进程作为 PID 1，但原程序没有注册 SIGTERM/SIGINT，HTTP 服务也没有 graceful shutdown。
修复在启动时注册信号，停止接收新连接、等待已接收请求完成并刷新日志；不通过超时强杀绕过问题。

## 常驻节点服务

| 服务 | 信号传递与退出路径 | 本次处理 |
| --- | --- | --- |
| control-plane | 入口 `exec` Rust；共享信号监听 → Axum graceful shutdown → logger shutdown | 补齐实现、HTTP 请求排空与真实 PID 1 测试 |
| balance-history | 入口 `exec` Rust；bootstrap 设置取消标记并等待 worker；daemon shutdown → DB flush → RPC close → logger shutdown | 前移注册，bootstrap 与 daemon 使用同一信号订阅，避免阶段切换丢失停止请求 |
| usdb-indexer | 入口 `exec` Rust；停止索引循环、关闭 RPC、刷新日志；未完成的事务遵守已有恢复协议 | 前移信号注册到打开/恢复数据库之前；初始化仍先完成当前安全步骤，不中途丢弃恢复任务 |
| USDB chain | `go-ethereum/scripts/usdb/docker/usdb_runtime_node.sh` 的 TERM/INT trap 转发至 geth/guard 并等待；geth `StartNode` 接收 TERM/INT 后关闭节点 | 核查现有路径；不改变数据库关闭或 guard 恢复语义 |
| Bitcoin Core | 发布镜像 `tini` → 入口 `exec bitcoind`；受管停机请求 RPC stop 并观察退出 | 核查已有退出与阶段日志测试；不缩短落盘等待 |
| 受管 Ord | `ord_runtime.py` 处理 TERM/INT，向子进程发送一次 SIGINT，并等待退出 | 核查已有回归测试，保留“不重复 SIGINT、不自动强杀”约束 |
| 开发环境 Ord / ETHW / regtest Bitcoin | 入口最终 `exec` 原生程序；Ord 0.29.0 启用 ctrlc `termination` feature；Geth/Core 自有信号处理 | 检查入口与原生信号支持，区别于受管 Ord 的等待机制 |

普通 `down` 顺序为 control-plane → chain → indexer → BH → Ord → Bitcoin。
资源阶段转换会选择相应子集；不在此变更中调整顺序或数据库格式。
`healthy` 只表示健康检查通过，不能证明已收到退出信号或正在落盘。
Ord 原生退出语义参见 [0.29.0 handler](https://github.com/ordinals/ord/blob/0.29.0/src/lib.rs#L245) 与
[termination feature](https://github.com/ordinals/ord/blob/0.29.0/Cargo.toml)。

## 启动脚本与一次性任务的边界

- 上表的常驻程序信号处理不等于所有入口脚本、所有初始化步骤都能立即取消。
  BH/indexer 的 shell 入口在 `exec` 前还会生成配置、等待依赖；数据库恢复本身也可能需要时间。
  本次前移注册解决 Rust 启动阶段的信号丢失，不强行中断数据库恢复。
- `btc-snapshot-bootstrap` 已使用 tini，Python 将 SIGTERM 转为受控中断；停止观察者不等于撤销 Core 的导入。
- `snapshot-loader`、旧 script-registry installer、chain init/checkpoint 工具，以及开发环境 world-sim/SourceDAO bootstrap，
  属于一次性或工作流任务，未统一实现 daemon 的排空协议。部分 shell 入口没有 TERM/INT 子进程转发与等待。
  这些路径不应标为“全部支持优雅退出”；新增统一取消机制须分别验证临时文件、发布 marker、数据库及链上交易的恢复语义。
  受管 SourceDAO 运维任务仍按现有互斥规则阻止节点维护，不能把停止容器当成撤销已提交交易。
- 直接 `docker compose down` 或主机关机受 Docker/systemd 自己的超时策略约束，不等价于 `usdb-node down` 的受管等待。

## 验证

- `tests/shutdown_signal.rs`：隔离子进程在开始 polling 之前接收 TERM/INT，验证早期信号被保留。
- `tests/control_plane_shutdown.rs`：真实 TCP 请求；停止监听后仍完成正在处理的请求；空闲 keep-alive 不阻止退出。
- `tests/run_control_plane_shutdown.py`：临时目录、无外部网络、真实二进制作为 Docker PID 1，分别验证 TERM/INT、退出码 0 和落盘日志；已接入 USDB Fast。
- 现有 `tests/test_ord_operations.py`、`tests/test_bitcoin_shutdown_progress.py`、资源 runner 和配套 Go runtime guard 测试，验证停机顺序及既有保护。

PID 1 测试须先构建控制台二进制并准备匹配其 glibc 的镜像，例如：

```bash
cargo build --manifest-path src/btc/Cargo.toml -p usdb-control-plane
python3 tests/run_control_plane_shutdown.py --image ubuntu:24.04
```

仅使用本次测试创建的容器与临时目录。常驻数据服务本次进行代码审查及已有回归测试，
不声称完成了所有真实数据库启动/落盘阶段的容器故障注入。
