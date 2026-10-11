<script lang="ts">
  // Chat (docs/16 §4.2): model, skills, optional history, markdown turns with tool calls and approvals, composer, stop, regenerate.
  // Unmounting aborts the active run, which calls chat_cancel (docs/16 §7 trap 4).
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import { getKitContext } from '../context'
  import type { ConversationSummary, ModelRef, SkillRef } from '../ports'
  import ModelSelect from '../providers/ModelSelect.svelte'
  import MessageItem from './MessageItem.svelte'
  import { RunSession } from './run-session.svelte'
  import SkillPicker from './SkillPicker.svelte'

  let {
    purpose = 'chat',
    model = null,
    scope,
    skills,
    onSkillsChange,
    skillsBudget = 48_000,
    onClose,
  }: {
    purpose?: string
    /** App default; the panel may switch it for itself. */
    model?: ModelRef | null
    scope?: string
    /** Controlled selection; without it the panel keeps its own. */
    skills?: SkillRef[]
    onSkillsChange?: (refs: SkillRef[]) => void
    /** The backend's `Budget.skills_chars`, shown next to the picker. */
    skillsBudget?: number
    onClose?: () => void
  } = $props()

  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const chat = ports.chat
  const history = chat?.history
  const session = chat ? new RunSession(chat) : null

  // svelte-ignore state_referenced_locally
  let picked = $state<ModelRef | null>(model)
  let draft = $state('')
  let conversations = $state<ConversationSummary[]>([])
  // svelte-ignore state_referenced_locally
  let ownSkills = $state<SkillRef[]>(skills ?? [])
  const selected = $derived(skills ?? ownSkills)
  function setSkills(refs: SkillRef[]) {
    ownSkills = refs
    onSkillsChange?.(refs)
  }
  const base = $derived({ purpose, scope: scope ?? null, language: i18n.locale, model: picked, skills: selected })
  const canRegenerate = $derived(session !== null && !session.running && session.turns.length > 0)

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

  function send() {
    const input = draft.trim()
    if (!session || input === '' || session.running) return
    draft = ''
    session.send(base, input)
  }
</script>

{#if session}
  <section class="flex min-h-0 flex-1 flex-col gap-3" aria-label={t('chat.title')}>
    <header class="flex items-center gap-2">
      <div class="w-72">
        <ModelSelect value={picked} filter={{ modelType: 'chat' }} onchange={(m) => (picked = m)} />
      </div>
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
      {:else}
        <Button variant="outline" size="sm" disabled={session.running} onclick={() => session.show(null)}>
          {t('chat.newConversation')}
        </Button>
      {/if}
      {#if onClose}
        <Button class="ml-auto" variant="ghost" size="sm" onclick={onClose}>{t('chat.close')}</Button>
      {/if}
    </header>
    <ol class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto" aria-label={t('chat.messages')}>
      {#each session.turns as turn, i (i)}
        <MessageItem {turn} onApprove={(tool, allow) => session.approve(tool, allow)} />
      {/each}
    </ol>
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
      <div class="flex justify-end gap-2">
        {#if canRegenerate}
          <Button type="button" variant="outline" onclick={() => session.regenerate(base)}
            >{t('chat.regenerate')}</Button
          >
        {/if}
        {#if session.running}
          <Button type="button" variant="destructive" onclick={() => session.stop()}>{t('chat.stop')}</Button>
        {:else}
          <Button type="submit" disabled={draft.trim() === '' || picked === null}>{t('chat.send')}</Button>
        {/if}
      </div>
    </form>
  </section>
{/if}
