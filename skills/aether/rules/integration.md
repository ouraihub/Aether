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

let aether = tauri_plugin_aether::Builder::new(host, logs)
    .providers(aether_providers::Store::open(&root)?)
    .chat(tauri_plugin_aether::ChatConfig::default(), None)
    .build()?;

tauri::Builder::default()
    .plugin(aether)
    .run(tauri::generate_context!())?;
```

Use `.chat(..., None)` only for intentionally stateless chat. For persistent history, pass `Some(Arc<dyn ConversationStore>)` backed by the application's chosen store. Do not call `.chat()` without `.providers()`.

Initialize observability before `tauri::Builder`; otherwise setup logs from plugins are lost. Create a store only when that feature is enabled. Each module owns its own database; do not add cross-database foreign keys.

## Features and permissions

The plugin features are compile-time gates:

- `providers` → provider commands and `aether:providers`
- `chat` → chat commands and `aether:chat`; it also enables providers
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
- `tools`: return the tools allowed for this request.
- `call_tool`: execute a selected host tool.
- `authorize`: final approval decision; it may restrict Aether defaults.

Keep synchronous host callbacks short. Long network or file work belongs in the appropriate async operation, not in a blocking callback.

## Lifecycle

On window destruction or application exit, cancel active runs and managed background tasks. Channel send failure is a cancellation signal. Do not leave probe, MCP refresh, or chat tasks detached. Follow the plugin's shutdown contract rather than inventing a second task registry.

## Verification

For a new integration, verify:

1. the selected feature combination compiles;
2. enabled stores are passed and missing stores fail at `build()`;
3. capabilities authorize only the commands actually used;
4. the UI hides absent ports instead of invoking unavailable commands;
5. a real window or controlled CLI smoke path reaches the intended behavior.
