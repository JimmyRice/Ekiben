//! C#: builds the native library for every .NET runtime this machine can target, then packs the
//! generated bindings and `target/uniffi/nuget-natives/<rid>/native/*` into a `.nupkg` in
//! `target/uniffi/nupkg`. Libraries for other platforms (built on those platforms) can be put
//! into `nuget-natives` before packing; they are kept.

use std::process::Command;

use crate::{Result, run_command};

use super::generate::{Generated, PROFILE};

/// (Rust target, .NET runtime id, library file name), for the host's operating system.
#[cfg(target_os = "macos")]
const RUNTIMES: &[(&str, &str, &str)] = &[
    (
        "aarch64-apple-darwin",
        "osx-arm64",
        "libkaisatsu_uniffi.dylib",
    ),
    ("x86_64-apple-darwin", "osx-x64", "libkaisatsu_uniffi.dylib"),
];
#[cfg(target_os = "linux")]
const RUNTIMES: &[(&str, &str, &str)] = &[
    (
        "x86_64-unknown-linux-gnu",
        "linux-x64",
        "libkaisatsu_uniffi.so",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "linux-arm64",
        "libkaisatsu_uniffi.so",
    ),
];
#[cfg(target_os = "windows")]
const RUNTIMES: &[(&str, &str, &str)] = &[
    ("x86_64-pc-windows-msvc", "win-x64", "kaisatsu_uniffi.dll"),
    (
        "aarch64-pc-windows-msvc",
        "win-arm64",
        "kaisatsu_uniffi.dll",
    ),
];

pub(crate) fn run(generated: &Generated) -> Result {
    if !generated.csharp {
        return Err("the C# sources were not generated: install uniffi-bindgen-cs".into());
    }
    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()?;
    let installed = String::from_utf8_lossy(&installed.stdout);

    for (target, rid, file) in RUNTIMES {
        if !installed.lines().any(|line| line == *target) {
            println!("(skipping {rid}: `rustup target add {target}` to include it)");
            continue;
        }
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
        let native = generated
            .out_dir
            .join("nuget-natives")
            .join(rid)
            .join("native");
        std::fs::create_dir_all(&native)?;
        std::fs::copy(
            generated
                .root
                .join("target")
                .join(target)
                .join(PROFILE)
                .join(file),
            native.join(file),
        )?;
    }

    let output = generated.out_dir.join("nupkg");
    run_command(
        Command::new("dotnet")
            .arg("pack")
            .arg(
                generated
                    .root
                    .join("crates/ffi/uniffi/csharp/Ekiben.Kaisatsu.csproj"),
            )
            .args(["-c", "Release", "-o"])
            .arg(&output),
    )?;
    println!("NuGet package in {}", output.display());
    Ok(())
}
