# Aether testing

## Test behavior, not wiring

Permanent tests should catch consumer-visible failures:

- feature combinations and missing-instance errors;
- typed error codes, input limits, budgets, and state transitions;
- streaming event order and exactly one terminal state;
- cancellation, approval allow/deny/timeout, and unknown side effects;
- Skill digest/revision conflicts and budget rejection;
- MCP trust, scope, tool visibility, and safe credential handling;
- optional-port UI behavior and accessible dialog cleanup.

Do not pin forwarding calls, copied values, source strings, incidental defaults, or bare "does not throw" behavior.

## Rust

Core crates use unit tests and local controlled upstreams. Use loopback HTTP/MCP fixtures, not external services. Test protocol boundaries, UTF-8 streaming, cancellation, error sanitization, database migration behavior, and security decisions. Keep Tauri command handlers thin; integration behavior belongs in a real example window when required.

## TypeScript and Svelte

Use Vitest, jsdom, Testing Library, and `createFakePorts()`. Instantiate state classes with fake ports. Test components through user-visible interaction and accessibility. Do not import the Tauri adapter in component tests unless the adapter itself is the subject.

## Repository gates

Run the smallest relevant checks first, then the repository gates required by the changed surface:

```text
pnpm run lint
pnpm run format:check
pnpm -r typecheck
pnpm -r test
pnpm run kit:dto-check
pnpm run kit:i18n
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

On Windows, use the repository's MSVC wrapper when the project requires its pinned compiler environment. For a changed feature, also run its standalone `cargo check` without default features. For Tauri `generate_context!` builds, build the example frontend before compiling the example so `frontendDist` exists.

## Smoke proof

Tests are not the only proof. For changed integration behavior, launch the relevant example or use a controlled local service and observe the consumer-visible result: the window opens, the command is authorized, streaming reaches the UI, cancellation closes the upstream, or a denied tool is not executed.
