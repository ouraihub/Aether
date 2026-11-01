<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Alert from '#lib/components/ui/alert/index.js'
  import type { ConversationClient, ImageRef, ModelOption, ModelRef } from '../ports'
  import type { ChatTurn, RunSession } from './run-session.svelte'
  import MessageItem from './MessageItem.svelte'
  import type { KitI18n } from '../i18n'

  let {
    i18n,
    session,
    groups,
    purpose,
    history,
    restoreBlocked,
    actionBusy,
    modelOptions,
    modelError,
    capability,
    noModels,
    onRetryModels,
    onOpenSettings,
    onInsert,
    onSaveNote,
    onExportMarkdown,
    sourceLabel,
    sourceModel,
    canEdit,
    onRetry,
    onEdit,
    onBranch,
    onCopyConversation,
    onSwitchModel,
    onTranslate,
    onDelete,
    onUseContext,
  }: {
    i18n: KitI18n
    session: RunSession
    groups: [string, ChatTurn[]][]
    purpose: string
    history: ConversationClient | undefined
    restoreBlocked: boolean
    actionBusy: boolean
    modelOptions: ModelOption[] | null
    modelError: string | null
    noModels: boolean
    capability: ModelOption | null
    onRetryModels: () => void
    onOpenSettings?: (() => void) | undefined
    onInsert?: ((markdown: string) => void) | undefined
    onSaveNote?: ((markdown: string, images: ImageRef[]) => Promise<void> | void) | undefined
    onExportMarkdown?: ((markdown: string) => Promise<void> | void) | undefined
    sourceLabel: (turn: ChatTurn) => string
    sourceModel: (turn: ChatTurn) => ModelRef | null
    canEdit: (turn: ChatTurn) => boolean
    onRetry?: (turn: ChatTurn) => void
    onEdit?: (turn: ChatTurn) => void
    onBranch?: (turn: ChatTurn) => void
    onCopyConversation?: (turn: ChatTurn) => void
    onSwitchModel?: (turn: ChatTurn, model: ModelRef) => void
    onTranslate?: (turn: ChatTurn) => void
    onDelete?: (turn: ChatTurn) => void
    onUseContext?: (turn: ChatTurn) => void
  } = $props()
  const t = $derived(i18n.t)
</script>

{#if session.turns.length > 0}
  <ol class="turns">
    {#each groups as [questionId, replies] (questionId)}
      {@const candidates = replies.filter((reply) => reply.kind === 'reply').length > 1}
      {#each replies as turn, i (turn.id)}
        <MessageItem
          {turn}
          {onInsert}
          {onSaveNote}
          {onExportMarkdown}
          latest={turn.id === session.turns.at(-1)?.id}
          modelLabel={turn.kind === 'translation'
            ? `${t('chat.translationReply')} · ${sourceLabel(turn)}`
            : sourceLabel(turn)}
          showInput={i === 0 && turn.kind !== 'translation'}
          chosen={candidates && session.selectedTurnId === turn.id}
          busy={actionBusy}
          editable={canEdit(turn)}
          onRetry={purpose === 'chat' && (turn.persisted || capability !== null)
            ? () => onRetry?.(turn)
            : undefined}
          onEdit={purpose === 'chat' && history && session.conversationId && turn.persisted
            ? () => onEdit?.(turn)
            : undefined}
          onBranch={purpose === 'chat' && history && session.conversationId && turn.persisted
            ? () => onBranch?.(turn)
            : undefined}
          onCopyConversation={purpose === 'chat' &&
          history &&
          session.conversationId &&
          turn.persisted
            ? () => onCopyConversation?.(turn)
            : undefined}
          onSwitchModel={purpose === 'chat' && history && session.conversationId && turn.persisted
            ? (picked: ModelRef) => onSwitchModel?.(turn, picked)
            : undefined}
          modelOptions={turn.images.length > 0
            ? (modelOptions?.filter((option) => option.canVision) ?? null)
            : modelOptions}
          sourceModel={sourceModel(turn)}
          onTranslate={purpose === 'chat' && history && session.conversationId && turn.persisted
            ? () => onTranslate?.(turn)
            : undefined}
          onDelete={purpose === 'chat' && history && session.conversationId && turn.persisted
            ? () => onDelete?.(turn)
            : undefined}
          onUseContext={candidates &&
          purpose === 'chat' &&
          history &&
          session.conversationId &&
          turn.persisted
            ? () => onUseContext?.(turn)
            : undefined}
        />
      {/each}
    {/each}
  </ol>
{/if}
{#if modelError}
  <Alert.Root variant="destructive">
    <Alert.Title>{t('chat.modelsFailed')}</Alert.Title>
    <Alert.Description
      >{modelError}<Button variant="outline" size="sm" onclick={onRetryModels}
        >{t('chat.retryModels')}</Button
      ></Alert.Description
    >
  </Alert.Root>
{:else if modelOptions === null}
  <p class="empty-message" role="status">{t('chat.modelsLoading')}</p>
{:else if noModels}
  <Empty.Root
    ><Empty.Description>{t('chat.noModels')}</Empty.Description>{#if onOpenSettings}<Button
        variant="outline"
        size="sm"
        onclick={onOpenSettings}>{t('chat.openSettings')}</Button
      >{/if}</Empty.Root
  >
{:else if session.turns.length === 0 && !restoreBlocked}
  <p class="empty-message">{t('chat.empty')}</p>
{/if}

<style>
  .turns {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    padding: 0;
    margin: 0;
    list-style: none;
  }
  .empty-message {
    color: var(--color-muted-foreground);
    text-align: center;
    padding: 1.5rem 0.5rem;
    margin: 0;
  }
</style>
