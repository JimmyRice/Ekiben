# Walkthrough: from an empty database to a verified ticket

This walks through Kippu by hand: root sets up an organization and an organizer, the organizer
opens a sale, a buyer purchases two tickets and pays through an attestor, and a gate verifies
a ticket offline. API calls are given as plain HTTP; the same requests are in
[`walkthrough.http`](walkthrough.http), which RustRover and IntelliJ run step by step and which
captures every id for you. Shell is used only for what is not HTTP.

Placeholders such as `{{organization_id}}` stand for values returned by an earlier step.

## Prepare

Generate four key pairs. Each command writes `<name>.key` (private, mode 0600) and
`<name>.pub`:

```bash
kippu keygen --out ticket
kippu keygen --out token
kippu keygen --out root
kippu keygen --out attestor
```

`ticket` signs tickets, `token` signs access tokens, `root` proves you are the deployment
owner, and `attestor` plays a payment provider. Write a minimal configuration:

```toml
# kippu.toml
[database]
url = "sqlite://kippu.db?mode=rwc"

[issuer]
id = "kippu.local"
```

Start the server with the keys in the environment. It listens on `0.0.0.0:8080` and logs one
block per request (`--log-format compact` or `json` for other formats):

```bash
export KIPPU_KEYS__TICKET_SIGNING_KEY=$(cat ticket.key)
export KIPPU_KEYS__TOKEN_SIGNING_KEY=$(cat token.key)
export KIPPU_ROOT__KEYS="[{name=\"owner\", public_key=\"$(cat root.pub)\"}]"
kippu serve --config kippu.toml
```

In another terminal, mint a root token. It is valid for 10 minutes; mint a new one when it
expires:

```bash
kippu root-token --config kippu.toml --key root.key --name owner
```

## 1–3. Root creates an organization and its organizer

```http
POST /v1/admin/organizations HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{root_token}}
Content-Type: application/json

{ "slug": "comiket", "name": "Comic Market" }
```

The response's `id` is `{{organization_id}}`.

```http
POST /v1/admin/accounts HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{root_token}}
Content-Type: application/json

{
  "email": "staff@example.org",
  "password": "correct horse battery",
  "display_name": "Staff",
  "role": "organizer"
}
```

The response's `id` is `{{organizer_id}}`. Make the organizer a member (`204 No Content`):

```http
PUT /v1/admin/organizations/{{organization_id}}/members/{{organizer_id}} HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{root_token}}
```

## 4–7. The organizer sets up a sale

```http
POST /v1/auth/login HTTP/1.1
Host: localhost:8080
Content-Type: application/json

{ "email": "staff@example.org", "password": "correct horse battery" }
```

`access_token.token` is `{{organizer_token}}`. Create the event — it starts as a draft.
`venue` is what people call the place; `address` is where it is, precisely enough for a map
app (leave it out for an online event). `content` is the event's page in any form your clients
render (HTML here; Markdown or your own JSON work as well); Kippu stores it as is, and
listings of events leave it out:

```http
POST /v1/organizations/{{organization_id}}/events HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{organizer_token}}
Content-Type: application/json

{
  "slug": "c110",
  "title": "Comic Market 110",
  "venue": "Tokyo Big Sight",
  "address": {
    "country": "JP",
    "region": "東京都",
    "locality": "江東区",
    "postal_code": "135-0063",
    "street": "有明3-11-1",
    "latitude": 35.6298,
    "longitude": 139.7942
  },
  "starts_at": "2027-08-16T10:00:00+09:00",
  "ends_at": "2027-08-17T16:00:00+09:00",
  "content": "<h1>Comic Market 110</h1><p>Two days at Tokyo Big Sight.</p>"
}
```

Open a sale for `{{event_id}}`. Without an `admission` policy anyone may buy right away; see
the OpenAPI document for waiting rooms:

```http
POST /v1/events/{{event_id}}/sales HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{organizer_token}}
Content-Type: application/json

{
  "name": "General",
  "opens_at": "2026-10-01T00:00:00Z",
  "closes_at": "2027-08-01T00:00:00Z"
}
```

Add a ticket type to `{{sale_id}}`. Extension claims (tags 128–255) are signed into every
ticket; here, the entrance:

```http
POST /v1/sales/{{sale_id}}/ticket-types HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{organizer_token}}
Content-Type: application/json

{
  "name": "Day 1",
  "price": { "amount_minor": 2500, "currency": "JPY" },
  "capacity": 100,
  "valid_from": "2027-08-16T08:00:00+09:00",
  "valid_until": "2027-08-17T00:00:00+09:00",
  "ticket_extensions": { "128": "EAST-1" }
}
```

## 8–10. Accept payments and publish

Kippu talks to no payment provider itself. An *attestor* — a service that wraps one — reports
payments signed with a key registered here. Put the contents of `attestor.pub` in
`public_key`:

```http
POST /v1/admin/attestors HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{root_token}}
Content-Type: application/json

{
  "name": "Card gateway",
  "environment": "live",
  "keys": [{ "key_id": "k1", "public_key": "{{attestor_public_key}}" }]
}
```

The organizer accepts `{{attestor_id}}` for the sale. `PATCH` takes only the fields that
change, plus the `version` last read; if someone else edited the sale meanwhile, it fails with
`412` instead of overwriting their change:

```http
PATCH /v1/sales/{{sale_id}} HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{organizer_token}}
Content-Type: application/merge-patch+json

{ "version": 1, "accepted_attestors": ["{{attestor_id}}"] }
```

Publish the event the same way:

```http
PATCH /v1/events/{{event_id}} HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{organizer_token}}
Content-Type: application/merge-patch+json

{ "version": 1, "status": "published" }
```

## 11–16. A buyer purchases two tickets

```http
POST /v1/auth/register HTTP/1.1
Host: localhost:8080
Content-Type: application/json

{ "email": "miku@example.org", "password": "correct horse battery", "display_name": "Miku" }
```

```http
POST /v1/auth/login HTTP/1.1
Host: localhost:8080
Content-Type: application/json

{ "email": "miku@example.org", "password": "correct horse battery" }
```

`access_token.token` is `{{buyer_token}}`. The event is now public. Every listing answers
`{"items": [...], "next_cursor": ...}`; the event listing also filters (`status`, `country`,
`starts_from`, `starts_before`, `ends_from`, `ends_before`) and sorts (`sort=starts_at`, the
default, `-starts_at`, `created_at`, `-created_at`). Events that are on or still to come in
Japan, twenty at a time:

```http
GET /v1/events?country=JP&ends_from=2026-10-01T00:00:00Z&limit=20 HTTP/1.1
Host: localhost:8080
```

While `next_cursor` is not `null`, more follow: send the same request with
`&cursor=<next_cursor>` for the next page. Cursors are opaque and resume exactly where the
page ended, however many events were added meanwhile.

Ask for two tickets. The request is queued durably and answered with `202 Accepted` and a
`Location`; sending it again with the same `Idempotency-Key` returns the same request instead
of buying twice:

```http
POST /v1/sales/{{sale_id}}/purchase-requests HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{buyer_token}}
Idempotency-Key: my-first-order
Content-Type: application/json

{ "items": [{ "ticket_type_id": "{{ticket_type_id}}", "quantity": 2 }] }
```

A background worker reserves the tickets within moments. Poll the `Location` until `status` is
`reserved`; the response then carries `{{reservation_id}}`:

```http
GET /v1/purchase-requests/{{purchase_request_id}} HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{buyer_token}}
```

Choose how to pay. The reservation becomes `payment_pending` and holds the tickets until it
expires:

```http
POST /v1/reservations/{{reservation_id}}/checkout HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{buyer_token}}
Content-Type: application/json

{ "attestor_id": "{{attestor_id}}" }
```

## 17. The attestor reports the payment

A real attestor does this after its provider confirms the payment. The simulator in this
repository signs the attestation with `attestor.key` and prints the `curl` command that
delivers it; the response's `disposition` is `applied` and lists the issued `ticket_ids`:

```bash
cargo run -p kippu-server --example attestor_sim -- \
    --attestor <attestor_id> --key attestor.key --reservation <reservation_id> \
    --amount 5000 --currency JPY | sh
```

## 18–20. Tickets, and a gate

```http
GET /v1/me/tickets HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{buyer_token}}
```

Each ticket in `items` carries `ticket`, the signed ticket in Base64. The same bytes are
available raw, `application/octet-stream`, from the ticket's `/raw` endpoint:

```http
GET /v1/tickets/{{ticket_id}}/raw HTTP/1.1
Host: localhost:8080
Authorization: Bearer {{buyer_token}}
```

How the bytes reach the gate — a binary QR code, Base45 text, the Base64 as is — is up to
the app; see [section 5 of the ticket protocol](../spec/ticket-protocol.md#5-transport-encoding).
A gate needs nothing from the server at the door except, once, the public keys to trust:

```http
GET /.well-known/kippu/ticket-keys HTTP/1.1
Host: localhost:8080
```

`kippu verify` checks a ticket exactly as a gate built on Kaisatsu does. `--key` takes the
`public_key` above or a file holding it, and `--at` pretends it is the day of the event:

```bash
kippu verify --key ticket.pub --issuer kippu.local --at 2027-08-16T10:00:00+09:00 '<ticket>'
```

```text
signature    valid, key 0a3d37cb8e554741
issuer       kippu.local
ticket       01a10d06-ffc8-7717-8d86-0e322f930088
event        01a10d06-f2ef-737c-a65a-522a455cd0f9
ticket type  01a10d06-f306-7294-9f28-1a2220759618
valid        2027-08-15T23:00:00Z until 2027-08-16T15:00:00Z
issued       2026-10-05T17:05:24Z
extension    0x80 = "EAST-1"
result       admitted at 2027-08-16T01:00:00Z
```

Without `--at` the gate refuses the ticket with `ピンポーン🔔 NotYetValid`, and a ticket whose claims
were altered fails with `ピンポーン🔔 BadSignature`.
