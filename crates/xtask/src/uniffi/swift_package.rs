//! Swift: builds the static library for iOS, the iOS simulator and macOS, wraps them in
//! `kaisatsu_uniffiFFI.xcframework` and puts the generated Swift next to it as a Swift package
//! in `target/uniffi/swift-package`, then compiles the package for macOS and iOS.

use std::path::Path;
use std::process::Command;

use crate::{Result, run_command};

use super::generate::{Generated, PROFILE};

/// iOS device, iOS simulator, macOS (Apple silicon, Intel).
const TARGETS: [&str; 4] = [
    "aarch64-apple-ios",
    "aarch64-apple-ios-sim",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
];

pub(crate) fn run(generated: &Generated) -> Result {
    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()?;
    let installed = String::from_utf8_lossy(&installed.stdout);
    let missing: Vec<_> = TARGETS
        .iter()
        .filter(|target| !installed.lines().any(|line| line == **target))
        .collect();
    if !missing.is_empty() {
        return Err(format!("run `rustup target add {}` first", join(&missing)).into());
    }

    let package = generated.out_dir.join("swift-package");
    let _ = std::fs::remove_dir_all(&package);
    let sources = package.join("Sources/Kaisatsu");
    std::fs::create_dir_all(&sources)?;
    std::fs::copy(
        generated.root.join("crates/ffi/uniffi/swift/Package.swift"),
        package.join("Package.swift"),
    )?;
    let swift = generated.out_dir.join("swift");
    std::fs::copy(
        swift.join("kaisatsu_uniffi.swift"),
        sources.join("kaisatsu_uniffi.swift"),
    )?;

    // The headers an XCFramework carries must name the module map `module.modulemap`.
    let headers = generated.out_dir.join("swift-headers");
    let _ = std::fs::remove_dir_all(&headers);
    std::fs::create_dir_all(&headers)?;
    std::fs::copy(
        swift.join("kaisatsu_uniffiFFI.h"),
        headers.join("kaisatsu_uniffiFFI.h"),
    )?;
    std::fs::copy(
        swift.join("kaisatsu_uniffiFFI.modulemap"),
        headers.join("module.modulemap"),
    )?;

    for target in TARGETS {
        run_command(
            Command::new(env!("CARGO"))
                .current_dir(&generated.root)
                .args([
                    "build",
                    "--package",
                    "kaisatsu-uniffi",
                    "--profile",
                    PROFILE,
                ])
                .args(["--target", target]),
        )?;
    }
    let archive = |target: &str| {
        generated
            .root
            .join("target")
            .join(target)
            .join(PROFILE)
            .join("libkaisatsu_uniffi.a")
    };

    // One macOS library for both architectures.
    let macos = generated.out_dir.join("libkaisatsu_uniffi-macos.a");
    run_command(
        Command::new("lipo")
            .arg("-create")
            .arg(archive("aarch64-apple-darwin"))
            .arg(archive("x86_64-apple-darwin"))
            .arg("-output")
            .arg(&macos),
    )?;

    let mut create = Command::new("xcodebuild");
    create.arg("-create-xcframework");
    for library in [
        archive("aarch64-apple-ios"),
        archive("aarch64-apple-ios-sim"),
        macos,
    ] {
        create
            .arg("-library")
            .arg(library)
            .arg("-headers")
            .arg(&headers);
    }
    create
        .arg("-output")
        .arg(package.join("kaisatsu_uniffiFFI.xcframework"));
    run_command(&mut create)?;

    build_package(&package)?;
    println!("Swift package: {}", package.display());
    Ok(())
}

/// Compiles the package for macOS and for iOS devices.
fn build_package(package: &Path) -> Result {
    run_command(Command::new("swift").current_dir(package).arg("build"))?;
    run_command(
        Command::new("xcodebuild")
            .current_dir(package)
            .args(["build", "-quiet", "-scheme", "Kaisatsu"])
            .args(["-destination", "generic/platform=iOS"])
            .args(["-derivedDataPath", ".build/xcode"]),
    )
}

fn join(targets: &[&&str]) -> String {
    targets
        .iter()
        .map(|target| **target)
        .collect::<Vec<_>>()
        .join(" ")
}
