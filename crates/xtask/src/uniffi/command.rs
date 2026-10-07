use std::process::Command;

use crate::Result;

use super::{generate, kotlin, nuget, swift_example, swift_package};

/// `cargo xtask uniffi [swift-package|kotlin|nuget]`.
pub(crate) fn run(args: &[String]) -> Result {
    let generated = generate::run()?;
    match args.first().map(String::as_str) {
        None => {
            if Command::new("swiftc").arg("--version").output().is_err() {
                println!("(swiftc not found: skipping the Swift example)");
                return Ok(());
            }
            swift_example::run(&generated.root, &generated.out_dir, &generated.profile_dir)
        }
        Some("swift-package") => swift_package::run(&generated),
        Some("kotlin") => kotlin::run(&generated),
        Some("nuget") => nuget::run(&generated),
        Some(other) => {
            Err(format!("unknown target `{other}`; expected swift-package, kotlin or nuget").into())
        }
    }
}
