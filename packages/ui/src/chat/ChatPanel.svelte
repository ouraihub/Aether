<script lang="ts">
  // Chat (docs/16 §4.2): model, skills, optional history, markdown turns with tool calls and approvals, copy / insert,
  // composer, stop, regenerate / retry. The host persists the shown conversation via `conversationId` + `onConversationChange`.
  // Unmounting aborts the active run, which calls chat_cancel (docs/16 §7 trap 4).
  import XIcon from '@lucide/svelte/icons/x'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import GlobeIcon from '@lucide/svelte/icons/globe'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
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
    /** App default; the panel may switch it for itself. */
    model?: ModelRef | null
    scope?: string
    /** Host data sent with each run; shown as a chip the user can drop for one message. */
    context?: { kind: string; id: string; label: string } | null
    /** Controlled selection; without it the panel keeps its own. */
    skills?: SkillRef[]
    onSkillsChange?: (refs: SkillRef[]) => void
    /** The backend's `Budget.skills_chars`, shown next to the picker. */
    skillsBudget?: number
    /** Shows "insert" on assistant turns; receives the turn's markdown. */
    onInsert?: (markdown: string) => void
    /** Shown as a button when there is no chat model to pick. */
    onOpenSettings?: () => void
    /** Conversation to show on mount; one that no longer exists starts a new conversation. */
    conversationId?: string | null
    /** Called whenever the shown conversation changes (`null` = new conversation). */
    onConversationChange?: (id: string | null) => void
    onClose?: () => void
  } = $props()

  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const chat = ports.chat
  const history = chat?.history
  const session = chat ? new RunSession(chat) : null

  // svelte-ignore state_referenced_locally
  let picked = $state<ModelRef | null>(model)
  let capability = $state<ModelOption | null>(null)
  let reasoningEffort = $state<ReasoningEffort>('default')
  const allowedEfforts = $derived(capability?.reasoning?.efforts ?? [])
  function pickModel(next: ModelRef, option: ModelOption | null) {
    picked = next
    capability = option
    reasoningEffort = 'default'
    session?.resetContext()
  }
  let draft = $state('')
  let conversations = $state<ConversationSummary[]>([])
  // svelte-ignore state_referenced_locally
  let ownSkills = $state<SkillRef[]>(skills ?? [])
  const selected = $derived(skills ?? ownSkills)
  function setSkills(refs: SkillRef[]) {
    ownSkills = refs
    onSkillsChange?.(refs)
  }
  // Key of the context the user removed; a new context brings the chip back.
  let droppedKey = $state<string | null>(null)
  const contextKey = $derived(context ? `${context.kind}:${context.id}` : null)
  const activeContext = $derived(context && contextKey !== droppedKey ? context : null)
  const base = $derived({
    purpose,
    scope: scope ?? null,
    language: i18n.locale,
    model: picked,
    skills: selected,
    reasoningEffort,
    context: activeContext ? { kind: activeContext.kind, id: activeContext.id } : null,
  })
  const canRegenerate = $derived(session !== null && !session.running && session.turns.length > 0)
  let webSearch = $state(false)
  let noModels = $state(false)
  $effect(() => {
    let alive = true
    void ports.providers
      ?.modelOptions({ modelType: 'chat' })
      .then((list) => {
        if (!alive) return
        noModels = list.length === 0
        capability =
          list.find((item) => item.endpointId === picked?.endpointId && item.modelId === picked?.modelId) ?? null
        if (!allowedEfforts.includes(reasoningEffort) && reasoningEffort !== 'default') reasoningEffort = 'default'
      })
      .catch(() => {
        if (alive) notify.toast('error', t('chat.modelsFailed'))
      })
    return () => {
      alive = false
    }
  })
  const currentContext = $derived(
    session?.contextUsage &&
      session.contextUsage.endpointId === picked?.endpointId &&
      session.contextUsage.modelId === picked?.modelId
      ? session.contextUsage
      : null,
  )

  async function refresh() {
    if (!history) return
    try {
      conversations = await history.list(scope)
    } catch {
      notify.toast('error', t('chat.historyFailed'))
    }
  }

  $effect(() => () => session?.stop())
  // On mount, and after each run: it may have created or updated a conversation.
  $effect(() => {
    if (session && !session.running) void refresh()
  })

  // svelte-ignore state_referenced_locally
  let reported = conversationId
  let restored = $state(false)
  $effect(() => {
    const id = session?.conversationId ?? null
    if (!restored || id === reported) return
    reported = id
    onConversationChange?.(id)
  })
  // svelte-ignore state_referenced_locally
  void restore(conversationId)

  async function restore(id: string | null) {
    if (history && session && id !== null) {
      try {
        const conversation = await history.get(id)
        session.show(id, conversation.turns)
      } catch (error) {
        if (!isKitClientError(error) || error.code !== 'not_found') notify.toast('error', t('chat.historyFailed'))
      }
    }
    restored = true
  }

  async function open(id: string) {
    if (!history || !session) return
    if (id === '') {
      session.show(null)
      return
    }
    try {
      const conversation = await history.get(id)
      session.show(id, conversation.turns)
    } catch {
      notify.toast('error', t('chat.historyFailed'))
    }
  }

  async function remove() {
    const id = session?.conversationId
    if (!history || !session || !id || session.running) return
    const ok = await notify.confirm({ title: t('chat.deleteTitle'), body: t('chat.deleteBody'), danger: true })
    if (!ok) return
    try {
      await history.remove(id)
    } catch {
      notify.toast('error', t('chat.deleteFailed'))
      return
    }
    if (session.conversationId === id) session.show(null)
    void refresh()
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && onClose && !session?.running && !e.defaultPrevented) onClose()
  }

  function send() {
    const input = draft.trim()
    if (!session || input === '' || session.running) return
    draft = ''
    session.send(base, input, webSearch)
    droppedKey = null
  }
</script>

{#if session}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions (Esc from any control inside the panel) -->
  <section class="flex min-h-0 flex-1 flex-col gap-3" aria-label={t('chat.title')} {onkeydown}>
    <header class="flex items-center gap-2">
      <div class="w-72">
        <ModelSelect value={picked} filter={{ modelType: 'chat' }} disabled={session.running} onchange={pickModel} />
      </div>
      {#if allowedEfforts.length > 0}
        <Select.Root
          type="single"
          value={reasoningEffort}
          onValueChange={(value) => (reasoningEffort = value as ReasoningEffort)}
        >
          <Select.Trigger class="w-36" disabled={session.running} aria-label={t('chat.reasoningEffort')}
            >{t(`chat.effort.${reasoningEffort}`)}</Select.Trigger
          >
          <Select.Content
            ><Select.Group>
              <Select.Item value="default" label={t('chat.effort.default')} />
              {#each REASONING_EFFORTS.filter( (effort) => allowedEfforts.includes(effort) ) as effort (effort)}<Select.Item
                  value={effort}
                  label={t(`chat.effort.${effort}`)}
                />{/each}
            </Select.Group></Select.Content
          >
        </Select.Root>
      {/if}
      {#if history}
        <Select.Root type="single" value={session.conversationId ?? ''} onValueChange={(id) => void open(id)}>
          <Select.Trigger class="w-64" aria-label={t('chat.history')}>
            {conversations.find((c) => c.id === session.conversationId)?.title ?? t('chat.newConversation')}
          </Select.Trigger>
          <Select.Content>
            <Select.Group>
              <Select.Item value="" label={t('chat.newConversation')} />
              {#each conversations as c (c.id)}
                <Select.Item value={c.id} label={c.title} />
              {/each}
            </Select.Group>
          </Select.Content>
        </Select.Root>
        {#if session.conversationId}
          <Button variant="ghost" size="sm" disabled={session.running} onclick={() => void remove()}>
            {t('chat.delete')}
          </Button>
        {/if}
      {:else}
        <Button variant="outline" size="sm" disabled={session.running} onclick={() => session.show(null)}>
          {t('chat.newConversation')}
        </Button>
      {/if}
      {#if onClose}
        <Button class="ml-auto" variant="ghost" size="sm" onclick={onClose}>{t('chat.close')}</Button>
      {/if}
    </header>
    {#if noModels}
      <Empty.Root>
        <Empty.Description>{t('chat.noModels')}</Empty.Description>
        {#if onOpenSettings}
          <Button variant="outline" size="sm" onclick={onOpenSettings}>{t('chat.openSettings')}</Button>
        {/if}
      </Empty.Root>
    {:else if session.turns.length === 0}
      <p class="text-muted-foreground text-kit-sm flex flex-1 items-center justify-center">{t('chat.empty')}</p>
    {:else}
      <ol class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto" aria-label={t('chat.messages')}>
        {#each session.turns as turn, i (i)}
          <MessageItem
            {turn}
            {onInsert}
            onRetry={i === session.turns.length - 1 && !session.running
              ? () => session.regenerate(base, webSearch)
              : undefined}
            onApprove={(tool, allow) => session.approve(tool, allow)}
          />
        {/each}
      </ol>
    {/if}
    <form
      class="flex flex-col gap-2"
      onsubmit={(e) => {
        e.preventDefault()
        send()
      }}
    >
      {#if ports.skills}
        <SkillPicker
          port={ports.skills}
          value={selected}
          budget={skillsBudget}
          changed={session.skillChanged}
          onchange={setSkills}
          onresolved={() => (session.skillChanged = null)}
        />
      {/if}
      {#if ports.search && purpose === 'chat'}
        {#if session.search.status === 'searching'}<p class="text-muted-foreground text-kit-sm" aria-live="polite">
            {t('chat.searching')}
          </p>{/if}
        {#if session.search.status === 'complete'}<p class="text-muted-foreground text-kit-sm" aria-live="polite">
            {t('chat.searchResults', { count: session.search.hits.length })}
          </p>{/if}
      {/if}
      {#if activeContext}
        <div class="flex">
          <span class="bg-muted rounded-kit-sm text-kit-sm flex max-w-full items-center gap-1 py-0.5 pr-0.5 pl-2">
            <span class="truncate">{activeContext.label}</span>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              class="size-5"
              aria-label={t('chat.context.remove', { label: activeContext.label })}
              onclick={() => (droppedKey = contextKey)}><XIcon /></Button
            >
          </span>
        </div>
      {/if}
      <Textarea
        bind:value={draft}
        aria-label={t('chat.input')}
        placeholder={t('chat.placeholder')}
        onkeydown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
            e.preventDefault()
            send()
          }
        }}
      />
      <div class="flex items-center gap-2">
        {#if ports.search && purpose === 'chat'}
          <Button
            type="button"
            variant={webSearch ? 'secondary' : 'ghost'}
            size="icon-sm"
            aria-label={t('chat.network')}
            aria-pressed={webSearch}
            disabled={session.running}
            title={`${t(webSearch ? 'chat.networkOn' : 'chat.networkOff')} · ${t('chat.networkPrivacy')}`}
            onclick={() => (webSearch = !webSearch)}
          >
            {#if webSearch}
              <GlobeIcon />
            {:else}
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <circle cx="12" cy="12" r="10" />
                <path d="M2 12h20M12 2a20 20 0 0 1 0 20 20 20 0 0 1 0-20" />
                <path d="m3 3 18 18" />
              </svg>
            {/if}
          </Button>
        {/if}
        <UsageDetails
          context={currentContext}
          usage={session.turns.at(-1)?.usage ?? null}
          total={session.cumulativeUsage}
        />
        <div class="ml-auto flex items-center gap-2">
          {#if canRegenerate}
            <Button type="button" variant="outline" onclick={() => session.regenerate(base, webSearch)}
              >{t('chat.regenerate')}</Button
            >
          {/if}
          {#if session.running}
            <Button type="button" variant="destructive" onclick={() => session.stop()}>{t('chat.stop')}</Button>
          {:else}
            <Button type="submit" disabled={draft.trim() === '' || picked === null}>{t('chat.send')}</Button>
          {/if}
        </div>
      </div>
    </form>
  </section>
{/if}
