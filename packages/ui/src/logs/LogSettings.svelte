<script lang="ts">
  // Log level (docs/16 §4.5): applies immediately; warns when the log file cannot be written.
  import { Alert, AlertDescription } from '#lib/components/ui/alert/index.js'
  import * as Field from '#lib/components/ui/field/index.js'
  import * as Select from '#lib/components/ui/select/index.js'
  import { getKitContext } from '../context'
  import { isKitClientError } from '../ports'
  import type { LogLevel, LogStatus } from '../ports'

  const LEVELS: readonly LogLevel[] = ['ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE']

  const { ports, i18n, notify } = getKitContext()
  const t = i18n.t
  const logs = ports.logs

  let status = $state<LogStatus | null>(null)

  async function run(call: () => Promise<LogStatus>) {
    try {
      status = await call()
    } catch (error) {
      notify.toast('error', t(`errors.${isKitClientError(error) ? error.code : 'internal'}`))
    }
  }

  $effect(() => {
    if (logs) void run(() => logs.getLevel())
  })
</script>

{#if logs}
  <Field.Group>
    <Field.Field>
      <Field.Label for="kit-log-level">{t('logs.level')}</Field.Label>
      <Select.Root
        type="single"
        value={status?.level ?? ''}
        disabled={!status}
        onValueChange={(value) => void run(() => logs.setLevel(value as LogLevel))}
      >
        <Select.Trigger id="kit-log-level" class="w-40">{status?.level ?? '—'}</Select.Trigger>
        <Select.Content>
          <Select.Group>
            {#each LEVELS as level (level)}
              <Select.Item value={level} label={level} />
            {/each}
          </Select.Group>
        </Select.Content>
      </Select.Root>
      <Field.Description>{t('logs.levelHint')}</Field.Description>
    </Field.Field>
    {#if status && !status.available}
      <Alert variant="destructive"><AlertDescription>{t('logs.unavailable')}</AlertDescription></Alert>
    {/if}
  </Field.Group>
{/if}
