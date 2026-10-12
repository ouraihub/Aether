<script lang="ts">
  // One endpoint: collapsed summary or the edit form (docs/16 §4.1). The parent keeps one card expanded.
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down'
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { getKitContext } from '../context'
  import type { Protocol } from '../ports'
  import { probeUrl, type EndpointDraft } from './draft'
  import SecretInput from './SecretInput.svelte'

  let {
    endpoint = $bindable(),
    index,
    expanded,
    fetching,
    ontoggle,
    onfetch,
    onremove,
  }: {
    endpoint: EndpointDraft
    index: number
    expanded: boolean
    fetching: boolean
    ontoggle: () => void
    onfetch: () => void
    onremove: () => void
  } = $props()

  const PROTOCOLS: Protocol[] = ['openai-chat', 'openai-responses', 'anthropic-messages']
  const { t } = getKitContext().i18n
  const prefix = $derived(`kit-endpoint-${index}`)
</script>

<div class="flex flex-col gap-3 rounded-kit border p-3">
  <button
    type="button"
    class="flex items-center gap-2 text-left text-kit-base"
    aria-expanded={expanded}
    onclick={ontoggle}
  >
    {#if expanded}<ChevronDownIcon />{:else}<ChevronRightIcon />{/if}
    <span class="font-medium">{endpoint.slug || '—'}</span>
    <span class="text-muted-foreground flex-1 truncate">{endpoint.baseUrl}</span>
    <span class="text-muted-foreground">{t('providers.model_count', { count: endpoint.models.length })}</span>
  </button>
  {#if expanded}
    <Field.Group>
      <div class="flex gap-3">
        <Field.Field class="w-32">
          <Field.Label for="{prefix}-slug">{t('providers.slug')}</Field.Label>
          <Input id="{prefix}-slug" bind:value={endpoint.slug} />
        </Field.Field>
        <Field.Field class="flex-1">
          <Field.Label for="{prefix}-protocol">{t('providers.protocol')}</Field.Label>
          <Select.Root type="single" bind:value={endpoint.protocol}>
            <Select.Trigger id="{prefix}-protocol" class="w-full">{endpoint.protocol}</Select.Trigger>
            <Select.Content>
              <Select.Group>
                {#each PROTOCOLS as protocol (protocol)}
                  <Select.Item value={protocol} label={protocol} />
                {/each}
              </Select.Group>
            </Select.Content>
          </Select.Root>
        </Field.Field>
      </div>
      <Field.Field>
        <Field.Label for="{prefix}-url">{t('providers.base_url')}</Field.Label>
        <Input id="{prefix}-url" placeholder="https://api.example.com/v1" bind:value={endpoint.baseUrl} />
        {#if endpoint.baseUrl.trim()}
          <Field.Description class="font-mono">→ GET {probeUrl(endpoint.baseUrl)}</Field.Description>
        {/if}
      </Field.Field>
      <Field.Field>
        <Field.Label for="{prefix}-key">{t('providers.api_key')}</Field.Label>
        <SecretInput id="{prefix}-key" saved={endpoint.saved} bind:value={endpoint.key} />
      </Field.Field>
      <Field.Field>
        <Field.Label>{t('providers.models')}</Field.Label>
        <Field.Description>
          {endpoint.models.length === 0 ? t('providers.no_models') : endpoint.models.map((m) => m.id).join(', ')}
        </Field.Description>
      </Field.Field>
    </Field.Group>
    <div class="flex gap-2">
      <Button size="sm" disabled={fetching || !endpoint.baseUrl.trim()} onclick={onfetch}>
        {t('providers.fetch_models')}
      </Button>
      <Button size="sm" variant="ghost" onclick={onremove}>{t('providers.remove_endpoint')}</Button>
    </div>
  {/if}
</div>
