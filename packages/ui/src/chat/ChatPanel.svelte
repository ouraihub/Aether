<script lang="ts">
  import { onDestroy, tick, untrack } from 'svelte'
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
  import PaperclipIcon from '@lucide/svelte/icons/paperclip'
  import MessageSquarePlusIcon from '@lucide/svelte/icons/message-square-plus'
  import ImageIcon from '@lucide/svelte/icons/image'
  import CheckIcon from '@lucide/svelte/icons/check'
  import GripVerticalIcon from '@lucide/svelte/icons/grip-vertical'
  import Settings2Icon from '@lucide/svelte/icons/settings-2'
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw'
  import { Switch } from '#lib/components/ui/switch/index.js'
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
    type ImagePart,
    type ImageRef,
    type ReasoningEffort,
  } from '../ports'
  import ModelSelect from '../providers/ModelSelect.svelte'
  import { providersRevision } from '../providers/revision.svelte'
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
    paintingModel = false,
    toolbar: toolbarProp,
    onToolbarChange,
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
    onSaveNote?: (markdown: string, images: ImageRef[]) => Promise<void> | void
    onExportMarkdown?: (markdown: string) => Promise<void> | void
    onOpenSettings?: () => void
    /** A failed restore stays visible until retried or explicitly replaced with a new conversation. */
    conversationId?: string | null
    onConversationChange?: (id: string | null) => void
    onClose?: () => void
    /** Whether the host has a painting model bound; gates “Generate image” (docs/20 §1). */
    paintingModel?: boolean
    /** Ordered ids of composer tools pinned to the toolbar (docs/38); omitted means the default. */
    toolbar?: string[]
    onToolbarChange?: (ids: string[]) => void
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
    void providersRevision.value
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
  let generateImage = $state(false)
  const base = $derived({
    purpose,
    scope: scope ?? null,
    language: i18n.locale,
    model: picked,
    skills: selected,
    reasoningEffort,
    generateImage: purpose === 'chat' && generateImage && paintingModel,
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
  let replyDialog = $state<'edit' | 'translate' | null>(null)
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
  function editReply(turn: ChatTurn) {
    sourceReply = turn
    editDraft = turn.text
    expectedText = turn.text
    replyError = null
    replyDialog = 'edit'
    replyOpen = true
  }
  function translateDialog(turn: ChatTurn) {
    sourceReply = turn
    replyModel = sourceModel(turn)
    targetLanguage = i18n.locale
    replyError = null
    replyDialog = 'translate'
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
        if (turn.status === 'running') return
        const reparent = session.turns.some((child) => child.parentId === turn.id)
        if (
          !(await notify.confirm({
            title: t('chat.deleteReplyTitle'),
            body: reparent ? t('chat.deleteReparentBody') : '',
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
        discardRecovery()
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
        notify.toast('error', modelError ?? t('chat.modelsLoading'))
        return
      }
      if (
        !source ||
        !modelOptions.some((option) => option.endpointId === source.endpointId && option.modelId === source.modelId)
      ) {
        notify.toast('error', t('chat.sourceModelUnavailable'))
        return
      }
    }
    follow = true
    await session.retry(base, turn.id)
  }
  async function switchModel(turn: ChatTurn, model: ModelRef) {
    if (!session || actionBusy) return
    follow = true
    if (!(await session.retry(base, turn.id, model)))
      notify.toast('error', t(`errors.${session.actionError ?? 'internal'}`))
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
      if (!targetLanguage.trim() || targetLanguage.trim().length > 80) return
      const accepted = await session.translate({ ...base, model: replyModel }, turn.id, targetLanguage.trim())
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
    // Popover content renders inline, so Escape inside it bubbles here; let the layer close itself.
    if (
      reasoningOpen ||
      historyOpen ||
      document.querySelector('[data-popover-content], [data-slot="select-content"], [role="dialog"]')
    )
      return
    if (onClose && !session?.running) onClose()
  }
  type Attached = ImagePart & { preview: string }
  const IMAGE_MIMES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif']
  const MAX_IMAGES = 4
  const MAX_IMAGE_BYTES = 4 * 1024 * 1024
  let images = $state<Attached[]>([])
  let reading = $state(0)
  let fileInput = $state<HTMLInputElement | null>(null)
  let toolsOpen = $state(false)
  const vision = $derived(capability?.canVision === true)
  const DEFAULT_TOOLBAR = ['new-conversation', 'web-search']
  // svelte-ignore state_referenced_locally
  let pinned = $state<string[]>(toolbarProp ?? DEFAULT_TOOLBAR)
  let customizing = $state(false)
  /** Tools usable here; unavailable pinned ids stay stored but hidden (docs/38 §3.2). */
  const tools = $derived(
    ['new-conversation', 'web-search', 'attachment', 'generate-image'].filter(
      (id) =>
        (id !== 'web-search' || (ports.search && purpose === 'chat')) &&
        (id !== 'generate-image' || purpose === 'chat'),
    ),
  )
  const toolOrder = $derived([
    ...pinned.filter((id) => tools.includes(id)),
    ...tools.filter((id) => !pinned.includes(id)),
  ])
  function setPinned(next: string[]) {
    const all = [...next, ...pinned.filter((id) => !tools.includes(id))]
    if (all.join() === pinned.join()) return
    pinned = all
    onToolbarChange?.(all)
  }
  function moveTool(id: string, to: number) {
    const order = toolOrder.filter((item) => item !== id)
    order.splice(Math.max(0, Math.min(to, order.length)), 0, id)
    setPinned(order.filter((item) => pinned.includes(item)))
  }
  // Window listeners: keyed reordering moves the row node, which would drop pointer capture.
  function dragTool(event: PointerEvent, id: string) {
    const list = (event.currentTarget as HTMLElement).closest('ul')
    if (!list) return
    event.preventDefault()
    const move = (e: PointerEvent) => {
      const over = [...list.children].findIndex((row) => {
        const box = row.getBoundingClientRect()
        return e.clientY >= box.top && e.clientY < box.bottom
      })
      if (over >= 0 && toolOrder[over] !== id) moveTool(id, over)
    }
    const stop = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', stop)
      window.removeEventListener('pointercancel', stop)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', stop)
    window.addEventListener('pointercancel', stop)
  }
  const TOOL_LABEL: Record<string, string> = {
    'new-conversation': 'chat.newConversation',
    'web-search': 'chat.network',
    attachment: 'chat.uploadAttachment',
    'generate-image': 'chat.generateImage',
  }
  const canSend = $derived(
    session !== null &&
      !session.running &&
      restored &&
      !restoreBlocked &&
      !historyBusy &&
      capability !== null &&
      modelError === null &&
      draft.trim() !== '' &&
      (images.length === 0 || vision) &&
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
    if (!recovery || draft !== '' || images.length > 0) return
    draft = recovery.draft
    images = recovery.images
    draftRevision++
    if (contextKey === recovery.contextKey) droppedKey = recovery.droppedKey
    contextRevision++
    recovery = null
  }
  async function send() {
    if (!session || !canSend) return
    const original = draft
    const attached = images
    const revision = draftRevision
    const contextVersion = contextRevision
    const removed = droppedKey
    const key = contextKey
    draft = ''
    images = []
    discardRecovery()
    follow = true
    const accepted = await session.send(
      base,
      original.trim(),
      webSearch,
      attached.map(({ mime, data }) => ({ mime, data })),
    )
    if (accepted) {
      for (const image of attached) URL.revokeObjectURL(image.preview)
      if (contextRevision === contextVersion && contextKey === key) droppedKey = null
    } else if (draftRevision === revision && images.length === 0) {
      draft = original
      images = attached
      if (contextRevision === contextVersion && contextKey === key) droppedKey = removed
    } else {
      recovery = { draft: original, images: attached, droppedKey: removed, contextKey: key }
    }
  }

  onDestroy(() => {
    for (const image of images) URL.revokeObjectURL(image.preview)
    discardRecovery()
  })
  function removeImage(index: number) {
    const [image] = images.splice(index, 1)
    if (image) URL.revokeObjectURL(image.preview)
  }
  function readBase64(file: File) {
    const { promise, resolve, reject } = Promise.withResolvers<string>()
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).slice(String(reader.result).indexOf(',') + 1))
    reader.onerror = () => reject(reader.error ?? new Error('read failed'))
    reader.readAsDataURL(file)
    return promise
  }
  /** Adds the image files to the tray, or rejects all of them with a toast. */
  async function attach(files: FileList | null | undefined) {
    const picked = [...(files ?? [])].filter((file) => file.type.startsWith('image/'))
    if (picked.length === 0) return
    if (!vision) notify.toast('error', t('chat.visionUnsupported'))
    else if (picked.some((file) => !IMAGE_MIMES.includes(file.type))) notify.toast('error', t('chat.imageType'))
    else if (picked.some((file) => file.size > MAX_IMAGE_BYTES)) notify.toast('error', t('chat.imageSize'))
    else if (images.length + picked.length > MAX_IMAGES)
      notify.toast('error', t('chat.imageLimit', { max: MAX_IMAGES }))
    else {
      reading++
      try {
        const read = await Promise.all(
          picked.map(async (file) => ({ mime: file.type, data: await readBase64(file), file })),
        )
        // Another add may have landed while reading.
        if (images.length + read.length > MAX_IMAGES) notify.toast('error', t('chat.imageLimit', { max: MAX_IMAGES }))
        else images.push(...read.map(({ mime, data, file }) => ({ mime, data, preview: URL.createObjectURL(file) })))
      } catch {
        notify.toast('error', t('errors.internal'))
      } finally {
        reading--
      }
    }
  }
  function hasImages(files: FileList | null | undefined) {
    return [...(files ?? [])].some((file) => file.type.startsWith('image/'))
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
          {...onOpenSettings ? { onOpenSettings } : {}}
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
            {@const candidates = replies.filter((reply) => reply.kind === 'reply').length > 1}
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
                chosen={candidates && session.selectedTurnId === turn.id}
                busy={actionBusy}
                editable={canEdit(turn)}
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
                  ? (picked: ModelRef) => void switchModel(turn, picked)
                  : undefined}
                modelOptions={turn.images.length > 0
                  ? (modelOptions?.filter((option) => option.canVision) ?? null)
                  : modelOptions}
                sourceModel={sourceModel(turn)}
                onTranslate={purpose === 'chat' && history && session.conversationId && turn.persisted
                  ? () => translateDialog(turn)
                  : undefined}
                onDelete={purpose === 'chat' && history && session.conversationId && turn.persisted
                  ? () => void mutateReply(turn, 'remove')
                  : undefined}
                onUseContext={candidates && purpose === 'chat' && history && session.conversationId && turn.persisted
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
      ondragover={(e) => {
        if (e.dataTransfer?.types.includes('Files')) e.preventDefault()
      }}
      ondrop={(e) => {
        if (!hasImages(e.dataTransfer?.files)) return
        e.preventDefault()
        void attach(e.dataTransfer?.files)
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
      {#if images.length > 0}
        <ul class="image-tray">
          {#each images as image, index (image.preview)}
            <li>
              <img src={image.preview} alt="" /><Button
                type="button"
                variant="secondary"
                size="icon-xs"
                aria-label={t('chat.removeImage', { index: index + 1 })}
                title={t('chat.removeImage', { index: index + 1 })}
                onclick={() => removeImage(index)}><XIcon /></Button
              >
            </li>
          {/each}
        </ul>
        {#if !vision}<p class="search-status" role="status">{t('chat.visionUnsupported')}</p>{/if}
      {/if}
      <Textarea
        bind:value={draft}
        rows={3}
        aria-label={t('chat.input')}
        placeholder={t('chat.placeholder')}
        oninput={() => draftRevision++}
        onpaste={(e) => {
          const files = e.clipboardData?.files
          if (!hasImages(files)) return
          e.preventDefault()
          void attach(files)
        }}
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
            disabled={draft !== '' || images.length > 0}
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
          {#snippet toolIcon(id: string)}
            {#if id === 'new-conversation'}<MessageSquarePlusIcon />{:else if id === 'web-search'}<GlobeIcon
                >{#if !webSearch}<path d="m2 2 20 20" />{/if}</GlobeIcon
              >{:else if id === 'attachment'}<PaperclipIcon />{:else}<ImageIcon />{/if}
          {/snippet}
          {#each pinned.filter((id) => tools.includes(id)) as id (id)}
            {#if id === 'new-conversation'}<Button
                type="button"
                variant="ghost"
                size="icon-sm"
                disabled={session.running || historyBusy}
                aria-label={t('chat.newConversation')}
                title={t('chat.newConversation')}
                onclick={newConversation}>{@render toolIcon(id)}</Button
              >{:else if id === 'web-search'}<Button
                type="button"
                variant="ghost"
                class="search-toggle"
                size="icon-sm"
                aria-label={t('chat.network')}
                aria-pressed={webSearch}
                disabled={session.running}
                title={`${t(webSearch ? 'chat.networkOn' : 'chat.networkOff')} · ${t('chat.networkPrivacy')}`}
                onclick={() => (webSearch = !webSearch)}>{@render toolIcon(id)}</Button
              >{:else if id === 'attachment'}<Button
                type="button"
                variant="ghost"
                size="icon-sm"
                aria-label={t('chat.uploadAttachment')}
                disabled={session.running || !vision}
                title={vision ? t('chat.uploadAttachment') : t('chat.visionUnsupported')}
                onclick={() => fileInput?.click()}>{@render toolIcon(id)}</Button
              >{:else}<Button
                type="button"
                variant="ghost"
                class="search-toggle"
                size="icon-sm"
                aria-label={t('chat.generateImage')}
                aria-pressed={generateImage}
                disabled={session.running || !paintingModel || capability?.supportsTools === false}
                title={!paintingModel
                  ? t('chat.paintingUnconfigured')
                  : capability?.supportsTools === false
                    ? t('chat.toolsUnsupported')
                    : t('chat.generateImage')}
                onclick={() => (generateImage = !generateImage)}>{@render toolIcon(id)}</Button
              >{/if}
          {/each}
          <input
            bind:this={fileInput}
            type="file"
            accept={IMAGE_MIMES.join(',')}
            multiple
            hidden
            onchange={(e) => {
              const input = e.currentTarget
              void attach(input.files).finally(() => (input.value = ''))
            }}
          />
          <Popover.Root bind:open={toolsOpen} onOpenChange={(open) => !open && (customizing = false)}>
            <Popover.Trigger disabled={session.running}>
              {#snippet child({ props })}<Button
                  {...props}
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label={t('chat.tools')}
                  title={t('chat.tools')}><PlusIcon /></Button
                >{/snippet}
            </Popover.Trigger>
            <Popover.Content class="tools-menu" side="top" align="start" sideOffset={6} collisionPadding={8}>
              {#if customizing}
                <p class="tools-title" id="{reasoningId}-toolbar">{t('chat.toolbarCustomize')}</p>
                <ul class="tools-order" aria-labelledby="{reasoningId}-toolbar">
                  {#each toolOrder as id, index (id)}
                    <li class="tools-row">
                      <button
                        type="button"
                        class="tools-grip"
                        aria-label={t('chat.toolbarDrag', { name: t(TOOL_LABEL[id] ?? id) })}
                        onpointerdown={(e) => dragTool(e, id)}
                        onkeydown={(e) => {
                          if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
                          e.preventDefault()
                          const button = e.currentTarget
                          moveTool(id, index + (e.key === 'ArrowUp' ? -1 : 1))
                          void tick().then(() => button.focus())
                        }}><GripVerticalIcon /></button
                      >
                      {@render toolIcon(id)}<span>{t(TOOL_LABEL[id] ?? id)}</span>
                      <Switch
                        size="sm"
                        aria-label={t(TOOL_LABEL[id] ?? id)}
                        checked={pinned.includes(id)}
                        onCheckedChange={(on) =>
                          setPinned(toolOrder.filter((item) => (item === id ? on : pinned.includes(item))))}
                      />
                    </li>
                  {/each}
                </ul>
                <button
                  type="button"
                  class="tools-item tools-footer"
                  disabled={pinned.join() === DEFAULT_TOOLBAR.join()}
                  onclick={() => {
                    pinned = [...DEFAULT_TOOLBAR]
                    onToolbarChange?.(pinned)
                  }}><RotateCcwIcon /><span>{t('chat.toolbarRestore')}</span></button
                >
              {:else}
                {#each toolOrder.filter((id) => !pinned.includes(id)) as id (id)}
                  {#if id === 'new-conversation'}<button
                      type="button"
                      class="tools-item"
                      disabled={historyBusy}
                      onclick={() => {
                        toolsOpen = false
                        newConversation()
                      }}>{@render toolIcon(id)}<span>{t('chat.newConversation')}</span></button
                    >{:else if id === 'web-search'}<button
                      type="button"
                      class="tools-item"
                      aria-pressed={webSearch}
                      onclick={() => {
                        toolsOpen = false
                        webSearch = !webSearch
                      }}
                      >{@render toolIcon(id)}<span>{t('chat.network')}</span>{#if webSearch}<CheckIcon />{/if}</button
                    >{:else if id === 'attachment'}<button
                      type="button"
                      class="tools-item"
                      disabled={!vision}
                      title={vision ? undefined : t('chat.visionUnsupported')}
                      onclick={() => {
                        toolsOpen = false
                        fileInput?.click()
                      }}
                      >{@render toolIcon(id)}<span>{t('chat.uploadAttachment')}</span>{#if !vision}<small
                          >{t('chat.visionUnsupported')}</small
                        >{/if}</button
                    >{:else}<button
                      type="button"
                      class="tools-item"
                      aria-pressed={generateImage}
                      disabled={!paintingModel || capability?.supportsTools === false}
                      title={!paintingModel
                        ? t('chat.paintingUnconfigured')
                        : capability?.supportsTools === false
                          ? t('chat.toolsUnsupported')
                          : undefined}
                      onclick={() => {
                        toolsOpen = false
                        generateImage = !generateImage
                      }}
                      >{@render toolIcon(id)}<span>{t('chat.generateImage')}</span>{#if !paintingModel}<small
                          >{t('chat.paintingUnconfigured')}</small
                        >{:else if generateImage}<CheckIcon />{/if}</button
                    >{/if}
                {/each}
                <button
                  type="button"
                  class="tools-item tools-footer"
                  aria-label={t('chat.toolbarCustomize')}
                  onclick={() => (customizing = true)}><span>{t('chat.quickPanel')}</span><Settings2Icon /></button
                >
              {/if}
            </Popover.Content>
          </Popover.Root>
          {#if generateImage && paintingModel && purpose === 'chat' && !pinned.includes('generate-image')}<Button
              type="button"
              variant="secondary"
              size="sm"
              class="tool-chip"
              aria-label={t('chat.generateImageOff')}
              title={t('chat.generateImageOff')}
              onclick={() => (generateImage = false)}><ImageIcon />{t('chat.generateImage')}<XIcon /></Button
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
          <Dialog.Title>{t(replyDialog === 'edit' ? 'chat.editReply' : 'chat.translate')}</Dialog.Title>
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
              <Field.Field
                ><Field.FieldLabel for="chat-target-language">{t('chat.targetLanguage')}</Field.FieldLabel><Input
                  id="chat-target-language"
                  bind:value={targetLanguage}
                  maxlength={80}
                  disabled={actionBusy}
                /></Field.Field
              >
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
                    : !validReplyModel || !targetLanguage.trim())}>{t('common.confirm')}</Button
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
  .image-tray {
    display: flex;
    flex-wrap: wrap;
    gap: 0.375rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .image-tray li {
    position: relative;
  }
  .image-tray img {
    width: 3.5rem;
    height: 3.5rem;
    object-fit: cover;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
  }
  .image-tray :global(button) {
    position: absolute;
    top: -0.375rem;
    right: -0.375rem;
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
  :global(.tools-menu) {
    z-index: 50;
    box-sizing: border-box;
    width: min(16rem, calc(100vw - 1rem));
    display: flex;
    flex-direction: column;
    padding: 0.25rem;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
    background: var(--color-popover);
    color: var(--color-popover-foreground);
    box-shadow: 0 4px 12px color-mix(in oklch, var(--color-foreground) 12%, transparent);
    font-size: var(--kit-font-size-sm, 0.8125rem);
  }
  :global(.tools-item) {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-height: 2.25rem;
    padding: 0.25rem 0.625rem;
    border: 0;
    border-radius: calc(var(--kit-radius, var(--radius)) - 2px);
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  :global(.tools-item:hover:not(:disabled)),
  :global(.tools-item:focus-visible) {
    background: var(--color-accent);
    color: var(--color-accent-foreground);
    outline: none;
  }
  :global(.tools-item:disabled) {
    cursor: not-allowed;
    opacity: 0.55;
  }
  :global(.tools-item svg) {
    width: 1rem;
    height: 1rem;
    flex-shrink: 0;
  }
  :global(.tools-item small) {
    margin-left: auto;
    color: var(--color-muted-foreground);
    font-size: 0.75rem;
  }
  :global(.tools-item > svg:last-child:not(:first-child)) {
    margin-left: auto;
  }
  :global(.tools-footer) {
    margin-top: 0.25rem;
    border-top: 1px solid var(--color-border);
    border-radius: 0 0 calc(var(--kit-radius, var(--radius)) - 2px) calc(var(--kit-radius, var(--radius)) - 2px);
    color: var(--color-muted-foreground);
  }
  :global(.tools-title) {
    margin: 0;
    padding: 0.375rem 0.5rem;
    font-size: 0.875rem;
    font-weight: 500;
  }
  :global(.tools-order) {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  :global(.tools-row) {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-height: 2rem;
    padding: 0 0.375rem;
    border-radius: calc(var(--kit-radius, var(--radius)) - 2px);
    font-size: 0.875rem;
  }
  :global(.tools-row:hover) {
    background: var(--color-accent);
  }
  :global(.tools-row > svg) {
    width: 1rem;
    height: 1rem;
    flex-shrink: 0;
    color: var(--color-muted-foreground);
  }
  :global(.tools-row > span) {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  :global(.tools-grip) {
    display: inline-flex;
    padding: 0.125rem;
    border: 0;
    border-radius: 0.25rem;
    background: none;
    color: var(--color-muted-foreground);
    cursor: grab;
    touch-action: none;
  }
  :global(.tools-grip:focus-visible) {
    outline: 2px solid var(--color-ring);
  }
  :global(.tools-grip svg) {
    width: 0.875rem;
    height: 0.875rem;
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
