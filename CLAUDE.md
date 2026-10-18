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
cargo test --workspace --all-features                                   # 430 tests incl. e2e over TCP
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
cargo build --profile dist                                              # server distribution build, release speed (~17.6 MB vs ~22 MB)
cargo build --profile dist-min                                          # smallest server build, 10-15% slower (~10.3 MB)
cargo build --profile dist --no-default-features --features sqlite,mimalloc   # only what you use (~11.6 MB; dist-min ~6.9 MB)
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

Run all of the checks before every commit. `tests/statements.rs` (kippu-core) bounds how many
database statements the busiest paths run on each backend; lower its bounds when a change
saves statements, never raise them to make a change pass. Tests for HTTP behaviour live in
`crates/kippu/core/tests/` (in-process router + SQLite, `tests/support/mod.rs` has helpers such
as `shop`, `attestor`, `reserve`, `drain`); the full journey over real TCP is
`crates/kippu/server/tests/e2e_purchase.rs`.

## Architecture rules

- **Stateless.** No in-process state shared between requests. Everything mutable lives in the
  store; tokens are signed and verified without lookups. Background tasks must be safe on any
  number of instances (leases, compare-and-set, conditional updates, locks held for one
  transaction).
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
  fn and shapes the response (status, headers); it never calls
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
  idempotent by design are declared in `Module::idempotent_routes`; the `Idempotency-Key`
  middleware lets them through untouched.
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

- **Kippu stays small.** The API serves what clients need to operate: records, listings,
  counters the domain keeps anyway (stock). Analytics, reports and dashboards (revenue over
  time, conversion, audit search) belong to data tools reading the database directly; do not
  add aggregation endpoints for them.
- **Ticket format:** compact binary **KP1** — 12-byte header + canonical TLV claims + Ed25519
  signature over `header‖claims` (`spec/ticket-protocol.md`). One signing key for all events;
  KP1's `key_id` already supports a key ring if an organizer ever needs its own.
- **Organizers** are isolated per Organization; Admin works across organizations.
- **Organizations are public once they have a public event** (published or cancelled):
  `GET /v1/organizations[/{id}]` shows `{id, slug, name}` only and `/{id}/events` the public
  events; its organizers and admins see it, and its drafts, from the start. Members are listed
  to admins only; organizations do not manage themselves (no owner role).
- **Organizers read their events' orders** (`orders.read`): reservations
  (`/v1/events/{id}/reservations`), tickets without the signed bytes (only the holder gets
  those) and refunds (`/v1/events/{id}/refunds`). Cancelling and checking out stay the buyer's.
  Buyers see a sale's `payment-methods` (accepted, unrevoked, environment-compatible attestors).
- **Deleting events:** only a draft without sales, with its images, favourites and its own
  denials (`DELETE /v1/events/{id}`, `event-not-deletable` otherwise). A published event is
  cancelled instead: buyers and gates may know it. Events and organizations are also found by
  slug (`/by-slug/{slug}`), under the same visibility rules as by id.
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
- **Profiles:** `PATCH /v1/me` changes `display_name` and `contacts`, free-form
  `{name: address}` (at most 10, names ≤ 40, addresses ≤ 200 characters), seen by the account
  itself, admins and the organizers of events it ordered for
  (`GET /v1/events/{id}/buyers/{account_id}`, never the email).
- **Closing accounts** (`DELETE /v1/me` with the password, `DELETE /v1/admin/accounts/{id}`)
  anonymizes, never deletes: the id stays with every order, ticket, refund and denial; email,
  password, name, contacts, sessions, memberships and favourites go. The email's SHA-256
  fingerprint (`email_hash`, unique, kept for every account) and the external identities stay,
  so neither can make another account (no way back: Kippu does not verify emails, so a
  "restore" would let anyone with the address take over the old tickets). Closed accounts
  change no more; `sessions::issue` refuses them (`account-closed`).
- **Event status and sales:** only a published event sells (front door and worker; a request
  queued before a cancellation is rejected). An event someone ordered for (a reservation not
  expired or cancelled) can be cancelled but not withdrawn to a draft. Cancelling is final
  (`illegal-transition` for any other status; other fields stay editable): buyers and gates
  were told, so a revived event is a new event. A payment for an event no longer on sale
  answers `refund_required`, also for a reservation still holding stock (the hold is
  released). A late payment for a lapsed reservation takes its tickets again only as a
  purchase could (event on sale, account open and not denied; the sale window is not
  checked, the buyer checked out within it), otherwise it answers `refund_required`.
- **Waiting rooms:** buyers may join before the sale opens and keep the order they joined in;
  nobody is admitted before `opens_at` (with no purchases yet the backlog is empty and would
  let the whole room through), then one batch per tick while the backlog is below
  `max_backlog`, of no more people than the tickets left beyond the requests queued: a
  sold-out sale admits nobody, and tickets that come back (an unpaid reservation expiring or
  cancelled, a refund) go to the next in line. Once the sale closes or its event is
  cancelled, joining and polling answer `sale-closed` and the room is left as it stands. Buyers
  polling for admission read `admitted_through` alone, never the counters. An account holds
  one position per sale (`waiting_room_entries`): joining
  again returns it with a new queue ticket, so repeated joins neither crowd others out nor
  improve anyone's place.
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
- **Sale audiences** (`Sale.audience`, on the sale, not the ticket type): `public` (default);
  `invited`, seen and bought from only by the accounts on the sale's invitation list (e.g.
  cosplayers whose application passed buy a cheaper ticket; `PUT`/`DELETE
  /v1/sales/{id}/invitations/{account_id}`, `events.write`); `private`, seen only by its
  organizers and bought by nobody (guests, exhibitors, NPCs). Everyone else is told a hidden
  sale does not exist (404). A list may be kept while the sale is public; leaving it stops
  further purchases (the worker rejects queued requests `not_invited`, read in the same
  `queued_requests` statement, only for invited sales) and never touches tickets bought. Private
  is chosen at creation and kept: no attestors, no waiting room, free ticket types. A PUT
  without `audience` keeps it, so a client unaware of it never widens who sees a sale.
- **Grants:** `POST /v1/sales/{id}/grants {account_id, items}` (`tickets.grant`, with an
  `Idempotency-Key`) gives a private sale's tickets to an account: one transaction records a
  purchase request already reserved (`PurchasesTx::insert_purchase_request`, id derived from
  account, sale and key) and its reservation, taking quota then stock; then the built-in free
  attestor settles it, so tickets, `tickets.issued`, refunds and denials work as for purchases.
  Per-account limits, stock, deny lists, the sale window and a published event apply.
  Organizers get account ids from the people themselves; Kippu looks nobody up. Every organizer
  has `tickets.grant`: manual payments and free ticket types already let any organizer issue
  tickets, so restricting grants alone would protect nothing.
- **Purchase workers hold a sale's queue for one transaction:** each transaction takes the
  sale's row in `purchase_queues` (`PurchasesTx::lock_purchase_queue`, `FOR UPDATE SKIP
  LOCKED`; SQLite needs none) and only then reads its oldest queued requests
  (`queued_requests`, `created_at` then id, with each account's denial and, for invited sales,
  invitation), so every sale is
  served in arrival order however many workers and instances run, and a worker that dies holds
  nothing once its transaction is gone (no leases to run out; PostgreSQL also ends a
  transaction holding a queue after 10 s without a statement). Workers skip a held queue and
  serve other sales.
- **Purchase worker commits in groups:** the oldest due requests of a sale, up to a batch, are
  served as one run in one transaction: the queue, then the run's quotas
  (`InventoryTx::lock_quotas`, account order), then its stock (`InventoryTx::lock_stock`, type
  order), each locked and read. Every request is then decided in memory in arrival order, as
  one-by-one processing would (an account's later requests see what its earlier ones took;
  one rejected for stock takes no quota), and the run is written in a few multi-row statements
  (`take_quotas`, one `try_hold`, `insert_reservations`, `set_purchase_statuses`), so a run
  costs about as many statements however long it is; a lone request skips the reads and is
  decided by its conditional writes. A run that fails is redone one request at a time, and a
  request that fails on its own is postponed (`lease_until`, by
  `workers.purchase_lease_seconds`) instead of holding up the queue. Any transaction that
  changes several requests' stock must lock it up front the same way
  (`stock_locked_up_front_never_deadlocks`, `quotas_locked_together_never_exceed_a_limit`).
- **Waiting-room positions** come from counter rows, not a sequence: an admission batch means
  "the next N people" and needs all but consecutive numbers. A room has 128 counters on
  PostgreSQL and MySQL (counter `c` hands out `c + 1`, `c + 129`, …; `waiting_room_counters`)
  and one on SQLite. A join takes the lowest position of a counter no other join holds (`FOR
  UPDATE SKIP LOCKED`; on PostgreSQL picked in a CTE, since a subquery in `WHERE` is re-run on
  re-check and often updates nothing) and enters the account in the same statement (MySQL: one
  transaction), so joins do not wait for each other and only positions handed out at the same
  moment come out of order. An advance goes no further than the positions handed out with none
  missing below them and never waits for a join; only when admission has caught up and the
  next position stays free (nobody joined since the last tick) does it seal the counters behind
  it, skipping their positions and stopping below any counter a join holds, so a join that
  starts after an advance is never admitted by it (`joins_after_an_advance_are_never_admitted`).
  A sale created with a waiting room gets its room and counters with it, any other on its first
  join. Numbers are lost only to seals and to one account's simultaneous joins racing each other
  on PostgreSQL and SQLite.

## Open tasks

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
  OpenTelemetry; per-module migrations with their own version tracking; roles within an
  organization (who may grant, refund, record payments, edit sales), carried per organization
  in access tokens.
- Request log: a "still processing" line for requests running longer than N seconds (not built).

## Known limitations

- One sale is served by one transaction at a time, so a single sale's purchase throughput does
  not grow with workers or instances (other sales are served in parallel): about 7,500–8,500
  requests/s on PostgreSQL and MySQL in local containers, with a run as long as the default
  `workers.purchase_batch_size` (50). Beyond that the options are longer runs (a larger batch
  size), or inventory buckets (Open tasks), which would relax strict arrival order.
- On MySQL, a purchase worker whose machine freezes or vanishes without closing its connection
  holds its transaction, and so its sale's queue, until the server drops the connection: set
  `wait_timeout` (seconds) low enough on the server. PostgreSQL ends such a transaction after
  10 s by itself. The same holds for a waiting-room join, whose transaction holds one counter:
  admission to that sale stops below the counter's position until the connection is dropped
  (other sales and joins carry on; a PostgreSQL join is one statement and holds nothing).
- A waiting room hands out about 7,300 first positions/s on PostgreSQL (where writing the WAL
  is the limit) and 4,200/s on MySQL in local containers, with 64 joins at once.
- Arrival order (who gets the last tickets) is the `created_at` the front-door instance gave a
  request by its own clock, not the database's: instances need NTP, and clock skew between
  them shifts the order by as much (milliseconds in practice). With an inbox, a request still
  on its way into the database can be overtaken by one accepted after it that is already
  there; the window is the inbox's persisting delay.
- Access tokens are not looked up: a closed account's tokens, and an organizer's membership
  in them, work until they expire (`auth.access_token_ttl_seconds`, 15 minutes by default).
- Withdrawing an event checks for orders, then updates it; a purchase the worker commits in
  between leaves a reservation on a draft (rare; the organizer publishes or cancels it).
- The generic `Idempotency-Key` middleware does not stop two simultaneous first requests with
  the same key from both running (purchases and payments are idempotent on their own). A strict
  fix is a "processing" placeholder row.
- A ticket type belongs to exactly one sale; venue capacity shared across sales is not modelled.
- Kippu has no check-in data, so it cannot refuse to refund a ticket already used; the buyer's
  refund period ending before the event starts is the protection. Organizer refunds and
  chargebacks after entry only block re-entry.
- `denied-tickets` reads every ticket of the event per page (an `EXISTS` per ticket); fine for
  conventions, not for stadiums polled every second.
