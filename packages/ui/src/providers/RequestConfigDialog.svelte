<script lang="ts">
  // Cherry's 请求配置 (ProviderCustomHeaderDrawer) for one endpoint: edits a local copy, Save writes it back (docs/36 §4).
  import BracesIcon from '@lucide/svelte/icons/braces'
  import ListIcon from '@lucide/svelte/icons/list'
  import PlusIcon from '@lucide/svelte/icons/plus'
  import TrashIcon from '@lucide/svelte/icons/trash-2'
  import { untrack } from 'svelte'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import CopyInput from '../CopyInput.svelte'
  import * as Select from '#lib/components/ui/select/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import { getKitContext } from '../context'
  import type { Protocol } from '../ports'
  import { chatUrl, checkHeaders, validUrl, type EndpointDraft } from './draft'

  let {
    open = $bindable(false),
    endpoint = $bindable(),
    onaddendpoint,
    onremove,
    oncommit,
  }: {
    open: boolean
    endpoint: EndpointDraft
    onaddendpoint: (() => void) | null
    onremove: () => void
    oncommit: () => void
  } = $props()

  const PROTOCOLS: Protocol[] = ['openai-chat', 'openai-responses', 'anthropic-messages']
  const { i18n } = getKitContext()
  const { t } = i18n

  type Row = { name: string; value: string }
  let protocol = $state<Protocol>('openai-chat')
  let baseUrl = $state('')
  let imageBaseUrl = $state('')
  let imageEditBaseUrl = $state('')
  let rows = $state<Row[]>([])
  let json = $state<string | null>(null)
  let error = $state<string | null>(null)

  // Copy the endpoint into the local draft each time the dialog opens.
  $effect.pre(() => {
    if (!open) return
    untrack(() => {
      protocol = endpoint.protocol
      baseUrl = endpoint.baseUrl
      imageBaseUrl = endpoint.imageBaseUrl ?? ''
      imageEditBaseUrl = endpoint.imageEditBaseUrl ?? ''
      rows = Object.entries(endpoint.headers).map(([name, value = '']) => ({ name, value }))
      json = null
      error = null
    })
  })

  /** JSON text → rows; `null` when it is not an object of strings. */
  function parse(text: string): Row[] | null {
    try {
      const value: unknown = JSON.parse(text.trim() || '{}')
      if (typeof value !== 'object' || value === null || Array.isArray(value)) return null
      const entries = Object.entries(value)
      return entries.every(([, v]) => typeof v === 'string')
        ? entries.map(([name, v]) => ({ name, value: v as string }))
        : null
    } catch {
      return null
    }
  }
  function toggle() {
    error = null
    if (json === null) {
      json = JSON.stringify(Object.fromEntries(rows.map((r) => [r.name, r.value])), null, 2)
      return
    }
    const parsed = parse(json)
    if (!parsed) error = t('providers.headers_invalid_json')
    else {
      rows = parsed
      json = null
    }
  }
  /** Same rule as the backend `image_base`: optional, http(s), ≤2048 bytes. */
  const badImage = (url: string) => !!url && (!validUrl(url) || new TextEncoder().encode(url).length > 2048)
  function save() {
    error = null
    const url = baseUrl.trim()
    const image = imageBaseUrl.trim()
    const imageEdit = imageEditBaseUrl.trim()
    if ((url && !validUrl(url)) || badImage(image) || badImage(imageEdit)) {
      error = t('providers.url_invalid')
      return
    }
    const list = json === null ? rows : parse(json)
    if (!list) {
      error = t('providers.headers_invalid_json')
      return
    }
    const checked = checkHeaders(list)
    if ('error' in checked) {
      error =
        checked.error.key === 'headers_reserved'
          ? t('providers.headers_reserved', { name: checked.error.name })
          : t(`providers.${checked.error.key}`)
      return
    }
    endpoint.protocol = protocol
    endpoint.baseUrl = url
    endpoint.imageBaseUrl = image || null
    endpoint.imageEditBaseUrl = imageEdit || null
    endpoint.headers = checked.headers
    open = false
    oncommit()
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{t('providers.request_config')}</Dialog.Title>
    </Dialog.Header>
    <Field.Group>
      <Field.Field>
        <Field.Label for="kit-endpoint-protocol">{t('providers.protocol')}</Field.Label>
        <Select.Root type="single" value={protocol} onValueChange={(value) => (protocol = value as Protocol)}>
          <Select.Trigger id="kit-endpoint-protocol" class="w-full">{protocol}</Select.Trigger>
          <Select.Content>
            <Select.Group>
              {#each PROTOCOLS as p (p)}<Select.Item value={p} label={p} />{/each}
            </Select.Group>
          </Select.Content>
        </Select.Root>
      </Field.Field>
      <Field.Field>
        <div class="flex items-center gap-2">
          <Field.Label for="kit-request-url">{t('providers.base_url')}</Field.Label>
          <span class="bg-primary/10 text-primary rounded-kit px-1.5 text-kit-xs">{t('providers.default')}</span>
        </div>
        <CopyInput
          id="kit-request-url"
          class="font-mono"
          placeholder="https://api.example.com/v1"
          bind:value={baseUrl}
        />
        {#if validUrl(baseUrl)}
          <Field.Description class="truncate font-mono text-kit-xs"
            >{t('providers.base_url_hint', { url: `POST ${chatUrl(baseUrl, protocol)}` })}</Field.Description
          >
        {/if}
      </Field.Field>
      {#if onaddendpoint}
        <Button
          variant="outline"
          class="w-full border-dashed"
          onclick={() => {
            open = false
            onaddendpoint()
          }}><PlusIcon />{t('providers.add_endpoint')}</Button
        >
      {/if}
      <Field.Field>
        <Field.Label for="kit-request-image-url">{t('providers.image_base_url')}</Field.Label>
        <CopyInput
          id="kit-request-image-url"
          class="font-mono"
          placeholder="https://example.com"
          bind:value={imageBaseUrl}
        />
        <Field.Description class="text-kit-xs">{t('providers.image_base_url_hint')}</Field.Description>
      </Field.Field>
      <Field.Field>
        <Field.Label for="kit-request-image-edit-url">{t('providers.image_edit_base_url')}</Field.Label>
        <CopyInput
          id="kit-request-image-edit-url"
          class="font-mono"
          placeholder="https://example.com"
          bind:value={imageEditBaseUrl}
        />
        <Field.Description class="text-kit-xs">{t('providers.image_edit_base_url_hint')}</Field.Description>
      </Field.Field>
      <Field.Field>
        <div class="flex items-center justify-between gap-2">
          <Field.Label for={json === null ? undefined : 'kit-request-headers'}>{t('providers.headers')}</Field.Label>
          <Button
            variant="ghost"
            size="icon"
            title={json === null ? t('providers.headers_to_json') : t('providers.headers_to_list')}
            aria-label={json === null ? t('providers.headers_to_json') : t('providers.headers_to_list')}
            onclick={toggle}
            >{#if json === null}<BracesIcon />{:else}<ListIcon />{/if}</Button
          >
        </div>
        {#if json === null}
          {#each rows as row, index (index)}
            <div class="flex min-w-0 items-center gap-2">
              <Input
                class="min-w-0 flex-1 font-mono"
                aria-label={t('providers.header_name')}
                placeholder={t('providers.header_name')}
                bind:value={row.name}
              />
              <Input
                class="min-w-0 flex-1 font-mono"
                aria-label={t('providers.header_value')}
                placeholder={t('providers.header_value')}
                bind:value={row.value}
              />
              <Button
                variant="ghost"
                size="icon"
                aria-label={t('providers.header_remove', { name: row.name })}
                onclick={() => rows.splice(index, 1)}><TrashIcon /></Button
              >
            </div>
          {/each}
          <Button
            variant="ghost"
            class="self-start"
            disabled={rows.length >= 20}
            onclick={() => rows.push({ name: '', value: '' })}><PlusIcon />{t('providers.headers_add')}</Button
          >
        {:else}
          <Textarea id="kit-request-headers" class="min-h-32 font-mono text-xs" spellcheck={false} bind:value={json} />
        {/if}
      </Field.Field>
    </Field.Group>
    {#if error}
      <p role="alert" class="text-destructive text-kit-xs">{error}</p>
    {/if}
    <Dialog.Footer class="sm:justify-between">
      <Button
        variant="ghost"
        class="text-destructive"
        onclick={() => {
          open = false
          onremove()
        }}>{t('providers.remove_endpoint')}</Button
      >
      <div class="flex gap-2">
        <Button variant="outline" onclick={() => (open = false)}>{t('common.cancel')}</Button>
        <Button onclick={save}>{t('providers.save')}</Button>
      </div>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
