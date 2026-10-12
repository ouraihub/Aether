---
name: aether
description: Integrates applications with the Aether AI desktop framework. Covers tauri-plugin-aether feature composition, KitHost and Builder setup, @ouraihub/aether-ui ports and components, generated DTOs, streaming chat, Skills, MCP, observability, security boundaries, testing, and CI contracts. Triggers when working with Aether crates, Tauri features or permissions, KitHost, KitPorts, createTauriPorts, ChatPanel, or Aether integration.
user-invocable: false
---

# Aether

Aether is a Rust + Tauri 2 framework for AI desktop applications. It separates reusable AI capabilities from host business logic and exposes the UI through typed ports.

## Source of truth

Use the current project files first. Do not invent an API from memory.

1. Current Rust and TypeScript source, especially public `lib.rs` and package `index.ts` files.
2. Generated DTOs in `packages/ui/src/ports/generated/` (never hand-edit them).
3. The project's feature configuration, Tauri capabilities, and examples.
4. Aether design contracts: `docs/00-概要设计.md`, `docs/15-详细设计-接入层.md`, and `docs/16-详细设计-kit-ui.md` when working in the Aether repository.
5. Historical proposals only for background, never as current API documentation.

## Detect the project before changing it

Look for:

- `tauri-plugin-aether` in `Cargo.toml` and its enabled features.
- `@ouraihub/aether-ui` in `package.json` and its subpath imports.
- `packages/ui/src/ports/`, `packages/ui/src/tauri/`, and capability files.
- `KitHost`, `tauri_plugin_aether::Builder`, `aether_observability::init`, and `Store::open` calls.
- `tauri.conf.json` and `src-tauri/capabilities/*.json`.
- Existing examples or application-specific host implementations.

Infer the actual configuration from these files. Do not add a second configuration file just for the assistant.

## Capability selection

Choose the smallest feature set that satisfies the requirement:

| Requirement | Features | Required integration |
| --- | --- | --- |
| Model/provider configuration | `providers` | provider store and provider UI/ports |
| Streaming generation | `providers,chat` | `KitHost`, provider store, chat config |
| Persistent conversations | `providers,chat` | pass a `ConversationStore` to `.chat(...)` |
| Skills in a run | add `skills` | skill store and selected `id + revision` refs |
| External MCP tools | add `mcp-client` | MCP store, trust, approval policy |
| Expose host tools to external clients | `mcp-server` | explicit tools, tokens, scopes |

`chat` enables `providers`. Skills and MCP client/server are independent. Observability is always present in the plugin.

## Required workflow

1. Identify whether the change is in the Rust core, Tauri integration, UI ports, UI components, host application, or tests.
2. Read the relevant public entry point and current example before writing code.
3. Select features, stores, commands, permissions, and UI ports as one contract.
4. Keep host policy in `KitHost` and application state; keep reusable behavior in Aether.
5. Use generated DTOs and existing ports/components. Do not create parallel contracts.
6. Update capabilities, i18n, tests, and docs when the public contract changes.
7. Run the narrow smoke scenario, then the applicable repository checks.

## Non-negotiable rules

- Core Aether crates must not depend on Tauri. Cross-module composition belongs in `tauri-plugin-aether`.
- Initialize `aether_observability` before `tauri::Builder`.
- Do not use global singletons, plugin-order probing, or `try_state` discovery for Aether services.
- A feature does not grant a window permission. Capabilities must name the exact enabled permission groups.
- Only `@ouraihub/aether-ui/tauri` may import `@tauri-apps/*`; other UI code consumes ports.
- DTO types come from `ports/generated`; regenerate and run the DTO check after Rust DTO changes.
- API keys, tokens, user text, model output, skill bodies, and raw tool arguments must not enter logs or frontend DTOs.
- Selecting a Skill does not grant tool permissions. A run uses a fixed Skill snapshot identified by `id + revision`.
- Chat request failures before acceptance return as command errors. Failures after acceptance are reported by the terminal `end` event.
- `AbortSignal` must cancel the corresponding backend operation. A cancelled tool with possible side effects is `unknown`, not successfully undone.
- Use `createFakePorts()` for UI tests and local controlled upstreams for network tests. Do not test forwarding or source text.

## References

Read only the topic needed for the task:

- [architecture.md](./rules/architecture.md) — boundaries and dependency direction
- [integration.md](./rules/integration.md) — Rust host, Builder, features, permissions, lifecycle
- [ui.md](./rules/ui.md) — ports, components, context, DTOs, and cancellation
- [security.md](./rules/security.md) — secrets, MCP trust, approvals, Skills, and logs
- [testing.md](./rules/testing.md) — behavior tests and repository gates
