//! What happens on a panic without the standard library.

/// Bare-metal builds have no standard library to provide a panic handler. Kaisatsu is written
/// to never panic, so this is never reached; halting is the safest possible behaviour.
#[cfg(all(not(feature = "std"), not(test)))]
#[panic_handler]
fn halt(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
