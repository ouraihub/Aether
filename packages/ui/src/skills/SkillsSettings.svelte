<script lang="ts">
  // Skill list, import → full-text preview → confirm, view, edit, duplicate, reorder, delete (docs/16 §4.3).
  import { Button } from '#lib/components/ui/button/index.js'
  import * as Empty from '#lib/components/ui/empty/index.js'
  import { getKitContext } from '../context'
  import { isKitClientError } from '../ports'
  import type { Preview, SkillDto, SkillMeta, Source } from '../ports'
  import { errorText } from './error'
  import SkillEditor from './SkillEditor.svelte'
  import SkillPreviewDialog from './SkillPreviewDialog.svelte'

  const { ports, i18n, notify } = getKitContext()
  const { t } = i18n
  const port = ports.skills

  let skills = $state<SkillMeta[]>([])
  let busy = $state(false)
  let file = $state<HTMLInputElement | null>(null)
  let source: Source | null = null
  let preview = $state<Preview | null>(null)
  let previewOpen = $state(false)
  let viewing = $state<SkillDto | null>(null)
  let viewOpen = $state(false)
  let editing = $state<SkillDto | null>(null)
  let editOpen = $state(false)

  async function act(run: () => Promise<void>) {
    busy = true
    try {
      await run()
    } catch (error) {
      notify.toast('error', errorText(t, error))
    } finally {
      busy = false
    }
  }

  if (port) void act(async () => void (skills = await port.list()))

  // A picked SKILL.md or .zip goes over IPC as bytes (docs/14 §6 `Source::Bytes`).
  function pickFile(input: HTMLInputElement) {
    const picked = input.files?.[0]
    input.value = ''
    if (!picked || !port) return
    void act(async () => {
      const next: Source = { bytes: { fileName: picked.name, bytes: [...new Uint8Array(await picked.arrayBuffer())] } }
      preview = await port.preview(next)
      source = next
      previewOpen = true
    })
  }

  function install(replace: boolean) {
    const confirmed = preview
    if (!port || !source || !confirmed) return
    const from = source
    void act(async () => {
      try {
        skills = await port.install(from, confirmed.digest, replace)
        previewOpen = false
        notify.toast('success', t('skills.installed', { name: confirmed.name }))
      } catch (error) {
        // The package changed after the preview: show the new text and confirm again (docs/14 §4).
        if (isKitClientError(error) && error.details.reason === 'content_changed') {
          preview = await port.preview(from)
          notify.toast('info', t('skills.content_changed'))
          return
        }
        throw error
      }
    })
  }

  function view(id: string) {
    if (!port) return
    void act(async () => {
      viewing = await port.get(id)
      viewOpen = true
    })
  }

  function edit(id: string | null) {
    if (!port) return
    void act(async () => {
      editing = id === null ? null : await port.get(id)
      editOpen = true
    })
  }

  function duplicate(skill: SkillMeta) {
    if (!port) return
    void act(async () => void (skills = await port.duplicate(skill.id, `${skill.name}-copy`)))
  }

  function move(index: number, by: number) {
    if (!port) return
    const ids = skills.map((s) => s.id)
    const [id] = ids.splice(index, 1)
    if (id === undefined) return
    ids.splice(index + by, 0, id)
    void act(async () => void (skills = await port.reorder(ids)))
  }

  async function remove(skill: SkillMeta) {
    if (!port) return
    const ok = await notify.confirm({
      title: t('skills.delete'),
      body: t('skills.delete_confirm', { name: skill.name }),
      danger: true,
    })
    if (ok) void act(async () => void (skills = await port.remove(skill.id)))
  }
</script>

{#if port}
  <section class="flex flex-col gap-4" aria-label={t('skills.title')}>
    <div class="flex gap-2">
      <Button disabled={busy} onclick={() => file?.click()}>{t('skills.import')}</Button>
      <Button variant="outline" disabled={busy} onclick={() => edit(null)}>{t('skills.new')}</Button>
      <input
        bind:this={file}
        type="file"
        accept=".md,.zip"
        class="hidden"
        aria-label={t('skills.import')}
        onchange={(e) => pickFile(e.currentTarget)}
      />
    </div>
    {#if skills.length === 0}
      <Empty.Root>
        <Empty.Header>
          <Empty.Title>{t('skills.empty')}</Empty.Title>
          <Empty.Description>{t('skills.empty_hint')}</Empty.Description>
        </Empty.Header>
      </Empty.Root>
    {:else}
      <ul class="flex flex-col gap-2" aria-label={t('skills.list')}>
        {#each skills as skill, index (skill.id)}
          <li class="flex items-center gap-3 rounded-md border p-3">
            <div class="flex min-w-0 flex-1 flex-col">
              <span class="flex min-w-0 items-center gap-2">
                <span class="truncate font-mono text-sm">{skill.name}</span>
                {#if skill.hasScripts}
                  <span class="shrink-0 rounded border px-1.5 text-xs">{t('skills.has_scripts')}</span>
                {/if}
              </span>
              <span class="text-muted-foreground truncate text-xs">
                {skill.version ?? t('skills.unversioned')} · {t(`skills.origin.${skill.origin}`)} ·
                {t('skills.revision', { revision: skill.revision })} · {t('skills.chars', { count: skill.bodyChars })}
                {#if skill.fileCount > 0}· {t('skills.file_count', { count: skill.fileCount })}{/if}
              </span>
            </div>
            <Button
              size="sm"
              variant="ghost"
              disabled={busy || index === 0}
              aria-label={t('skills.up', { name: skill.name })}
              onclick={() => move(index, -1)}>↑</Button
            >
            <Button
              size="sm"
              variant="ghost"
              disabled={busy || index === skills.length - 1}
              aria-label={t('skills.down', { name: skill.name })}
              onclick={() => move(index, 1)}>↓</Button
            >
            <Button size="sm" variant="outline" disabled={busy} onclick={() => view(skill.id)}
              >{t('skills.view')}</Button
            >
            {#if skill.origin === 'builtin'}
              <Button size="sm" variant="outline" disabled={busy} onclick={() => duplicate(skill)}
                >{t('skills.duplicate')}</Button
              >
            {:else}
              <Button size="sm" variant="outline" disabled={busy} onclick={() => edit(skill.id)}
                >{t('skills.edit')}</Button
              >
              <Button size="sm" variant="destructive" disabled={busy} onclick={() => void remove(skill)}
                >{t('skills.delete')}</Button
              >
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
  <SkillPreviewDialog
    bind:open={previewOpen}
    skill={preview}
    title={t('skills.preview_title')}
    current={preview?.current ?? null}
    onconfirm={install}
  />
  <SkillPreviewDialog bind:open={viewOpen} skill={viewing} title={viewing?.name ?? ''} />
  <SkillEditor bind:open={editOpen} {port} skill={editing} onsaved={(next) => (skills = next)} />
{/if}
