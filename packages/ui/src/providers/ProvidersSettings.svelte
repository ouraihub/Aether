<script lang="ts">
  // Providers on the left, the selected one's name + endpoint cards on the right (docs/16 §4.1). No roles.
  import * as Alert from '#lib/components/ui/alert/index.js'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { getKitContext } from '../context'
  import { isKitClientError, type ProbeResult, type Provider } from '../ports'
  import { cn } from '../utils'
  import { newEndpoint, probeTarget, toDraft, toSave, type ProviderDraft } from './draft'
  import EndpointCard from './EndpointCard.svelte'
  import ModelPickerDialog from './ModelPickerDialog.svelte'

  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const port = ports.providers

  let providers = $state<Provider[]>([])
  let draft = $state<ProviderDraft | null>(null)
  /** Snapshot of `draft` when it was loaded, for the unsaved-changes bar. */
  let baseline = $state('')
  let expanded = $state(0)
  let saving = $state(false)
  let probe = $state<{ index: number; result: ProbeResult | null; error: string | null } | null>(null)
  let pickerOpen = $state(false)
  let abort: AbortController | null = null

  const dirty = $derived(draft !== null && JSON.stringify(draft) !== baseline)

  function load(next: ProviderDraft | null) {
    abort?.abort()
    probe = null
    draft = next
    baseline = JSON.stringify(next)
    expanded = 0
  }

  function errorText(error: unknown): string {
    return isKitClientError(error) ? t(`errors.${error.code}`) : String(error)
  }

  async function refresh(selectId: string | null) {
    if (!port) return
    try {
      providers = await port.list()
      const selected = providers.find((p) => p.id === selectId) ?? providers[0]
      load(selected ? toDraft(selected) : null)
    } catch (error) {
      notify.toast('error', errorText(error))
    }
  }

  $effect(() => {
    if (port) void refresh(null)
    return () => abort?.abort()
  })

  function status(p: Provider): string {
    if (p.endpoints.some((e) => !e.hasKey || !e.keyReadable)) return 'bg-chart-4'
    if (p.endpoints.some((e) => e.models.length > 0)) return 'bg-chart-2'
    return 'bg-muted-foreground'
  }

  function add() {
    load({ id: null, name: t('providers.new_name'), endpoints: [newEndpoint(0)] })
    baseline = ''
  }

  async function save() {
    if (!draft || !port) return
    saving = true
    try {
      const before = new Set(providers.map((p) => p.id))
      providers = await port.save(toSave(draft))
      const id = draft.id ?? providers.find((p) => !before.has(p.id))?.id ?? null
      const saved = providers.find((p) => p.id === id)
      load(saved ? toDraft(saved) : null)
      notify.toast('success', t('providers.saved'))
    } catch (error) {
      notify.toast('error', errorText(error))
    } finally {
      saving = false
    }
  }

  async function remove() {
    if (!draft || !port) return
    if (draft.id === null) return load(null)
    const ok = await notify.confirm({
      title: t('providers.delete'),
      body: t('providers.delete_confirm', { name: draft.name }),
      danger: true,
    })
    if (!ok) return
    try {
      providers = await port.remove(draft.id)
      load(providers[0] ? toDraft(providers[0]) : null)
    } catch (error) {
      notify.toast('error', errorText(error))
    }
  }

  async function fetchModels(index: number) {
    const endpoint = draft?.endpoints[index]
    if (!endpoint || !port) return
    abort?.abort()
    const controller = new AbortController()
    abort = controller
    probe = { index, result: null, error: null }
    try {
      const result = await port.probe(probeTarget(endpoint), { signal: controller.signal })
      if (result.error) {
        const e = result.error
        const lines = [
          t(`providers.probe.${e.kind}`, { status: e.status ?? '' }),
          e.hint ? t(e.hint) : '',
          e.message ?? '',
        ]
        probe = { index, result: null, error: lines.filter(Boolean).join('\n') }
      } else {
        probe = { index, result, error: null }
        pickerOpen = true
      }
    } catch (error) {
      probe =
        isKitClientError(error) && error.code === 'cancelled' ? null : { index, result: null, error: errorText(error) }
    }
  }
</script>

{#if port}
  <div class="flex min-h-0 gap-4">
    <nav class="flex w-52 shrink-0 flex-col gap-1" aria-label={t('providers.title')}>
      {#each providers as p (p.id)}
        <Button
          variant={draft?.id === p.id ? 'secondary' : 'ghost'}
          class="justify-start"
          onclick={() => load(toDraft(p))}
        >
          <span class={cn('size-2 shrink-0 rounded-full', status(p))} aria-hidden="true"></span>
          <span class="truncate">{p.name}</span>
        </Button>
      {/each}
      <Button variant="outline" onclick={add}>{t('providers.add')}</Button>
    </nav>
    <section class="flex min-w-0 flex-1 flex-col gap-4">
      {#if draft}
        {#if dirty}
          <Alert.Root>
            <Alert.Title>{t('providers.unsaved')}</Alert.Title>
            <Alert.Description>
              <div class="flex gap-2">
                <Button size="sm" disabled={saving} onclick={() => void save()}>{t('providers.save')}</Button>
                <Button size="sm" variant="outline" onclick={() => void refresh(draft?.id ?? null)}>
                  {t('providers.discard')}
                </Button>
              </div>
            </Alert.Description>
          </Alert.Root>
        {/if}
        <Field.Field>
          <Field.Label for="kit-provider-name">{t('providers.name')}</Field.Label>
          <Input id="kit-provider-name" maxlength={80} bind:value={draft.name} />
        </Field.Field>
        <h3 class="text-sm font-medium">{t('providers.endpoints')}</h3>
        {#each draft.endpoints as endpoint, index (index)}
          <EndpointCard
            bind:endpoint={
              () => endpoint,
              (next) => {
                if (draft) draft.endpoints[index] = next
              }
            }
            {index}
            expanded={expanded === index}
            fetching={probe?.index === index && !probe.result && !probe.error}
            ontoggle={() => (expanded = expanded === index ? -1 : index)}
            onfetch={() => void fetchModels(index)}
            onremove={() => draft?.endpoints.splice(index, 1)}
          />
          {#if probe?.index === index && probe.error}
            <Alert.Root variant="destructive">
              <Alert.Description class="whitespace-pre-line">{probe.error}</Alert.Description>
            </Alert.Root>
          {/if}
        {/each}
        <div class="flex gap-2">
          <Button
            variant="outline"
            disabled={draft.endpoints.length >= 20}
            onclick={() => {
              draft?.endpoints.push(newEndpoint(draft.endpoints.length))
              expanded = (draft?.endpoints.length ?? 1) - 1
            }}
          >
            {t('providers.add_endpoint')}
          </Button>
          <Button variant="ghost" onclick={() => void remove()}>{t('providers.delete')}</Button>
        </div>
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
    selected={probe ? (draft?.endpoints[probe.index]?.models ?? []) : []}
    onapply={(models) => {
      const endpoint = probe && draft?.endpoints[probe.index]
      if (endpoint) endpoint.models = models
    }}
  />
{/if}
