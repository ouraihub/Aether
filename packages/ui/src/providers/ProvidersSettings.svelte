<script lang="ts">
  // Provider settings (docs/33): list on the left, detail on the right, every field saves itself.
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import Settings2Icon from '@lucide/svelte/icons/settings-2'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import { getKitContext } from '../context'
  import { isKitClientError, type ProbeResult, type Provider, type ProviderPreset } from '../ports'
  import { newEndpoint, savable, toDraft, validateModel, type ProviderDraft } from './draft'
  import { createProviderEditSession, type ProviderEditSession } from './edit-session'
  import EndpointPanel, { type CheckOutcome, type CheckState } from './EndpointPanel.svelte'
  import ModelPickerDialog from './ModelPickerDialog.svelte'
  import ProviderDialogs from './ProviderDialogs.svelte'
  import ProviderList from './ProviderList.svelte'
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
  let addOpen = $state(false)
  let newName = $state('')
  let infoOpen = $state(false)
  let editName = $state('')
  let notesOpen = $state(false)
  let editNotes = $state('')
  let editSession: ProviderEditSession | null = null
  let probeGeneration = 0

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
  function modelError(
    error: unknown,
    sent: ProviderDraft,
  ): { model: string | null; message: string } | null {
    if (!isKitClientError(error) || error.code !== 'bad_request') return null
    const { field, reason, entity } = error.details
    if (!field || !reason || !Object.hasOwn(MODEL_FIELDS, field)) return null
    const models = sent.endpoints.flatMap((e) =>
      e.models.map((model) => ({ model, protocol: e.protocol })),
    )
    const found =
      models.find(({ model }) => model.id === entity) ??
      models.find(({ model, protocol }) =>
        validateModel(model, protocol).some(
          (item) => item.field === field && item.reason === reason,
        ),
      )
    const model = found ? (found.model.name ?? found.model.id) : null
    return { model, message: t(`providers.error.${field}.${reason}`) }
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
    editSession?.invalidate()
    editSession = null
    probeGeneration++
    pickerOpen = false
    check = null
    probe = null
    failed = false
    invalid = null
    draft = next
    active = 0
    token++
    if (next && port)
      editSession = createProviderEditSession(port, {
        readDraft: () => {
          if (!draft) throw new Error('Provider edit session has no draft')
          return draft
        },
        writeDraft: (next) => {
          draft = next
        },
        readProviders: () => providers,
        onSaved: (next) => {
          providers = next
          notifyChanged()
        },
      })
  }
  async function select(p: Provider) {
    if (p.id === draft?.id) return
    if (
      failed &&
      !(await notify.confirm({
        title: t('providers.discard_title'),
        body: t('providers.discard_body'),
      }))
    )
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
          endpoints: [
            { slug: 'ep-1', baseUrl: preset.baseUrl, protocol: preset.protocol, models: [] },
          ],
        })
      }
      const target = settingsTarget.value
      const owner =
        target && providers.find((p) => p.endpoints.some((e) => e.id === target.endpointId))
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
    presets.find((preset) => preset.id === providers.find((p) => p.id === draft?.id)?.presetId)
      ?.apiKeyUrl,
  )
  $effect(() => {
    if (port) void refresh()
    return () => {
      editSession?.invalidate()
      probeGeneration++
    }
  })

  /** Presentation only; the ordinary TS session owns the queue and its save barrier. */
  async function commit() {
    const session = editSession
    if (!session || !draft) return
    const sent = $state.snapshot(savable(draft)) as ProviderDraft
    try {
      await session.commit()
      if (editSession !== session) return
      failed = false
      invalid = null
    } catch (error) {
      if (editSession !== session || (isKitClientError(error) && error.code === 'cancelled')) return
      failed = true
      invalid = modelError(error, sent)
      if (!invalid) notify.toast('error', errorText(error))
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
    const session = editSession
    if (!current || !port) return
    infoOpen = false
    if (current.id === null) return load(providers[0] ? toDraft(providers[0]) : null)
    const body = t('providers.delete_confirm', { name: current.name })
    if (!(await notify.confirm({ title: t('providers.delete'), body, danger: true }))) return
    if (editSession !== session) return
    session?.invalidate()
    probeGeneration++
    pickerOpen = false
    try {
      const next = await port.remove(current.id)
      if (editSession !== session) return
      providers = next
      load(providers[0] ? toDraft(providers[0]) : null)
      notifyChanged()
    } catch (error) {
      if (editSession !== session) return
      load(current)
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

  function outcome(result: ProbeResult): CheckOutcome {
    const e = result.error
    if (!e) return { state: 'ok', count: result.models.length, suggested: result.suggestedProtocol }
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
    const session = editSession
    if (!target || !session) return
    const generation = ++probeGeneration
    const slug = target.slug
    check = { slug, value: { state: 'running' } }
    try {
      const batch = await session.probe(slug)
      if (session !== editSession || generation !== probeGeneration) return
      failed = false
      invalid = null
      const first = batch.results[0]
      check = {
        slug,
        value:
          first?.keyId === null
            ? outcome(first.result)
            : {
                state: 'keys',
                results: batch.results.flatMap(({ keyId, result }) =>
                  keyId === null ? [] : [{ keyId, value: outcome(result) }],
                ),
              },
      }
      if (openPicker && batch.usable) {
        probe = { slug, result: batch.usable }
        pickerOpen = true
      }
    } catch (error) {
      if (session !== editSession || generation !== probeGeneration) return
      check =
        isKitClientError(error) && error.code === 'cancelled'
          ? null
          : { slug, value: { state: 'error', message: errorText(error) } }
    }
  }
  const probed = $derived(
    probe ? (draft?.endpoints.find((e) => e.slug === probe?.slug) ?? null) : null,
  )
</script>

{#if port}
  <div class="flex min-h-0 min-w-0 flex-col gap-4 sm:flex-row">
    <ProviderList
      {providers}
      selectedId={draft?.id ?? null}
      onselect={(provider) => void select(provider)}
      onaction={(provider, action) => void rowAction(provider, action)}
      onadd={() => (addOpen = true)}
    />

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
                ? t('providers.save_model_invalid', {
                    model: invalid.model,
                    error: invalid.message,
                  })
                : invalid.message
              : t('providers.save_failed')}
            <Button variant="link" size="sm" class="h-auto p-0" onclick={() => void commit()}
              >{t('providers.retry')}</Button
            >
          </p>
        {/if}
        {#if draft.endpoints.length > 1}
          <Field.Field orientation="horizontal" class="gap-2">
            <Field.Label for="kit-provider-endpoint" class="shrink-0"
              >{t('providers.endpoints')}</Field.Label
            >
            <Select.Root
              type="single"
              value={String(active)}
              onValueChange={(value) => (active = Number(value))}
            >
              <Select.Trigger id="kit-provider-endpoint" class="min-w-0 flex-1">
                <span class="truncate">{endpoint?.baseUrl || t('providers.endpoint_new')}</span>
              </Select.Trigger>
              <Select.Content>
                <Select.Group>
                  {#each draft.endpoints as e, index (e.slug)}
                    <Select.Item
                      value={String(index)}
                      label={e.baseUrl || t('providers.endpoint_new')}
                    />
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
  <ProviderDialogs
    bind:addOpen
    bind:infoOpen
    bind:notesOpen
    bind:newName
    bind:editName
    bind:editNotes
    {draft}
    {providers}
    {create}
    {rename}
    {saveNotes}
    {remove}
  />
{/if}
