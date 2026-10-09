# Kippu Attestor Protocol, version 1

Status: draft. Kippu never talks to a payment provider. Instead, a deployment runs one or more
**attestors** — small services that wrap Apple Pay, Google Pay, Stripe, Alipay, PayPay, a cash desk, anything — and
Kippu trusts what they sign. This document specifies how an attestor and Kippu talk.

## 1. Registering an attestor

An admin registers the attestor and its Ed25519 public keys at runtime:

```http
POST /v1/admin/attestors
{ "name": "Stripe gateway", "environment": "live",
  "keys": [{ "key_id": "2026-10", "public_key": "<base64 or SPKI PEM>" }] }
```

- `environment` is `live` or `sandbox`. A sandbox attestor can never settle a live sale.
- Each sale lists the attestors it accepts (`accepted_attestors`). Organizers choose per sale.
- Revoking an attestor or a key (`PUT …/revoked`) takes effect on every instance immediately.
  Adding a key alongside the old one allows rotation without downtime.

Kippu stores only public keys: a leaked database cannot be used to forge payments.

## 2. Signing requests

Every attestor request carries:

```text
Kippu-Signature: attestor=<attestor id>, key=<key id>, ts=<unix seconds>, sig=<base64url, no padding>
```

`sig` is an Ed25519 signature over these five lines, joined by `\n`:

```text
kippu-attestor-v1
<HTTP method, upper case>
<path and query, exactly as sent>
<ts>
<lowercase hex SHA-256 of the request body (of the empty string for GET)>
```

Kippu rejects the request (401) if the attestor or key is unknown or revoked, the signature
does not verify, or `ts` is more than 300 seconds from Kippu's clock.

## 3. Learning what to charge

When a buyer checks out with an attestor, Kippu records a `payment.requested` event:

```json
{ "topic": "payment.requested", "reservation_id": "…", "attestor_id": "…",
  "amount": { "amount_minor": 6000, "currency": "JPY" }, "expires_at": "…" }
```

An attestor reads its events with `GET /v1/attestor/feed?after=<sequence>&limit=<n>` (signed),
or looks a reservation up directly with `GET /v1/attestor/reservations/{id}` (signed) — for
example when the buyer's browser hands it a reservation id. **The amount to charge always comes
from Kippu**, never from the buyer.

The feed answers a JSON array of `{sequence, created_at, event}`, in sequence order, holding
only the events addressed to the attestor (`payment.requested`, `refund.required`). The outbox
behind it holds every event of the deployment, so Kippu reads on past other attestors' events
and says how far it read in the `Kippu-Feed-Position` header. Pass that value as the next
`after`: it never skips an event addressed to you, and it moves on even when an answer is
empty. (The sequence of the last event processed also works, but can stay behind a long run of
others' events.)

## 4. Reporting a payment

```http
POST /v1/payment-attestations
Kippu-Signature: …

{ "attestation_id": "pi_3Nk…", "outcome": "paid", "reservation_id": "…",
  "amount": { "amount_minor": 6000, "currency": "JPY" }, "occurred_at": "…" }
```

- `attestation_id` is the attestor's own reference for the payment, unique per attestor.
- **Retry until you get a 2xx.** Delivering the same `attestation_id` again is always safe:
  it is recorded once, and every delivery receives the same answer (`"replayed": true` after
  the first).
- `amount` must equal the reservation total exactly, or the request fails with 422 and nothing
  is recorded.

The response says what became of the money:

| `disposition` | Meaning | What the attestor does |
|---|---|---|
| `applied` | Tickets were issued, in the same transaction that recorded the payment. | Nothing. |
| `refund_required` | The money cannot be used: the reservation lapsed and its tickets sold out, or the reservation was already paid. | Refund the buyer, then report it (§5). |

A 200 with `refund_required` is a successful delivery: stop retrying. Kippu also records a
`refund.required` event in the feed.

Kippu guarantees there is no third outcome: a recorded payment always ends in issued tickets
or an explicit refund obligation.

## 5. Confirming a refund of an unusable payment

```http
POST /v1/payment-attestations
Kippu-Signature: …

{ "attestation_id": "pi_3Nk…", "outcome": "refunded" }
```

Marks the payment `refunded` (and the reservation, if it held no tickets, `refunded`).
Idempotent. A payment whose disposition is `applied` cannot be marked refunded this way: its
tickets are refunded one refund at a time (§6).

## 6. Refunds of issued tickets

Refunds are asked of Kippu, never of the attestor: the buyer (within the refund period of the
ticket type, `refundable_until`) or an organizer calls `POST /v1/reservations/{id}/refunds`. In
the same transaction Kippu revokes the tickets — gates refuse them from then on — returns their
stock to the sale and records a `refund.required` event that names the refund:

```json
{ "topic": "refund.required", "reservation_id": "…", "attestor_id": "…",
  "attestation_id": "pi_3Nk…", "amount": { "amount_minor": 3000, "currency": "JPY" },
  "refund_id": "0199…" }
```

`amount` is what the refunded tickets were bought for; a reservation can be refunded in several
parts, each with its own `refund_id`. Return `amount` of payment `attestation_id` to the buyer,
then confirm with the refund's id:

```http
POST /v1/payment-attestations
Kippu-Signature: …

{ "attestation_id": "pi_3Nk…", "outcome": "refunded", "refund_id": "0199…" }
```

The response is the usual settlement, with the refund (`"status": "completed"`) under `refund`.
**Retry until you get a 2xx**; confirming again answers `"replayed": true`. A `refund_id` that is
not one of this payment's refunds answers 404.

Revoking first means the worst case is money that arrives late, never money returned for a
ticket that still works. The tickets stay revoked whatever happens to the money: a refund that
cannot be paid out is the deployment's to settle by other means, never by restoring tickets.

## 7. Reporting a reversal

The whole payment can go back without Kippu asking: a chargeback, or a full refund made in the
provider's dashboard. Report it, so that the tickets it paid for stop working:

```http
POST /v1/payment-attestations
Kippu-Signature: …

{ "attestation_id": "pi_3Nk…", "outcome": "reversed" }
```

Every ticket of the payment still valid is revoked and its stock returned, and Kippu records a
completed refund with `"reason": "reversal"` (returned under `refund`). Refunds of the payment
still `pending` are completed too: their money went back with the payment, so do not pay out
their `refund.required`. Idempotent: reporting it again answers `"replayed": true` and changes
nothing. For a payment that paid for no tickets (`refund_required`), a reversal is the same as
confirming its refund (§5).

A partial refund is never a reversal. Money returned for some of the tickets is asked of Kippu
(`POST /v1/reservations/{id}/refunds`, by an organizer), which revokes exactly those tickets and
sends the matching `refund.required` (§6).

## 8. Built-in attestors

| Id | Name | Used for |
|---|---|---|
| `00000000-0000-0000-0000-000000000001` | Free tickets | Reservations totalling zero are settled at checkout; their refunds complete at once. |
| `00000000-0000-0000-0000-000000000002` | Manual | Organizers record in-person payments with `POST /v1/reservations/{id}/manual-payment`, and confirm handing cash back with `POST /v1/refunds/{id}/manual-confirmation`. |

Built-in attestors have no keys; nobody can sign as them.
