<script lang="ts">
  // Component playground (`pnpm -F @ouraihub/aether-ui dev`) and the overlay test fixture (docs/16 §6, 陷阱 2).
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { createI18n } from '../i18n'
  import KitNotifier from '../KitNotifier.svelte'
  import { createDefaultNotify } from '../notify.svelte'

  const notify = createDefaultNotify()
  const i18n = createI18n('zh-CN')
  let clicks = $state(0)
  let answer = $state('none')

  async function ask() {
    answer = String(await notify.confirm({ title: 'Delete?', body: 'This cannot be undone.', danger: true }))
  }
</script>

<main class="flex flex-col items-start gap-4 p-6">
  <Dialog.Root>
    <Dialog.Trigger>
      {#snippet child({ props })}
        <Button {...props}>Open dialog</Button>
      {/snippet}
    </Dialog.Trigger>
    <Dialog.Content>
      <Dialog.Header>
        <Dialog.Title>Settings</Dialog.Title>
        <Dialog.Description>Overlay must unmount on close.</Dialog.Description>
      </Dialog.Header>
      <Field.Group>
        <Field.Field>
          <Field.Label for="demo-name">Name</Field.Label>
          <Input id="demo-name" />
        </Field.Field>
      </Field.Group>
    </Dialog.Content>
  </Dialog.Root>
  <Button variant="outline" onclick={() => void ask()}>Confirm</Button>
  <Button variant="secondary" onclick={() => clicks++}>Background clicks: {clicks}</Button>
  <p class="text-muted-foreground">Answer: {answer}</p>
</main>
<KitNotifier {notify} {i18n} />
