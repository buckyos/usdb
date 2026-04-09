import type { SupportedLocale } from './i18n-provider'

type Dictionary = Record<string, string>

const en: Dictionary = {
  'console.title': 'USDB Control Console',
  'console.subtitle': 'Unified system overview for BTC, USDB, and ETHW services.',
  'common.refresh': 'Refresh',
  'common.language': 'Language',
  'sections.services': 'Service Status',
  'sections.bootstrap': 'Bootstrap',
  'states.pending': 'Pending',
  'states.inProgress': 'In Progress',
  'states.completed': 'Completed',
  'states.error': 'Error',
}

const zhCN: Dictionary = {
  'console.title': 'USDB 控制台',
  'console.subtitle': '统一查看 BTC、USDB 与 ETHW 服务状态。',
  'common.refresh': '刷新',
  'common.language': '语言',
  'sections.services': '服务状态',
  'sections.bootstrap': '冷启动',
  'states.pending': '待处理',
  'states.inProgress': '进行中',
  'states.completed': '已完成',
  'states.error': '错误',
}

export const dictionaries: Record<SupportedLocale, Dictionary> = {
  en,
  'zh-CN': zhCN,
}

