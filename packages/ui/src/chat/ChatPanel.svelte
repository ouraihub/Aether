<script lang="ts">
  import { tick, untrack } from 'svelte'
  import XIcon from '@lucide/svelte/icons/x'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import HistoryIcon from '@lucide/svelte/icons/history'
  import GlobeIcon from '@lucide/svelte/icons/globe'
  import MaximizeIcon from '@lucide/svelte/icons/maximize-2'
  import MinimizeIcon from '@lucide/svelte/icons/minimize-2'
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down'
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up'
  import SquareIcon from '@lucide/svelte/icons/square'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Alert from '#lib/components/ui/alert/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { getKitContext } from '../context'
  import {
    isKitClientError,
    type ConversationSummary,
    type ModelRef,
    type SkillRef,
    type ModelOption,
    type ReasoningEffort,
  } from '../ports'
  import ModelSelect from '../providers/ModelSelect.svelte'
  import { RunSession } from './run-session.svelte'
  import UsageDetails from './UsageDetails.svelte'
  import MessageItem from './MessageItem.svelte'
  import { REASONING_EFFORTS } from '../providers/reasoning'
  import SkillPicker from './SkillPicker.svelte'

  let {
    purpose = 'chat',
    model = null,
    scope,
    context = null,
    skills,
    onSkillsChange,
    skillsBudget = 48_000,
    onInsert,
    onOpenSettings,
    conversationId = null,
    onConversationChange,
    onClose,
  }: {
    purpose?: string
    model?: ModelRef | null
    scope?: string
    /** Host context can be removed for the next message without changing conversation ownership. */
    context?: { kind: string; id: string; label: string } | null
    skills?: SkillRef[]
    onSkillsChange?: (refs: SkillRef[]) => void
    skillsBudget?: number
    onInsert?: (markdown: string) => void
    onOpenSettings?: () => void
    /** A failed restore stays visible until retried or explicitly replaced with a new conversation. */
    conversationId?: string | null
    onConversationChange?: (id: string | null) => void
    onClose?: () => void
  } = $props()

  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const history = ports.chat?.history
  const session = ports.chat ? new RunSession(ports.chat) : null
  // svelte-ignore state_referenced_locally
  let picked = $state<ModelRef | null>(model)
  let modelOptions = $state<ModelOption[] | null>(null)
  let modelError = $state<string | null>(null)
  let modelReload = $state(0)
  const capability = $derived(
    modelOptions?.find((item) => item.endpointId === picked?.endpointId && item.modelId === picked?.modelId) ?? null,
  )
  let reasoningEffort = $state<ReasoningEffort>('default')
  const allowedEfforts = $derived(capability?.reasoning?.efforts ?? [])
  const noModels = $derived(modelOptions !== null && modelOptions.length === 0)
  const errorText = (error: unknown) => t(`errors.${isKitClientError(error) ? error.code : 'internal'}`)
  $effect(() => {
    void modelReload
    let alive = true
    modelOptions = null
    modelError = null
    if (!ports.providers) {
      modelOptions = []
      return
    }
    void ports.providers.modelOptions({ modelType: 'chat' }).then(
      (list) => {
        if (alive) modelOptions = list
      },
      (error: unknown) => {
        if (alive) modelError = errorText(error)
      },
    )
    return () => {
      alive = false
    }
  })
  $effect(() => {
    if (reasoningEffort !== 'default' && !allowedEfforts.includes(reasoningEffort)) reasoningEffort = 'default'
  })
  function pickModel(next: ModelRef) {
    picked = next
    reasoningEffort = 'default'
    session?.resetContext()
  }

  let draft = $state('')
  let draftRevision = 0
  let contextRevision = 0
  let expanded = $state(false)
  // svelte-ignore state_referenced_locally
  let ownSkills = $state<SkillRef[]>(skills ?? [])
  const selected = $derived(skills ?? ownSkills)
  function setSkills(refs: SkillRef[]) {
    ownSkills = refs
    onSkillsChange?.(refs)
  }
  let droppedKey = $state<string | null>(null)
  const contextKey = $derived(context ? `${context.kind}:${context.id}` : null)
  const activeContext = $derived(context && contextKey !== droppedKey ? context : null)
  let webSearch = $state(false)
  const base = $derived({
    purpose,
    scope: scope ?? null,
    language: i18n.locale,
    model: picked,
    skills: selected,
    reasoningEffort,
    context: activeContext ? { kind: activeContext.kind, id: activeContext.id } : null,
  })
  const currentContext = $derived(
    session?.contextUsage &&
      session.contextUsage.endpointId === picked?.endpointId &&
      session.contextUsage.modelId === picked?.modelId
      ? session.contextUsage
      : null,
  )

  let conversations = $state<ConversationSummary[]>([])
  let shownSummary = $state<ConversationSummary | null>(null)
  const title = $derived(
    (shownSummary?.id === session?.conversationId ? shownSummary?.title : null) ??
      conversations.find((c) => c.id === session?.conversationId)?.title ??
      t('chat.newConversation'),
  )
  let historyOpen = $state(false)
  let historyBusy = $state(false)
  let historyLoading = $state(false)
  let historyError = $state<string | null>(null)
  let retryHistory: () => Promise<void> = refresh
  let renameTitle = $state('')
  let renaming = $state(false)
  // svelte-ignore state_referenced_locally
  let reported = conversationId
  let restored = $state(false)
  let restoreBlocked = $state(false)
  $effect(() => () => session?.stop())
  $effect(() => {
    if (session && !session.running) void refresh()
  })
  $effect(() => {
    const id = session?.conversationId ?? null
    if (!restored || restoreBlocked || id === reported) return
    reported = id
    onConversationChange?.(id)
  })
  // svelte-ignore state_referenced_locally
  void restore(conversationId)

  async function refresh() {
    if (!history) return
    historyLoading = true
    try {
      conversations = await history.list(scope)
      if (retryHistory === refresh) historyError = null
    } catch (error) {
      if (!restoreBlocked) {
        historyError = errorText(error)
        retryHistory = refresh
      }
    } finally {
      historyLoading = false
    }
  }
  async function restore(id: string | null) {
    if (id !== null && history && session) {
      restoreBlocked = true
      await open(id)
    }
    restored = true
  }
  async function open(id: string) {
    if (!history || !session || session.running || historyBusy) return
    historyBusy = true
    try {
      const conversation = await history.get(id)
      session.show(id, conversation.turns)
      shownSummary = conversation.summary
      restoreBlocked = false
      restored = true
      historyError = null
      retryHistory = refresh
      historyOpen = false
      renaming = false
      follow = true
    } catch (error) {
      historyError = errorText(error)
      retryHistory = () => open(id)
    } finally {
      historyBusy = false
    }
  }
  function newConversation() {
    if (!session || session.running || historyBusy) return
    session.show(null)
    shownSummary = null
    restoreBlocked = false
    restored = true
    historyError = null
    historyOpen = false
    renaming = false
    follow = true
  }
  async function rename() {
    const id = session?.conversationId
    const next = renameTitle.trim()
    if (!history || !id || !next || session?.running || historyBusy) return
    historyBusy = true
    try {
      const summary = await history.rename(id, next)
      shownSummary = summary
      conversations = conversations.map((c) => (c.id === id ? summary : c))
      renaming = false
      historyError = null
    } catch (error) {
      historyError = errorText(error)
      retryHistory = rename
    } finally {
      historyBusy = false
    }
  }
  async function remove() {
    const id = session?.conversationId
    if (!history || !session || !id || session.running || historyBusy) return
    historyBusy = true
    try {
      const ok = await notify.confirm({ title: t('chat.deleteTitle'), body: t('chat.deleteBody'), danger: true })
      if (!ok) return
      await history.remove(id)
      if (session.conversationId === id) {
        session.show(null)
        shownSummary = null
        historyOpen = false
        renaming = false
        follow = true
      }
      historyError = null
      void refresh()
    } catch (error) {
      historyError = errorText(error)
      retryHistory = remove
    } finally {
      historyBusy = false
    }
  }

  let scroller = $state<HTMLDivElement | null>(null)
  let follow = $state(true)
  let lastScrollTop = 0
  function onscroll() {
    if (!scroller) return
    const nearBottom = scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop <= 48
    follow = nearBottom && scroller.scrollTop >= lastScrollTop
    lastScrollTop = scroller.scrollTop
  }
  function toBottom() {
    follow = true
    if (scroller) {
      scroller.scrollTop = scroller.scrollHeight
      lastScrollTop = scroller.scrollTop
    }
  }
  $effect.pre(() => {
    if (!session || !scroller) return
    for (const turn of session.turns) {
      void turn.text
      void turn.reasoningText
      void turn.status
      void turn.error
      void turn.usage
      void turn.search
      for (const tool of turn.tools) {
        void tool.pending
        void tool.summary
        void tool.outcome
      }
    }
    void expanded
    if (untrack(() => follow))
      void tick().then(() => {
        if (follow) toBottom()
      })
  })
  function onkeydown(e: KeyboardEvent) {
    if (e.key !== 'Escape' || e.defaultPrevented || e.isComposing) return
    if (historyOpen || document.querySelector('[data-slot="select-content"], [role="dialog"]')) return
    if (onClose && !session?.running) onClose()
  }
  const canSend = $derived(
    session !== null &&
      !session.running &&
      restored &&
      !restoreBlocked &&
      !historyBusy &&
      capability !== null &&
      modelError === null &&
      draft.trim() !== '',
  )
  let recovery = $state<{ draft: string; droppedKey: string | null; contextKey: string | null } | null>(null)
  function recoverDraft() {
    if (!recovery || draft !== '') return
    draft = recovery.draft
    draftRevision++
    if (contextKey === recovery.contextKey) droppedKey = recovery.droppedKey
    contextRevision++
    recovery = null
  }
  async function send() {
    if (!session || !canSend) return
    const original = draft
    const revision = draftRevision
    const contextVersion = contextRevision
    const removed = droppedKey
    const key = contextKey
    draft = ''
    recovery = null
    follow = true
    const accepted = await session.send(base, original.trim(), webSearch)
    if (accepted) {
      if (contextRevision === contextVersion && contextKey === key) droppedKey = null
    } else if (draftRevision === revision) {
      draft = original
      if (contextRevision === contextVersion && contextKey === key) droppedKey = removed
    } else {
      recovery = { draft: original, droppedKey: removed, contextKey: key }
    }
  }
</script>

{#if session}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions (Esc bubbles from controls) -->
  <section class="chat-panel" aria-label={t('chat.title')} {onkeydown}>
    <header class="chat-header">
      <div class="title-row">
        <h2 {title}>{title}</h2>
        {#if history}
          <Button
            variant="ghost"
            size="icon-sm"
            disabled={session.running || historyBusy}
            aria-label={t('chat.history')}
            title={t('chat.history')}
            onclick={() => {
              historyOpen = true
              void refresh()
            }}><HistoryIcon /></Button
          >
        {/if}
        <Button
          variant="ghost"
          size="icon-sm"
          disabled={session.running || historyBusy}
          aria-label={t('chat.newConversation')}
          title={t('chat.newConversation')}
          onclick={newConversation}><PlusIcon /></Button
        >
        {#if onClose}<Button
            variant="ghost"
            size="icon-sm"
            aria-label={t('chat.close')}
            title={t('chat.close')}
            onclick={onClose}><XIcon /></Button
          >{/if}
      </div>
      <div class="model-row">
        <ModelSelect
          value={picked}
          options={modelOptions}
          filter={{ modelType: 'chat' }}
          disabled={session.running || modelError !== null}
          onchange={pickModel}
        />
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
            onclick={() => void retryHistory()}>{t('chat.retryHistory')}</Button
          ></Alert.Description
        >
      </Alert.Root>
    {/if}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (native scroll region needs keyboard focus for PageUp/PageDown) -->
    <div class="messages" bind:this={scroller} {onscroll} role="region" aria-label={t('chat.messages')} tabindex="0">
      {#if session.turns.length > 0}
        <ol class="turns">
          {#each session.turns as turn, i (i)}
            <MessageItem
              {turn}
              {onInsert}
              onRetry={i === session.turns.length - 1 && !session.running && capability !== null && !restoreBlocked
                ? () => {
                    follow = true
                    session.regenerate(base, webSearch)
                  }
                : undefined}
              onApprove={(tool, allow) => session.approve(tool, allow)}
            />
          {/each}
        </ol>
      {/if}
      {#if modelError}
        <Alert.Root variant="destructive">
          <Alert.Title>{t('chat.modelsFailed')}</Alert.Title>
          <Alert.Description
            >{modelError}<Button variant="outline" size="sm" onclick={() => modelReload++}
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
    </div>
    {#if !follow && session.turns.length > 0}
      <div class="bottom-control">
        <Button type="button" variant="secondary" size="sm" onclick={toBottom}
          ><ArrowDownIcon data-icon="inline-start" />{t('chat.backToBottom')}</Button
        >
      </div>
    {/if}
    <form
      class="composer"
      class:expanded
      onsubmit={(e) => {
        e.preventDefault()
        void send()
      }}
    >
      {#if activeContext}
        <div class="context-chip">
          <span title={activeContext.label}>{activeContext.label}</span><Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={t('chat.context.remove', { label: activeContext.label })}
            title={t('chat.context.remove', { label: activeContext.label })}
            onclick={() => {
              droppedKey = contextKey
              contextRevision++
            }}><XIcon /></Button
          >
        </div>
      {/if}
      <Textarea
        bind:value={draft}
        rows={3}
        aria-label={t('chat.input')}
        placeholder={t('chat.placeholder')}
        oninput={() => draftRevision++}
        onkeydown={(e) => {
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && !e.isComposing && e.keyCode !== 229) {
            e.preventDefault()
            if (!e.repeat) void send()
          }
        }}
      />
      {#if recovery}
        <div class="draft-recovery" role="status">
          <span>{t('chat.draftRecoverable')}</span><Button
            type="button"
            variant="outline"
            size="sm"
            disabled={draft !== ''}
            onclick={recoverDraft}>{t('chat.restoreDraft')}</Button
          >
        </div>
      {/if}
      {#if ports.search && purpose === 'chat' && session.search.status === 'searching'}<p
          class="search-status"
          role="status"
        >
          {t('chat.searching')}
        </p>{/if}
      <div class="composer-tools">
        <div class="tool-group">
          {#if ports.search && purpose === 'chat'}<Button
              type="button"
              variant={webSearch ? 'secondary' : 'ghost'}
              size="icon-sm"
              aria-label={t('chat.network')}
              aria-pressed={webSearch}
              disabled={session.running}
              title={`${t(webSearch ? 'chat.networkOn' : 'chat.networkOff')} · ${t('chat.networkPrivacy')}`}
              onclick={() => (webSearch = !webSearch)}><GlobeIcon /></Button
            >{/if}
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={t(expanded ? 'chat.collapseInput' : 'chat.expandInput')}
            title={t(expanded ? 'chat.collapseInput' : 'chat.expandInput')}
            aria-expanded={expanded}
            onmousedown={(e) => e.preventDefault()}
            onclick={() => {
              expanded = !expanded
            }}
          >
            {#if expanded}<MinimizeIcon />{:else}<MaximizeIcon />{/if}
          </Button>
          {#if ports.skills}<div class="skills-control">
              <SkillPicker
                port={ports.skills}
                value={selected}
                budget={skillsBudget}
                changed={session.skillChanged}
                onchange={setSkills}
                onresolved={() => (session.skillChanged = null)}
              />
            </div>{/if}
        </div>
        <div class="tool-group send-tools">
          {#if allowedEfforts.length > 0}
            <div class="effort-control">
              <Select.Root
                type="single"
                value={reasoningEffort}
                onValueChange={(value) => (reasoningEffort = value as ReasoningEffort)}
              >
                <Select.Trigger size="sm" disabled={session.running} aria-label={t('chat.reasoningEffort')}
                  >{t(`chat.effort.${reasoningEffort}`)}</Select.Trigger
                >
                <Select.Content
                  ><Select.Group
                    ><Select.Item
                      value="default"
                      label={t('chat.effort.default')}
                    />{#each REASONING_EFFORTS.filter( (effort) => allowedEfforts.includes(effort) ) as effort (effort)}<Select.Item
                        value={effort}
                        label={t(`chat.effort.${effort}`)}
                      />{/each}</Select.Group
                  ></Select.Content
                >
              </Select.Root>
            </div>
          {/if}
          <UsageDetails
            context={currentContext}
            usage={session.turns.at(-1)?.usage ?? null}
            total={session.cumulativeUsage}
          />
          {#if session.running}<Button
              type="button"
              variant="destructive"
              size="icon-sm"
              aria-label={t('chat.stop')}
              title={t('chat.stop')}
              onclick={() => session.stop()}><SquareIcon /></Button
            >
          {:else}<Button
              type="submit"
              size="icon-sm"
              disabled={!canSend}
              aria-label={t('chat.send')}
              title={t('chat.send')}><ArrowUpIcon /></Button
            >{/if}
        </div>
      </div>
    </form>
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
                    ><Field.FieldLabel for="chat-conversation-title">{t('chat.conversationTitle')}</Field.FieldLabel
                    ><Input id="chat-conversation-title" bind:value={renameTitle} disabled={historyBusy} /></Field.Field
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
            <Button variant="destructive" disabled={historyBusy || session.running} onclick={() => void remove()}
              >{t('chat.delete')}</Button
            >
          {/if}
          <Dialog.Footer
            ><Button variant="ghost" onclick={() => (historyOpen = false)}>{t('chat.close')}</Button></Dialog.Footer
          >
        </Dialog.Content>
      </Dialog.Root>
    {/if}
  </section>
{/if}

<style>
  .chat-panel {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 0.75rem;
    min-width: 0;
    min-height: 0;
    height: 100%;
    container-type: size;
  }
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
  .model-row {
    min-width: 0;
  }
  .model-row :global([data-slot='select-trigger']) {
    width: 100%;
    min-width: 0;
  }
  .messages {
    flex: 1;
    min-height: 0;
    min-width: 0;
    overflow-y: auto;
    overflow-x: clip;
    overscroll-behavior: contain;
    overflow-anchor: none;
  }
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
  .bottom-control {
    display: flex;
    justify-content: center;
  }
  .composer {
    flex: none;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    border: 1px solid var(--color-border);
    border-radius: calc(var(--kit-radius, var(--radius)) * 1.5);
    padding: 0.5rem;
  }
  .composer :global(textarea) {
    min-height: 5rem;
    height: 5rem;
    max-height: 5rem;
    field-sizing: fixed;
    resize: none;
    border: 0;
    box-shadow: none;
    border-radius: calc(var(--kit-radius, var(--radius)) * 0.5);
  }
  .composer.expanded :global(textarea) {
    height: 40cqh;
    max-height: 40cqh;
  }
  .composer :global(textarea:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: -2px;
  }
  .context-chip {
    display: flex;
    align-items: center;
    align-self: flex-start;
    gap: 0.25rem;
    max-width: 100%;
    padding-left: 0.5rem;
    background: var(--color-muted);
    border-radius: var(--kit-radius, var(--radius));
  }
  .context-chip span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .context-chip :global(button) {
    flex-shrink: 0;
  }
  .composer-tools,
  .tool-group {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.25rem;
    min-width: 0;
  }
  .composer-tools {
    justify-content: space-between;
    gap: 0.5rem;
  }
  .send-tools {
    margin-left: auto;
  }
  .effort-control {
    min-width: 0;
  }
  .skills-control {
    min-width: 0;
    max-width: 100%;
  }
  .skills-control :global(div:first-child) {
    flex-wrap: wrap;
    min-width: 0;
  }
  .skills-control :global([data-slot='select-trigger']) {
    width: auto;
    max-width: 100%;
  }
  .search-status,
  .draft-recovery {
    margin: 0;
    color: var(--color-muted-foreground);
    font-size: var(--kit-font-size-sm, 0.8125rem);
  }
  .draft-recovery {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }
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
</style>
