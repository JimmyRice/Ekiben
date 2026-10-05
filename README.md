# Ekiben (駅弁)

Ticketing infrastructure for conventions. Going to a convention is like passing through the
ticket gate (改札) into another world — so the pieces are named after a Japanese train journey:

| Component | Path | What it is |
|---|---|---|
| **Kippu** (切符, *ticket*) | [`crates/kippu/`](crates/kippu) | Stateless backend: events, ticket sales under heavy contention, payment attestation, issuance. |
| **Kaisatsu** (改札, *ticket gate*) | [`crates/kaisatsu/`](crates/kaisatsu) | `no_std` library that verifies a ticket was signed by Kippu — and nothing more. |
| Bindings | [`crates/ffi/`](crates/ffi) | Kaisatsu for C, C++, C#, Swift, Kotlin, the Web and firmware. |
| Protocol | [`spec/`](spec) | Language-neutral wire format and shared test vectors. |

When a gate rejects you it goes *pinpōn* 🔔 and shuts its flaps, so Kaisatsu's error type is
called `Pinpon`.

## Workspace layout

```text
src/main.rs                     the `kippu` binary — about ten lines of wiring
spec/                           language-neutral protocols and test vectors
crates/
├── kippu/domain                kippu-domain        pure domain model, no I/O
├── kippu/store                 kippu-store         storage ports + consistency contract
├── kippu/core                  kippu-core          modules, auth, HTTP API, workers
├── kippu/server                kippu-server        CLI, configuration sources, adapter wiring
├── kippu/adapters/sqlite       kippu-store-sqlite  SQLite adapter
├── kaisatsu                    kaisatsu            ticket protocol + verification (no_std)
├── ffi/c                       kaisatsu-ffi        C ABI
└── xtask                       xtask               `cargo xtask …` automation
```

## Quickstart

```bash
cargo run -- keygen                      # run twice: one ticket key, one token key
cargo run -- keygen --pem > root.pem     # the root key pair (keep the private half offline)
```

Put the keys in the environment and start a server on a local SQLite database:

```bash
export KIPPU_KEYS__TICKET_SIGNING_KEY=<private key 1>
export KIPPU_KEYS__TOKEN_SIGNING_KEY=<private key 2>
export KIPPU_ROOT__KEYS='[{name="owner", public_key="<public half of root.pem>"}]'
cargo run -- serve --config kippu.example.toml --database-url "sqlite://$TMPDIR/kippu.db?mode=rwc"
```

Then sign in as root and explore the API (`/openapi.json` documents every endpoint):

```bash
TOKEN=$(cargo run -q -- root-token --config kippu.example.toml --key root.pem --name owner)
curl -H "authorization: Bearer $TOKEN" localhost:8080/v1/me
```

## How a purchase works

```text
buyer ─▶ waiting room ─▶ POST purchase-request (Idempotency-Key) ─▶ 202, poll Location
                                   │ durable, queued
                         worker ───┴─▶ reserve stock + quota in one transaction ─▶ Reserved
buyer ─▶ checkout(attestor) ─▶ payment.requested ─▶ your payment service charges the buyer
attestor ─▶ signed POST /v1/payment-attestations ─▶ one transaction: record payment, sell
                                                   stock, sign tickets, emit tickets.issued
gate ─▶ Kaisatsu verifies the QR offline with keys from /.well-known/kippu/ticket-keys
```

- No overselling: inventory changes are conditional updates guarded by a database `CHECK`.
- Everything that may be retried is idempotent: purchases (id derived from the key), payment
  attestations (unique per attestor), queue redelivery, expiry.
- Money is never taken without tickets: a payment that cannot be used ends in an explicit
  `refund_required` that the attestor acts on. See [`spec/attestor-protocol.md`](spec/attestor-protocol.md).

## Development

```bash
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo xtask c-example        # build the C ABI and run every test vector through it
cargo xtask size             # Kaisatsu library sizes
```
