import { mintingLabel, mintingTone, useMintingObservation } from '../lib/minting'
import { useI18n } from '../i18n/provider'
import type { MonitorSnapshot } from '../lib/types'

/** Optional dependencies never redefine consensus/mining readiness. */
export function MintingBackend({ snapshot }: { snapshot?: MonitorSnapshot }) {
  const { locale } = useI18n()
  const zh = locale === 'zh-CN'
  const text = (cn: string, en: string) => zh ? cn : en
  const { value, fresh, state, monitorStatus } = useMintingObservation(snapshot)
  const size = (bytes?: number) => bytes == null ? '—' : `${(bytes / 1024 ** 3).toFixed(1)} GiB`
  return <section className="console-card grid gap-3" aria-label={text('本机铸造后端', 'Local minting backend')}>
    <div className="flex flex-wrap justify-between gap-2"><h2 className="font-semibold">{text('本机铸造后端（可选）', 'Local minting backend (optional)')}</h2><strong role="status" className="status-pill" data-tone={mintingTone(state)}>{mintingLabel(state, locale)}</strong></div>
    <p className="text-sm">{text('Ord 的等待或故障不影响节点同步、挖矿及已有矿工证查询。正式钱包签名和广播尚未开放，后端就绪不代表可以提交铸造交易。', 'Ord waiting or failure does not block node synchronization, mining, or existing miner-pass queries. Production signing and broadcast are not enabled; backend readiness does not enable mint transactions.')}</p>
    {monitorStatus !== 'available' && <p role="alert">{text(
      '监控未启用或观测已过期，不能据此判断 Ord 离线。在节点执行 usdb-node controller install，再执行 usdb-node console start；前台模式可持续运行 usdb-node console monitor。',
      'Host monitoring is missing or unavailable; this does not establish that Ord is offline. Run usdb-node controller install, then usdb-node console start; foreground mode can keep usdb-node console monitor running.',
    )}</p>}
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
    {state === 'WAITING_TXINDEX' && <p className="text-sm">{text('Ord 容器正在等待 txindex 覆盖 Bitcoin 前台高度，Ord 本体及 HTTP 服务尚未启动；这是正常等待，完成后自动开始 Ord 索引，无需重启。若索引始终不存在，请检查 BTC_TXINDEX=1。', 'The container is waiting for txindex to cover the Bitcoin foreground tip; the Ord binary and HTTP server have not started yet. Ord indexing starts automatically when ready; no restart is needed. If txindex is absent, verify BTC_TXINDEX=1.')}</p>}
    {state === 'BLOCKED_DISK' && <p className="text-sm">{text('增加容量或清理无关文件后自动恢复；不要删除 Bitcoin 或 Ord 索引。', 'Add capacity or free unrelated files to resume automatically. Preserve Bitcoin and Ord indexes.')}</p>}
    {['FAILED', 'UNAVAILABLE', 'BLOCKED_CONFIG'].includes(state) && <p className="text-sm">{text('在节点执行 usdb-node minting-status --json，并检查 usdb-node logs ord-server。', 'Run usdb-node minting-status --json and inspect usdb-node logs ord-server.')}</p>}
  </section>
}
