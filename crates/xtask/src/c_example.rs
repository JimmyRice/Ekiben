//! Builds the C ABI exactly as it ships (no `std`, `release-small`), checks that the optional
//! Base45 decoder is not in it, compiles `crates/ffi/c/examples/verify.c` against it and runs
//! every test vector through the resulting program.

use std::process::Command;

use serde_json::Value;

use crate::{Result, run_command, workspace_root};

pub(crate) fn run() -> Result {
    let root = workspace_root();
    run_command(Command::new(env!("CARGO")).current_dir(&root).args([
        "build",
        "--package",
        "kaisatsu-ffi",
        "--no-default-features",
        "--profile",
        "release-small",
    ]))?;

    let library = root.join("target/release-small/libkaisatsu.a");
    let contents = std::fs::read(&library)?;
    if contents
        .windows(b"base45".len())
        .any(|window| window.eq_ignore_ascii_case(b"base45"))
    {
        return Err("the default build of kaisatsu-ffi contains Base45 code".into());
    }

    let out_dir = root.join("target/c-example");
    std::fs::create_dir_all(&out_dir)?;
    let program = out_dir.join("verify");
    let mut compile = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".to_owned()));
    compile
        .arg("-std=c11")
        .args(["-Wall", "-Wextra", "-Werror", "-O2"])
        .arg("-I")
        .arg(root.join("crates/ffi/c/include"))
        .arg(root.join("crates/ffi/c/examples/verify.c"))
        .arg(&library)
        .arg("-o")
        .arg(&program);
    if cfg!(target_os = "linux") {
        compile.args(["-lpthread", "-ldl", "-lm"]);
    }
    run_command(&mut compile)?;

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
        let expect = vector["expect"].as_str().unwrap_or("?");
        let output = Command::new(&program)
            .arg(trusted_key)
            .arg(vector["ticket"].as_str().unwrap_or_default())
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let passed = if expect == "Ok" {
            output.status.success()
        } else {
            !output.status.success() && stdout.contains(&format!("🔔 {expect}"))
        };
        println!(
            "{} {name}: {}",
            if passed { "✓" } else { "✗" },
            stdout.trim()
        );
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
