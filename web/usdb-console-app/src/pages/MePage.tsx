import { useEffect, useMemo, useState } from 'react'
import { Navigate, NavLink, useParams } from 'react-router-dom'
import { FieldValueList } from '../components/FieldValueList'
import { InlineHelpTooltip } from '../components/InlineHelpTooltip'
import {
  fetchBalanceHistorySingleBalance,
  fetchBtcWorldSimDevSigner,
  fetchBtcWorldSimIdentities,
  prepareBtcMintDraft,
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
import type {
  AddressBalanceRow,
  BtcMintPrepareResponse,
  BtcWorldSimIdentitiesResponse,
  OverviewResponse,
  PassEnergySnapshot,
  PassSnapshot,
} from '../lib/types'
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
type BtcIdentitySource = 'browser_wallet' | 'world_sim_agent' | 'manual_address'

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

function parseMintPrevInput(value: string) {
  return value
    .split(/[\n,]+/)
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
}

function runtimeProfileLabel(
  profile: string,
  t: MePageProps['t'],
) {
  if (profile === 'development') return t('me.values.runtimeDevelopment')
  if (profile === 'public') return t('me.values.runtimePublic')
  return t('me.values.runtimeUnknown')
}

function identitySourceLabel(
  source: BtcIdentitySource,
  t: MePageProps['t'],
) {
  if (source === 'browser_wallet') return t('me.values.identitySourceBrowserWallet')
  if (source === 'world_sim_agent') return t('me.values.identitySourceWorldSimAgent')
  return t('me.values.identitySourceManualAddress')
}

function signerSourceLabel(
  mode: BtcWalletMode,
  t: MePageProps['t'],
) {
  if (mode === 'dev-regtest') return t('me.values.signerSourceDevSigner')
  return t('me.values.signerSourceBrowserWallet')
}

export function MePage({ data, locale, t }: MePageProps) {
  const { identityKind } = useParams()
  const activeIdentity = normalizeIdentityKind(identityKind)
  const [ethAddress, setEthAddress] = useState('')
  const [btcAddress, setBtcAddress] = useState('')
  const [btcWallet, setBtcWallet] = useState<BtcWalletSnapshot | null>(null)
  const [btcBrowserWalletSnapshot, setBtcBrowserWalletSnapshot] = useState<BtcWalletSnapshot | null>(null)
  const [btcWalletMode, setBtcWalletMode] = useState<BtcWalletMode>('browser')
  const [btcIdentitySource, setBtcIdentitySource] = useState<BtcIdentitySource>('manual_address')
  const [btcWalletLoading, setBtcWalletLoading] = useState(false)
  const [btcWalletError, setBtcWalletError] = useState<string | null>(null)
  const [btcWorldSim, setBtcWorldSim] = useState<BtcWorldSimIdentitiesResponse | null>(null)
  const [btcWorldSimLoading, setBtcWorldSimLoading] = useState(false)
  const [btcWorldSimError, setBtcWorldSimError] = useState<string | null>(null)
  const [btcSelectedWorldSimWalletName, setBtcSelectedWorldSimWalletName] = useState('')
  const [btcAutoDevSignerLoading, setBtcAutoDevSignerLoading] = useState(false)
  const [btcAutoDevSignerError, setBtcAutoDevSignerError] = useState<string | null>(null)
  const [btcBrowserWalletMessage, setBtcBrowserWalletMessage] = useState('')
  const [btcBrowserWalletSignature, setBtcBrowserWalletSignature] = useState<string | null>(null)
  const [btcBrowserWalletSigning, setBtcBrowserWalletSigning] = useState(false)
  const [btcBrowserWalletSignatureError, setBtcBrowserWalletSignatureError] = useState<string | null>(null)
  const [btcBrowserWalletPsbt, setBtcBrowserWalletPsbt] = useState('')
  const [btcBrowserWalletPsbtSigning, setBtcBrowserWalletPsbtSigning] = useState(false)
  const [btcBrowserWalletPsbtError, setBtcBrowserWalletPsbtError] = useState<string | null>(null)
  const [btcBrowserWalletPsbtResult, setBtcBrowserWalletPsbtResult] =
    useState<BtcWalletPsbtSignatureResult | null>(null)
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
  const [btcMintEthMain, setBtcMintEthMain] = useState('')
  const [btcMintEthCollab, setBtcMintEthCollab] = useState('')
  const [btcMintPrev, setBtcMintPrev] = useState('')
  const [btcMintPrepareLoading, setBtcMintPrepareLoading] = useState(false)
  const [btcMintPrepareError, setBtcMintPrepareError] = useState<string | null>(null)
  const [btcMintPrepareResult, setBtcMintPrepareResult] =
    useState<BtcMintPrepareResponse | null>(null)

  if (!activeIdentity) {
    return <Navigate to="/me/eth" replace />
  }

  const ethwReachable = Boolean(data?.services.ethw.reachable)
  const ethwConsensusReady = Boolean(data?.services.ethw.data?.consensus_ready)
  const sourcedaoReady = Boolean(data?.bootstrap.sourcedao_bootstrap_marker.exists)
  const ordAvailable = Boolean(data?.capabilities.ord_available)
  const btcConsoleMode = data?.capabilities.btc_console_mode ?? 'read_only'
  const btcRuntimeProfile = data?.capabilities.btc_runtime_profile ?? 'unknown'
  const balanceHistoryReady = Boolean(data?.services.balance_history.data?.query_ready)
  const usdbIndexerReady = Boolean(data?.services.usdb_indexer.data?.query_ready)
  const hasInjectedBtcWallet = Boolean(detectBtcWalletProvider())
  const btcWalletAdapterCapabilities = getBtcWalletAdapterCapabilities(btcWalletMode)
  const btcWalletConnected = Boolean(btcWallet?.address)
  const btcBrowserWalletConnected = Boolean(btcBrowserWalletSnapshot?.address)
  const btcRuntimeNetwork = normalizeBtcRuntimeNetwork(data?.services.btc_node.data?.chain)
  const btcSignerReportedNetwork = normalizeInjectedWalletNetwork(btcWallet?.network)
  const btcSignerDerivedNetwork = inferBtcAddressNetwork(btcWallet?.address)
  const btcEffectiveSignerNetwork = btcSignerDerivedNetwork ?? btcSignerReportedNetwork
  const btcBrowserWalletReportedNetwork = normalizeInjectedWalletNetwork(
    btcBrowserWalletSnapshot?.network,
  )
  const btcBrowserWalletDerivedNetwork = inferBtcAddressNetwork(btcBrowserWalletSnapshot?.address)
  const btcEffectiveBrowserWalletNetwork =
    btcBrowserWalletDerivedNetwork ?? btcBrowserWalletReportedNetwork
  const btcSelectedWorldSimIdentity =
    btcWorldSim?.identities.find((item) => item.wallet_name === btcSelectedWorldSimWalletName) ?? null
  const btcLookupAddress =
    (btcIdentitySource === 'browser_wallet'
      ? btcBrowserWalletSnapshot?.address
      : btcIdentitySource === 'world_sim_agent'
        ? btcSelectedWorldSimIdentity?.owner_address
        : btcAddress.trim()) || null
  const btcLookupAddressDerivedNetwork = inferBtcAddressNetwork(btcLookupAddress)
  const btcLookupNetworkMismatch =
    btcRuntimeNetwork != null &&
    btcLookupAddressDerivedNetwork != null &&
    btcRuntimeNetwork !== btcLookupAddressDerivedNetwork
  const btcLookupNetworkMismatchMessage = btcLookupNetworkMismatch
    ? t('me.btc.networkMismatch', undefined, {
        walletNetwork: btcLookupAddressDerivedNetwork,
        runtimeNetwork: btcRuntimeNetwork,
      })
    : null
  const btcBrowserWalletNetworkMismatch =
    btcRuntimeNetwork != null &&
    btcEffectiveBrowserWalletNetwork != null &&
    btcRuntimeNetwork !== btcEffectiveBrowserWalletNetwork
  const btcBrowserWalletNetworkMismatchMessage = btcBrowserWalletNetworkMismatch
    ? t('me.btc.networkMismatch', undefined, {
        walletNetwork: btcEffectiveBrowserWalletNetwork,
        runtimeNetwork: btcRuntimeNetwork,
      })
    : null
  const btcRuntimeAllowsWrite = btcRuntimeProfile !== 'unknown'
  const btcMintIdentityReady =
    btcRuntimeProfile !== 'public' ||
    (btcIdentitySource === 'browser_wallet' && btcBrowserWalletConnected)
  const btcMintCapabilityReady =
    btcRuntimeAllowsWrite &&
    btcMintIdentityReady &&
    !btcLookupNetworkMismatch &&
    ordAvailable &&
    balanceHistoryReady &&
    usdbIndexerReady
  const btcDevSignerAutoManaged =
    btcRuntimeProfile === 'development' &&
    btcWalletMode === 'dev-regtest' &&
    btcIdentitySource === 'world_sim_agent' &&
    Boolean(btcSelectedWorldSimIdentity)

  const activeAddressValue =
    activeIdentity === 'eth'
      ? ethAddress.trim()
      : btcLookupAddress ?? ''

  const activeAddressStatus =
    activeIdentity === 'eth'
      ? activeAddressValue === ''
        ? t('me.identity.notSet')
        : validateEthAddress(activeAddressValue)
          ? t('me.identity.validFormat')
          : t('me.identity.checkFormat')
      : activeAddressValue === ''
        ? t('me.identity.notSet')
        : btcLookupNetworkMismatch
          ? t('me.identity.networkMismatch')
          : btcLookupAddressDerivedNetwork
            ? t('me.identity.validFormat')
            : t('me.identity.checkFormat')
  const btcLatestBalanceRow = useMemo(
    () =>
      btcAddressBalanceRows.length > 0
        ? btcAddressBalanceRows[btcAddressBalanceRows.length - 1]
        : null,
    [btcAddressBalanceRows],
  )
  const btcMintPrepareEnabled =
    Boolean(btcLookupAddress) &&
    btcMintCapabilityReady &&
    btcMintEthMain.trim() !== '' &&
    !btcMintPrepareLoading
  const btcMintDraftRequestJson = btcMintPrepareResult
    ? JSON.stringify(btcMintPrepareResult.prepare_request, null, 2)
    : ''
  const btcMintSummaryItems = [
    {
      label: t('me.fields.prepareMode'),
      value: displayText(btcMintPrepareResult?.prepare_mode, t),
      helpText: t('me.help.prepareMode'),
      tone: btcMintPrepareResult == null ? null : btcMintPrepareResult.eligible ? 'success' : 'warning',
    },
    {
      label: t('me.fields.ownerScriptHash'),
      value: displayText(btcMintPrepareResult?.owner_script_hash, t),
      helpText: t('me.help.ownerScriptHash'),
      tone: null,
    },
    {
      label: t('me.fields.activeMinerPass'),
      value: displayText(btcMintPrepareResult?.active_pass?.inscription_id, t),
      helpText: t('me.help.activeMinerPass'),
      tone: null,
    },
    {
      label: t('me.fields.suggestedPrev'),
      value:
        btcMintPrepareResult == null
          ? t('common.notYetAvailable')
          : btcMintPrepareResult.suggested_prev.length > 0
            ? btcMintPrepareResult.suggested_prev.join(', ')
            : t('me.values.noneSuggested'),
      helpText: t('me.help.suggestedPrev'),
      tone: null,
    },
    {
      label: t('me.fields.ordSyncGap'),
      value: displayNumber(locale, btcMintPrepareResult?.runtime.ord_sync_gap ?? null, t),
      helpText: t('me.help.ordSyncGap'),
      tone: null,
    },
  ] as const
  const btcSignerSourceValue = signerSourceLabel(btcWalletMode, t)
  const btcIdentitySourceValue = identitySourceLabel(btcIdentitySource, t)
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
            label: t('me.fields.runtimeProfile'),
            value: runtimeProfileLabel(btcRuntimeProfile, t),
            helpText: t('me.help.runtimeProfile'),
          },
          {
            label: t('me.fields.signerSource'),
            value: btcSignerSourceValue,
            helpText: t('me.help.signerSource'),
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
              btcMintCapabilityReady
                ? t('me.values.mintReady')
                : t('me.values.readOnly'),
            helpText: t('me.help.mintCapability'),
          },
          {
            label: t('me.fields.protocolData'),
            value: btcLookupNetworkMismatch
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
            value: displayText(
              btcLookupAddressDerivedNetwork ?? btcEffectiveBrowserWalletNetwork ?? btcEffectiveSignerNetwork,
              t,
            ),
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
              btcMintCapabilityReady
                ? t('me.values.btcMintAndPass')
                : t('me.values.btcReadOnlyData'),
            helpText: t('me.help.primaryAction'),
          },
          {
            label: t('me.fields.runtimeGate'),
            value:
              btcMintCapabilityReady
                ? t('me.values.mintReady')
                : t('me.values.readOnly'),
            helpText: t('me.help.runtimeGate'),
          },
          {
            label: t('me.fields.identitySource'),
            value: btcIdentitySourceValue,
            helpText: t('me.help.identitySource'),
          },
          {
            label: t('me.fields.signerSource'),
            value: btcSignerSourceValue,
            helpText: t('me.help.signerSource'),
          },
          {
            label: t('me.fields.currentAddress'),
            value: displayText(activeAddressValue || null, t),
            helpText: t('me.help.currentBtcAddress'),
          },
        ]

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    if (btcRuntimeProfile === 'public') {
      setBtcWalletMode('browser')
      setBtcIdentitySource((current) =>
        current === 'world_sim_agent'
          ? btcBrowserWalletConnected
            ? 'browser_wallet'
            : 'manual_address'
          : current,
      )
      return
    }
    if (btcRuntimeProfile === 'development') {
      setBtcWalletMode('dev-regtest')
    }
  }, [activeIdentity, btcBrowserWalletConnected, btcRuntimeProfile])

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    let cancelled = false

    void readBtcWalletSnapshotByMode(btcWalletMode)
      .then((snapshot) => {
        if (cancelled) return
        setBtcWallet(snapshot)
        if (btcWalletMode === 'browser') {
          setBtcBrowserWalletSnapshot(snapshot)
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
    if (activeIdentity !== 'btc') return
    if (btcRuntimeProfile !== 'development') {
      setBtcWorldSim(null)
      setBtcWorldSimError(null)
      setBtcSelectedWorldSimWalletName('')
      return
    }

    let cancelled = false
    setBtcWorldSimLoading(true)
    setBtcWorldSimError(null)

    void fetchBtcWorldSimIdentities()
      .then((response) => {
        if (cancelled) return
        setBtcWorldSim(response)
        setBtcWorldSimError(response.error ?? null)
        setBtcSelectedWorldSimWalletName((current) => {
          if (response.identities.some((item) => item.wallet_name === current)) {
            return current
          }
          return response.identities.length === 1 ? response.identities[0].wallet_name : ''
        })
      })
      .catch((error: Error) => {
        if (cancelled) return
        setBtcWorldSim(null)
        setBtcWorldSimError(error.message)
        setBtcSelectedWorldSimWalletName('')
      })
      .finally(() => {
        if (cancelled) return
        setBtcWorldSimLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [activeIdentity, btcRuntimeProfile])

  useEffect(() => {
    if (activeIdentity !== 'btc' || btcRuntimeProfile !== 'development') return
    if (!btcWorldSim?.available || btcWorldSim.identities.length !== 1) return

    const identity = btcWorldSim.identities[0]
    setBtcSelectedWorldSimWalletName((current) => current || identity.wallet_name)
    setBtcIdentitySource((current) => {
      if (current !== 'manual_address') return current
      return btcAddress.trim() ? current : 'world_sim_agent'
    })
  }, [activeIdentity, btcAddress, btcRuntimeProfile, btcWorldSim])

  useEffect(() => {
    if (activeIdentity !== 'btc' || btcRuntimeProfile !== 'development') {
      setBtcAutoDevSignerError(null)
      setBtcAutoDevSignerLoading(false)
      return
    }
    if (btcIdentitySource !== 'world_sim_agent' || !btcSelectedWorldSimIdentity) {
      setBtcAutoDevSignerError(null)
      setBtcAutoDevSignerLoading(false)
      return
    }

    let cancelled = false
    setBtcAutoDevSignerLoading(true)
    setBtcAutoDevSignerError(null)
    clearDevRegtestWallet()
    setBtcWallet(null)
    setBtcWalletError(null)

    void fetchBtcWorldSimDevSigner(btcSelectedWorldSimIdentity.wallet_name)
      .then(async (response) => {
        if (cancelled) return
        if (!response.available || !response.wif || !response.owner_address) {
          throw new Error(
            response.error ??
              'The selected world-sim identity did not expose dev signer material.',
          )
        }

        const snapshot = await importDevRegtestWallet({
          wif: response.wif,
          address: response.owner_address,
        })
        if (cancelled) return
        setBtcWalletMode('dev-regtest')
        setBtcWallet(snapshot)
        setBtcDevWalletAddress(response.owner_address)
        setBtcDevWalletWif('')
        setBtcWalletError(null)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setBtcAutoDevSignerError(error.message)
      })
      .finally(() => {
        if (cancelled) return
        setBtcAutoDevSignerLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [
    activeIdentity,
    btcIdentitySource,
    btcRuntimeProfile,
    btcSelectedWorldSimIdentity,
  ])

  useEffect(() => {
    if (activeIdentity !== 'btc' || btcRuntimeProfile !== 'public') return
    if (!btcBrowserWalletConnected || btcAddress.trim()) return
    setBtcIdentitySource((current) =>
      current === 'manual_address' || current === 'world_sim_agent'
        ? 'browser_wallet'
        : current,
    )
  }, [activeIdentity, btcAddress, btcBrowserWalletConnected, btcRuntimeProfile])

  useEffect(() => {
    setBtcWalletError(null)
    setBtcBrowserWalletSignature(null)
    setBtcBrowserWalletSignatureError(null)
    setBtcBrowserWalletPsbtResult(null)
    setBtcBrowserWalletPsbtError(null)
    setBtcDevWalletSignature(null)
    setBtcDevWalletSignatureError(null)
    setBtcDevWalletPsbtResult(null)
    setBtcDevWalletPsbtError(null)
    setBtcMintPrepareError(null)
    setBtcMintPrepareResult(null)
  }, [btcWalletMode])

  useEffect(() => {
    setBtcMintPrepareError(null)
    setBtcMintPrepareResult(null)
  }, [btcLookupAddress, btcLookupNetworkMismatchMessage, btcMintEthMain, btcMintEthCollab, btcMintPrev])

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    if (btcLookupNetworkMismatchMessage) {
      setBtcAddressBalanceRows([])
      setBtcAddressBalanceError(btcLookupNetworkMismatchMessage)
      setBtcActivePass(null)
      setBtcActivePassEnergy(null)
      setBtcProtocolError(btcLookupNetworkMismatchMessage)
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
    btcLookupNetworkMismatchMessage,
    usdbIndexerReady,
  ])

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    if (btcBrowserWalletNetworkMismatchMessage) {
      setBtcRecognizedPasses([])
      setBtcRecognizedPassesError(btcBrowserWalletNetworkMismatchMessage)
      return
    }
    if (!btcBrowserWalletSnapshot?.inscriptions.length || !usdbIndexerReady) {
      setBtcRecognizedPasses([])
      setBtcRecognizedPassesError(null)
      return
    }

    let cancelled = false
    setBtcRecognizedPassesLoading(true)
    setBtcRecognizedPassesError(null)

    void Promise.all(
      btcBrowserWalletSnapshot.inscriptions.map(async (walletInscription) => {
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
  }, [
    activeIdentity,
    btcBrowserWalletNetworkMismatchMessage,
    btcBrowserWalletSnapshot?.inscriptions,
    usdbIndexerReady,
  ])

  async function handleConnectBtcWallet() {
    setBtcWalletLoading(true)
    setBtcWalletError(null)

    try {
      const snapshot = await connectBtcWalletByMode('browser')
      setBtcWallet(snapshot)
      setBtcBrowserWalletSnapshot(snapshot)
      if (snapshot.address && btcRuntimeProfile === 'public') {
        setBtcIdentitySource('browser_wallet')
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
      if (btcIdentitySource === 'manual_address' && !btcAddress.trim()) {
        setBtcAddress(snapshot.address ?? '')
      }
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

  async function handleSignBrowserWalletMessage() {
    setBtcBrowserWalletSigning(true)
    setBtcBrowserWalletSignature(null)
    setBtcBrowserWalletSignatureError(null)

    try {
      const result = await signBtcWalletMessage('browser', btcBrowserWalletMessage)
      setBtcBrowserWalletSignature(result.signature)
    } catch (error) {
      setBtcBrowserWalletSignatureError(error instanceof Error ? error.message : String(error))
    } finally {
      setBtcBrowserWalletSigning(false)
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

  async function handleSignBrowserWalletPsbt() {
    setBtcBrowserWalletPsbtSigning(true)
    setBtcBrowserWalletPsbtResult(null)
    setBtcBrowserWalletPsbtError(null)

    try {
      const result = await signBtcWalletPsbt('browser', btcBrowserWalletPsbt)
      setBtcBrowserWalletPsbtResult(result)
    } catch (error) {
      setBtcBrowserWalletPsbtError(error instanceof Error ? error.message : String(error))
    } finally {
      setBtcBrowserWalletPsbtSigning(false)
    }
  }

  async function handlePrepareBtcMintDraft() {
    if (!btcLookupAddress) {
      setBtcMintPrepareError(t('me.btc.mintOwnerRequired'))
      setBtcMintPrepareResult(null)
      return
    }

    setBtcMintPrepareLoading(true)
    setBtcMintPrepareError(null)
    setBtcMintPrepareResult(null)

    try {
      const result = await prepareBtcMintDraft({
        owner_address: btcLookupAddress,
        eth_main: btcMintEthMain,
        eth_collab: btcMintEthCollab.trim() || null,
        prev: parseMintPrevInput(btcMintPrev),
      })
      setBtcMintPrepareResult(result)
    } catch (error) {
      setBtcMintPrepareError(error instanceof Error ? error.message : String(error))
    } finally {
      setBtcMintPrepareLoading(false)
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
                  {btcRuntimeProfile !== 'public' ? (
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
                  ) : null}
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
          {activeIdentity === 'btc' && btcLookupNetworkMismatchMessage ? (
            <p className="mt-2 text-sm text-[color:var(--cp-danger)]">
              {btcLookupNetworkMismatchMessage}
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
            {activeIdentity === 'eth' ? (
              <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                <span>{t('me.identity.ethInputLabel')}</span>
                <input
                  className="console-input"
                  value={ethAddress}
                  onChange={(event) => setEthAddress(event.target.value)}
                  placeholder={t('me.identity.ethPlaceholder')}
                />
              </label>
            ) : (
              <>
                <div className="flex flex-wrap items-center gap-2">
                  <button
                    type="button"
                    className={
                      btcIdentitySource === 'browser_wallet'
                        ? 'console-action-button'
                        : 'console-secondary-button'
                    }
                    onClick={() => setBtcIdentitySource('browser_wallet')}
                  >
                    {t('me.values.identitySourceBrowserWallet')}
                  </button>
                  {btcRuntimeProfile === 'development' ? (
                    <button
                      type="button"
                      className={
                        btcIdentitySource === 'world_sim_agent'
                          ? 'console-action-button'
                          : 'console-secondary-button'
                      }
                      onClick={() => setBtcIdentitySource('world_sim_agent')}
                    >
                      {t('me.values.identitySourceWorldSimAgent')}
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className={
                      btcIdentitySource === 'manual_address'
                        ? 'console-action-button'
                        : 'console-secondary-button'
                    }
                    onClick={() => setBtcIdentitySource('manual_address')}
                  >
                    {t('me.values.identitySourceManualAddress')}
                  </button>
                </div>

                {btcIdentitySource === 'manual_address' ? (
                  <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                    <span>{t('me.identity.btcInputLabel')}</span>
                    <input
                      className="console-input"
                      value={btcAddress}
                      onChange={(event) => setBtcAddress(event.target.value)}
                      placeholder={t('me.identity.btcPlaceholder')}
                    />
                  </label>
                ) : null}

                {btcIdentitySource === 'browser_wallet' && !btcBrowserWalletConnected ? (
                  <p className="text-sm text-[color:var(--cp-muted)]">
                    {t('me.btc.browserIdentityUnavailable')}
                  </p>
                ) : null}

                {btcIdentitySource === 'world_sim_agent' ? (
                  <div className="grid gap-3">
                    <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                      <span>{t('me.btc.worldSimSelectorLabel')}</span>
                      <select
                        className="console-input"
                        value={btcSelectedWorldSimWalletName}
                        onChange={(event) => setBtcSelectedWorldSimWalletName(event.target.value)}
                        disabled={btcWorldSimLoading || !btcWorldSim?.identities.length}
                      >
                        <option value="">{t('me.values.selectWorldSimAgent')}</option>
                        {(btcWorldSim?.identities ?? []).map((identity) => (
                          <option key={identity.wallet_name} value={identity.wallet_name}>
                            {`Agent ${identity.agent_id} | ${identity.wallet_name} | ${identity.owner_address}`}
                          </option>
                        ))}
                      </select>
                    </label>
                    <FieldValueList
                      items={[
                        {
                          label: t('me.fields.worldSimIdentity'),
                          value: displayText(btcSelectedWorldSimIdentity?.wallet_name, t),
                          helpText: t('me.help.worldSimIdentity'),
                        },
                        {
                          label: t('me.fields.currentAddress'),
                          value: displayText(btcSelectedWorldSimIdentity?.owner_address, t),
                          helpText: t('me.help.currentBtcAddress'),
                        },
                      ]}
                    />
                    {btcWorldSimLoading ? (
                      <p className="text-sm text-[color:var(--cp-muted)]">{t('actions.reloading')}</p>
                    ) : null}
                    {btcAutoDevSignerLoading ? (
                      <p className="text-sm text-[color:var(--cp-muted)]">
                        {t('me.btc.devSignerAutoSyncing')}
                      </p>
                    ) : null}
                    {btcWorldSimError ? (
                      <p className="text-sm text-[color:var(--cp-danger)]">{btcWorldSimError}</p>
                    ) : null}
                    {btcAutoDevSignerError ? (
                      <p className="text-sm text-[color:var(--cp-danger)]">{btcAutoDevSignerError}</p>
                    ) : null}
                    {!btcWorldSimLoading && !btcWorldSimError && !btcWorldSim?.identities.length ? (
                      <p className="text-sm text-[color:var(--cp-muted)]">
                        {t('me.btc.worldSimUnavailable')}
                      </p>
                    ) : null}
                  </div>
                ) : null}
              </>
            )}
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
                    {btcDevSignerAutoManaged && !btcAutoDevSignerError ? (
                      <p className="rounded-2xl border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-4 py-3 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {t('me.btc.devWalletWorldSimManaged')}
                      </p>
                    ) : (
                      <>
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
                      </>
                    )}
                    {btcAutoDevSignerLoading ? (
                      <p className="text-sm text-[color:var(--cp-muted)]">
                        {t('me.btc.devSignerAutoSyncing')}
                      </p>
                    ) : null}
                    {btcAutoDevSignerError ? (
                      <p className="text-sm text-[color:var(--cp-danger)]">{btcAutoDevSignerError}</p>
                    ) : null}
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
                          value: displayText(btcWallet?.network ?? btcEffectiveSignerNetwork, t),
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
                          value: displayText(btcWallet?.network ?? btcEffectiveSignerNetwork, t),
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
                        btcBrowserWalletSnapshot?.balance != null
                          ? displayBalanceSmart(locale, btcBrowserWalletSnapshot.balance.total, t)
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
                        btcBrowserWalletSnapshot != null
                          ? String(btcBrowserWalletSnapshot.inscriptions.length)
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
                  {t('me.btc.mintDraftTitle')}
                </h3>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('me.btc.mintDraftBody')}
                </p>
              </div>
              {btcMintPrepareResult ? (
                <span
                  className="status-pill"
                  data-tone={btcMintPrepareResult.eligible ? 'success' : 'warning'}
                >
                  {btcMintPrepareResult.eligible
                    ? t('me.values.mintDraftReady')
                    : t('me.values.mintDraftBlocked')}
                </span>
              ) : null}
            </div>
            <div className="mt-4 grid gap-4 xl:grid-cols-2">
              <div className="grid gap-4">
                <FieldValueList
                  items={[
                    {
                      label: t('me.fields.currentAddress'),
                      value: displayText(btcLookupAddress, t),
                      helpText: t('me.help.currentBtcAddress'),
                    },
                    {
                      label: t('me.fields.runtimeNetwork'),
                      value: displayText(btcRuntimeNetwork, t),
                      helpText: t('me.help.runtimeNetwork'),
                    },
                    {
                      label: t('me.fields.mintCapability'),
                      value:
                        btcMintCapabilityReady
                          ? t('me.values.mintReady')
                          : t('me.values.readOnly'),
                      helpText: t('me.help.mintCapability'),
                    },
                  ]}
                />
                <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                  <span>{t('me.btc.mintEthMainLabel')}</span>
                  <input
                    className="console-input"
                    value={btcMintEthMain}
                    onChange={(event) => setBtcMintEthMain(event.target.value)}
                    placeholder={t('me.btc.mintEthMainPlaceholder')}
                  />
                </label>
                <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                  <span>{t('me.btc.mintEthCollabLabel')}</span>
                  <input
                    className="console-input"
                    value={btcMintEthCollab}
                    onChange={(event) => setBtcMintEthCollab(event.target.value)}
                    placeholder={t('me.btc.mintEthCollabPlaceholder')}
                  />
                </label>
                <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                  <span>{t('me.btc.mintPrevLabel')}</span>
                  <textarea
                    className="console-textarea"
                    value={btcMintPrev}
                    onChange={(event) => setBtcMintPrev(event.target.value)}
                    placeholder={t('me.btc.mintPrevPlaceholder')}
                  />
                </label>
                <div className="flex flex-wrap items-center gap-3">
                  <button
                    type="button"
                    className="console-action-button"
                    disabled={!btcMintPrepareEnabled}
                    onClick={() => void handlePrepareBtcMintDraft()}
                  >
                    {btcMintPrepareLoading ? t('actions.reloading') : t('me.btc.prepareMintDraft')}
                  </button>
                </div>
                {btcRuntimeProfile === 'public' && btcIdentitySource !== 'browser_wallet' ? (
                  <p className="text-sm text-[color:var(--cp-warning)]">
                    {t('me.btc.publicMintBrowserWalletOnly')}
                  </p>
                ) : null}
                {btcLookupNetworkMismatchMessage ? (
                  <p className="text-sm text-[color:var(--cp-danger)]">
                    {btcLookupNetworkMismatchMessage}
                  </p>
                ) : null}
                {btcMintPrepareError ? (
                  <p className="text-sm text-[color:var(--cp-danger)]">{btcMintPrepareError}</p>
                ) : null}
              </div>

              <div className="grid gap-4">
                <div className="grid gap-3 sm:grid-cols-2">
                  {btcMintSummaryItems.map((item) => (
                    <div
                      key={`${item.label}:${item.value}`}
                      className="rounded-[20px] border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-4 py-4"
                    >
                      <div className="inline-flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-[color:var(--cp-muted)]">
                        <span>{item.label}</span>
                        <InlineHelpTooltip text={item.helpText} />
                      </div>
                      {item.tone ? (
                        <span className="mt-3 inline-flex status-pill" data-tone={item.tone}>
                          {item.value}
                        </span>
                      ) : (
                        <p className="mt-3 min-h-12 break-all text-sm font-semibold leading-6 text-[color:var(--cp-text)]">
                          {item.value}
                        </p>
                      )}
                    </div>
                  ))}
                </div>
                {btcMintPrepareResult?.blockers.length ? (
                  <div className="rounded-2xl border border-[color:var(--cp-danger)]/20 bg-[color:var(--cp-danger)]/6 px-4 py-3">
                    <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                      {t('me.btc.mintBlockersTitle')}
                    </h4>
                    <ul className="mt-2 grid gap-2 text-sm leading-6 text-[color:var(--cp-danger)]">
                      {btcMintPrepareResult.blockers.map((item) => (
                        <li key={item}>{item}</li>
                      ))}
                    </ul>
                  </div>
                ) : null}
                {btcMintPrepareResult?.warnings.length ? (
                  <div className="rounded-2xl border border-[color:var(--cp-warning)]/25 bg-[color:var(--cp-warning)]/8 px-4 py-3">
                    <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                      {t('me.btc.mintWarningsTitle')}
                    </h4>
                    <ul className="mt-2 grid gap-2 text-sm leading-6 text-[color:var(--cp-warning)]">
                      {btcMintPrepareResult.warnings.map((item) => (
                        <li key={item}>{item}</li>
                      ))}
                    </ul>
                  </div>
                ) : null}
              </div>
            </div>
            {btcMintPrepareResult ? (
              <div className="mt-4 grid gap-4 xl:grid-cols-2">
                <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                  <span>{t('me.btc.mintPayloadLabel')}</span>
                  <textarea
                    className="console-textarea"
                    value={btcMintPrepareResult.inscription_payload_json}
                    readOnly
                  />
                </label>
                <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                  <span>{t('me.btc.mintRequestLabel')}</span>
                  <textarea className="console-textarea" value={btcMintDraftRequestJson} readOnly />
                </label>
              </div>
            ) : null}
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
            {btcBrowserWalletSnapshot == null ? (
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

          {btcRuntimeProfile === 'development' && btcWalletMode === 'dev-regtest' ? (
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
          ) : btcRuntimeProfile === 'development' && btcWalletMode === 'browser' ? (
            <section className="grid gap-4 xl:grid-cols-2">
              <article className="console-card">
                <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                  {t('me.btc.browserSignatureTitle')}
                </h3>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('me.btc.browserSignatureBody')}
                </p>
                <div className="mt-4 grid gap-4">
                  <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                    <span>{t('me.btc.signatureInputLabel')}</span>
                    <textarea
                      className="console-textarea"
                      value={btcBrowserWalletMessage}
                      onChange={(event) => setBtcBrowserWalletMessage(event.target.value)}
                      placeholder={t('me.btc.signaturePlaceholder')}
                    />
                  </label>
                  <div className="flex flex-wrap items-center gap-3">
                    <button
                      type="button"
                      className="console-action-button"
                      disabled={
                        !btcWalletConnected ||
                        !btcWalletAdapterCapabilities?.canSignMessage ||
                        btcBrowserWalletSigning ||
                        btcBrowserWalletMessage.trim() === ''
                      }
                      onClick={() => void handleSignBrowserWalletMessage()}
                    >
                      {btcBrowserWalletSigning ? t('actions.reloading') : t('me.btc.signWithBrowserWallet')}
                    </button>
                  </div>
                  <FieldValueList
                    items={[
                      {
                        label: t('me.fields.signatureMode'),
                        value: btcWalletAdapterCapabilities?.canSignMessage
                          ? t('me.values.browserMessageSignature')
                          : t('common.notYetAvailable'),
                        helpText: t('me.help.signatureMode'),
                      },
                      {
                        label: t('me.fields.runtimeNetwork'),
                        value: displayText(btcRuntimeNetwork, t),
                        helpText: t('me.help.runtimeNetwork'),
                      },
                    ]}
                  />
                  {!btcWalletAdapterCapabilities?.canSignMessage ? (
                    <p className="text-sm text-[color:var(--cp-warning)]">
                      {t('me.btc.browserWalletMessageUnavailable')}
                    </p>
                  ) : null}
                  {btcBrowserWalletSignatureError ? (
                    <p className="text-sm text-[color:var(--cp-danger)]">{btcBrowserWalletSignatureError}</p>
                  ) : null}
                  {btcBrowserWalletSignature ? (
                    <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                      <span>{t('me.btc.signatureOutputLabel')}</span>
                      <textarea className="console-textarea" value={btcBrowserWalletSignature} readOnly />
                    </label>
                  ) : null}
                </div>
              </article>

              <article className="console-card">
                <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                  {t('me.btc.browserPsbtTitle')}
                </h3>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('me.btc.browserPsbtBody')}
                </p>
                <div className="mt-4 grid gap-4">
                  <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                    <span>{t('me.btc.psbtInputLabel')}</span>
                    <textarea
                      className="console-textarea"
                      value={btcBrowserWalletPsbt}
                      onChange={(event) => setBtcBrowserWalletPsbt(event.target.value)}
                      placeholder={t('me.btc.psbtPlaceholder')}
                    />
                  </label>
                  <div className="flex flex-wrap items-center gap-3">
                    <button
                      type="button"
                      className="console-action-button"
                      disabled={
                        !btcWalletConnected ||
                        !btcWalletAdapterCapabilities?.canSignPsbt ||
                        btcBrowserWalletPsbtSigning ||
                        btcBrowserWalletPsbt.trim() === ''
                      }
                      onClick={() => void handleSignBrowserWalletPsbt()}
                    >
                      {btcBrowserWalletPsbtSigning ? t('actions.reloading') : t('me.btc.signPsbt')}
                    </button>
                  </div>
                  <FieldValueList
                    items={[
                      {
                        label: t('me.fields.signatureMode'),
                        value: btcWalletAdapterCapabilities?.canSignPsbt
                          ? t('me.values.browserPsbtSignature')
                          : t('common.notYetAvailable'),
                        helpText: t('me.help.signatureMode'),
                      },
                      {
                        label: t('me.fields.psbtInputFormat'),
                        value: displayText(btcBrowserWalletPsbtResult?.inputFormat ?? null, t),
                        helpText: t('me.help.psbtInputFormat'),
                      },
                      {
                        label: t('me.fields.runtimeNetwork'),
                        value: displayText(btcRuntimeNetwork, t),
                        helpText: t('me.help.runtimeNetwork'),
                      },
                    ]}
                  />
                  {!btcWalletAdapterCapabilities?.canSignPsbt ? (
                    <p className="text-sm text-[color:var(--cp-warning)]">
                      {t('me.btc.browserWalletPsbtUnavailable')}
                    </p>
                  ) : null}
                  {btcBrowserWalletPsbtError ? (
                    <p className="text-sm text-[color:var(--cp-danger)]">{btcBrowserWalletPsbtError}</p>
                  ) : null}
                  {btcBrowserWalletPsbtResult ? (
                    <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                      <span>{t('me.btc.psbtOutputLabel')}</span>
                      <textarea className="console-textarea" value={btcBrowserWalletPsbtResult.outputPsbt} readOnly />
                    </label>
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
