import { useEffect, useRef, useState } from 'react'
import { executeBtcMint, fetchBtcWorldSimIdentities, prepareBtcMintDraft, verifyBtcMint } from '../lib/api'
import type { BtcMintPrepareRequest, BtcMintPrepareResponse, BtcMintVerifyResponse, BtcWorldSimIdentity } from '../lib/types'

const field = 'min-w-0 w-full rounded-xl border border-[color:var(--cp-border)] bg-white px-3 py-2 text-sm'
const button = 'rounded-xl border border-[color:var(--cp-border)] px-4 py-2 text-sm font-semibold disabled:opacity-50'

/** Public networks only prepare and verify. Local Ord broadcasting is a separate development capability. */
export function MinerPassMint({ locale, initialSource = '', development = false }: {
  locale: string; initialSource?: string; development?: boolean
}) {
  const text = (cn: string, en: string) => locale === 'zh-CN' ? cn : en
  const [source, setSource] = useState(initialSource)
  const [recipient, setRecipient] = useState('')
  const [kind, setKind] = useState<'usdb_main' | 'leader_pass_id' | 'leader_btc_addr'>('usdb_main')
  const [binding, setBinding] = useState('')
  const [prev, setPrev] = useState('')
  const [id, setId] = useState('')
  const [outpoint, setOutpoint] = useState('')
  const [wallet, setWallet] = useState('')
  const [wallets, setWallets] = useState<BtcWorldSimIdentity[]>([])
  const [prepared, setPrepared] = useState<BtcMintPrepareResponse | null>(null)
  const [verified, setVerified] = useState<BtcMintVerifyResponse | null>(null)
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const epoch = useRef(0)
  const executing = useRef(false)
  useEffect(() => () => { epoch.current++ }, [])
  useEffect(() => {
    const invalidate = () => {
      if (executing.current) return
      epoch.current++; setPrepared(null); setVerified(null); setBusy('')
    }
    window.addEventListener('focus', invalidate)
    return () => window.removeEventListener('focus', invalidate)
  }, [])
  useEffect(() => {
    if (!verified?.verified) return
    const timer = window.setTimeout(() => {
      setVerified(null)
      setNotice(text('核验观测已过期，入金前请再次核验。', 'Verification expired. Verify again before funding.'))
    }, 30000)
    return () => window.clearTimeout(timer)
  }, [verified, locale])

  function edit(change: () => void, draft = true) {
    epoch.current++; change(); setVerified(null); setError(''); setNotice(''); setBusy('')
    if (draft) { setPrepared(null); setId(''); setOutpoint('') }
  }
  function request(): BtcMintPrepareRequest {
    const identity = kind === 'usdb_main' ? { usdb_main: binding.trim() }
      : kind === 'leader_pass_id' ? { leader_pass_id: binding.trim() } : { leader_btc_addr: binding.trim() }
    return { source_address: source.trim(), recipient_address: recipient.trim(), prev: prev.split(/[\s,]+/).filter(Boolean), ...identity }
  }
  async function run(action: 'prepare' | 'verify' | 'execute' | 'wallets') {
    const revision = ++epoch.current
    executing.current = action === 'execute'
    setBusy(action); setError(''); setNotice(''); setVerified(null)
    if (action === 'prepare') setPrepared(null)
    try {
      if (action === 'prepare') {
        const result = await prepareBtcMintDraft(request())
        if (epoch.current === revision) setPrepared(result)
      } else if (action === 'verify') {
        const result = await verifyBtcMint({ mint: request(), inscription_id: id.trim(), expected_source_outpoint: outpoint.trim() || undefined })
        if (epoch.current === revision) setVerified(result)
      } else if (action === 'wallets') {
        const result = await fetchBtcWorldSimIdentities()
        if (!result.available) throw new Error(result.error || "Development wallet identities unavailable")
        if (epoch.current === revision) setWallets(result.identities)
      } else {
        const result = await executeBtcMint({ ...request(), wallet_name: wallet })
        if (epoch.current === revision) {
          setId(result.inscription_id); setOutpoint(result.source_outpoint); setPrepared(null)
          setNotice(text('已广播，尚未核验；请勿入金。等待确认后核验下方铭文 ID。', 'Broadcast, unverified; do not fund. Wait for confirmations and verify the ID below.'))
        }
      }
    } catch (failure) {
      if (epoch.current === revision) {
        setError(failure instanceof Error ? failure.message : String(failure))
        if (action === 'execute') setNotice(text('执行结果未知时，先检查 Ord 钱包和交易记录，避免重复铸造。', 'If execution status is unknown, inspect the Ord wallet and transactions before retrying.'))
      }
    } finally { if (epoch.current === revision) { setBusy(''); executing.current = false } }
  }
  const canRead = Boolean(source.trim() && recipient.trim() && binding.trim())
  const paths = { first_opening: text('首次零余额开户', 'First zero-balance opening'), same_owner: text('同地址操作', 'Same-owner operation'), cross_owner: text('跨地址继承', 'Cross-owner inheritance') }
  return <section className="console-card grid gap-4" aria-label="MinerPass V2">
    <h2 className="text-xl font-semibold">{text('MinerPass V2 铸造与核验', 'MinerPass V2 planning & verification')}</h2>
    <p className="text-sm">{text('铭文 JSON 使用 v: 1；这里的 V2 表示开户、来源与继承规则。请勿把 JSON 的 v 改为 2。', 'Mint JSON uses v: 1. V2 here refers to opening, source and inheritance rules; keep the JSON version at 1.')}</p>
    <p className="text-sm">{text('填写 D / E → 预检并在钱包铸造 → 核验指定铭文 → 按计划入金或迁移余额。', 'Set D / E → Prepare and mint with your wallet → Verify the exact inscription → Fund or migrate as planned.')}</p>
    <p className="text-sm">{text('冷地址请选择全新、未暴露公钥的 hash 地址（推荐原生 P2WPKH）。首次开户不带 prev；跨地址继承填写 D 持有的 Active / Dormant 旧证。E 不需要为草案签名。', 'Choose a fresh hash address (prefer native P2WPKH) with an unexposed public key for cold storage. First opening has no prev; cross-owner inheritance lists Active / Dormant passes owned by D. E need not sign a draft.')}</p>
    <fieldset disabled={busy === 'execute'} className="grid min-w-0 gap-4">
      {development && <div className="grid gap-2">
        <button className={`${button} justify-self-start`} disabled={Boolean(busy)} onClick={() => void run('wallets')}>{text('读取 regtest 开发钱包', 'Load regtest development wallets')}</button>
        <label className="grid gap-1">{text('开发来源钱包', 'Development source wallet')}<select className={field} value={wallet} onChange={event => edit(() => {
          setWallet(event.target.value); setSource(wallets.find(item => item.wallet_name === event.target.value)?.owner_address ?? '')
        })}><option value="">—</option>{wallets.map(item => <option key={item.wallet_name} value={item.wallet_name}>{item.wallet_name}</option>)}</select></label>
      </div>}
      <div className="grid gap-3 md:grid-cols-2">
        <label className="grid gap-1">{text('来源地址 D', 'Source address D')}<input className={field} value={source} onChange={event => edit(() => setSource(event.target.value))} autoComplete="off" spellCheck={false} /></label>
        <label className="grid gap-1">{text('接收地址 E', 'Recipient address E')}<input className={field} value={recipient} onChange={event => edit(() => setRecipient(event.target.value))} autoComplete="off" spellCheck={false} /></label>
      </div>
      <label className="grid gap-1">{text('矿工证类型／绑定', 'Pass kind / binding')}<select className={field} value={kind} onChange={event => edit(() => { setKind(event.target.value as typeof kind); setBinding('') })}>
        <option value="usdb_main">{text('标准证 · USDB 收益账户', 'Standard · USDB beneficiary')}</option>
        <option value="leader_pass_id">{text('协作证 · 固定 Leader 铭文 ID', 'Collaborative · fixed Leader inscription ID')}</option>
        <option value="leader_btc_addr">{text('协作证 · Leader BTC 地址', 'Collaborative · Leader BTC address')}</option>
      </select></label>
      <label className="grid gap-1">{kind}<input className={field} value={binding} onChange={event => edit(() => setBinding(event.target.value))} autoComplete="off" spellCheck={false} /></label>
      <label className="grid gap-1">{text('要继承的 prev（每行一个，首次开户留空）', 'prev to inherit (one per line; empty for first opening)')}<textarea className={field} value={prev} onChange={event => edit(() => setPrev(event.target.value))} /></label>
      <button className={`${button} justify-self-start`} disabled={!canRead || Boolean(busy)} onClick={() => void run('prepare')}>{text('预检并生成草案', 'Prepare draft')}</button>
    </fieldset>
    {prepared && <div className="grid min-w-0 gap-3" data-testid="mint-prepared">
      <p role="status">{prepared.eligible ? text('当前观测允许此草案；尚未铸造或核验。', 'Draft eligible at this observation; not minted or verified.') : text('草案被阻止，请检查原因。', 'Draft blocked; check the reasons.')}{' '}{prepared.operation_path && paths[prepared.operation_path]}</p>
      {prepared.observation && <p className="break-all text-sm">H={prepared.observation.height} · D: {prepared.observation.source_balance_sats} sat · E: {prepared.observation.recipient_balance_sats} sat · {text('E 曾持有效证', 'E ever held a valid pass')}: {String(prepared.observation.recipient_ever_valid_owner)}</p>}
      {prepared.blockers.map((reason, i) => <p role="alert" key={i} className="break-all text-amber-800">{reason}</p>)}
      {prepared.source_passes.length > 0 && <details><summary>{text('来源地址全部矿工证（逐项选择继承）', 'Source passes (select each prev explicitly)')}</summary>{prepared.source_passes.map(pass => <p key={pass.inscription_id} className="break-all text-sm">{pass.inscription_id} · {pass.state}</p>)}</details>}
      <p className="break-all text-sm">{text('未纳入 prev 的 Active / Dormant 证', 'Active / Dormant passes outside prev')}: {prepared.retained_pass_ids.join(', ') || '—'}</p>
      <label className="grid gap-1">{text('钱包铸造内容（完整 JSON）', 'Wallet inscription content (complete JSON)')}<textarea className={`${field} min-h-36 font-mono`} readOnly value={prepared.inscription_payload_json} /></label>
      <p className="text-sm">{text('外部钱包使用完整内容并指定 E 收件，确保铭文 sat 由 D 提供。本页不调用浏览器钱包签名或转账。', 'Use the complete content in your wallet, send to E and ensure D supplies the inscription sat. This page does not request browser-wallet signatures or transfers.')}</p>
      <details><summary>{text('预检注意事项', 'Preparation notes')}</summary>{prepared.warnings.map((warning, i) => <p key={i} className="mt-2 text-sm">{warning}</p>)}</details>
      {development && <button className={`${button} justify-self-start`} disabled={!prepared.eligible || !prepared.execution_available || !wallet || Boolean(busy)} onClick={() => void run('execute')}>{text('使用 regtest Ord 钱包广播', 'Broadcast with regtest Ord wallet')}</button>}
    </div>}
    <fieldset disabled={busy === 'execute'} className="grid min-w-0 gap-3 border-t border-[color:var(--cp-border)] pt-4">
      <label className="grid gap-1">{text('待核验铭文 ID', 'Inscription ID to verify')}<input className={field} value={id} onChange={event => edit(() => setId(event.target.value), false)} autoComplete="off" spellCheck={false} /></label>
      <label className="grid gap-1">{text('预期来源 UTXO（可选 txid:vout）', 'Expected source UTXO (optional txid:vout)')}<input className={field} value={outpoint} onChange={event => edit(() => setOutpoint(event.target.value), false)} autoComplete="off" spellCheck={false} /></label>
      <button className={`${button} justify-self-start`} disabled={!canRead || !id.trim() || Boolean(busy)} onClick={() => void run('verify')}>{text('核验后再入金', 'Verify before funding')}</button>
    </fieldset>
    {busy && <p role="status">{text('正在处理…', 'Working…')}</p>}
    {error && <p role="alert" className="break-all text-amber-800">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {verified && <div className="grid gap-2" data-testid="mint-verification">
      <p role="status" className={verified.verified ? 'text-green-800' : 'text-amber-800'}>{verified.verified ? text('核验通过：在以下观测状态下，可按计划入金或迁移余额。', 'Verified: funding or migration may proceed based on the observation below.') : text('尚未通过核验，请勿入金。', 'Not verified; do not fund.')}</p>
      <p className="text-sm">H={verified.observed_height} · {text('确认数', 'Confirmations')}: {verified.confirmations}/{verified.required_confirmations}</p>
      {verified.blockers.map((reason, i) => <p key={i} className="break-all text-sm">{reason}</p>)}
      <p className="break-all text-sm">D UTXO: {verified.source?.source_outpoint ?? '—'}</p>
      <p className="break-all text-sm">{text('D 观测余额／仍持有的 Active、Dormant 证', 'D observed balance / remaining Active, Dormant passes')}: {verified.source_balance_sats} sat / {verified.remaining_source_pass_ids.join(', ') || '—'}</p>
      <p className="text-sm">{text('观测不包含后续或未确认交易；迁移后检查旧地址余额、找零和所有铭文 UTXO。页面不自动归集资金。', 'Observation excludes later and unconfirmed transactions. Inspect old balances, change and all inscription UTXOs after migration. This page never sweeps funds.')}</p>
    </div>}
    <div className="grid gap-2 text-sm text-[color:var(--cp-muted)]">
      <p>{text('最低承诺：防止无主动操作的既有权益侵害。持证地址请专用于明确的 pass 操作；普通或受诱导付款、错误铸造仍可能改变权益。来源检查不等于对铭文内容的独立授权。', 'Minimum promise: protect existing rights without an active owner operation. Dedicate pass addresses to explicit operations; ordinary or induced payments and incorrect minting can change pass rights. Source checks are not independent consent to inscription content.')}</p>
      <p>{text('Taproot 地址本身公开输出公钥；冷地址轮换只能缩短暴露窗口。当前零余额不能保证 reveal 时仍满足条件。跨地址迁移 Leader 后，协作绑定不会自动跟随，需单独检查。', 'Taproot exposes its output key; cold-address rotation only shortens the exposure window. Zero balance now does not guarantee reveal-time eligibility. Leader migration does not automatically migrate collaborative bindings.')}</p>
    </div>
  </section>
}
