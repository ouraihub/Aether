# Aether security rules

## Secrets

API keys, MCP headers, query credentials, and issued tokens are backend data. The frontend may submit a new secret or receive `hasKey`/masked metadata, but must not receive or persist plaintext secrets. Never put credentials in URLs, logs, DTO details, screenshots, or error messages.

Do not add an export-secret, show-plaintext, arbitrary SQL, arbitrary file-write, or arbitrary command-execution path. Use the existing encrypted store and explicit typed commands.

## Tool authorization

A tool's source and default approval are not final authorization. The host's `authorize` decision is final. MCP client tools require configured server trust and an approval policy. Do not make write or destructive tools automatic by default.

For MCP server, expose only explicitly registered tools. Tokens are hashed, scopes are checked against enabled scopes, and destructive scopes require deliberate UI confirmation. `mcp-server-read` and `mcp-server-control` are separate permission groups.

## Skills

A Skill is instruction content, not a permission grant. Selecting a Skill must not add tools or broaden authorization. At run start, resolve and freeze each selected `id + revision`; if the revision changed, reject with the documented conflict and ask the user to review it again. Do not silently truncate over-budget Skill content.

Imported Skill content is untrusted input. Preserve the framework's size, format, digest, and content-change checks. Never execute scripts from a Skill package or treat its prose as a host policy.

## Logging and errors

Log IDs, counts, statuses, durations, and safe error codes. Never log secrets, user input, model output, Skill bodies, or raw tool arguments. Use structured `tracing` in Rust and the injected `KitLogger` in TypeScript; do not use `console.*` in UI code.

Use typed error codes, not message matching. Before a run is accepted, return a command error. After acceptance, emit exactly one terminal `end` event. If a side-effecting operation is cancelled after dispatch, report `unknown` rather than claiming rollback.

## Tauri boundary

Capabilities are an authorization layer, not a feature detector. Grant only the permissions needed by the compiled feature set and each window. The frontend cannot declare or elevate its own permissions.
