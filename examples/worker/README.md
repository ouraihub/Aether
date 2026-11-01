# Worker host example

Real Cloudflare Worker composition using D1 documents, R2 files/large documents and a SQLite Durable Object per run. Providers, search, skills, images, conversations, MCP client settings and HTTPS-only public MCP/OAuth use the public packages. No model response, storage or identity fallback is fabricated.

## Local development

From the repository root (Node 24+, pnpm):

```sh
pnpm install
pnpm --dir examples/worker init:local
pnpm --dir examples/worker exec wrangler d1 migrations apply AETHER_DB --local
pnpm --dir examples/worker dev
```

`init:local` creates ignored `examples/worker/.dev.vars` with cryptographically random secrets and refuses to overwrite an existing file. Keep that file private. Values are single-quoted literals (`KEY='value'`) so Wrangler passes JSON such as `CREDENTIAL_KEYS` through unchanged; the smoke script reads the same format and rejects anything else. A `.dev.vars` generated before this format must be deleted and regenerated. Wrangler listens on **127.0.0.1:8787** and persists local D1/R2/DO data under `.wrangler/state`. Migrations reference `packages/worker/migrations` directly: there is no second schema copy and requests never migrate the database.

Default outbound access is restricted to the explicit literal-loopback origin `http://127.0.0.1:9000`. Start your actual tool-capable model service there, or set `OUTBOUND_ORIGINS` to the exact origins of your own configured services. The example does not start a fake upstream. Remote origins require the trusted gateway described below, even in local development.

All API requests, including `/health`, require `Authorization: Bearer <SMOKE_TOKEN>`. The second configured token selects another independent subject, `actor-b`; the first selects `actor-a`. Wrong/missing tokens are rejected. Tokens are never accepted in URLs or cookies. Cookie authentication is intentionally absent, and the mandatory CSRF callback denies it rather than pretending to verify a token. Each actor gets explicit API read/write and marker read/write scopes; the app authorizes those scopes before dispatch. `/api/v1/host/markers` is an authenticated inspection route for this controlled example only.

Configure provider catalog and encrypted keys through `/api/v1/providers` and `/api/v1/providers/keys`, then send a `model: {endpointId, modelId}` in the run request. Alternatively configure `DEFAULT_ENDPOINT_ID` and `DEFAULT_MODEL_ID` from the saved catalog. Image generation uses explicitly configured `PAINTING_ENDPOINT_ID`/`PAINTING_MODEL_ID`. Provider/search/skill/MCP settings are actor-scoped and persist across requests. Keys are AES-GCM sealed with actor/application/purpose authenticated data, never stored plaintext.

Host tool `read_marker` returns the current actor's unpredictable configured marker. `write_marker` always requires approval, including full-access mode; after approval it writes an immutable R2 JSON file and CAS-updates the actor's D1 marker document. Failed/uncertain multi-service writes may leave an unreferenced R2 file; they never report success without the D1 commit. Host arguments have explicit exact-field predicates, while dynamic MCP/built-in schemas use the CSP-safe core JSON Schema interpreter. Models only see their current actor's tool boundary.

## Smoke check against a real model

With Wrangler running, set these environment variables in the terminal running the smoke command:

```sh
export SMOKE_PROVIDER_URL=http://127.0.0.1:9000/v1
export SMOKE_MODEL_ID=your-actual-tool-capable-model
# If the real provider requires a key:
export SMOKE_PROVIDER_KEY=your-provider-key
pnpm --dir examples/worker smoke
```

PowerShell uses `$env:SMOKE_PROVIDER_URL='http://127.0.0.1:9000/v1'` and analogous assignments. The script reads generated secrets from `.dev.vars` automatically; process environment variables override them. Optional `SMOKE_PROTOCOL` defaults to `openai-chat`. It creates a provider through HTTP, consumes live SSE, asserts an actual marker tool result, approves a write, inspects persisted D1/R2 effects, checks cross-actor isolation, stored history, cancellation and replay rejection. A real model that refuses the requested tools causes the smoke check to fail; the Worker does not special-case smoke prompts. The smoke run intentionally leaves persisted records for inspection.

### HTTPS and real MCP OAuth

Public MCP/OAuth remain unavailable on HTTP, including local HTTP. For the complete smoke path stop the HTTP dev server and run:

```sh
pnpm --dir examples/worker dev:https
export SMOKE_ORIGIN=https://127.0.0.1:8787
# Trust Wrangler's local certificate in Node using your development CA:
export NODE_EXTRA_CA_CERTS=/absolute/path/to/your/local-ca.pem
pnpm --dir examples/worker smoke
```

Configure a locally trusted certificate via Wrangler `--https-key-path` / `--https-cert-path` if needed. Do not disable certificate verification for deployment. The HTTPS smoke additionally enables the actor's MCP settings with explicit `enabledScopes`, registers a real client, obtains the consent form under authenticated host identity, posts its one-use CSRF with explicit approval, exchanges PKCE code for a scoped token, calls public `read_marker`, and verifies that the read-only OAuth grant cannot dispatch `write_marker`. Login is not inferred from an MCP token; OAuth consent requires the configured host bearer token. Browser products must replace this controlled bearer host with their own real login/session/CSRF implementation.

## Configuration and secrets

Nonsecret deployment vars:

| Variable | Meaning |
| --- | --- |
| `APPLICATION_ID`, `APP_ORIGIN` | Exact application identity and canonical origin; production requires HTTPS. |
| `TEST_MODE` | Literal `true` only permits configured smoke identities on a literal-loopback host. Production must be `false`. |
| `ALLOW_LOOPBACK_HTTP`, `ALLOW_LOOPBACK_OUTBOUND` | Separate local-only switches, ignored outside test mode. Outbound still needs an explicitly allowlisted origin. |
| `OUTBOUND_ORIGINS` | Nonempty JSON array of exact origin strings; no wildcard or unrestricted fetch. Include provider/search/MCP service origins explicitly. |
| `CREDENTIAL_CURRENT_KEY` | Current key-ring ID for new encryption. |
| `DEFAULT_ENDPOINT_ID`, `DEFAULT_MODEL_ID` | Optional saved chat model default. |
| `PAINTING_ENDPOINT_ID`, `PAINTING_MODEL_ID` | Optional saved image model selection. |

Secrets:

| Secret | Meaning |
| --- | --- |
| `CREDENTIAL_KEYS` | JSON object mapping key IDs to base64-encoded random **32-byte** AES keys. Imported as nonextractable CryptoKeys. Keep older IDs until stored credentials have been explicitly rotated. |
| `SMOKE_TOKEN`, `SMOKE_TOKEN2` | Independent high-entropy local test-only actor selectors. Only used with `TEST_MODE=true`. |
| `IDENTITY_TOKEN`, `IDENTITY_TOKEN2` | Explicit configured bearer identities outside test mode; first required, second optional. This controlled host does not provide general-purpose user login. |
| `MARKER_A`, `MARKER_B` | Independent unpredictable values, at least 32 characters; B required if the second identity is configured. |
| `REAUTH_TOKEN` | Optional separate high-entropy fresh-verification secret. Without it, secret reveal is denied. Provider reveal requires `X-Reauth-Token`; search reveal verifies its supplied password against this value. Never reuse an identity token. |

Never put real secrets in `wrangler.jsonc`, source control, browser bundles, logs or requests to unrelated origins. This example exposes read/write scope to configured tokens, not to anonymous callers. Rotate/revoke tokens at the host/deployment boundary.

## Production bindings and migration

`wrangler.jsonc` has a separate `production` environment with local flags disabled. Set its real D1 database UUID, R2 bucket, canonical HTTPS origin and exact outbound origins. The checked-in zero UUID and example hostname are configuration references, not working deployment credentials. Create real resources before deployment:

```sh
pnpm --dir examples/worker exec wrangler d1 create aether-worker-example
pnpm --dir examples/worker exec wrangler r2 bucket create aether-worker-example
# After replacing production database_id with the returned UUID:
pnpm --dir examples/worker exec wrangler d1 migrations apply AETHER_DB --remote --env production
pnpm --dir examples/worker exec wrangler secret put CREDENTIAL_KEYS --env production
pnpm --dir examples/worker exec wrangler secret put IDENTITY_TOKEN --env production
pnpm --dir examples/worker exec wrangler secret put MARKER_A --env production
# Configure CREDENTIAL_CURRENT_KEY as a nonsecret var matching the JSON key-ring ID.
pnpm --dir examples/worker exec wrangler deploy --env production
```

Bindings: `AETHER_DB` (D1), `AETHER_FILES` (R2), `RUNS` (RunCoordinator), `OUTBOUND_GATEWAY` (trusted service). The gateway must enforce **public-only addresses at connect time**, for every A/AAAA result including rebinding, preserve the requested upstream URL, reject redirects and not act as an open proxy. The example never substitutes global fetch for remote egress; an absent gateway fails closed. A separately administered gateway must already exist under the configured service name; deploying this example does not invent that security guarantee.

Search page fetching additionally requires `TARGET_CHECKER`, an optional trusted service binding with `POST https://aether-target-check.internal/check`, JSON `{url}`, returning **204 only when every resolved address passes public-address policy** and rejecting private/special/unknown/failed DNS. Add the binding to the relevant environment before enabling remote fetch. Only explicitly configured literal loopback targets bypass this preflight in test mode. Content proxies still independently enforce connect-time address policy.

The DO persists run ownership/session identity, terminal snapshots and approval state, never plaintext provider credentials. Isolate-local services are memoized by the explicit environment object only. Restarted accepted/running executions fail interrupted instead of replaying tools. SSE has one active subscriber, no event replay, and must be subscribed within its acceptance deadline.

## Build and typecheck

```sh
pnpm --dir examples/worker typecheck
pnpm --dir examples/worker build
# Equivalent deploy bundle without publishing:
pnpm --dir examples/worker dry-run
```

Typecheck includes only Worker `src`, not a UI directory. Build/dry-run bundles into ignored `dist`; neither runs migrations nor publishes. Health validates the deployed D1 schema and reports whether MCP is available over the configured transport.
