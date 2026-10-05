# Finite mock dimmable-light relay

Host demonstration of a forwarding-only
`router_app::RelayRouterApp + NoChildren`.

The example seeds persisted router security state, initializes the shared
frontend, applies On/Off and Level Control behavior through the profile, and
runs two finite `step()` calls.

It then simulates the product button. `request_commissioning()` returns
`false` while joined, so a joined light or plug keeps its normal button action
(for example toggling the load). After a local Leave the router stays
factory-new and makes no network search on its own; the simulated press is
accepted and the next `step()` makes exactly one Network Steering attempt.
The demo asserts this sequence and also runs as the crate's unit test.

```rust,ignore
let mut app = RelayRouterApp::new(
    node,
    NoChildren,
    &POLICY,
    RouterParts::new(NoStatus, NoSupervisor, NoDiagnostics),
)?;

app.initialize().await?;
let events = app.step().await?;
```

It does not model a child-admitting parent. Use it to understand how a plug or
light composition synchronizes fitted hardware after `StepEvents`.

## Run

```bash
cd examples/mock-light
cargo +nightly-2026-03-23 run --locked
```
