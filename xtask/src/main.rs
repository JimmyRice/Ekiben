//! Repository automation, invoked as `cargo xtask <command>`.
//!
//! | Command | What it does |
//! |---|---|
//! | `vectors [--check]` | (Re)generate `spec/test-vectors/v1/vectors.json`. |
//! | `header [--check]` | (Re)generate `ffi/c/include/kaisatsu.h` with cbindgen. |
//! | `c-example` | Build the C ABI, compile `ffi/c/examples/verify.c` and run it on every vector. |
//! | `size` | Report the size of the Kaisatsu C library built with the `release-small` profile. |
//!
//! `--check` fails instead of writing when the generated file is out of date, for CI.

#![allow(clippy::print_stdout, reason = "a command-line tool reports on stdout")]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod c_example;
mod header;
mod size;
mod vectors;

type Result<T = (), E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|arg| arg == "--check");
    let result = match args.first().map(String::as_str) {
        Some("vectors") => vectors::run(check),
        Some("header") => header::run(check),
        Some("c-example") => c_example::run(),
        Some("size") => size::run(),
        _ => {
            eprintln!("usage: cargo xtask <vectors|header|c-example|size> [--check]");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The repository root.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Writes `contents` to `path`, or with `check` verifies that the file already matches.
fn write_or_check(path: &Path, contents: &str, check: bool) -> Result {
    let relative = path
        .strip_prefix(workspace_root())
        .unwrap_or(path)
        .display();
    if check {
        let current = std::fs::read_to_string(path).unwrap_or_default();
        if current != contents {
            return Err(
                format!("{relative} is out of date; run `cargo xtask` to regenerate it").into(),
            );
        }
        println!("{relative} is up to date");
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        println!("wrote {relative}");
    }
    Ok(())
}

/// Runs a command to completion, failing if it exits unsuccessfully.
fn run_command(command: &mut Command) -> Result {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{command:?} failed with {status}").into())
    }
}
