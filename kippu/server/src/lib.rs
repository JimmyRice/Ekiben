#![doc = include_str!("../README.md")]

use std::process::ExitCode;

/// Builds and runs a Kippu instance.
#[derive(Debug, Default)]
pub struct Launcher;

impl Launcher {
    /// Creates a launcher with no modules.
    pub fn new() -> Self {
        Self
    }

    /// Runs the command-line interface and returns the process exit code.
    pub fn run(self) -> ExitCode {
        ExitCode::SUCCESS
    }
}
