# Snapshot/Checkpoint 安装边界审计（2026-09-04）

## 1. 审计范围

本轮人工审计覆盖：

- `snapshot_distribution.py` 的 content-addressed record 下载、断点续传、并行 Range 下载、staging
  和原子发布；
- balance-history signed snapshot manifest 的本地文件解析；
- `usdb-indexer-checkpoint-tool` 的 indexer staging、balance-history 恢复残留清理和 paired install；
- 已安装 artifact 的复用校验。

远端 release record、object key、文件名和 manifest 属于不可信输入；本地 destination root 和 trusted-key
catalog 路径属于 operator 输入。即使攻击者已经能写入 destination root，安装器也不能因此跟随 symlink
写出该目录，或把未声明文件发布为可信 artifact。

## 2. Findings

### USDB-AUDIT-001：顺序断点下载可跟随预置 `.part` symlink

- `source`：人工审计
- `component`：balance-history snapshot distribution installer
- `introduced_by`：USDB 自主代码
- `reachability/exposure`：`confirmed` / `local input`
- `impact`：以更高权限运行 installer 时，能写 destination root 的本地主体可使 `curl --output`
  续写 root 外部文件
- `decision`：`fix`
- `fix`：顺序和并行下载在读写前均要求 parent 是真实目录，target、range state 和 chunk 是普通文件；
  broken symlink 也按已存在的非法条目拒绝
- `verification`：symlinked partial、download cache、range state/workdir regression tests
- `release_gate`：`both`

### USDB-AUDIT-002：可恢复 staging 接受 symlink 或未声明条目

- `source`：人工审计
- `component`：snapshot distribution staging and installed artifact cache
- `introduced_by`：USDB 自主代码
- `reachability/exposure`：`confirmed` / `local input`
- `impact`：staging symlink 可把下载写到 destination root 外；额外文件可随目录 rename 一并进入不可变
  artifact
- `decision`：`fix`
- `fix`：`.downloads`、`.installing` 必须是真实目录；恢复阶段只允许 release record 声明的文件及对应
  `.part`/Range 状态；发布和复用时要求完整精确 inventory，所有条目均为普通文件
- `verification`：symlinked staging/cache/partial、unexpected staging entry、extra installed entry 和正常
  interrupted-resume tests
- `release_gate`：`both`

### USDB-AUDIT-003：paired-checkpoint 清理依赖 `remove_dir_all` 的 symlink 平台语义

- `source`：人工审计
- `component`：usdb-indexer-checkpoint-tool
- `introduced_by`：USDB 自主代码
- `reachability/exposure`：`likely` / `local input`
- `impact`：具有 managed prefix 的恶意条目不应依赖平台库行为决定是否删除或跟随
- `decision`：`fix`
- `fix`：indexer staging、balance-history live DB 和 snapshot install remnants 在清理前使用
  `symlink_metadata`，只删除真实目录；symlink、普通文件和特殊文件全部 fail closed
- `verification`：真实目录清理、symlink target 保留和 indexer staging symlink regression tests
- `release_gate`：`both`

### USDB-AUDIT-004：signed Rust manifest 仍允许重复 JSON key

- `source`：人工审计
- `component`：balance-history and usdb-indexer checkpoint manifest parsers
- `introduced_by`：Rust serde default behavior
- `reachability/exposure`：`confirmed` / `artifact input`
- `impact`：不同语言或工具可能对重复 key 采用不同解释，降低跨实现审计确定性
- `decision`：`mitigate`
- `current mitigation`：snapshot `file_name` 已强制为单个安全 basename；签名覆盖解析后的 canonical
  structure；paired binding 另外绑定 exact manifest SHA-256、snapshot SHA-256 和完整 state-ref
- `remaining work`：引入共享 strict JSON loader，对 snapshot/checkpoint manifest 的全部 object 层拒绝
  duplicate key，并增加 Rust/Python golden corpus；该变更单独实施以避免与文件系统修复混合
- `release_gate`：`mainnet`

## 3. 验证结论

本轮修复后，已知的预置 symlink、broken symlink、特殊条目和 staging inventory 污染均在任何 artifact
发布前失败关闭。正常顺序断点续传、并行 Range 续传、完整 artifact 复用和 paired install 故障恢复保持
支持。

仍不把 destination root 的并发同 UID 恶意修改视为隔离边界；部署要求该目录仅由 USDB operator
账户可写。若未来 installer 以 root 身份消费非 root 可写目录，需要进一步改为基于 directory FD、
`openat`/`O_NOFOLLOW` 的逐项操作。
