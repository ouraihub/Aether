<script lang="ts">
  // Approval of one `Ask` call (docs/16 §4.2): tool, source, arguments preview, allow once / deny.
  import * as Alert from '#lib/components/ui/alert/index.js'
  import { Button } from '#lib/components/ui/button/index.js'
  import { getKitContext } from '../context'
  import type { ToolItem } from './run-session.svelte'

  let {
    tool,
    onAnswer,
  }: { tool: ToolItem; onAnswer: (allow: boolean) => void } = $props()
  const { t } = getKitContext().i18n
</script>

<Alert.Root aria-label={t('chat.approval.title', { name: tool.name })}>
  <Alert.Title>{t('chat.approval.title', { name: tool.name })}</Alert.Title>
  <Alert.Description class="flex flex-col gap-2">
    <span
      >{tool.server
        ? t('chat.approval.mcp', { server: tool.server })
        : t('chat.approval.host')}</span
    >
    <pre
      class="bg-muted max-h-40 w-full overflow-auto rounded-kit-sm p-2 text-kit-xs whitespace-pre-wrap">{tool.pending}</pre>
    <span class="flex gap-2">
      <Button size="sm" onclick={() => onAnswer(true)}
        >{t('chat.approval.allow')}</Button
      >
      <Button size="sm" variant="outline" onclick={() => onAnswer(false)}
        >{t('chat.approval.deny')}</Button
      >
    </span>
  </Alert.Description>
</Alert.Root>
