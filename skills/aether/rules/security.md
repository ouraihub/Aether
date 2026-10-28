# Aether security rules

## Secrets

API keys, MCP headers/query credentials and issued tokens are backend data. Ordinary lists/runtime DTOs expose masks or presence only. Existing Provider/Search explicit reveal is permitted only after fresh host authentication; keep it transient and clear stale/closed credential sessions. MCP client credentials remain write-only. Never put credentials in URLs, logs, DTO details, screenshots or errors.

Do not invent another export-secret, arbitrary SQL/file-write or command-execution path. Reuse encrypted stores and explicit typed operations; UI possession of an identity token is not fresh verification.

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

## Worker boundary

Use AES-GCM credentials with explicit key ring/current ID and AAD bound to application/tenant/subject/purpose/recordId; retain older keys until explicit rotation. Prepared plaintext stays in memory, never DO metadata. D1 JSON CAS and immutable R2 blobs do not form a distributed transaction; superseded/uncertain blobs intentionally remain until reference-aware GC with a safe retention period.

Remote outbound requires an allowlisted trusted gateway enforcing public-address policy at connect time for every A/AAAA, including rebinding, with no redirects/open proxy. Search target fetching additionally needs trusted all-address DNS preflight; preflight is not connection pinning. A configured binding or a security string is not evidence that the deployed service enforces this. Literal-loopback test switches must not be used in production.

Public MCP/OAuth requires exact HTTPS origin, PKCE, explicit consent/CSRF, scoped grants and host resource authorization. Do not infer host login from MCP bearer credentials, or claim local HTTP proved OAuth. Worker run ownership also binds session; restart fails interrupted and never replays model/tool side effects. Actual UI/HTTPS/restart/CI evidence belongs in docs90.
