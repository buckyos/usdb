# 通知配置与投递

[核心监控](monitor.md) · [控制台](../services/control-plane.md)

本功能需要配套的新 node-kit 和 `usdb-services` 镜像。monitor 默认持续记录本地事件；
通知默认没有渠道，需要节点管理员配置。通知配置、队列或外部接收端异常均不阻塞本地采集。

## 配置入口

登录私有 control-plane，在“节点监控 → 通知配置与投递”编辑、保存配置。页面支持多个
Webhook/SMTP 渠道、开关、最低严重级别、通知间隔、恢复通知及测试发送。先保存，再测试；
测试需要 monitor 运行，页面显示接收端接受结果或失败原因，不代表运维人员已阅读。

也可以直接编辑 `node.env` 同目录下的 `monitor/notifications/config.json`。此文件与网页
编辑的是同一份配置，目录权限 0700，文件 0600，归节点运维用户所有。保存有效 JSON 后
运行中的通知进程自动应用，无需停节点。语法、字段或权限错误会显示 `CONFIG_INVALID`，
并继续使用上一份有效配置，重启后也保留最后有效配置。

示例（保留所需渠道，替换示例地址与凭据后再启用）：

```json
{
  "schema_version": "usdb-notifications:v1",
  "warning_interval_secs": 1800,
  "critical_interval_secs": 300,
  "notify_recovery": true,
  "channels": [
    {
      "id": "ops-webhook",
      "type": "webhook",
      "enabled": false,
      "min_severity": "warning",
      "url": "https://alerts.example.org/usdb",
      "bearer_token": "",
      "signing_secret": "",
      "allow_http": false
    },
    {
      "id": "ops-email",
      "type": "smtp",
      "enabled": false,
      "min_severity": "critical",
      "host": "smtp.example.org",
      "port": 587,
      "tls": "starttls",
      "username": "node@example.org",
      "password": "",
      "sender": "node@example.org",
      "recipients": ["operator@example.org"]
    }
  ]
}
```

最多 16 个渠道，每个 SMTP 渠道最多 16 个独立收件邮箱；渠道 ID 使用 1–64 位英文字母、
数字、下划线或连字符。通知间隔范围 60–604800 秒。未知字段和不合法的配置拒绝加载。
SMTP 支持 `starttls` 和连接即加密的 `tls`（通常使用 465 端口），校验证书；不自动降级为
明文。邮箱填写纯 ASCII 地址，不包含显示名。认证按邮件提供商要求填写账号和密码/授权码。

Webhook 默认要求 HTTPS 并校验证书；可信内网确需 HTTP 时显式设置 `allow_http: true`。
不跟随重定向，不继承宿主机代理环境。URL 可能包含凭据，因此网页不返回已保存的完整 URL、
token、签名密钥或密码；留空保留已保存值，网页提供清除可选凭据的操作。

控制台只挂载通知专用目录为可写，不获得事件库、通知队列或 `node.env` 的访问权限。
保存时保留宿主机运维用户的文件所有权。手动编辑与网页保存遵循最后一次有效保存生效。

## 通知行为

- info 默认只记录本地事件。
- warning 首次触发时通知，持续故障默认每 30 分钟通知。
- critical 首次和升级时立即通知，持续故障默认每 5 分钟通知。
- 新渠道不补发旧事件，也不立即扫描补发活动告警；之后正常产生的周期通知会包含它。
- 不需要 ack 或人工消警。故障获得连续有效恢复证据后，自动结束告警，停止故障提醒。
- 默认向已经关联该告警的有效渠道发送一次恢复通知；可关闭 `notify_recovery`。
- 尚未发送的重复提醒合并；恢复后取消未完成的故障提醒。已发送的通知不能撤回。
- 采集未知、过期或暂时失联不等于恢复；通知保留最近观察时间及 condition，便于识别旧证据。
- 禁用/删除渠道或修改投递目标会取消该渠道旧的待发任务，不转发到新地址；仅轮换认证凭据
  可继续重试原目标。新渠道和重新启用的渠道都从后续通知开始。

本地事件只在关键变化时追加，周期提醒不制造新的告警生命周期，也不重复写入相同故障事件。
升级后保留第一批产生的事件、告警和旧确认审计记录；旧确认字段不再影响通知或恢复。

## 接收协议与队列

Webhook 使用 JSON POST，正文 schema 为 `usdb-node-notification:v1`，包含稳定节点 ID、
网络身份、通知 ID、告警 ID、事件 ID（周期提醒可为空）、服务、代码、严重级别、生成时间、
最近观察时间及白名单证据。`kind` 为 `firing`、`escalated`、`ongoing`、`resolved` 或 `test`。

请求包含 `X-USDB-Notification-ID`。配置签名密钥时，`X-USDB-Timestamp` 为发送时的 Unix 秒，
`X-USDB-Signature` 为 `sha256=` 加十六进制 HMAC-SHA256，签名输入是 `timestamp + "." + 原始正文`。
接收端应校验签名、时间窗口，并用通知 ID 去重；同一任务重试的通知 ID 和正文保持不变。
HTTP 2xx 表示接收端已接受；408/425/429、5xx 和连接失败自动重试，其余拒绝结果记录失败。

SMTP 每个收件人独立投递和记账，成功的收件人不随其他人的失败重试。重试保留相同 Message-ID。
邮件正文包含同一份结构化告警信息。服务器接受邮件不保证邮件最终进入收件箱。

队列保存在 `monitor/notifications.sqlite3`，与 `events.sqlite3` 分开，绑定稳定节点 ID。
事件消费位置和新任务事务提交。发送在独立进程中执行，最多四个渠道并行、每渠道一个发送任务；
每次尝试最多 30 秒，网络操作单次超时 10 秒。发生未知结果时允许重复发送，不承诺 exactly-once。

失败按 30 秒起步退避，普通退避上限 30 分钟；有效 Retry-After 最多延后 24 小时。
临时失败最多自动重试 72 小时；认证、地址、协议等明确拒绝记录失败，持续故障的下一次正常
周期通知仍会按配置产生。终态记录保留最多 30 天/20000 条，待发上限 4096 条。
达到上限或消费源历史已被清理时，状态中明确显示计数/缺口，不无限占用磁盘。

监控快照升级为 `usdb-node-monitor:v2`，`notifications` 从旧版占位字符串变为状态对象；
事件 schema 和事件数据库仍为版本 1，新控制台也兼容旧快照。
`usdb-node monitor status --json` 的 `notifications` 展示渠道最近结果、最近成功时间、
待发送/失败/过期数量、配置错误、历史缺口及容量计数。完整事件仍通过 `monitor events` 查询。
通知失败不会再递归产生外发通知。

## 升级、停止与排障

通知工作进程归同一个 `usdb-node-monitor-<bundle-id>.service` 管理，不需要额外安装服务。
显式 down 停止发送并保留队列；up 恢复处理。升级沿用 down → activate/setup → up，
升级前按[核心监控备份说明](monitor.md#事件存储与备份)备份整个 monitor 目录。
回退第一批版本时通知会停止，本地事件库仍是兼容的版本 1；旧版本的人工恢复语义可能重新生效。

配置页面不可用通常意味着 node-kit、服务镜像或专用配置挂载尚未一起更新。查看：

```bash
usdb-node monitor status --json
systemctl status usdb-node-monitor-<bundle-id>.service
journalctl -u usdb-node-monitor-<bundle-id>.service -n 100
```

出现 `CONFIG_INVALID` 检查 JSON、字段、权限和所有者。HTTP_401/403 或 SMTP_535 检查凭据；
HTTP_429 或 SMTP_4xx 为暂时拒绝；CONNECTION_FAILED 检查 DNS、连通性和证书；SEND_TIMEOUT
表示发送超过总时间预算。修正后可用页面测试。若测试请求一直等待，先确认 monitor 正在运行；
请求超过五分钟后由工作进程丢弃。服务停止期间只保存配置，不会替你发送测试通知。
数据库损坏或版本不支持时保留文件并排查，不自动清空队列；本地事件记录继续运行。
