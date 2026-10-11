<script>
  // Host root: wires ports, i18n, notify and logger into the kit context (docs/16 §3).
  import { createDefaultNotify, createI18n, KitNotifier, setKitContext } from '@ouraihub/aether-ui'
  import { createLogger } from '@ouraihub/aether-ui/logs'
  import { McpServerSettings } from '@ouraihub/aether-ui/mcp'
  import { createTauriPorts } from '@ouraihub/aether-ui/tauri'

  const ports = createTauriPorts({ mcpServer: true })
  // Host-declared scope labels (ScopeSpec.label_key) come with the host's own texts.
  const i18n = createI18n(navigator.language, {
    'example.scope.read': navigator.language.startsWith('zh') ? '读取计数器' : 'Read counter',
    'example.scope.control': navigator.language.startsWith('zh') ? '重置计数器' : 'Reset counter',
  })
  const notify = createDefaultNotify()
  const logger = createLogger(ports.logs)
  setKitContext({ ports, i18n, notify, logger })
</script>

<main class="mx-auto flex max-w-3xl flex-col gap-6 p-6">
  <h1 class="text-xl font-semibold">Aether · MCP Server Only</h1>
  <McpServerSettings serverName="aether-demo" />
</main>
<KitNotifier {notify} {i18n} />
