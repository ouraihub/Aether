<script lang="ts">
  // Full reply candidate; the panel groups shared user input by questionId.
  import { Button } from '#lib/components/ui/button/index.js'
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js'
  import { getKitContext } from '../context'
  import { renderMarkdown } from '../markdown'
  import ApprovalCard from './ApprovalCard.svelte'
  import type { ChatTurn, ToolItem } from './run-session.svelte'
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
    onApprove,
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
    onApprove: (tool: ToolItem, allow: boolean) => void
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
  const html = $derived(renderMarkdown(turn.text))
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
  {#if showInput && turn.images.length > 0}
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
  {#if showInput}<p class="user-message">{turn.input}</p>{/if}
  <div class="assistant-message" class:chosen>
    {#if modelLabel}<div class="reply-source">{modelLabel}</div>{/if}
    {#if turn.reasoningText}
      <details class="thinking text-muted-foreground text-kit-sm">
        <summary class="cursor-pointer">{t('chat.reasoning')}</summary>
        <p class="whitespace-pre-wrap">{turn.reasoningText}</p>
      </details>
    {/if}
    {#each turn.tools as tool (tool.callId)}
      {#if tool.pending === null}
        <ToolCallItem {tool} />
      {:else}
        <ApprovalCard {tool} onAnswer={(allow) => onApprove(tool, allow)} />
      {/if}
    {/each}
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
    <div
      class="markdown"
      {@attach (node) => {
        node.innerHTML = html
      }}
    ></div>
    {#if savable || (onRetry && turn.status !== 'running') || onUseContext || onDelete}
      <div class="message-actions">
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
      </div>
    {/if}
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
  </div>
</li>

<style>
  .message-turn,
  .user-message,
  .assistant-message,
  .markdown {
    box-sizing: border-box;
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
  .user-message {
    align-self: flex-end;
    margin: 0;
    padding: 0.625rem 0.75rem;
    border-radius: var(--kit-radius);
    background: var(--color-primary);
    color: var(--color-primary-foreground);
    font-size: var(--kit-text-base);
    white-space: pre-wrap;
  }
  .assistant-message {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 0.75rem;
    border-radius: var(--kit-radius);
    background: var(--color-muted);
    color: var(--color-foreground);
  }
  .assistant-message.chosen {
    outline: 1px solid var(--color-primary);
  }
  .reply-source {
    font-size: var(--kit-text-sm);
    color: var(--color-muted-foreground);
    overflow-wrap: anywhere;
  }
  .thinking {
    min-width: 0;
  }
  .message-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.25rem;
    border-top: 1px solid var(--color-border);
    padding-top: 0.5rem;
  }
  .markdown {
    font-size: var(--kit-text-base);
    line-height: 1.6;
  }
  .markdown :global(p),
  .markdown :global(ul),
  .markdown :global(ol),
  .markdown :global(blockquote),
  .markdown :global(pre),
  .markdown :global(table) {
    margin: 0.75em 0;
  }
  .markdown :global(h1),
  .markdown :global(h2),
  .markdown :global(h3),
  .markdown :global(h4),
  .markdown :global(h5),
  .markdown :global(h6) {
    margin: 1em 0 0.5em;
    font-weight: 600;
    line-height: 1.35;
  }
  .markdown :global(h1) {
    font-size: 1.4em;
  }
  .markdown :global(h2) {
    font-size: 1.25em;
  }
  .markdown :global(h3) {
    font-size: 1.1em;
  }
  .markdown :global(ul),
  .markdown :global(ol) {
    padding-inline-start: 1.5em;
  }
  .markdown :global(ul) {
    list-style: disc;
  }
  .markdown :global(ol) {
    list-style: decimal;
  }
  .markdown :global(li) {
    margin: 0.25em 0;
  }
  .markdown :global(li::marker) {
    color: var(--color-muted-foreground);
  }
  .markdown :global(blockquote) {
    border-inline-start: 3px solid var(--color-border);
    padding-inline-start: 0.75em;
    color: var(--color-muted-foreground);
  }
  .markdown :global(a) {
    color: var(--color-primary);
    text-decoration: underline;
    overflow-wrap: anywhere;
  }
  .markdown :global(code) {
    border-radius: var(--kit-radius-sm);
    background: var(--color-background);
    padding: 0.1em 0.3em;
    font-size: 0.95em;
  }
  .markdown :global(pre) {
    max-width: 100%;
    overflow-x: auto;
    padding: 0.75em;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius-sm);
    background: var(--color-background);
    white-space: pre;
    overflow-wrap: normal;
  }
  .markdown :global(pre code) {
    padding: 0;
    background: none;
  }
  .markdown :global(table) {
    display: block;
    max-width: 100%;
    overflow-x: auto;
    border-collapse: collapse;
  }
  .markdown :global(th),
  .markdown :global(td) {
    padding: 0.4em 0.6em;
    border: 1px solid var(--color-border);
  }
  .markdown :global(th) {
    font-weight: 600;
  }
  .markdown :global(img),
  .markdown :global(video) {
    max-width: 100%;
    height: auto;
  }
  .markdown :global(:first-child) {
    margin-top: 0;
  }
  .markdown :global(:last-child) {
    margin-bottom: 0;
  }
</style>
