# Scenario runs: shared output and comparison

`scenarios.json` is executed on both backends by a runner each:

| Backend | Runner | Real service under test |
| --- | --- | --- |
| Native | `crates/aether-runtime/examples/scenario-output.rs` | `RunService` + `aether-chat` engine; controlled model on loopback HTTP (`aether-testkit`) |
| Worker | `packages/worker/scenarios/run.ts` | `RunCoordinator` + `WorkerRuntime` + core `RunEngine`, in Node; controlled model behind `OutboundHttp` |

`scripts/contracts-check.mjs` builds and runs both in the same invocation, validates every observed public value against the manifest schemas, checks each scenario's assertions, and compares the two backends. A runner never decides pass/fail and never reads expected values; inputs come from `scenario-fixtures.json`.

## Runner output

One JSON document on stdout:

```ts
interface RunnerOutput {
  backend: 'native' | 'worker'
  scenarios: Record<string, Observation> // keyed by scenarios.json id
}
interface Observation {
  accepted: RunAccepted[] // RunAccepted values delivered to the caller, in order
  events: RunEvent[] // events the receiver got, raw, in order
  snapshots: { label: string; value: RunSnapshot }[] // snapshots read at named points
  replies: { op: string; status: string; reason?: string }[] // 'ok', or the KitError code plus details.reason
  modelRequests: number // requests the controlled model received
  dispatches: string[] // callIds the host actually executed, in order
  marks: string[] // barrier labels in the order the runner passed them
}
```

- The owner, model replies and call ids are fixed by the fixtures. Generated ids are printed as `<run>`; `conversationId`/`turnId` keep `null` or become `<conversation>`/`<turn>`.
- `events` is the receiver's view: once the receiver is closed, later events are absent.
- Barriers, not sleeps: the runner releases the controlled model, host and receiver at named points and records each point in `marks`. A barrier that is not reached within 10 s fails the runner with the missing label.

## Procedures

A string `args` value written as `{ "prefix", "repeat", "count" }` means `prefix + repeat × count`. Model reply fields: `reasoning`/`text` are deltas in order, `calls` tool calls; `gate` holds everything after the reply's first delta (`afterGate` too) until the runner opens it.

**acceptance-response-race** — no history store.

1. Start; the backend accepts and the runner holds the `RunAccepted` value undelivered → `backend-accepted`. Native: `RunService::start` returned. Worker: `/accept` answered 202 and the receiver subscribed (execution starts on subscription).
2. Open gate `model` → `model-released`; wait for `end` → `end-received`.
3. Deliver the held value (append to `accepted`) → `accepted-delivered`; read snapshot `final`.

**tool-approval-allow** — `read_note` is `ask`; the argument preview source exceeds 2000 characters.

1. Wait for `approval_required` → `approval-required`; mark `dispatches-before-approval:<n>`.
2. Approve (allow) as owner → reply `approve`; approve again → reply `approve-again`.
3. Wait for `end` → `end-received`; read snapshot `final`.

**receiver-send-failure** — `read_note` is `auto`; reply 1 sends `first`, then waits for gate `model`, then sends `second` and a tool call.

1. Wait until the receiver got `text_delta` `first` → `first-delta`.
2. Close the receiver so the next send fails → `receiver-closed`. Native: the sink returns `Err` from then on. Worker: the subscriber stream is cancelled.
3. Open gate `model` → `model-released`; wait for a terminal snapshot → `terminal`; read snapshot `final`.

**round-exhaustion** — `maxRounds: 2`, `read_note` is `auto`, a fresh call id per round.

1. Wait for `end` → `end-received`; read snapshot `final`.

## Assertions

| Scenario | Checks |
| --- | --- |
| acceptance-response-race | marks in the order above; `accepted` has one value with `conversationId` and `turnId` null; joined `text_delta` = `Hello world`; joined `reasoning_delta` = `Let me think.`; snapshot `final` completed; one `end`, last, completed |
| tool-approval-allow | `tool_call` before `approval_required`; `argsPreview` ≤ 2000 Unicode characters; `dispatches-before-approval:0`; `dispatches` = `["call-approve"]`; reply `approve` ok; reply `approve-again` is `conflict` or `not_found`; `tool_result` outcome `ok`, summary `note contents`; one `end`, last, completed |
| receiver-send-failure | no `text_delta` `second` and no `end` in `events`; snapshot `final` cancelled with `cancelRequested: true`; `dispatches` empty; `modelRequests` = 1 |
| round-exhaustion | `modelRequests` = 2; `dispatches` = `["call-r1","call-r2"]`; one `end`, last, completed with `exhausted: true`; snapshot `final` completed with `exhausted: true` |

Every observed `accepted`, `events` and snapshot value must also validate against `RunAccepted`, `RunEvent` and `RunSnapshot`.

## Normalized trace and comparison

From each `Observation`, `contracts-check` keeps:

- `events` without `usage` and `context` (accounting differs by design, docs/22 §4.1). Each maximal run of consecutive `text_delta`/`reasoning_delta` events becomes one `{ kind: "output", text, reasoning }` entry with the joined text and the joined reasoning: their relative interleaving and chunk boundaries are not compared (observationRules), their position relative to every other event is. MCP boundary tokens become `<B>`;
- `accepted`, snapshot `final`, `replies`, `modelRequests`, `dispatches` and `marks` as is, all ordered.

It compares the two backends' traces per scenario and reports the first differing path, e.g. `$.events[3].text`. A scenario passes when both backends satisfy its assertions and their traces are equal.

A difference that is real and not yet fixed is listed in `scenario-fixtures.json` → `knownDrift` as `{ scenario, backend, check }` (an assertion that fails on one backend) or `{ scenario, path }` (the first differing trace path), each with a `note`. The gate prints every listed drift on each run, fails on any failure that is not listed, and fails when a listed drift no longer occurs, until its entry is removed. Drift is never skipped silently. The table below mirrors that list.

## Known drift

| Scenario | Backend / path | Difference | Tracked by |
| --- | --- | --- | --- |
