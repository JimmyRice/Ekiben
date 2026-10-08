# CLAUDE.md

Ekiben (駅弁) is ticketing infrastructure for conventions: **Kippu** (切符), a stateless Rust
backend, and **Kaisatsu** (改札), a `no_std` library that verifies Kippu tickets. The initial
feature set is complete; settled design decisions, open tasks and known limits are at the end of
this file.

Code, rustdoc, READMEs and `spec/` are written in English.

## Layout

```text
src/main.rs                    the `kippu` binary: ~10 lines of wiring, nothing else
spec/                          normative protocols (KP1 tickets, attestors, webhooks) + shared test vectors
docs/                          walkthrough.md/.http, external-login.md (writing a sign-in module),
                               building-kaisatsu.md (every platform's smallest library)
crates/kippu/domain            pure domain: ids, money, timestamps, state machines — no I/O, no clock
crates/kippu/store             storage ports, consistency contract, `conformance` test suite
crates/kippu/telemetry         span contract + log layer (feature `layer`): one call chain per request
crates/kippu/core              Module system, auth/RBAC, HTTP, background tasks, built-in modules
crates/kippu/server            launcher: CLI (`kippu …`), config sources, adapter wiring
crates/kippu/adapters/sqlite   SQLite adapter (single writer)
crates/kippu/adapters/postgres PostgreSQL adapter (concurrent writers, SKIP LOCKED, advisory-locked outbox)
crates/kippu/adapters/mysql    MySQL 8 adapter (READ COMMITTED, no RETURNING/ON CONFLICT, row-locked outbox)
crates/kippu/adapters/nats     NATS JetStream: purchase inbox + event bus (messaging ports in kippu-store)
crates/kippu/adapters/objects-* object storage for event images: `-common` (object_store bridge),
                               `-s3`, `-gcs`, `-azure`, one crate per provider (port in kippu-store)
crates/kaisatsu                KP1 encode/verify, no_std; `issuer` feature for signing; fuzz/
crates/ffi/c                   C ABI (the only crate with `unsafe`), generated include/kaisatsu.h
crates/ffi/uniffi              UniFFI (proc-macro, no UDL) for Swift, Kotlin and C#; safe Rust only; kotlin/ is a Gradle project
crates/xtask                   `cargo xtask vectors|header [--check] | c-example | uniffi | size`
```

New crates go into the matching partition under `crates/`, are listed explicitly in the root
`[workspace] members`, inherit `[workspace.package]` fields and use `lints.workspace = true`.
Keep the repository root uncluttered and do not add speculative `.gitignore` entries.

## Commands

```bash
cargo test --workspace --all-features                                   # 290 tests incl. e2e over TCP
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets --all-features
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets           # default features too
cargo fmt --all
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace --exclude kaisatsu-ffi
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features -p kaisatsu-ffi  # same lib name as kaisatsu
cargo xtask vectors --check && cargo xtask header --check               # generated files up to date
cargo xtask c-example                                                   # C ABI against all 15 vectors
cargo xtask uniffi [swift-package|kotlin|nuget]                         # bindings into target/uniffi; no argument: run the vectors in Swift
cargo build -p kaisatsu --target thumbv7em-none-eabihf                  # proves no_std
cargo run -- serve --config <file>                                      # see kippu.example.toml
cargo build --profile dist                                              # server distribution build (~13 MB vs ~21 MB)
cargo build --profile dist --no-default-features --features sqlite,mimalloc   # only what you use (~8.6 MB)
```

Adapters that need a server run their conformance suite only when a URL is set, and the HTTP
tests can run on PostgreSQL too; locally:

```bash
docker run -d --rm --name kippu-test-postgres -e POSTGRES_USER=kippu -e POSTGRES_PASSWORD=kippu -p 55432:5432 postgres:17-alpine
docker run -d --rm --name kippu-test-mysql -e MYSQL_ROOT_PASSWORD=kippu -p 53306:3306 mysql:8.4
docker run -d --rm --name kippu-test-nats -p 54222:4222 nats:2.11-alpine -js
export EKIBEN_TEST_POSTGRES_URL=postgres://kippu:kippu@localhost:55432/kippu
export EKIBEN_TEST_MYSQL_URL=mysql://root:kippu@127.0.0.1:53306/mysql
export EKIBEN_TEST_NATS_URL=nats://127.0.0.1:54222
cargo test -p kippu-store-postgres -p kippu-store-mysql -p kippu-nats   # against real servers
EKIBEN_TEST_BACKEND=postgres cargo test -p kippu-core                    # HTTP tests on PostgreSQL
EKIBEN_TEST_BACKEND=mysql cargo test -p kippu-core                       # … and on MySQL
```

Run all of the checks before every commit. Tests for HTTP behaviour live in
`crates/kippu/core/tests/` (in-process router + SQLite, `tests/support/mod.rs` has helpers such
as `shop`, `attestor`, `reserve`, `drain`); the full journey over real TCP is
`crates/kippu/server/tests/e2e_purchase.rs`.

## Architecture rules

- **Stateless.** No in-process state shared between requests. Everything mutable lives in the
  store; tokens are signed and verified without lookups. Background tasks must be safe on any
  number of instances (leases, compare-and-set, conditional updates).
- **Business logic lives once, in `kippu-core` services.** Adapters only implement the store
  primitives. A new guarantee the logic relies on goes into the store's rustdoc contract *and*
  a new case in `crates/kippu/store/src/conformance.rs`; every adapter runs the suite via
  `kippu_store::conformance_tests!`.
- **Module layout** (copy an existing module): `mod.rs` (Module impl, permission constants,
  grants, tasks), `service.rs` or `service/` (public use cases), `routes.rs` (thin handlers with
  `#[utoipa::path]`), `dto.rs` (request/response types with `ToSchema` + `From` conversions to
  and from the service's types). Paths are absolute (`/v1/...`).
- **Services own the logic, routes own HTTP.** A service fn takes `(&AppState, &Principal |
  Option<&Principal>, ids, its own input struct)` and does authorization, validation, store
  calls, audit and outbox events; it never imports `axum` or `dto` (build errors with
  `crate::error::StatusCode`). A handler extracts, converts with `dto`, calls **one** service
  fn and shapes the response (status, headers, `IdempotentByDesign`); it never calls
  `state.store()`, `authorize` or `audit`. Only failures of HTTP itself (an unreadable body)
  are raised in routes. PUT and PATCH map to the same `update_*(…, version, Changes)` service
  fn.
- **Authorization:** `state.authorize(&principal, PERMISSION, Scope::…)`. Grants inherit
  upward; organizers are confined to their organizations, users to their own records. Hide
  other people's records as 404, not 403.
- **Errors:** `ApiError::new(status, ProblemKind::SALE_CLOSED, detail)` →
  `urn:kippu:problem:<kind>`. Kinds are API: a new one is a `ProblemKind` constant added to
  `ProblemKind::ALL` and to the `kinds_are_never_renamed` list; never rename one. Take bodies with `crate::http::Json`, never `axum::Json`, so
  unreadable bodies are problems too (`invalid-json`, `unsupported-media-type`,
  `payload-too-large`). Errors the framework answers (unknown route, wrong method, bad path
  parameter, panic, timeout) are rewritten into problems by `http/problems.rs`; clients see one
  error format.
- **Idempotency:** anything that may be retried must be safe to repeat (derived ids, unique
  keys returning `Insertion::Existing`, status checks before transitions). Handlers that are
  idempotent by design insert `IdempotentByDesign` into the response extensions.
- **Money / time:** `Money` is integer minor units; `Timestamp` is UTC microseconds. Domain
  functions take `now` as an argument; only `Clock` reads time.
- **Updates** use optimistic concurrency: the client sends the `version` it read; stale → 412.
- **Listings** answer `http::Listing` (`{items, next_cursor}`), never a bare array, even when
  they fit one page (`Listing::all`). Paged ones are keyset-paged: the store reads
  `PageRequest<P>` in index order resuming after a position (an id, a `Keyset` of time + id,
  or a sequence number), the service reads `page.plus_one()` and returns
  `Page::from_lookahead`, the route turns positions into opaque cursors named by a
  `ListingTag`. Every order is total (ties by id), every filter and order has an index in all
  three adapters, and there is no `total` count. Query structs carry
  `#[into_params(parameter_in = Query)]` and `#[serde(deny_unknown_fields)]`.
- **Kaisatsu:** verify path is `no_std`, allocation-free and panic-free (crate-level `deny` on
  indexing, arithmetic side effects, unwrap…). Header first, then signature, then claims.
  The wire format is defined only in `kaisatsu` (`issuer` feature) — never duplicate it.
  Changing the format means regenerating vectors (`cargo xtask vectors`) and updating
  `spec/ticket-protocol.md`.
- **Logging goes through `kippu-telemetry`.** Libraries (core, adapters) depend on the crate
  without its `layer` feature: span names, `request_span`, `Origin` and `call`. Only the launcher
  enables `layer` and installs `kippu_telemetry::layer::init`. Never format log output elsewhere.
  Every request and task run is one unit; what runs inside it becomes its call chain:
  service functions carry `#[tracing::instrument(skip_all)]`, a call to anything outside the
  process (NATS, object storage, a webhook receiver — any new adapter) is wrapped in
  `kippu_telemetry::call("system", "operation", future)`, and database statements are counted
  from sqlx's own events (nothing to add). Errors capture where they were raised through
  `#[track_caller]` constructors and `Origin::capture()`; a new error type that reaches a
  response does the same. Log business milestones with `tracing::info!`. Never record headers,
  bodies or tokens.
- **Environment variables named `KIPPU_*` are configuration** and unknown keys are rejected:
  test settings use `EKIBEN_TEST_*` so a test run cannot break the config it loads.
- **Messaging is optional:** `PurchaseInbox`/`EventBus` (kippu-store) are injected with
  `Kippu::inbox`/`Kippu::event_bus`; without them the database is the queue. With an inbox,
  the database may not have a submitted request yet — poll with the receipt in `Location`.
- **Object storage** (event images) is an adapter like the databases: the `ObjectStorage` port
  lives in `kippu-store` (three methods; its contract is checked by
  `kippu_store::object_conformance_tests!`), `kippu-core` never sees a provider, and the launcher
  (`kippu-server/src/adapters.rs`, `connect_objects`) picks the adapter by the scheme of
  `images.url`. One crate per provider under `adapters/objects-*`, on `object_store` with `ring`
  (no `fs`, no aws-lc) through `kippu-objects-common`; each turns on its own `*-base` feature.
  Tests inject an in-memory `object_store` wrapped in `ObjectStoreAdapter` through
  `Kippu::object_storage` (`TestApp::start_custom`). Never add a local-disk backend.
- **Body limits:** `server.max_body_bytes` everywhere, except routes a module raises with
  `Module::body_limits`; code that buffers bodies reads the `RequestBodyLimit` extension.
- **Outbound HTTP** (webhooks) goes through `modules/webhooks/delivery.rs`: rustls + ring,
  no redirects, no environment proxy, and a resolver that drops non-public addresses (SSRF).
  Reuse it for any new outbound call; never call user-supplied URLs with a plain client.
- **Ticket transport:** the API returns tickets as Base64 (`ticket`) and raw bytes
  (`/v1/tickets/{id}/raw`); how they reach a gate is the integrator's choice. Base45 is an
  optional `base45` feature (kaisatsu, kaisatsu-ffi with `KAISATSU_BASE45`, kippu-server);
  default builds must not contain it (`cargo xtask c-example` checks the C library).
- **UniFFI:** one `Verifier` object, one `verify(ticket, now)` call per scan; keep the API this
  small. `uniffi` is pinned (`=0.31.0`) to the version `uniffi-bindgen-cs` is built on, and the
  generated sources must come from the library they are shipped with. C and C++ stay on the C ABI.
- **FFI:** every `unsafe` block has a `// SAFETY:` comment; regenerate the header with
  `cargo xtask header`; ship builds with `--no-default-features --profile release-small`.

## Conventions

- Lints: clippy pedantic + `unwrap_used`, `expect_used`, `print_stdout`, `dbg_macro`, `todo` as
  warnings. Tests may unwrap inside `#[test]` fns; integration-test helper files need a
  file-level `#![allow(clippy::unwrap_used, …, reason = "…")]`. Prefer `#[expect(…, reason)]`
  over `#[allow]` in production code.
- **Thin `lib.rs` / `mod.rs`.** They hold only module docs (what this layer does, which file
  holds what), crate attributes, `mod` declarations and `pub use` / `pub(crate) use`
  re-exports. Everything else — functions, types, constants, trait impls, tests — lives in a
  file named after its content (`store.rs`, `errors.rs`, `buying.rs`, …). The one exception is
  a feature module's `mod.rs`: its `Module` impl, the unit struct and the `permissions`
  constants (pure wiring; setup code such as building a task goes in the service).
  `tests/support/mod.rs` is exempt. Directory modules keep the `foo/mod.rs` form.
- Every public item has rustdoc; each crate's README is its crate docs (doctests run).
- Confirm design questions that change APIs or storage with the user before implementing.

## Gotchas learned the hard way

- **sqlx 0.9:** query strings must be `SqlSafeStr` — use `&'static str`, or
  `sqlx::AssertSqlSafe(format!(…))` for composed SQL built from constants. `sqlx::migrate!`
  needs the `macros` feature. SQLite writes go through a single-connection writer pool opened
  with `BEGIN IMMEDIATE`; reads use a separate WAL reader pool.
- **Outbox order on concurrent databases:** identity/serial values are assigned at insert,
  not commit; PostgreSQL appends take `pg_advisory_xact_lock` so a reader never skips a late
  commit (`outbox_readers_never_skip_late_commits`). Any new adapter needs the same.
- **MySQL:** `?` placeholders are positional (bind repeated values twice); no `RETURNING` or
  `ON CONFLICT` — catch duplicate keys with `is_duplicate`; `FOUND_ROWS` is on, so UPDATE
  reports matched rows; force the index when `FOR UPDATE SKIP LOCKED` meets `LIMIT`; bind
  quantities as `i64` (unsigned arithmetic cannot go negative). sqlx's `rsa` feature stays
  off (advisory), so MySQL 8 auth needs TLS — both server adapters use rustls.
- **sqlx PostgreSQL** has no `u32`/`u16` encoding: store counts as `BIGINT` and convert
  (`convert::count`/`uncount`).
- **SQLite cannot relax a column in place:** rebuild the table in a migration that starts
  with `-- no-transaction` (foreign keys off, one `BEGIN IMMEDIATE` … `COMMIT`), as
  `0002_identities.sql` does.
- **ed25519-dalek 3:** no `std` feature (use `fast`, `zeroize`, `pkcs8`, `pem`, `rand_core`);
  `verify_strict` needs one contiguous message. **sha2 0.11**, **argon2 0.6**
  (`PasswordHasher::hash_password(bytes)`), **getrandom 0.4** (`getrandom::fill`).
- **jsonwebtoken is deliberately not used** (its RSA dependency carries an unfixed advisory);
  extend `crates/kippu/core/src/auth/jwt.rs` instead.
- **utoipa 6 / utoipa-axum 0.3:** register handlers with `routes!(…)`; domain types get
  `ToSchema` only through `kippu-domain`'s `openapi` feature. **axum 0.8:** path params are
  `/{id}`; `Option<Principal>` works via `OptionalFromRequestParts`.
- **rustfmt reflows code:** scripted string replacements must match the formatted text.
- **zsh:** never name a shell variable `path` (it is tied to `PATH`).
- Debug builds compile the crypto crates with `opt-level = 3` (root `Cargo.toml`); without it
  Argon2 makes the test suite ~20× slower.
- The C example and `xtask size` build the FFI crate without `std`; with `std` the shared
  library is ~4× larger.

## Settled design decisions

Do not change these unless the user explicitly asks.

- **Ticket format:** compact binary **KP1** — 12-byte header + canonical TLV claims + Ed25519
  signature over `header‖claims` (`spec/ticket-protocol.md`). One signing key for all events;
  KP1's `key_id` already supports a key ring if an organizer ever needs its own.
- **Organizers** are isolated per Organization; Admin works across organizations.
- **Root login:** no separate CLI. Only the root public key is in the config; the private key
  signs an EdDSA JWT of at most 10 minutes locally (`kippu root-token` or any JWT library).
- **Payments:** the backend integrates no payment provider. Attestors with registered Ed25519
  keys report signed results (`spec/attestor-protocol.md`).
- **Messaging is optional** (database is the queue); the first MQ adapter is NATS JetStream.
  Known edge: the same idempotency key retried with a *different* cart before it reaches the
  database is dropped by JetStream dedup; the first cart wins.
- **Webhooks:** deployment-wide Ed25519 key, public key at `/.well-known/kippu/webhook-keys`,
  same signature format as attestors; subscriptions per organization (`spec/webhook-protocol.md`).
- **Bindings:** Swift/Kotlin/C# via UniFFI, C/C++/firmware via the hand-written C ABI. C++ stays
  on the C ABI: the C++ generator is stuck on UniFFI 0.29 and UniFFI would grow the library from
  65 KiB to 365 KiB (RustBuffer, std) for no gain.
- **External interface is HTTP + JSON only** (OpenAPI). No pluggable protocol/codec layer: CDNs,
  WAFs and edge rate limiting live in the HTTP ecosystem, and semantics lean on HTTP (`202 +
  Location`, problem+json, attestor signatures over METHOD + path + body hash). If gRPC is ever
  needed: first make the business layer HTTP-independent (error class separate from status,
  a `RequestContext`, public `service`), then add `crates/kippu/transports/grpc`. Prefer SSE for
  push.
- **External sign-in:** Kippu ships no OAuth client; forks write a `Module` that calls
  `auth::external::link_or_create` and `auth::sessions::issue` (`docs/external-login.md`).
  Accounts may have no password and no email; merging by email requires a verified email and is
  off by default (`auth.link_by_verified_email`). External bearer tokens are deliberately *not*
  accepted directly: Kippu could not revoke sessions or record roles.
- **Event location:** `venue` is the human name of the place (required); `address` is optional
  and structured (ISO country code, region, locality, postal code, street, WGS 84
  latitude/longitude given together), stored as columns so listings can filter by country.
  PATCH replaces it as a whole; `null` removes it.
- **Listings:** `{items, next_cursor}` envelope for lists only; single resources stay bare and
  errors stay problem+json. Cursors are opaque keyset positions, not offsets; filters and
  sorts are a whitelisted, indexed set per listing (no generic filter language). The outbox
  feeds keep `after=<sequence>` and a bare array: that is the attestor protocol. The attestor
  feed reads past other attestors' events and returns how far it read in `Kippu-Feed-Position`.
- **Event `content`** is an opaque string up to 256 KiB, returned only by `GET /v1/events/{id}`;
  listings return `EventSummary`. Per-person sensitive data (e.g. real-name IDs) is the
  integrator's job to encrypt, not the backend's.
- **Event images** need real object storage; no local-disk or in-memory backend outside tests.
- **Refunds of issued tickets** are asked of Kippu (buyer within the ticket type's
  `refundable_until`, which ends no later than `valid_from` and the event's start; organizers
  any time), never of an attestor. One transaction revokes the tickets (irreversible), returns
  stock and quota, and emits `tickets.revoked` plus `refund.required` with a `refund_id`; the
  attestor confirms with `outcome: refunded` and that id. Free tickets complete at once, cash
  is confirmed by an organizer. Chargebacks and other whole-payment returns are reported as
  `outcome: reversed`, which also completes the payment's pending refunds; partial refunds
  always go through Kippu. Reservations stay `issued`; per-ticket state lives in
  `tickets.status` and `refunds`.
- **Denials** (not "revocations"): organizers keep deny lists per event and per organization
  (CRUD), naming a ticket or an account. Gates never learn a ticket's holder (KP1 carries no
  account), so `GET /v1/events/{id}/denied-tickets` joins denied tickets, tickets of denied
  accounts and revoked tickets into ticket ids. It is a plain keyset-paged snapshot read with an
  organizer's credentials; venues poll it. Denied accounts' purchase requests are rejected
  (`account_denied`) by the worker, not the front door; a reservation held before the denial
  can still be paid (its tickets are refused at the gate). Webhooks are not the channel for gates
  (they need an inbound public endpoint and give no snapshot).
- **PostgreSQL queue numbers** come from a per-sale counter row (`INSERT … ON CONFLICT DO
  UPDATE`), not a sequence: an admission batch means "the next N people" and needs consecutive
  numbers.

## Open tasks

- Purchase worker: process in parallel when `StoreCapabilities::concurrent_writers`.
- CI: run the three UniFFI packaging flows (swift-package, kotlin, nuget) and add size budgets;
  run the Android AAR on a device or emulator (it has only been built).
- `kaisatsu-wasm` published as an npm package.
- Cortex-M (embassy) firmware example to measure real size; add the budget to CI. Static library
  size is meaningless (the linker prunes).
- Event search by text (needs a full-text index per database) and by region or city.
- Gates, when a snapshot per poll is no longer enough: a per-event numbered log of denial
  changes (deltas, `after=<number>`), a narrow machine credential for venue servers instead of
  organizer tokens, signed lists gates verify themselves. Not needed yet.
- Webhooks: resuming (`active: true`) does not reset the backoff; no retry-now; the backoff cap
  is hard-coded.
- When a deployment's auxiliary backend should vet refunds (e.g. against check-ins gates report
  to it): an attestor-signed `POST /v1/attestor/refunds {attestation_id, ticket_ids, reference}`
  that revokes at once (refund id derived from the reference; no `refund.required`). Requesting,
  cooling off and cancelling stay in that backend; Kippu keeps one irreversible step. Until
  then, leave `refundable_until` unset to keep buyers from refunding at Kippu directly.
- Later: passkeys as a second Admin credential; inventory buckets for extremely hot sessions;
  OpenTelemetry; per-module migrations with their own version tracking.
- Request log: a "still processing" line for requests running longer than N seconds (not built).

## Known limitations

- The generic `Idempotency-Key` middleware does not stop two simultaneous first requests with
  the same key from both running (purchases and payments are idempotent on their own). A strict
  fix is a "processing" placeholder row.
- A ticket type belongs to exactly one sale; venue capacity shared across sales is not modelled.
- Kippu has no check-in data, so it cannot refuse to refund a ticket already used; the buyer's
  refund period ending before the event starts is the protection. Organizer refunds and
  chargebacks after entry only block re-entry.
- `denied-tickets` reads every ticket of the event per page (an `EXISTS` per ticket); fine for
  conventions, not for stadiums polled every second.
