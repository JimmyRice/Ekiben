//! `cargo xtask uniffi`: the `UniFFI` bindings.
//!
//! | Command | What it does |
//! |---|---|
//! | `uniffi` | Build the library, generate the sources, run every vector through the Swift example. |
//! | `uniffi swift-package` | Build `kaisatsu_uniffiFFI.xcframework` and a Swift package around it. |
//! | `uniffi kotlin` | Run the JVM tests, build the jar, and the AAR when the Android NDK is found. |
//! | `uniffi nuget` | Pack the C# bindings and the native libraries built so far into a `.nupkg`. |
//!
//! - `generate.rs`: build and generate; `command.rs`: dispatch.
//! - `swift_example.rs`, `swift_package.rs`, `kotlin.rs`, `nuget.rs`: one per target.

mod command;
mod generate;
mod kotlin;
mod nuget;
mod swift_example;
mod swift_package;

pub(crate) use command::run;
