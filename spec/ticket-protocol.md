# KP1 — Kippu Ticket Protocol, version 1

Status: draft. This document is normative; the reference implementation is the
[`kaisatsu`](../kaisatsu) crate, and [`test-vectors/v1`](test-vectors/v1) is authoritative where
the two disagree with this text.

The key words MUST, MUST NOT, SHOULD and MAY are to be interpreted as in RFC 2119.

## 1. Goals

- **Offline verification.** A gate with no network can decide whether a ticket was issued by a
  trusted Kippu deployment.
- **Small.** About 180 bytes for a typical ticket, so it fits a low-density QR code.
- **Trivial to parse.** A fixed header, then tag-length-value claims in a canonical order. A
  verifier needs no general-purpose serialization library and no heap.
- **Minimal policy.** The protocol proves *authenticity*. Re-use detection, zone checks and
  similar rules belong to the application.

## 2. Structure

```text
Ticket    = Header ‖ Claims ‖ Signature

Header    = magic ‖ version ‖ algorithm ‖ key_id                 12 bytes
  magic     = 0x4B 0x50 ("KP")                                    2 bytes
  version   = 0x01                                                1 byte
  algorithm = 0x01 (Ed25519)                                      1 byte
  key_id    = SHA-512(public_key)[0..8]                           8 bytes

Claims    = Claim*
Claim     = tag ‖ length ‖ value
  tag       = u8
  length    = unsigned LEB128, 1 or 2 bytes, shortest form
  value     = length bytes

Signature = Ed25519(secret_key, Header ‖ Claims)                 64 bytes
```

A ticket MUST NOT exceed 2048 bytes.

### 2.1 Lengths

A length below 128 is one byte. A length from 128 to 16383 is two bytes: the low seven bits
with the high bit set, then the remaining bits. Verifiers MUST reject a two-byte length whose
second byte is zero (it should have been one byte) or has its high bit set (it would need a
third byte).

### 2.2 Ordering

Claim tags MUST be strictly ascending. This makes the encoding of a given set of claims unique
and lets verifiers parse in a single pass.

### 2.3 Signature

The signature covers every byte before it: `Header ‖ Claims`. Verifiers MUST use strict Ed25519
verification (rejecting non-canonical `S`, small-order `R` and small-order public keys).
Because the signed bytes always begin with `"KP"` and a version, a ticket signing key MUST NOT
be used to sign anything else.

## 3. Claims

| Tag | Name | Value | Required |
|---|---|---|---|
| `0x01` | `issuer` | UTF-8, 1–64 bytes, e.g. `kippu.example.org` | yes |
| `0x02` | `event_id` | 16-byte UUID | yes |
| `0x03` | `ticket_id` | 16-byte UUID, unique per ticket | yes |
| `0x04` | `ticket_type_id` | 16-byte UUID | yes |
| `0x05` | `valid_from` | `u64` big-endian Unix seconds, inclusive | yes |
| `0x06` | `valid_until` | `u64` big-endian Unix seconds, exclusive; MUST be greater than `valid_from` | yes |
| `0x07` | `issued_at` | `u64` big-endian Unix seconds | yes |
| `0x08`–`0x3F` | reserved, critical | — | — |
| `0x40`–`0x7F` | reserved, non-critical | — | — |
| `0x80`–`0xFF` | extensions | application-defined bytes | no |

- An unknown tag in `0x08`–`0x3F` MUST cause rejection: a future revision may define claims
  there that change the meaning of the ticket.
- An unknown tag in `0x40`–`0x7F` MUST be ignored: a future revision may add informational
  claims there without breaking existing verifiers.
- Extensions MUST be returned to the application verbatim. Kippu deployments define their own
  meanings (seat, hall, day of a multi-day event, …).

A ticket is valid at time `t` when `valid_from ≤ t < valid_until`. Checking this is OPTIONAL
for verifiers, since not every device has a trustworthy clock.

## 4. Verification

A verifier MUST perform these steps in order, and MUST NOT interpret claims before step 5:

1. Reject tickets shorter than 76 bytes (header + signature) or longer than 2048 bytes.
2. Check `magic`, then `version`, then `algorithm`.
3. Look up the trusted public key whose key id equals `key_id`. Reject if there is none.
4. Verify the signature over `Header ‖ Claims` with that key.
5. Parse the claims, enforcing §2.1, §2.2 and §3.

Kaisatsu reports failures as `Pinpon`: `Malformed`, `UnsupportedVersion`,
`UnsupportedAlgorithm`, `UnknownKey`, `BadSignature`, and — from the optional time check —
`NotYetValid` and `Expired`.

## 5. Text encoding

In QR codes and other text channels a ticket is encoded with Base45 (RFC 9285). Base45's
alphabet is exactly the QR alphanumeric character set, so the code stays almost as dense as a
binary one, while surviving scanner SDKs that only return strings.

## 6. Keys

Kippu publishes its ticket keys at `GET /.well-known/kippu/ticket-keys`. Retired keys stay
published until every ticket they signed has expired, so verifiers can keep a key ring that
covers rotation.

## 7. Test vectors

[`test-vectors/v1/vectors.json`](test-vectors/v1/vectors.json) lists keys (marked trusted or
not) and tickets, each with its hex and Base45 form and the expected outcome (`"Ok"` or a
`Pinpon` name). Valid tickets include their decoded claims and time checks. Every
implementation SHOULD run all of them. Regenerate the file with `cargo xtask vectors`.
