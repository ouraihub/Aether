---
name: aether
description: Integrates applications with the Aether shared AI foundation. Covers Rust/Tauri services and permissions, TypeScript contracts/core/Cloudflare Worker composition, WorkerHost, HTTP/SSE, D1/R2/DO, @ouraihub/aether-ui ports and components, generated DTOs, streaming chat, Skills, MCP, observability, security, and CI contracts. Triggers when working with Aether packages/crates, Worker/Tauri adapters, KitHost/WorkerHost, KitPorts, createTauriPorts/createWebPorts, ChatPanel, or Aether integration.
user-invocable: false
---

# Aether

Aether is a shared AI foundation with Rust/Tauri desktop services, a TypeScript core, Cloudflare Worker adapters, and Svelte UI. Reusable capabilities belong to Aether; product routes, login, data and final business authorization belong to each host.

## Source of truth

Use the current project files first. Do not invent an API from memory.

1. Current Rust and TypeScript source, especially public `lib.rs` and package `index.ts` files.
2. Canonical generated DTOs in `packages/contracts/src/generated/`, exported by `@ouraihub/aether-contracts` with precompiled validators (never hand-edit them).
3. The project's feature/binding configuration, Tauri capabilities, and `examples/worker` or Native examples.
4. Current contracts: docs00/03 architecture, docs22 wire, docs23 Worker composition/HTTP, docs29 platform security, docs25 Native runtime, docs26 conversations, docs15 IPC and docs16 clients; docs24 indexes capabilities. Actual verification lives in docs90, not design claims.
5. Historical proposals only for background, never as current API documentation.

## Detect the project before changing it

Look for:

- `tauri-plugin-aether` in `Cargo.toml` and its enabled features.
- `@ouraihub/aether-ui` in `package.json` and its subpath imports.
- `packages/ui/src/ports/`, `packages/ui/src/tauri/`, and capability files.
- `@ouraihub/aether-contracts`, `@ouraihub/aether-core`, `@ouraihub/aether-worker`, `packages/ui/src/web`, and Worker environment bindings.
- `KitHost`, `tauri_plugin_aether::Builder`, `aether_observability::init`, and `Store::open` calls.
- `tauri.conf.json` and `src-tauri/capabilities/*.json`.
- Existing examples or application-specific host implementations.

Infer the actual configuration from these files. Do not add a second configuration file just for the assistant.

## Capability selection

Choose the smallest feature set that satisfies the requirement:

| Requirement                           | Features                                     | Required integration                                                                      |
| ------------------------------------- | -------------------------------------------- | ----------------------------------------------------------------------------------------- |
| Model/provider configuration          | `providers`                                  | provider store and provider UI/ports                                                      |
| Streaming generation                  | runtime/runs + plugin/chat                   | construct RunService with KitHost and shared dependencies, inject via Builder.runs        |
| Persistent conversations              | runtime/conversations + plugin/conversations | share one Store with RunService and ConversationService; inject independent conversations |
| Skills in a run                       | add `skills`                                 | skill store and selected `id + revision` refs                                             |
| External MCP tools                    | add mcp-client                               | shared McpRuntime over Store, trust and approval policy                                   |
| Expose host tools to external clients | `mcp-server`                                 | explicit tools, tokens, scopes                                                            |

Plugin chat exposes runs but does not implicitly expose provider configuration commands; conversations is independent. Skills and MCP client/server remain independent. Observability is always present in the plugin.

Worker hosts explicitly construct capability services and `WorkerRuntime`, export the class from `createRunCoordinator(env => runtime)`, then compose `createAetherWorkerApp` with identity, RUNS, mandatory route authorization/CSRF callbacks and owned handlers. Use `createWebPorts` for browser HTTP/SSE. This is implemented, not a future TS scaffold; do not import UI from Core/Worker or migrate product business into Aether. Follow docs23 and the real example rather than inventing a second router/port registry.

## Required workflow

1. Identify whether the change is in Rust, TS Core/Contracts, Worker/Tauri integration, UI ports/components, host business code, or tests.
2. Read the relevant public entry point and current example before writing code.
3. Select features, stores, commands, permissions, and UI ports as one contract.
4. Keep host policy in `KitHost`/`WorkerHost` and application state; keep reusable capability behavior in Aether.
5. Use generated DTOs and existing ports/components. Do not create parallel contracts.
6. Update capabilities, i18n, tests, and docs when the public contract changes.
7. Run the narrow smoke scenario, then the applicable repository checks.

## Non-negotiable rules

- Core crates must not depend on Tauri. Full run composition belongs in aether-runtime; the plugin adapts commands, permissions and window lifecycle only.
- Initialize `aether_observability` before `tauri::Builder`.
- Do not use global singletons, plugin-order probing, or `try_state` discovery for Aether services.
- A feature does not grant a window permission. Capabilities must name the exact enabled permission groups.
- Only `@ouraihub/aether-ui/tauri` may import `@tauri-apps/*`; other UI code consumes ports.
- DTOs come from `@ouraihub/aether-contracts` (`packages/contracts/src/generated`). After Rust DTO changes, use `pnpm kit:dto`, then contracts `generate`; check bindings and generated index/validators without duplicating DTOs in UI.
- Secrets and raw diagnostic bodies never enter logs or ordinary runtime DTOs. User/model text and authorized result payloads are functional UI data, not diagnostic fields; explicit credential management has its own authorization boundary.
- Selecting a Skill does not grant tool permissions. A run uses a fixed Skill snapshot identified by `id + revision`.
- Chat request failures before acceptance return as command errors. Failures after acceptance are reported by the terminal `end` event.
- `AbortSignal` must cancel the corresponding backend operation. A cancelled tool with possible side effects is `unknown`, not successfully undone.
- Worker has one DO per run, POST preparation then first valid SSE subscription starts execution, 30-second acceptance expiry, no replay, and creating/accepted/running restart recovery to interrupted. Persist planned reservation identity before CAS; never persist prepared plaintext connections or replay tools after restart.
- Production remote egress needs a trusted connect-time address-policy gateway; Search target fetching also needs all-address DNS preflight. Binding declarations alone prove neither security nor DNS pinning. Public MCP/OAuth is HTTPS-only; local HTTP AI smoke is not OAuth proof. See docs29 and security rules.
- Use `createFakePorts()` for UI tests and local controlled upstreams for network tests. Do not test forwarding or source text.

## References

Read only the topic needed for the task:

- [architecture.md](./rules/architecture.md) — boundaries and dependency direction
- [integration.md](./rules/integration.md) — Rust host, Builder, features, permissions, lifecycle
- [ui.md](./rules/ui.md) — ports, components, context, DTOs, and cancellation
- [security.md](./rules/security.md) — secrets, MCP trust, approvals, Skills, and logs
- [testing.md](./rules/testing.md) — behavior tests and repository gates
