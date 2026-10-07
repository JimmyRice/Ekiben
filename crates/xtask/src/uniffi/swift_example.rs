//! Compiles `crates/ffi/uniffi/examples/verify.swift` against the generated Swift bindings and
//! runs every test vector, including its time checks, through it.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::{Result, run_command};

pub(crate) fn run(root: &Path, out_dir: &Path, library_dir: &Path) -> Result {
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
