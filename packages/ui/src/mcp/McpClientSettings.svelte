<script lang="ts">
  // External MCP servers the in-app AI may call (docs/16 §4.4, docs/13 §2): list, edit with
  // write-only credentials, test connection, trust/enable/private-network switches, and each
  // tool's approval default. Only enabled + trusted servers' tools reach a run.
  import { Alert, AlertDescription } from '#lib/components/ui/alert/index.js'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import CopyInput from '../CopyInput.svelte'
  import * as Select from '#lib/components/ui/select/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import { getKitContext } from '../context'
  import { isKitClientError } from '../ports'
  import type { Approval, ServerDto, ServerSave, ServerSecret } from '../ports'

  const APPROVALS: Approval[] = ['auto', 'ask', 'deny']

  const { ports, i18n, notify, logger } = getKitContext()
  const client = ports.mcpClient
  const t = i18n.t

  let servers = $state<ServerDto[]>([])
  let busy = $state(false)
  /** Last "test connection" failure per server id. */
  let testErrors = $state<Record<string, string>>({})

  let editing = $state(false)
  let editId = $state<string | null>(null)
  let name = $state('')
  let slug = $state('')
  let url = $state('')
  let replaceSecret = $state(true)
  let headers = $state('')
  let query = $state('')
  let currentKeys = $state<string[]>([])

  // Actions only render when the port exists; the throw marks a host wiring bug.
  function need() {
    if (!client) throw new Error('mcpClient port missing')
    return client
  }

  function errorText(error: unknown): string {
    if (!isKitClientError(error)) return t('errors.internal')
    const { reason, field } = error.details
    if (reason === 'credential_query') return t('mcp.client.err.credentialQuery')
    if (reason === 'duplicate_slug') return t('mcp.client.err.duplicateSlug')
    return field ? `${t(`errors.${error.code}`)} (${field})` : t(`errors.${error.code}`)
  }

  function fail(error: unknown) {
    logger.warn('mcp.client.ui_failed', { code: isKitClientError(error) ? error.code : 'internal' })
    notify.toast('error', errorText(error))
  }

  async function act(run: () => Promise<void>) {
    busy = true
    try {
      await run()
    } catch (error) {
      fail(error)
    } finally {
      busy = false
    }
  }

  $effect(() => {
    if (client) void act(async () => void (servers = await client.list()))
  })

  function base(server: ServerDto): ServerSave {
    const { id, slug, name, url, enabled, trusted, allowPrivate } = server
    return { id, slug, name, url, enabled, trusted, allowPrivate }
  }

  const update = (next: ServerSave) => act(async () => void (servers = await need().save(next)))

  async function setTrusted(server: ServerDto, on: boolean) {
    if (on) {
      const ok = await notify.confirm({
        title: t('mcp.client.trustTitle'),
        body: t('mcp.client.trustBody', { name: server.name }),
      })
      if (!ok) return
    }
    await update({ ...base(server), trusted: on })
  }

  async function setPrivate(server: ServerDto, on: boolean) {
    if (on) {
      const ok = await notify.confirm({
        title: t('mcp.client.privateTitle'),
        body: t('mcp.client.privateBody', { url: server.url }),
        danger: true,
      })
      if (!ok) return
    }
    await update({ ...base(server), allowPrivate: on })
  }

  const test = (server: ServerDto) =>
    act(async () => {
      testErrors = Object.fromEntries(Object.entries(testErrors).filter(([id]) => id !== server.id))
      try {
        await need().refresh(server.id)
      } catch (error) {
        testErrors = { ...testErrors, [server.id]: errorText(error) }
      }
      servers = await need().list()
    })

  const setApproval = (server: ServerDto, tool: string, approval: Approval) =>
    act(async () => {
      await need().setToolApproval(server.id, tool, approval)
      servers = await need().list()
    })

  const remove = (server: ServerDto) =>
    act(async () => {
      const ok = await notify.confirm({
        title: t('mcp.client.deleteTitle'),
        body: t('mcp.client.deleteBody', { name: server.name }),
        danger: true,
      })
      if (ok) servers = await need().remove(server.id)
    })

  function edit(server: ServerDto | null) {
    editId = server?.id ?? null
    name = server?.name ?? ''
    slug = server?.slug ?? ''
    url = server?.url ?? ''
    currentKeys = server?.secretKeys ?? []
    replaceSecret = server === null
    headers = ''
    query = ''
    editing = true
  }

  /** `Name: value` (headers) or `name=value` (query) lines; null on a malformed line. */
  function pairs(text: string, separator: string): Record<string, string> | null {
    const out: Record<string, string> = {}
    for (const line of text.split('\n').map((l) => l.trim())) {
      if (!line) continue
      const at = line.indexOf(separator)
      if (at <= 0) return null
      out[line.slice(0, at).trim()] = line.slice(at + 1).trim()
    }
    return out
  }

  function save() {
    const current = servers.find((s) => s.id === editId)
    const input: ServerSave = current
      ? { ...base(current), name: name.trim(), slug: slug.trim(), url: url.trim() }
      : { slug: slug.trim(), name: name.trim(), url: url.trim(), enabled: false, trusted: false, allowPrivate: false }
    if (replaceSecret) {
      const h = pairs(headers, ':')
      const q = pairs(query, '=')
      if (!h || !q) {
        notify.toast('error', t('mcp.client.err.secretLine'))
        return
      }
      const secret: ServerSecret = {}
      if (Object.keys(h).length) secret.headers = h
      if (Object.keys(q).length) secret.query = q
      // A new server without credentials sends none; an edit with nothing typed clears them.
      if (current || secret.headers || secret.query) input.secret = secret
    }
    void act(async () => {
      servers = await need().save(input)
      editing = false
    })
  }
</script>

{#if client}
  <section class="flex flex-col gap-4" aria-label={t('mcp.client.title')}>
    <div class="flex items-center gap-2">
      <p class="text-muted-foreground flex-1 text-sm">{t('mcp.client.hint')}</p>
      <Button disabled={busy} onclick={() => edit(null)}>{t('mcp.client.add')}</Button>
    </div>
    {#if servers.length === 0}
      <Empty.Root>
        <Empty.Header>
          <Empty.Title>{t('mcp.client.empty')}</Empty.Title>
          <Empty.Description>{t('mcp.client.emptyHint')}</Empty.Description>
        </Empty.Header>
      </Empty.Root>
    {/if}
    <ul class="flex flex-col gap-3">
      {#each servers as server (server.id)}
        <li class="flex flex-col gap-3 rounded-md border p-3" aria-label={server.name}>
          <div class="flex items-start gap-3">
            <div class="flex min-w-0 flex-1 flex-col">
              <span class="truncate font-medium"
                >{server.name} <span class="text-muted-foreground">({server.slug})</span></span
              >
              <span class="text-muted-foreground truncate font-mono text-xs">{server.url}</span>
              {#if server.secretKeys.length}
                <span class="text-muted-foreground text-xs">
                  {t('mcp.client.secretKeys', { keys: server.secretKeys.join(', ') })}
                </span>
              {/if}
            </div>
            <Button size="sm" variant="outline" disabled={busy} onclick={() => void test(server)}>
              {t('mcp.client.test')}
            </Button>
            <Button size="sm" variant="outline" disabled={busy} onclick={() => edit(server)}>
              {t('mcp.client.edit')}
            </Button>
            <Button size="sm" variant="destructive" disabled={busy} onclick={() => void remove(server)}>
              {t('mcp.client.delete')}
            </Button>
          </div>
          <div class="flex flex-wrap gap-6">
            <Field.Field orientation="horizontal" class="w-auto">
              <Switch
                id={`kit-mcp-enabled-${server.id}`}
                checked={server.enabled}
                disabled={busy}
                onCheckedChange={(on) => void update({ ...base(server), enabled: on })}
              />
              <Field.Label for={`kit-mcp-enabled-${server.id}`}>{t('mcp.client.enabled')}</Field.Label>
            </Field.Field>
            <Field.Field orientation="horizontal" class="w-auto">
              <Switch
                id={`kit-mcp-trusted-${server.id}`}
                checked={server.trusted}
                disabled={busy}
                onCheckedChange={(on) => void setTrusted(server, on)}
              />
              <Field.Label for={`kit-mcp-trusted-${server.id}`}>{t('mcp.client.trusted')}</Field.Label>
            </Field.Field>
            <Field.Field orientation="horizontal" class="w-auto">
              <Switch
                id={`kit-mcp-private-${server.id}`}
                checked={server.allowPrivate}
                disabled={busy}
                onCheckedChange={(on) => void setPrivate(server, on)}
              />
              <Field.Label for={`kit-mcp-private-${server.id}`}>{t('mcp.client.allowPrivate')}</Field.Label>
            </Field.Field>
          </div>
          {#if testErrors[server.id]}
            <Alert variant="destructive">
              <AlertDescription>{t('mcp.client.testFailed', { error: testErrors[server.id] ?? '' })}</AlertDescription>
            </Alert>
          {/if}
          {#if server.tools.length}
            <ul class="flex flex-col gap-2" aria-label={t('mcp.client.tools')}>
              {#each server.tools as tool (tool.name)}
                <li class="flex items-center gap-3">
                  <div class="flex min-w-0 flex-1 flex-col">
                    <span class="truncate font-mono text-sm">{tool.name}</span>
                    <span class="text-muted-foreground truncate text-xs">
                      {tool.description}{#if tool.readOnly && tool.approval !== 'auto'}
                        · {t('mcp.client.readOnlyHint')}{/if}
                    </span>
                  </div>
                  <Select.Root
                    type="single"
                    value={tool.approval}
                    onValueChange={(value) => void setApproval(server, tool.name, value as Approval)}
                  >
                    <Select.Trigger
                      size="sm"
                      class="w-28"
                      aria-label={t('mcp.client.approvalFor', { tool: tool.name })}
                    >
                      {t(`mcp.client.approval.${tool.approval}`)}
                    </Select.Trigger>
                    <Select.Content>
                      <Select.Group>
                        {#each APPROVALS as approval (approval)}
                          <Select.Item value={approval} label={t(`mcp.client.approval.${approval}`)} />
                        {/each}
                      </Select.Group>
                    </Select.Content>
                  </Select.Root>
                </li>
              {/each}
            </ul>
          {:else}
            <p class="text-muted-foreground text-sm">{t('mcp.client.noTools')}</p>
          {/if}
        </li>
      {/each}
    </ul>
  </section>

  <Dialog.Root bind:open={editing}>
    <Dialog.Content class="sm:max-w-xl">
      <Dialog.Header>
        <Dialog.Title>{editId ? t('mcp.client.editTitle') : t('mcp.client.add')}</Dialog.Title>
      </Dialog.Header>
      <Field.Group>
        <div class="flex gap-3">
          <Field.Field class="flex-1">
            <Field.Label for="kit-mcp-name">{t('mcp.client.name')}</Field.Label>
            <Input id="kit-mcp-name" maxlength={200} bind:value={name} />
          </Field.Field>
          <Field.Field class="w-40">
            <Field.Label for="kit-mcp-slug">{t('mcp.client.slug')}</Field.Label>
            <Input id="kit-mcp-slug" class="font-mono" maxlength={24} bind:value={slug} />
          </Field.Field>
        </div>
        <Field.Field>
          <Field.Label for="kit-mcp-url">{t('mcp.client.url')}</Field.Label>
          <CopyInput id="kit-mcp-url" class="font-mono" placeholder="https://example.com/mcp" bind:value={url} />
          <Field.Description>{t('mcp.client.urlHint')}</Field.Description>
        </Field.Field>
        {#if editId}
          <Field.Field orientation="horizontal">
            <Switch id="kit-mcp-replace" bind:checked={replaceSecret} />
            <Field.Label for="kit-mcp-replace">
              {t('mcp.client.replaceSecret', { keys: currentKeys.join(', ') || '—' })}
            </Field.Label>
          </Field.Field>
        {/if}
        {#if replaceSecret}
          <Field.Field>
            <Field.Label for="kit-mcp-headers">{t('mcp.client.headers')}</Field.Label>
            <Textarea
              id="kit-mcp-headers"
              class="font-mono text-xs"
              placeholder="Authorization: Bearer …"
              bind:value={headers}
            />
          </Field.Field>
          <Field.Field>
            <Field.Label for="kit-mcp-query">{t('mcp.client.query')}</Field.Label>
            <Textarea id="kit-mcp-query" class="font-mono text-xs" placeholder="ak=…" bind:value={query} />
            <Field.Description>{t('mcp.client.secretHint')}</Field.Description>
          </Field.Field>
        {/if}
      </Field.Group>
      <Dialog.Footer>
        <Button variant="outline" onclick={() => (editing = false)}>{t('common.cancel')}</Button>
        <Button disabled={busy || !name.trim() || !slug.trim() || !url.trim()} onclick={save}
          >{t('mcp.client.save')}</Button
        >
      </Dialog.Footer>
    </Dialog.Content>
  </Dialog.Root>
{/if}
