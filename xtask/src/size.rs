//! Reports the size of the Kaisatsu C library as it ships: without `std`, built with the
//! `release-small` profile.
//!
//! Static archives contain every object file; the linker drops what an application does not
//! use, so the shared library is the more representative number for apps. Bare-metal sizes
//! are reported when the `thumbv7em-none-eabihf` target is installed.

use std::path::Path;
use std::process::Command;

use crate::{Result, run_command, workspace_root};

const EMBEDDED_TARGET: &str = "thumbv7em-none-eabihf";

pub(crate) fn run() -> Result {
    let root = workspace_root();
    let build = |extra: &[&str]| {
        run_command(
            Command::new(env!("CARGO"))
                .current_dir(&root)
                .args([
                    "build",
                    "--package",
                    "kaisatsu-ffi",
                    "--no-default-features",
                ])
                .args(["--profile", "release-small"])
                .args(extra),
        )
    };

    build(&[])?;
    let host = root.join("target/release-small");
    for file in [
        "libkaisatsu.dylib",
        "libkaisatsu.so",
        "kaisatsu.dll",
        "libkaisatsu.a",
    ] {
        report(&host.join(file));
    }

    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(EMBEDDED_TARGET));
    if installed {
        build(&["--target", EMBEDDED_TARGET])?;
        report(
            &root
                .join("target")
                .join(EMBEDDED_TARGET)
                .join("release-small/libkaisatsu.a"),
        );
    } else {
        println!("(install `{EMBEDDED_TARGET}` with rustup to report bare-metal sizes)");
    }
    Ok(())
}

fn report(path: &Path) {
    if let Ok(metadata) = std::fs::metadata(path) {
        #[expect(clippy::cast_precision_loss, reason = "display only")]
        let kib = metadata.len() as f64 / 1024.0;
        println!("{kib:>10.1} KiB  {}", path.display());
    }
}
