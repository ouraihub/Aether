<script lang="ts">
  import { onDestroy, untrack } from 'svelte'
  import { Bot } from '@lucide/svelte'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import { getKitContext } from '../context'
  import {
    KitClientError,
    type ModelRef,
    type PreparedSkill,
    type RunEvent,
    type SkillDto,
  } from '../ports'
  import ModelSelect from '../providers/ModelSelect.svelte'
  import ApprovalCard from '../chat/ApprovalCard.svelte'
  import ToolCallItem from '../chat/ToolCallItem.svelte'
  import type { ToolItem } from '../chat/run-session.svelte'
  import { errorText } from './error'
  import SkillPreviewDialog from './SkillPreviewDialog.svelte'

  let {
    active,
    open,
    onclose,
    onprepared,
  }: {
    active: boolean
    open: boolean
    onclose: () => void
    onprepared: (prepared: PreparedSkill) => Promise<void>
  } = $props()
  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const authoring = ports.skillSources?.authoring
  type Turn = {
    id: number
    input: string
    text: string
    tools: ToolItem[]
    status: 'running' | 'completed' | 'failed' | 'cancelled'
  }
  let creator = $state<SkillDto | null>(null)
  let session = $state<string | null>(null)
  let model = $state<ModelRef | null>(null)
  let input = $state('')
  let creatorReview = $state(false)
  let turns = $state<Turn[]>([])
  let loading = $state(false)
  let running = $state(false)
  let error = $state<string | null>(null)
  let runId = $state<string | null>(null)
  let answering = $state<string[]>([])
  let operation: AbortController | null = null
  let epoch = 0
  let alive = true
  let nextTurn = 0
  let cleanup: Promise<void> = Promise.resolve()

  function report(cause: unknown): void {
    notify.toast('error', errorText(t, cause))
  }
  async function release(id: string, run: string | null): Promise<void> {
    if (!authoring) return
    try {
      if (run) await authoring.cancel(id, run)
    } catch (cause) {
      report(cause)
    }
    try {
      await authoring.close(id)
    } catch (cause) {
      report(cause)
    }
  }
  function reset(): void {
    epoch += 1
    operation?.abort()
    operation = null
    const previous = session
    const run = runId
    session = null
    runId = null
    running = false
    loading = false
    creator = null
    turns = []
    input = ''
    error = null
    answering = []
    creatorReview = false
    if (previous) cleanup = cleanup.then(() => release(previous, run))
  }
  async function loadCreator(): Promise<void> {
    reset()
    const currentEpoch = epoch
    loading = true
    try {
      await cleanup
      if (!alive || epoch !== currentEpoch || !active) return
      const skills = await ports.skills?.list()
      const builtin = skills?.find(
        (skill) => skill.origin === 'builtin' && skill.name === 'skill-creator',
      )
      if (!builtin || !ports.skills)
        throw new KitClientError({
          code: 'needs_config',
          message: 'skill-creator unavailable',
        })
      const full = await ports.skills.get(builtin.id)
      if (alive && epoch === currentEpoch) creator = full
    } catch (cause) {
      if (alive && epoch === currentEpoch) error = errorText(t, cause)
    } finally {
      if (alive && epoch === currentEpoch) loading = false
    }
  }
  $effect(() => {
    if (active)
      untrack(() => {
        void loadCreator()
      })
    else untrack(reset)
  })
  onDestroy(() => {
    alive = false
    reset()
  })

  async function confirmCreator(): Promise<void> {
    if (!authoring || !creator || loading || session) return
    const currentEpoch = epoch
    const controller = new AbortController()
    operation = controller
    loading = true
    error = null
    try {
      const id = await authoring.create({ signal: controller.signal })
      if (!alive || epoch !== currentEpoch || controller.signal.aborted) {
        await release(id, null)
        return
      }
      session = id
    } catch (cause) {
      if (alive && epoch === currentEpoch && !controller.signal.aborted)
        error = errorText(t, cause)
    } finally {
      if (alive && epoch === currentEpoch) {
        loading = false
        operation = null
      }
    }
  }
  function finish(turn: Turn, status: Turn['status']): void {
    turn.status = status
    for (const tool of turn.tools) tool.pending = null
    running = false
    runId = null
    operation = null
    answering = []
  }
  async function send(): Promise<void> {
    if (!authoring || !session || running || loading || !input.trim()) return
    if (!model) {
      error = t('skills.add.choose_model')
      return
    }
    const id = session
    const currentEpoch = epoch
    const controller = new AbortController()
    operation = controller
    const current = (): boolean =>
      alive &&
      epoch === currentEpoch &&
      session === id &&
      !controller.signal.aborted
    const turnId = ++nextTurn
    turns.push({
      id: turnId,
      input: input.trim(),
      text: '',
      tools: [],
      status: 'running',
    })
    // The pushed literal is wrapped by the $state proxy; mutate the proxied
    // entry.
    const turn = turns.at(-1)
    if (!turn) return
    input = ''
    error = null
    running = true
    let ended = false
    const receive = (event: RunEvent): void => {
      if (!current() || ended) return
      const tool = (callId: string): ToolItem | undefined =>
        turn.tools.find((item) => item.callId === callId)
      switch (event.kind) {
        case 'text_delta':
          turn.text += event.text
          break
        case 'tool_call':
          if (!tool(event.callId))
            turn.tools.push({
              callId: event.callId,
              name: event.name,
              server: event.server ?? null,
              pending: null,
              outcome: null,
              summary: '',
            })
          break
        case 'approval_required': {
          if (!tool(event.callId))
            turn.tools.push({
              callId: event.callId,
              name: event.name,
              server: null,
              pending: null,
              outcome: null,
              summary: '',
            })
          const item = tool(event.callId)
          if (item) item.pending = event.argsPreview
          break
        }
        case 'tool_result': {
          const item = tool(event.callId)
          if (item)
            Object.assign(item, {
              pending: null,
              outcome: event.outcome,
              summary: event.summary,
            })
          break
        }
        case 'end':
          ended = true
          finish(turn, event.status)
          if (event.status === 'failed')
            error = errorText(
              t,
              new KitClientError(
                event.error ?? { code: 'internal', message: '' },
              ),
            )
          break
      }
    }
    try {
      const accepted = await authoring.send(id, turn.input, model, receive, {
        signal: controller.signal,
      })
      if (!current()) {
        if (!ended) await authoring.cancel(id, accepted.runId).catch(report)
        if (
          alive &&
          epoch === currentEpoch &&
          session === id &&
          controller.signal.aborted
        ) {
          finish(turn, 'cancelled')
          loading = false
        }
        return
      }
      if (!ended) runId = accepted.runId
    } catch (cause) {
      if (alive && epoch === currentEpoch && session === id && !ended) {
        finish(turn, controller.signal.aborted ? 'cancelled' : 'failed')
        loading = false
        if (!controller.signal.aborted) error = errorText(t, cause)
      }
    }
  }
  async function approve(tool: ToolItem, allow: boolean): Promise<void> {
    if (
      !authoring ||
      !session ||
      !runId ||
      !running ||
      tool.pending === null ||
      answering.includes(tool.callId)
    )
      return
    const id = session
    const run = runId
    const currentEpoch = epoch
    answering = [...answering, tool.callId]
    try {
      await authoring.approve(id, run, tool.callId, allow)
      if (alive && epoch === currentEpoch && runId === run) tool.pending = null
    } catch (cause) {
      if (alive && epoch === currentEpoch && runId === run)
        error = errorText(t, cause)
    } finally {
      if (alive && epoch === currentEpoch)
        answering = answering.filter((callId) => callId !== tool.callId)
    }
  }
  async function stop(): Promise<void> {
    if (!authoring || !session || !running) return
    const id = session
    const currentEpoch = epoch
    const run = runId
    operation?.abort()
    loading = true
    if (!run) return
    try {
      await authoring.cancel(id, run)
    } catch (cause) {
      if (alive && epoch === currentEpoch) error = errorText(t, cause)
    } finally {
      if (alive && epoch === currentEpoch) {
        const turn = turns.findLast((item) => item.status === 'running')
        if (turn) finish(turn, 'cancelled')
        loading = false
      }
    }
  }
  async function prepare(): Promise<void> {
    if (!authoring || !session || running || loading) return
    const id = session
    const currentEpoch = epoch
    const controller = new AbortController()
    operation = controller
    loading = true
    error = null
    try {
      const prepared = await authoring.prepare(id, {
        signal: controller.signal,
      })
      if (
        alive &&
        epoch === currentEpoch &&
        session === id &&
        !controller.signal.aborted
      )
        await onprepared(prepared)
    } catch (cause) {
      if (alive && epoch === currentEpoch) error = errorText(t, cause)
    } finally {
      if (alive && epoch === currentEpoch) {
        loading = false
        operation = null
      }
    }
  }
</script>

<Dialog.Root
  open={open && !creatorReview}
  onOpenChange={(value) => {
    if (!value) onclose()
  }}
>
  <Dialog.Content class="flex max-h-[85vh] flex-col sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>{t('skills.add.agent')}</Dialog.Title>
      <Dialog.Description>{t('skills.add.authoring_notice')}</Dialog.Description
      >
    </Dialog.Header>
    <div class="flex min-h-0 flex-col gap-4 overflow-y-auto">
      {#if error}<p role="alert" class="text-destructive text-sm">
          {error}
        </p>{/if}
      {#if !session}
        <p class="text-sm">{t('skills.add.creator_review')}</p>
        {#if creator}
          <Button disabled={loading} onclick={() => (creatorReview = true)}
            >{t('skills.add.review_creator')}</Button
          >
        {/if}
      {:else}
        <Field.Group>
          <Field.Field>
            <Field.Label>{t('skills.add.model')}</Field.Label>
            <ModelSelect
              value={model}
              filter={{ modelType: 'chat' }}
              onchange={(value) => (model = value)}
              disabled={running || loading}
              placeholder={t('skills.add.choose_model')}
            />
          </Field.Field>
          <ol
            class="flex flex-col gap-4"
            aria-label={t('skills.add.conversation')}
          >
            {#each turns as turn (turn.id)}
              <li class="flex flex-col gap-2 rounded-md border p-3">
                <p class="whitespace-pre-wrap text-sm">{turn.input}</p>
                <p class="whitespace-pre-wrap text-sm">{turn.text}</p>
                {#each turn.tools as tool (tool.callId)}
                  {#if tool.pending !== null}
                    <p class="text-destructive text-xs">
                      {t('skills.add.tool_warning')}
                    </p>
                    <fieldset
                      disabled={!runId || answering.includes(tool.callId)}
                    >
                      <ApprovalCard
                        {tool}
                        onAnswer={(allow) => void approve(tool, allow)}
                      />
                    </fieldset>
                  {:else}<ToolCallItem {tool} />{/if}
                {/each}
                {#if turn.status === 'running'}<p
                    role="status"
                    class="text-muted-foreground text-xs"
                  >
                    {t('skills.add.working')}
                  </p>{/if}
                {#if turn.status === 'cancelled'}<p
                    role="status"
                    class="text-muted-foreground text-xs"
                  >
                    {t('chat.stopped')}
                  </p>{/if}
              </li>
            {/each}
          </ol>
          <Field.Field>
            <Field.Label for="skill-authoring-message"
              >{t('skills.add.authoring_input')}</Field.Label
            >
            <Textarea
              id="skill-authoring-message"
              bind:value={input}
              disabled={running || loading}
              rows={3}
            />
          </Field.Field>
          <div class="flex flex-wrap gap-2">
            <Button
              disabled={running || loading || !input.trim()}
              onclick={() => void send()}
              ><Bot data-icon="inline-start" />{t('chat.send')}</Button
            >
            {#if running}<Button variant="outline" onclick={() => void stop()}
                >{t('chat.stop')}</Button
              >{/if}
            <Button
              variant="outline"
              disabled={running || loading || turns.length === 0}
              onclick={() => void prepare()}
              >{t('skills.add.preview_artifact')}</Button
            >
          </div>
        </Field.Group>
      {/if}
      {#if loading}<p role="status" class="text-muted-foreground text-sm">
          {t('skills.add.working')}
        </p>{/if}
    </div>
    <Dialog.Footer
      ><Button variant="outline" onclick={onclose}>{t('common.cancel')}</Button
      ></Dialog.Footer
    >
  </Dialog.Content>
</Dialog.Root>

<SkillPreviewDialog
  skill={creator}
  title={t('skills.add.creator_review')}
  confirmLabel={t('skills.add.confirm_creator')}
  busy={loading}
  {error}
  onconfirm={() => {
    creatorReview = false
    void confirmCreator()
  }}
  bind:open={
    () => creatorReview && open,
    (value) => {
      if (!value) creatorReview = false
    }
  }
/>
