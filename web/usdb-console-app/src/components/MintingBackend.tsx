import { useEffect, useState } from 'react'
import { useI18n } from '../i18n/provider'
import type { MonitorSnapshot } from '../lib/types'

const states: Record<string, [string, string]> = {
  DISABLED: ['未启用', 'Disabled'], WAITING_CORE: ['等待 Bitcoin 前台追平', 'Waiting for Bitcoin foreground'],
  WAITING_HISTORY: ['等待 Bitcoin 历史校验', 'Waiting for historical validation'],
  WAITING_TXINDEX: ['等待交易索引追平', 'Waiting for txindex'], BLOCKED_DISK: ['磁盘余量不足，Ord 已暂停', 'Ord paused: low disk space'],
  BLOCKED_CONFIG: ['Bitcoin 配置不兼容', 'Incompatible Bitcoin configuration'],
  STARTING: ['Ord 启动中', 'Ord starting'], INDEXING: ['Ord 索引／规范链校验中', 'Ord indexing / checking canonical chain'],
  READY: ['索引后端已就绪', 'Index backend ready'], UNAVAILABLE: ['当前状态未知', 'Current state unknown'],
  FAILED: ['Ord 运行失败', 'Ord failed'], STOPPED: ['Ord 已停止', 'Ord stopped'],
}

/** Optional dependencies never redefine consensus/mining readiness. */
export function MintingBackend({ snapshot }: { snapshot?: MonitorSnapshot }) {
  const { locale } = useI18n()
  const zh = locale === 'zh-CN'
  const text = (cn: string, en: string) => zh ? cn : en
  const value = snapshot?.report?.minting
  const [elapsed, setElapsed] = useState(0)
  useEffect(() => {
    const received = performance.now()
    setElapsed(0)
    const timer = window.setInterval(() => setElapsed(performance.now() - received), 1000)
    return () => window.clearInterval(timer)
  }, [snapshot])
  const age = (snapshot?.age_ms ?? Infinity) + elapsed
    + Math.max(0, (snapshot?.report?.observed_at_ms ?? 0) - (value?.observed_at_ms ?? 0))
  const fresh = snapshot?.status === 'available' && (value?.enabled === false || age <= 60000)
    && (snapshot.age_ms ?? Infinity) + elapsed <= 120000
  const state = fresh ? value?.state ?? 'UNAVAILABLE' : 'UNAVAILABLE'
  const size = (bytes?: number) => bytes == null ? '—' : `${(bytes / 1024 ** 3).toFixed(1)} GiB`
  return <section className="console-card grid gap-3" aria-label={text('本机铸造后端', 'Local minting backend')}>
    <div className="flex flex-wrap justify-between gap-2"><h2 className="font-semibold">{text('本机铸造后端（可选）', 'Local minting backend (optional)')}</h2><strong role="status">{states[state]?.[zh ? 0 : 1] ?? state}</strong></div>
    <p className="text-sm">{text('Ord 的等待或故障不影响节点同步、挖矿及已有矿工证查询。正式钱包签名和广播尚未开放，后端就绪不代表可以提交铸造交易。', 'Ord waiting or failure does not block node synchronization, mining, or existing miner-pass queries. Production signing and broadcast are not enabled; backend readiness does not enable mint transactions.')}</p>
    {value?.enabled && <>
      {!fresh && <p role="alert">{text('以下为最后一次观测，不能用于判断当前能力。', 'The values below are the last observation and do not establish current readiness.')}</p>}
      <dl className="grid gap-3 text-sm sm:grid-cols-3">
        {[[text('Bitcoin 前台高度', 'Bitcoin foreground height'), value.core_height],
          [text('历史校验高度', 'Historical validation height'), value.history_height],
          [text('交易索引高度', 'Txindex height'), value.txindex_height],
          [text('Ord 已索引高度', 'Ord indexed height'), value.ord_height],
          [text('Ord 落后区块', 'Ord block gap'), value.ord_gap],
          [text('Ord 数据库文件大小', 'Ord database file size'), size(value.index_file_bytes)],
          [text('可用磁盘', 'Free disk'), size(value.disk_free_bytes)],
          [text('磁盘保留阈值', 'Free disk reserve'), size(value.disk_required_bytes)]].map(([label, item]) =>
            <div key={String(label)}><dt className="text-[color:var(--cp-muted)]">{label}</dt><dd>{item ?? '—'}</dd></div>)}
      </dl>
    </>}
    {state === 'DISABLED' && <p className="text-sm">{text('可在 setup 中选择启用；已有节点先 down，再运行 set-minting --enabled on 和 up。', 'Enable in setup, or stop an existing node, run set-minting --enabled on, then up.')}</p>}
    {state === 'WAITING_HISTORY' && <p className="text-sm">{text('等待历史区块补齐和校验，无需重启或重新导入快照。', 'Wait for historical blocks and validation; no restart or snapshot reimport is needed.')}</p>}
    {state === 'WAITING_TXINDEX' && <p className="text-sm">{text('等待 txindex 覆盖前台高度；若索引始终不存在，请检查 Bitcoin 是否已采用 BTC_TXINDEX=1。', 'Wait for txindex to cover the foreground tip. If absent, verify Core adopted BTC_TXINDEX=1.')}</p>}
    {state === 'BLOCKED_DISK' && <p className="text-sm">{text('增加容量或清理无关文件后自动恢复；不要删除 Bitcoin 或 Ord 索引。', 'Add capacity or free unrelated files to resume automatically. Preserve Bitcoin and Ord indexes.')}</p>}
    {['FAILED', 'UNAVAILABLE', 'BLOCKED_CONFIG'].includes(state) && <p className="text-sm">{text('在节点执行 usdb-node minting-status --json，并检查 usdb-node logs ord-server。', 'Run usdb-node minting-status --json and inspect usdb-node logs ord-server.')}</p>}
  </section>
}
