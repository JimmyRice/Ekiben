//! Builds the `UniFFI` library as it ships (`release-small`) and generates the Swift, Kotlin and
//! C# sources from it into `target/uniffi/` (C# only when `uniffi-bindgen-cs` is installed).

use std::path::PathBuf;
use std::process::Command;

use crate::{Result, run_command, workspace_root};

pub(crate) const PROFILE: &str = "release-small";

/// What [`run`] produced.
pub(crate) struct Generated {
    pub(crate) root: PathBuf,
    /// `target/release-small`.
    pub(crate) profile_dir: PathBuf,
    /// The shared library for the host.
    pub(crate) library: PathBuf,
    /// `target/uniffi`.
    pub(crate) out_dir: PathBuf,
    /// Whether the C# sources were generated.
    pub(crate) csharp: bool,
}

pub(crate) fn run() -> Result<Generated> {
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
    let config = root.join("crates/ffi/uniffi/uniffi.toml");

    for language in ["swift", "kotlin"] {
        let dir = out_dir.join(language);
        // Stale files from an earlier layout (a renamed package) would end up in the build.
        let _ = std::fs::remove_dir_all(&dir);
        run_command(
            Command::new(profile_dir.join("uniffi-bindgen"))
                .args(["generate", "--no-format", "--language", language])
                .arg("--config")
                .arg(&config)
                .arg("--library")
                .arg(&library)
                .arg("--out-dir")
                .arg(&dir),
        )?;
        println!("generated {language} sources in {}", dir.display());
    }

    let csharp = Command::new("uniffi-bindgen-cs")
        .arg("--version")
        .output()
        .is_ok();
    if csharp {
        let dir = out_dir.join("csharp");
        let _ = std::fs::remove_dir_all(&dir);
        run_command(
            Command::new("uniffi-bindgen-cs")
                .arg("--config")
                .arg(&config)
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
    Ok(Generated {
        root,
        profile_dir,
        library,
        out_dir,
        csharp,
    })
}
