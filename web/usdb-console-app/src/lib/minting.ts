import { useEffect, useState } from 'react'
import type { MonitorSnapshot } from './types'
import type { Tone } from './console'

const states: Record<string, [string, string]> = {
  DISABLED: ['未启用', 'Disabled'], WAITING_CORE: ['等待 Bitcoin 最新区块同步', 'Waiting for latest Bitcoin blocks'],
  WAITING_HISTORY: ['等待 Bitcoin 历史区块验证', 'Waiting for historical validation'],
  WAITING_TXINDEX: ['等待交易索引追平', 'Waiting for txindex'], BLOCKED_DISK: ['磁盘余量不足，Ord 已暂停', 'Ord paused: low disk space'],
  BLOCKED_CONFIG: ['Bitcoin 配置不兼容', 'Incompatible Bitcoin configuration'],
  STARTING: ['Ord 启动中', 'Ord starting'], INDEXING: ['Ord 索引及主链一致性校验中', 'Ord indexing / checking canonical chain'],
  READY: ['索引后端已就绪', 'Index backend ready'], UNAVAILABLE: ['当前状态未知', 'Current state unknown'],
  FAILED: ['Ord 运行失败', 'Ord failed'], STOPPED: ['Ord 已停止', 'Ord stopped'],
}

/** Keep all optional-Ord views on the same independently expiring observation. */
export function useMintingObservation(snapshot?: MonitorSnapshot) {
  const [elapsed, setElapsed] = useState(0)
  useEffect(() => {
    const received = performance.now()
    setElapsed(0)
    const timer = window.setInterval(() => setElapsed(performance.now() - received), 1000)
    return () => window.clearInterval(timer)
  }, [snapshot])
  const value = snapshot?.report?.minting
  const hostAge = (snapshot?.age_ms ?? Infinity) + elapsed
  const age = hostAge + Math.max(0, (snapshot?.report?.observed_at_ms ?? 0) - (value?.observed_at_ms ?? 0))
  const monitorStatus = snapshot?.status === 'available' && hostAge > 120000 ? 'stale' : snapshot?.status ?? 'loading'
  const fresh = monitorStatus === 'available' && (value?.enabled === false || age <= 60000)
  const state = fresh && value?.state && value.state in states ? value.state : 'UNAVAILABLE'
  return { value, fresh, state, monitorStatus }
}

export function mintingLabel(state: string, locale: string) {
  return states[state]?.[locale === 'zh-CN' ? 0 : 1] ?? states.UNAVAILABLE[locale === 'zh-CN' ? 0 : 1]
}

export function mintingTone(state: string): Tone {
  if (state === 'READY') return 'success'
  if (['FAILED', 'BLOCKED_CONFIG', 'BLOCKED_DISK'].includes(state)) return 'danger'
  if (state.startsWith('WAITING_') || ['STARTING', 'INDEXING', 'UNAVAILABLE'].includes(state)) return 'warning'
  return 'neutral'
}
