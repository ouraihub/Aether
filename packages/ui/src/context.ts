// Host-injected dependencies of kit containers (docs/16 §3, docs/04 §5 "依赖注入走 context").
import { createContext } from 'svelte'
import type { KitI18n } from './i18n'
import type { KitPorts } from './ports'

export type LogFields = Record<string, string | number | boolean>

export interface KitLogger {
  debug(event: string, fields?: LogFields): void
  info(event: string, fields?: LogFields): void
  warn(event: string, fields?: LogFields): void
  error(event: string, fields?: LogFields): void
}

export interface ConfirmOptions {
  title: string
  body: string
  danger?: boolean
}

export interface KitNotify {
  toast(kind: 'info' | 'success' | 'error', text: string): void
  confirm(options: ConfirmOptions): Promise<boolean>
}

export interface KitContext {
  ports: KitPorts
  i18n: KitI18n
  notify: KitNotify
  logger: KitLogger
}

/** The host root component calls `setKitContext` once; kit containers call `getKitContext`. */
export const [getKitContext, setKitContext] = createContext<KitContext>()
