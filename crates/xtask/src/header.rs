//! Generates the C header for `kaisatsu-ffi` with cbindgen.

use crate::{Result, workspace_root, write_or_check};

pub(crate) fn run(check: bool) -> Result {
    let crate_dir = workspace_root().join("crates/ffi/c");
    let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml"))?;
    let bindings = cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()?;
    let mut header = Vec::new();
    bindings.write(&mut header);
    write_or_check(
        &crate_dir.join("include/kaisatsu.h"),
        &String::from_utf8(header)?,
        check,
    )
}
