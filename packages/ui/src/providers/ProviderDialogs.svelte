<script lang="ts">
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import { Button } from '#lib/components/ui/button/index.js'
  import type { Provider } from '../ports'
  import type { ProviderDraft } from './draft'
  import { getKitContext } from '../context'

  let {
    addOpen = $bindable(false),
    infoOpen = $bindable(false),
    notesOpen = $bindable(false),
    newName = $bindable(''),
    editName = $bindable(''),
    editNotes = $bindable(''),
    draft,
    providers,
    create,
    rename,
    saveNotes,
    remove,
  }: {
    addOpen?: boolean
    infoOpen?: boolean
    notesOpen?: boolean
    newName?: string
    editName?: string
    editNotes?: string
    draft: ProviderDraft | null
    providers: Provider[]
    create: () => void | Promise<void>
    rename: () => void
    saveNotes: () => void
    remove: () => void | Promise<void>
  } = $props()
  const { i18n } = getKitContext()
  const { t } = i18n
</script>

<Dialog.Root bind:open={addOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header><Dialog.Title>{t('providers.add')}</Dialog.Title></Dialog.Header>
    <form
      class="contents"
      onsubmit={(event) => {
        event.preventDefault()
        void create()
      }}
    >
      <Field.Field>
        <Field.Label for="kit-provider-new">{t('providers.name')}</Field.Label>
        <Input id="kit-provider-new" maxlength={80} bind:value={newName} />
      </Field.Field>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (addOpen = false)}
          >{t('common.cancel')}</Button
        >
        <Button type="submit" disabled={!newName.trim()}>{t('common.confirm')}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root bind:open={infoOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header><Dialog.Title>{t('providers.settings')}</Dialog.Title></Dialog.Header>
    <form
      class="contents"
      onsubmit={(event) => {
        event.preventDefault()
        rename()
      }}
    >
      <Field.Field>
        <Field.Label for="kit-provider-name">{t('providers.name')}</Field.Label>
        <Input id="kit-provider-name" maxlength={80} bind:value={editName} />
      </Field.Field>
      <Dialog.Footer class="sm:justify-between">
        {#if !providers.find((p) => p.id === draft?.id)?.presetId}
          <Button
            type="button"
            variant="ghost"
            class="text-destructive"
            onclick={() => void remove()}>{t('providers.delete')}</Button
          >
        {:else}<span></span>{/if}
        <Button type="submit" disabled={!editName.trim()}>{t('providers.save')}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root bind:open={notesOpen}>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header><Dialog.Title>{t('providers.notes')}</Dialog.Title></Dialog.Header>
    <form
      class="contents"
      onsubmit={(event) => {
        event.preventDefault()
        saveNotes()
      }}
    >
      <Textarea
        class="min-h-64 font-mono text-kit-sm"
        aria-label={t('providers.notes')}
        placeholder={t('providers.notes_placeholder')}
        maxlength={10000}
        bind:value={editNotes}
      />
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (notesOpen = false)}
          >{t('common.cancel')}</Button
        >
        <Button type="submit">{t('providers.save')}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
