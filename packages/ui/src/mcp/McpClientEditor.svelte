<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import CopyInput from '../CopyInput.svelte'
  import * as Select from '#lib/components/ui/select/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import { getKitContext } from '../context'
  import type { McpTransport } from '../ports'

  let {
    open = $bindable(false),
    editId,
    name = $bindable(''),
    slug = $bindable(''),
    kind = $bindable<McpTransport['kind']>('http'),
    url = $bindable(''),
    command = $bindable(''),
    args = $bindable(''),
    cwd = $bindable(''),
    replaceSecret = $bindable(true),
    headers = $bindable(''),
    query = $bindable(''),
    env = $bindable(''),
    currentKeys,
    saving,
    onclose,
    onkind,
    onreplace,
    onsave,
    oninput,
    onsecret,
  }: {
    open: boolean
    editId: string | null
    name: string
    slug: string
    kind: McpTransport['kind']
    url: string
    command: string
    args: string
    cwd: string
    replaceSecret: boolean
    headers: string
    query: string
    env: string
    currentKeys: string[]
    saving: boolean
    onclose: () => void
    onkind: (kind: McpTransport['kind']) => void
    onreplace: (value: boolean) => void
    onsave: () => void
    oninput: () => void
    onsecret: () => void
  } = $props()
  const { t } = getKitContext().i18n
  const kinds: McpTransport['kind'][] = ['http', 'stdio']
  const argsPlaceholder = '-y\n@modelcontextprotocol/server-filesystem'
</script>

<Dialog.Root {open} onOpenChange={(value) => !value && onclose()}>
  <Dialog.Content class="sm:max-w-xl" oninput={oninput}>
    <Dialog.Header>
      <Dialog.Title>
        {editId ? t('mcp.client.editTitle') : t('mcp.client.add')}
      </Dialog.Title>
    </Dialog.Header>
    <Field.Group>
      <div class="flex gap-3">
        <Field.Field class="flex-1">
          <Field.Label for="kit-mcp-name">{t('mcp.client.name')}</Field.Label>
          <Input id="kit-mcp-name" maxlength={200} bind:value={name} />
        </Field.Field>
        <Field.Field class="w-40">
          <Field.Label for="kit-mcp-slug">{t('mcp.client.slug')}</Field.Label>
          <Input
            id="kit-mcp-slug"
            class="font-mono"
            maxlength={24}
            bind:value={slug}
          />
        </Field.Field>
      </div>
      <Field.Field>
        <Field.Label for="kit-mcp-transport">
          {t('mcp.client.transport')}
        </Field.Label>
        <Select.Root
          type="single"
          value={kind}
          onValueChange={(value) => onkind(value as McpTransport['kind'])}
        >
          <Select.Trigger id="kit-mcp-transport" class="w-full">
            {t(`mcp.client.transport.${kind}`)}
          </Select.Trigger>
          <Select.Content>
            <Select.Group>
              {#each kinds as option (option)}
                <Select.Item
                  value={option}
                  label={t(`mcp.client.transport.${option}`)}
                />
              {/each}
            </Select.Group>
          </Select.Content>
        </Select.Root>
      </Field.Field>
      {#if kind === 'http'}
        <Field.Field>
          <Field.Label for="kit-mcp-url">{t('mcp.client.url')}</Field.Label>
          <CopyInput
            id="kit-mcp-url"
            class="font-mono"
            placeholder="https://example.com/mcp"
            bind:value={url}
          />
          <Field.Description>{t('mcp.client.urlHint')}</Field.Description>
        </Field.Field>
      {:else}
        <Field.Field>
          <Field.Label for="kit-mcp-command">
            {t('mcp.client.command')}
          </Field.Label>
          <Input
            id="kit-mcp-command"
            class="font-mono"
            maxlength={1024}
            placeholder="npx"
            bind:value={command}
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="kit-mcp-args">{t('mcp.client.args')}</Field.Label>
          <Textarea
            id="kit-mcp-args"
            class="font-mono text-xs"
            placeholder={argsPlaceholder}
            bind:value={args}
          />
          <Field.Description>{t('mcp.client.argsHint')}</Field.Description>
        </Field.Field>
        <Field.Field>
          <Field.Label for="kit-mcp-cwd">{t('mcp.client.cwd')}</Field.Label>
          <Input id="kit-mcp-cwd" class="font-mono" bind:value={cwd} />
          <Field.Description>{t('mcp.client.cwdHint')}</Field.Description>
        </Field.Field>
      {/if}
      {#if editId}
        <Field.Field orientation="horizontal">
          <Switch
            id="kit-mcp-replace"
            checked={replaceSecret}
            onCheckedChange={onreplace}
          />
          <Field.Label for="kit-mcp-replace">
            {t('mcp.client.replaceSecret', {
              keys: currentKeys.join(', ') || '—',
            })}
          </Field.Label>
        </Field.Field>
      {/if}
      {#if replaceSecret && kind === 'http'}
        <Field.Field>
          <Field.Label for="kit-mcp-headers">
            {t('mcp.client.headers')}
          </Field.Label>
          <Textarea
            id="kit-mcp-headers"
            class="font-mono text-xs"
            placeholder="Authorization: Bearer …"
            bind:value={headers}
            oninput={onsecret}
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="kit-mcp-query">{t('mcp.client.query')}</Field.Label>
          <Textarea
            id="kit-mcp-query"
            class="font-mono text-xs"
            placeholder="ak=…"
            bind:value={query}
            oninput={onsecret}
          />
          <Field.Description>{t('mcp.client.secretHint')}</Field.Description>
        </Field.Field>
      {:else if replaceSecret}
        <Field.Field>
          <Field.Label for="kit-mcp-env">{t('mcp.client.env')}</Field.Label>
          <Textarea
            id="kit-mcp-env"
            class="font-mono text-xs"
            placeholder="GITHUB_TOKEN=…"
            bind:value={env}
            oninput={onsecret}
          />
          <Field.Description>{t('mcp.client.envHint')}</Field.Description>
        </Field.Field>
      {/if}
    </Field.Group>
    <Dialog.Footer>
      <Button variant="outline" onclick={onclose}>
        {t('common.cancel')}
      </Button>
      <Button
        disabled={saving || !name.trim() || !slug.trim() ||
          !(kind === 'http' ? url.trim() : command.trim())}
        onclick={onsave}
      >{t('mcp.client.save')}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
