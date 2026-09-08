# 2026-09-08 发布镜像增量安全复审

本记录承接 [2026-09-07 审查](release-image-2026-09-07.md)，用于准备下一次正式测试网发布。
审查基线为 `dd391e3`，本次源码基点为 `c050ab9`，另包含本批 Fast CI 指纹预检查改动。
最终输入以例外目录的 `source_sha256` 及配套验证记录为准；这里不宣称 r20 已构建或取得 GitHub 发布资格。

## r18 失败原因

[r18 运行](https://github.com/buckyos/usdb/actions/runs/34229204372)指向 `e49b74f`。
失败发生在 Rust fast gate 的历史镜像报告测试，两个镜像 producer 均被跳过。
services 登记指纹为 `af8c300e63f79516f3447d5405e4062b94ab1c1a740ff053c3655868b7a6f28e`，
r18 源码指纹已变为 `716a8df10048442b437583a1bac88f84ad54ac0a121a2e81a8ef5a2cd9794d8e`。
历史报告的 5 Critical / 89 High 因同一个范围变化原因全部失去例外；这不是 r18 新镜像扫描结果。
Bitcoin 的审查输入未变。

## 增量范围与结论

从基线到源码基点，services 审查范围内共 12 个文件变化；本批另修改
`src/btc/scripts/run_fast_ci.sh`，只增加只读指纹检查。Dockerfile、基础镜像引用、APT 包选择、
Rust/Ord 锁文件、Rust 服务源码、网页源码和运行 Compose 均未变化。

| 变化 | 安全影响及检查结论 |
| --- | --- |
| `chain_file_inspection.py`、`usdb_mining.py` | 权限不足时通过已缓存、固定的 chain 镜像读取本节点数据。探针禁网、只读根文件系统和挂载，丢弃全部 capabilities 后仅增加 `DAC_READ_SEARCH`，保留 `no-new-privileges`，有进程、内存、CPU 和时间限制。只输出元数据和 SHA-256，不输出 nodekey；失败不会作为未初始化或已恢复成功。此操作依赖已有 Docker 管理权限，不能用于不可信用户指定的任意主机路径。 |
| `check_explorer_rpc.py` | 仅允许列出的读取类 RPC，HTTP(S) 地址拒绝内嵌凭据、query 和 fragment，禁止重定向，限制响应大小；trace 需显式选择。先核验链身份，再采样；失败、重组及缺失 receipt 不应形成成功证据。解析 JSON，不引入 XML/HTML、系统 SQLite 或特殊 libcurl 认证路径。 |
| `usdb_sourcedao.py` | 新增展示从本地 journal 白名单提取的交易/receipt 元数据，检查 ceremony identity；不输出已签名交易或私钥，不修改恢复文件。进度和任务完成状态仍与链上验收分开，旧任务、RPC/日志读取失败不会变成已完成。 |
| `usdb_node.py` | 最新高度与哈希取自同一 block 对象，并维持链身份检查；增加宿主机镜像源参数传递和外部服务内存预算。未增加签名或公开管理 RPC。 |
| `prepare_usdb_host.sh` | Ubuntu 26.04 支持与 Docker 官方安装范围一致。新增下载重试及官方源到 TUNA 的有限回退；curl 禁用配置文件且只允许 HTTPS，签名公钥固定 SHA-256，APT 使用 `Signed-By`，密钥不匹配即失败，不降级为未认证安装。先完成下载再安装，dpkg/本地写入失败不会触发源回退；已有完整 Docker 安装保持原状。这里审查的是宿主机工具，不将 Debian 镜像例外扩展到 Ubuntu 宿主机包。 |
| `resource_policy.py` | 将外部服务预算计入所有运行阶段及实际并发容器核算，保留物理主机系统预留量和最小节点内存要求。预算非法或超额时拒绝执行。 |
| `prepare_release_node_kit.py` | 将新增链文件检查和 RPC 探测模块加入 node-kit；独立 public-services 发行不混入 node-kit。 |
| 四个对应测试文件、Fast CI runner | 测试和检查路径的变更仍参与原有目录指纹，不缩小既有审查范围，也不使测试脚本自动更新例外。 |

依据：[Docker 安装文档](https://docs.docker.com/engine/install/ubuntu/)、
[Docker 运行权限文档](https://docs.docker.com/engine/containers/run/)、
[curl CVE-2026-12064](https://curl.se/docs/CVE-2026-12064.html)、
[Debian CVE-2026-53613](https://security-tracker.debian.org/tracker/CVE-2026-53613)。
上述判断结合实际差异和回归测试作出，并不只依赖文本匹配。

## 既有例外适用性

本次可更新 services 的源文件指纹，沿用原 35 个 CVE 的精确包/版本/severity 和处置依据。
不新增 CVE 例外，不扩展镜像、平台或正式测试网 tag 范围；所有原始 `reviewed_at`、
`expires_at` 和逐条条件保持原值，仍在 **2026-10-07 00:00 UTC** 失效。

- 新增路径没有使用 Perl、HTMLParser、Expat/XML、LDAP、特殊 curl 认证或 `--proto-default`。
- 系统 SQLite 仍未被这些 Python 工具调用；Rust bundled SQLite 的禁用 FTS5 配置未变。
- 安装器对软/硬链接、绝对路径、路径穿越和重复归档条目的拒绝未放宽。
- 容器运行配置未开启 privileged 或 `SYS_ADMIN`，没有新增容器内 mount/nsenter/fstab 路径。
  只读链探针额外的 `DAC_READ_SEARCH` 仅用于既有本地数据读取，不等于授予挂载权限。
- 新漏洞、新 FixedVersion、包版本、架构、OS、源码范围、日期或静态/语言依赖变化继续阻断严格发布。

仓库新增的 `public-services` 和 RPC gateway 使用独立 `usdb-public-v*` 发布流程，未复制到这两个受审镜像，
也不继承本目录的例外。本记录不替代它们的独立镜像和公共网络验收。

## 门禁改进

`image_security_policy.py check-scope` 在 GitHub 安装 Rust 工具链前、独立 Fast runner 编译前执行。
输入变化时输出每个镜像的预期/实际 SHA-256、审查路径和记录位置，并非打印 94 条重复 CVE 断言。
该命令只读，不更新指纹、到期日或目录。实际扫描的 `policy-decision.json` 也包含源范围对照。
历史报告测试保留全部数量和严格分类断言；缺失文件、空 profiles 和真实范围变化仍失败。

## 验证与发布

验证通过：

- 完整 Fast CI：674 项 Rust 测试、486 项 Python 测试通过；10 项既有 Rust 测试保持 ignored。
  Rust 格式检查和严格 Clippy 通过。镜像策略测试共 29 项，包含真实 Fast runner 在编译前拒绝过期指纹的回归。
- 增量专项 8 组、111 项测试通过，覆盖密钥不匹配、源回退失败、容器检查权限、私密 journal、
  RPC 身份/重组/只读方法、资源预算、node-kit 打包及归档链接拒绝。
- services 当前输入本地构建成功；以当天更新的 Trivy 0.74.0 数据库复扫两个本地镜像。
  Bitcoin 5 Critical / 78 High、services 5 Critical / 89 High 的 CVE/包/版本/severity/FixedVersion
  多重集合与原审查基线完全相同；语言依赖 High/Critical 均为 0，services 的 5 个 Rust 二进制有非空依赖清单。
  原始扫描报告不改写，历史报告严格分类重放分别为 83/94 条接受、0 条未处置。
- 禁网只读临时容器内核对 12 个变化脚本的实际 SHA-256；5 个二进制及 3 个辅助命令的 `--help`
  成功，3 个网页入口存在，无 node_modules。官方 Ubuntu/Debian 和 TUNA 的公开 Docker 签名密钥
  实测均与脚本固定值一致。

具体验证结果和输入/报告摘要见配套[机器可读记录](release-image-2026-09-08-validation.json)。
本地镜像与扫描用于复核当前输入，不等于带有正式 tag 的 GHCR 发布证明。
合入验证通过的改动后，再创建新的递增正式 tag；原 r18 不会因重跑而取得新代码。
最终仍要求该 tag 的 fast gate、镜像构建、attestation、完整 Trivy 扫描和严格策略全部通过。
