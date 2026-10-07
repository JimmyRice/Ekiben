# Kippu Attestor Protocol, version 1

Status: draft. Kippu never talks to a payment provider. Instead, a deployment runs one or more
**attestors** — small services that wrap Stripe, Alipay, PayPay, a cash desk, anything — and
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

An attestor reads its events with `GET /v1/attestor/feed?after=<sequence>` (signed), or looks
a reservation up directly with `GET /v1/attestor/reservations/{id}` (signed) — for example
when the buyer's browser hands it a reservation id. **The amount to charge always comes from
Kippu**, never from the buyer.

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

## 5. Confirming a refund

```http
POST /v1/payment-attestations
Kippu-Signature: …

{ "attestation_id": "pi_3Nk…", "outcome": "refunded" }
```

Marks the payment `refunded` (and the reservation, if it held no tickets, `refunded`).
Idempotent. A payment whose disposition is `applied` cannot be marked refunded.

## 6. Built-in attestors

| Id | Name | Used for |
|---|---|---|
| `00000000-0000-0000-0000-000000000001` | Free tickets | Reservations totalling zero are settled at checkout. |
| `00000000-0000-0000-0000-000000000002` | Manual | Organizers record in-person payments with `POST /v1/reservations/{id}/manual-payment`. |

Built-in attestors have no keys; nobody can sign as them.
