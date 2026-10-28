# Aether architecture

## Boundaries

Aether has three important boundaries:

- Host ↔ Aether: Native `KitHost` or TS `WorkerHost`. The host owns model choice, business context, prompts, tools and final authorization.
- Adapter ↔ services: Tauri binds window owners/permissions to aether-runtime; native-fetch Worker composition binds verified actors to WorkerRuntime and one Durable Object per run.
- UI ↔ backend: `KitPorts`. Tauri uses `createTauriPorts`; browser HTTP/SSE uses `createWebPorts`. Neither adapter owns the model/tool execution algorithm.

Core crates remain reusable outside Tauri. `aether-chat` consumes a resolved connection, system parts, and a tool executor; it does not select providers or implement MCP. Providers, Skills, MCP client, and MCP server do not depend on Chat or on each other.

## Composition root

The application constructs capability instances, RunService and an independent ConversationService, then injects them into the adapter. Do not discover dependencies from globals or plugin order. Configuration-only hosts need no empty KitHost; conversations can be consumed without starting a model runner.

TS ownership: `packages/contracts` exports canonical generated DTOs (`src/generated`) and CSP-safe precompiled validators; `packages/core` owns protocol/RunEngine and injected capability execution; `packages/worker` owns platform adapters, runtime and public routes. Core/Worker never depend on UI or product types. D1 stores actor-scoped JSON with revision CAS; R2 stores immutable large blobs/files, intentionally retained until explicit safe GC. No callback transaction or cross-service atomicity claim.

## Ownership

| Concern                                              | Owner                                       |
| ---------------------------------------------------- | ------------------------------------------- |
| Model roles, fallback, business context              | Host application                            |
| Provider persistence, probing, connection resolution | `aether-providers`                          |
| Protocol streaming, tool loop, budgets, cancellation | `aether-chat`                               |
| Complete run lifecycle and independent conversations | aether-runtime                              |
| Independent image generation and editing             | aether-images                               |
| Skill import, preview, revision and snapshot         | `aether-skills`                             |
| External MCP connections and trust                   | `aether-mcp-client`                         |
| External MCP exposure and scopes                     | `aether-mcp-server`                         |
| Tauri commands, feature gating and permission groups | `tauri-plugin-aether`                       |
| UI state and layout                                  | Host application plus `@ouraihub/aether-ui` |

Do not move host-specific policy into a generic crate merely to avoid passing it through `KitHost`.

## Dependency checks

Before adding a dependency, verify the direction against the workspace metadata. Avoid a new shared `base` crate. Add a trait only at a real boundary with at least two implementations or a test replacement.
