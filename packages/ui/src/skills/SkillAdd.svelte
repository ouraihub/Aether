<script lang="ts">
  import { onDestroy } from 'svelte'
  import { Bot, ChevronDown, FolderSearch, Import, Plus, Search } from '@lucide/svelte'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { getKitContext } from '../context'
  import {
    isKitClientError,
    type Preview,
    type PreparedSkill,
    type SkillMeta,
    type Source,
    type SkillCandidate,
    type SystemSkillCandidate,
  } from '../ports'
  import { errorText } from './error'
  import SkillPreviewDialog from './SkillPreviewDialog.svelte'
  import SkillAuthoring from './SkillAuthoring.svelte'

  let {
    oninstalled,
    onmanual,
    disabled = false,
  }: {
    oninstalled: (skills: SkillMeta[]) => void
    onmanual: () => void
    disabled?: boolean
  } = $props()
  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const port = ports.skills
  const sources = ports.skillSources
  type Mode = 'agent' | 'online' | 'system' | 'local'
  let mode = $state<Mode | null>(null)
  let busy = $state(false)
  let error = $state<string | null>(null)
  let query = $state('')
  let candidates = $state<SkillCandidate[]>([])
  let system = $state<SystemSkillCandidate[]>([])
  let searched = $state(false)
  let file = $state<HTMLInputElement | null>(null)
  let source: Source | null = null
  let preview = $state<Preview | null>(null)
  let origin = $state('')
  let ignored = $state<string[]>([])
  let operation: AbortController | null = null
  let generation = 0
  let alive = true

  function invalidate(): void {
    generation += 1
    operation?.abort()
    operation = null
    busy = false
  }
  function close(): void {
    invalidate()
    mode = null
    preview = null
    source = null
    error = null
  }
  onDestroy(() => {
    alive = false
    invalidate()
  })

  async function act(work: (signal: AbortSignal, current: () => boolean) => Promise<void>): Promise<void> {
    invalidate()
    const id = generation
    const controller = new AbortController()
    operation = controller
    const current = (): boolean => alive && generation === id && !controller.signal.aborted
    busy = true
    error = null
    try {
      await work(controller.signal, current)
    } catch (cause) {
      if (current()) error = errorText(t, cause)
    } finally {
      if (current()) {
        busy = false
        operation = null
      }
    }
  }
  function open(next: Mode): void {
    close()
    query = ''
    candidates = []
    system = []
    searched = false
    mode = next
    if (next === 'system') void scan(false)
  }
  async function showPreview(
    next: Source,
    from: string,
    current: () => boolean,
    excluded: string[] = [],
  ): Promise<void> {
    if (!port || !current()) return
    const result = await port.preview(next)
    if (!current()) return
    source = next
    origin = from
    ignored = excluded
    preview = result
  }
  function search(): Promise<void> {
    return act(async (signal, current) => {
      if (!sources) return
      const result = await sources.search(query.trim(), { signal })
      if (current()) {
        candidates = result
        searched = true
      }
    })
  }
  function prepare(candidate: SkillCandidate): Promise<void> {
    return act(async (signal, current) => {
      if (!sources) return
      const prepared = await sources.prepare(candidate.url, { signal })
      await showPreview(prepared.source, prepared.origin, current, prepared.ignored)
    })
  }
  function scan(pick: boolean): Promise<void> {
    return act(async (signal, current) => {
      if (!sources) return
      const found = await (pick ? sources.pickSearchDirectory({ signal }) : sources.discover({ signal }))
      if (current() && found !== null) {
        system = found
        searched = true
      }
    })
  }
  function prepareSystem(candidate: SystemSkillCandidate): Promise<void> {
    return act(async (signal, current) => {
      if (!sources) return
      const prepared = await sources.prepareSystem(candidate.path, { signal })
      await showPreview(prepared.source, prepared.origin, current, prepared.ignored)
    })
  }
  function pickDirectory(): Promise<void> {
    return act(async (signal, current) => {
      const prepared = await sources?.pickDirectory({ signal })
      if (prepared) await showPreview(prepared.source, prepared.origin, current, prepared.ignored)
    })
  }
  function pickFile(input: HTMLInputElement): void {
    const picked = input.files?.[0]
    input.value = ''
    if (!picked) return
    void act(async (_signal, current) => {
      const next: Source = { bytes: { fileName: picked.name, bytes: [...new Uint8Array(await picked.arrayBuffer())] } }
      await showPreview(next, picked.name, current)
    })
  }
  function closePreview(): void {
    invalidate()
    preview = null
    source = null
  }
  function install(replace: boolean): void {
    const confirmed = preview
    const from = source
    if (!port || !confirmed || !from || busy) return
    void act(async (_signal, current) => {
      try {
        const installed = await port.install(from, confirmed.digest, replace)
        if (!current()) return
        oninstalled(installed)
        close()
        notify.toast('success', t('skills.installed', { name: confirmed.name }))
      } catch (cause) {
        if (isKitClientError(cause) && cause.details.reason === 'content_changed') {
          const refreshed = await port.preview(from)
          if (current()) {
            preview = refreshed
            notify.toast('info', t('skills.content_changed'))
          }
          return
        }
        throw cause
      }
    })
  }
</script>

<div class="flex gap-2">
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <Button {...props} {disabled}
          ><Plus data-icon="inline-start" />{t('skills.add.title')}<ChevronDown data-icon="inline-end" /></Button
        >
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start">
      <DropdownMenu.Group>
        {#if sources}
          {#if sources.authoring && ports.providers}
            <DropdownMenu.Item onSelect={() => open('agent')}><Bot />{t('skills.add.agent')}</DropdownMenu.Item>
          {/if}
          <DropdownMenu.Item onSelect={() => open('online')}><Search />{t('skills.add.online')}</DropdownMenu.Item>
          <DropdownMenu.Item onSelect={() => open('system')}><FolderSearch />{t('skills.add.system')}</DropdownMenu.Item
          >
        {/if}
        <DropdownMenu.Item onSelect={() => open('local')}><Import />{t('skills.add.local')}</DropdownMenu.Item>
      </DropdownMenu.Group>
    </DropdownMenu.Content>
  </DropdownMenu.Root>
  <Button variant="outline" {disabled} onclick={onmanual}>{t('skills.new')}</Button>
</div>
<input
  bind:this={file}
  type="file"
  accept=".md,.zip"
  class="hidden"
  aria-label={t('skills.import')}
  onchange={(e) => pickFile(e.currentTarget)}
/>

<Dialog.Root
  open={mode !== null && mode !== 'agent' && preview === null}
  onOpenChange={(value) => {
    if (!value) close()
  }}
>
  <Dialog.Content class="flex max-h-[85vh] flex-col sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>{mode ? t(`skills.add.${mode}`) : t('skills.add.title')}</Dialog.Title>
      <Dialog.Description>{t('skills.add.review_notice')}</Dialog.Description>
    </Dialog.Header>
    <div class="flex min-h-0 flex-col gap-4 overflow-y-auto">
      {#if error}<p role="alert" class="text-destructive text-sm">{error}</p>{/if}
      {#if mode === 'online'}
        <form
          onsubmit={(event) => {
            event.preventDefault()
            if (!busy && query.trim()) void search()
          }}
        >
          <Field.Group>
            <Field.Field>
              <Field.Label for="skill-source-query">{t('skills.add.query')}</Field.Label>
              <Input
                id="skill-source-query"
                bind:value={query}
                placeholder={t('skills.add.query_hint')}
                disabled={busy}
              />
            </Field.Field>
            <Button type="submit" disabled={busy || !query.trim()}
              ><Search data-icon="inline-start" />{t('skills.add.search')}</Button
            >
          </Field.Group>
        </form>
        <ul class="flex flex-col gap-2" aria-label={t('skills.add.results')}>
          {#each candidates as candidate (candidate.url)}
            <li class="flex items-start gap-3 rounded-md border p-3">
              <div class="flex min-w-0 flex-1 flex-col gap-1">
                <strong class="text-sm">{candidate.name}</strong>
                <p class="text-muted-foreground text-xs">{candidate.description}</p>
                <p class="text-muted-foreground break-all text-xs">{candidate.origin} · {candidate.url}</p>
              </div>
              <Button size="sm" variant="outline" disabled={busy} onclick={() => void prepare(candidate)}
                >{t('skills.add.preview')}</Button
              >
            </li>
          {/each}
        </ul>
      {:else if mode === 'system'}
        <p class="text-muted-foreground text-sm">{t('skills.add.system_hint')}</p>
        <div class="flex flex-wrap gap-2">
          <Button variant="outline" disabled={busy} onclick={() => void scan(false)}>{t('skills.add.rescan')}</Button>
          <Button variant="outline" disabled={busy} onclick={() => void scan(true)}
            ><FolderSearch data-icon="inline-start" />{t('skills.add.pick_search_directory')}</Button
          >
        </div>
        <ul class="flex flex-col gap-2" aria-label={t('skills.add.results')}>
          {#each system as candidate (candidate.path)}
            <li class="flex items-start gap-3 rounded-md border p-3">
              <div class="flex min-w-0 flex-1 flex-col gap-1">
                <strong class="text-sm">{candidate.name}</strong>
                <p class="text-muted-foreground break-all text-xs">{candidate.origin} · {candidate.path}</p>
              </div>
              <Button size="sm" variant="outline" disabled={busy} onclick={() => void prepareSystem(candidate)}
                >{t('skills.add.preview')}</Button
              >
            </li>
          {/each}
        </ul>
      {:else if mode === 'local'}
        <p class="text-muted-foreground text-sm">{t('skills.add.local_hint')}</p>
        <div class="flex flex-wrap gap-2">
          <Button disabled={busy} onclick={() => file?.click()}
            ><Import data-icon="inline-start" />{t('skills.import')}</Button
          >
          <Button variant="outline" disabled={busy || !sources} onclick={() => void pickDirectory()}
            ><FolderSearch data-icon="inline-start" />{t('skills.add.pick_directory')}</Button
          >
        </div>
      {/if}
      {#if busy}<p role="status" class="text-muted-foreground text-sm">{t('skills.add.working')}</p>{/if}
      {#if !busy && searched && ((mode === 'online' && candidates.length === 0) || (mode === 'system' && system.length === 0))}
        <p role="status" class="text-muted-foreground text-sm">{t('skills.add.no_results')}</p>
      {/if}
    </div>
    <Dialog.Footer><Button variant="outline" onclick={close}>{t('common.cancel')}</Button></Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<SkillAuthoring
  active={mode === 'agent'}
  open={mode === 'agent' && preview === null}
  onclose={close}
  onprepared={async (prepared: PreparedSkill) => {
    const id = generation
    await showPreview(
      prepared.source,
      prepared.origin,
      () => alive && generation === id && mode === 'agent',
      prepared.ignored,
    )
  }}
/>

<SkillPreviewDialog
  bind:open={
    () => preview !== null,
    (value) => {
      if (!value) closePreview()
    }
  }
  skill={preview}
  title={t('skills.preview_title')}
  current={preview?.current ?? null}
  {busy}
  {error}
  {origin}
  {ignored}
  onconfirm={install}
/>
