import { useEffect, useMemo, useState } from 'react'
import { Navigate, NavLink, useParams } from 'react-router-dom'
import { FieldValueList } from '../components/FieldValueList'
import {
  fetchBalanceHistorySingleBalance,
  fetchUsdbOwnerActivePass,
  fetchUsdbPassEnergy,
  fetchUsdbPassSnapshot,
} from '../lib/api'
import {
  displayBalanceDeltaSmart,
  displayBalanceSmart,
  displayNumber,
  displayText,
} from '../lib/format'
import type { AddressBalanceRow, OverviewResponse, PassEnergySnapshot, PassSnapshot } from '../lib/types'
import {
  clearDevRegtestWallet,
  connectBtcWalletByMode,
  detectBtcWalletProvider,
  getBtcWalletAdapterCapabilities,
  importDevRegtestWallet,
  readBtcWalletSnapshotByMode,
  signBtcWalletMessage,
  signBtcWalletPsbt,
  type BtcWalletMode,
  type BtcWalletPsbtSignatureResult,
  type BtcWalletSnapshot,
} from '../lib/btcWallet'

interface MePageProps {
  data?: OverviewResponse
  locale: string
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

type IdentityKind = 'eth' | 'btc'

interface WalletPassRecognition {
  walletInscriptionId: string
  walletInscriptionNumber: number | string | null
  pass: PassSnapshot
}

type BtcRuntimeNetwork = 'mainnet' | 'testnet' | 'testnet4' | 'regtest' | 'signet'

function normalizeIdentityKind(value?: string): IdentityKind | null {
  if (value === 'eth' || value === 'btc') return value
  return null
}

function validateEthAddress(value: string) {
  return /^0x[a-fA-F0-9]{40}$/.test(value.trim())
}

function normalizeBtcRuntimeNetwork(value?: string | null): BtcRuntimeNetwork | null {
  switch ((value ?? '').trim().toLowerCase()) {
    case 'bitcoin':
    case 'main':
    case 'mainnet':
      return 'mainnet'
    case 'test':
    case 'testnet':
    case 'testnet3':
      return 'testnet'
    case 'testnet4':
      return 'testnet4'
    case 'regtest':
      return 'regtest'
    case 'signet':
      return 'signet'
    default:
      return null
  }
}

function normalizeInjectedWalletNetwork(value?: string | null): BtcRuntimeNetwork | null {
  switch ((value ?? '').trim().toLowerCase()) {
    case 'livenet':
    case 'main':
    case 'mainnet':
    case 'bitcoin':
      return 'mainnet'
    case 'testnet':
    case 'testnet3':
      return 'testnet'
    case 'testnet4':
      return 'testnet4'
    case 'regtest':
      return 'regtest'
    case 'signet':
      return 'signet'
    default:
      return null
  }
}

function inferBtcAddressNetwork(address?: string | null): BtcRuntimeNetwork | null {
  const normalized = (address ?? '').trim().toLowerCase()
  if (!normalized) return null
  if (normalized.startsWith('bc1') || normalized.startsWith('1') || normalized.startsWith('3')) {
    return 'mainnet'
  }
  if (normalized.startsWith('tb1')) {
    return 'testnet'
  }
  if (normalized.startsWith('bcrt1')) {
    return 'regtest'
  }
  if (
    normalized.startsWith('m') ||
    normalized.startsWith('n') ||
    normalized.startsWith('2')
  ) {
    return null
  }
  return null
}

export function MePage({ data, locale, t }: MePageProps) {
  const { identityKind } = useParams()
  const activeIdentity = normalizeIdentityKind(identityKind)
  const [ethAddress, setEthAddress] = useState('')
  const [btcAddress, setBtcAddress] = useState('')
  const [btcWallet, setBtcWallet] = useState<BtcWalletSnapshot | null>(null)
  const [btcWalletMode, setBtcWalletMode] = useState<BtcWalletMode>('browser')
  const [btcWalletLoading, setBtcWalletLoading] = useState(false)
  const [btcWalletError, setBtcWalletError] = useState<string | null>(null)
  const [btcDevWalletWif, setBtcDevWalletWif] = useState('')
  const [btcDevWalletAddress, setBtcDevWalletAddress] = useState('')
  const [btcDevWalletMessage, setBtcDevWalletMessage] = useState('')
  const [btcDevWalletSignature, setBtcDevWalletSignature] = useState<string | null>(null)
  const [btcDevWalletSigning, setBtcDevWalletSigning] = useState(false)
  const [btcDevWalletSignatureError, setBtcDevWalletSignatureError] = useState<string | null>(null)
  const [btcDevWalletPsbt, setBtcDevWalletPsbt] = useState('')
  const [btcDevWalletPsbtSigning, setBtcDevWalletPsbtSigning] = useState(false)
  const [btcDevWalletPsbtError, setBtcDevWalletPsbtError] = useState<string | null>(null)
  const [btcDevWalletPsbtResult, setBtcDevWalletPsbtResult] =
    useState<BtcWalletPsbtSignatureResult | null>(null)
  const [btcAddressBalanceRows, setBtcAddressBalanceRows] = useState<AddressBalanceRow[]>([])
  const [btcAddressBalanceLoading, setBtcAddressBalanceLoading] = useState(false)
  const [btcAddressBalanceError, setBtcAddressBalanceError] = useState<string | null>(null)
  const [btcActivePass, setBtcActivePass] = useState<PassSnapshot | null>(null)
  const [btcActivePassEnergy, setBtcActivePassEnergy] = useState<PassEnergySnapshot | null>(null)
  const [btcProtocolLoading, setBtcProtocolLoading] = useState(false)
  const [btcProtocolError, setBtcProtocolError] = useState<string | null>(null)
  const [btcRecognizedPasses, setBtcRecognizedPasses] = useState<WalletPassRecognition[]>([])
  const [btcRecognizedPassesLoading, setBtcRecognizedPassesLoading] = useState(false)
  const [btcRecognizedPassesError, setBtcRecognizedPassesError] = useState<string | null>(null)

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
  const btcWalletAdapterCapabilities = getBtcWalletAdapterCapabilities(btcWalletMode)
  const btcWalletConnected = Boolean(btcWallet?.address)
  const btcRuntimeNetwork = normalizeBtcRuntimeNetwork(data?.services.btc_node.data?.chain)
  const btcWalletReportedNetwork = normalizeInjectedWalletNetwork(btcWallet?.network)

  const activeAddressValue =
    activeIdentity === 'eth'
      ? ethAddress.trim()
      : btcWallet?.address ?? btcAddress.trim()
  const btcAddressDerivedNetwork = inferBtcAddressNetwork(btcWallet?.address ?? btcAddress)
  const btcEffectiveWalletNetwork = btcAddressDerivedNetwork ?? btcWalletReportedNetwork
  const btcWalletNetworkMismatch =
    btcRuntimeNetwork != null &&
    btcEffectiveWalletNetwork != null &&
    btcRuntimeNetwork !== btcEffectiveWalletNetwork
  const btcWalletNetworkMismatchMessage = btcWalletNetworkMismatch
    ? t('me.btc.networkMismatch', undefined, {
        walletNetwork: btcEffectiveWalletNetwork,
        runtimeNetwork: btcRuntimeNetwork,
      })
    : null

  const activeAddressStatus =
    activeIdentity === 'eth'
      ? activeAddressValue === ''
        ? t('me.identity.notSet')
        : validateEthAddress(activeAddressValue)
          ? t('me.identity.validFormat')
          : t('me.identity.checkFormat')
      : activeAddressValue === ''
        ? t('me.identity.notSet')
        : btcWalletNetworkMismatch
          ? t('me.identity.networkMismatch')
        : t('me.identity.checkFormat')
  const btcLookupAddress = btcWallet?.address ?? null
  const btcLatestBalanceRow = useMemo(
    () =>
      btcAddressBalanceRows.length > 0
        ? btcAddressBalanceRows[btcAddressBalanceRows.length - 1]
        : null,
    [btcAddressBalanceRows],
  )
  const btcWalletModeLabel =
    btcWalletMode === 'dev-regtest'
      ? t('me.values.devRegtestWallet')
      : btcWallet?.address
        ? t('me.values.browserWalletConnected')
        : t('me.values.manualAddressFirst')
  const btcWalletCapabilityItems = [
    {
      label: t('me.fields.connectCapability'),
      value: btcWalletAdapterCapabilities?.canConnect ? t('states.completed') : t('common.notYetAvailable'),
      helpText: t('me.help.connectCapability'),
    },
    {
      label: t('me.fields.messageCapability'),
      value: btcWalletAdapterCapabilities?.canSignMessage
        ? t('states.completed')
        : t('common.notYetAvailable'),
      helpText: t('me.help.messageCapability'),
    },
    {
      label: t('me.fields.psbtCapability'),
      value: btcWalletAdapterCapabilities?.canSignPsbt
        ? t('states.completed')
        : t('common.notYetAvailable'),
      helpText: t('me.help.psbtCapability'),
    },
  ]

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
            label: t('me.fields.walletMode'),
            value: btcWalletMode === 'dev-regtest' ? t('me.values.devRegtestWallet') : t('me.values.browserWallet'),
            helpText: t('me.help.walletMode'),
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
              !btcWalletNetworkMismatch && ordAvailable && balanceHistoryReady && usdbIndexerReady
                ? t('me.values.mintReady')
                : t('me.values.readOnly'),
            helpText: t('me.help.mintCapability'),
          },
          {
            label: t('me.fields.protocolData'),
            value: btcWalletNetworkMismatch
              ? t('me.values.networkMismatch')
              : usdbIndexerReady
                ? t('service.queryReady')
                : t('common.notYetAvailable'),
            helpText: t('me.help.protocolData'),
          },
          {
            label: t('me.fields.runtimeNetwork'),
            value: displayText(btcRuntimeNetwork, t),
            helpText: t('me.help.runtimeNetwork'),
          },
          {
            label: t('me.fields.detectedWalletNetwork'),
            value: displayText(btcEffectiveWalletNetwork ?? btcWallet?.network, t),
            helpText: t('me.help.detectedWalletNetwork'),
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
              !btcWalletNetworkMismatch && ordAvailable && balanceHistoryReady && usdbIndexerReady
                ? t('me.values.btcMintAndPass')
                : t('me.values.btcReadOnlyData'),
            helpText: t('me.help.primaryAction'),
          },
          {
            label: t('me.fields.runtimeGate'),
            value:
              !btcWalletNetworkMismatch && ordAvailable && balanceHistoryReady && usdbIndexerReady
                ? t('me.values.mintReady')
                : t('me.values.readOnly'),
            helpText: t('me.help.runtimeGate'),
          },
          {
            label: t('me.fields.identityMode'),
            value: btcWalletModeLabel,
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

    void readBtcWalletSnapshotByMode(btcWalletMode)
      .then((snapshot) => {
        if (cancelled) return
        setBtcWallet(snapshot)
        if (snapshot?.address) {
          setBtcAddress(snapshot.address)
        }
        setBtcWalletError(null)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setBtcWalletError(error.message)
      })

    return () => {
      cancelled = true
    }
  }, [activeIdentity, btcWalletMode])

  useEffect(() => {
    setBtcWalletError(null)
    setBtcDevWalletSignature(null)
    setBtcDevWalletSignatureError(null)
    setBtcDevWalletPsbtResult(null)
    setBtcDevWalletPsbtError(null)
  }, [btcWalletMode])

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    if (btcWalletNetworkMismatchMessage) {
      setBtcAddressBalanceRows([])
      setBtcAddressBalanceError(btcWalletNetworkMismatchMessage)
      setBtcActivePass(null)
      setBtcActivePassEnergy(null)
      setBtcProtocolError(btcWalletNetworkMismatchMessage)
      return
    }
    if (!btcLookupAddress || !balanceHistoryReady || !usdbIndexerReady) {
      setBtcAddressBalanceRows([])
      setBtcAddressBalanceError(null)
      setBtcActivePass(null)
      setBtcActivePassEnergy(null)
      setBtcProtocolError(null)
      return
    }

    let cancelled = false
    setBtcAddressBalanceLoading(true)
    setBtcProtocolLoading(true)
    setBtcAddressBalanceError(null)
    setBtcProtocolError(null)

    void Promise.all([
      fetchBalanceHistorySingleBalance({
        address: btcLookupAddress,
        block_height: null,
        block_range: null,
      }),
      fetchUsdbOwnerActivePass(btcLookupAddress, null),
    ])
      .then(async ([rows, activePass]) => {
        if (cancelled) return
        setBtcAddressBalanceRows(rows)
        setBtcActivePass(activePass)
        if (!activePass) {
          setBtcActivePassEnergy(null)
          return
        }

        try {
          const energy = await fetchUsdbPassEnergy(activePass.inscription_id, null, 'at_or_before')
          if (cancelled) return
          setBtcActivePassEnergy(energy)
        } catch (error) {
          if (cancelled) return
          setBtcActivePassEnergy(null)
          setBtcProtocolError(error instanceof Error ? error.message : String(error))
        }
      })
      .catch((error) => {
        if (cancelled) return
        const message = error instanceof Error ? error.message : String(error)
        setBtcAddressBalanceRows([])
        setBtcActivePass(null)
        setBtcActivePassEnergy(null)
        setBtcAddressBalanceError(message)
        setBtcProtocolError(message)
      })
      .finally(() => {
        if (cancelled) return
        setBtcAddressBalanceLoading(false)
        setBtcProtocolLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [
    activeIdentity,
    balanceHistoryReady,
    btcLookupAddress,
    btcWalletNetworkMismatchMessage,
    usdbIndexerReady,
  ])

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    if (btcWalletNetworkMismatchMessage) {
      setBtcRecognizedPasses([])
      setBtcRecognizedPassesError(btcWalletNetworkMismatchMessage)
      return
    }
    if (!btcWallet?.inscriptions.length || !usdbIndexerReady) {
      setBtcRecognizedPasses([])
      setBtcRecognizedPassesError(null)
      return
    }

    let cancelled = false
    setBtcRecognizedPassesLoading(true)
    setBtcRecognizedPassesError(null)

    void Promise.all(
      btcWallet.inscriptions.map(async (walletInscription) => {
        const pass = await fetchUsdbPassSnapshot(walletInscription.inscriptionId, null)
        if (!pass) return null
        return {
          walletInscriptionId: walletInscription.inscriptionId,
          walletInscriptionNumber: walletInscription.inscriptionNumber ?? null,
          pass,
        } satisfies WalletPassRecognition
      }),
    )
      .then((items) => {
        if (cancelled) return
        const recognized = items.filter((item): item is WalletPassRecognition => item !== null)
        setBtcRecognizedPasses(recognized)
      })
      .catch((error) => {
        if (cancelled) return
        setBtcRecognizedPasses([])
        setBtcRecognizedPassesError(error instanceof Error ? error.message : String(error))
      })
      .finally(() => {
        if (cancelled) return
        setBtcRecognizedPassesLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [activeIdentity, btcWallet?.inscriptions, btcWalletNetworkMismatchMessage, usdbIndexerReady])

  async function handleConnectBtcWallet() {
    setBtcWalletLoading(true)
    setBtcWalletError(null)

    try {
      const snapshot = await connectBtcWalletByMode('browser')
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

  async function handleImportDevBtcWallet() {
    setBtcWalletLoading(true)
    setBtcWalletError(null)

    try {
      const snapshot = await importDevRegtestWallet({
        wif: btcDevWalletWif,
        address: btcDevWalletAddress,
      })
      setBtcWallet(snapshot)
      setBtcAddress(snapshot.address ?? '')
      setBtcDevWalletSignature(null)
      setBtcDevWalletSignatureError(null)
      setBtcDevWalletPsbtResult(null)
      setBtcDevWalletPsbtError(null)
    } catch (error) {
      setBtcWalletError(error instanceof Error ? error.message : String(error))
    } finally {
      setBtcWalletLoading(false)
    }
  }

  function handleClearDevBtcWallet() {
    clearDevRegtestWallet()
    setBtcWallet(null)
    setBtcAddress('')
    setBtcWalletError(null)
    setBtcAddressBalanceRows([])
    setBtcAddressBalanceError(null)
    setBtcActivePass(null)
    setBtcActivePassEnergy(null)
    setBtcProtocolError(null)
    setBtcRecognizedPasses([])
    setBtcRecognizedPassesError(null)
    setBtcDevWalletSignature(null)
    setBtcDevWalletSignatureError(null)
    setBtcDevWalletPsbt('')
    setBtcDevWalletPsbtResult(null)
    setBtcDevWalletPsbtError(null)
  }

  async function handleSignDevWalletMessage() {
    setBtcDevWalletSigning(true)
    setBtcDevWalletSignature(null)
    setBtcDevWalletSignatureError(null)

    try {
      const result = await signBtcWalletMessage('dev-regtest', btcDevWalletMessage)
      setBtcDevWalletSignature(result.signature)
    } catch (error) {
      setBtcDevWalletSignatureError(error instanceof Error ? error.message : String(error))
    } finally {
      setBtcDevWalletSigning(false)
    }
  }

  async function handleSignDevWalletPsbt(finalize: boolean) {
    setBtcDevWalletPsbtSigning(true)
    setBtcDevWalletPsbtResult(null)
    setBtcDevWalletPsbtError(null)

    try {
      const result = await signBtcWalletPsbt('dev-regtest', btcDevWalletPsbt, { finalize })
      setBtcDevWalletPsbtResult(result)
    } catch (error) {
      setBtcDevWalletPsbtError(error instanceof Error ? error.message : String(error))
    } finally {
      setBtcDevWalletPsbtSigning(false)
    }
  }

  const currentTitle =
    activeIdentity === 'eth' ? t('me.eth.title') : t('me.btc.title')
  const currentBody =
    activeIdentity === 'eth' ? t('me.eth.body') : t('me.btc.body')
  const btcWalletCardTitle =
    btcWalletMode === 'dev-regtest' ? t('me.btc.devWalletTitle') : t('me.btc.walletTitle')
  const btcWalletCardBody =
    btcWalletMode === 'dev-regtest' ? t('me.btc.devWalletBody') : t('me.btc.walletBody')

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
                <div className="flex flex-wrap items-center gap-2">
                  <button
                    type="button"
                    className={
                      btcWalletMode === 'browser' ? 'console-action-button' : 'console-secondary-button'
                    }
                    onClick={() => setBtcWalletMode('browser')}
                  >
                    {t('me.btc.browserWalletTab')}
                  </button>
                  <button
                    type="button"
                    className={
                      btcWalletMode === 'dev-regtest'
                        ? 'console-action-button'
                        : 'console-secondary-button'
                    }
                    onClick={() => setBtcWalletMode('dev-regtest')}
                  >
                    {t('me.btc.devWalletTab')}
                  </button>
                </div>
                {btcWalletMode === 'browser' ? (
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
                ) : null}
                {btcWalletConnected ? (
                  <span className="status-pill" data-tone={btcWalletMode === 'dev-regtest' ? 'warning' : 'success'}>
                    {btcWalletMode === 'dev-regtest'
                      ? t('me.values.devWalletLoaded')
                      : t('me.values.walletConnected')}
                  </span>
                ) : null}
              </div>
            ) : null}
          </div>
          <div className="mt-4">
            <FieldValueList items={workspaceSummaryItems} />
          </div>
          {activeIdentity === 'btc' && btcWalletMode === 'browser' && !hasInjectedBtcWallet ? (
            <p className="mt-4 text-sm text-[color:var(--cp-warning)]">
              {t('me.btc.walletUnavailable')}
            </p>
          ) : null}
          {activeIdentity === 'btc' && btcWalletError ? (
            <p className="mt-2 text-sm text-[color:var(--cp-danger)]">{btcWalletError}</p>
          ) : null}
          {activeIdentity === 'btc' && btcWalletNetworkMismatchMessage ? (
            <p className="mt-2 text-sm text-[color:var(--cp-danger)]">
              {btcWalletNetworkMismatchMessage}
            </p>
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
        <>
          <section className="grid gap-4 xl:grid-cols-2">
            <article className="console-card">
              <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                {btcWalletCardTitle}
              </h3>
              <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                {btcWalletCardBody}
              </p>
              <div className="mt-4">
                {btcWalletMode === 'dev-regtest' ? (
                  <div className="grid gap-4">
                    <p className="rounded-2xl border border-[color:var(--cp-warning)]/25 bg-[color:var(--cp-warning)]/8 px-4 py-3 text-sm leading-6 text-[color:var(--cp-warning)]">
                      {t('me.btc.devWalletWarning')}
                    </p>
                    <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                      <span>{t('me.btc.devWalletWifLabel')}</span>
                      <input
                        className="console-input"
                        type="password"
                        autoComplete="off"
                        value={btcDevWalletWif}
                        onChange={(event) => setBtcDevWalletWif(event.target.value)}
                        placeholder={t('me.btc.devWalletWifPlaceholder')}
                      />
                    </label>
                    <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                      <span>{t('me.btc.devWalletAddressLabel')}</span>
                      <input
                        className="console-input"
                        value={btcDevWalletAddress}
                        onChange={(event) => setBtcDevWalletAddress(event.target.value)}
                        placeholder={t('me.btc.devWalletAddressPlaceholder')}
                      />
                    </label>
                    <div className="flex flex-wrap items-center gap-3">
                      <button
                        type="button"
                        className="console-action-button"
                        disabled={btcWalletLoading}
                        onClick={() => void handleImportDevBtcWallet()}
                      >
                        {btcWalletLoading ? t('actions.reloading') : t('me.btc.importDevWallet')}
                      </button>
                      <button
                        type="button"
                        className="console-secondary-button"
                        disabled={!btcWalletConnected}
                        onClick={handleClearDevBtcWallet}
                      >
                        {t('me.btc.clearDevWallet')}
                      </button>
                    </div>
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
                          value: displayText(btcWallet?.network ?? btcEffectiveWalletNetwork, t),
                          helpText: t('me.help.walletNetwork'),
                        },
                        {
                          label: t('me.fields.runtimeNetwork'),
                          value: displayText(btcRuntimeNetwork, t),
                          helpText: t('me.help.runtimeNetwork'),
                        },
                        {
                          label: t('me.fields.addressStatus'),
                          value: activeAddressStatus,
                          helpText: t('me.help.addressStatus'),
                        },
                        ...btcWalletCapabilityItems,
                      ]}
                    />
                  </div>
                ) : (
                  <div className="grid gap-4">
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
                          value: displayText(btcWallet?.network ?? btcEffectiveWalletNetwork, t),
                          helpText: t('me.help.walletNetwork'),
                        },
                        {
                          label: t('me.fields.runtimeNetwork'),
                          value: displayText(btcRuntimeNetwork, t),
                          helpText: t('me.help.runtimeNetwork'),
                        },
                        {
                          label: t('me.fields.addressStatus'),
                          value: activeAddressStatus,
                          helpText: t('me.help.addressStatus'),
                        },
                        ...btcWalletCapabilityItems,
                      ]}
                    />
                    {!btcWalletAdapterCapabilities?.canSignPsbt ? (
                      <p className="rounded-2xl border border-[color:var(--cp-warning)]/25 bg-[color:var(--cp-warning)]/8 px-4 py-3 text-sm leading-6 text-[color:var(--cp-warning)]">
                        {t('me.btc.browserWalletPsbtUnavailable')}
                      </p>
                    ) : null}
                  </div>
                )}
              </div>
            </article>

            <article className="console-card">
              <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                {t('me.btc.protocolTitle')}
              </h3>
              <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                {t('me.btc.protocolBody')}
              </p>
              <div className="mt-4">
                <FieldValueList
                  items={[
                    {
                      label: t('me.fields.walletBalance'),
                      value:
                        btcWallet?.balance != null
                          ? displayBalanceSmart(locale, btcWallet.balance.total, t)
                          : t('common.notYetAvailable'),
                      helpText: t('me.help.walletBalance'),
                    },
                    {
                      label: t('me.fields.balanceHistoryBalance'),
                      value: btcAddressBalanceLoading
                        ? t('actions.reloading')
                        : displayBalanceSmart(locale, btcLatestBalanceRow?.balance, t),
                      helpText: t('me.help.balanceHistoryBalance'),
                    },
                    {
                      label: t('me.fields.balanceHistoryHeight'),
                      value: btcAddressBalanceLoading
                        ? t('actions.reloading')
                        : displayNumber(locale, btcLatestBalanceRow?.block_height, t),
                      helpText: t('me.help.balanceHistoryHeight'),
                    },
                    {
                      label: t('me.fields.balanceHistoryDelta'),
                      value: btcAddressBalanceLoading
                        ? t('actions.reloading')
                        : displayBalanceDeltaSmart(locale, btcLatestBalanceRow?.delta, t),
                      helpText: t('me.help.balanceHistoryDelta'),
                    },
                    {
                      label: t('me.fields.activeMinerPass'),
                      value: btcProtocolLoading
                        ? t('actions.reloading')
                        : btcActivePass?.inscription_id ?? t('me.values.noActivePass'),
                      helpText: t('me.help.activeMinerPass'),
                    },
                    {
                      label: t('me.fields.passState'),
                      value: btcProtocolLoading
                        ? t('actions.reloading')
                        : btcActivePass?.state ?? t('common.notYetAvailable'),
                      helpText: t('me.help.passState'),
                    },
                    {
                      label: t('me.fields.passEnergy'),
                      value: btcProtocolLoading
                        ? t('actions.reloading')
                        : displayNumber(locale, btcActivePassEnergy?.energy ?? null, t),
                      helpText: t('me.help.passEnergy'),
                    },
                    {
                      label: t('me.fields.walletInscriptions'),
                      value:
                        btcWallet != null
                          ? String(btcWallet.inscriptions.length)
                          : t('common.notYetAvailable'),
                      helpText: t('me.help.walletInscriptions'),
                    },
                    {
                      label: t('me.fields.recognizedPasses'),
                      value: btcRecognizedPassesLoading
                        ? t('actions.reloading')
                        : String(btcRecognizedPasses.length),
                      helpText: t('me.help.recognizedPasses'),
                    },
                  ]}
                />
              </div>
              {!btcLookupAddress ? (
                <p className="mt-4 text-sm text-[color:var(--cp-muted)]">
                  {t('me.btc.protocolUnavailable')}
                </p>
              ) : null}
              {btcAddressBalanceError ? (
                <p className="mt-4 text-sm text-[color:var(--cp-danger)]">{btcAddressBalanceError}</p>
              ) : null}
              {btcProtocolError ? (
                <p className="mt-2 text-sm text-[color:var(--cp-danger)]">{btcProtocolError}</p>
              ) : null}
            </article>
          </section>

          <section className="console-card">
            <div className="flex flex-wrap items-start justify-between gap-4">
              <div>
                <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                  {t('me.btc.passListTitle')}
                </h3>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('me.btc.passListBody')}
                </p>
              </div>
              {btcRecognizedPasses.length > 0 ? (
                <span className="status-pill" data-tone="success">
                  {t('me.values.recognizedPassCount', undefined, { count: btcRecognizedPasses.length })}
                </span>
              ) : null}
            </div>
            {btcRecognizedPassesError ? (
              <p className="mt-4 text-sm text-[color:var(--cp-danger)]">{btcRecognizedPassesError}</p>
            ) : null}
            {btcWallet == null ? (
              <p className="mt-4 text-sm text-[color:var(--cp-muted)]">
                {t('me.btc.passListUnavailable')}
              </p>
            ) : btcRecognizedPassesLoading ? (
              <p className="mt-4 text-sm text-[color:var(--cp-muted)]">{t('actions.reloading')}</p>
            ) : btcRecognizedPasses.length === 0 ? (
              <p className="mt-4 text-sm text-[color:var(--cp-muted)]">
                {t('me.btc.passListEmpty')}
              </p>
            ) : (
              <div className="mt-4 overflow-x-auto">
                <table className="console-table">
                  <thead>
                    <tr>
                      <th>{t('fields.inscriptionId')}</th>
                      <th>{t('me.fields.walletInscriptionNumber')}</th>
                      <th>{t('me.fields.passState')}</th>
                      <th>{t('me.fields.passOwner')}</th>
                      <th>{t('me.fields.passEthMain')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {btcRecognizedPasses.map((item) => (
                      <tr key={item.walletInscriptionId}>
                        <td>{item.pass.inscription_id}</td>
                        <td>{displayText(item.walletInscriptionNumber, t)}</td>
                        <td>{item.pass.state}</td>
                        <td>{item.pass.owner}</td>
                        <td>{item.pass.eth_main}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>

          {btcWalletMode === 'dev-regtest' ? (
            <section className="grid gap-4 xl:grid-cols-2">
              <article className="console-card">
                <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                  {t('me.btc.signatureTitle')}
                </h3>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('me.btc.signatureBody')}
                </p>
                <div className="mt-4 grid gap-4">
                  <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                    <span>{t('me.btc.signatureInputLabel')}</span>
                    <textarea
                      className="console-textarea"
                      value={btcDevWalletMessage}
                      onChange={(event) => setBtcDevWalletMessage(event.target.value)}
                      placeholder={t('me.btc.signaturePlaceholder')}
                    />
                  </label>
                  <div className="flex flex-wrap items-center gap-3">
                    <button
                      type="button"
                      className="console-action-button"
                      disabled={!btcWalletConnected || btcDevWalletSigning || btcDevWalletMessage.trim() === ''}
                      onClick={() => void handleSignDevWalletMessage()}
                    >
                      {btcDevWalletSigning ? t('actions.reloading') : t('me.btc.signWithDevWallet')}
                    </button>
                  </div>
                  <FieldValueList
                    items={[
                      {
                        label: t('me.fields.signatureMode'),
                        value: t('me.values.devMessageSignature'),
                        helpText: t('me.help.signatureMode'),
                      },
                      {
                        label: t('me.fields.runtimeNetwork'),
                        value: displayText(btcRuntimeNetwork, t),
                        helpText: t('me.help.runtimeNetwork'),
                      },
                    ]}
                  />
                  {btcDevWalletSignatureError ? (
                    <p className="text-sm text-[color:var(--cp-danger)]">{btcDevWalletSignatureError}</p>
                  ) : null}
                  {btcDevWalletSignature ? (
                    <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                      <span>{t('me.btc.signatureOutputLabel')}</span>
                      <textarea className="console-textarea" value={btcDevWalletSignature} readOnly />
                    </label>
                  ) : null}
                </div>
              </article>

              <article className="console-card">
                <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                  {t('me.btc.psbtTitle')}
                </h3>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('me.btc.psbtBody')}
                </p>
                <div className="mt-4 grid gap-4">
                  <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                    <span>{t('me.btc.psbtInputLabel')}</span>
                    <textarea
                      className="console-textarea"
                      value={btcDevWalletPsbt}
                      onChange={(event) => setBtcDevWalletPsbt(event.target.value)}
                      placeholder={t('me.btc.psbtPlaceholder')}
                    />
                  </label>
                  <div className="flex flex-wrap items-center gap-3">
                    <button
                      type="button"
                      className="console-action-button"
                      disabled={!btcWalletConnected || btcDevWalletPsbtSigning || btcDevWalletPsbt.trim() === ''}
                      onClick={() => void handleSignDevWalletPsbt(false)}
                    >
                      {btcDevWalletPsbtSigning ? t('actions.reloading') : t('me.btc.signPsbt')}
                    </button>
                    <button
                      type="button"
                      className="console-secondary-button"
                      disabled={!btcWalletConnected || btcDevWalletPsbtSigning || btcDevWalletPsbt.trim() === ''}
                      onClick={() => void handleSignDevWalletPsbt(true)}
                    >
                      {btcDevWalletPsbtSigning ? t('actions.reloading') : t('me.btc.signAndFinalizePsbt')}
                    </button>
                  </div>
                  <FieldValueList
                    items={[
                      {
                        label: t('me.fields.signatureMode'),
                        value: t('me.values.devPsbtSignature'),
                        helpText: t('me.help.signatureMode'),
                      },
                      {
                        label: t('me.fields.psbtInputFormat'),
                        value: displayText(btcDevWalletPsbtResult?.inputFormat ?? null, t),
                        helpText: t('me.help.psbtInputFormat'),
                      },
                      {
                        label: t('me.fields.runtimeNetwork'),
                        value: displayText(btcRuntimeNetwork, t),
                        helpText: t('me.help.runtimeNetwork'),
                      },
                      {
                        label: t('me.fields.psbtSignedInputs'),
                        value: displayNumber(locale, btcDevWalletPsbtResult?.signedInputs ?? null, t),
                        helpText: t('me.help.psbtSignedInputs'),
                      },
                      {
                        label: t('me.fields.psbtFinalizeState'),
                        value:
                          btcDevWalletPsbtResult == null
                            ? t('common.notYetAvailable')
                            : btcDevWalletPsbtResult.finalized
                              ? t('states.completed')
                              : t('states.pending'),
                        helpText: t('me.help.psbtFinalizeState'),
                      },
                    ]}
                  />
                  {btcDevWalletPsbtError ? (
                    <p className="text-sm text-[color:var(--cp-danger)]">{btcDevWalletPsbtError}</p>
                  ) : null}
                  {btcDevWalletPsbtResult ? (
                    <>
                      <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                        <span>{t('me.btc.psbtOutputLabel')}</span>
                        <textarea className="console-textarea" value={btcDevWalletPsbtResult.outputPsbt} readOnly />
                      </label>
                      {btcDevWalletPsbtResult.extractedTxHex ? (
                        <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                          <span>{t('me.btc.extractedTxLabel')}</span>
                          <textarea
                            className="console-textarea"
                            value={btcDevWalletPsbtResult.extractedTxHex}
                            readOnly
                          />
                        </label>
                      ) : null}
                    </>
                  ) : null}
                </div>
              </article>
            </section>
          ) : null}
        </>
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
