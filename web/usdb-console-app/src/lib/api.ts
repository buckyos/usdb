import type {
  AddressBalanceRow,
  BalanceHistorySyncStatus,
  BtcMintExecuteResponse,
  BtcWorldSimDevSignerResponse,
  BtcWorldSimIdentitiesResponse,
  OverviewResponse,
  PassBlockCommitInfo,
  PassEnergyLeaderboardPage,
  PassEnergyRangePage,
  PassEnergySnapshot,
  PassHistoryPage,
  BtcMintPrepareResponse,
  PassSnapshot,
  EthwDevIdentityResponse,
  PassStatsAtHeight,
  RpcActiveBalanceSnapshot,
  UsdbIndexerSyncStatus,
  UsdbRpcInfo,
} from './types'

export async function fetchOverview(): Promise<OverviewResponse> {
  const response = await fetch('/api/system/overview', {
    cache: 'no-store',
  })

  if (!response.ok) {
    throw new Error(`Failed to load overview: HTTP ${response.status}`)
  }

  return response.json() as Promise<OverviewResponse>
}

async function callServiceRpc<T>(
  service: 'balance-history' | 'usdb-indexer',
  method: string,
  params: unknown[],
): Promise<T> {
  const response = await fetch(`/api/services/${service}/rpc`, {
    method: 'POST',
    cache: 'no-store',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      method,
      params,
    }),
  })

  if (!response.ok) {
    const errorPayload = (await response.json().catch(() => null)) as { error?: string } | null
    throw new Error(errorPayload?.error ?? `Failed ${service} RPC call: HTTP ${response.status}`)
  }

  return response.json() as Promise<T>
}

export function callBalanceHistoryRpc<T>(method: string, params: unknown[]): Promise<T> {
  return callServiceRpc<T>('balance-history', method, params)
}

export function callUsdbIndexerRpc<T>(method: string, params: unknown[]): Promise<T> {
  return callServiceRpc<T>('usdb-indexer', method, params)
}

export function fetchBalanceHistorySyncStatus(): Promise<BalanceHistorySyncStatus> {
  return callBalanceHistoryRpc('get_sync_status', [])
}

export function fetchBalanceHistorySingleBalance(
  request: Record<string, unknown>,
): Promise<AddressBalanceRow[]> {
  return callBalanceHistoryRpc('get_address_balance', [request])
}

export function fetchBalanceHistoryBatchBalances(
  request: Record<string, unknown>,
): Promise<AddressBalanceRow[][]> {
  return callBalanceHistoryRpc('get_addresses_balances', [request])
}

export function fetchUsdbRpcInfo(): Promise<UsdbRpcInfo> {
  return callUsdbIndexerRpc('get_rpc_info', [])
}

export function fetchUsdbSyncStatus(): Promise<UsdbIndexerSyncStatus> {
  return callUsdbIndexerRpc('get_sync_status', [])
}

export function fetchUsdbPassStats(atHeight: number | null): Promise<PassStatsAtHeight> {
  return callUsdbIndexerRpc('get_pass_stats_at_height', [{ at_height: atHeight }])
}

export function fetchUsdbLatestActiveBalanceSnapshot(): Promise<RpcActiveBalanceSnapshot | null> {
  return callUsdbIndexerRpc('get_latest_active_balance_snapshot', [])
}

export function fetchUsdbPassSnapshot(
  inscriptionId: string,
  atHeight: number | null,
): Promise<PassSnapshot | null> {
  return callUsdbIndexerRpc('get_pass_snapshot', [
    {
      inscription_id: inscriptionId,
      at_height: atHeight,
    },
  ])
}

export function fetchUsdbOwnerActivePass(
  ownerOrAddress: string,
  atHeight: number | null,
): Promise<PassSnapshot | null> {
  return callUsdbIndexerRpc('get_owner_active_pass_at_height', [
    {
      address: ownerOrAddress,
      at_height: atHeight,
    },
  ])
}

export function fetchUsdbPassBlockCommit(
  blockHeight: number | null,
): Promise<PassBlockCommitInfo | null> {
  return callUsdbIndexerRpc('get_pass_block_commit', [
    {
      block_height: blockHeight,
    },
  ])
}

export function fetchUsdbPassHistory(
  inscriptionId: string,
  fromHeight: number,
  toHeight: number,
  page: number,
  pageSize: number,
  order: 'asc' | 'desc' = 'desc',
): Promise<PassHistoryPage> {
  return callUsdbIndexerRpc('get_pass_history', [
    {
      inscription_id: inscriptionId,
      from_height: fromHeight,
      to_height: toHeight,
      order,
      page,
      page_size: pageSize,
    },
  ])
}

export function fetchUsdbPassEnergy(
  inscriptionId: string,
  blockHeight: number | null,
  mode: 'exact' | 'at_or_before' = 'at_or_before',
): Promise<PassEnergySnapshot> {
  return callUsdbIndexerRpc('get_pass_energy', [
    {
      inscription_id: inscriptionId,
      block_height: blockHeight,
      mode,
    },
  ])
}

export function fetchUsdbPassEnergyRange(
  inscriptionId: string,
  fromHeight: number,
  toHeight: number,
  page: number,
  pageSize: number,
  order: 'asc' | 'desc' = 'desc',
): Promise<PassEnergyRangePage> {
  return callUsdbIndexerRpc('get_pass_energy_range', [
    {
      inscription_id: inscriptionId,
      from_height: fromHeight,
      to_height: toHeight,
      order,
      page,
      page_size: pageSize,
    },
  ])
}

export function fetchUsdbPassEnergyLeaderboard(
  scope: 'active' | 'active_dormant' | 'all',
  page: number,
  pageSize: number,
): Promise<PassEnergyLeaderboardPage> {
  return callUsdbIndexerRpc('get_pass_energy_leaderboard', [
    {
      at_height: null,
      scope,
      page,
      page_size: pageSize,
    },
  ])
}

export async function prepareBtcMintDraft(request: {
  owner_address: string
  eth_main: string
  eth_collab?: string | null
  prev: string[]
}): Promise<BtcMintPrepareResponse> {
  const response = await fetch('/api/btc/mint/prepare', {
    method: 'POST',
    cache: 'no-store',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  })

  if (!response.ok) {
    const errorPayload = (await response.json().catch(() => null)) as { error?: string } | null
    throw new Error(errorPayload?.error ?? `Failed BTC mint prepare: HTTP ${response.status}`)
  }

  return response.json() as Promise<BtcMintPrepareResponse>
}

export async function executeBtcMint(request: {
  wallet_name: string
  owner_address: string
  eth_main: string
  eth_collab?: string | null
  prev: string[]
}): Promise<BtcMintExecuteResponse> {
  const response = await fetch('/api/btc/mint/execute', {
    method: 'POST',
    cache: 'no-store',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  })

  if (!response.ok) {
    const errorPayload = (await response.json().catch(() => null)) as { error?: string } | null
    throw new Error(errorPayload?.error ?? `Failed BTC mint execute: HTTP ${response.status}`)
  }

  return response.json() as Promise<BtcMintExecuteResponse>
}

export async function fetchBtcWorldSimIdentities(): Promise<BtcWorldSimIdentitiesResponse> {
  const response = await fetch('/api/btc/world-sim/identities', {
    cache: 'no-store',
  })

  if (!response.ok) {
    throw new Error(`Failed to load BTC world-sim identities: HTTP ${response.status}`)
  }

  return response.json() as Promise<BtcWorldSimIdentitiesResponse>
}

export async function fetchBtcWorldSimDevSigner(
  walletName: string,
): Promise<BtcWorldSimDevSignerResponse> {
  const search = new URLSearchParams({ wallet_name: walletName })
  const response = await fetch(`/api/btc/world-sim/dev-signer?${search.toString()}`, {
    cache: 'no-store',
  })

  if (!response.ok) {
    throw new Error(`Failed to load BTC world-sim dev signer: HTTP ${response.status}`)
  }

  return response.json() as Promise<BtcWorldSimDevSignerResponse>
}

export async function fetchEthwDevIdentity(): Promise<EthwDevIdentityResponse> {
  const response = await fetch('/api/ethw/dev-sim/identity', {
    cache: 'no-store',
  })

  if (!response.ok) {
    throw new Error(`Failed to load ETHW dev identity: HTTP ${response.status}`)
  }

  return response.json() as Promise<EthwDevIdentityResponse>
}
