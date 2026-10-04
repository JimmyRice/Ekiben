# Kaisatsu (改札)

Minimal verification of tickets issued by Kippu. Kaisatsu answers exactly one question —
*was this ticket signed by a key I trust?* — and leaves every other policy (re-entry, gates,
zones, time windows on devices without a clock) to the application that embeds it:

```text
minimal gate        = kaisatsu
your gate           = kaisatsu + your rules (re-use detection, zones, staff overrides, …)
```

- `#![no_std]`, no allocation, no `unsafe`, no panics: runs the same on a phone, a desktop
  and a microcontroller. The returned ticket borrows from the scanned bytes.
- One dependency tree: `ed25519-dalek` and `sha2`.
- Header first, signature second, claims last: nothing beyond the fixed-size header is
  interpreted before it is known to be authentic.

```rust,no_run
use kaisatsu::{Pinpon, TrustedKey, Verifier};

# fn scan() -> &'static [u8] { &[] }
# fn main() -> Result<(), Box<dyn core::error::Error>> {
let key = TrustedKey::from_bytes(&[0x3d; 32])?; // from /.well-known/kippu/ticket-keys
let verifier = Verifier::new(key);

match verifier.verify(scan()) {
    Ok(ticket) => println!("welcome, ticket {}", ticket.ticket_id()),
    Err(pinpon) => println!("{pinpon}"), // ピンポーン🔔 BadSignature
}
# Ok(()) }
```

## Features

| Feature | Enables |
|---|---|
| *(none)* | Verification and Base45 decoding into a caller-provided buffer. |
| `alloc` | `KeyRing` for `Vec<TrustedKey>`, `base45::encode`. |
| `issuer` | `Issuer` and `Claims`: encoding and signing, as used by the Kippu backend. |

The wire format is specified in [`spec/ticket-protocol.md`](../spec/ticket-protocol.md).
