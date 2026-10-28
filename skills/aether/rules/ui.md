# Aether UI integration

## Public imports

Use package exports, not internal file paths:

```ts
import { setKitContext, createDefaultNotify } from '@ouraihub/aether-ui'
import { createTauriPorts } from '@ouraihub/aether-ui/tauri'
import { createWebPorts } from '@ouraihub/aether-ui/web'
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
  ports: createTauriPorts({ providers: true, chat: true, conversations: true }),
  i18n: createI18n('en'),
  notify: createDefaultNotify(),
  logger,
})
```

Use `createTauriPorts` only for desktop; browser hosts inject `createWebPorts({baseURL,fetch,csrf,...capabilityFlags})` with real authentication/fresh-verification behavior in their fetch/identity integration. Use `createFakePorts` in tests. Optional absent capabilities stay hidden/disabled; do not probe unavailable commands or fabricate MCP Server management ports.

Only `@ouraihub/aether-ui/tauri` may import `@tauri-apps/api`. Components and state classes use `KitPorts`, `KitClientError`, and `AbortSignal`.

## DTO and cancellation

Import public DTOs from `@ouraihub/aether-contracts`; the canonical ts-rs output is `packages/contracts/src/generated`. UI ports consume these bindings rather than owning a second generated directory. After Rust DTO changes, use `pnpm kit:dto`, contracts `generate` and the repository checks; commit bindings plus generated index/validators. Never hand-write parallel DTOs.

`RunClient.start` resolves when accepted, not when finished; events may arrive first. Render the single final `end`, pass an AbortSignal, and use `cancel`/`getSnapshot` for explicit control. Cancellation acknowledgement is not proof of a cancelled terminal state. ConversationClient is independent at `ports.conversations`, not nested under chat.

## Component boundaries

- Container components obtain ports and injected services from context.
- Presentation components receive data through props and report intent through callbacks.
- Host preferences, routing, default model selection, and page layout stay in the host application.
- Use existing Aether components before recreating provider, chat, Skills, MCP, or logs UI.
- For base controls, follow the repository's shadcn-svelte skill and existing `components/ui` implementation.

A one-shot generation flow can call `ports.chat.start` directly and consume `text_delta`; use `ChatPanel` for a full conversation surface. Logs do not require local filesystem actions; only offer opening the log directory when `ports.desktop?.openLogsDirectory` exists.

## UI checks

Keep user-visible text in the Aether i18n contract. Test state behavior with `createFakePorts`, including absent optional ports, cancellation, a single terminal event, approval decisions, and Skill revision conflicts. Verify closed dialogs remove their overlay and restore interaction with the page.
