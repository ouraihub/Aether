<script>
  // Host root: settings live in a dialog so the model picker stacks over it
  // (docs/90 P2-3, docs/16 §3).
  import {
    createDefaultNotify,
    createI18n,
    KitNotifier,
    setKitContext,
  } from '@ouraihub/aether-ui'
  import { createLogger } from '@ouraihub/aether-ui/logs'
  import { ProvidersSettings } from '@ouraihub/aether-ui/providers'
  import { createTauriPorts } from '@ouraihub/aether-ui/tauri'
  // Stands in for the host's own shadcn Dialog; the package does not export
  // its UI primitives.
  import {
    Button,
  } from '../../../../packages/ui/src/components/ui/button/index.js'
  import * as Dialog from
    '../../../../packages/ui/src/components/ui/dialog/index.js'

  const ports = createTauriPorts({ providers: true })
  const i18n = createI18n(navigator.language)
  const notify = createDefaultNotify()
  setKitContext({ ports, i18n, notify, logger: createLogger(ports.logs) })
  let open = $state(true)
</script>

<main class="flex flex-col items-start gap-4 p-6">
  <h1 class="text-xl font-semibold">Aether · Providers Only</h1>
  <Button onclick={() => (open = true)}>{i18n.t('providers.title')}</Button>
</main>
<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-4xl">
    <Dialog.Header>
      <Dialog.Title>{i18n.t('providers.title')}</Dialog.Title>
    </Dialog.Header>
    <div class="max-h-[70vh] overflow-y-auto"><ProvidersSettings /></div>
  </Dialog.Content>
</Dialog.Root>
<KitNotifier {notify} {i18n} />
