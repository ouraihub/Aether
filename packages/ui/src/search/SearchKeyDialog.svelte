<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { getKitContext } from '../context'
  import type { SearchKey, SearchProvider } from '../ports'

  let {
    dialog = $bindable<HTMLDialogElement>(),
    provider, busy, password, editValue, editingId, revealed, error,
    revealNeedsPassword, onclose, onpassword, onedit, oneditstart, onsave,
    onreveal, onremove, oncancel,
  }: {
    dialog: HTMLDialogElement
    provider: SearchProvider | null
    busy: boolean
    password: string
    editValue: string
    editingId: string | undefined
    revealed: Record<string, string>
    error: string
    revealNeedsPassword: boolean
    onclose: () => void
    onpassword: (value: string) => void
    onedit: (value: string) => void
    oneditstart: (key: SearchKey) => void
    onsave: () => void
    onreveal: (key: SearchKey, copy?: boolean) => void
    onremove: (key: SearchKey) => void
    oncancel: () => void
  } = $props()
  const { t } = getKitContext().i18n
</script>

  <dialog
    bind:this={dialog}
    aria-labelledby="search-key-dialog-title"
    onclose={onclose}
    class="bg-background text-foreground border-border m-auto max-h-[80dvh]
      w-[min(560px,calc(100vw-32px))] overflow-y-auto rounded-kit border p-4"
  >
  <h2 id="search-key-dialog-title" class="text-kit-base font-semibold">
    {t('search.manageKeys')}{#if provider} · {provider.name}{/if}
  </h2>
  {#if provider}
    <Field.Group class="mt-3 gap-3">
      {#if revealNeedsPassword}
        <Field.Field>
          <Field.Label for="search-key-password">
            {t('search.currentPassword')}
          </Field.Label>
          <Input
            id="search-key-password"
            type="password"
            autocomplete="off"
            disabled={busy}
            value={password}
            oninput={(event) => onpassword(event.currentTarget.value)}
          />
        </Field.Field>
      {/if}
      <ul class="flex flex-col gap-3">
        {#each provider.keys as key (key.id)}
          <li class="flex min-w-0 flex-col gap-1 border-b pb-2 text-kit-sm">
            <span class="break-all font-mono">
              {revealed[key.id] ?? key.masked}
            </span>
            <div class="flex flex-wrap gap-1">
              <Button type="button" variant="ghost" size="sm" disabled={busy}
                onclick={() => oneditstart(key)}>{t('search.editKey')}</Button>
              <Button type="button" variant="ghost" size="sm"
                disabled={busy || (revealNeedsPassword && !password)}
                onclick={() => onreveal(key)}>{t('search.reveal')}</Button>
              <Button type="button" variant="ghost" size="sm"
                disabled={busy || (revealNeedsPassword && !password)}
                onclick={() => onreveal(key, true)}>{t('search.copy')}</Button>
              <Button type="button" variant="ghost" size="sm" disabled={busy}
                onclick={() => onremove(key)}>{t('search.delete')}</Button>
            </div>
          </li>
        {/each}
      </ul>
      <Field.Field>
        <Field.Label for="search-edit-key">
          {t(editingId ? 'search.editKey' : 'search.newKey')}
        </Field.Label>
        <Input
          id="search-edit-key"
          type="password"
          autocomplete="off"
          spellcheck={false}
          maxlength={4096}
          disabled={busy}
          value={editValue}
          oninput={(event) => onedit(event.currentTarget.value)}
        />
      </Field.Field>
      {#if error}
        <p class="text-destructive text-kit-sm" role="alert">{error}</p>
      {/if}
      <div class="flex flex-wrap justify-end gap-2">
        <Button
          type="button"
          size="sm"
          disabled={busy || !editValue.trim() ||
            (!editingId && provider.keys.length >= 16)}
          onclick={onsave}
        >{t(editingId ? 'search.save' : 'search.addKey')}</Button>
        <Button type="button" variant="outline" size="sm" onclick={oncancel}>
          {t('common.cancel')}
        </Button>
      </div>
    </Field.Group>
  {/if}
</dialog>
