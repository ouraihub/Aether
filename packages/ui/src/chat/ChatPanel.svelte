<script lang="ts">
  import { tick, untrack } from 'svelte'
  import { SvelteMap } from 'svelte/reactivity'
  import XIcon from '@lucide/svelte/icons/x'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import HistoryIcon from '@lucide/svelte/icons/history'
  import GlobeIcon from '@lucide/svelte/icons/globe'
  import MaximizeIcon from '@lucide/svelte/icons/maximize-2'
  import MinimizeIcon from '@lucide/svelte/icons/minimize-2'
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down'
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up'
  import SquareIcon from '@lucide/svelte/icons/square'
  import GaugeIcon from '@lucide/svelte/icons/gauge'
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down'
  import { Popover } from 'bits-ui'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Alert from '#lib/components/ui/alert/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { getKitContext } from '../context'
  import {
    isKitClientError,
    type ConversationSummary,
    type Conversation,
    type ModelRef,
    type SkillRef,
    type ModelOption,
    type ReasoningEffort,
  } from '../ports'
  import ModelSelect from '../providers/ModelSelect.svelte'
  import { RunSession, type ChatTurn } from './run-session.svelte'
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
    onSaveNote,
    onExportMarkdown,
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
    onSaveNote?: (markdown: string) => Promise<void> | void
    onExportMarkdown?: (markdown: string) => Promise<void> | void
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
  const allowedEfforts = $derived(REASONING_EFFORTS.filter((effort) => capability?.reasoning?.efforts.includes(effort)))
  let reasoningOpen = $state(false)
  const reasoningId = $props.id()
  let reasoningTrigger = $state<HTMLButtonElement | null>(null)
  const effortLabel = $derived(t(`chat.effort.${reasoningEffort}`))
  const effortIndex = $derived(
    reasoningEffort === 'default' ? Math.floor(allowedEfforts.length / 2) : allowedEfforts.indexOf(reasoningEffort),
  )
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
  function commitReasoning(event: Event) {
    const effort = allowedEfforts[Number((event.currentTarget as HTMLInputElement).value)]
    if (effort && !session?.running) reasoningEffort = effort
  }
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
      session.show(id, conversation.turns, conversation.summary.selectedTurnId)
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

  const groups = $derived.by(() => {
    const grouped = new SvelteMap<string, ChatTurn[]>()
    for (const turn of session?.turns ?? []) {
      const replies = grouped.get(turn.questionId)
      if (replies) replies.push(turn)
      else grouped.set(turn.questionId, [turn])
    }
    return [...grouped.entries()]
  })
  const actionBusy = $derived(historyBusy || session?.running === true || restoreBlocked)
  let replyDialog = $state<'edit' | 'model' | 'translate' | null>(null)
  let replyOpen = $state(false)
  let sourceReply = $state<ChatTurn | null>(null)
  let editDraft = $state('')
  let expectedText = ''
  let targetLanguage = $state('')
  let replyModel = $state<ModelRef | null>(null)
  let replyError = $state<string | null>(null)
  const validReplyModel = $derived(
    modelOptions?.some(
      (option) => option.endpointId === replyModel?.endpointId && option.modelId === replyModel?.modelId,
    ) === true,
  )
  function sourceModel(turn: ChatTurn): ModelRef | null {
    return turn.endpointId && turn.modelId ? { endpointId: turn.endpointId, modelId: turn.modelId } : null
  }
  function sourceLabel(turn: ChatTurn) {
    return (
      modelOptions?.find((option) => option.endpointId === turn.endpointId && option.modelId === turn.modelId)?.label ??
      (turn.modelId ? `${turn.endpointId ?? t('chat.unknown')} · ${turn.modelId}` : t('chat.noSourceModel'))
    )
  }
  function hasChildren(turn: ChatTurn) {
    return session?.turns.some((child) => child.parentId === turn.id || child.supersedesId === turn.id) === true
  }
  function canEdit(turn: ChatTurn) {
    return turn.status !== 'running' && !hasChildren(turn)
  }
  function canDelete(turn: ChatTurn) {
    return canEdit(turn) && session?.selectedTurnId !== turn.id
  }
  function deletionReason(turn: ChatTurn) {
    return session?.selectedTurnId === turn.id
      ? t('chat.deleteSelectedDisabled')
      : hasChildren(turn)
        ? t('chat.deleteChildrenDisabled')
        : undefined
  }
  function editReply(turn: ChatTurn) {
    sourceReply = turn
    editDraft = turn.text
    expectedText = turn.text
    replyError = null
    replyDialog = 'edit'
    replyOpen = true
  }
  function generationDialog(turn: ChatTurn, kind: 'model' | 'translate') {
    sourceReply = turn
    replyModel = sourceModel(turn)
    targetLanguage = i18n.locale
    replyError = null
    replyDialog = kind
    replyOpen = true
  }
  function showConversation(conversation: Conversation) {
    session?.show(conversation.summary.id, conversation.turns, conversation.summary.selectedTurnId)
    shownSummary = conversation.summary
    follow = true
  }
  async function mutateReply(turn: ChatTurn, action: 'select' | 'remove' | 'branch' | 'copy') {
    const id = session?.conversationId
    if (!history || !session || !id || actionBusy) return
    historyBusy = true
    try {
      if (action === 'remove') {
        if (!canDelete(turn)) return
        const siblings = session.turns.filter((candidate) => candidate.questionId === turn.questionId).length > 1
        if (
          !(await notify.confirm({
            title: t('chat.deleteReplyTitle'),
            body: t(siblings ? 'chat.deleteCandidateBody' : 'chat.deleteOnlyReplyBody'),
            danger: true,
          }))
        )
          return
      }
      const conversation =
        action === 'select' || action === 'branch'
          ? await history.selectTurn(id, turn.id)
          : action === 'remove'
            ? await history.removeTurn(id, turn.id)
            : await history.fork(id, turn.id)
      showConversation(conversation)
      if (action === 'branch') {
        draft = ''
        draftRevision++
        recovery = null
      }
      historyError = null
      void refresh()
    } catch (error) {
      notify.toast('error', errorText(error))
    } finally {
      historyBusy = false
    }
  }
  async function retryReply(turn: ChatTurn) {
    if (!session || actionBusy) return
    const source = sourceModel(turn)
    if (history && turn.persisted) {
      if (modelOptions === null || modelError) {
        generationDialog(turn, 'model')
        return
      }
      if (
        !source ||
        !modelOptions.some((option) => option.endpointId === source.endpointId && option.modelId === source.modelId)
      ) {
        generationDialog(turn, 'model')
        replyError = t('chat.sourceModelUnavailable')
        return
      }
    }
    follow = true
    await session.retry(base, turn.id)
  }
  async function submitReply() {
    const turn = sourceReply
    if (!session || !turn || actionBusy) return
    replyError = null
    if (replyDialog === 'edit') {
      const id = session.conversationId
      if (!history || !id || !editDraft.trim() || editDraft.length > 24_000 || !canEdit(turn)) return
      historyBusy = true
      try {
        showConversation(await history.editTurn(id, turn.id, editDraft, expectedText))
        replyOpen = false
      } catch (error) {
        replyError = errorText(error)
      } finally {
        historyBusy = false
      }
    } else {
      if (!validReplyModel || !replyModel) return
      if (replyDialog === 'translate' && (!targetLanguage.trim() || targetLanguage.trim().length > 80)) return
      const accepted =
        replyDialog === 'translate'
          ? await session.translate({ ...base, model: replyModel }, turn.id, targetLanguage.trim())
          : await session.retry(base, turn.id, replyModel)
      if (accepted) replyOpen = false
      else replyError = session.actionError ? t(`errors.${session.actionError}`) : t('errors.internal')
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
    if (reasoningOpen || historyOpen || document.querySelector('[data-slot="select-content"], [role="dialog"]')) return
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
    {#if session.actionError}<p role="alert" class="text-destructive text-kit-sm">
        {t(`errors.${session.actionError}`)}
      </p>{/if}
    {#if session.historyRefreshError}<p role="alert" class="text-destructive text-kit-sm">
        {t('chat.replyRefreshFailed')}<Button
          variant="outline"
          size="sm"
          disabled={actionBusy}
          onclick={() => session.conversationId && void open(session.conversationId)}>{t('chat.retryHistory')}</Button
        >
      </p>{/if}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (native scroll region needs keyboard focus for PageUp/PageDown) -->
    <div class="messages" bind:this={scroller} {onscroll} role="region" aria-label={t('chat.messages')} tabindex="0">
      {#if session.turns.length > 0}
        <ol class="turns">
          {#each groups as [questionId, replies] (questionId)}
            {#each replies as turn, i (turn.id)}
              <MessageItem
                {turn}
                {onInsert}
                {onSaveNote}
                {onExportMarkdown}
                modelLabel={turn.kind === 'translation'
                  ? `${t('chat.translationReply')} · ${sourceLabel(turn)}`
                  : sourceLabel(turn)}
                showInput={i === 0 && turn.kind !== 'translation'}
                chosen={session.selectedTurnId === turn.id}
                busy={actionBusy}
                editable={canEdit(turn)}
                deletable={canDelete(turn)}
                deleteReason={deletionReason(turn)}
                onRetry={purpose === 'chat' && !restoreBlocked && (turn.persisted || capability !== null)
                  ? () => void retryReply(turn)
                  : undefined}
                onEdit={purpose === 'chat' && history && session.conversationId && turn.persisted
                  ? () => editReply(turn)
                  : undefined}
                onBranch={purpose === 'chat' && history && session.conversationId && turn.persisted
                  ? () => void mutateReply(turn, 'branch')
                  : undefined}
                onCopyConversation={purpose === 'chat' && history && session.conversationId && turn.persisted
                  ? () => void mutateReply(turn, 'copy')
                  : undefined}
                onSwitchModel={purpose === 'chat' && history && session.conversationId && turn.persisted
                  ? () => generationDialog(turn, 'model')
                  : undefined}
                onTranslate={purpose === 'chat' && history && session.conversationId && turn.persisted
                  ? () => generationDialog(turn, 'translate')
                  : undefined}
                onDelete={purpose === 'chat' && history && session.conversationId && turn.persisted
                  ? () => void mutateReply(turn, 'remove')
                  : undefined}
                onUseContext={purpose === 'chat' && history && session.conversationId && turn.persisted
                  ? () => void mutateReply(turn, 'select')
                  : undefined}
                onApprove={(tool, allow) => session.approve(tool, allow)}
              />
            {/each}
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
      <div class="expand-control">
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
      </div>
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
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            disabled={session.running || historyBusy}
            aria-label={t('chat.newConversation')}
            title={t('chat.newConversation')}
            onclick={newConversation}><PlusIcon /></Button
          >
          {#if ports.search && purpose === 'chat'}<Button
              type="button"
              variant="ghost"
              class="search-toggle"
              size="icon-sm"
              aria-label={t('chat.network')}
              aria-pressed={webSearch}
              disabled={session.running}
              title={`${t(webSearch ? 'chat.networkOn' : 'chat.networkOff')} · ${t('chat.networkPrivacy')}`}
              onclick={() => (webSearch = !webSearch)}
              ><GlobeIcon
                >{#if !webSearch}<path d="m2 2 20 20" />{/if}</GlobeIcon
              ></Button
            >{/if}
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
          {#if picked}
            <Popover.Root bind:open={reasoningOpen}>
              <Popover.Trigger disabled={session.running}>
                {#snippet child({ props })}
                  <Button
                    {...props}
                    bind:ref={reasoningTrigger}
                    variant="ghost"
                    size="sm"
                    aria-label={`${t('chat.reasoningEffort')}: ${effortLabel}`}
                  >
                    <GaugeIcon data-icon="inline-start" />
                    {effortLabel}
                    <ChevronDownIcon data-icon="inline-end" />
                  </Button>
                {/snippet}
              </Popover.Trigger>
              <Popover.Content
                class="reasoning-popover"
                side="top"
                align="end"
                sideOffset={8}
                collisionPadding={8}
                trapFocus={false}
                preventScroll={false}
                aria-label={t('chat.reasoningEffort')}
                onEscapeKeydown={(event) => {
                  if (event.isComposing) event.preventDefault()
                  event.stopPropagation()
                }}
                onCloseAutoFocus={(event) => {
                  event.preventDefault()
                  reasoningTrigger?.focus({ preventScroll: true })
                }}
              >
                <div class="reasoning-heading">
                  <p>{t('chat.reasoningStrength')}: <strong>{effortLabel}</strong></p>
                  <Button
                    variant="secondary"
                    size="xs"
                    disabled={session.running || reasoningEffort === 'default'}
                    onclick={() => (reasoningEffort = 'default')}>{t('chat.resetReasoning')}</Button
                  >
                </div>
                {#if allowedEfforts.length === 0}
                  <p class="reasoning-hint">{t('chat.reasoningUnconfigured')}</p>
                  {#if onOpenSettings}<Button
                      variant="outline"
                      size="sm"
                      onclick={() => {
                        reasoningOpen = false
                        onOpenSettings()
                      }}>{t('chat.openSettings')}</Button
                    >{/if}
                {:else if allowedEfforts.length > 1}
                  <div class="reasoning-scale" aria-hidden="true">
                    <span>{t('chat.reasoningFaster')}</span><span>{t('chat.reasoningSmarter')}</span>
                  </div>
                  <div
                    class="reasoning-slider"
                    style:--reasoning-progress={`${(effortIndex / (allowedEfforts.length - 1)) * 100}%`}
                  >
                    <input
                      type="range"
                      min="0"
                      max={allowedEfforts.length - 1}
                      step="1"
                      value={effortIndex}
                      aria-label={t('chat.reasoningEffort')}
                      aria-valuetext={effortLabel}
                      disabled={session.running}
                      oninput={commitReasoning}
                      onpointerup={(event) => {
                        if (event.button === 0) commitReasoning(event)
                      }}
                      onkeyup={(event) => {
                        if (
                          !event.isComposing &&
                          ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)
                        ) {
                          commitReasoning(event)
                        }
                      }}
                    />
                    <div class="reasoning-ticks" aria-hidden="true">
                      {#each allowedEfforts as effort, index (effort)}
                        <span
                          class="reasoning-tick"
                          data-effort={effort}
                          class:current={index === effortIndex}
                          style:left={`${(index / (allowedEfforts.length - 1)) * 100}%`}
                        ></span>
                      {/each}
                    </div>
                  </div>
                {:else}
                  <Field.FieldSet disabled={session.running}>
                    <Field.FieldLegend class="sr-only">{t('chat.reasoningEffort')}</Field.FieldLegend>
                    {#each ['default', ...allowedEfforts] as effort (effort)}
                      <label class="reasoning-option">
                        <input
                          type="radio"
                          name={reasoningId}
                          value={effort}
                          checked={reasoningEffort === effort}
                          onchange={() => (reasoningEffort = effort as ReasoningEffort)}
                        />
                        {t(`chat.effort.${effort}`)}
                      </label>
                    {/each}
                  </Field.FieldSet>
                {/if}
                {#if reasoningEffort === 'default'}<p class="reasoning-hint">{t('chat.reasoningDefaultHint')}</p>{/if}
              </Popover.Content>
            </Popover.Root>
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
    <Dialog.Root bind:open={replyOpen}>
      <Dialog.Content class="reply-dialog" showCloseButton={false}>
        <Dialog.Header>
          <Dialog.Title
            >{t(
              replyDialog === 'edit'
                ? 'chat.editReply'
                : replyDialog === 'translate'
                  ? 'chat.translate'
                  : 'chat.switchModel',
            )}</Dialog.Title
          >
          <Dialog.Description
            >{t(replyDialog === 'edit' ? 'chat.editReplyHint' : 'chat.candidateHint')}</Dialog.Description
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
                ><Field.FieldLabel for="chat-reply-edit">{t('chat.replyText')}</Field.FieldLabel><Textarea
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
                  {modelError}<Button type="button" variant="outline" size="sm" onclick={() => modelReload++}
                    >{t('chat.retryModels')}</Button
                  >
                </p>{/if}
              {#if replyDialog === 'translate'}<Field.Field
                  ><Field.FieldLabel for="chat-target-language">{t('chat.targetLanguage')}</Field.FieldLabel><Input
                    id="chat-target-language"
                    bind:value={targetLanguage}
                    maxlength={80}
                    disabled={actionBusy}
                  /></Field.Field
                >{/if}
            {/if}
            {#if replyError}<p role="alert" class="text-destructive">{replyError}</p>{/if}
            <Dialog.Footer>
              <Button type="button" variant="ghost" disabled={actionBusy} onclick={() => (replyOpen = false)}
                >{t('common.cancel')}</Button
              >
              <Button
                type="submit"
                disabled={actionBusy ||
                  (replyDialog === 'edit'
                    ? !editDraft.trim() || editDraft.length > 24_000
                    : !validReplyModel || (replyDialog === 'translate' && !targetLanguage.trim()))}
                >{t('common.confirm')}</Button
              >
            </Dialog.Footer>
          </Field.FieldGroup>
        </form>
      </Dialog.Content>
    </Dialog.Root>
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
    position: relative;
    flex: none;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    border: 1px solid var(--color-border);
    border-radius: calc(var(--kit-radius, var(--radius)) * 1.5);
    padding: 0.5rem;
  }
  .expand-control {
    position: absolute;
    top: 0.375rem;
    right: 0.375rem;
  }
  .composer :global(textarea) {
    min-height: 5rem;
    height: 5rem;
    max-height: 5rem;
    field-sizing: fixed;
    resize: none;
    padding-right: 2.25rem;
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
    max-width: calc(100% - 2rem);
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
  .chat-panel :global(button.search-toggle) {
    color: var(--color-muted-foreground);
  }
  .chat-panel :global(button.search-toggle[aria-pressed='true']) {
    color: var(--color-primary-foreground);
    background: var(--color-primary);
    border-color: var(--color-primary);
  }
  .chat-panel :global(button.search-toggle[aria-pressed='true']:hover:not(:disabled)) {
    background: color-mix(in oklch, var(--color-primary) 85%, var(--color-foreground));
  }
  :global(.reasoning-popover) {
    box-sizing: border-box;
    width: min(18rem, calc(100vw - 1rem));
    max-height: calc(100dvh - 1rem);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 0.75rem;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
    background: var(--color-popover);
    color: var(--color-popover-foreground);
    box-shadow: 0 4px 12px color-mix(in oklch, var(--color-foreground) 12%, transparent);
    font-size: var(--kit-font-size-sm, 0.8125rem);
  }
  .reasoning-heading,
  .reasoning-scale {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
  }
  .reasoning-heading p,
  .reasoning-hint {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .reasoning-scale,
  .reasoning-hint {
    color: var(--color-muted-foreground);
  }
  :global(.reasoning-popover input) {
    accent-color: var(--color-primary);
  }
  .reasoning-slider {
    position: relative;
    height: 1.75rem;
    display: flex;
    align-items: center;
  }
  :global(.reasoning-slider input[type='range']) {
    appearance: none;
    width: 100%;
    height: 1.75rem;
    margin: 0;
    background: transparent;
    cursor: pointer;
  }
  :global(.reasoning-slider input::-webkit-slider-runnable-track) {
    height: 0.375rem;
    border-radius: 999px;
    background: linear-gradient(
      to right,
      var(--color-primary) var(--reasoning-progress),
      var(--color-muted) var(--reasoning-progress)
    );
  }
  :global(.reasoning-slider input::-moz-range-track) {
    height: 0.375rem;
    border-radius: 999px;
    background: linear-gradient(
      to right,
      var(--color-primary) var(--reasoning-progress),
      var(--color-muted) var(--reasoning-progress)
    );
  }
  :global(.reasoning-slider input::-webkit-slider-thumb) {
    appearance: none;
    width: 1.125rem;
    height: 1.125rem;
    margin-top: -0.375rem;
    border: 1px solid var(--color-border);
    border-radius: 50%;
    background: var(--color-popover);
  }
  :global(.reasoning-slider input::-moz-range-thumb) {
    width: 1.125rem;
    height: 1.125rem;
    box-sizing: border-box;
    border: 1px solid var(--color-border);
    border-radius: 50%;
    background: var(--color-popover);
  }
  .reasoning-ticks {
    position: absolute;
    inset: 50% 0.5625rem auto;
    pointer-events: none;
  }
  .reasoning-tick {
    position: absolute;
    width: 0.25rem;
    height: 0.25rem;
    border-radius: 50%;
    background: var(--color-popover);
    transform: translate(-50%, -50%);
  }
  .reasoning-tick.current {
    visibility: hidden;
  }
  :global(.reasoning-slider input:disabled) {
    cursor: not-allowed;
    opacity: 0.5;
  }
  :global(.reasoning-popover input:focus-visible) {
    outline: 2px solid var(--color-ring);
    outline-offset: 2px;
  }
  .reasoning-option {
    display: flex;
    align-items: center;
    gap: 0.5rem;
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
