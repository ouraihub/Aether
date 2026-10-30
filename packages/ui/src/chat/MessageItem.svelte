<script lang="ts">
  // Full reply candidate; the panel groups shared user input by questionId.
  import { Button } from '#lib/components/ui/button/index.js'
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js'
  import { getKitContext } from '../context'
  import ChatMessage from './ChatMessage.svelte'
  import type { ChatTurn } from './run-session.svelte'
  import type { ImageRef, ModelOption, ModelRef } from '../ports'
  import ModelSelect from '../providers/ModelSelect.svelte'
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link'
  import { onDestroy } from 'svelte'
  import CopyIcon from '@lucide/svelte/icons/copy'
  import CheckIcon from '@lucide/svelte/icons/check'
  import FileInputIcon from '@lucide/svelte/icons/file-input'
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw'
  import ToolCallItem from './ToolCallItem.svelte'
  import MenuIcon from '@lucide/svelte/icons/menu'
  import LanguagesIcon from '@lucide/svelte/icons/languages'
  import TrashIcon from '@lucide/svelte/icons/trash-2'
  import FilePlusIcon from '@lucide/svelte/icons/file-plus'
  import AtSignIcon from '@lucide/svelte/icons/at-sign'
  import ThumbsUpIcon from '@lucide/svelte/icons/thumbs-up'
  import FilePenLineIcon from '@lucide/svelte/icons/file-pen-line'
  import SplitIcon from '@lucide/svelte/icons/split'
  import CopyPlusIcon from '@lucide/svelte/icons/copy-plus'
  import SaveIcon from '@lucide/svelte/icons/save'
  import UploadIcon from '@lucide/svelte/icons/upload'

  let {
    turn,
    latest = false,
    onInsert,
    onRetry,
    modelLabel,
    chosen = false,
    showInput = true,
    busy = false,
    editable = false,
    onEdit,
    onBranch,
    onCopyConversation,
    onSwitchModel,
    modelOptions = null,
    sourceModel = null,
    onTranslate,
    onDelete,
    onUseContext,
    onSaveNote,
    onExportMarkdown,
  }: {
    turn: ChatTurn
    latest?: boolean
    onInsert?: ((markdown: string) => void) | undefined
    onRetry?: (() => void) | undefined
    modelLabel?: string
    chosen?: boolean
    showInput?: boolean
    busy?: boolean
    editable?: boolean
    onEdit?: (() => void) | undefined
    onBranch?: (() => void) | undefined
    onCopyConversation?: (() => void) | undefined
    /** Regenerates this reply with the picked model. */
    onSwitchModel?: ((model: ModelRef) => void) | undefined
    modelOptions?: ModelOption[] | null
    sourceModel?: ModelRef | null
    onTranslate?: (() => void) | undefined
    onDelete?: (() => void) | undefined
    onUseContext?: (() => void) | undefined
    onSaveNote?: ((markdown: string, images: ImageRef[]) => Promise<void> | void) | undefined
    onExportMarkdown?: ((markdown: string) => Promise<void> | void) | undefined
  } = $props()
  const { i18n, notify } = getKitContext()
  const { t } = i18n
  const savable = $derived(turn.text !== '' || turn.generated.length > 0)
  function safeUrl(value: string): string | null {
    try {
      const protocol = new URL(value).protocol
      return protocol === 'http:' || protocol === 'https:' ? value : null
    } catch {
      return null
    }
  }
  let copied = $state(false)
  let copyTimer: ReturnType<typeof setTimeout> | undefined
  let alive = true
  onDestroy(() => {
    alive = false
    clearTimeout(copyTimer)
  })

  function insert() {
    try {
      onInsert?.(turn.text)
    } catch {
      notify.toast('error', t('errors.internal'))
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(turn.text)
    } catch {
      notify.toast('error', t('chat.copyFailed'))
      return
    }
    if (!alive) return
    copied = true
    clearTimeout(copyTimer)
    copyTimer = setTimeout(() => (copied = false), 1500)
  }

  async function saveNote() {
    try {
      await onSaveNote?.(turn.text, turn.generated)
    } catch {
      notify.toast('error', t('chat.saveNoteFailed'))
    }
  }
  async function download() {
    let url: string | null = null
    try {
      if (onExportMarkdown) {
        await onExportMarkdown(turn.text)
        return
      }
      url = URL.createObjectURL(new Blob([turn.text], { type: 'text/markdown;charset=utf-8' }))
      const link = document.createElement('a')
      link.href = url
      link.download = `reply-${turn.id}.md`
      document.body.append(link)
      try {
        link.click()
      } finally {
        link.remove()
      }
    } catch {
      notify.toast('error', t('chat.exportFailed'))
    } finally {
      if (url) URL.revokeObjectURL(url)
    }
  }
  // Browsers refuse to navigate a new tab to a `data:` URL, and a strict CSP connect-src
  // blocks fetch(data:); decode locally and hand it over as a blob.
  function openImage(event: MouseEvent, url: string | null) {
    if (!url?.startsWith('data:')) return
    event.preventDefault()
    const comma = url.indexOf(',')
    const mime = url.slice(5, url.indexOf(';'))
    const bytes = Uint8Array.from(atob(url.slice(comma + 1)), (c) => c.charCodeAt(0))
    const blobUrl = URL.createObjectURL(new Blob([bytes], { type: mime }))
    window.open(blobUrl, '_blank', 'noopener')
    setTimeout(() => URL.revokeObjectURL(blobUrl), 60_000)
  }
</script>

<li class="message-turn flex min-w-0 flex-col gap-2">
  {#if showInput}
    <ChatMessage role="user" text={turn.input} showMoreLabel={t('chat.showMore')} showLessLabel={t('chat.showLess')}>
      {#snippet attachments()}
        {#if turn.images.length > 0}
          <ul class="user-images">
            {#each turn.images as image, index (index)}
              <li>
                {#if image.url}<a
                    href={image.url}
                    target="_blank"
                    rel="noopener"
                    onclick={(event) => openImage(event, image.url)}
                    aria-label={t('chat.openImage', { index: index + 1 })}
                    title={t('chat.openImage', { index: index + 1 })}><img src={image.url} alt="" /></a
                  >{:else}<span class="image-expired">{t('chat.imageExpired')}</span>{/if}
              </li>
            {/each}
          </ul>
        {/if}
      {/snippet}
    </ChatMessage>
  {/if}
  <ChatMessage
    role="assistant"
    text={turn.text}
    label={modelLabel}
    status={turn.status}
    reasoning={turn.reasoningText}
    reasoningLabel={t('chat.reasoning')}
    selected={chosen}
    {latest}
  >
    {#snippet process()}
      {#if turn.tools.some((tool) => tool.pending === null)}
        <details class="tool-process" open={turn.status === 'running'}>
          <summary>{t('chat.toolProcess')}</summary>
          {#each turn.tools.filter((tool) => tool.pending === null) as tool (tool.callId)}<ToolCallItem {tool} />{/each}
        </details>
      {/if}
    {/snippet}
    {#snippet attachments()}
      {#if turn.generated.length > 0}
        <ul class="generated-images" aria-label={t('chat.generatedImages')}>
          {#each turn.generated as image, index (index)}
            <li>
              {#if image.url}<a
                  href={image.url}
                  target="_blank"
                  rel="noopener"
                  onclick={(event) => openImage(event, image.url)}
                  aria-label={t('chat.openGeneratedImage', { index: index + 1 })}
                  title={t('chat.openGeneratedImage', { index: index + 1 })}><img src={image.url} alt="" /></a
                >{:else}<span class="image-expired">{t('chat.imageExpired')}</span>{/if}
            </li>
          {/each}
        </ul>
      {/if}
    {/snippet}
    {#snippet actions()}
      {#if savable || (onRetry && turn.status !== 'running') || onUseContext || onDelete}
        {#if turn.text !== ''}
          <Button
            variant="ghost"
            size="icon-xs"
            title={t('chat.copy')}
            aria-label={t('chat.copy')}
            onclick={() => void copy()}
          >
            {#if copied}<CheckIcon />{:else}<CopyIcon />{/if}
          </Button>
        {/if}
        {#if onRetry && turn.status !== 'running'}
          {@const label = t(turn.status === 'failed' ? 'chat.retry' : 'chat.regenerate')}
          <Button variant="ghost" size="icon-xs" title={label} aria-label={label} disabled={busy} onclick={onRetry}
            ><RotateCcwIcon /></Button
          >
        {/if}
        {#if onSwitchModel}<ModelSelect
            value={sourceModel}
            options={modelOptions}
            disabled={busy}
            onchange={(model) => onSwitchModel(model)}
          >
            {#snippet trigger({ props })}<Button
                {...props}
                variant="ghost"
                size="icon-xs"
                title={t('chat.switchModel')}
                aria-label={t('chat.switchModel')}><AtSignIcon /></Button
              >{/snippet}
          </ModelSelect>{/if}
        {#if onTranslate}<Button
            variant="ghost"
            size="icon-xs"
            title={t('chat.translate')}
            aria-label={t('chat.translate')}
            disabled={busy || !turn.text}
            onclick={onTranslate}><LanguagesIcon /></Button
          >{/if}
        {#if onUseContext}<Button
            variant="ghost"
            size="icon-xs"
            title={t('chat.useContext')}
            aria-label={t('chat.useContext')}
            aria-pressed={chosen}
            disabled={busy || turn.status !== 'completed'}
            onclick={onUseContext}><ThumbsUpIcon class={chosen ? 'fill-primary text-primary' : ''} /></Button
          >{/if}
        {#if onSaveNote && savable}<Button
            variant="ghost"
            size="icon-xs"
            title={t('chat.saveNote')}
            aria-label={t('chat.saveNote')}
            disabled={busy || turn.status === 'running'}
            onclick={() => void saveNote()}><FilePlusIcon /></Button
          >{/if}
        {#if onDelete}<Button
            variant="ghost"
            size="icon-xs"
            title={t('chat.deleteReply')}
            aria-label={t('chat.deleteReply')}
            disabled={busy || turn.status === 'running'}
            onclick={onDelete}><TrashIcon /></Button
          >{/if}
        {#if onInsert}
          <Button
            variant="ghost"
            size="icon-xs"
            title={t('chat.insert')}
            aria-label={t('chat.insert')}
            disabled={busy || turn.status === 'running'}
            onclick={insert}
          >
            <FileInputIcon />
          </Button>
        {/if}
        {#if savable || onEdit || onBranch || onCopyConversation}
          <DropdownMenu.Root>
            <DropdownMenu.Trigger>
              {#snippet child({ props })}<Button
                  {...props}
                  variant="ghost"
                  size="icon-xs"
                  title={t('chat.more')}
                  aria-label={t('chat.more')}><MenuIcon /></Button
                >{/snippet}
            </DropdownMenu.Trigger>
            <DropdownMenu.Content class="w-auto">
              {#if onEdit || onBranch || onCopyConversation}
                <DropdownMenu.Group>
                  {#if onEdit}<DropdownMenu.Item disabled={busy || !editable} onSelect={() => onEdit?.()}
                      ><FilePenLineIcon />{t('chat.editReply')}</DropdownMenu.Item
                    >{/if}
                  {#if onBranch}<DropdownMenu.Item disabled={busy} onSelect={() => onBranch?.()}
                      ><SplitIcon />{t('chat.branch')}</DropdownMenu.Item
                    >{/if}
                  {#if onCopyConversation}<DropdownMenu.Item disabled={busy} onSelect={() => onCopyConversation?.()}
                      ><CopyPlusIcon />{t('chat.copyConversation')}</DropdownMenu.Item
                    >{/if}
                </DropdownMenu.Group>
                <DropdownMenu.Separator />
              {/if}
              <DropdownMenu.Sub>
                <DropdownMenu.SubTrigger disabled={!savable}><SaveIcon />{t('chat.save')}</DropdownMenu.SubTrigger>
                <DropdownMenu.SubContent>
                  <DropdownMenu.Item disabled={!turn.text} onSelect={() => void download()}
                    >{t('chat.saveFile')}</DropdownMenu.Item
                  >
                  {#if onSaveNote}<DropdownMenu.Item
                      disabled={busy || turn.status === 'running' || !savable}
                      onSelect={() => void saveNote()}>{t('chat.saveNote')}</DropdownMenu.Item
                    >{/if}
                </DropdownMenu.SubContent>
              </DropdownMenu.Sub>
              <DropdownMenu.Separator />
              <DropdownMenu.Sub>
                <DropdownMenu.SubTrigger disabled={!turn.text}><UploadIcon />{t('chat.export')}</DropdownMenu.SubTrigger
                >
                <DropdownMenu.SubContent>
                  <DropdownMenu.Item disabled={!turn.text} onSelect={() => void download()}
                    >{t('chat.exportMarkdown')}</DropdownMenu.Item
                  >
                </DropdownMenu.SubContent>
              </DropdownMenu.Sub>
            </DropdownMenu.Content>
          </DropdownMenu.Root>
        {/if}
        <span class="text-muted-foreground text-kit-sm" role="status">{copied ? t('chat.copied') : ''}</span>
      {/if}
    {/snippet}
    {#if turn.search.status === 'complete'}
      <section class="flex flex-col gap-1 text-kit-sm" aria-label={t('chat.sources')}>
        <p class="text-muted-foreground">{t('chat.sources')}</p>
        {#each turn.search.hits as hit (hit.url)}
          {@const url = safeUrl(hit.url)}
          {#if url}<a
              class="text-primary flex min-w-0 items-center gap-1 underline"
              href={url}
              target="_blank"
              rel="noopener noreferrer"
              ><ExternalLinkIcon data-icon="inline-start" /><span class="truncate">{hit.title || url}</span></a
            >{/if}
        {/each}
      </section>
    {/if}
    {#if turn.status === 'cancelled'}
      <p class="text-muted-foreground text-kit-sm" role="status">{t('chat.stopped')}</p>
    {:else if turn.status === 'failed'}
      <div class="flex items-center gap-2">
        <p class="text-destructive text-kit-sm" role="alert">
          {turn.error === 'interrupted' ? t('chat.interrupted') : t(`errors.${turn.error ?? 'internal'}`)}
        </p>
      </div>
    {:else if turn.exhausted}
      <p class="text-muted-foreground text-kit-sm">{t('chat.exhausted')}</p>
    {/if}
  </ChatMessage>
</li>

<style>
  .message-turn {
    min-width: 0;
    max-width: 100%;
    overflow-wrap: anywhere;
  }
  .user-images {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.375rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .generated-images {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 12rem), 1fr));
    gap: 0.5rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .generated-images a {
    display: block;
    overflow: hidden;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
  }
  .generated-images a:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
  .generated-images img {
    display: block;
    width: 100%;
    height: auto;
  }
  .user-images a,
  .image-expired {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 5rem;
    height: 5rem;
    overflow: hidden;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
  }
  .user-images a:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
  .user-images img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .image-expired {
    padding: 0.25rem;
    color: var(--color-muted-foreground);
    font-size: var(--kit-font-size-sm, 0.8125rem);
    text-align: center;
  }
  .tool-process {
    color: var(--color-muted-foreground);
    font-size: var(--kit-text-sm);
  }
  .tool-process summary {
    cursor: pointer;
  }
  .tool-process summary:focus-visible {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
</style>
