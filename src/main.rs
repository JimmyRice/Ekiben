//! The `kippu` binary: every built-in module, plus the storage adapters enabled as features.

use std::process::ExitCode;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    kippu_server::Launcher::new()
        .modules(kippu_core::default_modules())
        .run()
}
