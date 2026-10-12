# Aether architecture

## Boundaries

Aether has three important boundaries:

- Host ↔ Aether: `KitHost`. The host owns model choice, business context, prompts, host tools, and final authorization.
- Tauri ↔ core: `tauri-plugin-aether`. It converts commands, assembles enabled modules, and owns command permissions.
- UI ↔ backend: `KitPorts`. The UI package owns typed ports and components; the Tauri adapter is the only IPC implementation.

Core crates remain reusable outside Tauri. `aether-chat` consumes a resolved connection, system parts, and a tool executor; it does not select providers or implement MCP. Providers, Skills, MCP client, and MCP server do not depend on Chat or on each other.

## Composition root

Concrete stores and host services are created by the application and passed to `tauri_plugin_aether::Builder`. Do not make Aether discover them from globals or application state. A missing enabled instance is a configuration error from `build()`.

A host may provide only the capability it needs. A Providers-only host can implement the default `KitHost` methods. A Chat host normally needs `providers`, while Skills and MCP server are independently selectable.

## Ownership

| Concern | Owner |
| --- | --- |
| Model roles, fallback, business context | Host application |
| Provider persistence, probing, connection resolution | `aether-providers` |
| Protocol streaming, tool loop, budgets, cancellation | `aether-chat` |
| Skill import, preview, revision and snapshot | `aether-skills` |
| External MCP connections and trust | `aether-mcp-client` |
| External MCP exposure and scopes | `aether-mcp-server` |
| Tauri commands, feature gating and permission groups | `tauri-plugin-aether` |
| UI state and layout | Host application plus `@ouraihub/aether-ui` |

Do not move host-specific policy into a generic crate merely to avoid passing it through `KitHost`.

## Dependency checks

Before adding a dependency, verify the direction against the workspace metadata. Avoid a new shared `base` crate. Add a trait only at a real boundary with at least two implementations or a test replacement.
