# Kippu Webhook Protocol, version 1

Status: draft. Kippu records every change other systems may react to — a payment to collect,
tickets issued, a refund owed, a reservation lapsed — as an integration event in an outbox,
in the same transaction as the change. Integrators can read the outbox as a feed, or register
a **webhook** and have Kippu POST each event to them. This document specifies the webhook
side.

## 1. Registering a webhook

An organizer registers a webhook for an organization and receives only the events of that
organization's events; an admin registers a global one and receives everything:

```http
POST /v1/organizations/{organization_id}/webhooks
POST /v1/admin/webhooks
{ "url": "https://integrator.example/kippu", "topics": ["tickets.issued"] }
```

- `topics` limits what is delivered; empty or absent means every topic:
  `payment.requested`, `tickets.issued`, `refund.required`, `reservation.expired`.
- A new webhook receives events recorded from then on, not earlier ones (read the feed for
  those).
- `url` must use `https` and must not point to a loopback, private, link-local or otherwise
  non-public address, neither literally nor through DNS, unless the deployment allows it
  (`webhooks.allow_http`, `webhooks.allow_private_networks`) — typically only in development.
- `PATCH /v1/webhooks/{id}` changes `url` or `topics`, or pauses (`"active": false`) and
  resumes delivery; a paused webhook resumes where it stopped. `DELETE` removes it.
  `GET /v1/webhooks/{id}` shows progress: `delivered_through`, `failures`, `last_error`,
  `next_attempt_at`.

Webhooks require the deployment to configure a webhook signing key; without one the
endpoints answer `501 webhooks-not-configured`.

## 2. Deliveries

Each event is one request:

```http
POST <url>
Content-Type: application/json
Kippu-Signature: key=<key id>, ts=<unix seconds>, sig=<base64url, no padding>
Kippu-Delivery: <webhook id>:<sequence>

{ "webhook_id": "…", "sequence": 42, "created_at": "2027-08-16T01:00:00Z",
  "event": { "topic": "tickets.issued", "reservation_id": "…", "account_id": "…",
             "ticket_ids": ["…"] } }
```

`event` is the integration event as the feed returns it, tagged by `topic`; `sequence` is
its position in the outbox, increasing with commit order.

- **Success** is any `2xx` answer within the timeout (10 seconds by default). Redirects are
  not followed and count as failures.
- **Order.** Events reach a webhook in sequence order. After a failure nothing later is sent
  until the failed event is accepted.
- **At least once.** An event may be delivered more than once (a timeout after the receiver
  processed it, a worker that died mid-delivery). Deduplicate by `Kippu-Delivery`, or by
  `sequence` per webhook.
- **Retries** wait 2ⁿ seconds after the n-th consecutive failure, at most an hour, without
  giving up. A webhook that keeps failing shows it in `failures` and `last_error`.

## 3. Verifying a delivery

`sig` is an Ed25519 signature over these four lines, joined by `\n`:

```text
kippu-webhook-v1
<the webhook's URL, exactly as registered>
<ts>
<lowercase hex SHA-256 of the request body>
```

The URL binds the signature to the endpoint: a delivery cannot be replayed to another one.
A receiver:

1. looks the key up by `key` in `GET /.well-known/kippu/webhook-keys` (cache it; fetch again
   when an unknown key id appears, which is how keys rotate);
2. verifies `sig` over the raw body, before parsing it;
3. rejects `ts` more than five minutes from its clock;
4. ignores a `Kippu-Delivery` it has already processed.

```http
GET /.well-known/kippu/webhook-keys

{ "keys": [{ "key_id": "0a3d37cb8e554741", "algorithm": "Ed25519",
             "public_key": "<32 bytes, base64>" }] }
```

The key id is the first eight bytes of SHA-512 of the public key, in hex — derived the same
way as ticket key ids. Rust receivers can use `kippu_core::modules::webhooks::signature::verify`.
