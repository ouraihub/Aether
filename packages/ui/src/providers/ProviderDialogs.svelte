<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import { getKitContext } from '../context'
  import type { Provider } from '../ports'

  let {
    addOpen = $bindable(),
    infoOpen = $bindable(),
    notesOpen = $bindable(),
    newName = $bindable(),
    editName = $bindable(),
    editNotes = $bindable(),
    providers,
    draft,
    create,
    rename,
    remove,
    saveNotes,
  }: {
    addOpen: boolean
    infoOpen: boolean
    notesOpen: boolean
    newName: string
    editName: string
    editNotes: string
    providers: Provider[]
    draft: { id: string | null } | null
    create: () => void
    rename: () => void
    remove: () => void
    saveNotes: () => void
  } = $props()
  const { t } = getKitContext().i18n
</script>

<Dialog.Root bind:open={addOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header><Dialog.Title>{t('providers.add')}</Dialog.Title></Dialog.Header>
    <form class="contents" onsubmit={(event) => { event.preventDefault(); create() }}>
      <label class="flex flex-col gap-2">
        <span>{t('providers.name')}</span>
        <Input id="kit-provider-new" maxlength={80} bind:value={newName} />
      </label>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (addOpen = false)}>
          {t('common.cancel')}
        </Button>
        <Button type="submit" disabled={!newName.trim()}>{t('common.confirm')}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root bind:open={infoOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header><Dialog.Title>{t('providers.settings')}</Dialog.Title></Dialog.Header>
    <form class="contents" onsubmit={(event) => { event.preventDefault(); rename() }}>
      <label class="flex flex-col gap-2">
        <span>{t('providers.name')}</span>
        <Input id="kit-provider-name" maxlength={80} bind:value={editName} />
      </label>
      <Dialog.Footer class="sm:justify-between">
        {#if !providers.find((p) => p.id === draft?.id)?.presetId}
          <Button type="button" variant="ghost" class="text-destructive" onclick={remove}>
            {t('providers.delete')}
          </Button>
        {:else}<span></span>{/if}
        <Button type="submit" disabled={!editName.trim()}>{t('providers.save')}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root bind:open={notesOpen}>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header><Dialog.Title>{t('providers.notes')}</Dialog.Title></Dialog.Header>
    <form class="contents" onsubmit={(event) => { event.preventDefault(); saveNotes() }}>
      <Textarea
        class="min-h-64 font-mono text-kit-sm"
        aria-label={t('providers.notes')}
        placeholder={t('providers.notes_placeholder')}
        maxlength={10000}
        bind:value={editNotes}
      />
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (notesOpen = false)}>
          {t('common.cancel')}
        </Button>
        <Button type="submit">{t('providers.save')}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
