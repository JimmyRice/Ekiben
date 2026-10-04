# Ekiben (駅弁)

Ticketing infrastructure for conventions. Going to a convention is like passing through the
ticket gate (改札) into another world — so the pieces are named after a Japanese train journey:

| Component | Path | What it is |
|---|---|---|
| **Kippu** (切符, *ticket*) | [`kippu/`](kippu) | Stateless backend: events, ticket sales under heavy contention, payment attestation, issuance. |
| **Kaisatsu** (改札, *ticket gate*) | [`kaisatsu/`](kaisatsu) | `no_std` library that verifies a ticket was signed by Kippu — and nothing more. |
| Bindings | [`ffi/`](ffi) | Kaisatsu for C, C++, C#, Swift, Kotlin, the Web and firmware. |
| Protocol | [`spec/`](spec) | Language-neutral wire format and shared test vectors. |

When a gate rejects you it goes *pinpōn* 🔔 and shuts its flaps, so Kaisatsu's error type is
called `Pinpon`.

## Workspace layout

```text
src/main.rs              the `kippu` binary — about ten lines of wiring
kippu/domain             kippu-domain        pure domain model, no I/O
kippu/store              kippu-store         storage ports + consistency contract
kippu/core               kippu-core          modules, auth, HTTP API, workers
kippu/server             kippu-server        CLI, configuration sources, adapter wiring
kippu/adapters/sqlite    kippu-store-sqlite  SQLite adapter
kaisatsu                 kaisatsu            ticket protocol + verification (no_std)
ffi/c                    kaisatsu-ffi        C ABI
xtask                    xtask               `cargo xtask …` automation
```

## Common commands

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -- --help
```
