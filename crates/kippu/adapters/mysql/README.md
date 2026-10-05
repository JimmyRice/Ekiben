# kippu-store-mysql

MySQL adapter for [`kippu-store`](../../store). MySQL 8.0.16 or newer, InnoDB.

- **Concurrent writers at READ COMMITTED.** Every connection switches from InnoDB's default
  REPEATABLE READ, whose gap locks turn concurrent inserts into deadlocks. Stock and quotas
  move by conditional `UPDATE`s, which InnoDB re-checks on the locked row, and `CHECK`
  constraints back them up. Quantities are bound as signed integers, so a subtraction cannot
  wrap around in unsigned arithmetic.
- **Claiming work** uses `FOR UPDATE SKIP LOCKED` on the queue index. The index is forced:
  with a filesort MySQL would lock every queued row before applying `LIMIT`, and concurrent
  workers would find nothing to claim.
- **Outbox order.** Appends lock the single row of `outbox_lock` until commit, so
  `AUTO_INCREMENT` sequence numbers follow commit order.
- **No `RETURNING`, no `ON CONFLICT DO NOTHING`.** Inserts that may collide run plainly and
  treat a duplicate-key error as "already there"; claims lock, update and re-read in one
  transaction; waiting-room positions come back through `LAST_INSERT_ID(expr)`.
- **Schema** in [`migrations/`](migrations): `BINARY(16)` UUIDs and `BIGINT` microseconds (MySQL's
  `TIMESTAMP` ends in 2038 and `DATETIME` has no zone), JSON as text.
- **Conformance:** `KIPPU_TEST_MYSQL_URL=mysql://… cargo test -p kippu-store-mysql` runs the
  shared suite, each case in a database of its own (the user needs `CREATE`); without the
  variable the cases are skipped.

```text
database.url = "mysql://kippu:secret@db.internal/kippu?max_connections=32"
```

Connections use TLS when the server offers it (`ssl-mode=preferred`, the default; set
`ssl-mode=verify-identity` in production). MySQL 8's default `caching_sha2_password`
authentication needs either TLS or an RSA key exchange; the latter is not compiled in, because
the `rsa` crate carries an unfixed advisory, so plain-text connections to such accounts fail.
