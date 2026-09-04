# CodeQL 初始基线（2026-09-04）

## 1. 扫描身份

| Repository | Analyzed revision | Open alerts | 覆盖说明 |
| --- | --- | ---: | --- |
| `buckyos/usdb` | `fd2855a38b07f005496a185bf24573a2e09e3b81` | 26 | Rust、Python、JavaScript/TypeScript、Actions |
| `buckyos/go-ethereum` | `55a09404986feb5d226037b2a4f9191a19a51f9e` | 13 | Go、C/C++、Java/Kotlin、Python、JavaScript/TypeScript、Actions |
| `buckyos/SourceDAO` | `a9388b15398b07a1bb4846d7c470c055c98b1e7c` | 0 | JavaScript/TypeScript、Actions；不覆盖 Solidity 语义 |

这是 GitHub default setup 的首次分类基线。三个仓库当前均为 report-only；alert 本身不阻断
Fast/Nightly/Weekly。`SourceDAO=0` 不能替代 Slither、合约人工审计和 bytecode/storage golden
comparison。

## 2. USDB findings

### USDB-CQL-001：fake-chain 固定 block nonce 被识别为硬编码密钥

- `source`：CodeQL `rust/hard-coded-cryptographic-value`，alerts
  [15](https://github.com/buckyos/usdb/security/code-scanning/15)-[26](https://github.com/buckyos/usdb/security/code-scanning/26)
- `component`：`balance-history/tests/fake_chain_sync.rs`
- `artifact/owner`：不进入 release artifact / balance-history owner
- `introduced_by`：USDB 自主测试代码
- `reachability/exposure`：`unreachable` / `test-only`
- `impact`：无密钥泄露；数值是构造确定性 fake block hash 的 nonce
- `decision`：`false-positive`
- `release_gate`：`none`
- `recheck`：该 fixture 或测试 helper 进入 release target/image 时重新审计

### USDB-CQL-002：control-plane 配置 URL 被识别为 SSRF

- `source`：CodeQL `rust/request-forgery`，alerts
  [12](https://github.com/buckyos/usdb/security/code-scanning/12)、
  [13](https://github.com/buckyos/usdb/security/code-scanning/13)、
  [14](https://github.com/buckyos/usdb/security/code-scanning/14)
- `component`：`usdb-control-plane` HTTP/JSON-RPC client
- `artifact/owner`：USDB services image / control-plane owner
- `introduced_by`：USDB 自主代码
- `reachability/exposure`：`unreachable` from remote request / `local input`
- `impact`：恶意本地配置可以让服务访问任意 HTTP endpoint；这与本地运维配置权限等价
- `decision`：`mitigate`
- `evidence`：URL 仅来自启动时加载的本地 `config.toml`；proxy request 只能选择 allowlist method，
  不能提交目标 URL
- `worktree mitigation`：统一要求绝对 HTTP(S) URL、host、无内嵌凭据/fragment；每次请求前
  再校验；共享 client 禁用 redirect
- `release_gate`：`both`，仅当 URL 来源变为远程可控或上述约束失效时阻断
- `verification`：`cargo test -p usdb-control-plane` 与
  `cargo clippy -p usdb-control-plane --all-targets -- -D warnings` 已通过；待提交后的下一次
  CodeQL 扫描

### USDB-CQL-003：world-sim wallet name 参与 mint payload 文件路径

- `source`：CodeQL `rust/path-injection`，alerts
  [9](https://github.com/buckyos/usdb/security/code-scanning/9)、
  [10](https://github.com/buckyos/usdb/security/code-scanning/10)
- `component`：`usdb-control-plane` development mint
- `artifact/owner`：USDB services image / control-plane owner
- `introduced_by`：USDB 自主代码
- `reachability/exposure`：`likely` with corrupted local marker / `local input`
- `impact`：development profile 下可能把 mint payload 写出预期 runtime 目录
- `decision`：`fix`
- `worktree fix`：marker 解码和最终写文件前都要求长度不超过 64 bytes，字符集仅为 ASCII
  字母、数字、`-`、`_`
- `release_gate`：`both`
- `verification`：路径穿越/字符集/长度 corpus 单元测试与 control-plane `clippy -D warnings`
  已通过；待提交后的下一次 CodeQL 扫描

### USDB-CQL-004：Bitcoin cookie file 路径来自配置

- `source`：CodeQL `rust/path-injection`，alert
  [7](https://github.com/buckyos/usdb/security/code-scanning/7)
- `component`：`usdb-control-plane` Bitcoin RPC authentication
- `artifact/owner`：USDB services image / control-plane owner
- `reachability/exposure`：`unreachable` from remote request / `local input`
- `impact`：能修改 control-plane 配置的本地运维者可以指定另一个可读文件
- `decision`：`accept-temporarily`
- `evidence`：路径只来自启动配置；远程 API 不能修改该字段；绝对路径是容器 secret/bind mount
  的必要能力
- `owner/expires_at`：control-plane owner / public-mainnet security freeze
- `release_gate`：`mainnet`，需要在主网前确认容器只读挂载、文件类型/权限和 secret 生命周期

### USDB-CQL-005：bootstrap artifact 路径来自配置

- `source`：CodeQL `rust/path-injection`，alerts
  [8](https://github.com/buckyos/usdb/security/code-scanning/8)、
  [11](https://github.com/buckyos/usdb/security/code-scanning/11)
- `component`：`usdb-control-plane` artifact summary
- `artifact/owner`：USDB services image / control-plane owner
- `reachability/exposure`：`unreachable` from remote request / `local input`
- `impact`：本地配置可选择要展示摘要的 artifact
- `decision`：`false-positive`
- `evidence`：读取目标不是请求参数；绝对路径是 Docker bind mount 和 release bundle 的既定接口；
  返回值仅包含解析后的摘要，不返回任意文件原文
- `release_gate`：`none`

### USDB-CQL-006：snapshot trusted-key 日志被识别为明文 secret

- `source`：CodeQL `rust/cleartext-logging`，alerts
  [4](https://github.com/buckyos/usdb/security/code-scanning/4)-[6](https://github.com/buckyos/usdb/security/code-scanning/6)
- `component`：`balance-history` snapshot verifier
- `artifact/owner`：balance-history binary and snapshot tools / balance-history owner
- `reachability/exposure`：`unreachable` / `operator log`
- `impact`：无私钥泄露
- `decision`：`false-positive`
- `evidence`：日志只输出 trusted public-key set 的文件路径或 signer key ID；签名私钥不由该
  verifier 加载，也不在这些日志参数中
- `release_gate`：`none`

### USDB-CQL-007：Bitcoin `rpcauth` HMAC-SHA256 被识别为弱摘要

- `source`：CodeQL `py/weak-sensitive-data-hashing`，alerts
  [2](https://github.com/buckyos/usdb/security/code-scanning/2)、
  [3](https://github.com/buckyos/usdb/security/code-scanning/3)
- `component`：Bitcoin RPC credential generator and bundle validator
- `artifact/owner`：release node kit / release tooling owner
- `reachability/exposure`：`confirmed` / `local input`
- `impact`：无算法降级选择；输出必须符合 Bitcoin Core `rpcauth=user:salt$hmac` 格式
- `decision`：`false-positive`
- `evidence`：实现使用随机 128-bit salt、随机 256-bit password 和 HMAC-SHA256；validator
  使用 constant-time `hmac.compare_digest`
- `release_gate`：`none`
- `recheck`：Bitcoin Core `rpcauth` 格式变化时重新审计

### USDB-CQL-008：独立 rpcauth CLI 将一次性密码写到 stdout

- `source`：CodeQL `py/clear-text-logging-sensitive-data`，alert
  [1](https://github.com/buckyos/usdb/security/code-scanning/1)
- `component`：`generate_bitcoin_rpcauth.py` standalone CLI
- `artifact/owner`：release node kit / release tooling owner
- `reachability/exposure`：`confirmed` / `operator terminal or CI log`
- `impact`：命令输出被日志或 shell history 周边工具捕获时泄露 Bitcoin RPC password
- `decision`：`fix`
- `worktree fix`：standalone CLI 要求已有 mode `0600` 的普通 `node.env`，原子更新其中的
  `BTC_RPC_USER/BTC_RPC_PASSWORD`；stdout 只返回 username 和文件位置。symlink、权限过宽、字段
  重复/缺失、rpcauth 路径不匹配或并发修改均失败关闭，失败时删除本次新建的 rpcauth
- `verification`：生成器 7 个 CLI/unit tests、Bitcoin runner 12 个 contract tests、bundle
  validator 49 tests、`usdb-node` 94 tests、shellcheck 与 release fragment validation 已通过；
  待提交后的下一次 CodeQL 扫描
- `release_gate`：`mainnet`

## 3. go-ethereum inherited baseline

### GO-CQL-001：ETHW/geth 继承告警组

- `source`：13 个 open alerts：request forgery 2、allocation overflow 3、clear-text logging 2、
  path injection 1、integer conversion 1、CDN without SRI 4
- `component`：`cmd/faucet`、`accounts/usbwallet`、`log`、`cmd/clef`、`signer`、`p2p/simulations`
- `artifact/owner`：待按最终 binary/image reachability 拆分 / go-ethereum security owner
- `introduced_by`：ETHW/geth inherited code
- `reachability`：`unknown` for final USDB release artifacts
- `decision`：`accept-temporarily` pending Stage D artifact reachability audit
- `evidence`：当前 alerts 不位于 `internal/usdb`、`core/usdbstate` 或 USDB consensus/miner 增量路径
- `owner/expires_at`：go-ethereum security owner / public-mainnet toolchain qualification
- `release_gate`：`mainnet`；若 faucet、clef、USB wallet 或 simulations 进入公开 testnet artifact/exposure，
  提前升级为 `testnet`

不能仅按源码目录关闭这些 alerts。阶段 D 必须核对实际编译 target、镜像内容、启动参数和公开端口。

## 4. SourceDAO baseline

### SDA-CQL-001：CodeQL 无 open alert，但 Solidity 未覆盖

- `source`：CodeQL default setup
- `component`：SourceDAO JavaScript/TypeScript scripts and Actions
- `artifact/owner`：SourceDAO artifact/bootstrap toolchain / SourceDAO owner
- `reachability`：不适用于 Solidity 合约结论
- `decision`：`mitigate`
- `next`：运行 Slither、initializer/权限/外部调用人工审计，并保持 bytecode、ABI、storage layout
  和 bootstrap transcript golden comparison
- `release_gate`：`mainnet`

## 5. 下一步

1. USDB-CQL-002/003/008 修复已经提交；复跑 CodeQL 后把新 scan revision/alert 状态写入下一份基线。
2. Snapshot/checkpoint 的首轮 symlink、staging inventory 和 cleanup 专项审计已经完成，结论见
   [Snapshot/checkpoint 安装边界审计](./snapshot-checkpoint-install-audit-2026-09-04.md)；strict JSON
   duplicate-key corpus 已落地，剩余真实 artifact 跨进程演练继续跟踪。
3. 为 SourceDAO 增加 Solidity 专用扫描和人工 findings。
4. 按最终 go-ethereum binary/image 的可达性拆分 GO-CQL-001。
