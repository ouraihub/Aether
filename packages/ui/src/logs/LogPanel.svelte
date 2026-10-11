<script lang="ts">
  // Log viewer (docs/16 §4.5): tail(300) every 2 s while the page is visible; problems-only filter; open folder.
  import { Alert, AlertDescription } from '#lib/components/ui/alert/index.js'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import { getKitContext } from '../context'
  import type { LogEntry } from '../ports'
  import { cn } from '../utils'

  const POLL_MS = 2000
  const LIMIT = 300

  const { ports, i18n, notify } = getKitContext()
  const t = i18n.t

  let entries = $state<LogEntry[]>([])
  let failed = $state(false)
  let problemsOnly = $state(false)
  const shown = $derived(problemsOnly ? entries.filter((e) => e.level === 'ERROR' || e.level === 'WARN') : entries)

  async function refresh() {
    try {
      entries = await ports.logs.tail(LIMIT)
      failed = false
    } catch {
      failed = true
    }
  }

  $effect(() => {
    let timer: number | undefined
    const sync = () => {
      window.clearInterval(timer)
      timer = undefined
      if (document.visibilityState !== 'visible') return
      void refresh()
      timer = window.setInterval(() => void refresh(), POLL_MS)
    }
    sync()
    document.addEventListener('visibilitychange', sync)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', sync)
    }
  })

  async function openDir() {
    try {
      await ports.logs.openDir()
    } catch {
      notify.toast('error', t('errors.internal'))
    }
  }
</script>

<section class="flex min-h-0 flex-1 flex-col gap-3" aria-label={t('logs.title')}>
  <header class="flex items-center gap-3">
    <h2 class="text-base font-medium">{t('logs.title')}</h2>
    <label class="ml-auto flex items-center gap-2 text-sm">
      <Switch bind:checked={problemsOnly} />
      {t('logs.problemsOnly')}
    </label>
    <Button variant="outline" size="sm" onclick={() => void openDir()}>{t('logs.openDir')}</Button>
  </header>
  {#if failed}
    <Alert variant="destructive"><AlertDescription>{t('logs.loadFailed')}</AlertDescription></Alert>
  {/if}
  {#if shown.length === 0}
    <Empty.Root>
      <Empty.Header><Empty.Title>{t('logs.empty')}</Empty.Title></Empty.Header>
    </Empty.Root>
  {:else}
    <ol class="bg-muted min-h-0 flex-1 overflow-auto rounded-md p-2 font-mono text-xs">
      {#each shown as entry (entry.seq)}
        <li
          class={cn(
            'break-all whitespace-pre-wrap',
            entry.level === 'ERROR' && 'text-destructive',
            (entry.level === 'DEBUG' || entry.level === 'TRACE') && 'text-muted-foreground',
          )}
        >
          {entry.time}
          {entry.level}
          {entry.target}: {entry.message}
        </li>
      {/each}
    </ol>
  {/if}
</section>
