<script lang="ts">
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down'
  import CheckIcon from '@lucide/svelte/icons/check'
  import ShieldIcon from '@lucide/svelte/icons/shield'
  import HandIcon from '@lucide/svelte/icons/hand'
  import RouteIcon from '@lucide/svelte/icons/route'
  import FilePenIcon from '@lucide/svelte/icons/file-pen'
  import ShieldCheckIcon from '@lucide/svelte/icons/shield-check'
  import ShieldAlertIcon from '@lucide/svelte/icons/shield-alert'
  import InfoIcon from '@lucide/svelte/icons/info'
  import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js'
  import { Button } from '#lib/components/ui/button/index.js'
  import type { KitI18n } from '../i18n'
  import type { PermissionMode } from '../ports'
  let {
    i18n,
    mode,
    allowedModes,
    running,
    chooseMode,
  }: {
    i18n: KitI18n
    mode: PermissionMode
    allowedModes: PermissionMode[]
    running: boolean
    chooseMode: (mode: PermissionMode) => Promise<void>
  } = $props()
  const t = $derived(i18n.t)
</script>

{#if mode === 'plan'}<span class="plan-badge" role="status">{t('chat.mode.planBadge')}</span>{/if}
{#if allowedModes.length > 1 || allowedModes.includes('full_access')}
  <DropdownMenu.Root>
    <DropdownMenu.Trigger disabled={running}>
      {#snippet child({ props })}<Button
          {...props}
          variant="ghost"
          size="sm"
          class={mode === 'full_access' ? 'mode-danger' : undefined}
          aria-label={`${t('chat.mode.label')}: ${t(`chat.mode.${mode}`)}`}
          ><ShieldIcon data-icon="inline-start" />{t(`chat.mode.${mode}`)}<ChevronDownIcon
            data-icon="inline-end"
          /></Button
        >{/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content side="top" align="end" class="mode-menu">
      <DropdownMenu.Group aria-label={t('chat.mode.label')}>
        {#each allowedModes as option (option)}
          <DropdownMenu.Item
            class={option === 'full_access' ? 'mode-option mode-danger' : 'mode-option'}
            aria-current={option === mode ? 'true' : undefined}
            onSelect={() => void chooseMode(option)}
            >{#if option === 'default'}<HandIcon />
            {:else if option === 'plan'}<RouteIcon />
            {:else if option === 'accept_edits'}<FilePenIcon />
            {:else if option === 'auto'}<ShieldCheckIcon />
            {:else}<ShieldAlertIcon />{/if}
            <span class="mode-name">{t(`chat.mode.${option}`)}</span>
            <span class="mode-description">{t(`chat.mode.${option}Hint`)}</span>
            <span class="mode-notice">
              {#if option === 'auto' || option === 'full_access'}
                <span
                  title={t(option === 'auto' ? 'chat.mode.ruleNotice' : 'chat.mode.fullAccessBody')}
                >
                  <InfoIcon />
                </span>
              {/if}
            </span>
            <span class="mode-check"
              >{#if option === mode}<CheckIcon />{/if}</span
            ></DropdownMenu.Item
          >
        {/each}
      </DropdownMenu.Group>
      <div class="mode-footer">
        <span>{t('chat.mode.label')}</span>
        <span class="mode-keys"
          ><kbd>↑↓</kbd>{t('chat.mode.navigate')}<kbd>↵</kbd>{t('chat.mode.confirm')}<kbd>Esc</kbd
          >{t('chat.mode.close')}</span
        >
      </div>
    </DropdownMenu.Content>
  </DropdownMenu.Root>
{/if}

<style>
  .plan-badge {
    padding: 0.125rem 0.5rem;
    border: 1px solid var(--color-border);
    border-radius: 999px;
    color: var(--color-muted-foreground);
    font-size: var(--kit-font-size-sm, 0.8125rem);
    white-space: nowrap;
  }
  :global(.mode-menu) {
    width: min(35rem, calc(100vw - 1rem));
    padding: 0.375rem;
    border-radius: 0.75rem;
  }
  :global(.mode-option) {
    display: grid;
    grid-template-columns: 1rem 7.5rem minmax(0, 1fr) 1rem 0.875rem;
    align-items: center;
    column-gap: 0.5rem;
    min-height: 2.125rem;
    padding: 0.375rem 0.5rem;
    border-radius: 0.5rem;
    font-size: var(--kit-font-size-sm, 0.8125rem);
  }
  :global(.mode-option[aria-current='true']) {
    background: var(--color-muted);
  }
  :global(.mode-option > svg) {
    color: var(--color-muted-foreground);
    width: 0.875rem;
    height: 0.875rem;
  }
  .mode-name {
    white-space: nowrap;
  }
  .mode-description {
    color: var(--color-muted-foreground);
    font-size: var(--kit-font-size-xs, 0.75rem);
    text-align: right;
    line-height: 1.5;
  }
  .mode-notice,
  .mode-check {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--color-muted-foreground);
  }
  .mode-notice {
    color: var(--color-warning, var(--color-muted-foreground));
  }
  .mode-notice :global(svg),
  .mode-check :global(svg) {
    display: block;
    width: 0.8125rem;
    height: 0.8125rem;
  }
  :global(.mode-danger),
  :global(.mode-danger) :is(.mode-name, .mode-description, .mode-notice),
  :global(.mode-option.mode-danger > svg) {
    color: var(--color-destructive);
  }
  .mode-footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.75rem;
    padding: 0.625rem 0.375rem 0.125rem;
    color: var(--color-muted-foreground);
    font-size: var(--kit-font-size-xs, 0.75rem);
  }
  .mode-keys {
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }
  .mode-keys kbd {
    padding: 0 0.25rem;
    border-radius: 0.25rem;
    background: var(--color-muted);
    font: inherit;
  }
  .mode-keys kbd:not(:first-child) {
    margin-left: 0.375rem;
  }
  @media (max-width: 480px) {
    :global(.mode-option) {
      grid-template-columns: 1rem minmax(0, 1fr) 1rem 0.875rem;
      row-gap: 0.125rem;
    }
    .mode-description {
      grid-column: 2;
      grid-row: 2;
      text-align: left;
    }
    .mode-notice {
      grid-column: 3;
      grid-row: 1;
    }
    .mode-check {
      grid-column: 4;
      grid-row: 1;
    }
  }
</style>
