import { useEffect, useState } from 'react'
import { Navigate, NavLink, useParams } from 'react-router-dom'
import { FieldValueList } from '../components/FieldValueList'
import { displayText } from '../lib/format'
import type { OverviewResponse } from '../lib/types'
import {
  connectBtcWallet,
  detectBtcWalletProvider,
  readBtcWalletSnapshot,
  type BtcWalletSnapshot,
} from '../lib/btcWallet'

interface MePageProps {
  data?: OverviewResponse
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

type IdentityKind = 'eth' | 'btc'

function normalizeIdentityKind(value?: string): IdentityKind | null {
  if (value === 'eth' || value === 'btc') return value
  return null
}

function validateEthAddress(value: string) {
  return /^0x[a-fA-F0-9]{40}$/.test(value.trim())
}

export function MePage({ data, t }: MePageProps) {
  const { identityKind } = useParams()
  const activeIdentity = normalizeIdentityKind(identityKind)
  const [ethAddress, setEthAddress] = useState('')
  const [btcAddress, setBtcAddress] = useState('')
  const [btcWallet, setBtcWallet] = useState<BtcWalletSnapshot | null>(null)
  const [btcWalletLoading, setBtcWalletLoading] = useState(false)
  const [btcWalletError, setBtcWalletError] = useState<string | null>(null)

  if (!activeIdentity) {
    return <Navigate to="/me/eth" replace />
  }

  const ethwReachable = Boolean(data?.services.ethw.reachable)
  const ethwConsensusReady = Boolean(data?.services.ethw.data?.consensus_ready)
  const sourcedaoReady = Boolean(data?.bootstrap.sourcedao_bootstrap_marker.exists)
  const ordAvailable = Boolean(data?.capabilities.ord_available)
  const btcConsoleMode = data?.capabilities.btc_console_mode ?? 'read_only'
  const balanceHistoryReady = Boolean(data?.services.balance_history.data?.query_ready)
  const usdbIndexerReady = Boolean(data?.services.usdb_indexer.data?.query_ready)
  const hasInjectedBtcWallet = Boolean(detectBtcWalletProvider())
  const btcWalletConnected = Boolean(btcWallet?.address)

  const activeAddressValue =
    activeIdentity === 'eth'
      ? ethAddress.trim()
      : btcWallet?.address ?? btcAddress.trim()

  const activeAddressStatus =
    activeIdentity === 'eth'
      ? activeAddressValue === ''
        ? t('me.identity.notSet')
        : validateEthAddress(activeAddressValue)
          ? t('me.identity.validFormat')
          : t('me.identity.checkFormat')
      : activeAddressValue === ''
        ? t('me.identity.notSet')
        : t('me.identity.checkFormat')

  const capabilityItems =
    activeIdentity === 'eth'
      ? [
          {
            label: t('me.fields.ethwRuntime'),
            value: ethwReachable ? t('service.reachable') : t('service.offline'),
            helpText: t('me.help.ethwRuntime'),
          },
          {
            label: t('me.fields.ethwConsensus'),
            value: ethwConsensusReady ? t('service.consensusReady') : t('common.notYetAvailable'),
            helpText: t('me.help.ethwConsensus'),
          },
          {
            label: t('me.fields.sourcedaoBootstrap'),
            value: sourcedaoReady ? t('states.completed') : t('states.pending'),
            helpText: t('me.help.sourcedaoBootstrap'),
          },
          {
            label: t('me.fields.walletMode'),
            value: t('me.values.manualAddressFirst'),
            helpText: t('me.help.walletMode'),
          },
        ]
      : [
          {
            label: t('me.fields.btcConsoleMode'),
            value:
              btcConsoleMode === 'inscription_enabled'
                ? t('capabilities.consoleMode.inscriptionEnabled')
                : t('capabilities.consoleMode.readOnly'),
            helpText: t('me.help.btcConsoleMode'),
          },
          {
            label: t('me.fields.ordBackend'),
            value: ordAvailable ? t('capabilities.ord.available') : t('capabilities.ord.unavailable'),
            helpText: t('me.help.ordBackend'),
          },
          {
            label: t('me.fields.balanceHistory'),
            value: balanceHistoryReady ? t('service.queryReady') : t('common.notYetAvailable'),
            helpText: t('me.help.balanceHistory'),
          },
          {
            label: t('me.fields.usdbIndexer'),
            value: usdbIndexerReady ? t('service.queryReady') : t('common.notYetAvailable'),
            helpText: t('me.help.usdbIndexer'),
          },
        ]

  const identityItems =
    activeIdentity === 'eth'
      ? [
          {
            label: t('me.fields.currentAddress'),
            value: displayText(activeAddressValue || null, t),
            helpText: t('me.help.currentEthAddress'),
          },
          {
            label: t('me.fields.addressStatus'),
            value: activeAddressStatus,
            helpText: t('me.help.addressStatus'),
          },
          {
            label: t('me.fields.chainId'),
            value: displayText(data?.services.ethw.data?.chain_id, t),
            helpText: t('help.fields.chainId'),
          },
          {
            label: t('me.fields.latestBlock'),
            value: displayText(data?.services.ethw.data?.block_number, t),
            helpText: t('help.fields.blockNumber'),
          },
        ]
      : [
          {
            label: t('me.fields.currentAddress'),
            value: displayText(activeAddressValue || null, t),
            helpText: t('me.help.currentBtcAddress'),
          },
          {
            label: t('me.fields.addressStatus'),
            value: activeAddressStatus,
            helpText: t('me.help.addressStatus'),
          },
          {
            label: t('me.fields.mintCapability'),
            value:
              ordAvailable && balanceHistoryReady && usdbIndexerReady
                ? t('me.values.mintReady')
                : t('me.values.readOnly'),
            helpText: t('me.help.mintCapability'),
          },
          {
            label: t('me.fields.protocolData'),
            value: usdbIndexerReady ? t('service.queryReady') : t('common.notYetAvailable'),
            helpText: t('me.help.protocolData'),
          },
        ]

  const workspaceSummaryItems =
    activeIdentity === 'eth'
      ? [
          {
            label: t('me.fields.primaryAction'),
            value: t('me.values.ethContractActions'),
            helpText: t('me.help.primaryAction'),
          },
          {
            label: t('me.fields.runtimeGate'),
            value: sourcedaoReady ? t('states.completed') : t('states.pending'),
            helpText: t('me.help.runtimeGate'),
          },
          {
            label: t('me.fields.identityMode'),
            value: t('me.values.manualAddressFirst'),
            helpText: t('me.help.identityMode'),
          },
          {
            label: t('me.fields.currentAddress'),
            value: displayText(activeAddressValue || null, t),
            helpText: t('me.help.currentEthAddress'),
          },
        ]
      : [
          {
            label: t('me.fields.primaryAction'),
            value:
              ordAvailable && balanceHistoryReady && usdbIndexerReady
                ? t('me.values.btcMintAndPass')
                : t('me.values.btcReadOnlyData'),
            helpText: t('me.help.primaryAction'),
          },
          {
            label: t('me.fields.runtimeGate'),
            value:
              ordAvailable && balanceHistoryReady && usdbIndexerReady
                ? t('me.values.mintReady')
                : t('me.values.readOnly'),
            helpText: t('me.help.runtimeGate'),
          },
          {
            label: t('me.fields.identityMode'),
            value: btcWallet?.address ? t('me.values.browserWalletConnected') : t('me.values.manualAddressFirst'),
            helpText: t('me.help.identityMode'),
          },
          {
            label: t('me.fields.currentAddress'),
            value: displayText(activeAddressValue || null, t),
            helpText: t('me.help.currentBtcAddress'),
          },
        ]

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    let cancelled = false

    void readBtcWalletSnapshot()
      .then((snapshot) => {
        if (cancelled) return
        if (snapshot?.address) {
          setBtcWallet(snapshot)
          setBtcWalletError(null)
        }
      })
      .catch((error: Error) => {
        if (cancelled) return
        setBtcWalletError(error.message)
      })

    return () => {
      cancelled = true
    }
  }, [activeIdentity])

  async function handleConnectBtcWallet() {
    setBtcWalletLoading(true)
    setBtcWalletError(null)

    try {
      const snapshot = await connectBtcWallet()
      setBtcWallet(snapshot)
      if (snapshot.address) {
        setBtcAddress(snapshot.address)
      }
    } catch (error) {
      setBtcWalletError(error instanceof Error ? error.message : String(error))
    } finally {
      setBtcWalletLoading(false)
    }
  }

  const currentTitle =
    activeIdentity === 'eth' ? t('me.eth.title') : t('me.btc.title')
  const currentBody =
    activeIdentity === 'eth' ? t('me.eth.body') : t('me.btc.body')

  return (
    <div className="grid gap-5">
      <section className="console-page-intro">
        <h2 className="text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
          {t('pages.me.title')}
        </h2>
        <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
          {t('pages.me.subtitle')}
        </p>
      </section>

      <section className="grid gap-4 xl:grid-cols-[280px,minmax(0,1fr)]">
        <aside className="console-card">
          <div>
            <p className="shell-kicker m-0">{t('me.selector.kicker')}</p>
            <h3 className="mt-2 text-base font-semibold text-[color:var(--cp-text)]">
              {t('me.selector.title')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('me.selector.body')}
            </p>
          </div>
          <div className="mt-4 grid gap-3">
            <NavLink
              to="/me/eth"
              className={({ isActive }) =>
                isActive ? 'console-service-selector active' : 'console-service-selector'
              }
            >
              <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                {t('me.selector.ethTitle')}
              </h4>
              <p className="mt-1 text-sm text-[color:var(--cp-muted)]">
                {t('me.selector.ethBody')}
              </p>
            </NavLink>
            <NavLink
              to="/me/btc"
              className={({ isActive }) =>
                isActive ? 'console-service-selector active' : 'console-service-selector'
              }
            >
              <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                {t('me.selector.btcTitle')}
              </h4>
              <p className="mt-1 text-sm text-[color:var(--cp-muted)]">
                {t('me.selector.btcBody')}
              </p>
            </NavLink>
          </div>
        </aside>

        <article className="console-card">
          <div className="flex flex-wrap items-start justify-between gap-4">
            <div>
              <p className="shell-kicker m-0">{t('me.workspace.kicker')}</p>
              <h3 className="mt-2 text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
                {currentTitle}
              </h3>
              <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
                {currentBody}
              </p>
            </div>
            {activeIdentity === 'btc' ? (
              <div className="flex flex-wrap items-center gap-3">
                <button
                  type="button"
                  className={btcWalletConnected ? 'console-secondary-button' : 'console-action-button'}
                  disabled={!hasInjectedBtcWallet || btcWalletLoading}
                  onClick={() => void handleConnectBtcWallet()}
                >
                  {btcWalletLoading
                    ? t('actions.reloading')
                    : btcWalletConnected
                      ? t('me.btc.refreshWallet')
                      : t('me.btc.connectWallet')}
                </button>
                {btcWalletConnected ? (
                  <span className="status-pill" data-tone="success">
                    {t('me.values.walletConnected')}
                  </span>
                ) : null}
              </div>
            ) : null}
          </div>
          <div className="mt-4">
            <FieldValueList items={workspaceSummaryItems} />
          </div>
          {activeIdentity === 'btc' && !hasInjectedBtcWallet ? (
            <p className="mt-4 text-sm text-[color:var(--cp-warning)]">
              {t('me.btc.walletUnavailable')}
            </p>
          ) : null}
          {activeIdentity === 'btc' && btcWalletError ? (
            <p className="mt-2 text-sm text-[color:var(--cp-danger)]">{btcWalletError}</p>
          ) : null}
        </article>
      </section>

      <section className="grid gap-4 xl:grid-cols-2">
        <article className="console-card">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('me.capability.title')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {activeIdentity === 'eth' ? t('me.capability.ethBody') : t('me.capability.btcBody')}
          </p>
          <div className="mt-4">
            <FieldValueList items={capabilityItems} />
          </div>
        </article>

        <article className="console-card">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('me.identity.title')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {activeIdentity === 'eth' ? t('me.identity.ethBody') : t('me.identity.btcBody')}
          </p>
          <div className="mt-4 grid gap-4">
            <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
              <span>
                {activeIdentity === 'eth'
                  ? t('me.identity.ethInputLabel')
                  : t('me.identity.btcInputLabel')}
              </span>
              <input
                className="console-input"
                value={activeIdentity === 'eth' ? ethAddress : btcAddress}
                onChange={(event) =>
                  activeIdentity === 'eth'
                    ? setEthAddress(event.target.value)
                    : setBtcAddress(event.target.value)
                }
                placeholder={
                  activeIdentity === 'eth'
                    ? t('me.identity.ethPlaceholder')
                    : t('me.identity.btcPlaceholder')
                }
              />
            </label>
            <FieldValueList items={identityItems} />
          </div>
        </article>
      </section>

      {activeIdentity === 'btc' ? (
        <section className="grid gap-4 xl:grid-cols-2">
          <article className="console-card">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('me.btc.walletTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('me.btc.walletBody')}
            </p>
            <div className="mt-4">
              <FieldValueList
                items={[
                  {
                    label: t('me.fields.walletProvider'),
                    value: displayText(btcWallet?.source, t),
                    helpText: t('me.help.walletProvider'),
                  },
                  {
                    label: t('me.fields.walletAddress'),
                    value: displayText(btcWallet?.address, t),
                    helpText: t('me.help.walletAddress'),
                  },
                  {
                    label: t('me.fields.walletPublicKey'),
                    value: displayText(btcWallet?.publicKey, t),
                    helpText: t('me.help.walletPublicKey'),
                  },
                  {
                    label: t('me.fields.walletNetwork'),
                    value: displayText(btcWallet?.network, t),
                    helpText: t('me.help.walletNetwork'),
                  },
                  {
                    label: t('me.fields.addressStatus'),
                    value: activeAddressStatus,
                    helpText: t('me.help.addressStatus'),
                  },
                ]}
              />
            </div>
          </article>

          <article className="console-card">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('me.btc.walletDataTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('me.btc.walletDataBody')}
            </p>
            <div className="mt-4">
              <FieldValueList
                items={[
                  {
                    label: t('me.fields.walletBalance'),
                    value:
                      btcWallet?.balance != null
                        ? `${btcWallet.balance.total} sat`
                        : t('common.notYetAvailable'),
                    helpText: t('me.help.walletBalance'),
                  },
                  {
                    label: t('me.fields.walletConfirmed'),
                    value:
                      btcWallet?.balance != null
                        ? `${btcWallet.balance.confirmed} sat`
                        : t('common.notYetAvailable'),
                    helpText: t('me.help.walletConfirmed'),
                  },
                  {
                    label: t('me.fields.walletUnconfirmed'),
                    value:
                      btcWallet?.balance != null
                        ? `${btcWallet.balance.unconfirmed} sat`
                        : t('common.notYetAvailable'),
                    helpText: t('me.help.walletUnconfirmed'),
                  },
                  {
                    label: t('me.fields.walletInscriptions'),
                    value:
                      btcWallet != null
                        ? String(btcWallet.inscriptions.length)
                        : t('common.notYetAvailable'),
                    helpText: t('me.help.walletInscriptions'),
                  },
                ]}
              />
            </div>
          </article>
        </section>
      ) : null}

      <section className="console-card">
        <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
          {t('me.nextActions.title')}
        </h3>
        <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
          {activeIdentity === 'eth' ? t('me.nextActions.ethBody') : t('me.nextActions.btcBody')}
        </p>
        <ul className="mt-4 grid gap-2 text-sm leading-6 text-[color:var(--cp-text)]">
          {(activeIdentity === 'eth'
            ? ['me.nextActions.ethItem1', 'me.nextActions.ethItem2', 'me.nextActions.ethItem3']
            : ['me.nextActions.btcItem1', 'me.nextActions.btcItem2', 'me.nextActions.btcItem3']
          ).map((key) => (
            <li key={key}>{t(key)}</li>
          ))}
        </ul>
      </section>
    </div>
  )
}
