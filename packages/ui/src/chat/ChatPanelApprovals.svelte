<script lang="ts">
  import { Button } from '#lib/components/ui/button/index.js'
  import ApprovalCard from './ApprovalCard.svelte'
  import type { RunSession, ToolItem } from './run-session.svelte'
  import type { KitI18n } from '../i18n'
  let { session, tools, i18n }: { session: RunSession; tools: ToolItem[]; i18n: KitI18n } = $props()
</script>

<div class="approval-override">
  {#each tools as tool (tool.callId)}
    <ApprovalCard {tool} onAnswer={(allow) => session.approve(tool, allow)} />
  {/each}
  <Button type="button" variant="outline" onclick={() => session.stop()}
    >{i18n.t('chat.stop')}</Button
  >
</div>

<style>
  .approval-override {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-width: 0;
  }
  .approval-override > :global(button) {
    align-self: flex-end;
  }
</style>
