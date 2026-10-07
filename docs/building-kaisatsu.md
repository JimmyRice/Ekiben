# Building Kaisatsu for each platform

Kaisatsu (the ticket verifier) ships in two forms. Pick by who calls it:

| You are writing… | Use | Why |
|---|---|---|
| C, C++, Zig, Go, firmware | the **C ABI** (`crates/ffi/c`) | no `std`, no allocation, nothing to free: **65 KiB** shared library |
| Swift, Kotlin/Android, C# | **UniFFI** (`crates/ffi/uniffi`) | idiomatic API, generated: one `Verifier`, one `verify(ticket, now)` |
| Rust | the `kaisatsu` crate directly | |

UniFFI needs `std`, so its libraries are bigger (**~300–490 KiB** per platform). C and C++ stay
on the C ABI for that reason: the C++ generator for UniFFI also lags the C# one by two versions,
so the two could not share a library anyway. See "Why the sizes are what they are" below.

All commands run from the repository root. Always build with `--profile release-small`
(size-optimized, `panic = "abort"`, symbols stripped): `cargo build --release` is tuned for the
server and gives bigger libraries.

## C ABI

```bash
cargo build -p kaisatsu-ffi --no-default-features --profile release-small
```

Produces `target/release-small/libkaisatsu.a` and the shared library (`.dylib`, `.so`, `.dll`).
`--no-default-features` drops `std`, which is what keeps it small. The header is
`crates/ffi/c/include/kaisatsu.h`, generated and checked in (`cargo xtask header`).

For another platform, add `--target <triple>` (after `rustup target add <triple>`):

| Platform | Target triple | Notes |
|---|---|---|
| Linux x86-64 / arm64 | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | build on Linux, or cross-compile with a linker for the target |
| Windows | `x86_64-pc-windows-msvc` | build on Windows |
| iOS | `aarch64-apple-ios`, `aarch64-apple-ios-sim` | link the `.a` |
| Android | `aarch64-linux-android`, … | `cargo ndk -t arm64-v8a build -p kaisatsu-ffi --no-default-features --profile release-small` |
| Cortex-M4F/M7F | `thumbv7em-none-eabihf` | link the `.a`; `CARGO_PROFILE_RELEASE_SMALL_OPT_LEVEL=z` trades speed for size |

Only the host (macOS) build has been measured: 65 KiB. A static archive (`.a`) holds every
object file, so its size says nothing; the linker drops what the application does not use.
Measure the shared library, or the final application. `cargo xtask size` prints the numbers.

Base45 text decoding is off by default; `--features base45` adds it (define `KAISATSU_BASE45`
before including the header). `cargo xtask c-example` builds the C example and runs all shared
test vectors through it; it also fails if Base45 code has leaked into the default build.

## Swift, Kotlin and C# (UniFFI)

One-time setup:

```bash
# Swift: the Apple targets
rustup target add aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin x86_64-apple-darwin
# Kotlin/Android: the Android targets, cargo-ndk, and an NDK from the Android SDK Manager
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
cargo install cargo-ndk --locked
# C#: the binding generator, at the tag built on the same UniFFI version as this repository
cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.11.0+v0.31.0 --locked
```

Then, one command per target. Each regenerates the bindings first, so the sources always match
the library they ship with:

```bash
cargo xtask uniffi                 # generate everything; run all test vectors through Swift
cargo xtask uniffi swift-package   # Swift package around an XCFramework (iOS, simulator, macOS)
cargo xtask uniffi kotlin          # JVM tests + jar, and the Android AAR
cargo xtask uniffi nuget           # NuGet package
```

| Target | Output | Size (measured) |
|---|---|---|
| Swift | `target/uniffi/swift-package/` | a gate app linking it is 673 KiB, mostly Swift runtime glue |
| Kotlin JVM | `crates/ffi/uniffi/kotlin/jvm/build/libs/jvm-0.1.0.jar` | host library only (365 KiB on macOS arm64) |
| Android | `crates/ffi/uniffi/kotlin/android/build/outputs/aar/android-release.aar` | 956 KiB for four ABIs; per ABI: arm64-v8a 411, armeabi-v7a 300, x86_64 462, x86 488 KiB |
| C# | `target/uniffi/nupkg/Ekiben.Kaisatsu.0.1.0.nupkg` | 374 KiB (osx-arm64) and 388 KiB (osx-x64) of native code |

Notes per target:

- **Swift** needs macOS and Xcode. The package is not published: add `target/uniffi/swift-package`
  as a local package and `import Kaisatsu`.
- **Kotlin** needs a JDK 17+. The AAR step needs `ANDROID_HOME` (or `ANDROID_SDK_ROOT`) with an
  NDK installed and `cargo-ndk`; without them the command stops after the jar and says what is
  missing. The generated code loads the library through JNA, so apps add
  `net.java.dev.jna:jna:5.19.1@aar` next to the AAR (the jar declares it itself). The jar
  bundles only the library of the machine that built it, under JNA's `<os>-<arch>/` name.
- **C#** does not need to be uploaded anywhere: put the `.nupkg` in a folder, `dotnet nuget add
  source <folder>`, then `dotnet add package Ekiben.Kaisatsu`. After changing a package without
  bumping its version, delete `~/.nuget/packages/ekiben.kaisatsu`. A package carries one native
  library per runtime id; `cargo xtask uniffi nuget` builds those this machine can target
  (macOS: `osx-arm64`, `osx-x64`). For Windows and Linux, run
  `cargo build -p kaisatsu-uniffi --profile release-small` on that system, copy the library to
  `target/uniffi/nuget-natives/<rid>/native/` (for example `win-x64`, `linux-x64`) and run the
  command again; existing files there are kept.

Only the library alone, without bindings: `cargo build -p kaisatsu-uniffi --profile release-small`
(add `--target …`, or use `cargo ndk` for Android).

The `uniffi` dependency is pinned (`=0.31.0`) to the version `uniffi-bindgen-cs` is built on.
Never mix generated sources with a library from another build.

## Why the sizes are what they are

- The C ABI is `no_std`: Ed25519 and SHA-512 are nearly all of its 65 KiB.
- UniFFI libraries carry Rust's standard library (formatting, allocation, threading), roughly
  200 KiB, plus the UniFFI runtime. A stable toolchain cannot drop that part.
- Raising the optimization level of only the crypto crates did nothing: `release-small` uses
  fat LTO, which re-optimizes everything at the profile's level, and the size did not change by
  a byte. Verification takes about 40 µs per ticket through Swift, almost all of it Ed25519
  itself, so there is no speed to buy by calling less often either.
- Smaller still would need a nightly toolchain with `-Z build-std` and
  `panic_immediate_abort`. Nothing here depends on that, and it has not been tried.

## Checking your build

```bash
cargo xtask size              # sizes of the C and UniFFI libraries
cargo xtask c-example         # C ABI against all shared test vectors
cargo xtask uniffi            # UniFFI against all shared test vectors, through Swift
cargo test -p kaisatsu-uniffi # the same vectors from Rust
```

The Kotlin build runs the vectors on the JVM too (`cargo xtask uniffi kotlin`). The AAR is only
built, not run, so try it on a device or an emulator before shipping.
