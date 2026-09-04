# USDB Release 变更记录与 Changelog 管理

## 1. 目标

一次 USDB release 同时冻结 `usdb`、`go-ethereum` 和 `SourceDAO` 三个仓库。仅使用 Git commit
subject 会遗漏运维影响，仅靠发布负责人手工整理又容易漏项。本方案把变更记录拆成三类互相校验的证据：

1. 开发阶段提交的结构化 change fragment，说明行为、影响范围和运维动作；
2. 前后两份 release manifest 的机器比较，识别网络身份、数据兼容、配置和镜像变化；
3. 三个冻结 Git revision range 的完整 commit inventory，暴露尚未分类的提交。

GitHub 自动生成 Release Notes 可以作为 PR/贡献者视图的补充，但不是 USDB 跨仓发布事实来源。正式
Release 正文和资产必须由 Candidate 中冻结的 revision、manifest 和 fragment 确定性生成。

## 2. Change Fragment

三个仓库统一使用：

```text
.release-notes/fragments/<change_id>.json
```

schema、允许的 type/scope 和示例见仓库根目录
[`.release-notes/README.md`](../../.release-notes/README.md)。核心要求是：

- `change_id` 在三个仓库之间全局唯一，文件名必须与它一致；
- 一项跨仓变更只在主要归属仓库记录一次，并在 `scopes` 中列出全部影响面；
- `details` 描述可观察行为和重要边界，不复述 commit subject；
- `operator_actions` 只记录部署者确实需要执行的动作，没有动作时使用空数组；
- compatibility 四个布尔字段必须保守填写；
- 已经进入发布版本的 fragment 只读，后续 release 不得编辑或删除。

提交可以用一个或多个 trailer 建立 commit 与 fragment 的对应关系；当同一个 commit 确实包含多个可独立
review 的发布影响时，每个 fragment 各写一行：

```text
Release-Note: <change_id>
Release-Note: <another_change_id>
```

同一个 `change_id` 在一个 commit 中不得重复。只有全部 ID 都指向当前 release range 中存在的 fragment，
该 commit 才归类为 `classified`。纯维护提交单独使用：

```text
Release-Note: none
```

`Release-Note: none` 不得与真实 change ID 混用。

首版的 commit coverage 是 report-only。缺少 trailer 不会让 Candidate 失败，但会在 Candidate summary、
`release-changes.json` 和 Release 正文中列为 `unclassified`，发布 review 必须逐项确认。等三仓开发流程
稳定后，再把“每个 commit 必须 classified 或 exempt”升级为阻断门禁。

## 3. Candidate 生成过程

`USDB Release Candidate Manifest` 在现有 manifest 生成完成后执行以下步骤：

1. 从当前 `release_id` 得到 network bundle ID 和 `rN`；
2. 按 release sequence 向前查找同一 bundle 最近一个已发布、非 draft GitHub Release；
3. 下载上一版 `usdb-release-manifest.json` 及 checksum，并在使用前校验；
4. 从前后 manifest 取得三仓 previous/current revision；
5. 要求每仓 previous revision 都是 current revision 的 ancestor，拒绝分叉或回退的发布范围；
6. 只从这些 Git objects 读取新增 fragment 和 commit，不读取 checkout 中未提交内容；
7. 拒绝前后 revision 之间修改或删除既有 fragment；
8. 比较 manifest 中被冻结的 network、runtime compatibility、image、snapshot、qualification 和 source 字段；
9. 生成并复核以下 Candidate 资产。

```text
usdb-release-manifest.json
usdb-release-manifest.json.sha256
release-changes.json
release-changes.json.sha256
release-changes.md
```

如果不存在上一版已发布 Release，则按 initial release 处理：所有当前 fragment 和 Git history 都进入报告，
manifest compatibility comparison 标记为无上一版证据。

## 4. Compatibility 推导

最终分类同时消费 fragment 声明和 manifest 自动比较，采用最保守结果：

| 分类 | 含义 | 典型证据 |
| --- | --- | --- |
| `network_reset` | 不能在现有网络身份上原地升级 | chain ID、genesis、BTC origin/registry 或 bundle identity 变化 |
| `data_rebuild` | 网络不重置，但本地派生数据不兼容 | runtime compatibility ID 或 data layout 变化 |
| `config_change` | 需要修改 operator-owned 配置 | fragment 声明或 snapshot trust catalog 变化 |
| `restart_required` | 替换镜像或重启服务即可 | 任一 digest-pinned runtime image 变化 |
| `in_place` | 没有声明或检测到上述动作 | 纯文档、测试或发布元数据更新 |

`release-changes.json` 同时保留全部布尔 flags，不能只根据单一 classification 丢弃并列要求。例如配置与
重启同时需要时，classification 显示风险更高的一项，但 flags 和 operator actions 会保留两项事实。

Snapshot record 的更新会记录在 manifest diff 中，但 snapshot 是节点可选安装输入，因此不会仅凭它把
已经运行的节点判为必须重配。涉及 snapshot 信任根变化时，trusted-key catalog hash 会独立触发
`config_change`。

## 5. Publish 与 Review

Publish workflow 校验 Candidate artifact digest、两个 JSON checksum，并重新执行
`release_notes.py validate-release`。发布后：

- `release-changes.md` 被合并到 GitHub Release 正文的 deployment identity 之前；
- JSON、JSON checksum 和 Markdown 作为独立 Release assets 长期保留；
- 已存在同名 Release 时，正文和完整资产集合仍必须逐字节一致，否则 fail closed。

批准 Candidate 前至少检查：

1. compatibility classification 是否符合本次升级策略；
2. operator actions 是否完整、顺序是否可执行；
3. security/removed/deprecated 项是否清楚描述；
4. 每个 unclassified commit 是否确实不需要 fragment，必要时在创建下一个 release tag 前补充；
5. 三仓 compare URL 和 previous release 是否指向预期边界；
6. 自动 manifest diff 是否存在未被人工说明的网络身份或数据兼容变化。

## 6. 本地校验

单仓 fragment schema 校验：

```bash
python3 docker/scripts/tools/release_notes.py validate-fragments \
  --repository-root .
```

完整跨仓输出由 Candidate workflow 生成，因为它需要当前 manifest、上一版已发布 manifest 和三个冻结
revision。工具的纯单元测试为：

```bash
PYTHONDONTWRITEBYTECODE=1 \
  python3 docker/scripts/tools/test_release_notes.py
```

## 7. 后续门禁

首版先建立可发布闭环，不立即阻断历史提交。后续可以分两步收紧：

1. 新 PR 要求 fragment 或显式 `Release-Note: none`，但 merge queue 仍只报告；
2. 三仓都稳定执行后，把 `coverage_enforced` 升级到新 schema，并让任何 unclassified commit 阻断
   Candidate。

不能在不升级 schema 和文档的情况下直接改变现有 schema 的 report-only 语义，否则同一 tag 重跑可能产生不同
门禁结果。
