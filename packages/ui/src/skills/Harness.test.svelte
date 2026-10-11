<script lang="ts">
  // Test-only host root: SkillsSettings, or a ChatPanel whose skill selection the "app" keeps.
  import ChatPanel from '../chat/ChatPanel.svelte'
  import { setKitContext, type KitContext } from '../context'
  import type { SkillRef } from '../ports'
  import SkillsSettings from './SkillsSettings.svelte'

  let {
    kit,
    skills,
    onSkillsChange,
  }: { kit: KitContext; skills?: SkillRef[]; onSkillsChange?: (refs: SkillRef[]) => void } = $props()
  // svelte-ignore state_referenced_locally
  setKitContext(kit)
  // svelte-ignore state_referenced_locally
  let refs = $state(skills)
</script>

{#if refs}
  <ChatPanel
    model={{ endpointId: 'e1', modelId: 'm1' }}
    skills={refs}
    onSkillsChange={(next) => {
      refs = next
      onSkillsChange?.(next)
    }}
  />
{:else}
  <SkillsSettings />
{/if}
