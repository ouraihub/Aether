<script lang="ts">
  // Full text of a skill before it is installed or used (docs/16 §4.3, docs/14 §4).
  // Without `onconfirm` it is a read-only viewer.
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import { getKitContext } from '../context'
  import type { Preview, PreviewCurrent } from '../ports'

  /** What both a Preview and a stored SkillDto carry. */
  type SkillText = Pick<Preview, 'name' | 'description' | 'version' | 'body' | 'license' | 'extra'>

  let {
    open = $bindable(false),
    skill,
    title,
    current = null,
    confirmLabel,
    onconfirm,
  }: {
    open: boolean
    skill: SkillText | null
    title: string
    /** The installed skill a confirm would replace; confirming then needs the "replace" box. */
    current?: PreviewCurrent | null
    confirmLabel?: string
    onconfirm?: (replace: boolean) => void
  } = $props()

  const { t } = getKitContext().i18n
  let replace = $state(false)
  $effect.pre(() => {
    if (open) replace = false
  })
  const extra = $derived(Object.entries(skill?.extra ?? {}))
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex max-h-[85vh] flex-col sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>{title}</Dialog.Title>
      <Dialog.Description>{t('skills.preview_desc')}</Dialog.Description>
    </Dialog.Header>
    {#if skill}
      <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
        <dt class="text-muted-foreground">{t('skills.name')}</dt>
        <dd class="font-mono">{skill.name}</dd>
        <dt class="text-muted-foreground">{t('skills.version')}</dt>
        <dd>{skill.version ?? t('skills.unversioned')}</dd>
        <dt class="text-muted-foreground">{t('skills.description')}</dt>
        <dd>{skill.description}</dd>
        {#each extra as [key, value] (key)}
          <dt class="text-muted-foreground font-mono">{key}</dt>
          <dd>{value}</dd>
        {/each}
      </dl>
      <pre
        class="bg-muted min-h-0 flex-1 overflow-y-auto rounded-md p-3 text-xs whitespace-pre-wrap"
        aria-label={t('skills.body')}>{skill.body}</pre>
      {#if skill.license}
        <details class="text-xs">
          <summary class="cursor-pointer">{t('skills.license')}</summary>
          <pre class="max-h-40 overflow-y-auto whitespace-pre-wrap">{skill.license}</pre>
        </details>
      {/if}
      {#if onconfirm && current}
        <label class="flex items-center gap-2 text-sm">
          <input type="checkbox" class="accent-primary" bind:checked={replace} />
          {t('skills.replace', { version: current.version ?? t('skills.unversioned'), revision: current.revision })}
        </label>
      {/if}
    {/if}
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (open = false)}>{t('common.cancel')}</Button>
      {#if onconfirm}
        <Button disabled={!skill || (current !== null && !replace)} onclick={() => onconfirm(replace)}>
          {confirmLabel ?? t('skills.install')}
        </Button>
      {/if}
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
