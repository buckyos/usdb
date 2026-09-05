# SourceDAO Stage B 安全基线（2026-09-04）

## 1. 审计身份与边界

本轮审计针对 SourceDAO revision
`32c91abd8c3875aa46a8beb25e66129bdb611bf1` 的生产合约、USDB bootstrap/validation/upgrade
脚本和 USDB 专用构建产物。合约源码在本轮基础设施修改中未发生变化。

固定工具链为：

| 项目 | 版本/身份 |
| --- | --- |
| Node.js | `24.12.0` |
| npm | `11.6.2` |
| Hardhat | `3.9.0` |
| Solidity | `0.8.20+commit.a1b79de6` |
| EVM target | `shanghai` |
| Slither | `0.11.6` |
| USDB contract golden schema | `sourcedao-usdb-contract-golden:v1` |
| golden SHA-256 | `cb000da469286fbac81571c532ed7cee52c2fc29e7b318682dc8eab02e0ace8d` |

`security/usdb-contract-golden.json` 覆盖 8 个生产合约，固定 compiler settings、ABI、method
selectors、creation/runtime bytecode 及完整 storage layout。当前与 USDB genesis/config 绑定的关键
runtime identity 为：

| Contract | Runtime code hash | Runtime bytes |
| --- | --- | ---: |
| `SourceDao` | `0xc6549189a694031f7d94e6b1b8b11b2bc43d1753cdaa7ac885a97c5e6d462593` | 4536 |
| `DividendContract` | `0x24859958e81d46d4a3df2de8b346600d05ed8a3e683ad4c2117fe9e73804144e` | 12385 |

本基线不把以下边界视为相互等价：

- USDB genesis 直接写入的 runtime code 与普通 UUPS proxy deployment；
- 测试网可重置 bootstrap candidate 与已经发布 accepted checkpoint 的网络；
- 合约源码安全结论与 Hardhat/npm 构建供应链结论；
- Slither detector severity 与人工确认后的项目风险等级。

## 2. 自动扫描结果

Slither 在排除依赖、mock、legacy 与 test 路径后分析 64 个 contract，得到：

| Impact | Count |
| --- | ---: |
| High | 6 |
| Medium | 9 |
| Low | 43 |
| Informational | 44 |

主要 detector 分布为：`naming-convention=20`、`timestamp=12`、`shadowing-local=11`、
`missing-zero-check=7`、`reentrancy-no-eth=7`、`unindexed-event-address=7`、
`reentrancy-benign=6`、`calls-loop=5`。扫描 workflow 当前是 report-only：发现不会直接阻断
Fast/Nightly/Weekly，但 scanner、编译、报告格式和 golden 漂移失败会阻断该 workflow。

## 3. Confirmed findings

### USDB-AUDIT-005：full proposal 可跨 settlement batch 重复计算已转移投票权

- `source`：人工审计；现有 characterization test
  `test/committee.ts` 的 `documents that transferred voting power can be counted twice across settlement batches`
- `component`：`SourceDaoCommittee.endFullPropose`
- `introduced_by`：SourceDAO 自主合约
- `reachability/exposure`：`confirmed` / `public network`
- `impact`：full proposal 在每个 batch 使用 settlement 时余额；同一批 token 可在 batch 间转移并被多个
  已投票地址重复计入，可能改变治理结果
- `decision`：`fix`
- `owner`：SourceDAO governance owner
- `release_gate`：`both`
- `required fix`：proposal 创建或投票时冻结可验证的 voting-power snapshot，并使所有 settlement batch
  只消费同一 snapshot；修复后补 transfer、burn/mint、batch ordering 和 replay tests

### USDB-AUDIT-006：project approval 未绑定 result/contributions，批准后 manager 仍可改分配

- `source`：人工审计；现有 characterization test
  `test/project.ts` 的 `uses the latest contribution weights for payout even if they change after acceptProject has already been approved`
- `component`：`ProjectManagement._makeProjectParams`、`acceptProject`、`updateContribute`
- `introduced_by`：SourceDAO 自主合约
- `reachability/exposure`：`confirmed` / `public network`
- `impact`：委员会批准的 proposal root 不包含最终 project result 或贡献者分配；manager 可在批准后、
  `promoteProject` 前改写 dev reward 和 extra-token recipients/weights
- `decision`：`fix`
- `owner`：SourceDAO project/governance owner
- `release_gate`：`both`
- `required fix`：proposal commitment 必须绑定 canonical result 与完整 contribution digest；进入
  `Accepting` 后禁止修改，或任何修改必须废弃旧 proposal 并创建新 proposal

### USDB-AUDIT-007：accepted bootstrap 未绑定全部 module implementation/runtime identity

- `source`：人工审计
- `component`：`usdb_validate_bootstrap.ts`、bootstrap state/validation summary、
  `internal/usdbacceptance`
- `introduced_by`：SourceDAO 与 go-ethereum 的跨仓 bootstrap identity
- `reachability/exposure`：`confirmed` / `release input`
- `impact`：当前 strict validator 对 Committee、DevToken、NormalToken、Lockup、Project 和 Acquired
  主要校验 address、version 与少量语义 probe；acceptance identity 只绑定这些字段和 bootstrap
  transaction set，未把每个 proxy/implementation runtime hash 与受审 artifact 绑定。接口兼容但逻辑恶意或
  错误的 module 可能被 release operator 意外接受，并影响投票权、解锁、mint 与资产流向
- `decision`：`fix`
- `owner`：SourceDAO bootstrap owner / go-ethereum release owner
- `release_gate`：`both`
- `required fix`：validation summary 记录 proxy runtime hash、ERC-1967 implementation address/runtime
  hash 和 expected artifact identity；acceptance schema 将其纳入 canonical commitment；补 wrong proxy、
  wrong implementation、same-version malicious module、restart/joiner 和 tampered-summary tests

### USDB-AUDIT-008：增删委员会成员未保持 non-zero/non-empty 不变量

- `source`：人工审计
- `component`：`SourceDaoCommittee.prepareAddMember`、`addCommitteeMember`、`removeCommitteeMember`
- `introduced_by`：SourceDAO 自主合约
- `reachability/exposure`：`confirmed` / `governance action`
- `impact`：已通过的 proposal 可加入 zero address；单成员委员会可删除最后一名成员，使普通治理永久
  无法达到投票条件
- `decision`：`fix`
- `owner`：SourceDAO governance owner
- `release_gate`：`mainnet`
- `required fix`：增员同时拒绝 zero address；减员要求结果集合至少保留一名成员；与 full replacement
  共用同一 committee-list invariant 和边界测试

### USDB-AUDIT-009：任意 ERC-20 路径按声明 amount 记账而非实际 received amount

- `source`：人工审计；Slither 的外部调用结果用于定位，不直接覆盖该业务语义
- `component`：`Dividend.deposit`、`ProjectManagement.createProject`、`Acquired.startInvestment`
- `introduced_by`：SourceDAO 自主合约与任意第三方 token 交互
- `reachability/exposure`：`confirmed` / `public network`
- `impact`：fee-on-transfer、rebasing 或恶意 token 可令内部 liability 大于真实余额，造成同 token 后续
  payout/withdrawal 失败或用户实收少于协议记录
- `decision`：`fix`
- `owner`：SourceDAO asset-accounting owner
- `release_gate`：`mainnet`
- `required fix`：明确只支持标准 ERC-20 并由治理 whitelist 强制，或以 transfer 前后余额差作为实收额；
  对 fee-on-transfer、rebase、false/no-return 和 callback token 增加矩阵测试

### USDB-AUDIT-010：生产 runtime 仍包含 Hardhat console 调用

- `source`：人工 artifact 审计
- `component`：`Acquired.invest`、`DividendContract._tryNewCycle`
- `introduced_by`：SourceDAO 调试代码
- `reachability/exposure`：`confirmed` / `production runtime`
- `impact`：不会在当前 EVM 上直接授予权限，但引入不必要的 magic-address external call、gas 开销和
  开发工具耦合；也会改变 genesis-bound runtime code hash
- `decision`：`fix`
- `owner`：SourceDAO build owner
- `release_gate`：`mainnet`
- `required fix`：独立 artifact-changing batch 删除 active `console.log` 与 imports，更新 SourceDAO golden、
  Go bootstrap configs、network bundle/genesis hash，并让 bytecode audit 拒绝 console magic address

## 4. Mitigated/operational findings

### USDB-AUDIT-011：public initializer 与 bootstrap-admin migration 依赖 accepted bootstrap/atomic upgrade

- `source`：人工审计；Slither `unprotected-upgrade`
- `component`：`SourceDao.initialize`、`DividendContract.initialize`、
  `SourceDao.migrateBootstrapAdmin`、USDB bootstrap scripts
- `introduced_by`：SourceDAO/USDB bootstrap 设计
- `reachability/exposure`：`confirmed` / `bootstrap or upgrade transaction ordering`
- `impact`：未接受的 candidate chain 上可以抢先调用 public initializer；legacy proxy 若先执行不带
  migration calldata 的升级，后续 public reinitializer 可被第三方抢先调用
- `decision`：`mitigate`
- `evidence`：USDB 发布只接受绑定 exact genesis/config/state、completed bootstrap transaction set、
  checkpoint block hash/state root 的 acceptance artifact；upgrade proposal 绑定
  `keccak256(data)` 并要求 atomic `upgradeToAndCall`
- `owner/expires_at`：SourceDAO bootstrap owner / public-mainnet security freeze
- `release_gate`：`mainnet`，若任何 public release 允许绕过 acceptance artifact 或非原子 migration
- `recheck`：initializer、acceptance schema、upgrade executor 或 governance calldata commitment 变化时

### USDB-AUDIT-012：upgrade CLI 和持久化报告可能暴露 operator secret

- `source`：人工审计
- `component`：`upgrade_existing_sourcedao.ts`、bootstrap/validation scripts
- `introduced_by`：SourceDAO operator tooling
- `reachability/exposure`：`confirmed` / `local process list, shell history, CI/operator logs`
- `impact`：`--private-key` 会进入 argv；包含 credentials/query token 的 RPC URL 会写入日志或 state
- `decision`：`fix`
- `owner`：SourceDAO release tooling owner
- `release_gate`：`mainnet`
- `required fix`：签名密钥只从受限 env/file/external signer 读取，移除 argv/config 明文入口；报告和日志
  对 URL userinfo/query/secret path 做 fail-closed validation 或 canonical redaction

## 5. Slither high/medium disposition

| Detector | Disposition | Evidence/remaining work |
| --- | --- | --- |
| `arbitrary-send-eth` | `false-positive` | recipient 分别固定为 investment owner/buyer 或 dividend claimant；调用失败原子 revert |
| `reentrancy-eth` | `mitigated` | `withdrawDividends` 使用 `nonReentrant`，claim marker 和 token liability 均在外部调用前更新；已有 callback/native receiver tests |
| `unchecked-transfer` | `fix` | `DevToken.mintDevToken` 的 self-call 当前由本项目 ERC-20 实现保证 revert/true；后续改为内部 `_transfer` 以消除不必要 external self-call |
| `uninitialized-state` | `false-positive` | mapping 的 Solidity 默认空状态即合法初始值 |
| `unprotected-upgrade` | `mitigated` | direct implementation 受 `onlyProxy` 限制；proxy upgrade 还需 committee 对 implementation 和 calldata hash 的批准；initializer 风险单列为 AUDIT-011 |
| `missing-zero-check` | `false-positive` | 7 个 setter 都经 `_requireValidAddress` 统一检查 non-zero 和 `code.length` |
| `reentrancy-no-eth`/`benign` | `mitigated` | 命中入口均有 `nonReentrant`；仍需在 AUDIT-009 修复后重跑 callback-token matrix |
| `divide-before-multiply` | `accept-temporarily` | 当前行为是向下取整且不会超发；需要冻结 dust policy，并用 oracle/invariant tests 证明累计账本边界 |
| `timestamp` | `accept-temporarily` | 时间用于日级 governance/cycle/vesting，不用于短窗口定价；需保留 miner timestamp-boundary tests |

其余 naming、event indexing、shadowing 和 style detector 不作为漏洞结论，但在修改相邻 runtime 时应一并
清理；不能为降低告警数量而单独制造无业务价值的 bytecode 变化。

## 6. 当前发布结论与后续批次

1. Slither 已进入独立 report-only workflow；现阶段不会阻塞持续进行的 testnet 功能开发。
2. USDB 专用构建的 ABI/bytecode/storage 未经显式更新 golden 将失败，能阻止 Hardhat、solc、依赖或
   合约修改静默改变发布 artifact。
3. 按当前统一安全策略，AUDIT-005/006/007 在修复并重建 acceptance/network identity 前是
   testnet/mainnet blocker。
   如果要对已经存在的可重置测试网临时例外，必须另行记录风险接受、禁用治理动作的运维约束和失效日期，
   不能仅把 scanner 设为 report-only 视为接受。
4. 下一批按顺序处理：bootstrap 全 module identity；治理 snapshot/commitment；committee 与
   arbitrary-token accounting；initializer/signer tooling；最后清理 console 并统一更新
   SourceDAO/Go/network-bundle golden。
5. 合约修复每批都必须运行 full Hardhat tests、USDB build/audit/golden、Slither、bootstrap
   restart/joiner、accepted checkpoint 和对应 network reset/replay tests。
