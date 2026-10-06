# kippu-store-sqlite

SQLite adapter for [`kippu-store`](../../store). Suited to development, tests and single-node
deployments that do not expect thousands of purchases per second.

- **One writer.** SQLite allows a single writer, so all writes share one connection and are
  serialized in the application rather than colliding on `SQLITE_BUSY`. Readers use a separate
  pool and, in WAL mode, never block the writer.
- **Schema** in `migrations/`: STRICT tables, UUIDs as 16-byte blobs, instants as
  integer microseconds, `CHECK (held + sold <= capacity)` on inventory as a last line of defence.
- **Conformance:** `cargo test -p kippu-store-sqlite` runs the shared `kippu-store` suite.

```text
database.url = "sqlite:///var/lib/kippu/kippu.db"
```
