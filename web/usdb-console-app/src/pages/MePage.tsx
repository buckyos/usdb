import { useEffect, useMemo, useRef, useState } from 'react'
import { Navigate, NavLink, useParams } from 'react-router-dom'
import { FieldValueList } from '../components/FieldValueList'
import {
  executeBtcMint,
  fetchBalanceHistorySingleBalance,
  fetchUsdbChainAddressStatus,
  fetchBtcWorldSimDevSigner,
  fetchBtcWorldSimIdentities,
  fetchUsdbChainDevIdentity,
  prepareBtcMintDraft,
  fetchUsdbOwnerActivePass,
  fetchUsdbPassEnergy,
  fetchUsdbPassSnapshot,
} from '../lib/api'
import {
  displayBalanceDeltaSmart,
  displayBalanceSmart,
  displayDecimalInteger,
  displayNumber,
  displayText,
} from '../lib/format'
import type {
  AddressBalanceRow,
  BtcMintIdentityFields,
  BtcMintExecuteResponse,
  BtcMintPrepareResponse,
  BtcWorldSimIdentitiesResponse,
  UsdbChainAddressStatusResponse,
  UsdbChainDevIdentityResponse,
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
  type BtcWalletMessageSignatureResult,
  type BtcWalletPsbtSignatureResult,
  type BtcWalletSnapshot,
} from '../lib/btcWallet'
import {
  connectEvmWallet,
  detectEvmWalletProvider,
  readEvmWalletSnapshot,
  type EvmWalletSnapshot,
} from '../lib/evmWallet'

interface MePageProps {
  data?: OverviewResponse
  locale: string
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

type IdentityKind = 'usdb' | 'btc'
type UsdbIdentitySource = 'browser_wallet' | 'dev_sim_identity' | 'manual_address'
type BtcIdentitySource = 'browser_wallet' | 'world_sim_agent' | 'manual_address'
type BtcMintFlowStep = 'edit' | 'review' | 'signing' | 'submitting' | 'waiting' | 'success'
type BtcMintPassKind = 'standard' | 'collab'
type BtcMintLeaderBinding = 'pass_id' | 'btc_address'

interface WalletPassRecognition {
  walletInscriptionId: string
  walletInscriptionNumber: number | string | null
  pass: PassSnapshot
}

type BtcRuntimeNetwork = 'mainnet' | 'testnet' | 'testnet4' | 'regtest' | 'signet'

const BTC_ME_SESSION_STORAGE_KEY = 'usdb.console.me.btc.v2'
const USDB_ME_SESSION_STORAGE_KEY = 'usdb.console.me.usdb.v1'

interface PersistedUsdbSessionState {
  identitySource?: UsdbIdentitySource
  manualAddress?: string
}

interface PersistedBtcSessionState {
  walletMode?: BtcWalletMode
  identitySource?: BtcIdentitySource
  manualAddress?: string
  selectedWorldSimWalletName?: string
  selectedWorldSimOwnerAddress?: string
  mintPassKind?: BtcMintPassKind
  mintLeaderBinding?: BtcMintLeaderBinding
  mintUsdbMain?: string
  mintLeaderPassId?: string
  mintLeaderBtcAddr?: string
  mintPrev?: string
  mintStep?: BtcMintFlowStep
  mintPrepareResult?: BtcMintPrepareResponse | null
  mintSigningError?: string | null
  mintSigningResult?: BtcWalletMessageSignatureResult | null
  mintExecutionError?: string | null
  mintExecutionResult?: BtcMintExecuteResponse | null
  mintExecutionPass?: PassSnapshot | null
  mintTechnicalOpen?: boolean
  devToolsOpen?: boolean
}

function readSessionStorageValue(key: string) {
  if (typeof window === 'undefined') return null
  try {
    return window.sessionStorage.getItem(key)
  } catch {
    return null
  }
}

function writeSessionStorageValue(key: string, value: string | null) {
  if (typeof window === 'undefined') return
  try {
    if (value == null) {
      window.sessionStorage.removeItem(key)
      return
    }
    window.sessionStorage.setItem(key, value)
  } catch {
    // Ignore storage write failures in locked-down browser contexts.
  }
}

function loadPersistedBtcSessionState(): PersistedBtcSessionState {
  const raw = readSessionStorageValue(BTC_ME_SESSION_STORAGE_KEY)
  if (!raw) return {}

  try {
    const parsed = JSON.parse(raw) as Partial<PersistedBtcSessionState>
    const walletMode =
      parsed.walletMode === 'browser' || parsed.walletMode === 'dev-regtest'
        ? parsed.walletMode
        : undefined
    const identitySource =
      parsed.identitySource === 'browser_wallet' ||
      parsed.identitySource === 'world_sim_agent' ||
      parsed.identitySource === 'manual_address'
        ? parsed.identitySource
        : undefined
    const mintStep =
      parsed.mintStep === 'edit' ||
      parsed.mintStep === 'review' ||
      parsed.mintStep === 'signing' ||
      parsed.mintStep === 'submitting' ||
      parsed.mintStep === 'waiting' ||
      parsed.mintStep === 'success'
        ? parsed.mintStep
        : undefined
    const mintPassKind =
      parsed.mintPassKind === 'standard' || parsed.mintPassKind === 'collab'
        ? parsed.mintPassKind
        : undefined
    const mintLeaderBinding =
      parsed.mintLeaderBinding === 'pass_id' || parsed.mintLeaderBinding === 'btc_address'
        ? parsed.mintLeaderBinding
        : undefined
    const restored: PersistedBtcSessionState = {
      walletMode,
      identitySource,
      manualAddress: typeof parsed.manualAddress === 'string' ? parsed.manualAddress : undefined,
      selectedWorldSimWalletName:
        typeof parsed.selectedWorldSimWalletName === 'string'
          ? parsed.selectedWorldSimWalletName
          : undefined,
      selectedWorldSimOwnerAddress:
        typeof parsed.selectedWorldSimOwnerAddress === 'string'
          ? parsed.selectedWorldSimOwnerAddress
          : undefined,
      mintPassKind,
      mintLeaderBinding,
      mintUsdbMain: typeof parsed.mintUsdbMain === 'string' ? parsed.mintUsdbMain : undefined,
      mintLeaderPassId:
        typeof parsed.mintLeaderPassId === 'string' ? parsed.mintLeaderPassId : undefined,
      mintLeaderBtcAddr:
        typeof parsed.mintLeaderBtcAddr === 'string' ? parsed.mintLeaderBtcAddr : undefined,
      mintPrev: typeof parsed.mintPrev === 'string' ? parsed.mintPrev : undefined,
      mintStep,
      mintPrepareResult: parsed.mintPrepareResult ?? null,
      mintSigningError:
        typeof parsed.mintSigningError === 'string' ? parsed.mintSigningError : null,
      mintSigningResult: parsed.mintSigningResult ?? null,
      mintExecutionError:
        typeof parsed.mintExecutionError === 'string' ? parsed.mintExecutionError : null,
      mintExecutionResult: parsed.mintExecutionResult ?? null,
      mintExecutionPass: parsed.mintExecutionPass ?? null,
      mintTechnicalOpen: parsed.mintTechnicalOpen === true,
      devToolsOpen: parsed.devToolsOpen === true,
    }
    if (
      (restored.mintStep === 'review' ||
        restored.mintStep === 'signing' ||
        restored.mintStep === 'submitting' ||
        restored.mintStep === 'waiting' ||
        restored.mintStep === 'success') &&
      restored.mintPrepareResult == null
    ) {
      restored.mintStep = 'edit'
    }
    if (
      (restored.mintStep === 'waiting' || restored.mintStep === 'success') &&
      restored.mintExecutionResult == null
    ) {
      restored.mintStep = restored.mintPrepareResult ? 'review' : 'edit'
    }
    return restored
  } catch {
    writeSessionStorageValue(BTC_ME_SESSION_STORAGE_KEY, null)
    return {}
  }
}

function loadPersistedUsdbSessionState(): PersistedUsdbSessionState {
  const raw = readSessionStorageValue(USDB_ME_SESSION_STORAGE_KEY)
  if (!raw) return {}

  try {
    const parsed = JSON.parse(raw) as Partial<PersistedUsdbSessionState>
    const identitySource =
      parsed.identitySource === 'browser_wallet' ||
      parsed.identitySource === 'dev_sim_identity' ||
      parsed.identitySource === 'manual_address'
        ? parsed.identitySource
        : undefined
    return {
      identitySource,
      manualAddress: typeof parsed.manualAddress === 'string' ? parsed.manualAddress : undefined,
    }
  } catch {
    writeSessionStorageValue(USDB_ME_SESSION_STORAGE_KEY, null)
    return {}
  }
}

function normalizeIdentityKind(value?: string): IdentityKind | null {
  if (value === 'usdb' || value === 'btc') return value
  return null
}

function validateEvmAddress(value: string) {
  return /^0x[a-fA-F0-9]{40}$/.test(value.trim())
}

function normalizeEvmChainId(value?: string | null) {
  const normalized = (value ?? '').trim().toLowerCase()
  if (!normalized) return null
  if (/^0x[0-9a-f]+$/.test(normalized)) return normalized
  if (/^[0-9]+$/.test(normalized)) return `0x${BigInt(normalized).toString(16)}`
  return null
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

function usdbIdentitySourceLabel(
  source: UsdbIdentitySource,
  t: MePageProps['t'],
) {
  if (source === 'browser_wallet') return t('me.values.identitySourceUsdbBrowserWallet')
  if (source === 'dev_sim_identity') return t('me.values.identitySourceUsdbDevSim')
  return t('me.values.identitySourceUsdbManualAddress')
}

function btcSignerSourceLabel(
  source: BtcIdentitySource,
  t: MePageProps['t'],
) {
  if (source === 'world_sim_agent') return t('me.values.signerSourceDevSigner')
  if (source === 'manual_address') return t('me.values.signerSourceReadOnly')
  return t('me.values.signerSourceBrowserWallet')
}

function formatUsdbAtoms(value: string | null | undefined, t: MePageProps['t']) {
  if (!value) return t('common.notYetAvailable')
  try {
    const atoms = BigInt(value)
    const base = 10n ** 18n
    const whole = atoms / base
    const fraction = atoms % base
    const fractionText = fraction.toString().padStart(18, '0').slice(0, 6).replace(/0+$/, '')
    return `${whole.toString()}${fractionText ? `.${fractionText}` : ''} USDB`
  } catch {
    return value
  }
}

export function MePage({ data, locale, t }: MePageProps) {
  const { identityKind } = useParams()
  const activeIdentity = normalizeIdentityKind(identityKind)
  const [usdbSessionBoot] = useState<PersistedUsdbSessionState>(() => loadPersistedUsdbSessionState())
  const [btcSessionBoot] = useState<PersistedBtcSessionState>(() => loadPersistedBtcSessionState())
  const [usdbAddress, setUsdbAddress] = useState(usdbSessionBoot.manualAddress ?? '')
  const [usdbIdentitySource, setUsdbIdentitySource] = useState<UsdbIdentitySource>(
    usdbSessionBoot.identitySource ?? 'manual_address',
  )
  const [evmWallet, setEvmWallet] = useState<EvmWalletSnapshot | null>(null)
  const [evmWalletLoading, setEvmWalletLoading] = useState(false)
  const [evmWalletError, setEvmWalletError] = useState<string | null>(null)
  const [usdbDevIdentity, setUsdbDevIdentity] = useState<UsdbChainDevIdentityResponse | null>(null)
  const [usdbDevIdentityLoading, setUsdbDevIdentityLoading] = useState(false)
  const [usdbDevIdentityError, setUsdbDevIdentityError] = useState<string | null>(null)
  const [usdbAddressStatus, setUsdbAddressStatus] = useState<UsdbChainAddressStatusResponse | null>(null)
  const [usdbAddressStatusLoading, setUsdbAddressStatusLoading] = useState(false)
  const [usdbAddressStatusError, setUsdbAddressStatusError] = useState<string | null>(null)
  const [btcAddress, setBtcAddress] = useState(btcSessionBoot.manualAddress ?? '')
  const [btcWallet, setBtcWallet] = useState<BtcWalletSnapshot | null>(null)
  const [btcBrowserWalletSnapshot, setBtcBrowserWalletSnapshot] = useState<BtcWalletSnapshot | null>(null)
  const [btcWalletMode, setBtcWalletMode] = useState<BtcWalletMode>(btcSessionBoot.walletMode ?? 'browser')
  const [btcIdentitySource, setBtcIdentitySource] = useState<BtcIdentitySource>(
    btcSessionBoot.identitySource ?? 'manual_address',
  )
  const [btcWalletLoading, setBtcWalletLoading] = useState(false)
  const [btcWalletError, setBtcWalletError] = useState<string | null>(null)
  const [btcWorldSim, setBtcWorldSim] = useState<BtcWorldSimIdentitiesResponse | null>(null)
  const [btcWorldSimLoading, setBtcWorldSimLoading] = useState(false)
  const [btcWorldSimError, setBtcWorldSimError] = useState<string | null>(null)
  const [btcSelectedWorldSimWalletName, setBtcSelectedWorldSimWalletName] = useState(
    btcSessionBoot.selectedWorldSimWalletName ?? '',
  )
  const [btcSelectedWorldSimOwnerAddressHint, setBtcSelectedWorldSimOwnerAddressHint] = useState(
    btcSessionBoot.selectedWorldSimOwnerAddress ?? '',
  )
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
  const [btcMintPassKind, setBtcMintPassKind] = useState<BtcMintPassKind>(
    btcSessionBoot.mintPassKind ?? 'standard',
  )
  const [btcMintLeaderBinding, setBtcMintLeaderBinding] = useState<BtcMintLeaderBinding>(
    btcSessionBoot.mintLeaderBinding ?? 'pass_id',
  )
  const [btcMintUsdbMain, setBtcMintUsdbMain] = useState(btcSessionBoot.mintUsdbMain ?? '')
  const [btcMintLeaderPassId, setBtcMintLeaderPassId] = useState(
    btcSessionBoot.mintLeaderPassId ?? '',
  )
  const [btcMintLeaderBtcAddr, setBtcMintLeaderBtcAddr] = useState(
    btcSessionBoot.mintLeaderBtcAddr ?? '',
  )
  const [btcMintPrev, setBtcMintPrev] = useState(btcSessionBoot.mintPrev ?? '')
  const [btcMintPrepareLoading, setBtcMintPrepareLoading] = useState(false)
  const [btcMintPrepareError, setBtcMintPrepareError] = useState<string | null>(null)
  const [btcMintPrepareResult, setBtcMintPrepareResult] =
    useState<BtcMintPrepareResponse | null>(btcSessionBoot.mintPrepareResult ?? null)
  const [btcMintStep, setBtcMintStep] = useState<BtcMintFlowStep>(btcSessionBoot.mintStep ?? 'edit')
  const [btcMintSigningLoading, setBtcMintSigningLoading] = useState(false)
  const [btcMintSigningError, setBtcMintSigningError] = useState<string | null>(
    btcSessionBoot.mintSigningError ?? null,
  )
  const [btcMintSigningResult, setBtcMintSigningResult] =
    useState<BtcWalletMessageSignatureResult | null>(btcSessionBoot.mintSigningResult ?? null)
  const [btcMintExecutionLoading, setBtcMintExecutionLoading] = useState(false)
  const [btcMintExecutionError, setBtcMintExecutionError] = useState<string | null>(
    btcSessionBoot.mintExecutionError ?? null,
  )
  const [btcMintExecutionResult, setBtcMintExecutionResult] =
    useState<BtcMintExecuteResponse | null>(btcSessionBoot.mintExecutionResult ?? null)
  const [btcMintExecutionPass, setBtcMintExecutionPass] = useState<PassSnapshot | null>(
    btcSessionBoot.mintExecutionPass ?? null,
  )
  const [btcMintExecutionPolling, setBtcMintExecutionPolling] = useState(false)
  const [btcMintTechnicalOpen, setBtcMintTechnicalOpen] = useState(
    btcSessionBoot.mintTechnicalOpen ?? false,
  )
  const [btcDevToolsOpen, setBtcDevToolsOpen] = useState(btcSessionBoot.devToolsOpen ?? false)
  const btcWalletModeResetReady = useRef(false)
  const btcMintInputResetReady = useRef(false)

  if (!activeIdentity) {
    return <Navigate to="/me/usdb" replace />
  }

  const usdbReachable = Boolean(data?.services.usdb_chain.reachable)
  const usdbConsensusReady = Boolean(data?.services.usdb_chain.data?.consensus_ready)
  const usdbChainRuntimeProfile = data?.capabilities.usdb_chain_runtime_profile ?? 'unknown'
  const usdbRuntimeChainId = normalizeEvmChainId(data?.services.usdb_chain.data?.chain_id)
  const evmWalletChainId = normalizeEvmChainId(evmWallet?.chainId)
  const hasInjectedEvmWallet = detectEvmWalletProvider()
  const evmWalletConnected = Boolean(evmWallet?.address)
  const usdbDevIdentityAvailable = Boolean(usdbDevIdentity?.available && usdbDevIdentity.address)
  const usdbLookupAddress =
    (usdbIdentitySource === 'browser_wallet'
      ? evmWallet?.address
      : usdbIdentitySource === 'dev_sim_identity'
        ? usdbDevIdentity?.address
        : usdbAddress.trim()) || null
  const evmWalletNetworkMismatch =
    usdbIdentitySource === 'browser_wallet' &&
    usdbRuntimeChainId != null &&
    evmWalletChainId != null &&
    usdbRuntimeChainId !== evmWalletChainId
  const evmWalletNetworkMismatchMessage = evmWalletNetworkMismatch
    ? t('me.usdb.networkMismatch', undefined, {
        walletChain: evmWalletChainId ?? t('common.notYetAvailable'),
        runtimeChain: usdbRuntimeChainId ?? t('common.notYetAvailable'),
      })
    : null
  const sourcedaoReady = Boolean(data?.bootstrap.sourcedao_bootstrap_marker.exists)
  const ordAvailable = Boolean(data?.capabilities.ord_available)
  const btcConsoleMode = data?.capabilities.btc_console_mode ?? 'read_only'
  const btcRuntimeProfile = data?.capabilities.btc_runtime_profile ?? 'unknown'
  const balanceHistoryReachable = Boolean(data?.services.balance_history.reachable)
  const balanceHistoryReady = Boolean(data?.services.balance_history.data?.query_ready)
  const usdbIndexerReachable = Boolean(data?.services.usdb_indexer.reachable)
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
        ? btcSelectedWorldSimIdentity?.owner_address ?? btcSelectedWorldSimOwnerAddressHint.trim()
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
  const ethActiveIdentityCanWrite =
    usdbIdentitySource === 'browser_wallet'
      ? evmWalletConnected && !evmWalletNetworkMismatch
      : usdbIdentitySource === 'dev_sim_identity'
        ? usdbChainRuntimeProfile === 'development' && usdbDevIdentityAvailable
        : false
  const btcBrowserSignerReady = btcBrowserWalletConnected && !btcBrowserWalletNetworkMismatch
  const btcWorldSimSignerReady =
    btcRuntimeProfile === 'development' &&
    Boolean(btcSelectedWorldSimIdentity) &&
    btcWalletMode === 'dev-regtest' &&
    Boolean(btcWallet?.address) &&
    !btcAutoDevSignerLoading &&
    !btcAutoDevSignerError &&
    !btcLookupNetworkMismatch
  const btcActiveIdentityCanWrite =
    btcIdentitySource === 'browser_wallet'
      ? btcBrowserSignerReady
      : btcIdentitySource === 'world_sim_agent'
        ? btcWorldSimSignerReady
        : false
  const btcRuntimeAllowsWrite = btcRuntimeProfile !== 'unknown'
  const btcMintIdentityBlocker =
    btcIdentitySource === 'manual_address'
      ? t('me.identity.manualAddressReadOnly')
      : btcRuntimeProfile === 'development' && btcIdentitySource !== 'world_sim_agent'
        ? t('me.btc.mintExecutionRequiresWorldSim')
        : btcRuntimeProfile === 'public' && btcIdentitySource !== 'browser_wallet'
          ? t('me.btc.publicMintBrowserWalletOnly')
          : null
  const btcMintSignerBlocker =
    btcMintIdentityBlocker ??
    (btcIdentitySource === 'browser_wallet' && !btcBrowserSignerReady
      ? t('me.btc.browserIdentityUnavailable')
      : btcIdentitySource === 'world_sim_agent' && !btcWorldSimSignerReady
        ? btcAutoDevSignerError ??
          (btcAutoDevSignerLoading
            ? t('me.btc.devSignerAutoSyncing')
            : t('me.btc.worldSimAgentRequired'))
        : null)
  const btcMintIdentityReady =
    btcMintSignerBlocker == null && btcActiveIdentityCanWrite
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
    activeIdentity === 'usdb'
      ? usdbLookupAddress ?? ''
      : btcLookupAddress ?? ''

  const activeAddressStatus =
    activeIdentity === 'usdb'
      ? activeAddressValue === ''
        ? t('me.identity.notSet')
        : evmWalletNetworkMismatch
          ? t('me.identity.networkMismatch')
          : validateEvmAddress(activeAddressValue)
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
  const btcMintIdentityFields: BtcMintIdentityFields =
    btcMintPassKind === 'standard'
      ? { usdb_main: btcMintUsdbMain.trim() }
      : btcMintLeaderBinding === 'pass_id'
        ? { leader_pass_id: btcMintLeaderPassId.trim() }
        : { leader_btc_addr: btcMintLeaderBtcAddr.trim() }
  const btcMintPassFieldBlocker =
    btcMintPassKind === 'standard'
      ? btcMintUsdbMain.trim() === ''
        ? t('me.btc.mintUsdbMainRequired')
        : null
      : btcMintLeaderBinding === 'pass_id'
        ? btcMintLeaderPassId.trim() === ''
          ? t('me.btc.mintLeaderPassIdRequired')
          : null
        : btcMintLeaderBtcAddr.trim() === ''
          ? t('me.btc.mintLeaderBtcAddrRequired')
          : null
  const btcMintPrepareClientBlockers = [
    !btcLookupAddress ? t('me.btc.mintOwnerRequired') : null,
    btcMintPassFieldBlocker,
    btcLookupNetworkMismatchMessage,
    btcMintSignerBlocker,
  ].filter((item): item is string => Boolean(item))
  const btcMintPrepareEnabled =
    btcMintPrepareClientBlockers.length === 0 && !btcMintPrepareLoading
  const btcMintParsedPrev = useMemo(() => parseMintPrevInput(btcMintPrev), [btcMintPrev])
  const btcMintPreparedPrev = btcMintPrepareResult?.prev ?? btcMintParsedPrev
  const btcMintDraftRequestJson = btcMintPrepareResult
    ? JSON.stringify(btcMintPrepareResult.prepare_request, null, 2)
    : ''
  const btcMintDraftMessage = btcMintPrepareResult?.inscription_payload_json ?? ''
  const btcPreparedActivePass =
    btcMintPrepareResult?.owner_address === btcLookupAddress
      ? btcMintPrepareResult.active_pass ?? null
      : null
  const btcDisplayActivePass = btcActivePass ?? btcPreparedActivePass
  const btcMintSuggestedPrev = btcMintPrepareResult?.suggested_prev ?? []
  const btcMintSuggestedPrevNeedsApply =
    btcMintSuggestedPrev.length > 0 &&
    (btcMintSuggestedPrev.length !== btcMintPreparedPrev.length ||
      btcMintSuggestedPrev.some((item, index) => item !== btcMintPreparedPrev[index]))
  const btcMintSuggestedPrevCanAutoApply =
    btcMintSuggestedPrevNeedsApply && btcMintPreparedPrev.length === 0
  const btcMintIntentLabel =
    btcDisplayActivePass != null || btcPreparedActivePass != null
      ? t('me.values.mintIntentRemint')
      : t('me.values.mintIntentInitial')
  const btcMintNextStepText =
    btcMintPrepareResult == null
      ? t('me.btc.mintReviewPending')
      : btcRuntimeProfile === 'development'
        ? t('me.btc.mintNextStepDevelopment')
        : btcRuntimeProfile === 'public'
          ? t('me.btc.mintNextStepPublic')
          : t('me.btc.mintNextStepUnknown')
  const btcBrowserWalletProtocolItems =
    btcIdentitySource === 'browser_wallet'
      ? [
          {
            label: t('me.fields.walletBalance'),
            value:
              btcBrowserWalletSnapshot?.balance != null
                ? displayBalanceSmart(locale, btcBrowserWalletSnapshot.balance.total, t)
                : t('common.notYetAvailable'),
            helpText: t('me.help.walletBalance'),
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
        ]
      : []
  const btcIdentityStatusItems = [
    ...btcBrowserWalletProtocolItems,
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
        : btcDisplayActivePass?.inscription_id ?? t('me.values.noActivePass'),
      helpText: t('me.help.activeMinerPass'),
    },
    {
      label: t('me.fields.passState'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : btcDisplayActivePass?.state ?? t('common.notYetAvailable'),
      helpText: t('me.help.passState'),
    },
    {
      label: t('me.fields.passOwner'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayText(btcDisplayActivePass?.owner, t),
      helpText: t('me.help.passOwner'),
    },
    {
      label: t('me.fields.passKind'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayText(btcDisplayActivePass?.pass_kind, t),
      helpText: t('me.help.passKind'),
    },
    {
      label: t('me.fields.passUsdbMain'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayText(btcDisplayActivePass?.usdb_main, t),
      helpText: t('me.help.passUsdbMain'),
    },
    {
      label: t('me.fields.leaderPassId'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayText(btcDisplayActivePass?.leader_pass_id, t),
      helpText: t('me.help.leaderPassId'),
    },
    {
      label: t('me.fields.leaderBtcAddress'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayText(btcDisplayActivePass?.leader_btc_addr, t),
      helpText: t('me.help.leaderBtcAddress'),
    },
    {
      label: t('me.fields.rawEnergy'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayDecimalInteger(locale, btcActivePassEnergy?.raw_energy, t),
      helpText: t('me.help.rawEnergy'),
    },
    {
      label: t('me.fields.collabContribution'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayDecimalInteger(locale, btcActivePassEnergy?.collab_contribution, t),
      helpText: t('me.help.collabContribution'),
    },
    {
      label: t('me.fields.effectiveEnergy'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayDecimalInteger(locale, btcActivePassEnergy?.effective_energy, t),
      helpText: t('me.help.effectiveEnergy'),
    },
    {
      label: t('me.fields.passLevel'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayNumber(locale, btcActivePassEnergy?.level, t),
      helpText: t('me.help.passLevel'),
    },
    {
      label: t('me.fields.difficultyFactorBps'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayNumber(locale, btcActivePassEnergy?.difficulty_factor_bps, t),
      helpText: t('me.help.difficultyFactorBps'),
    },
    {
      label: t('me.fields.passEnergyHeight'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayNumber(locale, btcActivePassEnergy?.record_block_height, t),
      helpText: t('me.help.passEnergyHeight'),
    },
    {
      label: t('me.fields.passOwnerBalance'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayBalanceSmart(locale, btcActivePassEnergy?.owner_balance, t),
      helpText: t('me.help.passOwnerBalance'),
    },
    {
      label: t('me.fields.passOwnerDelta'),
      value: btcProtocolLoading
        ? t('actions.reloading')
        : displayBalanceDeltaSmart(locale, btcActivePassEnergy?.owner_delta, t),
      helpText: t('me.help.passOwnerDelta'),
    },
  ]
  const usdbIdentitySourceValue = usdbIdentitySourceLabel(usdbIdentitySource, t)
  const btcSignerSourceValue = btcSignerSourceLabel(btcIdentitySource, t)
  const btcIdentitySourceValue = identitySourceLabel(btcIdentitySource, t)
  const evmWalletSummaryItems = [
    {
      label: t('me.fields.walletProvider'),
      value: displayText(evmWallet?.source, t),
      helpText: t('me.help.usdbWalletProvider'),
    },
    {
      label: t('me.fields.walletAddress'),
      value: displayText(evmWallet?.address, t),
      helpText: t('me.help.usdbWalletAddress'),
    },
    {
      label: t('me.fields.walletNetwork'),
      value: displayText(evmWallet?.chainId, t),
      helpText: t('me.help.usdbWalletNetwork'),
    },
    {
      label: t('me.fields.runtimeChainId'),
      value: displayText(usdbRuntimeChainId, t),
      helpText: t('help.fields.chainId'),
    },
    {
      label: t('me.fields.walletBalance'),
      value: formatUsdbAtoms(evmWallet?.balanceAtomsHex, t),
      helpText: t('me.help.usdbWalletBalance'),
    },
  ]
  const usdbDevIdentityItems = [
    {
      label: t('me.fields.currentAddress'),
      value: displayText(usdbDevIdentity?.address, t),
      helpText: t('me.help.currentUsdbAddress'),
    },
    {
      label: t('me.fields.identityMode'),
      value: displayText(usdbDevIdentity?.identity_mode, t),
      helpText: t('me.help.usdbDevIdentityMode'),
    },
    {
      label: t('me.fields.identityScheme'),
      value: displayText(usdbDevIdentity?.identity_scheme, t),
      helpText: t('me.help.usdbDevIdentityScheme'),
    },
    {
      label: t('me.fields.markerPath'),
      value: displayText(usdbDevIdentity?.marker_path, t),
      helpText: t('me.help.usdbDevIdentityMarker'),
    },
  ]
  const usdbIdentityStatusItems = [
    {
      label: t('me.fields.currentAddress'),
      value: displayText(usdbLookupAddress, t),
      helpText: t('me.help.currentUsdbAddress'),
    },
    {
      label: t('me.fields.addressStatus'),
      value: activeAddressStatus,
      helpText: t('me.help.addressStatus'),
    },
    {
      label: t('me.fields.usdbBalance'),
      value: usdbAddressStatusLoading
        ? t('actions.reloading')
        : formatUsdbAtoms(
            usdbAddressStatus?.balance_atoms_hex ??
              (usdbIdentitySource === 'browser_wallet' ? evmWallet?.balanceAtomsHex : null),
            t,
          ),
      helpText: t('me.help.usdbAddressBalance'),
    },
    {
      label: t('me.fields.runtimeChainId'),
      value: displayText(usdbAddressStatus?.usdb_chain_id ?? usdbRuntimeChainId, t),
      helpText: t('help.fields.chainId'),
    },
    {
      label: t('me.fields.latestBlock'),
      value: displayText(
        usdbAddressStatus?.latest_block_number ?? data?.services.usdb_chain.data?.block_number,
        t,
      ),
      helpText: t('help.fields.blockNumber'),
    },
    {
      label: t('me.fields.linkedMinerPass'),
      value: t('common.notYetAvailable'),
      helpText: t('me.help.usdbLinkedMinerPass'),
    },
    {
      label: t('me.fields.passEnergy'),
      value: t('common.notYetAvailable'),
      helpText: t('me.help.usdbLinkedPassEnergy'),
    },
    {
      label: t('me.fields.miningDifficulty'),
      value: t('common.notYetAvailable'),
      helpText: t('me.help.usdbMiningDifficulty'),
    },
  ]
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
  const btcMintReviewItems = btcMintPrepareResult
    ? [
        {
          label: t('me.fields.currentAddress'),
          value: displayText(btcMintPrepareResult.owner_address, t),
          helpText: t('me.help.currentBtcAddress'),
        },
        {
          label: t('me.fields.mintIntent'),
          value: btcMintIntentLabel,
          helpText: t('me.help.mintIntent'),
        },
        {
          label: t('me.fields.passKind'),
          value: displayText(btcMintPrepareResult.pass_kind, t),
          helpText: t('me.help.passKind'),
        },
        {
          label: t('me.fields.passUsdbMain'),
          value: displayText(btcMintPrepareResult.usdb_main, t),
          helpText: t('me.help.passUsdbMain'),
        },
        {
          label: t('me.fields.leaderPassId'),
          value: displayText(btcMintPrepareResult.leader_pass_id, t),
          helpText: t('me.help.leaderPassId'),
        },
        {
          label: t('me.fields.leaderBtcAddress'),
          value: displayText(btcMintPrepareResult.leader_btc_addr, t),
          helpText: t('me.help.leaderBtcAddress'),
        },
        {
          label: t('me.fields.activeMinerPass'),
          value: displayText(btcMintPrepareResult.active_pass?.inscription_id, t),
          helpText: t('me.help.activeMinerPass'),
        },
        {
          label: t('me.fields.currentPrev'),
          value:
            btcMintPrepareResult.prev.length > 0
              ? btcMintPrepareResult.prev.join(', ')
              : t('me.values.noneApplied'),
          helpText: t('me.help.currentPrev'),
        },
        {
          label: t('me.fields.suggestedPrev'),
          value:
            btcMintPrepareResult.suggested_prev.length > 0
              ? btcMintPrepareResult.suggested_prev.join(', ')
              : t('me.values.noneSuggested'),
          helpText: t('me.help.suggestedPrev'),
        },
        {
          label: t('me.fields.signerSource'),
          value: btcSignerSourceValue,
          helpText: t('me.help.signerSource'),
        },
        {
          label: t('me.fields.prepareMode'),
          value: displayText(btcMintPrepareResult.prepare_mode, t),
          helpText: t('me.help.prepareMode'),
        },
      ]
    : []
  const btcMintExecuteAvailable =
    btcRuntimeProfile === 'development' &&
    btcIdentitySource === 'world_sim_agent' &&
    Boolean(btcSelectedWorldSimIdentity?.wallet_name)
  const btcMintFlowSteps: Array<{ id: BtcMintFlowStep; label: string }> = [
    { id: 'edit', label: t('me.btc.mintStepEdit') },
    { id: 'review', label: t('me.btc.mintStepReview') },
    { id: 'signing', label: t('me.btc.mintStepSigning') },
    { id: 'submitting', label: t('me.btc.mintStepSubmitting') },
    { id: 'waiting', label: t('me.btc.mintStepWaiting') },
    { id: 'success', label: t('me.btc.mintStepSuccess') },
  ]
  const btcMintStepIndex = btcMintFlowSteps.findIndex((step) => step.id === btcMintStep)
  const btcMintSuccessItems = [
    ...btcMintReviewItems,
    {
      label: t('me.fields.signatureMode'),
      value: displayText(btcMintSigningResult?.signatureType, t),
      helpText: t('me.help.signatureMode'),
    },
    {
      label: t('fields.inscriptionId'),
      value: displayText(btcMintExecutionResult?.inscription_id, t),
      helpText: t('me.help.activeMinerPass'),
    },
    {
      label: t('fields.txHash'),
      value: displayText(btcMintExecutionResult?.txid, t),
      helpText: t('help.fields.txHash'),
    },
    {
      label: t('me.fields.passState'),
      value: displayText(btcMintExecutionPass?.state, t),
      helpText: t('me.help.passState'),
    },
  ]
  const btcSignerSummaryItems =
    btcIdentitySource === 'manual_address'
      ? [
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
          {
            label: t('me.fields.addressStatus'),
            value: activeAddressStatus,
            helpText: t('me.help.addressStatus'),
          },
          {
            label: t('me.fields.runtimeNetwork'),
            value: displayText(btcRuntimeNetwork, t),
            helpText: t('me.help.runtimeNetwork'),
          },
        ]
      : [
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
        ]

  const capabilityItems =
    activeIdentity === 'usdb'
      ? [
          {
            label: t('me.fields.usdbRuntime'),
            value: usdbReachable ? t('service.reachable') : t('service.offline'),
            helpText: t('me.help.usdbRuntime'),
          },
          {
            label: t('me.fields.usdbConsensus'),
            value: usdbConsensusReady ? t('service.consensusReady') : t('common.notYetAvailable'),
            helpText: t('me.help.usdbConsensus'),
          },
          {
            label: t('me.fields.sourcedaoBootstrap'),
            value: sourcedaoReady ? t('states.completed') : t('states.pending'),
            helpText: t('me.help.sourcedaoBootstrap'),
          },
          {
            label: t('me.fields.identitySource'),
            value: usdbIdentitySourceValue,
            helpText: t('me.help.usdbIdentitySource'),
          },
          {
            label: t('me.fields.runtimeProfile'),
            value: runtimeProfileLabel(usdbChainRuntimeProfile, t),
            helpText: t('me.help.usdbChainRuntimeProfile'),
          },
          {
            label: t('me.fields.connectCapability'),
            value: hasInjectedEvmWallet ? t('states.completed') : t('common.notYetAvailable'),
            helpText: t('me.help.usdbConnectCapability'),
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
    activeIdentity === 'usdb'
      ? [
          {
            label: t('me.fields.identitySource'),
            value: usdbIdentitySourceValue,
            helpText: t('me.help.usdbIdentitySource'),
          },
          {
            label: t('me.fields.writeCapability'),
            value: ethActiveIdentityCanWrite ? t('me.values.writeEnabled') : t('me.values.readOnly'),
            helpText: t('me.help.writeCapability'),
          },
          {
            label: t('me.fields.currentAddress'),
            value: displayText(activeAddressValue || null, t),
            helpText: t('me.help.currentUsdbAddress'),
          },
          {
            label: t('me.fields.addressStatus'),
            value: activeAddressStatus,
            helpText: t('me.help.addressStatus'),
          },
          {
            label: t('me.fields.chainId'),
            value: displayText(usdbRuntimeChainId, t),
            helpText: t('help.fields.chainId'),
          },
          {
            label: t('me.fields.detectedWalletNetwork'),
            value: displayText(evmWallet?.chainId, t),
            helpText: t('me.help.usdbWalletNetwork'),
          },
          {
            label: t('me.fields.latestBlock'),
            value: displayText(data?.services.usdb_chain.data?.block_number, t),
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
            label: t('me.fields.writeCapability'),
            value: btcActiveIdentityCanWrite ? t('me.values.writeEnabled') : t('me.values.readOnly'),
            helpText: t('me.help.writeCapability'),
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
    activeIdentity === 'usdb'
      ? [
          {
            label: t('me.fields.primaryAction'),
            value: t('me.values.usdbContractActions'),
            helpText: t('me.help.primaryAction'),
          },
          {
            label: t('me.fields.runtimeGate'),
            value: sourcedaoReady ? t('states.completed') : t('states.pending'),
            helpText: t('me.help.runtimeGate'),
          },
          {
            label: t('me.fields.identityMode'),
            value: usdbIdentitySourceValue,
            helpText: t('me.help.identityMode'),
          },
          {
            label: t('me.fields.currentAddress'),
            value: displayText(activeAddressValue || null, t),
            helpText: t('me.help.currentUsdbAddress'),
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
    writeSessionStorageValue(
      USDB_ME_SESSION_STORAGE_KEY,
      JSON.stringify({
        identitySource: usdbIdentitySource,
        manualAddress: usdbAddress,
      } satisfies PersistedUsdbSessionState),
    )
  }, [usdbAddress, usdbIdentitySource])

  useEffect(() => {
    if (activeIdentity !== 'usdb') return
    if (usdbChainRuntimeProfile === 'public' && usdbIdentitySource === 'dev_sim_identity') {
      setUsdbIdentitySource(evmWalletConnected ? 'browser_wallet' : 'manual_address')
    }
  }, [activeIdentity, usdbIdentitySource, evmWalletConnected, usdbChainRuntimeProfile])

  useEffect(() => {
    if (activeIdentity !== 'usdb') return
    let cancelled = false

    void readEvmWalletSnapshot()
      .then((snapshot) => {
        if (cancelled) return
        setEvmWallet(snapshot)
        setEvmWalletError(null)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setEvmWalletError(error.message)
      })

    return () => {
      cancelled = true
    }
  }, [activeIdentity])

  useEffect(() => {
    if (activeIdentity !== 'usdb') return
    if (usdbChainRuntimeProfile !== 'development') {
      setUsdbDevIdentity(null)
      setUsdbDevIdentityError(null)
      return
    }

    let cancelled = false
    setUsdbDevIdentityLoading(true)
    setUsdbDevIdentityError(null)

    void fetchUsdbChainDevIdentity()
      .then((response) => {
        if (cancelled) return
        setUsdbDevIdentity(response)
        setUsdbDevIdentityError(response.error ?? null)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setUsdbDevIdentity(null)
        setUsdbDevIdentityError(error.message)
      })
      .finally(() => {
        if (cancelled) return
        setUsdbDevIdentityLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [activeIdentity, usdbChainRuntimeProfile])

  useEffect(() => {
    if (activeIdentity !== 'usdb' || usdbChainRuntimeProfile !== 'development') return
    if (!usdbDevIdentityAvailable || usdbAddress.trim()) return
    setUsdbIdentitySource((current) => (current === 'manual_address' ? 'dev_sim_identity' : current))
  }, [activeIdentity, usdbAddress, usdbDevIdentityAvailable, usdbChainRuntimeProfile])

  useEffect(() => {
    if (activeIdentity !== 'usdb') return
    if (!usdbLookupAddress || !validateEvmAddress(usdbLookupAddress) || !usdbReachable) {
      setUsdbAddressStatus(null)
      setUsdbAddressStatusError(null)
      setUsdbAddressStatusLoading(false)
      return
    }

    let cancelled = false
    setUsdbAddressStatusLoading(true)
    setUsdbAddressStatusError(null)

    void fetchUsdbChainAddressStatus(usdbLookupAddress)
      .then((response) => {
        if (cancelled) return
        setUsdbAddressStatus(response)
        setUsdbAddressStatusError(response.error ?? null)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setUsdbAddressStatus(null)
        setUsdbAddressStatusError(error.message)
      })
      .finally(() => {
        if (cancelled) return
        setUsdbAddressStatusLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [activeIdentity, usdbLookupAddress, usdbReachable])

  useEffect(() => {
    if (!btcSelectedWorldSimIdentity?.owner_address) return
    setBtcSelectedWorldSimOwnerAddressHint((current) =>
      current === btcSelectedWorldSimIdentity.owner_address
        ? current
        : btcSelectedWorldSimIdentity.owner_address,
    )
  }, [btcSelectedWorldSimIdentity])

  useEffect(() => {
    writeSessionStorageValue(
      BTC_ME_SESSION_STORAGE_KEY,
      JSON.stringify({
        walletMode: btcWalletMode,
        identitySource: btcIdentitySource,
        manualAddress: btcAddress,
        selectedWorldSimWalletName: btcSelectedWorldSimWalletName,
        selectedWorldSimOwnerAddress:
          btcSelectedWorldSimIdentity?.owner_address ?? btcSelectedWorldSimOwnerAddressHint,
        mintPassKind: btcMintPassKind,
        mintLeaderBinding: btcMintLeaderBinding,
        mintUsdbMain: btcMintUsdbMain,
        mintLeaderPassId: btcMintLeaderPassId,
        mintLeaderBtcAddr: btcMintLeaderBtcAddr,
        mintPrev: btcMintPrev,
        mintStep: btcMintStep,
        mintPrepareResult: btcMintPrepareResult,
        mintSigningError: btcMintSigningError,
        mintSigningResult: btcMintSigningResult,
        mintExecutionError: btcMintExecutionError,
        mintExecutionResult: btcMintExecutionResult,
        mintExecutionPass: btcMintExecutionPass,
        mintTechnicalOpen: btcMintTechnicalOpen,
        devToolsOpen: btcDevToolsOpen,
      } satisfies PersistedBtcSessionState),
    )
  }, [
    btcAddress,
    btcDevToolsOpen,
    btcIdentitySource,
    btcMintLeaderBinding,
    btcMintLeaderBtcAddr,
    btcMintLeaderPassId,
    btcMintPassKind,
    btcMintUsdbMain,
    btcMintExecutionError,
    btcMintExecutionPass,
    btcMintExecutionResult,
    btcMintPrepareResult,
    btcMintPrev,
    btcMintSigningError,
    btcMintSigningResult,
    btcMintStep,
    btcMintTechnicalOpen,
    btcSelectedWorldSimIdentity,
    btcSelectedWorldSimOwnerAddressHint,
    btcSelectedWorldSimWalletName,
    btcWalletMode,
  ])

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    if (btcRuntimeProfile === 'public') {
      setBtcIdentitySource((current) =>
        current === 'world_sim_agent'
          ? btcBrowserWalletConnected
            ? 'browser_wallet'
            : 'manual_address'
          : current,
      )
    }
  }, [activeIdentity, btcBrowserWalletConnected, btcRuntimeProfile])

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    const desiredMode = btcIdentitySource === 'world_sim_agent' ? 'dev-regtest' : 'browser'
    if (btcWalletMode !== desiredMode) {
      setBtcWalletMode(desiredMode)
    }
  }, [activeIdentity, btcIdentitySource, btcWalletMode])

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
    if (!btcWalletModeResetReady.current) {
      btcWalletModeResetReady.current = true
      return
    }
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
    setBtcMintSigningError(null)
    setBtcMintSigningResult(null)
    setBtcMintExecutionError(null)
    setBtcMintExecutionResult(null)
    setBtcMintExecutionPass(null)
  }, [btcWalletMode])

  useEffect(() => {
    if (!btcMintInputResetReady.current) {
      btcMintInputResetReady.current = true
      return
    }
    setBtcMintPrepareError(null)
    setBtcMintPrepareResult(null)
    setBtcMintStep('edit')
    setBtcMintSigningError(null)
    setBtcMintSigningResult(null)
    setBtcMintExecutionError(null)
    setBtcMintExecutionResult(null)
    setBtcMintExecutionPass(null)
    setBtcMintTechnicalOpen(false)
  }, [
    btcLookupAddress,
    btcLookupNetworkMismatchMessage,
    btcMintLeaderBinding,
    btcMintLeaderBtcAddr,
    btcMintLeaderPassId,
    btcMintPassKind,
    btcMintPrev,
    btcMintUsdbMain,
  ])

  useEffect(() => {
    if (btcRuntimeProfile !== 'development') return
    if (btcMintStep !== 'signing') return
    if (!btcMintDraftMessage.trim()) return
    setBtcDevWalletMessage((current) => (current.trim() === btcMintDraftMessage.trim() ? current : btcMintDraftMessage))
  }, [btcMintDraftMessage, btcMintStep, btcRuntimeProfile])

  useEffect(() => {
    if (btcMintStep !== 'waiting') return
    if (!btcMintExecutionResult?.inscription_id) return

    let cancelled = false
    let timer: number | null = null

    const poll = async () => {
      setBtcMintExecutionPolling(true)
      try {
        const [passSnapshot, activePass] = await Promise.all([
          fetchUsdbPassSnapshot(btcMintExecutionResult.inscription_id, null),
          fetchUsdbOwnerActivePass(btcMintExecutionResult.owner_address, null),
        ])
        if (cancelled) return
        const resolvedPass =
          passSnapshot ??
          (activePass?.inscription_id === btcMintExecutionResult.inscription_id ? activePass : null) ??
          (btcMintExecutionResult.txid != null && activePass?.mint_txid === btcMintExecutionResult.txid
            ? activePass
            : null)
        if (resolvedPass) {
          setBtcMintExecutionPass(resolvedPass)
          setBtcMintExecutionError(null)
          setBtcMintStep('success')
          setBtcMintExecutionPolling(false)
          return
        }
        timer = window.setTimeout(() => {
          void poll()
        }, 3000)
      } catch (error) {
        if (cancelled) return
        setBtcMintExecutionError(error instanceof Error ? error.message : String(error))
        timer = window.setTimeout(() => {
          void poll()
        }, 3000)
      } finally {
        if (!cancelled) {
          setBtcMintExecutionPolling(false)
        }
      }
    }

    void poll()

    return () => {
      cancelled = true
      if (timer != null) {
        window.clearTimeout(timer)
      }
    }
  }, [btcMintExecutionResult, btcMintStep])

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
    if (!btcLookupAddress) {
      setBtcAddressBalanceRows([])
      setBtcAddressBalanceError(null)
      setBtcActivePass(null)
      setBtcActivePassEnergy(null)
      setBtcProtocolError(null)
      return
    }

    let cancelled = false

    if (balanceHistoryReachable) {
      setBtcAddressBalanceLoading(true)
      setBtcAddressBalanceError(null)
      void fetchBalanceHistorySingleBalance({
        address: btcLookupAddress,
        block_height: null,
        block_range: null,
      })
        .then((rows) => {
          if (cancelled) return
          setBtcAddressBalanceRows(rows)
        })
        .catch((error) => {
          if (cancelled) return
          setBtcAddressBalanceRows([])
          setBtcAddressBalanceError(error instanceof Error ? error.message : String(error))
        })
        .finally(() => {
          if (cancelled) return
          setBtcAddressBalanceLoading(false)
        })
    } else {
      setBtcAddressBalanceRows([])
      setBtcAddressBalanceError(null)
      setBtcAddressBalanceLoading(false)
    }

    if (usdbIndexerReachable) {
      setBtcProtocolLoading(true)
      setBtcProtocolError(null)
      void fetchUsdbOwnerActivePass(btcLookupAddress, null)
        .then(async (activePass) => {
          if (cancelled) return
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
          setBtcActivePass(null)
          setBtcActivePassEnergy(null)
          setBtcProtocolError(error instanceof Error ? error.message : String(error))
        })
        .finally(() => {
          if (cancelled) return
          setBtcProtocolLoading(false)
        })
    } else {
      setBtcActivePass(null)
      setBtcActivePassEnergy(null)
      setBtcProtocolError(null)
      setBtcProtocolLoading(false)
    }

    return () => {
      cancelled = true
    }
  }, [
    activeIdentity,
    balanceHistoryReachable,
    btcLookupAddress,
    btcLookupNetworkMismatchMessage,
    usdbIndexerReachable,
  ])

  useEffect(() => {
    if (activeIdentity !== 'btc') return
    if (btcIdentitySource !== 'browser_wallet') {
      setBtcRecognizedPasses([])
      setBtcRecognizedPassesError(null)
      return
    }
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
    btcIdentitySource,
    btcBrowserWalletNetworkMismatchMessage,
    btcBrowserWalletSnapshot?.inscriptions,
    usdbIndexerReady,
  ])

  async function handleConnectEvmWallet() {
    setEvmWalletLoading(true)
    setEvmWalletError(null)

    try {
      const snapshot = await connectEvmWallet()
      setEvmWallet(snapshot)
      if (snapshot.address) {
        setUsdbIdentitySource('browser_wallet')
      }
    } catch (error) {
      setEvmWalletError(error instanceof Error ? error.message : String(error))
    } finally {
      setEvmWalletLoading(false)
    }
  }

  async function handleConnectBtcWallet() {
    setBtcWalletLoading(true)
    setBtcWalletError(null)

    try {
      const snapshot = await connectBtcWalletByMode('browser')
      setBtcWallet(snapshot)
      setBtcBrowserWalletSnapshot(snapshot)
      if (snapshot.address) {
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

    setBtcMintPrepareError(null)
    setBtcMintPrepareResult(null)
    setBtcMintSigningError(null)
    setBtcMintSigningResult(null)
    setBtcMintExecutionError(null)
    setBtcMintExecutionResult(null)
    setBtcMintExecutionPass(null)

    try {
      await refreshBtcMintDraft(parseMintPrevInput(btcMintPrev))
      setBtcMintStep('review')
    } catch (error) {
      setBtcMintPrepareError(error instanceof Error ? error.message : String(error))
      setBtcMintStep('edit')
    }
  }

  async function refreshBtcMintDraft(prev: string[]) {
    if (!btcLookupAddress) {
      const message = t('me.btc.mintOwnerRequired')
      setBtcMintPrepareError(message)
      throw new Error(message)
    }

    setBtcMintPrepareLoading(true)
    setBtcMintPrepareError(null)
    setBtcMintSigningError(null)
    setBtcMintSigningResult(null)
    setBtcMintExecutionError(null)
    setBtcMintExecutionResult(null)
    setBtcMintExecutionPass(null)

    try {
      const result = await prepareBtcMintDraft({
        owner_address: btcLookupAddress,
        prev,
        ...btcMintIdentityFields,
      })
      setBtcMintPrepareResult(result)
      setBtcMintTechnicalOpen(false)
      return result
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setBtcMintPrepareError(message)
      throw error
    } finally {
      setBtcMintPrepareLoading(false)
    }
  }

  async function handleApplySuggestedPrev() {
    if (btcMintSuggestedPrev.length === 0) return
    const nextPrevText = btcMintSuggestedPrev.join('\n')
    setBtcMintPrev(nextPrevText)
    try {
      await refreshBtcMintDraft(btcMintSuggestedPrev)
      setBtcMintStep('review')
    } catch {
      setBtcMintStep('review')
    }
  }

  function handleResetBtcMintFlow(clearInputs: boolean) {
    setBtcMintPrepareError(null)
    setBtcMintPrepareResult(null)
    setBtcMintStep('edit')
    setBtcMintSigningError(null)
    setBtcMintSigningResult(null)
    setBtcMintExecutionError(null)
    setBtcMintExecutionResult(null)
    setBtcMintExecutionPass(null)
    setBtcMintTechnicalOpen(false)
    if (!clearInputs) return
    setBtcMintPassKind('standard')
    setBtcMintLeaderBinding('pass_id')
    setBtcMintUsdbMain('')
    setBtcMintLeaderPassId('')
    setBtcMintLeaderBtcAddr('')
    setBtcMintPrev('')
  }

  async function handleAdvanceBtcMintSigning() {
    if (!btcMintPrepareResult?.eligible) return
    if (btcMintPrepareLoading) return

    if (btcMintSuggestedPrevCanAutoApply) {
      const nextPrevText = btcMintSuggestedPrev.join('\n')
      setBtcMintPrev(nextPrevText)
      try {
        const refreshed = await refreshBtcMintDraft(btcMintSuggestedPrev)
        if (!refreshed.eligible) {
          setBtcMintStep('review')
          return
        }
      } catch {
        setBtcMintStep('review')
        return
      }
    }

    setBtcMintStep('signing')
    setBtcMintSigningError(null)
    setBtcMintSigningResult(null)
    setBtcMintExecutionError(null)
    setBtcMintExecutionResult(null)
    setBtcMintExecutionPass(null)
    setBtcMintTechnicalOpen(false)
  }

  function handleOpenBtcDevTools() {
    setBtcDevToolsOpen(true)
    if (typeof document !== 'undefined') {
      window.setTimeout(() => {
        document.getElementById('btc-dev-tools')?.scrollIntoView({ behavior: 'smooth', block: 'start' })
      }, 0)
    }
  }

  async function handleSignMintDraftWithDevSigner() {
    if (!btcMintDraftMessage.trim()) {
      setBtcMintSigningError(t('me.btc.mintSigningMissingDraft'))
      return
    }
    if (!btcSelectedWorldSimIdentity?.wallet_name) {
      setBtcMintSigningError(t('me.btc.mintExecutionRequiresWorldSim'))
      return
    }

    setBtcMintSigningLoading(true)
    setBtcMintSigningError(null)
    setBtcMintSigningResult(null)
    setBtcMintExecutionError(null)
    setBtcMintExecutionResult(null)
    setBtcMintExecutionPass(null)
    setBtcDevWalletMessage(btcMintDraftMessage)
    setBtcDevWalletSignature(null)
    setBtcDevWalletSignatureError(null)

    try {
      const result = await signBtcWalletMessage('dev-regtest', btcMintDraftMessage)
      setBtcMintSigningResult(result)
      setBtcDevWalletSignature(result.signature)
      setBtcMintStep('submitting')
      const preparedIdentityFields: BtcMintIdentityFields = btcMintPrepareResult?.usdb_main
        ? { usdb_main: btcMintPrepareResult.usdb_main }
        : btcMintPrepareResult?.leader_pass_id
          ? { leader_pass_id: btcMintPrepareResult.leader_pass_id }
          : btcMintPrepareResult?.leader_btc_addr
            ? { leader_btc_addr: btcMintPrepareResult.leader_btc_addr }
            : btcMintIdentityFields
      const executionResult = await executeBtcMint({
        wallet_name: btcSelectedWorldSimIdentity.wallet_name,
        owner_address: btcMintPrepareResult?.owner_address ?? btcLookupAddress ?? '',
        prev: btcMintPrepareResult?.prev ?? btcMintParsedPrev,
        ...preparedIdentityFields,
      })
      setBtcMintExecutionResult(executionResult)
      setBtcMintStep('waiting')
      setBtcDevToolsOpen(true)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setBtcMintSigningError(message)
      setBtcMintExecutionError(message)
      setBtcDevWalletSignatureError(message)
      setBtcMintStep('signing')
    } finally {
      setBtcMintSigningLoading(false)
    }
  }

  const currentTitle =
    activeIdentity === 'usdb' ? t('me.usdb.title') : t('me.btc.title')
  const currentBody =
    activeIdentity === 'usdb' ? t('me.usdb.body') : t('me.btc.body')
  const btcWalletCardTitle =
    btcIdentitySource === 'manual_address'
      ? t('me.btc.readOnlyIdentityTitle')
      : btcIdentitySource === 'world_sim_agent'
        ? t('me.btc.devWalletTitle')
        : t('me.btc.walletTitle')
  const btcWalletCardBody =
    btcIdentitySource === 'manual_address'
      ? t('me.btc.readOnlyIdentityBody')
      : btcIdentitySource === 'world_sim_agent'
        ? t('me.btc.devWalletBody')
        : t('me.btc.walletBody')
  const runtimeOverviewItems = [
    {
      label: t('me.fields.btcRuntimeProfile'),
      value: runtimeProfileLabel(btcRuntimeProfile, t),
      helpText: t('me.help.runtimeProfile'),
    },
    {
      label: t('me.fields.btcConsoleMode'),
      value:
        btcConsoleMode === 'inscription_enabled'
          ? t('capabilities.consoleMode.inscriptionEnabled')
          : t('capabilities.consoleMode.readOnly'),
      helpText: t('me.help.btcConsoleMode'),
    },
    {
      label: t('me.fields.runtimeNetwork'),
      value: displayText(btcRuntimeNetwork, t),
      helpText: t('me.help.runtimeNetwork'),
    },
    {
      label: t('me.fields.usdbChainRuntimeProfile'),
      value: runtimeProfileLabel(usdbChainRuntimeProfile, t),
      helpText: t('me.help.usdbChainRuntimeProfile'),
    },
    {
      label: t('me.fields.runtimeChainId'),
      value: displayText(usdbRuntimeChainId, t),
      helpText: t('help.fields.chainId'),
    },
    {
      label: t('me.fields.sourcedaoBootstrap'),
      value: sourcedaoReady ? t('states.completed') : t('states.pending'),
      helpText: t('me.help.sourcedaoBootstrap'),
    },
  ]

  return (
    <div className="grid gap-5">
      <section className="console-page-intro">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h2 className="text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
              {t('pages.me.title')}
            </h2>
            <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
              {t('pages.me.subtitle')}
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <span className="status-pill" data-tone={btcRuntimeProfile === 'development' ? 'warning' : 'neutral'}>
              {`${t('me.fields.btcRuntimeProfile')}: ${runtimeProfileLabel(btcRuntimeProfile, t)}`}
            </span>
            <span className="status-pill" data-tone={usdbChainRuntimeProfile === 'development' ? 'warning' : 'neutral'}>
              {`${t('me.fields.usdbChainRuntimeProfile')}: ${runtimeProfileLabel(usdbChainRuntimeProfile, t)}`}
            </span>
          </div>
        </div>
        <div className="mt-5 rounded-3xl border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)]/70 p-4">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('me.runtimeOverview.title')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {t('me.runtimeOverview.body')}
          </p>
          <div className="mt-4">
            <FieldValueList items={runtimeOverviewItems} />
          </div>
        </div>
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
              to="/me/usdb"
              className={({ isActive }) =>
                isActive ? 'console-service-selector active' : 'console-service-selector'
              }
            >
              <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                {t('me.selector.usdbTitle')}
              </h4>
              <p className="mt-1 text-sm text-[color:var(--cp-muted)]">
                {t('me.selector.usdbBody')}
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
            {activeIdentity === 'usdb' ? (
              <div className="flex flex-wrap items-center gap-3">
                {usdbIdentitySource === 'browser_wallet' ? (
                  <button
                    type="button"
                    className={evmWalletConnected ? 'console-secondary-button' : 'console-action-button'}
                    disabled={!hasInjectedEvmWallet || evmWalletLoading}
                    onClick={() => void handleConnectEvmWallet()}
                  >
                    {evmWalletLoading
                      ? t('actions.reloading')
                      : evmWalletConnected
                        ? t('me.usdb.refreshWallet')
                        : t('me.usdb.connectWallet')}
                  </button>
                ) : null}
                {evmWalletConnected && usdbIdentitySource === 'browser_wallet' ? (
                  <span className="status-pill" data-tone="success">
                    {t('me.values.walletConnected')}
                  </span>
                ) : null}
              </div>
            ) : activeIdentity === 'btc' ? (
              <div className="flex flex-wrap items-center gap-3">
                {btcIdentitySource === 'browser_wallet' ? (
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
                {btcWalletConnected && btcIdentitySource !== 'manual_address' ? (
                  <span className="status-pill" data-tone={btcWalletMode === 'dev-regtest' ? 'warning' : 'success'}>
                    {btcWalletMode === 'dev-regtest'
                      ? t('me.values.devWalletLoaded')
                      : t('me.values.walletConnected')}
                  </span>
                ) : null}
              </div>
            ) : null}
          </div>
          <div className="mt-5 rounded-3xl border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)]/70 p-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <h4 className="text-base font-semibold text-[color:var(--cp-text)]">
                  {t('me.identity.selectorTitle')}
                </h4>
                <p className="mt-1 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {activeIdentity === 'usdb'
                    ? t('me.identity.usdbSelectorBody')
                    : t('me.identity.btcSelectorBody')}
                </p>
              </div>
              <span
                className="status-pill"
                data-tone={
                  activeIdentity === 'usdb'
                    ? ethActiveIdentityCanWrite
                      ? 'success'
                      : 'warning'
                    : btcActiveIdentityCanWrite
                      ? 'success'
                      : 'warning'
                }
              >
                {activeIdentity === 'usdb'
                  ? ethActiveIdentityCanWrite
                    ? t('me.values.writeEnabled')
                    : t('me.values.readOnly')
                  : btcActiveIdentityCanWrite
                    ? t('me.values.writeEnabled')
                    : t('me.values.readOnly')}
              </span>
            </div>
            <div className="mt-4 grid gap-4">
              {activeIdentity === 'usdb' ? (
                <>
                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      type="button"
                      className={
                        usdbIdentitySource === 'browser_wallet'
                          ? 'console-action-button'
                          : 'console-secondary-button'
                      }
                      onClick={() => setUsdbIdentitySource('browser_wallet')}
                    >
                      {t('me.values.identitySourceUsdbBrowserWallet')}
                    </button>
                    {usdbChainRuntimeProfile === 'development' ? (
                      <button
                        type="button"
                        className={
                          usdbIdentitySource === 'dev_sim_identity'
                            ? 'console-action-button'
                            : 'console-secondary-button'
                        }
                        onClick={() => setUsdbIdentitySource('dev_sim_identity')}
                      >
                        {t('me.values.identitySourceUsdbDevSim')}
                      </button>
                    ) : null}
                    <button
                      type="button"
                      className={
                        usdbIdentitySource === 'manual_address'
                          ? 'console-action-button'
                          : 'console-secondary-button'
                      }
                      onClick={() => setUsdbIdentitySource('manual_address')}
                    >
                      {t('me.values.identitySourceUsdbManualAddress')}
                    </button>
                  </div>

                  {usdbIdentitySource === 'manual_address' ? (
                    <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                      <span>{t('me.identity.usdbInputLabel')}</span>
                      <input
                        className="console-input"
                        value={usdbAddress}
                        onChange={(event) => setUsdbAddress(event.target.value)}
                        placeholder={t('me.identity.usdbPlaceholder')}
                      />
                    </label>
                  ) : null}

                  {usdbIdentitySource === 'browser_wallet' && !evmWalletConnected ? (
                    <p className="text-sm text-[color:var(--cp-muted)]">
                      {t('me.usdb.browserIdentityUnavailable')}
                    </p>
                  ) : null}

                  {usdbIdentitySource === 'dev_sim_identity' ? (
                    <div className="grid gap-3">
                      <FieldValueList items={usdbDevIdentityItems} />
                      {usdbDevIdentityLoading ? (
                        <p className="text-sm text-[color:var(--cp-muted)]">
                          {t('actions.reloading')}
                        </p>
                      ) : null}
                      {usdbDevIdentityError ? (
                        <p className="text-sm text-[color:var(--cp-danger)]">
                          {usdbDevIdentityError}
                        </p>
                      ) : null}
                      {!usdbDevIdentityLoading && !usdbDevIdentityError && !usdbDevIdentityAvailable ? (
                        <p className="text-sm text-[color:var(--cp-muted)]">
                          {t('me.usdb.devIdentityUnavailable')}
                        </p>
                      ) : null}
                    </div>
                  ) : null}
                </>
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
            </div>
          </div>
          <div className="mt-4">
            <FieldValueList items={workspaceSummaryItems} />
          </div>
          {activeIdentity === 'btc' && btcIdentitySource === 'browser_wallet' && !hasInjectedBtcWallet ? (
            <p className="mt-4 text-sm text-[color:var(--cp-warning)]">
              {t('me.btc.walletUnavailable')}
            </p>
          ) : null}
          {activeIdentity === 'usdb' && usdbIdentitySource === 'browser_wallet' && !hasInjectedEvmWallet ? (
            <p className="mt-4 text-sm text-[color:var(--cp-warning)]">
              {t('me.usdb.walletUnavailable')}
            </p>
          ) : null}
          {activeIdentity === 'usdb' && evmWalletError ? (
            <p className="mt-2 text-sm text-[color:var(--cp-danger)]">{evmWalletError}</p>
          ) : null}
          {activeIdentity === 'usdb' && evmWalletNetworkMismatchMessage ? (
            <p className="mt-2 text-sm text-[color:var(--cp-danger)]">
              {evmWalletNetworkMismatchMessage}
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
            {activeIdentity === 'usdb' ? t('me.capability.usdbBody') : t('me.capability.btcBody')}
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
            {activeIdentity === 'usdb' ? t('me.identity.usdbBody') : t('me.identity.btcBody')}
          </p>
          <div className="mt-4">
            <FieldValueList items={identityItems} />
          </div>
        </article>
      </section>

      {activeIdentity === 'usdb' ? (
        <section className="grid gap-4 xl:grid-cols-2">
          <article className="console-card">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('me.identityStatus.title')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('me.identityStatus.usdbBody')}
            </p>
            <div className="mt-4">
              <FieldValueList items={usdbIdentityStatusItems} />
            </div>
            {!usdbLookupAddress ? (
              <p className="mt-4 text-sm text-[color:var(--cp-muted)]">
                {t('me.identityStatus.usdbUnavailable')}
              </p>
            ) : null}
            {usdbAddressStatusError ? (
              <p className="mt-4 text-sm text-[color:var(--cp-danger)]">{usdbAddressStatusError}</p>
            ) : null}
          </article>

          <article className="console-card">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('me.usdb.walletTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('me.usdb.walletBody')}
            </p>
            <div className="mt-4 grid gap-4">
              <FieldValueList items={evmWalletSummaryItems} />
              {!hasInjectedEvmWallet ? (
                <p className="text-sm text-[color:var(--cp-warning)]">
                  {t('me.usdb.walletUnavailable')}
                </p>
              ) : null}
              {evmWalletNetworkMismatchMessage ? (
                <p className="text-sm text-[color:var(--cp-danger)]">
                  {evmWalletNetworkMismatchMessage}
                </p>
              ) : null}
              {evmWalletError ? (
                <p className="text-sm text-[color:var(--cp-danger)]">{evmWalletError}</p>
              ) : null}
            </div>
          </article>

          {usdbChainRuntimeProfile === 'development' ? (
            <article className="console-card">
              <div className="flex flex-wrap items-start justify-between gap-4">
                <div>
                  <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                    {t('me.usdb.devIdentityTitle')}
                  </h3>
                  <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                    {t('me.usdb.devIdentityBody')}
                  </p>
                </div>
                <span className="status-pill" data-tone={usdbDevIdentityAvailable ? 'success' : 'warning'}>
                  {usdbDevIdentityAvailable ? t('states.completed') : t('states.pending')}
                </span>
              </div>
              <div className="mt-4 grid gap-4">
                <FieldValueList items={usdbDevIdentityItems} />
                {usdbDevIdentityLoading ? (
                  <p className="text-sm text-[color:var(--cp-muted)]">{t('actions.reloading')}</p>
                ) : null}
                {usdbDevIdentityError ? (
                  <p className="text-sm text-[color:var(--cp-danger)]">{usdbDevIdentityError}</p>
                ) : null}
              </div>
            </article>
          ) : null}
        </section>
      ) : null}

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
              <div className="mt-4 grid gap-4">
                <FieldValueList items={btcSignerSummaryItems} />
                {btcWalletMode === 'dev-regtest' ? (
                  <p className="rounded-2xl border border-[color:var(--cp-warning)]/25 bg-[color:var(--cp-warning)]/8 px-4 py-3 text-sm leading-6 text-[color:var(--cp-warning)]">
                    {t('me.btc.devWalletWarning')}
                  </p>
                ) : null}
                {btcDevSignerAutoManaged && !btcAutoDevSignerError ? (
                  <p className="rounded-2xl border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-4 py-3 text-sm leading-6 text-[color:var(--cp-muted)]">
                    {t('me.btc.devWalletWorldSimManaged')}
                  </p>
                ) : null}
                {btcRuntimeProfile === 'development' && btcWalletMode === 'dev-regtest' && !btcDevSignerAutoManaged ? (
                  <p className="rounded-2xl border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-4 py-3 text-sm leading-6 text-[color:var(--cp-muted)]">
                    {t('me.btc.devToolsManualSignerHint')}
                  </p>
                ) : null}
                {btcAutoDevSignerLoading ? (
                  <p className="text-sm text-[color:var(--cp-muted)]">
                    {t('me.btc.devSignerAutoSyncing')}
                  </p>
                ) : null}
                {btcAutoDevSignerError ? (
                  <p className="text-sm text-[color:var(--cp-danger)]">{btcAutoDevSignerError}</p>
                ) : null}
              </div>
            </article>

            <article className="console-card">
              <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                {t('me.identityStatus.title')}
              </h3>
              <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                {t('me.identityStatus.btcBody')}
              </p>
              <div className="mt-4">
                <FieldValueList items={btcIdentityStatusItems} />
              </div>
              {!btcLookupAddress ? (
                <p className="mt-4 text-sm text-[color:var(--cp-muted)]">
                  {t('me.identityStatus.btcUnavailable')}
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
              ) : (
                <span className="status-pill" data-tone="warning">
                  {t('states.pending')}
                </span>
              )}
            </div>
            <div className="mt-4 grid gap-4">
              <div className="grid gap-3 lg:grid-cols-6">
                {btcMintFlowSteps.map((step, index) => {
                  const completed = index < btcMintStepIndex
                  const active = index === btcMintStepIndex
                  return (
                    <div
                      key={step.id}
                      className={`rounded-[20px] border px-4 py-3 transition ${
                        active
                          ? 'border-[color:var(--cp-accent)] bg-[color:var(--cp-accent)]/8'
                          : completed
                            ? 'border-[color:var(--cp-success)]/35 bg-[color:var(--cp-success)]/8'
                            : 'border-[color:var(--cp-border)] bg-[color:var(--cp-surface)]'
                      }`}
                    >
                      <p className="text-[11px] font-semibold uppercase tracking-[0.16em] text-[color:var(--cp-muted)]">
                        {String(index + 1).padStart(2, '0')}
                      </p>
                      <p className="mt-2 text-sm font-semibold text-[color:var(--cp-text)]">
                        {step.label}
                      </p>
                    </div>
                  )
                })}
              </div>

              <div className="rounded-[24px] border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-5 py-5">
                {btcMintStep === 'edit' ? (
                  <div className="grid gap-4">
                    <div>
                      <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintEditTitle')}
                      </h4>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {t('me.btc.mintEditBody')}
                      </p>
                    </div>
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
                          label: t('me.fields.signerSource'),
                          value: btcSignerSourceValue,
                          helpText: t('me.help.signerSource'),
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
                    <div className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                      <span>{t('me.btc.mintPassKindLabel')}</span>
                      <div
                        className="inline-flex w-fit rounded-md border border-[color:var(--cp-border)] bg-[color:var(--cp-panel)] p-1"
                        role="group"
                        aria-label={t('me.btc.mintPassKindLabel')}
                      >
                        {(['standard', 'collab'] as const).map((kind) => (
                          <button
                            key={kind}
                            type="button"
                            className={`rounded px-3 py-2 text-sm font-semibold transition ${
                              btcMintPassKind === kind
                                ? 'bg-[color:var(--cp-accent)] text-white'
                                : 'text-[color:var(--cp-muted)] hover:text-[color:var(--cp-text)]'
                            }`}
                            aria-pressed={btcMintPassKind === kind}
                            onClick={() => setBtcMintPassKind(kind)}
                          >
                            {t(`me.values.passKind.${kind}`)}
                          </button>
                        ))}
                      </div>
                    </div>
                    {btcMintPassKind === 'standard' ? (
                      <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                        <span>{t('me.btc.mintUsdbMainLabel')}</span>
                        <input
                          className="console-input"
                          value={btcMintUsdbMain}
                          onChange={(event) => setBtcMintUsdbMain(event.target.value)}
                          placeholder={t('me.btc.mintUsdbMainPlaceholder')}
                        />
                      </label>
                    ) : (
                      <div className="grid gap-4">
                        <div className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                          <span>{t('me.btc.mintLeaderBindingLabel')}</span>
                          <div
                            className="inline-flex w-fit rounded-md border border-[color:var(--cp-border)] bg-[color:var(--cp-panel)] p-1"
                            role="group"
                            aria-label={t('me.btc.mintLeaderBindingLabel')}
                          >
                            {(['pass_id', 'btc_address'] as const).map((binding) => (
                              <button
                                key={binding}
                                type="button"
                                className={`rounded px-3 py-2 text-sm font-semibold transition ${
                                  btcMintLeaderBinding === binding
                                    ? 'bg-[color:var(--cp-accent)] text-white'
                                    : 'text-[color:var(--cp-muted)] hover:text-[color:var(--cp-text)]'
                                }`}
                                aria-pressed={btcMintLeaderBinding === binding}
                                onClick={() => setBtcMintLeaderBinding(binding)}
                              >
                                {t(`me.values.leaderBinding.${binding}`)}
                              </button>
                            ))}
                          </div>
                        </div>
                        {btcMintLeaderBinding === 'pass_id' ? (
                          <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                            <span>{t('me.btc.mintLeaderPassIdLabel')}</span>
                            <input
                              className="console-input"
                              value={btcMintLeaderPassId}
                              onChange={(event) => setBtcMintLeaderPassId(event.target.value)}
                              placeholder={t('me.btc.mintLeaderPassIdPlaceholder')}
                            />
                          </label>
                        ) : (
                          <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                            <span>{t('me.btc.mintLeaderBtcAddrLabel')}</span>
                            <input
                              className="console-input"
                              value={btcMintLeaderBtcAddr}
                              onChange={(event) => setBtcMintLeaderBtcAddr(event.target.value)}
                              placeholder={t('me.btc.mintLeaderBtcAddrPlaceholder')}
                            />
                          </label>
                        )}
                      </div>
                    )}
                    <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                      <span>{t('me.btc.mintPrevLabel')}</span>
                      <textarea
                        className="console-textarea"
                        value={btcMintPrev}
                        onChange={(event) => setBtcMintPrev(event.target.value)}
                        placeholder={t('me.btc.mintPrevPlaceholder')}
                      />
                    </label>
                    {btcMintCapabilityReady ? (
                      <p className="text-sm text-[color:var(--cp-muted)]">
                        {t('me.btc.mintPrepareReadinessHint')}
                      </p>
                    ) : (
                      <p className="text-sm text-[color:var(--cp-muted)]">
                        {t('me.btc.mintPrepareRuntimeGateHint')}
                      </p>
                    )}
                    {btcMintPrepareClientBlockers.length > 0 ? (
                      <div className="rounded-2xl border border-[color:var(--cp-warning)]/25 bg-[color:var(--cp-warning)]/8 px-4 py-3">
                        <ul className="grid gap-2 text-sm leading-6 text-[color:var(--cp-warning)]">
                          {btcMintPrepareClientBlockers.map((item) => (
                            <li key={item}>{item}</li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                    {btcMintPrepareError ? (
                      <p className="text-sm text-[color:var(--cp-danger)]">{btcMintPrepareError}</p>
                    ) : null}
                    <div className="flex flex-wrap items-center gap-3">
                      <button
                        type="button"
                        className="console-action-button"
                        disabled={!btcMintPrepareEnabled}
                        onClick={() => void handlePrepareBtcMintDraft()}
                      >
                        {btcMintPrepareLoading ? t('actions.reloading') : t('me.btc.prepareMintDraft')}
                      </button>
                      {(btcMintUsdbMain ||
                        btcMintLeaderPassId ||
                        btcMintLeaderBtcAddr ||
                        btcMintPrev) &&
                      !btcMintPrepareLoading ? (
                        <button
                          type="button"
                          className="console-secondary-button"
                          onClick={() => handleResetBtcMintFlow(true)}
                        >
                          {t('me.btc.mintResetFlow')}
                        </button>
                      ) : null}
                    </div>
                  </div>
                ) : null}

                {btcMintStep === 'review' ? (
                  <div className="grid gap-4">
                    <div>
                      <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintReviewTitle')}
                      </h4>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {t('me.btc.mintReviewBody')}
                      </p>
                    </div>
                    <FieldValueList items={btcMintReviewItems} />
                    <div className="rounded-2xl border border-[color:var(--cp-border)] bg-[color:var(--cp-panel)] px-4 py-4">
                      <h5 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintNextStepTitle')}
                      </h5>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {btcMintNextStepText}
                      </p>
                    </div>
                    {btcMintSuggestedPrevNeedsApply ? (
                      <div className="rounded-2xl border border-[color:var(--cp-warning)]/25 bg-[color:var(--cp-warning)]/8 px-4 py-4">
                        <p className="text-sm leading-6 text-[color:var(--cp-text)]">
                          {btcMintSuggestedPrevCanAutoApply
                            ? t('me.btc.mintSuggestedPrevAutoBody')
                            : t('me.btc.mintSuggestedPrevMismatchBody')}
                        </p>
                        <div className="mt-3 flex flex-wrap items-center gap-3">
                          <button
                            type="button"
                            className="console-secondary-button"
                            disabled={btcMintPrepareLoading}
                            onClick={() => void handleApplySuggestedPrev()}
                          >
                            {btcMintPrepareLoading
                              ? t('actions.reloading')
                              : t('me.btc.mintUseSuggestedPrev')}
                          </button>
                        </div>
                      </div>
                    ) : null}
                    {btcMintPrepareError ? (
                      <p className="text-sm text-[color:var(--cp-danger)]">{btcMintPrepareError}</p>
                    ) : null}
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
                    <div className="flex flex-wrap items-center gap-3">
                      <button
                        type="button"
                        className="console-secondary-button"
                        onClick={() => setBtcMintStep('edit')}
                      >
                        {t('me.btc.mintBackToEdit')}
                      </button>
                      <button
                        type="button"
                        className="console-secondary-button"
                        onClick={() => handleResetBtcMintFlow(true)}
                      >
                        {t('me.btc.mintResetFlow')}
                      </button>
                      <button
                        type="button"
                        className="console-action-button"
                        disabled={!btcMintPrepareResult?.eligible || btcMintPrepareLoading}
                        onClick={() => void handleAdvanceBtcMintSigning()}
                      >
                        {btcMintPrepareLoading ? t('actions.reloading') : t('me.btc.mintConfirmAndSign')}
                      </button>
                    </div>
                  </div>
                ) : null}

                {btcMintStep === 'signing' ? (
                  <div className="grid gap-4">
                    <div>
                      <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintSigningTitle')}
                      </h4>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {btcRuntimeProfile === 'development'
                          ? t('me.btc.mintSigningDevelopmentBody')
                          : btcRuntimeProfile === 'public'
                            ? t('me.btc.mintSigningPublicBody')
                            : t('me.btc.mintSigningUnknownBody')}
                      </p>
                    </div>
                    <FieldValueList items={btcMintReviewItems} />
                    <div className="rounded-2xl border border-[color:var(--cp-border)] bg-[color:var(--cp-panel)] px-4 py-4">
                      <h5 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintSigningActionTitle')}
                      </h5>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {btcRuntimeProfile === 'development'
                          ? t('me.btc.mintSigningDevelopmentActionBody')
                          : t('me.btc.mintSigningPublicActionBody')}
                      </p>
                    </div>
                    <div className="rounded-2xl border border-[color:var(--cp-warning)]/25 bg-[color:var(--cp-warning)]/8 px-4 py-4">
                      <h5 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintExecutionBoundaryTitle')}
                      </h5>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {btcMintNextStepText}
                      </p>
                    </div>
                    {btcMintSigningError ? (
                      <p className="text-sm text-[color:var(--cp-danger)]">{btcMintSigningError}</p>
                    ) : null}
                    {!btcMintExecuteAvailable && btcRuntimeProfile === 'development' ? (
                      <p className="text-sm text-[color:var(--cp-warning)]">
                        {t('me.btc.mintExecutionRequiresWorldSim')}
                      </p>
                    ) : null}
                    <div className="flex flex-wrap items-center gap-3">
                      {btcRuntimeProfile === 'development' ? (
                        <button
                          type="button"
                          className="console-action-button"
                          disabled={
                            !btcWalletConnected ||
                            btcMintSigningLoading ||
                            btcMintDraftMessage.trim() === '' ||
                            !btcMintExecuteAvailable
                          }
                          onClick={() => void handleSignMintDraftWithDevSigner()}
                        >
                          {btcMintSigningLoading
                            ? t('actions.reloading')
                            : t('me.btc.mintConfirmWithDevSigner')}
                        </button>
                      ) : null}
                      {btcRuntimeProfile === 'development' ? (
                        <button
                          type="button"
                          className="console-secondary-button"
                          onClick={handleOpenBtcDevTools}
                        >
                          {t('me.btc.openDevTools')}
                        </button>
                      ) : null}
                      <button
                        type="button"
                        className="console-secondary-button"
                        onClick={() => setBtcMintStep('review')}
                      >
                        {t('me.btc.mintBackToReview')}
                      </button>
                      <button
                        type="button"
                        className="console-secondary-button"
                        onClick={() => handleResetBtcMintFlow(true)}
                      >
                        {t('me.btc.mintResetFlow')}
                      </button>
                    </div>
                  </div>
                ) : null}

                {btcMintStep === 'submitting' ? (
                  <div className="grid gap-4">
                    <div>
                      <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintSubmittingTitle')}
                      </h4>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {t('me.btc.mintSubmittingBody')}
                      </p>
                    </div>
                    <FieldValueList items={btcMintReviewItems} />
                    {btcMintSigningResult?.signature ? (
                      <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                        <span>{t('me.btc.signatureOutputLabel')}</span>
                        <textarea className="console-textarea" value={btcMintSigningResult.signature} readOnly />
                      </label>
                    ) : null}
                    <p className="text-sm text-[color:var(--cp-muted)]">{t('actions.reloading')}</p>
                  </div>
                ) : null}

                {btcMintStep === 'waiting' ? (
                  <div className="grid gap-4">
                    <div>
                      <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintWaitingTitle')}
                      </h4>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {t('me.btc.mintWaitingBody')}
                      </p>
                    </div>
                    <FieldValueList
                      items={[
                        ...btcMintReviewItems,
                        {
                          label: t('fields.inscriptionId'),
                          value: displayText(btcMintExecutionResult?.inscription_id, t),
                          helpText: t('me.help.activeMinerPass'),
                        },
                        {
                          label: t('fields.txHash'),
                          value: displayText(btcMintExecutionResult?.txid, t),
                          helpText: t('help.fields.txHash'),
                        },
                      ]}
                    />
                    {btcMintExecutionError ? (
                      <p className="text-sm text-[color:var(--cp-danger)]">{btcMintExecutionError}</p>
                    ) : null}
                    <p className="text-sm text-[color:var(--cp-muted)]">
                      {btcMintExecutionPolling ? t('me.btc.mintWaitingPolling') : t('me.btc.mintWaitingRetry')}
                    </p>
                    <div className="flex flex-wrap items-center gap-3">
                      {btcRuntimeProfile === 'development' ? (
                        <button
                          type="button"
                          className="console-secondary-button"
                          onClick={handleOpenBtcDevTools}
                        >
                          {t('me.btc.openDevTools')}
                        </button>
                      ) : null}
                      <button
                        type="button"
                        className="console-secondary-button"
                        onClick={() => handleResetBtcMintFlow(true)}
                      >
                        {t('me.btc.mintResetFlow')}
                      </button>
                    </div>
                  </div>
                ) : null}

                {btcMintStep === 'success' ? (
                  <div className="grid gap-4">
                    <div>
                      <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintSuccessTitle')}
                      </h4>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {btcRuntimeProfile === 'development'
                          ? t('me.btc.mintSuccessDevelopmentBody')
                          : t('me.btc.mintSuccessPublicBody')}
                      </p>
                    </div>
                    <FieldValueList items={btcMintSuccessItems} />
                    {btcMintSigningResult?.signature ? (
                      <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                        <span>{t('me.btc.signatureOutputLabel')}</span>
                        <textarea className="console-textarea" value={btcMintSigningResult.signature} readOnly />
                      </label>
                    ) : null}
                    {btcMintExecutionResult?.ord_output ? (
                      <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                        <span>{t('me.btc.mintOrdOutputLabel')}</span>
                        <textarea className="console-textarea" value={btcMintExecutionResult.ord_output} readOnly />
                      </label>
                    ) : null}
                    <div className="rounded-2xl border border-[color:var(--cp-warning)]/25 bg-[color:var(--cp-warning)]/8 px-4 py-4">
                      <h5 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.mintExecutionBoundaryTitle')}
                      </h5>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {t('me.btc.mintSuccessBoundaryBody')}
                      </p>
                    </div>
                    <div className="flex flex-wrap items-center gap-3">
                      <button
                        type="button"
                        className="console-secondary-button"
                        onClick={() => setBtcMintStep('review')}
                      >
                        {t('me.btc.mintBackToReview')}
                      </button>
                      {btcRuntimeProfile === 'development' ? (
                        <button
                          type="button"
                          className="console-secondary-button"
                          onClick={handleOpenBtcDevTools}
                        >
                          {t('me.btc.openDevTools')}
                        </button>
                      ) : null}
                      <button
                        type="button"
                        className="console-action-button"
                        onClick={() => handleResetBtcMintFlow(true)}
                      >
                        {t('me.btc.mintStartAnother')}
                      </button>
                    </div>
                  </div>
                ) : null}

                {btcMintPrepareResult && btcMintStep !== 'edit' ? (
                  <div className="mt-4 rounded-[24px] border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)]">
                    <button
                      type="button"
                      className="flex w-full items-center justify-between gap-3 px-5 py-4 text-left text-sm font-semibold text-[color:var(--cp-text)]"
                      onClick={() => setBtcMintTechnicalOpen((current) => !current)}
                    >
                      <span>{t('me.btc.mintTechnicalSummary')}</span>
                      <span className="text-xs text-[color:var(--cp-muted)]">
                        {btcMintTechnicalOpen ? t('actions.hide') : t('actions.show')}
                      </span>
                    </button>
                    {btcMintTechnicalOpen ? (
                      <div className="border-t border-[color:var(--cp-border)] px-5 py-5">
                        <p className="text-sm leading-6 text-[color:var(--cp-muted)]">
                          {t('me.btc.mintTechnicalBody')}
                        </p>
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
                      </div>
                    ) : null}
                  </div>
                ) : null}
              </div>
            </div>
          </section>

          {btcIdentitySource === 'browser_wallet' ? (
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
                        <th>{t('me.fields.passUsdbMain')}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {btcRecognizedPasses.map((item) => (
                        <tr key={item.walletInscriptionId}>
                          <td>{item.pass.inscription_id}</td>
                          <td>{displayText(item.walletInscriptionNumber, t)}</td>
                          <td>{item.pass.state}</td>
                          <td>{item.pass.owner}</td>
                          <td>{item.pass.usdb_main}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </section>
          ) : null}

          {btcRuntimeProfile === 'development' ? (
            <section id="btc-dev-tools" className="console-card">
              <div className="flex flex-wrap items-start justify-between gap-4">
                <div>
                  <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                    {t('me.btc.devToolsTitle')}
                  </h3>
                  <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                    {t('me.btc.devToolsBody')}
                  </p>
                </div>
                <div className="flex flex-wrap items-center gap-3">
                  <span className="status-pill" data-tone="warning">
                    {t('me.values.runtimeDevelopment')}
                  </span>
                  <button
                    type="button"
                    className="console-secondary-button"
                    onClick={() => setBtcDevToolsOpen((current) => !current)}
                  >
                    {btcDevToolsOpen ? t('actions.hide') : t('actions.show')}
                  </button>
                </div>
              </div>
              {btcDevToolsOpen ? (
              <div className="mt-4 grid gap-4">
                {btcWalletMode === 'dev-regtest' ? (
                  <>
                    <article className="rounded-[24px] border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-5 py-5">
                      <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {t('me.btc.devSignerManagementTitle')}
                      </h4>
                      <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                        {t('me.btc.devSignerManagementBody')}
                      </p>
                      <div className="mt-4 grid gap-4">
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
                        {btcAutoDevSignerError ? (
                          <p className="text-sm text-[color:var(--cp-danger)]">{btcAutoDevSignerError}</p>
                        ) : null}
                        <FieldValueList items={btcSignerSummaryItems} />
                      </div>
                    </article>

                    <section className="grid gap-4 xl:grid-cols-2">
                      <article className="rounded-[24px] border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-5 py-5">
                        <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
                          {t('me.btc.signatureTitle')}
                        </h3>
                        <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                          {t('me.btc.signatureBody')}
                        </p>
                        <div className="mt-4 grid gap-4">
                          {btcMintPrepareResult ? (
                            <div className="rounded-2xl border border-[color:var(--cp-border)] bg-[color:var(--cp-panel)] px-4 py-4">
                              <p className="text-sm leading-6 text-[color:var(--cp-muted)]">
                                {t('me.btc.devToolsMintPayloadHint')}
                              </p>
                              <div className="mt-3 flex flex-wrap items-center gap-3">
                                <button
                                  type="button"
                                  className="console-secondary-button"
                                  onClick={() => setBtcDevWalletMessage(btcMintDraftMessage)}
                                >
                                  {t('me.btc.useCurrentMintPayload')}
                                </button>
                              </div>
                            </div>
                          ) : null}
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

                      <article className="rounded-[24px] border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-5 py-5">
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
                  </>
                ) : (
                  <section className="grid gap-4 xl:grid-cols-2">
                    <article className="rounded-[24px] border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-5 py-5">
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

                    <article className="rounded-[24px] border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-5 py-5">
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
                )}
              </div>
              ) : null}
            </section>
          ) : null}
        </>
      ) : null}

      <section className="console-card">
        <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
          {t('me.nextActions.title')}
        </h3>
        <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
          {activeIdentity === 'usdb' ? t('me.nextActions.usdbBody') : t('me.nextActions.btcBody')}
        </p>
        <ul className="mt-4 grid gap-2 text-sm leading-6 text-[color:var(--cp-text)]">
          {(activeIdentity === 'usdb'
            ? ['me.nextActions.usdbItem1', 'me.nextActions.usdbItem2', 'me.nextActions.usdbItem3']
            : ['me.nextActions.btcItem1', 'me.nextActions.btcItem2', 'me.nextActions.btcItem3']
          ).map((key) => (
            <li key={key}>{t(key)}</li>
          ))}
        </ul>
      </section>
    </div>
  )
}
