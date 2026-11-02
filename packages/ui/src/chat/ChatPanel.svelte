<script lang="ts">
  import { onDestroy, untrack } from 'svelte'
  import { getKitContext } from '../context'
  import {
    isKitClientError,
    type ModelRef,
    type SkillRef,
    type ModelOption,
    type ReasoningEffort,
    type PermissionMode,
  } from '../ports'
  import { providersRevision } from '../providers/revision.svelte'
  import { RunSession } from './run-session.svelte'
  import { ChatPanelHistoryState } from './chat-panel-history.svelte'
  import { ChatAttachments, hasImages, type AttachedImage } from './chat-attachments.svelte'
  import ChatSurface from './ChatSurface.svelte'
  import ChatPanelApprovals from './ChatPanelApprovals.svelte'
  import { nearestEffort, REASONING_EFFORTS } from '../providers/reasoning'
  import ChatPanelHeader from './ChatPanelHeader.svelte'
  import ChatPanelMessages from './ChatPanelMessages.svelte'
  import ChatPanelComposer from './ChatPanelComposer.svelte'
  import ChatPanelDialogs from './ChatPanelDialogs.svelte'
  import type { ChatPanelProps } from './chat-panel-props'

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
    paintingModel = false,
    toolbar: toolbarProp,
    onToolbarChange,
    reasoningEffort: reasoningEffortProp,
    onReasoningEffortChange,
    permissionModes = ['default', 'plan', 'accept_edits'],
    permissionMode = $bindable('default'),
  }: ChatPanelProps = $props()

  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const history = ports.conversations
  const session = ports.chat ? new RunSession(ports.chat, history) : null
  // svelte-ignore state_referenced_locally
  let picked = $state<ModelRef | null>(model)
  let modelOptions = $state<ModelOption[] | null>(null)
  let modelError = $state<string | null>(null)
  let modelReload = $state(0)
  const capability = $derived(
    modelOptions?.find(
      (item) => item.endpointId === picked?.endpointId && item.modelId === picked?.modelId,
    ) ?? null,
  )
  // The saved choice survives model switches; each model sends its nearest allowed effort (docs/28 RT3).
  // svelte-ignore state_referenced_locally
  let chosenEffort = $state<ReasoningEffort>(reasoningEffortProp ?? 'default')
  const allowedEfforts = $derived(
    REASONING_EFFORTS.filter((effort) => capability?.reasoning?.efforts.includes(effort)),
  )
  const reasoningEffort = $derived(nearestEffort(chosenEffort, allowedEfforts))
  function chooseEffort(effort: ReasoningEffort) {
    if (session?.running || effort === chosenEffort) return
    chosenEffort = effort
    onReasoningEffortChange?.(effort)
  }
  const MODES: PermissionMode[] = ['default', 'plan', 'accept_edits', 'auto', 'full_access']
  const isAgent = $derived(purpose === 'agent')
  const allowedModes: PermissionMode[] = $derived(
    isAgent ? MODES.filter((mode) => permissionModes.includes(mode)) : ['default'],
  )
  let fullAccess = $state(false)
  const mode: PermissionMode = $derived(
    fullAccess && allowedModes.includes('full_access')
      ? 'full_access'
      : permissionMode !== 'full_access' && allowedModes.includes(permissionMode)
        ? permissionMode
        : (allowedModes.find((option) => option !== 'full_access') ?? 'default'),
  )
  async function chooseMode(next: PermissionMode) {
    if (session?.running || next === mode || !allowedModes.includes(next)) return
    const generation = historyState.viewGeneration
    if (next === 'full_access') {
      if (
        !(await notify.confirm({
          title: t('chat.mode.fullAccessTitle'),
          body: t('chat.mode.fullAccessBody'),
          danger: true,
        }))
      )
        return
      if (
        !historyState.disposed &&
        generation === historyState.viewGeneration &&
        !session?.running &&
        allowedModes.includes(next)
      )
        fullAccess = true
    } else {
      fullAccess = false
      permissionMode = next
    }
  }
  let reasoningOpen = $state(false)
  const noModels = $derived(modelOptions !== null && modelOptions.length === 0)
  const errorText = (error: unknown) =>
    t(`errors.${isKitClientError(error) ? error.code : 'internal'}`)
  $effect(() => {
    void modelReload
    void providersRevision.value
    let alive = true
    // A provider save swaps options in place: nulling them would reset the chosen effort (docs/41 §3.5).
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
    // A model that turns out not to take tools cannot keep a pending generate_image request.
    if (capability?.supportsTools === false) generateImage = false
  })
  function pickModel(next: ModelRef) {
    picked = next
    session?.resetContext()
  }

  let draft = $state('')
  let draftRevision = 0
  let contextRevision = 0
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
  let generateImage = $state(false)
  const base = $derived({
    purpose,
    scope: scope ?? null,
    language: i18n.locale,
    model: picked,
    skills: isAgent ? selected : [],
    reasoningEffort,
    permissionMode: mode,
    generateImage: purpose === 'chat' && generateImage && paintingModel,
    context: activeContext ? { kind: activeContext.kind, id: activeContext.id } : null,
  })
  // Consume before awaiting run acceptance, including retries and translations. Never persist full access.
  function consumeBase() {
    const request = { ...base }
    fullAccess = false
    return request
  }

  let scrollRevision = $state(0)
  const historyState = new ChatPanelHistoryState(
    {
      history,
      getSession: () => session,
      getModelOptions: () => modelOptions,
      getModelError: () => modelError,
      setDraft: (value) => {
        draft = value
      },
      bumpDraftRevision: () => draftRevision++,
      setFullAccess: (value) => {
        fullAccess = value
      },
      getScope: () => scope,
      getLocale: () => i18n.locale,
      onConversationChange: (id) => onConversationChange?.(id),
      bumpScrollRevision: () => scrollRevision++,
      notify,
      t,
      errorText,
      consumeBase,
      discardRecovery: () => discardRecovery(),
    },
    untrack(() => conversationId),
  )
  $effect(() => () => historyState.dispose())
  $effect(() => {
    if (session && !session.running) void historyState.refresh()
  })
  $effect(() => historyState.syncConversation())
  void historyState.restore(untrack(() => conversationId))

  let followKey = $state(0)

  let contentRevision = 0
  const pendingApprovals = $derived(
    session?.running
      ? session.turns.flatMap((turn) => turn.tools.filter((tool) => tool.pending !== null))
      : [],
  )
  $effect.pre(() => {
    for (const turn of session?.turns ?? []) {
      void turn.text
      void turn.reasoningText
      void turn.status
      void turn.error
      void turn.usage
      void turn.search
      void turn.generated.length
      for (const tool of turn.tools) {
        void tool.pending
        void tool.summary
        void tool.outcome
      }
    }
    followKey = ++contentRevision
  })
  function onkeydown(e: KeyboardEvent) {
    if (e.key !== 'Escape' || e.defaultPrevented || e.isComposing) return
    // Popover content renders inline, so Escape inside it bubbles here; let the layer close itself.
    if (
      reasoningOpen ||
      historyState.historyOpen ||
      document.querySelector(
        '[data-popover-content], [data-slot="select-content"], [role="dialog"]',
      )
    )
      return
    if (onClose && !session?.running) onClose()
  }
  const attachments = new ChatAttachments(() => capability?.canVision === true, notify, i18n)
  type Attached = AttachedImage
  const currentContext = $derived(
    draft.trim() === '' &&
      attachments.images.length === 0 &&
      session?.contextUsage &&
      session.contextUsage.endpointId === picked?.endpointId &&
      session.contextUsage.modelId === picked?.modelId
      ? session.contextUsage
      : null,
  )
  const reading = $derived(attachments.reading)
  const vision = $derived(capability?.canVision === true)
  const canSend = $derived(
    session !== null &&
      !session.running &&
      historyState.restored &&
      !historyState.restoreBlocked &&
      !historyState.historyBusy &&
      capability !== null &&
      modelError === null &&
      draft.trim() !== '' &&
      (attachments.images.length === 0 || vision) &&
      reading === 0,
  )
  let recovery = $state<{
    draft: string
    images: Attached[]
    droppedKey: string | null
    contextKey: string | null
  } | null>(null)
  function discardRecovery() {
    for (const image of recovery?.images ?? []) URL.revokeObjectURL(image.preview)
    recovery = null
  }
  function recoverDraft() {
    if (!recovery || draft !== '' || attachments.images.length > 0) return
    draft = recovery.draft
    attachments.images = recovery.images
    draftRevision++
    if (contextKey === recovery.contextKey) droppedKey = recovery.droppedKey
    contextRevision++
    recovery = null
  }
  async function send() {
    if (!session || !canSend) return
    const generation = historyState.viewGeneration
    const original = draft
    const attached = attachments.images
    const revision = draftRevision
    const contextVersion = contextRevision
    const removed = droppedKey
    const key = contextKey
    draft = ''
    attachments.images = []
    discardRecovery()
    scrollRevision++
    const accepted = await session.send(
      consumeBase(),
      original.trim(),
      webSearch,
      attached.map(({ mime, data }) => ({ mime, data })),
    )
    if (historyState.disposed || generation !== historyState.viewGeneration) {
      for (const image of attached) URL.revokeObjectURL(image.preview)
      return
    }
    if (accepted) {
      for (const image of attached) URL.revokeObjectURL(image.preview)
      if (contextRevision === contextVersion && contextKey === key) droppedKey = null
    } else if (draftRevision === revision && attachments.images.length === 0) {
      draft = original
      attachments.images = attached
      if (contextRevision === contextVersion && contextKey === key) droppedKey = removed
    } else {
      recovery = { draft: original, images: attached, droppedKey: removed, contextKey: key }
    }
  }

  onDestroy(() => {
    for (const image of attachments.images) URL.revokeObjectURL(image.preview)
    discardRecovery()
  })
  const removeImage = attachments.remove
  const attach = attachments.attach
</script>

{#if session}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions (Esc bubbles from controls) -->
  <section
    class="chat-panel"
    aria-label={t('chat.title')}
    {onkeydown}
  >
    <ChatSurface
      label={t('chat.title')}
      messagesLabel={t('chat.messages')}
      latestLabel={t('chat.backToBottom')}
      {followKey}
      scrollKey={`${session.conversationId ?? ''}:${scrollRevision}`}
    >
      {#snippet header()}
        <ChatPanelHeader
          {i18n}
          title={historyState.title}
          {session}
          {history}
          historyBusy={historyState.historyBusy}
          historyOpen={historyState.historyOpen}
          historyError={historyState.historyError}
          {modelOptions}
          {modelError}
          onHistory={() => {
            historyState.historyOpen = true
            void historyState.refresh()
          }}
          {onClose}
          onRetryHistory={() => void historyState.retryHistory()}
          {picked}
          onPickModel={pickModel}
          {onOpenSettings}
        />
      {/snippet}
      <ChatPanelMessages
        {i18n}
        {session}
        groups={historyState.groups}
        {purpose}
        {history}
        restoreBlocked={historyState.restoreBlocked}
        actionBusy={historyState.actionBusy}
        {modelOptions}
        {modelError}
        {capability}
        {noModels}
        onRetryModels={() => modelReload++}
        {onOpenSettings}
        {onInsert}
        {onSaveNote}
        {onExportMarkdown}
        sourceLabel={historyState.sourceLabel}
        sourceModel={historyState.sourceModel}
        canEdit={historyState.canEdit}
        onRetry={(turn) => void historyState.retryReply(turn)}
        onEdit={historyState.editReply}
        onBranch={(turn) => void historyState.mutateReply(turn, 'branch')}
        onCopyConversation={(turn) => void historyState.mutateReply(turn, 'copy')}
        onSwitchModel={(turn, next) => void historyState.switchModel(turn, next)}
        onTranslate={historyState.translateDialog}
        onDelete={(turn) => void historyState.mutateReply(turn, 'remove')}
        onUseContext={(turn) => void historyState.mutateReply(turn, 'select')}
      />
      {#snippet composer()}
        <ChatPanelComposer
          {i18n}
          {session}
          {purpose}
          bind:draft
          {canSend}
          {activeContext}
          images={attachments.images}
          {vision}
          {recovery}
          bind:webSearch
          bind:generateImage
          toolbar={toolbarProp}
          {onToolbarChange}
          {paintingModel}
          {capability}
          historyBusy={historyState.historyBusy}
          onNewConversation={historyState.newConversation}
          {attach}
          onDraftInput={() => draftRevision++}
          onRemoveImage={removeImage}
          onRecoverDraft={recoverDraft}
          onDropContext={() => {
            droppedKey = contextKey
            contextRevision++
          }}
          hasSearch={!!ports.search}
          {selected}
          {skillsBudget}
          skillsPort={isAgent ? ports.skills : undefined}
          onSkillsChange={setSkills}
          onSkillResolved={() => (session.skillChanged = null)}
          {mode}
          {allowedModes}
          {chooseMode}
          {reasoningEffort}
          {allowedEfforts}
          bind:reasoningOpen
          {chooseEffort}
          {currentContext}
          {picked}
          {onOpenSettings}
          onSend={() => void send()}
          onStop={() => session.stop()}
          {hasImages}
          override={pendingApprovals.length ? approvals : undefined}
        />
      {/snippet}
    </ChatSurface>
    {#snippet approvals()}
      <ChatPanelApprovals {session} tools={pendingApprovals} {i18n} />
    {/snippet}
    <ChatPanelDialogs
      {i18n}
      {history}
      {session}
      conversations={historyState.conversations}
      title={historyState.title}
      historyLoading={historyState.historyLoading}
      historyBusy={historyState.historyBusy}
      historyError={historyState.historyError}
      retryHistory={historyState.retryHistory}
      open={historyState.open}
      rename={historyState.rename}
      remove={historyState.remove}
      actionBusy={historyState.actionBusy}
      replyDialog={historyState.replyDialog}
      {modelOptions}
      {modelError}
      {noModels}
      validReplyModel={historyState.validReplyModel}
      submitReply={historyState.submitReply}
      reloadModels={() => modelReload++}
      bind:historyOpen={historyState.historyOpen}
      bind:renaming={historyState.renaming}
      bind:renameTitle={historyState.renameTitle}
      bind:replyOpen={historyState.replyOpen}
      bind:editDraft={historyState.editDraft}
      bind:replyModel={historyState.replyModel}
      bind:targetLanguage={historyState.targetLanguage}
      replyError={historyState.replyError}
    />
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
</style>
