# Public wire contract assets

Normative field/behavior owner: [docs/22](../docs/22-详细设计-跨运行时公共契约.md), with dependencies owned by the documents it references. `manifest.json` declares contract version 1, each type owner, schema references, scope and Rust output coverage. These are Draft07 schemas; generated TypeScript is a checked binding, not the specification.

The canonical TypeScript package is `@ouraihub/aether-contracts`: `pnpm kit:dto` exports Rust bindings into a temporary directory, formats them to the project's 80-column policy, then writes `packages/contracts/src/generated`. Ordinary Rust export tests write raw bindings under `target/ts-rs` and cannot overwrite canonical sources. UI and runtime packages import this package; there is no UI-local generated copy. Existing search client DTOs live in `packages/contracts/src/search.ts` until their Rust owner exports equivalent bindings.

`pnpm --filter @ouraihub/aether-contracts generate` compiles the normative schemas with AJV standalone and generates capability validators from the TypeScript declarations. Normative schemas take precedence over binding-derived shapes for the shared run, error, image and conversation contracts. The committed validator module is self-contained ESM and performs no runtime schema compilation, `eval`, or dynamic function construction. Public predicates include `validateRunRequest`, `validateRunEvent`, `validateRunAccepted`, `validateRunSnapshot`, and `isContract(name, unknown)`; validation errors are available separately and never throw backend-specific error classes.

`pnpm --filter @ouraihub/aether-contracts check-generated` checks reproducibility. The full data gate below also compares standalone validation with schema validation for every shared fixture and expanded boundary, while retaining all TypeScript assignments and actual Rust serialization checks.

## Run the data/binding gate

```sh
pnpm kit:dto
node scripts/contracts-check.mjs
```

The script uses the root's existing pinned `ajv` 8.20.0 and `typescript` 5.9.3, with no new dependencies. It:

1. Compiles every manifest contract with AJV's Draft07 implementation in strict mode, without coercion, defaults or unknown-field removal.
2. Runs shared positive/negative `cases.json`, Unicode length boundaries, and the decoded 4 MiB image boundary. Boundary descriptors avoid committing megabytes of repeated characters; consumers expand `unit` exactly `limit` and `limit + 1` times on a clone of the named base case.
3. Creates an in-memory TypeScript source importing real generated DTOs and assigns all legal literals and the `tsReject` invalid literals. Compiler diagnostics are attributed to each assignment's source range: legal fixtures require none, invalid fixtures require at least one, and diagnostics outside those expected invalid ranges fail. No suppression directives, generated-source text matching or type assertions are used. Limits/provenance/dependent states are intentionally not marked `tsReject` because TypeScript's primitive types do not express them.
4. Builds `cargo build -p aether-runtime --features runs --example contract-output --quiet`, then directly executes `debug/examples/contract-output` (`.exe` on Windows) under `CARGO_TARGET_DIR` or the repository's `target` directory. Build/MSVC wrapper logs are inherited, never mixed into parsed JSON or trimmed off. Windows uses the same desktop-kit MSVC wrapper as `scripts/dto-check.mjs`. The executable's actual public Rust serialization must cover every event kind, all four snapshot statuses, accepted/error/conversation outputs.

The final line reports exercised data/binding/Rust counts, not backend behavior or future Worker compatibility. During migration, a missing generated `RunSnapshot` is a binding diagnostic until `pnpm kit:dto` runs; the gate does not silently skip missing types.

Already captured real Rust output can replace the cargo invocation:

```sh
node scripts/contracts-check.mjs --rust-output path/to/serialized.json
```

`--rust-output -` reads UTF-8 JSON from stdin. The format is a nonempty array of `{ "contract": "RunEvent", "value": <actual serde_json value> }` records, not fixtures echoed by an adapter. Entries reject unknown envelope fields and must collectively satisfy the manifest coverage requirements. This path supports other runtimes producing the same public values, but does not itself prove where a file originated.

Root package scripts and CI integration are owned by the main integrator; invoke this gate after DTO generation/binding freshness checks. No checks were run while these assets were authored.

## Data policy

- Every structured request **and response** rejects unknown fields recursively. `tool_result.payload` alone permits arbitrary JSON, including null: host authorization and budgets apply before projection. Schema cannot detect secrets inside otherwise legal feature strings/payload; behavioral leak checks remain required.
- Optional fields may be omitted; null is accepted only where declared. The schemas never insert defaults. Explicit nullable required fields (accepted IDs, token counters, display image URL) must still be present.
- Permission modes are `default`, `plan`, `accept_edits`, `auto`, `full_access`. Unknown enums and obsolete `providerId`/context-array/transport request fields reject.
- IDs retain opaque existing values, without a ULID-only pattern. Conversation/action IDs use the existing nonempty 128-character bound. Host-defined context/model/skill IDs do not gain unrelated format restrictions.
- Token counts/revisions use existing `u32` bounds. Timestamps are finite safe integers; details limits use safe nonnegative integers, status/port use existing `u16` bounds. Cost is nonnegative and USD/CNY only.
- Image input checks supported MIME, canonical padded standard base64 without a data prefix and decoded byte upper bound. Actual image decoding/content, vision support and effective-source image selection are service behavior. `ImageRef.url` provenance cannot be established from a string schema.
- The base64 pattern uses ECMA-262 lookahead/backreference chunks to avoid per-byte V8 backtracking stack growth at the 4 MiB boundary. Canonical padding/alphabet, full-string matching and decoded-size constraints remain enforced; large malformed endings are also checked. This is standard Draft07 `pattern`, not a custom AJV keyword. Validation exceptions report the fixture label and stack frames without dumping input values.
- Request action combinations, authority/membership, effective source selection, purpose/capability policy and all timing are explicitly outside field-schema validation, not silently accepted backend operations.

## Shared behavior scenarios

`scenarios.json` describes concrete setup, ordered actions and observable assertions for Rust service tests and a future TS backend runner. `requestCase` references the shared legal fixture; no complete TS Worker is added or claimed. Runners must use real services with controlled model/tool/store boundaries and observe dispatch, commits, events and snapshots. Merely loading scenario descriptions or returning their expected values is not execution.

Coverage includes early event/acceptance races, preaccept atomicity, effective request/CAS, approval decisions/expiry, call-ID conflicts, unknown no-replay, confirmed error continuation, round exhaustion, partial output failure, save-before-End/save failure, cancel intent, owner isolation, 128-terminal completion-order eviction, receiver failure, safe projections, conversation branch actions and host-terminal permission authority. Platform/real UI/network smoke checks remain separate.
