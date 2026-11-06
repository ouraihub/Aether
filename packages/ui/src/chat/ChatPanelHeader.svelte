<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x'
  import HistoryIcon from '@lucide/svelte/icons/history'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Alert from '#lib/components/ui/alert/index.js'
  import type { KitI18n } from '../i18n'
  import type { ConversationClient } from '../ports'
  import type { RunSession } from './run-session.svelte'

  let {
    i18n,
    title,
    session,
    history,
    historyBusy,
    historyOpen,
    historyError,
    onHistory,
    onClose,
    onRetryHistory,
  }: {
    i18n: KitI18n
    title: string
    session: RunSession
    history: ConversationClient | undefined
    historyBusy: boolean
    historyOpen: boolean
    historyError: string | null
    onHistory: () => void
    onClose?: (() => void) | undefined
    onRetryHistory: () => void
  } = $props()
  const t = $derived(i18n.t)
</script>

<header class="chat-header">
  <div class="title-row">
    <h2>{title}</h2>
    {#if history}<Button
        variant="ghost"
        size="icon-sm"
        disabled={session.running || historyBusy}
        aria-label={t('chat.history')}
        title={t('chat.history')}
        onclick={onHistory}><HistoryIcon /></Button
      >{/if}
    {#if onClose}<Button
        variant="ghost"
        size="icon-sm"
        aria-label={t('chat.close')}
        title={t('chat.close')}
        onclick={onClose}><XIcon /></Button
      >{/if}
  </div>
</header>
{#if historyError && !historyOpen}
  <Alert.Root variant="destructive">
    <Alert.Title>{t('chat.historyFailed')}</Alert.Title>
    <Alert.Description
      >{historyError}<Button
        variant="outline"
        size="sm"
        disabled={historyBusy || session.running}
        onclick={onRetryHistory}>{t('chat.retryHistory')}</Button
      ></Alert.Description
    >
  </Alert.Root>
{/if}
{#if session.actionError}<p role="alert" class="text-destructive text-kit-sm">
    {t(`errors.${session.actionError}`)}
  </p>{/if}
{#if session.historyRefreshError}<p role="alert" class="text-destructive text-kit-sm">
    {t('chat.replyRefreshFailed')}<Button
      variant="outline"
      size="sm"
      disabled={session.running || historyBusy}
      onclick={() => session.conversationId && onRetryHistory()}>{t('chat.retryHistory')}</Button
    >
  </p>{/if}

<style>
  .chat-header {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    flex: none;
    min-width: 0;
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    min-width: 0;
  }
  .title-row h2 {
    flex: 1;
    min-width: 0;
    margin: 0;
    font: inherit;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .title-row :global(button) {
    flex-shrink: 0;
  }
</style>
