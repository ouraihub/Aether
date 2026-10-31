// Bumped after a provider save/remove so open pickers and chat panels reload model capabilities.
export const providersRevision = $state({ value: 0 })
/** Model the next `ProvidersSettings` mount opens; set by a chat "open settings" action, cleared once used. */
export const settingsTarget = $state<{
  value: { endpointId: string; modelId: string } | null
}>({ value: null })
