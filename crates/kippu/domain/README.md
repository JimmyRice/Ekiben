# kippu-domain

Kippu's pure domain model: identifiers, money, roles, the catalog (events, sales, ticket
types), purchase requests and the reservation state machine.

This crate performs no I/O and never reads the clock: every function that depends on the
current time takes it as an argument. The whole domain is deterministic and testable without
a database.

```text
Reserved ──checkout──▶ PaymentPending ──payment attested──▶ Issued
   │  └──── cancel ───▶ Cancelled  │
   └─────── TTL ───────────────────┴──▶ Expired ──late payment──┬─ stock left ─▶ Issued
                                                                └─ sold out ──▶ RefundRequired ─▶ Refunded
```
