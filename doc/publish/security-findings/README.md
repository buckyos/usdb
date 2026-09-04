# USDB Security Finding Registry

本目录保存人工审计和自动扫描结果的稳定分类记录。扫描器中的 alert 状态不是发布结论；
发布 reviewer 必须能从这里确认输入是否可控、问题是否进入发布 artifact、采取了什么措施，
以及什么变化会触发重新审计。

## 记录规则

每个 finding 使用稳定的 `finding_id`，至少记录：

- `source` 与原始 alert/报告链接；
- `component`、`introduced_by` 和受影响 artifact；
- `reachability`、`exposure` 与 `impact`；
- `decision`、证据、验证状态和剩余工作；
- `owner`、`expires_at` 与 `release_gate`。

允许的 `decision` 为 `fix`、`upgrade`、`replace`、`mitigate`、
`accept-temporarily` 和 `false-positive`。`false-positive` 只表示告警规则不适用于该语义，
不表示对应组件已经完成完整安全审计。

以下变化必须重新检查相关 finding：

- 输入从本地配置变成远程请求或链上数据；
- 路径、URL、认证、feature、target 或发布镜像发生变化；
- 测试代码进入 release binary/image；
- 协议规定的摘要、签名或认证格式发生变化；
- CodeQL query、编译器或依赖 major version 更新。

## 当前基线

- [CodeQL 初始基线（2026-09-04）](./codeql-baseline-2026-09-04.md)
- [Snapshot/checkpoint 安装边界审计（2026-09-04）](./snapshot-checkpoint-install-audit-2026-09-04.md)

基线文件记录一次不可变扫描事实；后续修复进度可以更新，但新一轮全量扫描应新增日期文件，
不要覆盖旧基线。
