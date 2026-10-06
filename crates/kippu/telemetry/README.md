# kippu-telemetry

How Kippu explains what happened. Every request and every background task run is **one unit**
of log output, and everything that ran on its behalf hangs off it as a **call chain**:

```text
┌─ 10:34:55.433  POST /v1/sales/…/purchase-requests              req 5cdc82aa
│  caller            account:0199 (user)
│  db                7 queries, 1.1 ms · 6× SELECT stock FROM ticket_types
│  10:34:55.433      ▸ purchasing::buying::submit_purchase       (7 queries, 1.1 ms)  2.4 ms
│  10:34:55.433        ◆ nats enqueue                                          0.9 ms
│  10:34:55.434 WARN   sale is not open  @ crates/…/sales.rs:828
│  problem           sale-closed: this sale is not open
│  origin            submit_purchase @ crates/…/buying.rs:58
└─ 409 Conflict                                                         2.5 ms
```

The crate has two halves, so each part of Kippu depends only on what it uses:

- **The contract** (always built): span names and fields, [`request_span`], [`task_span`],
  [`Origin`] (where an error was raised) and [`call`] (a call to something outside the
  process). A library — core, a storage adapter, a queue — depends on this and nothing else;
  it only needs `tracing`.
- **The layer** (feature `layer`): [`layer::LogLayer`] collects the events and spans of each
  unit and renders them as `pretty` blocks, `compact` lines or `json`; [`layer::init`]
  installs it. Only a launcher needs this.

## What goes into a call chain

| Source | Appears as | How |
|---|---|---|
| A service function | `▸ module::file::function`, nested by call, with its duration | `#[tracing::instrument(skip_all)]` on it |
| A call to something outside the process (NATS, object storage, a webhook receiver) | `◆ system operation`; `✗` and the error when it fails | wrap the future in [`call`] |
| A database statement | not a line of its own: counted per step and per unit, and a statement repeated five or more times is named (the N+1 hint) | nothing: `sqlx` already emits them; run with `RUST_LOG=sqlx::query=debug` (the default when `RUST_LOG` is unset) |
| An error response | `problem`, and `origin` (step, file and line); the steps it passed through are marked `✗` | build it with `#[track_caller]` constructors and [`Origin::capture`] |
| A warning or an error | with `@ file:line` of the logging call | `tracing::warn!`, `tracing::error!` |

Headers, bodies and tokens are never recorded.
