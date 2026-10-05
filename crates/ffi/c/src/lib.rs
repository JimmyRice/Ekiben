#![doc = include_str!("../README.md")]
#![no_std]

#[cfg(feature = "std")]
extern crate std;

mod abi;
mod types;

pub use abi::*;
pub use types::{KaisatsuKey, KaisatsuStatus, KaisatsuTicket};

/// The largest ticket Kaisatsu accepts, in bytes. Size ticket buffers with it.
pub const KAISATSU_MAX_TICKET_LEN: usize = 2048;
const _: () = assert!(KAISATSU_MAX_TICKET_LEN == kaisatsu::wire::MAX_TICKET_LEN);

/// Bare-metal builds have no standard library to provide a panic handler. Kaisatsu is written
/// to never panic, so this is never reached; halting is the safest possible behaviour.
#[cfg(all(not(feature = "std"), not(test)))]
#[panic_handler]
fn halt(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
