import { useEffect, useState } from 'react'
import { useI18n } from '../i18n/provider'

export type DeliveryStatus = {
  state: string; updated_at_ms?: number; code?: string; config_error?: string | null
  history_gap?: boolean; overflow_count?: number; counts?: Record<string, number>
  channels?: Array<{ id: string; type: string; enabled: boolean; last_success_ms?: number | null; pending_count?: number; failed_count?: number; last_error?: string | null
    latest?: { id: string; state: string; code?: string | null; attempts: number; created_ms: number; next_ms: number } | null }>
}
type Channel = {
  id: string; type: 'webhook' | 'smtp'; enabled: boolean; min_severity: 'warning' | 'critical'
  url?: string; url_set?: boolean; bearer_token?: string; bearer_token_set?: boolean
  signing_secret?: string; signing_secret_set?: boolean; allow_http?: boolean
  host?: string; port?: number; tls?: 'tls' | 'starttls'; username?: string; password?: string; password_set?: boolean
  sender?: string; recipients?: string[]
}
type Config = { schema_version: string; warning_interval_secs: number; critical_interval_secs: number; notify_recovery: boolean; channels: Channel[] }
const empty: Config = { schema_version: 'usdb-notifications:v1', warning_interval_secs: 1800, critical_interval_secs: 300, notify_recovery: true, channels: [] }
async function request(path: string, method = 'GET', body?: unknown) {
  const response = await fetch(`/api/monitor/notifications/${path}`, { method, cache: 'no-store',
    headers: { 'Content-Type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) })
  if (response.status === 401) window.dispatchEvent(new Event('usdb-session-expired'))
  const result = await response.json().catch(() => ({})) as { config: Config; error?: string; request_id?: string }
  if (!response.ok) throw new Error(result.error ?? `HTTP ${response.status}`)
  return result
}

/** Both the file and console edit the host's notification policy. */
export function NotificationSettings({ status }: { status?: DeliveryStatus | string }) {
  const { locale } = useI18n()
  const text = (cn: string, en: string) => locale === 'zh-CN' ? cn : en
  const [config, setConfig] = useState<Config>(empty)
  const [loaded, setLoaded] = useState(false)
  const [busy, setBusy] = useState(false)
  const [dirty, setDirty] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  useEffect(() => {
    let active = true
    request('config').then(result => { if (active) { setConfig(result.config); setLoaded(true) } })
      .catch(e => { if (active) setError(String(e.message)) })
    return () => { active = false }
  }, [])
  const update = (change: Partial<Config>) => { setConfig(value => ({ ...value, ...change })); setDirty(true); setNotice('') }
  const edit = (index: number, change: Partial<Channel>) => update({ channels: config.channels.map((value, i) => i === index ? { ...value, ...change } : value) })
  const add = (type: Channel['type']) => {
    const common = { id: `${type}-${Date.now()}`, type, enabled: true, min_severity: 'warning' as const }
    const channel: Channel = type === 'webhook'
      ? { ...common, url: '', bearer_token: '', signing_secret: '', allow_http: false }
      : { ...common, host: '', port: 587, tls: 'starttls', username: '', password: '', sender: '', recipients: [] }
    update({ channels: [...config.channels, channel] })
  }
  const act = async (channel?: string) => {
    setBusy(true); setError(''); setNotice('')
    try {
      const result = channel ? await request('test', 'POST', { channel }) : await request('config', 'PUT', config)
      if (channel) setNotice(text(`测试请求已提交（${result.request_id}），需要 monitor 正在运行；请查看投递结果。`, `Test requested (${result.request_id}); monitor must be running. Check delivery status.`))
      else { setConfig(result.config); setDirty(false); setNotice(text('已保存；运行中的 monitor 会自动应用。', 'Saved; the running monitor applies changes automatically.')) }
    } catch (e) { setError(e instanceof Error ? e.message : String(e)) }
    finally { setBusy(false) }
  }
  const delivery = typeof status === 'object' ? status : undefined
  const input = 'rounded border border-[color:var(--cp-border)] bg-transparent p-2 w-full'
  const field = (channel: Channel, index: number, key: 'id' | 'host' | 'username' | 'sender' | 'url' | 'bearer_token' | 'signing_secret' | 'password', label: string, secret = false) => {
    const stored = channel[`${key}_set` as keyof Channel] === true
    return <label>{label}<input className={input} type={secret ? 'password' : 'text'} autoComplete={secret ? 'new-password' : 'off'} value={String(channel[key] ?? '')}
      placeholder={stored ? text('已配置，留空保留', 'Configured; leave blank to retain') : ''} onChange={e => edit(index, { [key]: e.target.value })} />
      {stored && key !== 'url' && <button type="button" className="underline" onClick={() => edit(index, { [key]: '', [`${key}_set`]: false })}>{text('清除已保存值', 'Clear saved value')}</button>}</label>
  }
  return <details className="rounded border border-[color:var(--cp-border)] p-4 text-sm">
    <summary className="cursor-pointer font-semibold">{text('通知配置与投递', 'Notification settings and delivery')}</summary>
    <div className="mt-4 grid gap-4">
      <p>{text('故障持续时按级别定期通知，恢复后自动结束。新增渠道只接收之后生成的通知。', 'Ongoing problems notify at the configured interval and resolve automatically. New channels receive future notifications only.')}</p>
      <p>{text('投递进程', 'Delivery worker')}: {delivery?.state ?? '—'} · {text('待发送', 'Pending')}: {delivery?.counts?.pending ?? 0}</p>
      {delivery?.config_error && <p role="alert">{text('通知配置未能应用，继续使用上一份有效配置：', 'Configuration was not applied; the last valid configuration remains active: ')}{delivery.config_error}</p>}
      {(delivery?.history_gap || !!delivery?.overflow_count) && <p role="alert">{text('通知历史存在缺口或队列曾达到容量上限，请检查本地记录。', 'Notification history has a gap or the queue reached capacity. Inspect local records.')}</p>}
      {!!delivery?.channels?.length && <div className="overflow-x-auto"><table className="w-full text-left"><thead><tr>
        <th>{text('渠道', 'Channel')}</th><th>{text('最近投递', 'Latest delivery')}</th><th>{text('结果 / 尝试次数', 'Result / attempts')}</th><th>{text('最近成功', 'Last accepted')}</th>
      </tr></thead><tbody>{delivery.channels.map(channel => <tr key={channel.id}>
        <td className="py-2">{channel.id}</td><td>{channel.latest?.state === 'accepted' ? text('接收端已接受', 'Accepted by receiver') : channel.latest?.state ?? '—'}</td>
        <td>{channel.last_error ?? channel.latest?.code ?? '—'} / {channel.latest?.attempts ?? 0}
          {!!channel.failed_count && <span role="alert"> · {text('失败', 'Failed')}: {channel.failed_count}</span>}</td>
        <td>{channel.last_success_ms ? new Date(channel.last_success_ms).toLocaleString(locale) : '—'}</td>
      </tr>)}</tbody></table></div>}
      {error && <p role="alert" className="break-words">{error}</p>}
      {notice && <p role="status" className="break-words">{notice}</p>}
      {loaded && <fieldset disabled={busy} className="grid gap-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <label>warning {text('通知间隔（秒）', 'interval (seconds)')}<input className={input} type="number" min={60} max={604800} value={config.warning_interval_secs} onChange={e => update({ warning_interval_secs: Number(e.target.value) })} /></label>
          <label>critical {text('通知间隔（秒）', 'interval (seconds)')}<input className={input} type="number" min={60} max={604800} value={config.critical_interval_secs} onChange={e => update({ critical_interval_secs: Number(e.target.value) })} /></label>
        </div>
        <label><input type="checkbox" checked={config.notify_recovery} onChange={e => update({ notify_recovery: e.target.checked })} /> {text('发送一次恢复通知', 'Send a recovery notification')}</label>
        {config.channels.map((channel, index) => <fieldset key={index} className="grid gap-3 rounded border border-[color:var(--cp-border)] p-3">
          <legend>{channel.type === 'smtp' ? text('SMTP 邮件', 'SMTP email') : 'Webhook'}</legend>
          <div className="grid gap-3 sm:grid-cols-2">{field(channel, index, 'id', text('渠道名称', 'Channel ID'))}
            <label>{text('最低通知级别', 'Minimum severity')}<select className={input} value={channel.min_severity} onChange={e => edit(index, { min_severity: e.target.value as Channel['min_severity'] })}><option value="warning">warning</option><option value="critical">critical</option></select></label>
          </div>
          <label><input type="checkbox" checked={channel.enabled} onChange={e => edit(index, { enabled: e.target.checked })} /> {text('启用', 'Enabled')}</label>
          {channel.type === 'webhook' ? <>
            {field(channel, index, 'url', 'URL', true)}
            {field(channel, index, 'bearer_token', text('Bearer token（可选）', 'Bearer token (optional)'), true)}
            {field(channel, index, 'signing_secret', text('HMAC 签名密钥（可选）', 'HMAC signing secret (optional)'), true)}
            <label><input type="checkbox" checked={channel.allow_http} onChange={e => edit(index, { allow_http: e.target.checked })} /> {text('允许明文 HTTP（仅可信网络）', 'Allow unencrypted HTTP (trusted networks only)')}</label>
          </> : <>
            <div className="grid gap-3 sm:grid-cols-2">
              {field(channel, index, 'host', text('SMTP 服务器', 'SMTP host'))}
              <label>{text('端口', 'Port')}<input className={input} type="number" min={1} max={65535} value={channel.port} onChange={e => edit(index, { port: Number(e.target.value) })} /></label>
              <label>{text('连接加密', 'Transport security')}<select className={input} value={channel.tls} onChange={e => edit(index, { tls: e.target.value as Channel['tls'] })}><option value="starttls">STARTTLS</option><option value="tls">TLS</option></select></label>
              {field(channel, index, 'username', text('用户名（可选）', 'Username (optional)'))}
            </div>
            {field(channel, index, 'password', text('密码 / 邮箱授权码', 'Password / app password'), true)}
            {field(channel, index, 'sender', text('发件邮箱', 'Sender mailbox'))}
            <label>{text('收件邮箱（每行一个）', 'Recipients (one per line)')}<textarea className={input} rows={3} value={channel.recipients?.join('\n')} onChange={e => edit(index, { recipients: e.target.value.split('\n') })} /></label>
          </>}
          <div className="flex flex-wrap gap-4">
            <button type="button" className="underline disabled:opacity-50" disabled={dirty || !channel.enabled} onClick={() => void act(channel.id)}>{text('发送测试通知', 'Send test notification')}</button>
            <button type="button" className="underline" onClick={() => update({ channels: config.channels.filter((_, i) => i !== index) })}>{text('删除渠道', 'Remove channel')}</button>
          </div>
        </fieldset>)}
        <div className="flex flex-wrap gap-4">
          <button type="button" className="underline" disabled={config.channels.length >= 16} onClick={() => add('webhook')}>{text('添加 Webhook', 'Add webhook')}</button>
          <button type="button" className="underline" disabled={config.channels.length >= 16} onClick={() => add('smtp')}>{text('添加 SMTP 邮件', 'Add SMTP email')}</button>
          <button type="button" className="rounded border border-[color:var(--cp-border)] px-4 py-2 disabled:opacity-50" disabled={!dirty} onClick={() => void act()}>{text('保存配置', 'Save settings')}</button>
        </div>
        <p>{text('先保存再测试。接收端接受通知不代表收件人已阅读；网络中断时重试可能造成重复通知。', 'Save before testing. Receiver acceptance does not confirm a person read the message; network interruptions may cause duplicate delivery.')}</p>
      </fieldset>}
    </div>
  </details>
}
