<script lang="ts">
  // External MCP server settings (docs/16 §4.4): status, start/stop, port,
  // scope switches, tokens.
  import { onDestroy } from 'svelte'
  import {
    Alert,
    AlertDescription,
    AlertTitle,
  } from '#lib/components/ui/alert/index.js'
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Switch } from '#lib/components/ui/switch/index.js'
  import { getKitContext } from '../context'
  import { isKitClientError } from '../ports'
  import type { IssuedToken, ScopeState, Status, TokenRecord } from '../ports'
  import { createClientSession } from '../session'
  import { buildClaudePosixCommand, buildMcpJsonConfig } from './config'

  /** Server key used in the copied client configs. */
  let { serverName = 'aether' }: { serverName?: string } = $props()

  const { ports, i18n, notify, logger } = getKitContext()
  const server = ports.mcpServer
  const t = i18n.t

  let status = $state<Status | null>(null)
  let scopes = $state<ScopeState[]>([])
  let tokens = $state<TokenRecord[]>([])
  let port = $state('')
  let tokenName = $state('')
  let tokenScopes = $state<string[]>([])
  let issued = $state<IssuedToken | null>(null)
  let busy = $state(false)
  let issuing = $state(false)
  const tokenSession = createClientSession()
  let tokenRevision = 0

  function clearIssued() {
    tokenSession.invalidate()
    issued = null
    issuing = false
  }

  $effect(() => {
    void serverName
    clearIssued()
    tokenName = ''
    tokenScopes = []
    tokenRevision++
  })

  onDestroy(() => {
    clearIssued()
    tokenName = ''
    tokenScopes = []
  })

  function hasHttpUrl(value: string | undefined): value is string {
    if (!value?.trim()) return false
    try {
      const url = new URL(value)
      return url.protocol === 'http:' || url.protocol === 'https:'
    } catch {
      return false
    }
  }

  // JSON accepts dynamic keys; Claude CLI names must also satisfy its naming
  // rules.
  const configInput = $derived.by(() => {
    const url = status?.url
    const token = issued?.token
    if (!hasHttpUrl(url) || !serverName.trim() || !token?.trim()) return null
    return { serverName, url, token }
  })
  const claudeNameValid = $derived(/^[A-Za-z0-9_-]+$/.test(serverName))

  // Actions only render when the port exists; the throw marks a host wiring
  // bug.
  function need() {
    if (!server) throw new Error('mcpServer port missing')
    return server
  }

  function fail(error: unknown) {
    const code = isKitClientError(error) ? error.code : 'internal'
    logger.warn('mcp.server.ui_failed', { code })
    notify.toast('error', t(`errors.${code}`))
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

  function setStatus(next: Status) {
    if (status && status.url !== next.url) clearIssued()
    status = next
    port = String(next.port)
  }

  async function load() {
    if (!server) return
    const [nextStatus, nextScopes, nextTokens] = await Promise.all([
      server.status(),
      server.scopes(),
      server.tokens(),
    ])
    setStatus(nextStatus)
    scopes = nextScopes
    tokens = nextTokens
  }

  $effect(() => {
    void act(load)
  })

  const toggleRunning = () =>
    act(async () =>
      setStatus(status?.running ? await need().stop() : await need().start()),
    )

  const savePort = () =>
    act(async () => setStatus(await need().setPort(Number(port))))

  async function toggleScope(scope: ScopeState, on: boolean) {
    if (on && scope.destructive) {
      const ok = await notify.confirm({
        title: t('mcp.server.destructiveTitle'),
        body: t('mcp.server.destructiveBody', { scope: t(scope.labelKey) }),
        danger: true,
      })
      if (!ok) return
    }
    const ids = scopes
      .filter((s) => (s.id === scope.id ? on : s.enabled))
      .map((s) => s.id)
    await act(async () => {
      scopes = await need().setScopes(ids)
    })
  }

  function toggleTokenScope(id: string, on: boolean) {
    tokenRevision++
    tokenScopes = on
      ? [...tokenScopes, id]
      : tokenScopes.filter((s) => s !== id)
  }

  async function issue() {
    clearIssued()
    const token = tokenSession.capture()
    const targetName = serverName
    const targetUrl = status?.url
    const revision = tokenRevision
    const name = tokenName
    const grants = [...tokenScopes]
    const current = () =>
      tokenSession.isCurrent(token) &&
      targetName === serverName &&
      targetUrl === status?.url
    issuing = true
    try {
      const next = await need().issueToken(name, grants)
      if (!current()) return
      issued = next
      if (revision === tokenRevision) {
        tokenName = ''
        tokenScopes = []
      }
      const nextTokens = await need().tokens()
      if (current()) tokens = nextTokens
    } catch (error) {
      if (current()) fail(error)
    } finally {
      if (current()) issuing = false
    }
  }

  const revoke = (token: TokenRecord) =>
    act(async () => {
      const ok = await notify.confirm({
        title: t('mcp.server.revokeTitle'),
        body: t('mcp.server.revokeBody', { name: token.name }),
        danger: true,
      })
      if (!ok) return
      if (issued?.id === token.id) clearIssued()
      await need().revokeToken(token.id)
      tokens = await need().tokens()
    })

  async function copy(format: 'json' | 'posix') {
    const input = configInput
    if (!input || (format === 'posix' && !claudeNameValid)) return
    const token = tokenSession.capture()
    const current = () =>
      tokenSession.isCurrent(token) &&
      serverName === input.serverName &&
      status?.url === input.url &&
      issued?.token === input.token
    const text =
      format === 'json'
        ? buildMcpJsonConfig(input)
        : buildClaudePosixCommand(input)
    if (!current()) return
    try {
      await navigator.clipboard.writeText(text)
      if (current()) notify.toast('success', t('mcp.server.copied'))
    } catch (error) {
      if (current()) fail(error)
    }
  }

  const date = (ms: number | null) =>
    ms === null ? '—' : new Date(ms).toLocaleString(i18n.locale)
</script>

{#if server && status}
  <section class="flex flex-col gap-6" aria-label={t('mcp.server.title')}>
    <Field.Group>
      <Field.Field orientation="horizontal">
        <Field.Content>
          <Field.Label>{t('mcp.server.title')}</Field.Label>
          <Field.Description>
            {status.running
              ? t('mcp.server.running', { url: status.url })
              : t('mcp.server.stopped')}
            · {t('mcp.server.toolCount', { count: status.toolCount })}
          </Field.Description>
        </Field.Content>
        <Button
          disabled={busy || issuing}
          variant={status.running ? 'outline' : 'default'}
          onclick={() => void toggleRunning()}
        >
          {status.running ? t('mcp.server.stop') : t('mcp.server.start')}
        </Button>
      </Field.Field>
      <Field.Field orientation="horizontal">
        <Field.Label for="kit-mcp-port">{t('mcp.server.port')}</Field.Label>
        <Input
          id="kit-mcp-port"
          class="w-32"
          type="number"
          min="1024"
          max="65535"
          bind:value={port}
        />
        <Button
          disabled={busy || issuing || port === String(status.port)}
          variant="outline"
          onclick={() => void savePort()}
        >
          {t('mcp.server.savePort')}
        </Button>
      </Field.Field>
    </Field.Group>

    <Field.Set>
      <Field.Legend>{t('mcp.server.scopes')}</Field.Legend>
      <Field.Description>{t('mcp.server.scopesHint')}</Field.Description>
      <Field.Group>
        {#each scopes as scope (scope.id)}
          <Field.Field orientation="horizontal">
            <Switch
              id={`kit-mcp-scope-${scope.id}`}
              bind:checked={
                () => scope.enabled, (on) => void toggleScope(scope, on)
              }
              disabled={busy || issuing}
            />
            <Field.Label for={`kit-mcp-scope-${scope.id}`}
              >{t(scope.labelKey)} ({scope.id})</Field.Label
            >
          </Field.Field>
        {/each}
      </Field.Group>
    </Field.Set>

    <Field.Set>
      <Field.Legend>{t('mcp.server.newToken')}</Field.Legend>
      <Field.Group>
        <Field.Field>
          <Field.Label for="kit-mcp-token-name"
            >{t('mcp.server.tokenName')}</Field.Label
          >
          <Input
            id="kit-mcp-token-name"
            bind:value={tokenName}
            maxlength={200}
            oninput={() => tokenRevision++}
          />
        </Field.Field>
        {#each scopes as scope (scope.id)}
          <Field.Field orientation="horizontal">
            <Switch
              id={`kit-mcp-grant-${scope.id}`}
              checked={tokenScopes.includes(scope.id)}
              onCheckedChange={(on) => toggleTokenScope(scope.id, on)}
            />
            <Field.Label for={`kit-mcp-grant-${scope.id}`}
              >{t(scope.labelKey)} ({scope.id})</Field.Label
            >
          </Field.Field>
        {/each}
        <Field.Field orientation="horizontal">
          <Button
            disabled={busy ||
              issuing ||
              tokenName.trim() === '' ||
              tokenScopes.length === 0}
            onclick={() => void issue()}
          >
            {t('mcp.server.issue')}
          </Button>
        </Field.Field>
      </Field.Group>
    </Field.Set>

    {#if issued}
      <Alert>
        <AlertTitle
          >{t('mcp.server.issuedTitle', { name: issued.name })}</AlertTitle
        >
        <AlertDescription class="flex flex-col gap-2">
          <p>{t('mcp.server.issuedOnce')}</p>
          <code class="break-all">{issued.token}</code>
          <div class="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={!configInput || !claudeNameValid}
              onclick={() => void copy('posix')}
            >
              {t('mcp.server.copyClaude')}
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={!configInput}
              onclick={() => void copy('json')}
            >
              {t('mcp.server.copyJson')}
            </Button>
            <Button size="sm" variant="ghost" onclick={clearIssued}
              >{t('mcp.server.dismiss')}</Button
            >
          </div>
        </AlertDescription>
      </Alert>
    {/if}

    <Field.Set>
      <Field.Legend>{t('mcp.server.tokens')}</Field.Legend>
      {#if tokens.length === 0}
        <Field.Description>{t('mcp.server.noTokens')}</Field.Description>
      {/if}
      <ul class="flex flex-col gap-2">
        {#each tokens as token (token.id)}
          <li
            class="flex items-center justify-between gap-4 rounded-md border
              p-3"
          >
            <div class="flex min-w-0 flex-col">
              <span class="truncate font-medium">{token.name}</span>
              <span class="text-muted-foreground text-sm">
                {token.scopes.join(', ')} · {t('mcp.server.lastUsed')}: {date(
                  token.lastUsedAt,
                )}
              </span>
            </div>
            {#if token.revokedAt === null}
              <Button
                size="sm"
                variant="destructive"
                disabled={busy || issuing}
                onclick={() => void revoke(token)}
              >
                {t('mcp.server.revoke')}
              </Button>
            {:else}
              <span class="text-muted-foreground text-sm"
                >{t('mcp.server.revoked')}</span
              >
            {/if}
          </li>
        {/each}
      </ul>
    </Field.Set>
  </section>
{/if}
