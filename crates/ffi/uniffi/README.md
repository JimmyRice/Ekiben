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

## Building

Everything is generated from the compiled library (no UDL file), so the bindings are whatever
this crate exports. Run these from the repository root; outputs land in `target/uniffi/`,
except the Kotlin artifacts, which Gradle builds in this crate's `kotlin/` project.

| Command | Result |
|---|---|
| `cargo xtask uniffi` | the library, the Swift and Kotlin sources (and C# if `uniffi-bindgen-cs` is installed), and every test vector run through `examples/verify.swift` |
| `cargo xtask uniffi swift-package` | `swift-package/`: a Swift package around `kaisatsu_uniffiFFI.xcframework` (iOS, iOS simulator, macOS) |
| `cargo xtask uniffi kotlin` | the JVM tests and `kotlin/jvm/build/libs/jvm-0.1.0.jar`, plus `kotlin/android/build/outputs/aar/android-release.aar` |
| `cargo xtask uniffi nuget` | `nupkg/Ekiben.Kaisatsu.0.1.0.nupkg` |

What each needs on the machine:

- **Swift package**: macOS with Xcode, and `rustup target add aarch64-apple-ios
  aarch64-apple-ios-sim aarch64-apple-darwin x86_64-apple-darwin`.
- **Kotlin jar**: a JDK 17+. It bundles the host's library only; JNA picks it by `<os>-<arch>`.
- **Kotlin AAR**: an Android SDK (`ANDROID_HOME`) with an NDK, `cargo install cargo-ndk`, and
  `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
  i686-linux-android`. Without them the command stops after the jar and says what is missing.
- **NuGet**: the .NET SDK and `cargo install uniffi-bindgen-cs --git
  https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.11.0+v0.31.0`. It packs a native
  library for each runtime this machine can build (on macOS: `osx-arm64`, `osx-x64`); libraries
  built elsewhere can be dropped into `target/uniffi/nuget-natives/<rid>/native/` first.

## Using the results

- **Swift**: add `target/uniffi/swift-package` as a local package dependency and `import Kaisatsu`.
- **Kotlin**: `org.ekiben.kaisatsu.Verifier`. The generated code loads the library through
  [JNA](https://github.com/java-native-access/jna), which both artifacts declare as a dependency.
- **C#**: no need to publish the package. Put the `.nupkg` in a folder and add it as a source
  (`dotnet nuget add source <folder>`), then reference `Ekiben.Kaisatsu`; the namespace is
  `Ekiben.Kaisatsu`.

The `uniffi` version is pinned to the one `uniffi-bindgen-cs` is built on; sources and library
must come from the same build.
