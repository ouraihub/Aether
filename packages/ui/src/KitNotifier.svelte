<script lang="ts">
  // Mounts the toaster and the single confirm dialog behind DefaultNotify (docs/16 §3).
  import * as AlertDialog from '#lib/components/ui/alert-dialog/index.js'
  import { Toaster } from '#lib/components/ui/sonner/index.js'
  import type { KitI18n } from './i18n'
  import type { DefaultNotify } from './notify.svelte'

  interface Props {
    notify: DefaultNotify
    i18n: KitI18n
  }

  let { notify, i18n }: Props = $props()
</script>

<Toaster />
<AlertDialog.Root open={notify.open} onOpenChange={(isOpen) => !isOpen && notify.answer(false)}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{notify.request?.title}</AlertDialog.Title>
      <AlertDialog.Description>{notify.request?.body}</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>{i18n.t('common.cancel')}</AlertDialog.Cancel>
      <AlertDialog.Action
        variant={notify.request?.danger ? 'destructive' : 'default'}
        onclick={() => notify.answer(true)}
      >
        {i18n.t('common.confirm')}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
