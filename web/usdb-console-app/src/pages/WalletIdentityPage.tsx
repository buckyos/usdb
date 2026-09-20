import { useEffect, useState, type FormEvent, type ReactNode } from 'react'
import { Link, Navigate, NavLink, useParams } from 'react-router-dom'
import useSWR from 'swr'
import { fetchUsdbChainAddressStatus, fetchUsdbOwnerActivePass } from '../lib/api'
import type { OverviewResponse } from '../lib/types'
import {
  bitcoinAddress, bitcoinNetwork, chainId, discoverWallets, evmAddress, genesisHash, nativeBalance, WalletSession,
  type IdentityKind, type WalletAdapter, type WalletState,
} from '../lib/walletIdentity'

const button = 'rounded-xl border border-[color:var(--cp-border)] px-4 py-2 text-sm font-semibold disabled:opacity-50'
const field = 'min-w-0 rounded-xl border border-[color:var(--cp-border)] bg-white px-3 py-2 text-sm'

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return <div className="min-w-0"><dt className="text-sm text-[color:var(--cp-muted)]">{label}</dt><dd className="mt-1 break-all font-mono text-sm">{children ?? '—'}</dd></div>
}

/** The formal page is read-only; developer signing flows live behind a separate route and server flag. */
export function WalletIdentityPage({ data, locale }: { data?: OverviewResponse; locale: string }) {
  const { identityKind } = useParams()
  if (identityKind !== 'usdb' && identityKind !== 'btc') return <Navigate to="/me/usdb" replace />
  const network = data?.node_monitor?.status === 'available' ? data.node_monitor.report?.network : undefined
  // A changed deployment identity remounts the session and discards its pending reads.
  const key = `${identityKind}:${network?.chain_id}:${network?.genesis_hash}:${network?.bitcoin_network}`
  return <IdentityPanel key={key} kind={identityKind} data={data} locale={locale} />
}

function IdentityPanel({ kind, data, locale }: { kind: IdentityKind; data?: OverviewResponse; locale: string }) {
  const zh = locale === 'zh-CN'
  const text = (cn: string, en: string) => zh ? cn : en
  const monitor = data?.node_monitor
  const report = monitor?.report
  const network = monitor?.status === 'available' ? report?.network : undefined
  const targetChain = chainId(network?.chain_id)
  const targetGenesis = genesisHash(network?.genesis_hash)
  const targetBitcoin = bitcoinNetwork(network?.bitcoin_network)
  const [mode, setMode] = useState<'wallet' | 'watch'>('wallet')
  const [adapters, setAdapters] = useState<WalletAdapter[]>([])
  const [selected, setSelected] = useState('')
  const [wallet, setWallet] = useState<WalletState>({ status: 'disconnected', snapshot: null })
  const [session, setSession] = useState<WalletSession | null>(null)
  const [input, setInput] = useState('')
  const [watched, setWatched] = useState('')
  const [invalid, setInvalid] = useState(false)
  const adapter = adapters.find(item => item.id === selected) ?? adapters[0]
  useEffect(() => discoverWallets(kind, setAdapters), [kind])
  useEffect(() => {
    setWallet({ status: 'disconnected', snapshot: null })
    if (!adapter || mode !== 'wallet') { setSession(null); return }
    const controller = new WalletSession(adapter, setWallet)
    setSession(controller)
    const refresh = () => { void controller.refresh() }
    window.addEventListener('focus', refresh)
    return () => { controller.dispose(); window.removeEventListener('focus', refresh) }
  }, [adapter, mode])

  const snapshot = wallet.snapshot
  const expectedKnown = kind === 'usdb' ? Boolean(targetChain && targetGenesis) : Boolean(targetBitcoin)
  const walletKnown = kind === 'usdb' ? Boolean(snapshot?.network && snapshot.genesis) : Boolean(bitcoinNetwork(snapshot?.network))
  const matched = Boolean(snapshot && expectedKnown && walletKnown && (kind === 'usdb'
    ? snapshot.network === targetChain && snapshot.genesis === targetGenesis
    : snapshot.network === targetBitcoin && bitcoinAddress(snapshot.address, targetBitcoin)))
  const address = mode === 'watch' ? watched : matched ? snapshot?.address ?? '' : ''
  const chain = data?.services.usdb_chain
  const indexer = data?.services.usdb_indexer
  const queryReady = kind === 'usdb'
    ? chain?.reachable && chainId(chain.data?.chain_id) === targetChain && genesisHash(chain.data?.genesis_hash) === targetGenesis
    : indexer?.reachable && indexer.data?.query_ready === true && bitcoinNetwork(indexer.data.network) === targetBitcoin
  const canQuery = Boolean(expectedKnown && queryReady && address)
  const balance = useSWR(kind === 'usdb' && canQuery ? ['identity-balance', address, targetChain, targetGenesis] : null, async () => {
    const value = await fetchUsdbChainAddressStatus(address)
    if (!value.available || chainId(value.usdb_chain_id) !== targetChain || genesisHash(value.usdb_genesis_hash) !== targetGenesis || value.address.toLowerCase() !== address.toLowerCase() || nativeBalance(value.balance_atoms_hex) === null) throw new Error('ADDRESS_QUERY_UNAVAILABLE')
    return value
  }, { refreshInterval: 15000, shouldRetryOnError: false, revalidateOnFocus: false })
  const pass = useSWR(kind === 'btc' && canQuery ? ['identity-pass', address, targetBitcoin] : null,
    () => fetchUsdbOwnerActivePass(address, null), { refreshInterval: 15000, shouldRetryOnError: false, revalidateOnFocus: false })
  const query = kind === 'usdb' ? balance : pass
  const queryError = query.error

  function watch(event: FormEvent) {
    event.preventDefault()
    const normalized = kind === 'usdb' ? evmAddress(input) : bitcoinAddress(input, targetBitcoin)
    setInvalid(!normalized)
    setWatched(normalized ?? '')
  }
  const errors: Record<string, string> = {
    WALLET_REJECTED: text('已取消钱包授权；可以重新连接。', 'Wallet authorization was cancelled. You can connect again.'),
    WALLET_TIMEOUT: text('钱包响应超时。检查扩展是否锁定或仍有待处理提示，然后重试。', 'Wallet timed out. Check for a locked extension or pending prompt, then retry.'),
    WALLET_NO_ACCOUNT: text('钱包未提供账户，可能已锁定或撤销授权。请重新连接。', 'No account exposed. Unlock the wallet or reconnect after revoking permission.'),
    WALLET_UNKNOWN_CHAIN: text('钱包尚未配置此网络。请使用网络发布方提供的 RPC 信息手动添加，再重新连接。', 'This network is not configured in your wallet. Add it using RPC details from the network operator, then reconnect.'),
    WALLET_CHANGED: text('账户或网络在读取期间变化，请刷新钱包状态。', 'The account or network changed during the read. Refresh wallet status.'),
    WALLET_UNSUPPORTED: text('钱包缺少账户读取或事件接口，请升级扩展，或使用只读地址查询。', 'This wallet lacks account or event APIs. Upgrade the extension or use watch-only lookup.'),
    WALLET_UNAVAILABLE: text('无法读取钱包。检查扩展授权和钱包所连接的 RPC，然后重试。', 'Cannot read the wallet. Check extension permissions and its RPC connection, then retry.'),
  }

  return <div className="grid gap-5">
    <section className="console-card grid gap-4">
      <h1 className="text-2xl font-semibold">{text('钱包与身份', 'Wallets & identity')}</h1>
      <p className="text-sm text-[color:var(--cp-muted)]">{text('连接浏览器钱包或查询只读地址。连接仅授权读取账户，不构成所有权证明；本页不签名或发送交易。', 'Connect a browser wallet or look up a watch-only address. Connection exposes an account, not proof of ownership. This page does not sign or send transactions.')}</p>
      <nav aria-label={text('身份类型', 'Identity type')} className="flex flex-wrap gap-3">
        <NavLink to="/me/usdb" className={({ isActive }) => `${button} ${isActive ? 'bg-blue-50 text-blue-800' : ''}`}>{text('USDB 账户', 'USDB account')}</NavLink>
        <NavLink to="/me/btc" className={({ isActive }) => `${button} ${isActive ? 'bg-blue-50 text-blue-800' : ''}`}>{text('BTC 矿工证身份', 'BTC miner-pass identity')}</NavLink>
      </nav>
      <dl className="grid gap-4 md:grid-cols-2">
        <Detail label={text('本节点网络', 'Node network')}>{network?.name ?? text('未知／监控数据不可用', 'Unknown / monitor unavailable')}</Detail>
        <Detail label={text('节点配置矿工地址（USDB）', 'Configured node miner address (USDB)')}>{monitor?.status === 'available' ? report?.node_identity?.configured_miner_address ?? text('未配置', 'Not configured') : text('未知', 'Unknown')}</Detail>
        {kind === 'usdb' ? <><Detail label="USDB Chain ID">{targetChain ? `${BigInt(targetChain)} (${targetChain})` : null}</Detail><Detail label="USDB genesis">{targetGenesis}</Detail></> : <Detail label={text('本节点 Bitcoin 网络', 'Node Bitcoin network')}>{targetBitcoin}</Detail>}
      </dl>
      <p className="text-sm text-[color:var(--cp-muted)]">{text('节点矿工地址来自节点配置，与浏览器钱包独立；连接钱包不会更改节点挖矿身份。BTC 地址用于矿工证持有人查询，USDB 地址用于链上余额。', 'The node miner address comes from node configuration and is independent of this browser wallet. Connecting does not change the mining identity. BTC addresses identify miner-pass owners; USDB addresses identify chain balances.')}</p>
    </section>
    <section className="console-card grid gap-4">
      <div className="flex flex-wrap gap-3">
        <button className={button} aria-pressed={mode === 'wallet'} onClick={() => { setMode('wallet'); setWatched(''); setInvalid(false) }}>{text('浏览器钱包', 'Browser wallet')}</button>
        <button className={button} aria-pressed={mode === 'watch'} onClick={() => { session?.disconnect(); setMode('watch') }}>{text('只读地址查询', 'Watch-only lookup')}</button>
      </div>
      {mode === 'wallet' ? <>
        <label className="grid gap-2 text-sm">{text('选择钱包', 'Choose wallet')}
          <select className={field} value={adapter?.id ?? ''} onChange={event => { session?.disconnect(); setSelected(event.target.value) }} disabled={!adapters.length || wallet.status === 'loading'}>
            {!adapters.length && <option value="">{text('未检测到钱包扩展', 'No wallet extension detected')}</option>}
            {adapters.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
          </select>
        </label>
        {!adapters.length && <p>{text('在当前浏览器启用 EVM 钱包，或 BTC 的 UniSat / OKX 扩展后刷新；也可以直接查询只读地址。', 'Enable an EVM wallet or a UniSat / OKX Bitcoin extension in this browser and reload, or use watch-only lookup.')}</p>}
        <div className="flex flex-wrap gap-3">
          <button className={button} disabled={!session || wallet.status === 'loading'} onClick={() => void session?.refresh(true)}>{text('连接钱包', 'Connect wallet')}</button>
          <button className={button} disabled={!session || wallet.status === 'disconnected' || wallet.status === 'loading'} onClick={() => void session?.refresh()}>{text('刷新钱包状态', 'Refresh wallet')}</button>
          <button className={button} disabled={wallet.status === 'disconnected'} onClick={() => session?.disconnect()}>{text('断开本页连接', 'Disconnect this page')}</button>
        </div>
        <p role="status">{wallet.status === 'loading' ? text('正在读取钱包／等待授权…', 'Reading wallet / awaiting authorization…') : wallet.status === 'disconnected' ? text('未连接', 'Disconnected') : wallet.status === 'error' ? text('钱包读取失败', 'Wallet read failed') : text('已连接，账户由钱包提供', 'Connected; account provided by wallet')}</p>
        {wallet.error && <p role="alert" className="text-amber-800">{errors[wallet.error] ?? errors.WALLET_UNAVAILABLE}</p>}
        {snapshot && <dl className="grid gap-4 md:grid-cols-2"><Detail label={text('钱包账户', 'Wallet account')}>{snapshot.address}</Detail><Detail label={text('钱包网络', 'Wallet network')}>{snapshot.network}</Detail>{kind === 'usdb' && <Detail label="Wallet genesis">{snapshot.genesis}</Detail>}</dl>}
        {snapshot && <p role="status" className={matched ? 'text-green-800' : 'text-amber-800'}>{matched ? text('网络匹配，可查询本节点数据。', 'Network matches; local node queries are available.') : !expectedKnown || !walletKnown ? text('网络身份尚未确认，暂不查询。请检查节点监控或钱包 RPC。', 'Network identity is unverified; queries are paused. Check node monitoring or wallet RPC.') : text('钱包与本节点网络不匹配，已暂停查询。请在钱包中切换网络。', 'Wallet and node networks do not match. Queries are paused; switch the wallet network.')}</p>}
        {snapshot && kind === 'usdb' && targetChain && snapshot.network !== targetChain && <button className={`${button} justify-self-start`} disabled={wallet.status === 'loading'} onClick={() => void session?.switchChain(targetChain)}>{text('请求钱包切换至本节点 Chain ID', 'Request switch to node Chain ID')}</button>}
        <p className="text-xs text-[color:var(--cp-muted)]">{text('切换账户、网络或身份页会清除旧结果。断开本页连接不撤销扩展中的站点授权；可在钱包扩展中撤销。', 'Account, network and identity-page changes clear previous results. Disconnecting this page does not revoke extension permissions; revoke them in the wallet.')}</p>
      </> : <form onSubmit={watch} className="grid gap-3">
        <label className="grid gap-2 text-sm">{text('只读地址', 'Watch-only address')}<input className={field} value={input} autoComplete="off" spellCheck={false} onChange={event => { setInput(event.target.value); setWatched(''); setInvalid(false) }} /></label>
        <p className="text-sm">{text('手动地址仅用于观察，不代表已连接钱包或拥有此地址。', 'A manually entered address is for observation only; it does not indicate a connected wallet or ownership.')}</p>
        <button className={`${button} justify-self-start`} disabled={!input.trim() || !expectedKnown}>{text('查询地址', 'Look up address')}</button>
        {invalid && <p role="alert">{text('地址格式、校验和或 Bitcoin 网络不匹配。', 'Invalid address format, checksum or Bitcoin network.')}</p>}
      </form>}
      {!expectedKnown && <p role="alert">{text('本节点网络信息缺失或已过期。先恢复 console monitor；不会根据钱包或地址猜测目标网络。', 'Node network information is missing or stale. Restore the console monitor first; the target network is not inferred from a wallet or address.')}</p>}
    </section>
    {address && <section className="console-card grid gap-4" aria-label={text('本节点查询结果', 'Local node results')}>
      <h2 className="text-lg font-semibold">{kind === 'usdb' ? text('USDB 余额（本节点）', 'USDB balance (local node)') : text('有效矿工证（本节点索引）', 'Active miner pass (local index)')}</h2>
      <p className="break-all font-mono text-sm">{address}</p>
      {!canQuery ? <p role="status">{text('本节点查询尚不可用或 RPC 网络不匹配。请查看节点监控与服务状态；这不表示余额为零或没有矿工证。', 'Local queries are unavailable or the RPC network differs. Check node and service status; this does not mean zero balance or no miner pass.')}</p> : queryError ? <p role="alert">{text('本节点查询失败，未显示旧结果。请检查对应服务后刷新。', 'Local query failed; previous results are hidden. Check the service and refresh.')}</p> : query.isLoading || query.isValidating ? <p role="status">{text('正在查询本节点…', 'Querying the local node…')}</p> : kind === 'usdb' && balance.data ? <dl className="grid gap-3"><Detail label={text('余额', 'Balance')}>{nativeBalance(balance.data.balance_atoms_hex)} USDB</Detail><Detail label={text('节点观测区块高度', 'Observed node block height')}>{balance.data.latest_block_number}</Detail></dl> : kind === 'btc' && pass.data !== undefined ? pass.data ? <dl className="grid gap-4 md:grid-cols-2">
        <Detail label={text('矿工证 ID', 'Miner pass ID')}>{pass.data.inscription_id}</Detail>
        <Detail label={text('状态', 'State')}>{pass.data.state}</Detail>
        <Detail label={text('类型', 'Kind')}>{pass.data.pass_kind}</Detail>
        <Detail label={text('关联 USDB 地址', 'Associated USDB address')}>{pass.data.usdb_main}</Detail>
        <Detail label={text('索引查询高度', 'Resolved index height')}>{pass.data.resolved_height}</Detail>
      </dl> : <p>{text('在本节点当前索引高度未查到有效矿工证；不代表网络最新状态。', 'No active miner pass at this node’s indexed height; this does not certify the network tip.')}</p> : null}
      {canQuery && <button className={`${button} justify-self-start`} disabled={query.isValidating} onClick={() => void query.mutate()}>{text('刷新查询', 'Refresh query')}</button>}
      <p className="text-xs text-[color:var(--cp-muted)]">{text('数据来自私有节点，约每 15 秒刷新，可能落后于网络；不作为钱包所有权或挖矿资格证明。', 'Data comes from this private node, refreshes about every 15 seconds and may lag the network. It does not prove wallet ownership or mining eligibility.')}</p>
    </section>}
    {data?.development_enabled && <Link className="text-sm underline" to={`/development/${kind}`}>{text('开发工具（仅限开发环境）', 'Development tools (development environments only)')}</Link>}
  </div>
}
