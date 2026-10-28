# Aether integration

## Minimal host sequence

The exact APIs must be checked against the installed version, but the integration order is stable:

```rust
let root = aether_observability::data_root("my-app")?;
let logs = aether_observability::init(
    &root.join("logs"),
    aether_observability::LogConfig::default(),
    None,
)?;
let host = Arc::new(MyHost::new(/* host dependencies */));
let providers = Arc::new(aether_providers::Store::open(&root)?);
let deps = aether_runtime::RunDependencies {
    providers: Arc::clone(&providers),
    conversations: None,
}; // Supply optional capability fields when their features are enabled.
let runs = aether_runtime::RunService::new(
    host, aether_runtime::RunConfig::default(), deps, AddressPolicy::default(),
)?;
let aether = tauri_plugin_aether::Builder::new(logs)
    .providers(providers)
    .runs(runs)
    .build()?;

tauri::Builder::default()
    .plugin(aether)
    .run(tauri::generate_context!())?;
```

Use no conversation Store only for intentionally stateless runs. For history, share one Arc<dyn ConversationStore> between RunDependencies and ConversationService, and inject .conversations(...). Provider management UI is independent of runs; the service still needs a provider resolver instance. Verify the concrete constructor imports against the installed version.

Initialize observability before `tauri::Builder`; otherwise setup logs from plugins are lost. Create a store only when that feature is enabled. Each module owns its own database; do not add cross-database foreign keys.

## Features and permissions

The plugin features are compile-time gates:

- `providers` → provider commands and `aether:providers`
- chat → four runtime commands and aether:chat; provider management commands are independent
- conversations → independent history commands and aether:chat-history; requires ConversationService, not RunService
- `skills` → Skills commands and `aether:skills`
- `mcp-client` → client commands and `aether:mcp-client`
- `mcp-server` → read and control permission groups
- logs are always available through `aether:logs`

Feature compilation and capability authorization are separate. A static capability must not reference a permission that may be compiled out. Add runtime capabilities for feature-dependent permissions, or keep static files limited to always-present groups.

## KitHost

Implement only the host decisions required by the selected capability:

- `select_model`: application model selection and fallback.
- `system_prompt`: application prompt policy.
- `context`: resolve an application-owned context reference.
- tools(request, run_id): fix tools and capabilities to this server-assigned run.
- run_finished(run_id): release snapshots once after the run lifecycle and in-flight tools end.
- permission_modes and mcp_tools: declare allowed modes and whether MCP is prepared.
- call_tool: execute a selected host tool, preserving unknown side effects.
- authorize(call, context): final in-memory decision over mode proposal; cannot relax catalogue Deny.

Keep synchronous host callbacks short. Long network or file work belongs in the appropriate async operation, not in a blocking callback.

## Lifecycle

On window destruction or application exit, cancel active runs and managed background tasks. Channel send failure is a cancellation signal. Do not leave probe, MCP refresh, or chat tasks detached. Follow the plugin's shutdown contract rather than inventing a second task registry.

## Worker composition

Read `examples/worker/src/index.ts` and docs23 for the installed API. Validate unknown bindings, apply `packages/worker/migrations/0001.sql` explicitly, then construct D1DocumentStore with R2 blobs, R2FileStore, credentials, trusted outbound and selected capability services. Construct WorkerRuntime with WorkerHost, providers, explicit conversations (or null) and an argument validator; export the class from createRunCoordinator and compose createAetherWorkerApp with mandatory identity/authorization/CSRF behavior. Do not turn the example's controlled bearer identities into product login.

The app routes POST preparation to one DO per run. The runtime persists a planned reservation identity before CAS commit, and only the first authenticated SSE subscriber starts execution; expiry/disconnect cancels. Persisted creating/accepted/running runs recover interrupted after restart rather than regenerating secrets or replaying tools. Use HTTPS for public MCP/OAuth and supply real trusted egress plus DNS target checking before remote page fetch; bindings alone are not a security proof. Product routes and resources stay in their host.

## Verification

For a new integration, verify:

1. the selected feature combination compiles;
2. enabled stores are passed and missing stores fail at `build()`;
3. capabilities authorize only the commands actually used;
4. the UI hides absent ports instead of invoking unavailable commands;
5. a real window or controlled CLI smoke path reaches the intended behavior.
