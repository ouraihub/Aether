<script lang="ts">
  // One endpoint's keys and address.
  // Fields edit the draft in place; `oncommit` asks the parent to save (docs/33 §3); keys save on their own (docs/34).
  import LoaderIcon from '@lucide/svelte/icons/loader-circle'
  import Settings2Icon from '@lucide/svelte/icons/settings-2'
  import { Button } from '#lib/components/ui/button/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import CopyInput from '../CopyInput.svelte'
  import * as Select from '#lib/components/ui/select/index.js'
  import { getKitContext } from '../context'
  import type { KeyStrategy, Protocol, Provider } from '../ports'
  import { cn } from '../utils'
  import { keyError, maskedText, probeUrl, validUrl, type EndpointDraft } from './draft'
  import RequestConfigDialog from './RequestConfigDialog.svelte'
  import { settingsTarget } from './revision.svelte'
  import SecretInput from './SecretInput.svelte'
  import EndpointModels from './EndpointModels.svelte'

  export type CheckOutcome =
    { state: 'ok'; count: number; suggested: Protocol | null } | { state: 'error'; message: string }
  export type CheckState =
    | { state: 'running' }
    | CheckOutcome
    /** One probe per enabled key when there are at least two (docs/34 §5.3). */
    | { state: 'keys'; results: { keyId: string; value: CheckOutcome }[] }

  let {
    endpoint = $bindable(),
    check,
    onaddendpoint,
    onremove,
    oncommit,
    onkeys,
    oncheck,
    onsync,
    keyUrl,
    usedBy,
  }: {
    endpoint: EndpointDraft
    check: CheckState | null
    onaddendpoint: (() => void) | null
    onremove: () => void
    oncommit: () => void
    /** Providers returned by a key operation. */
    onkeys: (providers: Provider[]) => void
    oncheck: () => void
    onsync: () => void
    /** Preset "get a key" link shown beside the label (docs/39). */
    keyUrl?: string | undefined
    /** Localized roles bound to a stored model (contract §6). */
    usedBy?: ((endpointId: string, modelId: string) => string[]) | undefined
  } = $props()
  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  let keySaving = $state(false)
  async function setKeyEnabled(keyId: string, enabled: boolean) {
    const endpointId = endpoint.saved?.id
    if (!ports.providers || !endpointId || keySaving) return
    keySaving = true
    try {
      onkeys(await ports.providers.saveKey({ endpointId, id: keyId, enabled }))
    } catch (error) {
      notify.toast('error', keyError(error, t))
    } finally {
      keySaving = false
    }
  }

  let url = $state(endpoint.baseUrl)
  let urlError = $state(false)
  // A save replaces the draft object; only a changed committed address may overwrite what the user is typing.
  let seenUrl = endpoint.baseUrl
  $effect.pre(() => {
    const next = endpoint.baseUrl
    if (next === seenUrl) return
    seenUrl = next
    url = next
    urlError = false
  })
  let settingsOpen = $state(false)
  // A chat "open settings" action names this endpoint's model: consume it once.
  const targeted = settingsTarget.value?.endpointId === endpoint.saved?.id
  const targetModel = targeted
    ? (endpoint.models.find((model) => model.id === settingsTarget.value?.modelId) ?? null)
    : null
  if (targeted) settingsTarget.value = null
  function commitUrl() {
    const next = url.trim()
    if (next === endpoint.baseUrl) return
    if (next && !validUrl(next)) {
      urlError = true
      return
    }
    urlError = false
    endpoint.baseUrl = next
    oncommit()
  }
  /** First probe suggestion: the whole check or any key's (all keys share one relay). */
  const suggested = $derived.by((): Protocol | null => {
    if (check?.state === 'ok') return check.suggested
    if (check?.state !== 'keys') return null
    for (const { value } of check.results)
      if (value.state === 'ok' && value.suggested) return value.suggested
    return null
  })
</script>

<div class="flex min-w-0 flex-col gap-5">
  <Field.Field class="gap-2">
    <div class="flex items-center gap-2">
      <Field.Label for="kit-endpoint-key">{t('providers.api_key')}</Field.Label>
      {#if keyUrl && /^https:\/\//.test(keyUrl)}<a
          class="text-primary text-kit-xs hover:underline"
          href={keyUrl}
          target="_blank"
          rel="noopener noreferrer">{t('providers.get_key')}</a
        >{/if}
    </div>
    <div class="flex min-w-0 items-start gap-2">
      <SecretInput id="kit-endpoint-key" endpoint={endpoint.saved} onchanged={onkeys} />
      <Button
        variant="outline"
        class="shrink-0"
        disabled={!validUrl(endpoint.baseUrl)}
        onclick={oncheck}
      >
        {#if check?.state === 'running'}<LoaderIcon class="animate-spin" />{/if}
        {t('providers.check')}
      </Button>
    </div>
    {#if check?.state === 'keys'}
      <ul role="status" class="flex min-w-0 flex-col gap-1">
        {#each check.results as { keyId, value } (keyId)}
          {@const key = endpoint.saved?.keys.find((k) => k.id === keyId)}
          {#if key}
            <li class="flex min-w-0 items-start gap-2 text-kit-xs">
              <span
                class={cn(
                  'min-w-0 flex-1 whitespace-pre-line',
                  value.state === 'ok' ? 'text-success' : 'text-destructive',
                )}
                >{value.state === 'ok' ? '✓' : '✕'}
                {key.label || maskedText(key, t)} — {value.state === 'ok'
                  ? t('providers.check_ok', { count: value.count })
                  : value.message}</span
              >
              <Switch
                size="sm"
                aria-label={t('providers.keys_enable_for', {
                  name: key.label || maskedText(key, t),
                })}
                checked={key.enabled}
                disabled={keySaving}
                onCheckedChange={(enabled) => void setKeyEnabled(key.id, enabled)}
              />
            </li>
          {/if}
        {/each}
      </ul>
    {:else if check && check.state !== 'running'}
      <p
        role="status"
        class={cn(
          'text-kit-xs whitespace-pre-line',
          check.state === 'ok' ? 'text-success' : 'text-destructive',
        )}
      >
        {check.state === 'ok' ? t('providers.check_ok', { count: check.count }) : check.message}
      </p>
    {/if}
    {#if suggested && suggested !== endpoint.protocol}
      {@const next = suggested}
      <div
        role="status"
        class="bg-info/10 flex min-w-0 items-center gap-2 rounded-md px-2.5 py-1.5 text-kit-xs"
      >
        <span class="min-w-0 flex-1">{t('providers.suggest_protocol')}</span>
        <Button
          variant="outline"
          size="sm"
          class="shrink-0"
          onclick={() => {
            endpoint.protocol = next
            oncommit()
          }}>{t('providers.suggest_protocol_apply')}</Button
        >
      </div>
    {/if}
  </Field.Field>

  {#if ports.providers?.keyStrategies}
    <Field.Field class="gap-2">
      <Field.Label for="kit-endpoint-key-strategy">{t('providers.key_strategy')}</Field.Label>
      <Select.Root
        type="single"
        value={endpoint.keyStrategy}
        onValueChange={(value) => {
          if (value === endpoint.keyStrategy) return
          endpoint.keyStrategy = value as KeyStrategy
          oncommit()
        }}
      >
        <Select.Trigger id="kit-endpoint-key-strategy" class="w-full sm:w-64"
          >{t(`providers.key_strategy_${endpoint.keyStrategy}`)}</Select.Trigger
        >
        <Select.Content>
          <Select.Group>
            {#each ['round_robin', 'fill_first'] as const as strategy (strategy)}<Select.Item
                value={strategy}
                label={t(`providers.key_strategy_${strategy}`)}
              />{/each}
          </Select.Group>
        </Select.Content>
      </Select.Root>
      <Field.Description class="text-kit-xs"
        >{t(`providers.key_strategy_${endpoint.keyStrategy}_desc`)}</Field.Description
      >
    </Field.Field>
  {/if}

  <Field.Field class="gap-2">
    <div class="flex items-center gap-2">
      <Field.Label for="kit-endpoint-url">{t('providers.base_url')}</Field.Label>
      {#if onaddendpoint}
        <button
          type="button"
          class="text-primary text-kit-xs hover:underline"
          onclick={onaddendpoint}
        >
          {t('providers.add_endpoint')}
        </button>
      {/if}
    </div>
    <div class="flex min-w-0 items-center gap-2">
      <CopyInput
        id="kit-endpoint-url"
        class="min-w-0 flex-1 font-mono"
        placeholder="https://api.example.com/v1"
        aria-invalid={urlError}
        bind:value={url}
        onblur={commitUrl}
        onkeydown={(event) => event.key === 'Enter' && event.currentTarget.blur()}
      />
      <Button
        variant="outline"
        size="icon"
        class="shrink-0"
        title={t('providers.request_config_tip')}
        aria-label={t('providers.request_config_tip')}
        onclick={() => (settingsOpen = true)}><Settings2Icon /></Button
      >
    </div>
    {#if urlError}
      <p role="alert" class="text-destructive text-kit-xs">{t('providers.url_invalid')}</p>
    {:else if validUrl(endpoint.baseUrl)}
      <p class="text-muted-foreground truncate font-mono text-kit-xs">
        GET {probeUrl(endpoint.baseUrl)}
      </p>
    {/if}
  </Field.Field>

  <EndpointModels {endpoint} {onsync} {oncommit} {usedBy} {targetModel} />
</div>

<RequestConfigDialog bind:open={settingsOpen} bind:endpoint {onaddendpoint} {onremove} {oncommit} />
