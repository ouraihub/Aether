import { SvelteMap } from 'svelte/reactivity'
import type { KitNotify } from '../context'
import type {
  Conversation,
  ConversationClient,
  ConversationSummary,
  ModelOption,
  ModelRef,
} from '../ports'
import type { ChatTurn, RunSession } from './run-session.svelte'

type Translate = (key: string, values?: Record<string, string | number>) => string

type HistoryDependencies = {
  history: ConversationClient | undefined
  getSession: () => RunSession | null
  getModelOptions: () => ModelOption[] | null
  getModelError: () => string | null
  setDraft: (value: string) => void
  bumpDraftRevision: () => void
  setFullAccess: (value: boolean) => void
  getScope: () => string | undefined
  getLocale: () => string
  onConversationChange?: ((id: string | null) => void) | undefined
  bumpScrollRevision: () => void
  notify: KitNotify
  t: Translate
  errorText: (error: unknown) => string
  consumeBase: () => Parameters<RunSession['translate']>[0]
  discardRecovery: () => void
}

export class ChatPanelHistoryState {
  readonly history: ConversationClient | undefined
  conversations = $state<ConversationSummary[]>([])
  shownSummary = $state<ConversationSummary | null>(null)
  historyOpen = $state(false)
  historyBusy = $state(false)
  historyLoading = $state(false)
  historyError = $state<string | null>(null)
  retryHistory = $state<() => Promise<void>>(async () => {
    await this.refresh()
  })
  renameTitle = $state('')
  renaming = $state(false)
  restored = $state(false)
  restoreBlocked = $state(false)
  replyDialog = $state<'edit' | 'translate' | null>(null)
  replyOpen = $state(false)
  sourceReply = $state<ChatTurn | null>(null)
  editDraft = $state('')
  expectedText = ''
  targetLanguage = $state('')
  replyModel = $state<ModelRef | null>(null)
  replyError = $state<string | null>(null)
  viewGeneration = $state(0)
  disposed = $state(false)
  #listGeneration = 0
  #reported: string | null
  readonly #deps: HistoryDependencies

  constructor(deps: HistoryDependencies, initialConversationId: string | null) {
    this.#deps = deps
    this.history = deps.history
    this.#reported = initialConversationId
    this.retryHistory = this.refresh
  }

  get title() {
    const session = this.#deps.getSession()
    return (
      (this.shownSummary?.id === session?.conversationId ? this.shownSummary?.title : null) ??
      this.conversations.find((conversation) => conversation.id === session?.conversationId)
        ?.title ??
      this.#deps.t('chat.newConversation')
    )
  }

  get groups() {
    const grouped = new SvelteMap<string, ChatTurn[]>()
    for (const turn of this.#deps.getSession()?.turns ?? []) {
      const replies = grouped.get(turn.questionId)
      if (replies) replies.push(turn)
      else grouped.set(turn.questionId, [turn])
    }
    return [...grouped.entries()]
  }

  get actionBusy() {
    return this.historyBusy || this.#deps.getSession()?.running === true || this.restoreBlocked
  }

  get validReplyModel() {
    const model = this.replyModel
    return (
      this.#deps
        .getModelOptions()
        ?.some(
          (option) => option.endpointId === model?.endpointId && option.modelId === model?.modelId,
        ) === true
    )
  }

  get isRestored() {
    return this.restored
  }

  dispose = () => {
    this.disposed = true
    this.viewGeneration++
    this.#listGeneration++
    this.#deps.setFullAccess(false)
    this.#deps.getSession()?.dispose()
  }

  syncConversation = () => {
    const session = this.#deps.getSession()
    const id = session?.conversationId ?? null
    if (!this.restored || this.restoreBlocked || id === this.#reported) return
    this.#reported = id
    this.#deps.onConversationChange?.(id)
  }

  refresh = async () => {
    if (!this.history) return
    const generation = ++this.#listGeneration
    this.historyLoading = true
    try {
      const next = await this.history.list(this.#deps.getScope())
      if (this.disposed || generation !== this.#listGeneration) return
      this.conversations = next
      if (this.retryHistory === this.refresh) this.historyError = null
    } catch (error) {
      if (!this.disposed && generation === this.#listGeneration && !this.restoreBlocked) {
        this.historyError = this.#deps.errorText(error)
        this.retryHistory = this.refresh
      }
    } finally {
      if (!this.disposed && generation === this.#listGeneration) this.historyLoading = false
    }
  }

  restore = async (id: string | null) => {
    if (id !== null && this.history && this.#deps.getSession()) {
      this.restoreBlocked = true
      await this.open(id)
    }
    this.restored = true
  }

  open = async (id: string) => {
    const session = this.#deps.getSession()
    if (!this.history || !session || session.running || this.historyBusy) return
    const generation = ++this.viewGeneration
    this.#deps.setFullAccess(false)
    this.historyBusy = true
    try {
      const conversation = await this.history.get(id)
      if (this.disposed || generation !== this.viewGeneration) return
      session.show(id, conversation.turns, conversation.summary.selectedTurnId)
      this.shownSummary = conversation.summary
      this.restoreBlocked = false
      this.restored = true
      this.historyError = null
      this.retryHistory = this.refresh
      this.historyOpen = false
      this.renaming = false
      this.#deps.bumpScrollRevision()
    } catch (error) {
      if (this.disposed || generation !== this.viewGeneration) return
      this.historyError = this.#deps.errorText(error)
      this.retryHistory = () => this.open(id)
    } finally {
      if (!this.disposed && generation === this.viewGeneration) this.historyBusy = false
    }
  }

  newConversation = () => {
    const session = this.#deps.getSession()
    if (!session || session.running || this.historyBusy) return
    this.viewGeneration++
    this.#deps.setFullAccess(false)
    session.show(null)
    this.shownSummary = null
    this.restoreBlocked = false
    this.restored = true
    this.historyError = null
    this.historyOpen = false
    this.renaming = false
    this.#deps.bumpScrollRevision()
  }

  rename = async () => {
    const session = this.#deps.getSession()
    const id = session?.conversationId
    const next = this.renameTitle.trim()
    if (!this.history || !id || !next || session?.running || this.historyBusy) return
    const generation = this.viewGeneration
    this.historyBusy = true
    try {
      const summary = await this.history.rename(id, next)
      if (this.disposed || generation !== this.viewGeneration) return
      this.shownSummary = summary
      this.conversations = this.conversations.map((item) => (item.id === id ? summary : item))
      this.renaming = false
      this.historyError = null
    } catch (error) {
      if (this.disposed || generation !== this.viewGeneration) return
      this.historyError = this.#deps.errorText(error)
      this.retryHistory = this.rename
    } finally {
      if (!this.disposed && generation === this.viewGeneration) this.historyBusy = false
    }
  }

  remove = async () => {
    const session = this.#deps.getSession()
    const id = session?.conversationId
    if (!this.history || !session || !id || session.running || this.historyBusy) return
    const generation = this.viewGeneration
    this.historyBusy = true
    try {
      const ok = await this.#deps.notify.confirm({
        title: this.#deps.t('chat.deleteTitle'),
        body: this.#deps.t('chat.deleteBody'),
        danger: true,
      })
      if (!ok || this.disposed || generation !== this.viewGeneration) return
      await this.history.remove(id)
      if (this.disposed || generation !== this.viewGeneration) return
      if (session.conversationId === id) {
        session.show(null)
        this.shownSummary = null
        this.historyOpen = false
        this.renaming = false
        this.#deps.bumpScrollRevision()
      }
      this.historyError = null
      void this.refresh()
    } catch (error) {
      if (this.disposed || generation !== this.viewGeneration) return
      this.historyError = this.#deps.errorText(error)
      this.retryHistory = this.remove
    } finally {
      if (!this.disposed && generation === this.viewGeneration) this.historyBusy = false
    }
  }

  sourceModel = (turn: ChatTurn): ModelRef | null =>
    turn.endpointId && turn.modelId ? { endpointId: turn.endpointId, modelId: turn.modelId } : null

  sourceLabel = (turn: ChatTurn) => {
    const option = this.#deps
      .getModelOptions()
      ?.find((item) => item.endpointId === turn.endpointId && item.modelId === turn.modelId)
    return option
      ? (option.name ?? option.modelId)
      : turn.modelId
        ? `${turn.endpointId ?? this.#deps.t('chat.unknown')} · ${turn.modelId}`
        : this.#deps.t('chat.noSourceModel')
  }

  #hasChildren(turn: ChatTurn) {
    return (
      this.#deps
        .getSession()
        ?.turns.some((child) => child.parentId === turn.id || child.supersedesId === turn.id) ===
      true
    )
  }

  canEdit = (turn: ChatTurn) => turn.status !== 'running' && !this.#hasChildren(turn)

  editReply = (turn: ChatTurn) => {
    this.sourceReply = turn
    this.editDraft = turn.text
    this.expectedText = turn.text
    this.replyDialog = 'edit'
    this.replyOpen = true
  }

  translateDialog = (turn: ChatTurn) => {
    this.sourceReply = turn
    this.replyModel = this.sourceModel(turn)
    this.targetLanguage = this.#deps.getLocale()
    this.replyError = null
    this.replyDialog = 'translate'
    this.replyOpen = true
  }

  #showConversation(conversation: Conversation) {
    this.viewGeneration++
    this.#deps.setFullAccess(false)
    this.#deps
      .getSession()
      ?.show(conversation.summary.id, conversation.turns, conversation.summary.selectedTurnId)
    this.shownSummary = conversation.summary
    this.#deps.bumpScrollRevision()
  }

  mutateReply = async (turn: ChatTurn, action: 'select' | 'remove' | 'branch' | 'copy') => {
    const session = this.#deps.getSession()
    const id = session?.conversationId
    if (!this.history || !session || !id || this.actionBusy) return
    let generation = this.viewGeneration
    this.historyBusy = true
    try {
      if (action === 'remove') {
        if (turn.status === 'running') return
        const reparent = session.turns.some((child) => child.parentId === turn.id)
        if (
          !(await this.#deps.notify.confirm({
            title: this.#deps.t('chat.deleteReplyTitle'),
            body: reparent ? this.#deps.t('chat.deleteReparentBody') : '',
            danger: true,
          }))
        )
          return
      }
      if (this.disposed || generation !== this.viewGeneration) return
      const conversation =
        action === 'select' || action === 'branch'
          ? await this.history.selectTurn(id, turn.id)
          : action === 'remove'
            ? await this.history.removeTurn(id, turn.id)
            : await this.history.fork(id, turn.id)
      if (this.disposed || generation !== this.viewGeneration) return
      this.#showConversation(conversation)
      generation = this.viewGeneration
      if (action === 'branch') {
        this.#deps.setDraft('')
        this.#deps.bumpDraftRevision()
        this.#deps.discardRecovery()
      }
      this.historyError = null
      void this.refresh()
    } catch (error) {
      if (this.disposed || generation !== this.viewGeneration) return
      this.#deps.notify.toast('error', this.#deps.errorText(error))
    } finally {
      if (!this.disposed && generation === this.viewGeneration) this.historyBusy = false
    }
  }

  retryReply = async (turn: ChatTurn) => {
    const session = this.#deps.getSession()
    if (!session || this.actionBusy) return
    const source = this.sourceModel(turn)
    const options = this.#deps.getModelOptions()
    if (this.history && turn.persisted) {
      if (options === null || this.#deps.getModelError()) {
        this.#deps.notify.toast(
          'error',
          this.#deps.getModelError() ?? this.#deps.t('chat.modelsLoading'),
        )
        return
      }
      if (
        !source ||
        !options.some(
          (option) => option.endpointId === source.endpointId && option.modelId === source.modelId,
        )
      ) {
        this.#deps.notify.toast('error', this.#deps.t('chat.sourceModelUnavailable'))
        return
      }
    }
    this.#deps.bumpScrollRevision()
    await session.retry(this.#deps.consumeBase(), turn.id)
  }

  switchModel = async (turn: ChatTurn, model: ModelRef) => {
    const session = this.#deps.getSession()
    if (!session || this.actionBusy) return
    this.#deps.bumpScrollRevision()
    if (!(await session.retry(this.#deps.consumeBase(), turn.id, model)))
      this.#deps.notify.toast('error', this.#deps.t(`errors.${session.actionError ?? 'internal'}`))
  }

  submitReply = async () => {
    const session = this.#deps.getSession()
    const turn = this.sourceReply
    if (this.disposed || !session || !turn || this.actionBusy) return
    let generation = this.viewGeneration
    this.replyError = null
    if (this.replyDialog === 'edit') {
      const id = session.conversationId
      if (
        !this.history ||
        !id ||
        !this.editDraft.trim() ||
        this.editDraft.length > 24_000 ||
        !this.canEdit(turn)
      )
        return
      this.historyBusy = true
      try {
        const conversation = await this.history.editTurn(
          id,
          turn.id,
          this.editDraft,
          this.expectedText,
        )
        if (this.disposed || generation !== this.viewGeneration) return
        this.#showConversation(conversation)
        generation = this.viewGeneration
        this.replyOpen = false
      } catch (error) {
        if (this.disposed || generation !== this.viewGeneration) return
        this.replyError = this.#deps.errorText(error)
      } finally {
        if (!this.disposed && generation === this.viewGeneration) this.historyBusy = false
      }
    } else {
      if (!this.validReplyModel || !this.replyModel) return
      if (!this.targetLanguage.trim() || this.targetLanguage.trim().length > 80) return
      const accepted = await session.translate(
        { ...this.#deps.consumeBase(), model: this.replyModel },
        turn.id,
        this.targetLanguage.trim(),
      )
      if (this.disposed || generation !== this.viewGeneration) return
      if (accepted) this.replyOpen = false
      else
        this.replyError = session.actionError
          ? this.#deps.t(`errors.${session.actionError}`)
          : this.#deps.t('errors.internal')
    }
  }
}
