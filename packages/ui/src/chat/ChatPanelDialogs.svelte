<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import ModelSelect from '../providers/ModelSelect.svelte'
  import type { KitI18n } from '../i18n'
  import type { ConversationClient, ConversationSummary, ModelOption, ModelRef } from '../ports'
  let {
    i18n,
    history,
    session,
    conversations,
    title,
    historyLoading,
    historyBusy,
    historyError,
    retryHistory,
    open,
    rename,
    remove,
    actionBusy,
    replyDialog,
    modelOptions,
    modelError,
    noModels,
    validReplyModel,
    submitReply,
    reloadModels,
    historyOpen = $bindable(false),
    renaming = $bindable(false),
    renameTitle = $bindable(''),
    replyOpen = $bindable(false),
    editDraft = $bindable(''),
    replyModel = $bindable<ModelRef | null>(null),
    targetLanguage = $bindable(''),
    replyError,
  }: {
    i18n: KitI18n
    history: ConversationClient | undefined
    session: { running: boolean; conversationId: string | null }
    conversations: ConversationSummary[]
    title: string
    historyLoading: boolean
    historyBusy: boolean
    historyError: string | null
    retryHistory: () => Promise<void>
    open: (id: string) => Promise<void>
    rename: () => Promise<void>
    remove: () => Promise<void>
    actionBusy: boolean
    replyDialog: 'edit' | 'translate' | null
    modelOptions: ModelOption[] | null
    modelError: string | null
    noModels: boolean
    validReplyModel: boolean
    submitReply: () => Promise<void>
    reloadModels: () => void
    historyOpen?: boolean
    renaming?: boolean
    renameTitle?: string
    replyOpen?: boolean
    editDraft?: string
    replyModel?: ModelRef | null
    targetLanguage?: string
    replyError: string | null
  } = $props()
  const t = $derived(i18n.t)
</script>

{#if history}
  <Dialog.Root bind:open={historyOpen}>
    <Dialog.Content showCloseButton={false}>
      <Dialog.Header><Dialog.Title>{t('chat.history')}</Dialog.Title></Dialog.Header>
      {#if historyLoading}<p role="status">{t('chat.historyLoading')}</p>{/if}
      {#if historyError}<p role="alert">
          {t('chat.historyFailed')}: {historyError}<Button
            variant="outline"
            size="sm"
            disabled={historyBusy}
            onclick={() => void retryHistory()}>{t('chat.retryHistory')}</Button
          >
        </p>{/if}
      <div class="history-list">
        {#each conversations as c (c.id)}<Button
            variant="ghost"
            title={c.title}
            disabled={historyBusy || session.running}
            onclick={() => void open(c.id)}><span>{c.title}</span></Button
          >{/each}
      </div>
      {#if session.conversationId}
        {#if renaming}
          <form
            onsubmit={(e) => {
              e.preventDefault()
              void rename()
            }}
          >
            <Field.FieldGroup
              ><Field.Field
                ><Field.FieldLabel for="chat-conversation-title"
                  >{t('chat.conversationTitle')}</Field.FieldLabel
                ><Input
                  id="chat-conversation-title"
                  bind:value={renameTitle}
                  disabled={historyBusy}
                /></Field.Field
              ><Button type="submit" disabled={historyBusy || renameTitle.trim() === ''}
                >{t('common.confirm')}</Button
              ></Field.FieldGroup
            >
          </form>
        {:else}<Button
            variant="outline"
            disabled={historyBusy || session.running}
            onclick={() => {
              renameTitle = title
              renaming = true
            }}>{t('chat.rename')}</Button
          >{/if}
        <Button
          variant="destructive"
          disabled={historyBusy || session.running}
          onclick={() => void remove()}>{t('chat.delete')}</Button
        >
      {/if}
      <Dialog.Footer
        ><Button variant="ghost" onclick={() => (historyOpen = false)}>{t('chat.close')}</Button
        ></Dialog.Footer
      >
    </Dialog.Content>
  </Dialog.Root>
{/if}
<Dialog.Root bind:open={replyOpen}>
  <Dialog.Content class="reply-dialog" showCloseButton={false}>
    <Dialog.Header>
      <Dialog.Title>{t(replyDialog === 'edit' ? 'chat.editReply' : 'chat.translate')}</Dialog.Title>
      <Dialog.Description
        >{t(
          replyDialog === 'edit' ? 'chat.editReplyHint' : 'chat.candidateHint',
        )}</Dialog.Description
      >
    </Dialog.Header>
    <form
      onsubmit={(event) => {
        event.preventDefault()
        void submitReply()
      }}
    >
      <Field.FieldGroup>
        {#if replyDialog === 'edit'}
          <Field.Field
            ><Field.FieldLabel for="chat-reply-edit">{t('chat.replyText')}</Field.FieldLabel
            ><Textarea
              id="chat-reply-edit"
              bind:value={editDraft}
              rows={8}
              maxlength={24_000}
              disabled={actionBusy}
            /></Field.Field
          >
        {:else}
          <Field.Field
            ><Field.FieldLabel>{t('chat.replyModel')}</Field.FieldLabel><ModelSelect
              value={replyModel}
              options={modelOptions}
              disabled={actionBusy || modelError !== null}
              onchange={(value) => (replyModel = value)}
            /></Field.Field
          >
          {#if noModels}<p role="status">{t('chat.noModels')}</p>{/if}
          {#if modelError}<p role="alert">
              {modelError}<Button type="button" variant="outline" size="sm" onclick={reloadModels}
                >{t('chat.retryModels')}</Button
              >
            </p>{/if}
          <Field.Field
            ><Field.FieldLabel for="chat-target-language"
              >{t('chat.targetLanguage')}</Field.FieldLabel
            ><Input
              id="chat-target-language"
              bind:value={targetLanguage}
              maxlength={80}
              disabled={actionBusy}
            /></Field.Field
          >
        {/if}
        {#if replyError}<p role="alert" class="text-destructive">{replyError}</p>{/if}
        <Dialog.Footer>
          <Button
            type="button"
            variant="ghost"
            disabled={actionBusy}
            onclick={() => (replyOpen = false)}>{t('common.cancel')}</Button
          >
          <Button
            type="submit"
            disabled={actionBusy ||
              (replyDialog === 'edit'
                ? !editDraft.trim() || editDraft.length > 24_000
                : !validReplyModel || !targetLanguage.trim())}>{t('common.confirm')}</Button
          >
        </Dialog.Footer>
      </Field.FieldGroup>
    </form>
  </Dialog.Content>
</Dialog.Root>

<style>
  .history-list {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    max-height: 40vh;
    overflow-y: auto;
    min-width: 0;
  }
  .history-list :global(button) {
    justify-content: flex-start;
    min-width: 0;
  }
  .history-list span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  :global(.reply-dialog) {
    max-height: calc(100dvh - 2rem);
    overflow-y: auto;
  }
  :global(.reply-dialog form),
  :global(.reply-dialog [data-slot='field-group']) {
    min-width: 0;
  }
  :global(.reply-dialog textarea) {
    max-height: 45dvh;
    overflow-y: auto;
  }
</style>
