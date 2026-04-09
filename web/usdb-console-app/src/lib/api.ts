import type { OverviewResponse } from './types'

export async function fetchOverview(): Promise<OverviewResponse> {
  const response = await fetch('/api/system/overview', {
    cache: 'no-store',
  })

  if (!response.ok) {
    throw new Error(`Failed to load overview: HTTP ${response.status}`)
  }

  return response.json() as Promise<OverviewResponse>
}

