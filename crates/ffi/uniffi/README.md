# kaisatsu-uniffi

[UniFFI](https://mozilla.github.io/uniffi-rs) bindings for [Kaisatsu](../../kaisatsu): the same
ticket check as the [C ABI](../c), as idiomatic Swift, Kotlin and C#.

One object, one call: build a `Verifier` once with the trusted public keys, then call
`verify(ticket, now)` for every scan. It checks the signature and the validity window and
returns the claims, or throws a `KaisatsuError` (`BadSignature`, `Expired`, …).

```swift
let verifier = try Verifier(publicKeys: [key])          // 32-byte Ed25519 keys
let ticket = try verifier.verify(ticket: scanned, now: UInt64(Date().timeIntervalSince1970))
print(ticket.ticketId)
```

Ids are lowercase hyphenated UUID strings, the key id is hex, times are Unix seconds.

`cargo xtask uniffi` builds the library and generates the sources. The bindings are generated
from the compiled library (no UDL file), so the API is whatever this crate exports.
