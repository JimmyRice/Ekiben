//! Builds the `UniFFI` library as it ships (`release-small`), generates the Swift and Kotlin
//! sources into `target/uniffi/` (and C# when `uniffi-bindgen-cs` is installed), then compiles
//! `crates/ffi/uniffi/examples/verify.swift` against the Swift bindings and runs every test
//! vector, including its time checks, through it.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::{Result, run_command, workspace_root};

const PROFILE: &str = "release-small";

pub(crate) fn run() -> Result {
    let root = workspace_root();
    let cargo = |extra: &[&str]| {
        run_command(
            Command::new(env!("CARGO"))
                .current_dir(&root)
                .args([
                    "build",
                    "--package",
                    "kaisatsu-uniffi",
                    "--profile",
                    PROFILE,
                ])
                .args(extra),
        )
    };
    cargo(&[])?;
    cargo(&["--features", "bindgen", "--bin", "uniffi-bindgen"])?;

    let profile_dir = root.join("target").join(PROFILE);
    let library = [
        "libkaisatsu_uniffi.dylib",
        "libkaisatsu_uniffi.so",
        "kaisatsu_uniffi.dll",
    ]
    .map(|name| profile_dir.join(name))
    .into_iter()
    .find(|path| path.exists())
    .ok_or("the shared library was not built")?;
    let out_dir = root.join("target/uniffi");

    for language in ["swift", "kotlin"] {
        let dir = out_dir.join(language);
        run_command(
            Command::new(profile_dir.join("uniffi-bindgen"))
                .args([
                    "generate",
                    "--no-format",
                    "--language",
                    language,
                    "--library",
                ])
                .arg(&library)
                .arg("--out-dir")
                .arg(&dir),
        )?;
        println!("generated {language} sources in {}", dir.display());
    }
    if Command::new("uniffi-bindgen-cs")
        .arg("--version")
        .output()
        .is_ok()
    {
        let dir = out_dir.join("csharp");
        run_command(
            Command::new("uniffi-bindgen-cs")
                .arg("--library")
                .arg(&library)
                .arg("--out-dir")
                .arg(&dir),
        )?;
        println!("generated csharp sources in {}", dir.display());
    } else {
        println!(
            "(install uniffi-bindgen-cs, tag v0.11.0+v0.31.0 of github.com/NordSecurity/uniffi-bindgen-cs, to generate C#)"
        );
    }

    if Command::new("swiftc").arg("--version").output().is_err() {
        println!("(swiftc not found: skipping the Swift example)");
        return Ok(());
    }
    swift_example(&root, &out_dir, &profile_dir)
}

fn swift_example(root: &Path, out_dir: &Path, library_dir: &Path) -> Result {
    let swift_dir = out_dir.join("swift");
    let example = out_dir.join("swift-example");
    std::fs::create_dir_all(&example)?;
    // A top-level-code file must be called `main.swift`.
    std::fs::copy(
        root.join("crates/ffi/uniffi/examples/verify.swift"),
        example.join("main.swift"),
    )?;
    let program = example.join("verify");
    run_command(
        Command::new("swiftc")
            .arg("-O")
            .arg("-Xcc")
            .arg(format!(
                "-fmodule-map-file={}",
                swift_dir.join("kaisatsu_uniffiFFI.modulemap").display()
            ))
            .arg(swift_dir.join("kaisatsu_uniffi.swift"))
            .arg(example.join("main.swift"))
            .arg("-L")
            .arg(library_dir)
            .arg("-lkaisatsu_uniffi")
            .arg("-Xlinker")
            .arg("-rpath")
            .arg("-Xlinker")
            .arg(library_dir)
            .arg("-o")
            .arg(&program),
    )?;

    let vectors: Value = serde_json::from_str(&std::fs::read_to_string(
        root.join("spec/test-vectors/v1/vectors.json"),
    )?)?;
    let trusted_key = vectors["keys"]
        .as_array()
        .and_then(|keys| keys.iter().find(|key| key["trusted"] == true))
        .and_then(|key| key["public_key"].as_str())
        .ok_or("no trusted key in vectors.json")?;

    let mut failures = 0;
    for vector in vectors["vectors"]
        .as_array()
        .ok_or("no vectors in vectors.json")?
    {
        let name = vector["name"].as_str().unwrap_or("?");
        let ticket = vector["ticket"].as_str().unwrap_or_default();
        let gate = |now: u64| -> Result<(bool, String)> {
            let output = Command::new(&program)
                .arg(trusted_key)
                .arg(ticket)
                .arg(now.to_string())
                .output()?;
            Ok((
                output.status.success(),
                String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            ))
        };
        let expect = vector["expect"].as_str().unwrap_or("?");
        // Tickets are checked at each of their time checks, or else at the start of their
        // validity; rejected ones never get as far as the clock.
        let checks: Vec<(u64, &str)> = match vector["time_checks"].as_array() {
            Some(checks) => checks
                .iter()
                .map(|check| {
                    (
                        check["now"].as_u64().unwrap_or_default(),
                        check["expect"].as_str().unwrap_or("?"),
                    )
                })
                .collect(),
            None => vec![(
                vector["claims"]["valid_from"].as_u64().unwrap_or_default(),
                expect,
            )],
        };
        let mut passed = true;
        let mut shown = String::new();
        for (now, expect) in checks {
            let (ok, stdout) = gate(now)?;
            passed &= if expect == "Ok" {
                ok && stdout.starts_with("OK ticket ")
                    && vector["claims"]["ticket_id"]
                        .as_str()
                        .is_none_or(|id| stdout.contains(id))
            } else {
                !ok && stdout.contains(&format!("🔔 {expect}"))
            };
            shown = stdout;
        }
        println!("{} {name}: {shown}", if passed { "✓" } else { "✗" });
        if !passed {
            failures += 1;
        }
    }
    if failures == 0 {
        Ok(())
    } else {
        Err(format!("{failures} vector(s) failed").into())
    }
}
