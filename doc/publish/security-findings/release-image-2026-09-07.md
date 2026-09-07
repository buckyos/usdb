# 2026-09-07 发布镜像安全处置

本记录对应 `usdb-testnet-v0-r16` 失败的 image scan：
[run 34121165089](https://github.com/buckyos/usdb/actions/runs/34121165089)，
源码 `312be38456c30b6259726a2b9719feb40f7341ac`。
原始 Trivy 0.74.0 报告为 Bitcoin 5 Critical / 78 High（29 个 CVE），
services 5 Critical / 89 High（35 个 CVE）；177 个包告警行全部来自 Debian 12.15。
告警数量包含同源包映射重复，不能代表独立可利用路径数量。

本批处置经维护者要求实施：能兼容修复的依赖先修复；其余按实际适用性限时登记，保留严格门禁。
登记维护责任为 `buckyos/usdb maintainers`，复审/失效时间为 **2026-10-07 00:00 UTC**。
本例外只覆盖正式测试网发布及标准部署范围，不是主网安全认证。

## 已实施修复

1. `.cargo/config.toml` 统一强制 `LIBSQLITE3_FLAGS=-USQLITE_ENABLE_FTS5`，Docker builder 复制该配置。
   `libsqlite3-sys` 支持追加 `-U` 参数，消除未使用 FTS5 的攻击路径。Rust 回归测试验证
   `sqlite_compileoption_used('ENABLE_FTS5') == 0`，并验证无法创建 FTS5 虚拟表。
   这针对 CVE-2026-11822/11824 的静态副本；系统包告警仍单独处置。
2. USDB workspace 的兼容依赖更新消除了原 17 条 RustSec vulnerability 记录。
   包括 `bytes`、`h2`、`crossbeam-epoch`、`aws-lc-rs/sys`、`rustls-webpki`、
   `quinn-proto`、`time`，以及通过 `rss`/`atom_syndication` 更新 `quick-xml`。
3. Ord 源码仍为 0.23.3、commit `ba60f87b530c01b15f6f8645e2ed4ef52f3f9f74`。
   固定 `docker/locks/ord-0.23.3.Cargo.lock`，兼容依赖更新消除了该锁文件原 14 条漏洞记录。
   两个锁文件当前 `cargo-audit 0.22.2` vulnerability count 都为 0；
   unmaintained/unsound 等 warnings 不等于漏洞已被修复，仍按依赖维护事项跟进。
   真实二进制 Trivy 扫描又发现 Rust OpenSSL 绑定的 5 个 CVE，两个锁文件均升级到
   `openssl 0.10.81` / `openssl-sys 0.9.117`，最终产物扫描不再报告这些漏洞。
4. 4 个 USDB 服务/工具二进制和 Ord 均使用固定 `cargo-auditable 0.7.5`、`build --locked`。
   workflow 要求 Trivy 报告实际包含这 5 个路径的非空 Rust 依赖清单；缺失即失败。
5. 安装器原本已在解包前拒绝所有软硬链接，本批将该保护纳入回归测试。
   验证期间发现的 checkpoint 测试全局故障注入竞态改为 task-local，生产环境故障注入接口保持兼容。
6. 两个镜像固定审查过的 Debian bookworm-slim digest
   `sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171`。
   `.dockerignore` 排除本地 node_modules、dist 和 Python 缓存，避免开发工具或旧产物被复制进镜像；
   三套网页 dist 继续来自独立 web-builder 的干净构建。

## 例外规则

机器可读记录位于 `.github/security/image-vulnerability-exceptions.json`；没有通配符包忽略。
每条登记包括 CVE、精确包/版本、severity、镜像、实际依据、前提、负责人、修复方向和期限。
当前机制只允许 Debian OS 告警匹配这些例外；Rust、其他语言和静态依赖告警始终按自身结果处理。

除逐条字段外，还必须满足：

- release ref 为 `refs/tags/usdb-testnet-vN-rN`，不适用于 mainnet、普通分支或任意镜像；
- Debian 12.15 / linux/amd64，维持原始审查的功能与部署范围；
- Dockerfile、锁文件、运行源码、脚本和 Compose 等选定路径的文件名、内容、可执行位指纹一致；
- 处置日期已生效且当天 UTC 日期严格早于 expires_at；期限最多 30 天；
- 扫描没有新提供 FixedVersion；有发行版修复时要求升级或重新审查。

不存在目录时按零例外执行，不会自动放宽任何条目。目录损坏、字段缺失、重复条目、错误镜像 digest/
源码 revision、缺少报告或二进制依赖元数据均失败。扫描原始 JSON/SARIF 不过滤，分别记录原始数量、
接受数量、未处置数量；`policy-decision.json` 和例外目录快照随 metadata/checksums 一并归档。
正式严格发布对未处置 High/Critical 失败；原有人工 dispatch report-only 语义保持。

本登记的已知前提：标准 Compose、no-new-privileges、非 privileged/无 SYS_ADMIN、
可信签名快照与本地数据；未启用恶意输入驱动的 Perl/XML/FTS5/LDAP/特殊 curl 认证/挂载路径。
不得因这里的例外启用任意数据库上传或放松 snapshot 签名验证。services 当前默认 root 的收敛是后续事项。

## 逐 CVE 处置

全部包名/版本对应关系请以目录为准。下表摘要源于上一轮原始报告、确切镜像组件检查及 Debian/上游公告。

| CVE | 级别 | 决策 | 依据 |
| --- | --- | --- | --- |
| [CVE-2023-45853](https://security-tracker.debian.org/tracker/CVE-2023-45853) | CRITICAL | not_affected | 漏洞位于 contrib/minizip。Debian 明确注明本版 zlib 不构建该组件；不能把 zlib1g 等同于易受攻击的 MiniZip。 |
| [CVE-2025-7458](https://security-tracker.debian.org/tracker/CVE-2025-7458) | CRITICAL | not_reachable | 系统 SQLite 3.40.1 落在受影响范围，需执行攻击者构造的 SQL/大量排序表达式。实际 Bitcoin 和 USDB 二进制不动态链接系统 libsqlite3；Python 脚本无 sqlite3 使用。源码锁定的 bundled SQLite 3.51.1 已超过本 CVE 上游修复版本。 |
| [CVE-2026-13221](https://security-tracker.debian.org/tracker/CVE-2026-13221) | CRITICAL | not_reachable | 需 Perl 编译超过 65535 个固定分支的正则 trie 并用于安全判断；维护的服务和发布脚本没有该路径。Debian 版本表和 introduced v5.37.10 注释存在矛盾，本结论不依赖版本排除。 |
| [CVE-2026-42496](https://security-tracker.debian.org/tracker/CVE-2026-42496) | CRITICAL | not_affected | 受影响的是 Perl Archive::Tar；两个确切镜像均无法加载该模块。GNU tar、Python tarfile 不属于这个组件。 |
| [CVE-2026-8376](https://security-tracker.debian.org/tracker/CVE-2026-8376) | CRITICAL | not_affected | 该 Perl 正则堆溢出限定 32 位；两个确切镜像均为 amd64，Perl ptrsize=8。 |
| [CVE-2023-2953](https://security-tracker.debian.org/tracker/CVE-2023-2953) | HIGH | not_reachable | OpenLDAP 分配失败检查缺失；由 services 的 curl 依赖引入，维护的应用/下载不使用 LDAP。 |
| [CVE-2025-59375](https://security-tracker.debian.org/tracker/CVE-2025-59375) | HIGH | not_reachable | Expat 的 XML 内存消耗、标签缓冲区溢出和属性哈希碰撞。交付的 Python 运行/安装工具不解析外部 XML，实际服务也不动态链接 libexpat。 |
| [CVE-2025-69534](https://security-tracker.debian.org/tracker/CVE-2025-69534) | HIGH | not_reachable | 涉及 Python HTMLParser 异常或增量 feed 的 CPU DoS；维护的 Python 工具无此解析器调用。 |
| [CVE-2025-69720](https://security-tracker.debian.org/tracker/CVE-2025-69720) | HIGH | not_reachable | 受影响逻辑为 infocmp 的 analyze_string；不能等同于任何链接 libtinfo 的程序。当前服务/脚本不调用 infocmp 处理外部数据。 |
| [CVE-2026-11822](https://security-tracker.debian.org/tracker/CVE-2026-11822) | HIGH | not_reachable | 本例外仅覆盖未被应用调用的 Debian libsqlite3-0；Python 工具不使用 sqlite3。USDB Rust 静态副本已经通过统一 Cargo 配置禁用 FTS5，并有编译选项及创建 FTS5 表失败的回归测试。静态/语言依赖告警不得使用此 OS 包例外。 |
| [CVE-2026-11824](https://security-tracker.debian.org/tracker/CVE-2026-11824) | HIGH | not_reachable | 本例外仅覆盖未被应用调用的 Debian libsqlite3-0；Python 工具不使用 sqlite3。USDB Rust 静态副本已经通过统一 Cargo 配置禁用 FTS5，并有编译选项及创建 FTS5 表失败的回归测试。静态/语言依赖告警不得使用此 OS 包例外。 |
| [CVE-2026-11940](https://security-tracker.debian.org/tracker/CVE-2026-11940) | HIGH | mitigated | tarfile 的 hardlink 指向 symlink 可绕过 extraction filter。install_usdb_node.sh 在 extractall 前遍历全部条目，仅允许普通文件/目录，拒绝软硬链接、绝对路径、.. 和重复条目，目标为新临时目录。 |
| [CVE-2026-12064](https://security-tracker.debian.org/tracker/CVE-2026-12064) | HIGH | not_reachable | curl 上游评为 Low；需 --proto-default sftp/scp 加无 scheme URL。维护的下载调用不使用该组合。 |
| [CVE-2026-15308](https://security-tracker.debian.org/tracker/CVE-2026-15308) | HIGH | not_reachable | 涉及 Python HTMLParser 异常或增量 feed 的 CPU DoS；维护的 Python 工具无此解析器调用。 |
| [CVE-2026-16742](https://security-tracker.debian.org/tracker/CVE-2026-16742) | HIGH | not_affected | 漏洞位于 systemd-homed 的用户/组签名处理；镜像只有相关 systemd 库，没有 systemd-homed 服务程序。 |
| [CVE-2026-25210](https://security-tracker.debian.org/tracker/CVE-2026-25210) | HIGH | not_reachable | Expat 的 XML 内存消耗、标签缓冲区溢出和属性哈希碰撞。交付的 Python 运行/安装工具不解析外部 XML，实际服务也不动态链接 libexpat。 |
| [CVE-2026-3644](https://security-tracker.debian.org/tracker/CVE-2026-3644) | HIGH | not_reachable | 涉及 http.cookies.Morsel 更新/反序列化及 js_output；维护的 Python 工具无这些调用。 |
| [CVE-2026-41992](https://security-tracker.debian.org/tracker/CVE-2026-41992) | HIGH | not_reachable | 需同一次 GNU gzip -d 先处理恶意 LZW 再处理恶意 LZH 文件；维护的脚本无这种调用。Python tarfile 的 gzip 解码不是该 GNU gzip 全局缓冲区路径。 |
| [CVE-2026-42497](https://security-tracker.debian.org/tracker/CVE-2026-42497) | HIGH | not_affected | 受影响的是 Perl Archive::Tar；两个确切镜像均无法加载该模块。GNU tar、Python tarfile 不属于这个组件。 |
| [CVE-2026-45186](https://security-tracker.debian.org/tracker/CVE-2026-45186) | HIGH | not_reachable | Expat 的 XML 内存消耗、标签缓冲区溢出和属性哈希碰撞。交付的 Python 运行/安装工具不解析外部 XML，实际服务也不动态链接 libexpat。 |
| [CVE-2026-48962](https://security-tracker.debian.org/tracker/CVE-2026-48962) | HIGH | not_affected | 受影响的是 IO::Compress 所带 File::GlobMapper 的输出 glob eval；两个确切镜像均无 File::GlobMapper。 |
| [CVE-2026-53613](https://security-tracker.debian.org/tracker/CVE-2026-53613) | HIGH | not_reachable | 分别涉及 fstab user/users 挂载竞态、失败 mount helper 后续特权操作、nsenter --join-cgroup、X-mount.subdir 路径逃逸、restricted bind 的属性修改。镜像 fstab 无条目，脚本不调用这些路径，Compose 未启用 privileged/SYS_ADMIN。同一源包映射到 9 个二进制子包造成重复计数。 |
| [CVE-2026-54369](https://security-tracker.debian.org/tracker/CVE-2026-54369) | HIGH | not_reachable | 需特权程序对攻击者可替换路径调用 acl_*_file 接口；维护的发布/运行脚本没有此类 ACL 操作，服务无 libacl 动态依赖。 |
| [CVE-2026-57432](https://security-tracker.debian.org/tracker/CVE-2026-57432) | HIGH | not_reachable | 需攻击者控制 Perl pack/unpack 模板的巨大重复次数；服务和维护脚本没有这样的 Perl 输入处理。 |
| [CVE-2026-57433](https://security-tracker.debian.org/tracker/CVE-2026-57433) | HIGH | not_affected | 受影响的是 Storable 反序列化 SX_HOOK 记录；两个确切镜像均没有 Storable。 |
| [CVE-2026-6276](https://security-tracker.debian.org/tracker/CVE-2026-6276) | HIGH | not_reachable | curl 上游评为 Low，且明确不影响 curl CLI；需 libcurl easy handle 自定义 Host 后复用。实际 Rust/Ord 二进制不动态链接 libcurl。 |
| [CVE-2026-7210](https://security-tracker.debian.org/tracker/CVE-2026-7210) | HIGH | not_reachable | 涉及 Python XML Expat 的哈希随机化与恶意 XML；维护的 Python 工具不解析该输入。 |
| [CVE-2026-76642](https://security-tracker.debian.org/tracker/CVE-2026-76642) | HIGH | not_reachable | 分别涉及 fstab user/users 挂载竞态、失败 mount helper 后续特权操作、nsenter --join-cgroup、X-mount.subdir 路径逃逸、restricted bind 的属性修改。镜像 fstab 无条目，脚本不调用这些路径，Compose 未启用 privileged/SYS_ADMIN。同一源包映射到 9 个二进制子包造成重复计数。 |
| [CVE-2026-78408](https://security-tracker.debian.org/tracker/CVE-2026-78408) | HIGH | not_reachable | 分别涉及 fstab user/users 挂载竞态、失败 mount helper 后续特权操作、nsenter --join-cgroup、X-mount.subdir 路径逃逸、restricted bind 的属性修改。镜像 fstab 无条目，脚本不调用这些路径，Compose 未启用 privileged/SYS_ADMIN。同一源包映射到 9 个二进制子包造成重复计数。 |
| [CVE-2026-78409](https://security-tracker.debian.org/tracker/CVE-2026-78409) | HIGH | not_reachable | 分别涉及 fstab user/users 挂载竞态、失败 mount helper 后续特权操作、nsenter --join-cgroup、X-mount.subdir 路径逃逸、restricted bind 的属性修改。镜像 fstab 无条目，脚本不调用这些路径，Compose 未启用 privileged/SYS_ADMIN。同一源包映射到 9 个二进制子包造成重复计数。 |
| [CVE-2026-78410](https://security-tracker.debian.org/tracker/CVE-2026-78410) | HIGH | not_reachable | 分别涉及 fstab user/users 挂载竞态、失败 mount helper 后续特权操作、nsenter --join-cgroup、X-mount.subdir 路径逃逸、restricted bind 的属性修改。镜像 fstab 无条目，脚本不调用这些路径，Compose 未启用 privileged/SYS_ADMIN。同一源包映射到 9 个二进制子包造成重复计数。 |
| [CVE-2026-8286](https://security-tracker.debian.org/tracker/CVE-2026-8286) | HIGH | not_reachable | curl 上游评为 Low；需 IMAP/POP3/SMTP/FTP/LDAP 等 STARTTLS 连接在不同 TLS 配置间复用。当前 HTTPS 下载未使用该路径。 |
| [CVE-2026-8458](https://security-tracker.debian.org/tracker/CVE-2026-8458) | HIGH | not_reachable | curl 上游评为 Low；需 HTTP Negotiate 在不同 service name 下复用连接。维护的调用未使用 Negotiate。 |
| [CVE-2026-8927](https://security-tracker.debian.org/tracker/CVE-2026-8927) | HIGH | not_reachable | curl 上游评为 Medium，明确不影响 CLI；需同一 libcurl handle 经环境变量切换 Digest 代理。实际服务未链接 libcurl。 |
| [CVE-2026-9538](https://security-tracker.debian.org/tracker/CVE-2026-9538) | HIGH | not_affected | 受影响的是 Perl Archive::Tar；两个确切镜像均无法加载该模块。GNU tar、Python tarfile 不属于这个组件。 |


## 后续更新和发布

修改审查范围中的文件会令既有例外失效。维护者先审查对可达性和防护的影响，再用以下命令对目录中
该 profile 的 source_paths 重新计算指纹，更新 source_sha256；不要自动更新指纹或顺延日期：

```bash
python3 .github/scripts/image_security_policy.py fingerprint <source_paths...>
python3 tests/test_image_security_policy.py
```

测试 fixtures 是 r16 真实报告的 High/Critical 最小投影，包含原完整报告哈希。
它们用于验证规则，不是修复镜像的扫描证明。原始证据 artifact：
[Bitcoin](https://github.com/buckyos/usdb/actions/runs/34121165089/artifacts/10019062618)、
[services](https://github.com/buckyos/usdb/actions/runs/34121165089/artifacts/10019623037)。

原 r16 tag 的 workflow/源码不会因重跑失败 job 自动更新。合入本批改动后应创建新的、递增的正式 release tag，
让镜像 producer 重新构建、attest 并扫描；candidate resolver 继续只选择同 tag/revision 上成功的 producer run。
不得移动既有正式 tag 或绕过 candidate 的成功运行校验。GitHub 上的正式发布资格必须以新运行结果为准。

## 验证记录

- 工作区 `cargo check --workspace`、严格 Clippy、全量测试通过；FTS5 拒绝测试通过。
- 两个实际锁文件 cargo-audit 漏洞条目为 0；保留的维护性 warnings 未登记为 OS 例外。
- 例外正向/反向、日期边界、输入身份、Rust 覆盖缺失等 25 项测试通过。
- 安装器测试通过，包含软/硬链接拒绝；workflow actionlint 和 release fragment schema 检查通过。
- Bitcoin 原始 digest 使用同版 Trivy 真实复扫，5 Critical / 78 High 与原始报告一致。
- 最终本地 Bitcoin 和 services 镜像均构建成功；新镜像 Debian 12.15 的 High/Critical
  包、版本、severity、FixedVersion 多重集合与原审查基线完全一致，无新增 OS 项。
- services 实际包含 5 个非空 Rust 二进制依赖清单，语言依赖 High/Critical 为 0。
  Ord 版本仍为 0.23.3，5 个二进制 `--help` 通过，3 个数据库工具无 ENABLE_FTS5 标识。
  三套网页 index.html 均存在，镜像没有 node_modules。
- 完整 Fast CI 最后一轮通过。真实 Bitcoin 报告重放了 workflow 的分类、metadata v2、
  SHA256SUMS 和严格 enforcement：83 条接受、0 条未处置，原始 JSON 保持不变。
- [机器可读验证记录](release-image-2026-09-07-validation.json)保存本地 image ID 和报告哈希。
  这些是未提交工作区的本地构建，不是 GHCR 发布 digest 或新 release tag 的 GitHub 资格证明。
  当前两个仓库最新 tag 均为 r16；提交后可使用新的 r17 运行正式构建与扫描。
