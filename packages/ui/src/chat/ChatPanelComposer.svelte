<script lang="ts">
  import type { Snippet } from 'svelte'
  import XIcon from '@lucide/svelte/icons/x'
  import { Button } from '#lib/components/ui/button/index.js'
  import type { KitI18n } from '../i18n'
  import type {
    ContextUsage,
    ImagePart,
    ModelOption,
    ModelRef,
    PermissionMode,
    ReasoningEffort,
    SkillRef,
    SkillsPort,
  } from '../ports'
  import { settingsTarget } from '../providers/revision.svelte'
  import ChatComposer from './ChatComposer.svelte'
  import ChatPanelMode from './ChatPanelMode.svelte'
  import ChatPanelToolbar from './ChatPanelToolbar.svelte'
  import ReasoningControl from './ReasoningControl.svelte'
  import SkillPicker from './SkillPicker.svelte'
  import UsageDetails from './UsageDetails.svelte'
  import type { RunSession } from './run-session.svelte'

  type Attached = ImagePart & { preview: string }
  type Recovery = {
    draft: string
    images: Attached[]
    droppedKey: string | null
    contextKey: string | null
  }

  let {
    i18n,
    session,
    purpose,
    draft = $bindable(''),
    canSend,
    activeContext,
    images,
    vision,
    recovery,
    webSearch = $bindable(false),
    generateImage = $bindable(false),
    toolbar,
    onToolbarChange,
    paintingModel,
    capability,
    historyBusy,
    onNewConversation,
    attach,
    onDraftInput,
    onRemoveImage,
    onRecoverDraft,
    onDropContext,
    hasSearch,
    selected,
    skillsBudget,
    skillsPort,
    onSkillsChange,
    onSkillResolved,
    mode,
    allowedModes,
    chooseMode,
    reasoningEffort,
    allowedEfforts,
    reasoningOpen = $bindable(false),
    chooseEffort,
    currentContext,
    picked,
    onOpenSettings,
    onSend,
    onStop,
    hasImages,
    override,
  }: {
    i18n: KitI18n
    session: RunSession
    purpose: string
    draft?: string
    canSend: boolean
    activeContext: { kind: string; id: string; label: string } | null
    images: Attached[]
    vision: boolean
    recovery: Recovery | null
    webSearch?: boolean
    generateImage?: boolean
    toolbar?: string[] | undefined
    onToolbarChange?: ((ids: string[]) => void) | undefined
    paintingModel: boolean
    capability: ModelOption | null
    historyBusy: boolean
    onNewConversation: () => void
    attach: (files: FileList | File[] | null | undefined) => Promise<void>
    onDraftInput: () => void
    onRemoveImage: (index: number) => void
    onRecoverDraft: () => void
    onDropContext: () => void
    hasSearch: boolean
    selected: SkillRef[]
    skillsBudget: number
    skillsPort?: SkillsPort | undefined
    onSkillsChange: (refs: SkillRef[]) => void
    onSkillResolved: () => void
    mode: PermissionMode
    allowedModes: PermissionMode[]
    chooseMode: (mode: PermissionMode) => Promise<void>
    reasoningEffort: ReasoningEffort
    allowedEfforts: ReasoningEffort[]
    reasoningOpen?: boolean
    chooseEffort: (effort: ReasoningEffort) => void
    currentContext: ContextUsage | null
    picked: ModelRef | null
    onOpenSettings?: (() => void) | undefined
    onSend: () => void
    onStop: () => void
    hasImages: (files: FileList | null | undefined) => boolean
    override?: Snippet | undefined
  } = $props()
  const t = $derived(i18n.t)
</script>

<ChatComposer
  bind:value={draft}
  label={t('chat.input')}
  placeholder={t('chat.placeholder')}
  submitLabel={t('chat.send')}
  stopLabel={t('chat.stop')}
  disabled={!canSend}
  running={session.running}
  expandLabel={t('chat.expandInput')}
  collapseLabel={t('chat.collapseInput')}
  onsubmit={onSend}
  onstop={onStop}
  oninput={onDraftInput}
  onpaste={(event) => {
    const files = event.clipboardData?.files
    if (!hasImages(files)) return
    event.preventDefault()
    void attach(files)
  }}
  onfiles={(files) => void attach(files)}
  {override}
>
  {#snippet top()}
    {#if activeContext}
      <div class="context-chip">
        <span title={activeContext.label}>{activeContext.label}</span><Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={t('chat.context.remove', { label: activeContext.label })}
          title={t('chat.context.remove', { label: activeContext.label })}
          onclick={onDropContext}><XIcon /></Button
        >
      </div>
    {/if}
    {#if images.length > 0}
      <ul class="image-tray">
        {#each images as image, index (image.preview)}
          <li>
            <img src={image.preview} alt="" /><Button
              type="button"
              variant="secondary"
              size="icon-xs"
              aria-label={t('chat.removeImage', { index: index + 1 })}
              title={t('chat.removeImage', { index: index + 1 })}
              onclick={() => onRemoveImage(index)}><XIcon /></Button
            >
          </li>
        {/each}
      </ul>
      {#if !vision}<p class="search-status" role="status">{t('chat.visionUnsupported')}</p>{/if}
    {/if}
    {#if recovery}
      <div class="draft-recovery" role="status">
        <span>{t('chat.draftRecoverable')}</span><Button
          type="button"
          variant="outline"
          size="sm"
          disabled={draft !== '' || images.length > 0}
          onclick={onRecoverDraft}>{t('chat.restoreDraft')}</Button
        >
      </div>
    {/if}
    {#if hasSearch && purpose === 'chat' && session.search.status === 'searching'}<p
        class="search-status"
        role="status"
      >
        {t('chat.searching')}
      </p>{/if}
  {/snippet}
  {#snippet tools()}
    <ChatPanelToolbar
      {i18n}
      running={session.running}
      {historyBusy}
      {vision}
      {paintingModel}
      {capability}
      {purpose}
      {toolbar}
      {onToolbarChange}
      {hasSearch}
      newConversation={onNewConversation}
      {attach}
      bind:webSearch
      bind:generateImage
    />
    {#if skillsPort}<div class="skills-control">
        <SkillPicker
          port={skillsPort}
          value={selected}
          budget={skillsBudget}
          changed={session.skillChanged}
          onchange={onSkillsChange}
          onresolved={onSkillResolved}
        />
      </div>{/if}
  {/snippet}
  {#snippet actions()}
    <ChatPanelMode {i18n} {mode} {allowedModes} running={session.running} {chooseMode} />
    {#if picked}
      <ReasoningControl
        value={reasoningEffort}
        efforts={allowedEfforts}
        disabled={session.running}
        bind:open={reasoningOpen}
        {i18n}
        onchange={chooseEffort}
        onOpenSettings={onOpenSettings
          ? () => {
              settingsTarget.value = { endpointId: picked.endpointId, modelId: picked.modelId }
              onOpenSettings()
            }
          : undefined}
      />
    {/if}
    <UsageDetails
      context={currentContext}
      modelName={capability?.name ?? capability?.modelId ?? ''}
      usage={session.turns.at(-1)?.usage ?? null}
      total={session.cumulativeUsage}
      cost={session.turns.at(-1)?.cost ?? null}
      totalCost={session.cumulativeCost}
    />
  {/snippet}
</ChatComposer>

<style>
  .context-chip {
    display: flex;
    align-items: center;
    align-self: flex-start;
    gap: 0.25rem;
    max-width: calc(100% - 2rem);
    padding-left: 0.5rem;
    background: var(--color-muted);
    border-radius: var(--kit-radius, var(--radius));
  }
  .context-chip span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .context-chip :global(button) {
    flex-shrink: 0;
  }
  .image-tray {
    display: flex;
    flex-wrap: wrap;
    gap: 0.375rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .image-tray li {
    position: relative;
  }
  .image-tray img {
    width: 3.5rem;
    height: 3.5rem;
    object-fit: cover;
    border: 1px solid var(--color-border);
    border-radius: var(--kit-radius, var(--radius));
  }
  .image-tray :global(button) {
    position: absolute;
    top: -0.375rem;
    right: -0.375rem;
  }
  .skills-control {
    min-width: 0;
    max-width: 100%;
  }
  .skills-control :global(div:first-child) {
    flex-wrap: wrap;
    min-width: 0;
  }
  .skills-control :global([data-slot='select-trigger']) {
    width: auto;
    max-width: 100%;
  }
  .search-status,
  .draft-recovery {
    margin: 0;
    color: var(--color-muted-foreground);
    font-size: var(--kit-font-size-sm, 0.8125rem);
  }
  .draft-recovery {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }
</style>
