# Aether UI integration

## Public imports

Use package exports, not internal file paths:

```ts
import { setKitContext, createDefaultNotify } from '@ouraihub/aether-ui'
import { createTauriPorts } from '@ouraihub/aether-ui/tauri'
import { ChatPanel } from '@ouraihub/aether-ui/chat'
import { ProvidersSettings, ModelSelect } from '@ouraihub/aether-ui/providers'
import { SkillsSettings } from '@ouraihub/aether-ui/skills'
import { McpClientSettings, McpServerSettings } from '@ouraihub/aether-ui/mcp'
```

The exact named exports must be checked in the installed package. Do not import a component's internal `.svelte` file from a host application.

## Context and ports

The host root creates the context once:

```ts
setKitContext({
  ports: createTauriPorts({ providers: true, chat: true, chatHistory: true }),
  i18n: createI18n('en'),
  notify: createDefaultNotify(),
  logger,
})
```

Use `createTauriPorts` only for a desktop Tauri adapter. Use `createFakePorts` in tests. Optional ports are deliberate: when a feature or history store is absent, the corresponding UI area must be absent or disabled without probing an unavailable command.

Only `@ouraihub/aether-ui/tauri` may import `@tauri-apps/api`. Components and state classes use `KitPorts`, `KitClientError`, and `AbortSignal`.

## DTO and cancellation

Import DTO types from `@ouraihub/aether-ui/ports`. They are generated from Rust with `ts-rs`; do not recreate them by hand. After Rust DTO changes, run the repository DTO generation/check command and commit the generated result.

`ChatPort.run` resolves when the run is accepted, not when it finishes. Render `RunEvent` values, especially `end`, for the terminal state. Pass an `AbortSignal`; the Tauri adapter maps abort to `chat_cancel` or the feature-specific cancel command and reports `cancelled`.

## Component boundaries

- Container components obtain ports and injected services from context.
- Presentation components receive data through props and report intent through callbacks.
- Host preferences, routing, default model selection, and page layout stay in the host application.
- Use existing Aether components before recreating provider, chat, Skills, MCP, or logs UI.
- For base controls, follow the repository's shadcn-svelte skill and existing `components/ui` implementation.

A one-shot generation flow can call `ChatPort.run` directly and consume `text_delta`; use `ChatPanel` for a full conversation surface.

## UI checks

Keep user-visible text in the Aether i18n contract. Test state behavior with `createFakePorts`, including absent optional ports, cancellation, a single terminal event, approval decisions, and Skill revision conflicts. Verify closed dialogs remove their overlay and restore interaction with the page.
