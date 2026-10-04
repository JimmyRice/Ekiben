//! The `kippu` binary: Kippu with every built-in module and the adapters selected by features.

use std::process::ExitCode;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    kippu_server::Launcher::new().run()
}
