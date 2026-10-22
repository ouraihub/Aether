<script lang="ts">
  // Provider settings (docs/33): list on the left, detail on the right, every field saves itself.
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis-vertical'
  import SquarePenIcon from '@lucide/svelte/icons/square-pen'
  import Trash2Icon from '@lucide/svelte/icons/trash-2'
  import UserPenIcon from '@lucide/svelte/icons/user-pen'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import SearchIcon from '@lucide/svelte/icons/search'
  import Settings2Icon from '@lucide/svelte/icons/settings-2'
  import SlidersHorizontalIcon from '@lucide/svelte/icons/sliders-horizontal'
  import { getKitContext } from '../context'
  import { isKitClientError, type ProbeResult, type Provider, type ProviderPreset } from '../ports'
  import { cn } from '../utils'
  import { adopt, newEndpoint, probeTarget, savable, toDraft, toSave, validateModel, type ProviderDraft } from './draft'
  import EndpointPanel, { type CheckState } from './EndpointPanel.svelte'
  import ModelPickerDialog from './ModelPickerDialog.svelte'
  import { providersRevision, settingsTarget } from './revision.svelte'

  let {
    onchanged,
    presets = [],
    usedBy,
  }: {
    onchanged?: () => void
    presets?: ProviderPreset[]
    /** Localized role names bound to a model, shown when its type changes (contract §6). */
    usedBy?: (endpointId: string, modelId: string) => string[]
  } = $props()
  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const port = ports.providers
  const FILTERS = ['all', 'enabled', 'disabled'] as const

  let providers = $state<Provider[]>([])
  let draft = $state<ProviderDraft | null>(null)
  /** Remounts the detail when another provider is loaded, resetting its local input state. */
  let token = $state(0)
  let active = $state(0)
  let check = $state<{ slug: string; value: CheckState } | null>(null)
  let probe = $state<{ slug: string; result: ProbeResult } | null>(null)
  let pickerOpen = $state(false)
  let failed = $state(false)
  /** Last rejected model setting, named in the banner (docs/41 §4.3). */
  let invalid = $state<{ model: string | null; message: string } | null>(null)
  let enabledSaving = $state(false)
  let query = $state('')
  let filter = $state<(typeof FILTERS)[number]>('all')
  let addOpen = $state(false)
  let newName = $state('')
  let infoOpen = $state(false)
  let editName = $state('')
  let notesOpen = $state(false)
  let editNotes = $state('')
  /** Provider whose row menu is open (hover ⋯ or right-click). */
  let menuFor = $state<string | null>(null)
  let abort: AbortController | null = null
  let saving = false
  let again = false

  const visible = $derived(
    providers.filter(
      (p) =>
        p.name.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()) &&
        (filter === 'all' || (filter === 'enabled') === p.enabled),
    ),
  )
  const endpoint = $derived(draft?.endpoints[active] ?? null)

  function errorText(error: unknown): string {
    return isKitClientError(error) ? t(`errors.${error.code}`) : String(error)
  }
  const MODEL_FIELDS: Record<string, true> = {
    name: true,
    groupName: true,
    contextWindow: true,
    maxOutput: true,
    reasoning: true,
    pricing: true,
  }
  /** A rejected model setting: the backend names field + reason; the model is `entity` or found by the local rules. */
  function modelError(error: unknown, sent: ProviderDraft): { model: string | null; message: string } | null {
    if (!isKitClientError(error) || error.code !== 'bad_request') return null
    const { field, reason, entity } = error.details
    if (!field || !reason || !Object.hasOwn(MODEL_FIELDS, field)) return null
    const models = sent.endpoints.flatMap((e) => e.models.map((model) => ({ model, protocol: e.protocol })))
    const found =
      models.find(({ model }) => model.id === entity) ??
      models.find(({ model, protocol }) =>
        validateModel(model, protocol).some((item) => item.field === field && item.reason === reason),
      )
    const model = found ? (found.model.name ?? found.model.id) : null
    return { model, message: t(`providers.error.${field}.${reason}`) }
  }
  function status(p: Provider): string | null {
    if (!p.enabled) return null
    return p.endpoints.length === 0 || p.endpoints.some((e) => !e.keys.some((k) => k.enabled && k.readable))
      ? 'bg-warning'
      : 'bg-success'
  }
  function notifyChanged() {
    providersRevision.value++
    onchanged?.()
  }
  /** Key operations return the whole list; only the matching endpoint's stored record changes in the draft. */
  function keysChanged(next: Provider[]) {
    providers = next
    const stored = next.find((p) => p.id === draft?.id)
    for (const e of draft?.endpoints ?? []) {
      const record = stored?.endpoints.find((s) => s.id === e.saved?.id)
      if (record) e.saved = record
    }
    notifyChanged()
  }

  function load(next: ProviderDraft | null) {
    abort?.abort()
    check = null
    probe = null
    failed = false
    invalid = null
    draft = next
    active = 0
    token++
  }
  async function select(p: Provider) {
    if (p.id === draft?.id) return
    if (failed && !(await notify.confirm({ title: t('providers.discard_title'), body: t('providers.discard_body') })))
      return
    load(toDraft(p))
  }
  async function refresh() {
    if (!port) return
    try {
      providers = await port.list()
      // Backend no-ops a preset already present and lets a same-named provider adopt it (docs/39 §3).
      for (const preset of presets) {
        if (providers.some((p) => p.presetId === preset.id)) continue
        providers = await port.save({
          name: preset.name,
          presetId: preset.id,
          enabled: false,
          endpoints: [{ slug: 'ep-1', baseUrl: preset.baseUrl, protocol: preset.protocol, models: [] }],
        })
      }
      const target = settingsTarget.value
      const owner = target && providers.find((p) => p.endpoints.some((e) => e.id === target.endpointId))
      if (!owner) settingsTarget.value = null
      load(owner ? toDraft(owner) : providers[0] ? toDraft(providers[0]) : null)
      // EndpointPanel opens the target model's dialog when it mounts (and clears the target).
      if (owner && target && draft)
        active = Math.max(
          0,
          draft.endpoints.findIndex((e) => e.saved?.id === target.endpointId),
        )
    } catch (error) {
      notify.toast('error', errorText(error))
    }
  }
  const keyUrl = $derived(
    presets.find((preset) => preset.id === providers.find((p) => p.id === draft?.id)?.presetId)?.apiKeyUrl,
  )
  $effect(() => {
    if (port) void refresh()
    return () => abort?.abort()
  })

  /** Saves the whole draft; calls during a save coalesce into one more save of the latest draft. */
  async function commit() {
    if (!draft || !port) return
    if (saving) {
      again = true
      return
    }
    saving = true
    let last: ProviderDraft | null = null
    try {
      do {
        again = false
        const sent = $state.snapshot(savable(draft)) as ProviderDraft
        last = sent
        const before = new Set(providers.map((p) => p.id))
        providers = await port.save(toSave(sent))
        const stored = providers.find((p) => (sent.id === null ? !before.has(p.id) : p.id === sent.id))
        if (stored && draft && draft.id === sent.id) draft = adopt(draft, sent, stored)
        failed = false
        invalid = null
        notifyChanged()
      } while (again && draft)
    } catch (error) {
      failed = true
      invalid = last && modelError(error, last)
      if (!invalid) notify.toast('error', errorText(error))
    } finally {
      saving = false
    }
  }

  async function setEnabled(enabled: boolean) {
    const id = draft?.id
    if (!draft || !id || !port || enabledSaving) return
    const previous = draft.enabled
    draft.enabled = enabled
    enabledSaving = true
    try {
      providers = await port.setEnabled(id, enabled)
      notifyChanged()
    } catch (error) {
      if (draft?.id === id) draft.enabled = previous
      notify.toast('error', errorText(error))
    } finally {
      enabledSaving = false
    }
  }

  async function create() {
    const name = newName.trim()
    if (!name) return
    addOpen = false
    newName = ''
    load({ id: null, name, enabled: true, notes: '', endpoints: [newEndpoint([])] })
    await commit()
  }
  function rename() {
    const name = editName.trim()
    infoOpen = false
    if (!draft || !name || name === draft.name) return
    draft.name = name
    void commit()
  }
  /** Row menu actions act on that row, so select it first (honours the unsaved-changes prompt). */
  async function rowAction(p: Provider, action: 'edit' | 'notes' | 'delete') {
    menuFor = null
    await select(p)
    if (draft?.id !== p.id) return
    if (action === 'delete') return remove()
    editName = draft.name
    editNotes = draft.notes
    if (action === 'edit') infoOpen = true
    else notesOpen = true
  }
  function saveNotes() {
    notesOpen = false
    if (!draft || editNotes === draft.notes) return
    draft.notes = editNotes
    void commit()
  }
  async function remove() {
    const current = draft
    if (!current || !port) return
    infoOpen = false
    if (current.id === null) return load(providers[0] ? toDraft(providers[0]) : null)
    const body = t('providers.delete_confirm', { name: current.name })
    if (!(await notify.confirm({ title: t('providers.delete'), body, danger: true }))) return
    try {
      providers = await port.remove(current.id)
      load(providers[0] ? toDraft(providers[0]) : null)
      notifyChanged()
    } catch (error) {
      notify.toast('error', errorText(error))
    }
  }
  function addEndpoint() {
    if (!draft) return
    draft.endpoints.push(newEndpoint(draft.endpoints))
    active = draft.endpoints.length - 1
    // Stored right away so its keys can be added before it has an address (docs/34 §2).
    void commit()
  }
  async function removeEndpoint() {
    if (!draft || !endpoint) return
    const index = active
    if (
      endpoint.saved &&
      !(await notify.confirm({
        title: t('providers.remove_endpoint'),
        body: t('providers.remove_endpoint_confirm'),
        danger: true,
      }))
    )
      return
    const wasSaved = draft.endpoints[index]?.saved !== null
    draft.endpoints.splice(index, 1)
    active = 0
    if (wasSaved) void commit()
  }

  function outcome(result: ProbeResult): { state: 'ok'; count: number } | { state: 'error'; message: string } {
    const e = result.error
    if (!e) return { state: 'ok', count: result.models.length }
    const message = [
      t(`providers.probe.${e.kind}`, { status: e.status ?? '' }),
      e.hint ? t(e.hint) : '',
      e.message ?? '',
    ]
      .filter(Boolean)
      .join('\n')
    return { state: 'error', message }
  }
  async function runProbe(index: number, openPicker: boolean) {
    const target = draft?.endpoints[index]
    if (!target || !port) return
    abort?.abort()
    const controller = new AbortController()
    abort = controller
    const slug = target.slug
    check = { slug, value: { state: 'running' } }
    const enabled = target.saved?.keys.filter((k) => k.enabled) ?? []
    try {
      if (enabled.length >= 2) {
        // "All keys" check: one probe per enabled key, in order.
        const results = []
        let usable: ProbeResult | null = null
        for (const key of enabled) {
          const result = await port.probe(probeTarget(target, key.id), { signal: controller.signal })
          if (!result.error) usable ??= result
          results.push({ keyId: key.id, value: outcome(result) })
        }
        check = { slug, value: { state: 'keys', results } }
        if (openPicker && usable) {
          probe = { slug, result: usable }
          pickerOpen = true
        }
        return
      }
      const result = await port.probe(probeTarget(target), { signal: controller.signal })
      check = { slug, value: outcome(result) }
      if (!result.error && openPicker) {
        probe = { slug, result }
        pickerOpen = true
      }
    } catch (error) {
      check =
        isKitClientError(error) && error.code === 'cancelled'
          ? null
          : { slug, value: { state: 'error', message: errorText(error) } }
    }
  }
  const probed = $derived(probe ? (draft?.endpoints.find((e) => e.slug === probe?.slug) ?? null) : null)
</script>

{#if port}
  <div class="flex min-h-0 min-w-0 flex-col gap-4 sm:flex-row">
    <nav class="flex w-full shrink-0 flex-col gap-1 sm:w-60" aria-label={t('providers.title')}>
      <div class="mb-1 flex items-center gap-1.5">
        <div class="relative min-w-0 flex-1">
          <SearchIcon
            class="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2"
          />
          <Input
            class="pl-8"
            placeholder={t('providers.search')}
            aria-label={t('providers.search')}
            bind:value={query}
            autocomplete="off"
          />
        </div>
        <Select.Root type="single" value={filter} onValueChange={(value) => (filter = value as typeof filter)}>
          <Select.Trigger
            class={cn('shrink-0 px-2 [&>svg:last-child]:hidden', filter !== 'all' && 'bg-accent')}
            aria-label={t('providers.filter')}
            title={t(`providers.filter_${filter}`)}
          >
            <SlidersHorizontalIcon />
          </Select.Trigger>
          <Select.Content>
            <Select.Group>
              {#each FILTERS as f (f)}<Select.Item value={f} label={t(`providers.filter_${f}`)} />{/each}
            </Select.Group>
          </Select.Content>
        </Select.Root>
      </div>
      <ul class="flex min-w-0 flex-col gap-0.5">
        {#each visible as p (p.id)}
          {@const dot = status(p)}
          <li
            class="group/row relative"
            oncontextmenu={(event) => {
              event.preventDefault()
              menuFor = p.id
            }}
          >
            <button
              type="button"
              class={cn(
                'hover:bg-muted flex h-9 w-full min-w-0 items-center gap-2.5 rounded-kit px-2 pr-8 text-left text-kit-base',
                draft?.id === p.id && 'bg-muted font-medium',
              )}
              aria-current={draft?.id === p.id ? 'true' : undefined}
              onclick={() => void select(p)}
            >
              <span
                aria-hidden="true"
                class="bg-primary/10 text-primary flex size-6 shrink-0 items-center justify-center rounded-md text-kit-xs font-semibold"
                >{p.name.slice(0, 1).toUpperCase()}</span
              >
              <span class={cn('min-w-0 flex-1 truncate', !p.enabled && 'text-muted-foreground')}>{p.name}</span>
              {#if dot}<span class={cn('size-1.5 shrink-0 rounded-full', dot)}></span><span class="sr-only"
                  >{t(dot === 'bg-success' ? 'providers.filter_enabled' : 'providers.needs_setup')}</span
                >{/if}
            </button>
            <DropdownMenu.Root open={menuFor === p.id} onOpenChange={(open) => (menuFor = open ? p.id : null)}>
              <DropdownMenu.Trigger>
                {#snippet child({ props })}<button
                    {...props}
                    type="button"
                    class={cn(
                      'text-muted-foreground hover:text-foreground hover:bg-accent absolute top-1/2 right-1 inline-flex size-6 -translate-y-1/2 items-center justify-center rounded-md opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100',
                      menuFor === p.id && 'opacity-100',
                    )}
                    aria-label={t('providers.row_menu', { name: p.name })}
                    title={t('providers.row_menu', { name: p.name })}><EllipsisIcon class="size-4" /></button
                  >{/snippet}
              </DropdownMenu.Trigger>
              <DropdownMenu.Content align="start" class="min-w-36">
                <DropdownMenu.Item onSelect={() => void rowAction(p, 'edit')}
                  ><SquarePenIcon />{t('providers.edit')}</DropdownMenu.Item
                >
                <DropdownMenu.Item onSelect={() => void rowAction(p, 'notes')}
                  ><UserPenIcon />{t('providers.notes')}</DropdownMenu.Item
                >
                {#if !p.presetId}<DropdownMenu.Item variant="destructive" onSelect={() => void rowAction(p, 'delete')}
                    ><Trash2Icon />{t('providers.remove')}</DropdownMenu.Item
                  >{/if}
              </DropdownMenu.Content>
            </DropdownMenu.Root>
          </li>
        {:else}
          <li class="text-muted-foreground px-2 py-4 text-center text-kit-sm">{t('providers.no_matches')}</li>
        {/each}
      </ul>
      <Button variant="outline" class="mt-2" onclick={() => (addOpen = true)}><PlusIcon />{t('providers.add')}</Button>
    </nav>

    <section class="flex min-w-0 flex-1 flex-col gap-5">
      {#if draft}
        <header class="flex min-h-8 min-w-0 items-center gap-1.5">
          <h2 class="min-w-0 truncate text-[15px] font-semibold">{draft.name}</h2>
          <Button
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground"
            title={t('providers.settings')}
            aria-label={t('providers.settings')}
            onclick={() => {
              editName = draft?.name ?? ''
              infoOpen = true
            }}><Settings2Icon /></Button
          >
          <Switch
            id="kit-provider-enabled"
            class="ml-auto"
            aria-label={t('providers.enabled')}
            checked={draft.enabled}
            disabled={enabledSaving || draft.id === null}
            onCheckedChange={(enabled) => void setEnabled(enabled)}
          />
        </header>
        {#if failed}
          <p role="alert" class="text-destructive flex items-center gap-2 text-kit-sm">
            {invalid
              ? invalid.model
                ? t('providers.save_model_invalid', { model: invalid.model, error: invalid.message })
                : invalid.message
              : t('providers.save_failed')}
            <Button variant="link" size="sm" class="h-auto p-0" onclick={() => void commit()}
              >{t('providers.retry')}</Button
            >
          </p>
        {/if}
        {#if draft.endpoints.length > 1}
          <Field.Field orientation="horizontal" class="gap-2">
            <Field.Label for="kit-provider-endpoint" class="shrink-0">{t('providers.endpoints')}</Field.Label>
            <Select.Root type="single" value={String(active)} onValueChange={(value) => (active = Number(value))}>
              <Select.Trigger id="kit-provider-endpoint" class="min-w-0 flex-1">
                <span class="truncate">{endpoint?.baseUrl || t('providers.endpoint_new')}</span>
              </Select.Trigger>
              <Select.Content>
                <Select.Group>
                  {#each draft.endpoints as e, index (e.slug)}
                    <Select.Item value={String(index)} label={e.baseUrl || t('providers.endpoint_new')} />
                  {/each}
                </Select.Group>
              </Select.Content>
            </Select.Root>
          </Field.Field>
        {/if}
        {#if endpoint}
          {#key `${token}:${endpoint.slug}`}
            <EndpointPanel
              bind:endpoint={
                () => endpoint,
                (next) => {
                  if (draft) draft.endpoints[active] = next
                }
              }
              check={check?.slug === endpoint.slug ? check.value : null}
              onaddendpoint={draft.endpoints.length < 20 ? addEndpoint : null}
              onremove={() => void removeEndpoint()}
              oncommit={() => void commit()}
              onkeys={keysChanged}
              oncheck={() => void runProbe(active, false)}
              onsync={() => void runProbe(active, true)}
              {keyUrl}
              {usedBy}
            />
          {/key}
        {:else}
          <Button variant="outline" class="self-start" onclick={addEndpoint}>
            <PlusIcon />{t('providers.add_endpoint')}
          </Button>
        {/if}
      {:else}
        <Empty.Root>
          <Empty.Header>
            <Empty.Title>{t('providers.empty')}</Empty.Title>
            <Empty.Description>{t('providers.empty_hint')}</Empty.Description>
          </Empty.Header>
        </Empty.Root>
      {/if}
    </section>
  </div>

  <ModelPickerDialog
    bind:open={pickerOpen}
    result={probe?.result ?? null}
    selected={probed?.models ?? []}
    onchange={(models) => {
      if (!probed) return
      probed.models = models
      void commit()
    }}
  />

  <Dialog.Root bind:open={addOpen}>
    <Dialog.Content class="sm:max-w-md">
      <Dialog.Header><Dialog.Title>{t('providers.add')}</Dialog.Title></Dialog.Header>
      <form
        class="contents"
        onsubmit={(event) => {
          event.preventDefault()
          void create()
        }}
      >
        <Field.Field>
          <Field.Label for="kit-provider-new">{t('providers.name')}</Field.Label>
          <Input id="kit-provider-new" maxlength={80} bind:value={newName} />
        </Field.Field>
        <Dialog.Footer>
          <Button type="button" variant="outline" onclick={() => (addOpen = false)}>{t('common.cancel')}</Button>
          <Button type="submit" disabled={!newName.trim()}>{t('common.confirm')}</Button>
        </Dialog.Footer>
      </form>
    </Dialog.Content>
  </Dialog.Root>

  <Dialog.Root bind:open={infoOpen}>
    <Dialog.Content class="sm:max-w-md">
      <Dialog.Header><Dialog.Title>{t('providers.settings')}</Dialog.Title></Dialog.Header>
      <form
        class="contents"
        onsubmit={(event) => {
          event.preventDefault()
          rename()
        }}
      >
        <Field.Field>
          <Field.Label for="kit-provider-name">{t('providers.name')}</Field.Label>
          <Input id="kit-provider-name" maxlength={80} bind:value={editName} />
        </Field.Field>
        <Dialog.Footer class="sm:justify-between">
          {#if !providers.find((p) => p.id === draft?.id)?.presetId}<Button
              type="button"
              variant="ghost"
              class="text-destructive"
              onclick={() => void remove()}>{t('providers.delete')}</Button
            >{:else}<span></span>{/if}
          <Button type="submit" disabled={!editName.trim()}>{t('providers.save')}</Button>
        </Dialog.Footer>
      </form>
    </Dialog.Content>
  </Dialog.Root>

  <Dialog.Root bind:open={notesOpen}>
    <Dialog.Content class="sm:max-w-lg">
      <Dialog.Header><Dialog.Title>{t('providers.notes')}</Dialog.Title></Dialog.Header>
      <form
        class="contents"
        onsubmit={(event) => {
          event.preventDefault()
          saveNotes()
        }}
      >
        <Textarea
          class="min-h-64 font-mono text-kit-sm"
          aria-label={t('providers.notes')}
          placeholder={t('providers.notes_placeholder')}
          maxlength={10000}
          bind:value={editNotes}
        />
        <Dialog.Footer>
          <Button type="button" variant="outline" onclick={() => (notesOpen = false)}>{t('common.cancel')}</Button>
          <Button type="submit">{t('providers.save')}</Button>
        </Dialog.Footer>
      </form>
    </Dialog.Content>
  </Dialog.Root>
{/if}
