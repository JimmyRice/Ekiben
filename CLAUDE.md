# CLAUDE.md

Ekiben (駅弁) is ticketing infrastructure for conventions: **Kippu** (切符), a stateless Rust
backend, and **Kaisatsu** (改札), a `no_std` library that verifies Kippu tickets.

Code, rustdoc, READMEs and `spec/` are written in English.

## Layout

```text
src/main.rs                    the `kippu` binary: ~10 lines of wiring, nothing else
spec/                          normative protocols (KP1 tickets, attestors, webhooks) + shared test vectors
docs/                          walkthrough.md/.http, external-login.md (writing a sign-in module)
crates/kippu/domain            pure domain: ids, money, timestamps, state machines — no I/O, no clock
crates/kippu/store             storage ports, consistency contract, `conformance` test suite
crates/kippu/core              Module system, auth/RBAC, HTTP, background tasks, built-in modules
crates/kippu/server            launcher: CLI (`kippu …`), config sources, adapter wiring
crates/kippu/adapters/sqlite   SQLite adapter (single writer)
crates/kippu/adapters/postgres PostgreSQL adapter (concurrent writers, SKIP LOCKED, advisory-locked outbox)
crates/kippu/adapters/mysql    MySQL 8 adapter (READ COMMITTED, no RETURNING/ON CONFLICT, row-locked outbox)
crates/kaisatsu                KP1 encode/verify, no_std; `issuer` feature for signing; fuzz/
crates/ffi/c                   C ABI (the only crate with `unsafe`), generated include/kaisatsu.h
crates/xtask                   `cargo xtask vectors|header [--check] | c-example | size`
```

New crates go into the matching partition under `crates/`, are listed explicitly in the root
`[workspace] members`, inherit `[workspace.package]` fields and use `lints.workspace = true`.
Keep the repository root uncluttered and do not add speculative `.gitignore` entries.

## Commands

```bash
cargo test --workspace --all-features                                   # 175 tests incl. e2e over TCP
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets --all-features
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets           # default features too
cargo fmt --all
cargo xtask vectors --check && cargo xtask header --check               # generated files up to date
cargo xtask c-example                                                   # C ABI against all 15 vectors
cargo build -p kaisatsu --target thumbv7em-none-eabihf                  # proves no_std
cargo run -- serve --config <file>                                      # see kippu.example.toml
```

Adapters that need a server run their conformance suite only when a URL is set, and the HTTP
tests can run on PostgreSQL too; locally:

```bash
docker run -d --rm --name kippu-test-postgres -e POSTGRES_USER=kippu -e POSTGRES_PASSWORD=kippu -p 55432:5432 postgres:17-alpine
docker run -d --rm --name kippu-test-mysql -e MYSQL_ROOT_PASSWORD=kippu -p 53306:3306 mysql:8.4
export KIPPU_TEST_POSTGRES_URL=postgres://kippu:kippu@localhost:55432/kippu
export KIPPU_TEST_MYSQL_URL=mysql://root:kippu@127.0.0.1:53306/mysql
cargo test -p kippu-store-postgres -p kippu-store-mysql                 # conformance on both
KIPPU_TEST_BACKEND=postgres cargo test -p kippu-core                    # HTTP tests on PostgreSQL
KIPPU_TEST_BACKEND=mysql cargo test -p kippu-core                       # … and on MySQL
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
  grants, tasks), `routes.rs` (thin handlers with `#[utoipa::path]`), `service.rs` (use cases),
  `dto.rs` (request/response types with `ToSchema`). Paths are absolute (`/v1/...`).
- **Authorization:** `state.authorize(&principal, PERMISSION, Scope::…)`. Grants inherit
  upward; organizers are confined to their organizations, users to their own records. Hide
  other people's records as 404, not 403.
- **Errors:** `ApiError::new(status, "kebab-kind", detail)` → `urn:kippu:problem:<kind>`. Kinds
  are API: never rename them. Take bodies with `crate::http::Json`, never `axum::Json`, so
  unreadable bodies are problems too (`invalid-json`, `unsupported-media-type`,
  `payload-too-large`).
- **Idempotency:** anything that may be retried must be safe to repeat (derived ids, unique
  keys returning `Insertion::Existing`, status checks before transitions). Handlers that are
  idempotent by design insert `IdempotentByDesign` into the response extensions.
- **Money / time:** `Money` is integer minor units; `Timestamp` is UTC microseconds. Domain
  functions take `now` as an argument; only `Clock` reads time.
- **Updates** use optimistic concurrency: the client sends the `version` it read; stale → 412.
- **Kaisatsu:** verify path is `no_std`, allocation-free and panic-free (crate-level `deny` on
  indexing, arithmetic side effects, unwrap…). Header first, then signature, then claims.
  The wire format is defined only in `kaisatsu` (`issuer` feature) — never duplicate it.
  Changing the format means regenerating vectors (`cargo xtask vectors`) and updating
  `spec/ticket-protocol.md`.
- **Logging:** requests run in a `request` span and task runs in a `task` span, defined with
  their fields in `crates/kippu/core/src/http/trace.rs`; `crates/kippu/server/src/telemetry.rs`
  renders them (pretty blocks, compact lines, JSON). Log business milestones with
  `tracing::info!` and they land in the right block. Never record headers, bodies or tokens.
- **Outbound HTTP** (webhooks) goes through `modules/webhooks/delivery.rs`: rustls + ring,
  no redirects, no environment proxy, and a resolver that drops non-public addresses (SSRF).
  Reuse it for any new outbound call; never call user-supplied URLs with a plain client.
- **Ticket transport:** the API returns tickets as Base64 (`ticket`) and raw bytes
  (`/v1/tickets/{id}/raw`); how they reach a gate is the integrator's choice. Base45 is an
  optional `base45` feature (kaisatsu, kaisatsu-ffi with `KAISATSU_BASE45`, kippu-server);
  default builds must not contain it (`cargo xtask c-example` checks the C library).
- **FFI:** every `unsafe` block has a `// SAFETY:` comment; regenerate the header with
  `cargo xtask header`; ship builds with `--no-default-features --profile release-small`.

## Conventions

- Lints: clippy pedantic + `unwrap_used`, `expect_used`, `print_stdout`, `dbg_macro`, `todo` as
  warnings. Tests may unwrap inside `#[test]` fns; integration-test helper files need a
  file-level `#![allow(clippy::unwrap_used, …, reason = "…")]`. Prefer `#[expect(…, reason)]`
  over `#[allow]` in production code.
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
