<script lang="ts">
  // Per-run skill selection above the composer (docs/16 §4.2): metadata only, selected chars / budget.
  // `changed` (a `skill_changed` rejection) opens the current text for re-confirmation (docs/14 §5).
  import * as Select from '#lib/components/ui/select/index.js'
  import { cn } from '#lib/utils.js'
  import { getKitContext } from '../context'
  import { isKitClientError } from '../ports'
  import type { SkillDto, SkillMeta, SkillRef, SkillsPort } from '../ports'
  import SkillPreviewDialog from '../skills/SkillPreviewDialog.svelte'

  let {
    port,
    value,
    budget,
    changed,
    onchange,
    onresolved,
  }: {
    port: SkillsPort
    value: SkillRef[]
    /** `Budget.skills_chars` of the backend. */
    budget: number
    /** Skill id from a `skill_changed` rejection, or null. */
    changed: string | null
    onchange: (refs: SkillRef[]) => void
    onresolved: () => void
  } = $props()

  const { i18n, notify } = getKitContext()
  const { t } = i18n
  let skills = $state<SkillMeta[]>([])
  let reconfirm = $state<SkillDto | null>(null)
  let reconfirmOpen = $state(false)

  const ids = $derived(value.map((r) => r.id))
  const chars = $derived(skills.filter((s) => ids.includes(s.id)).reduce((sum, s) => sum + s.bodyChars, 0))

  async function refresh() {
    try {
      skills = await port.list()
    } catch (error) {
      notify.toast('error', t(`errors.${isKitClientError(error) ? error.code : 'internal'}`))
    }
  }
  void refresh()

  // Kept refs keep their pinned revision: an edited skill is caught by `snapshot`, not silently re-pinned.
  function pick(next: string[]) {
    onchange(
      next.flatMap((id) => {
        const kept = value.find((r) => r.id === id)
        const meta = skills.find((s) => s.id === id)
        return kept ? [kept] : meta ? [{ id, revision: meta.revision }] : []
      }),
    )
  }

  $effect(() => {
    if (changed !== null) void openChanged(changed)
  })

  async function openChanged(id: string) {
    void refresh()
    try {
      reconfirm = await port.get(id)
      reconfirmOpen = true
    } catch {
      // Deleted since it was selected: drop it.
      onchange(value.filter((r) => r.id !== id))
      notify.toast('info', t('skills.removed'))
      onresolved()
    }
  }

  function confirmChanged() {
    const skill = reconfirm
    if (skill) onchange(value.map((r) => (r.id === skill.id ? { id: r.id, revision: skill.revision } : r)))
    reconfirm = null
    reconfirmOpen = false
    onresolved()
  }

  // Closing the dialog without confirming deselects the changed skill.
  $effect(() => {
    if (!reconfirmOpen && reconfirm) {
      const id = reconfirm.id
      reconfirm = null
      onchange(value.filter((r) => r.id !== id))
      onresolved()
    }
  })
</script>

<div class="flex items-center gap-2 text-xs">
  <Select.Root
    type="multiple"
    value={ids}
    onValueChange={pick}
    onOpenChange={(open) => {
      if (open) void refresh()
    }}
  >
    <Select.Trigger size="sm" class="w-64" aria-label={t('skills.picker')}>
      {value.length === 0 ? t('skills.picker_none') : t('skills.picker_count', { count: value.length })}
    </Select.Trigger>
    <Select.Content>
      <Select.Group>
        {#each skills as skill (skill.id)}
          <Select.Item value={skill.id} label={skill.name} />
        {:else}
          <p class="text-muted-foreground px-2 py-1">{t('skills.empty')}</p>
        {/each}
      </Select.Group>
    </Select.Content>
  </Select.Root>
  <span class={cn('text-muted-foreground', chars > budget && 'text-destructive')}>
    {t('skills.budget', { chars, budget })}
  </span>
</div>
<SkillPreviewDialog
  bind:open={reconfirmOpen}
  skill={reconfirm}
  title={t('skills.changed_title')}
  confirmLabel={t('skills.use')}
  onconfirm={confirmChanged}
/>
