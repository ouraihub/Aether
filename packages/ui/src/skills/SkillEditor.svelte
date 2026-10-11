<script lang="ts">
  // Create or edit a hand-written skill (docs/14 §6). Editing sends `expectedRevision`, so a
  // concurrent edit elsewhere fails with `conflict{revision_changed}` instead of being overwritten.
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Dialog from '#lib/components/ui/dialog/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import { Input } from '#lib/components/ui/input/index.js'
  import { Textarea } from '#lib/components/ui/textarea/index.js'
  import { getKitContext } from '../context'
  import type { ManualSave, SkillDto, SkillMeta, SkillsPort } from '../ports'
  import { errorText } from './error'

  let {
    open = $bindable(false),
    port,
    skill,
    onsaved,
  }: {
    open: boolean
    port: SkillsPort
    /** The skill being edited; null creates a new one. */
    skill: SkillDto | null
    onsaved: (skills: SkillMeta[]) => void
  } = $props()

  const { i18n, notify } = getKitContext()
  const { t } = i18n
  let name = $state('')
  let description = $state('')
  let version = $state('')
  let body = $state('')
  let saving = $state(false)

  $effect.pre(() => {
    if (!open) return
    name = skill?.name ?? ''
    description = skill?.description ?? ''
    version = skill?.version ?? ''
    body = skill?.body ?? ''
  })

  async function save() {
    const input: ManualSave = {
      name: name.trim(),
      description: description.trim(),
      version: version.trim() || null,
      body,
    }
    if (skill) Object.assign(input, { id: skill.id, expectedRevision: skill.revision })
    saving = true
    try {
      onsaved(await port.save(input))
      open = false
    } catch (error) {
      notify.toast('error', errorText(t, error))
    } finally {
      saving = false
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex max-h-[85vh] flex-col sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>{skill ? t('skills.edit_title', { name: skill.name }) : t('skills.new')}</Dialog.Title>
    </Dialog.Header>
    <Field.Group class="min-h-0 overflow-y-auto">
      <div class="flex gap-3">
        <Field.Field class="flex-1">
          <Field.Label for="kit-skill-name">{t('skills.name')}</Field.Label>
          <Input id="kit-skill-name" class="font-mono" maxlength={64} bind:value={name} />
        </Field.Field>
        <Field.Field class="w-32">
          <Field.Label for="kit-skill-version">{t('skills.version')}</Field.Label>
          <Input id="kit-skill-version" maxlength={32} placeholder={t('skills.unversioned')} bind:value={version} />
        </Field.Field>
      </div>
      <Field.Field>
        <Field.Label for="kit-skill-description">{t('skills.description')}</Field.Label>
        <Input id="kit-skill-description" maxlength={500} bind:value={description} />
      </Field.Field>
      <Field.Field>
        <Field.Label for="kit-skill-body">{t('skills.body')}</Field.Label>
        <Textarea id="kit-skill-body" class="min-h-64 font-mono text-xs" bind:value={body} />
      </Field.Field>
    </Field.Group>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (open = false)}>{t('common.cancel')}</Button>
      <Button disabled={saving || !name.trim() || !description.trim() || !body.trim()} onclick={() => void save()}>
        {t('skills.save')}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
